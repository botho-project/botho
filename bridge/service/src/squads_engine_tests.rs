//! Real local-validator engine integration; explicit execution never
//! self-skips.
use crate::{
    attestation::FederationAttestationProvider,
    db::Database,
    engine::OrderProcessor,
    mint::{
        solana::SolMinter,
        squads::{self, state::ProposalState},
        Minter,
    },
    solana_rpc::{HttpSolanaRpc, LegacyMessage, Pubkey, SolanaRpc, Transaction},
};
use bth_bridge_core::{
    sign_attestation_ed25519, AttestationKind, BridgeConfig, BridgeOrder, Chain, OrderStatus,
    SolanaCommitment, SquadsConfig,
};
use ed25519_dalek::{Signer, SigningKey};
use std::{collections::HashMap, sync::Arc, time::Duration};
/// Fault injection changes only transport delivery/response; all chain reads
/// and accepted transactions still go to the real pinned local validator.
struct ControlledRpc {
    inner: HttpSolanaRpc,
    // 1 = accept send then lose response once; 2 = drop all outbound sends.
    send_mode: std::sync::atomic::AtomicU8,
    send_calls: std::sync::atomic::AtomicUsize,
    history_null_once: std::sync::atomic::AtomicBool,
    history_pages: std::sync::atomic::AtomicUsize,
    genesis_override: std::sync::Mutex<Option<String>>,
    lost_responses: std::sync::atomic::AtomicUsize,
    signatures: std::sync::Mutex<Vec<String>>,
    accepted_raw: std::sync::Mutex<Vec<Vec<u8>>>,
    account_overrides: std::sync::Mutex<HashMap<String, Option<crate::solana_rpc::SolanaAccount>>>,
}
impl ControlledRpc {
    fn new(url: String) -> Self {
        Self {
            inner: HttpSolanaRpc::new(url).unwrap(),
            send_mode: 1.into(),
            send_calls: 0.into(),
            history_null_once: false.into(),
            history_pages: 0.into(),
            genesis_override: Default::default(),
            lost_responses: 0.into(),
            signatures: Default::default(),
            accepted_raw: Default::default(),
            account_overrides: Default::default(),
        }
    }
}
#[async_trait::async_trait]
impl SolanaRpc for ControlledRpc {
    async fn genesis_hash(&self) -> Result<String, String> {
        if let Some(hash) = self.genesis_override.lock().unwrap().clone() {
            return Ok(hash);
        }
        self.inner.genesis_hash().await
    }
    async fn history_page(
        &self,
        address: &str,
        before: Option<&str>,
        until: Option<&str>,
        limit: usize,
    ) -> Result<Vec<crate::solana_rpc::history::HistorySignature>, String> {
        self.history_pages
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.history_page(address, before, until, limit).await
    }
    async fn historical_transaction(
        &self,
        signature: &str,
    ) -> Result<Option<crate::solana_rpc::history::HistoricalTransaction>, String> {
        if self
            .history_null_once
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Ok(None);
        }
        self.inner.historical_transaction(signature).await
    }

    async fn get_account_info(
        &self,
        a: &str,
        c: &str,
    ) -> Result<Option<crate::solana_rpc::SolanaAccount>, String> {
        if let Some(value) = self.account_overrides.lock().unwrap().get(a).cloned() {
            return Ok(value);
        }
        self.inner.get_account_info(a, c).await
    }
    async fn get_program_accounts(
        &self,
        p: &str,
        m: &str,
        c: &str,
    ) -> Result<Vec<(Pubkey, crate::solana_rpc::SolanaAccount)>, String> {
        self.inner.get_program_accounts(p, m, c).await
    }
    async fn get_block_height(&self) -> Result<u64, String> {
        self.inner.get_block_height().await
    }
    async fn get_latest_blockhash(&self) -> Result<([u8; 32], u64), String> {
        self.inner.get_latest_blockhash().await
    }
    async fn send_transaction(&self, raw: &[u8]) -> Result<String, String> {
        use std::sync::atomic::Ordering::SeqCst;
        self.send_calls.fetch_add(1, SeqCst);
        if self.send_mode.load(SeqCst) == 2 {
            return Err("injected outbound packet loss".into());
        }
        let signature = self.inner.send_transaction(raw).await?;
        self.signatures.lock().unwrap().push(signature.clone());
        self.accepted_raw.lock().unwrap().push(raw.to_vec());
        if self
            .send_mode
            .compare_exchange(1, 0, SeqCst, SeqCst)
            .is_ok()
        {
            self.lost_responses.fetch_add(1, SeqCst);
            return Err("injected response loss AFTER accepted send".into());
        }
        Ok(signature)
    }
    async fn get_signature_status(
        &self,
        s: &str,
    ) -> Result<crate::solana_rpc::SignatureState, String> {
        self.inner.get_signature_status(s).await
    }
    async fn get_account_data(&self, a: &str, c: &str) -> Result<Option<Vec<u8>>, String> {
        self.inner.get_account_data(a, c).await
    }
    async fn get_signatures_for_address(
        &self,
        a: &str,
        u: Option<&str>,
        c: &str,
    ) -> Result<Vec<(String, u64)>, String> {
        self.inner.get_signatures_for_address(a, u, c).await
    }
    async fn get_transaction_logs(
        &self,
        s: &str,
        c: &str,
    ) -> Result<Option<(Vec<String>, u64)>, String> {
        self.inner.get_transaction_logs(s, c).await
    }
    async fn get_token_supply(&self, m: &str, c: &str) -> Result<u128, String> {
        self.inner.get_token_supply(m, c).await
    }
}
fn key(n: u8) -> SigningKey {
    SigningKey::from_bytes(&[n; 32])
}
fn pk(n: u8) -> Pubkey {
    Pubkey(key(n).verifying_key().to_bytes())
}
fn processor(config: &BridgeConfig, db: &Database, orders: &[BridgeOrder]) -> OrderProcessor {
    processor_with_rpc(config, db, orders, None)
}
fn processor_with_rpc(
    config: &BridgeConfig,
    db: &Database,
    orders: &[BridgeOrder],
    injected: Option<(Arc<dyn SolanaRpc>, u8)>,
) -> OrderProcessor {
    let provider = FederationAttestationProvider::from_config(config)
        .unwrap()
        .unwrap();
    let now = chrono::Utc::now().timestamp() as u64;
    for order in orders {
        for n in [1, 2] {
            let kind = AttestationKind::MintWbth {
                dest_chain: Chain::Solana,
                dest_address: order.dest_address.clone(),
                amount: order.net_amount(),
                order_id: order.id,
                source_tx: order.source_tx.clone().unwrap(),
                safe_nonce: None,
            };
            let envelope = sign_attestation_ed25519(
                &kind,
                &key(n),
                &uuid::Uuid::new_v4().simple().to_string(),
                now,
                now + 120,
            )
            .unwrap();
            let outcome = provider.submit_attestation(&envelope, order);
            assert!(outcome.accepted, "{}", outcome.message);
        }
    }
    let minter = match injected {
        Some((rpc, n)) => SolMinter::with_parts(config.solana.clone(), rpc, Some((key(n), pk(n)))),
        None => SolMinter::new(config.solana.clone()),
    }
    .unwrap()
    .with_store(db.clone());
    let mut minters: HashMap<Chain, Arc<dyn Minter>> = HashMap::new();
    minters.insert(Chain::Solana, Arc::new(minter));
    OrderProcessor::new(
        config.clone(),
        db.clone(),
        minters,
        None,
        Arc::new(provider),
    )
}
async fn proposal(rpc: &HttpSolanaRpc, multisig: Pubkey, index: u64) -> Option<ProposalState> {
    rpc.get_account_info(
        &squads::derive_proposal_pda(&multisig, index)
            .unwrap()
            .to_base58(),
        "finalized",
    )
    .await
    .unwrap()
    .map(|a| ProposalState::parse(&a).unwrap())
}
async fn tick(p: &OrderProcessor) {
    p.process_pending_orders().await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
}
fn context(
    fixture: &serde_json::Value,
    order: &BridgeOrder,
    index: u64,
    member: u8,
) -> squads::SquadsMintContext {
    use crate::mint::solana::{build_bridge_mint_instruction, ORDER_MARKER_SEED};
    let program = Pubkey::from_base58("CZDnzeywrqEM5ereWJmtYKUQ9uJXxX2PydqqKTQStxxE").unwrap();
    let parse = |name: &str| Pubkey::from_base58(fixture[name].as_str().unwrap()).unwrap();
    let bridge = Pubkey::find_program_address(&[b"bridge"], &program)
        .unwrap()
        .0;
    let marker =
        Pubkey::find_program_address(&[ORDER_MARKER_SEED, &order.order_id_bytes()], &program)
            .unwrap()
            .0;
    let inner = build_bridge_mint_instruction(
        program,
        bridge,
        marker,
        parse("mint"),
        parse("ata"),
        parse("recipient"),
        parse("vault"),
        order.net_amount(),
        order.order_id_bytes(),
    );
    squads::SquadsMintContext::resolve(parse("multisig"), 0, index, pk(member), inner).unwrap()
}
fn instruction_tag(name: &str) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    Sha256::digest(format!("global:{name}").as_bytes())[..8].to_vec()
}
async fn send_many(
    rpc: &HttpSolanaRpc,
    n: u8,
    mut instructions: Vec<crate::solana_rpc::Instruction>,
) -> Result<String, String> {
    use crate::solana_rpc::{Instruction, SignatureState};
    use std::sync::atomic::{AtomicU32, Ordering::SeqCst};
    static SERIAL: AtomicU32 = AtomicU32::new(0);
    let mut data = vec![2];
    data.extend_from_slice(&(400_000 + SERIAL.fetch_add(1, SeqCst)).to_le_bytes());
    instructions.insert(
        0,
        Instruction {
            program_id: Pubkey::from_base58("ComputeBudget111111111111111111111111111111").unwrap(),
            accounts: vec![],
            data,
        },
    );
    let (hash, _) = rpc.get_latest_blockhash().await?;
    let message = LegacyMessage::compile(pk(n), &instructions, hash);
    let tx = Transaction {
        signatures: vec![key(n).sign(&message.serialize()).to_bytes()],
        message,
    };
    let signature = rpc.send_transaction(&tx.serialize()).await?;
    for _ in 0..160 {
        match rpc.get_signature_status(&signature).await? {
            SignatureState::Landed { err: Some(e), .. } => return Err(e),
            SignatureState::Landed {
                confirmation_status: Some(c),
                ..
            } if c == "finalized" => return Ok(signature),
            _ => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
    Err("manual transaction did not finalize within bound".into())
}
async fn send(rpc: &HttpSolanaRpc, n: u8, instruction: crate::solana_rpc::Instruction) -> String {
    send_many(rpc, n, vec![instruction]).await.unwrap()
}
/// Audited IDL ConfigAction::SetTimeLock(0), a real threshold-approved config
/// execution that marks older proposals stale without weakening membership.
async fn advance_stale_index(rpc: &HttpSolanaRpc, ctx: &squads::SquadsMintContext) {
    use crate::solana_rpc::{AccountMeta, Instruction, SYSTEM_PROGRAM_ID};
    let mut data = instruction_tag("config_transaction_create");
    data.extend_from_slice(&1u32.to_le_bytes());
    data.push(3);
    data.extend_from_slice(&0u32.to_le_bytes());
    data.push(0);
    let create = Instruction {
        program_id: squads::SQUADS_V4_PROGRAM_ID,
        accounts: vec![
            AccountMeta::writable(ctx.multisig),
            AccountMeta::writable(ctx.transaction_pda),
            AccountMeta::readonly_signer(pk(1)),
            AccountMeta::writable_signer(pk(1)),
            AccountMeta::readonly(SYSTEM_PROGRAM_ID),
        ],
        data,
    };
    send_many(
        rpc,
        1,
        vec![
            create,
            ctx.build_proposal_create(),
            ctx.build_proposal_approve(),
        ],
    )
    .await
    .unwrap();
    let mut second = ctx.clone();
    second.member = pk(2);
    send(rpc, 2, second.build_proposal_approve()).await;
    let execute = Instruction {
        program_id: squads::SQUADS_V4_PROGRAM_ID,
        accounts: vec![
            AccountMeta::writable(ctx.multisig),
            AccountMeta::readonly_signer(pk(1)),
            AccountMeta::writable(ctx.proposal_pda),
            AccountMeta::readonly(ctx.transaction_pda),
            AccountMeta::writable_signer(pk(1)),
            AccountMeta::readonly(SYSTEM_PROGRAM_ID),
        ],
        data: instruction_tag("config_transaction_execute"),
    };
    send(rpc, 1, execute).await;
}
async fn custody_rejection_matrix(
    config: &BridgeConfig,
    db: &Database,
    fixture: &serde_json::Value,
) {
    use crate::mint::solana::BRIDGE_MINT_AUTHORITY_OFFSET;
    let rpc = Arc::new(ControlledRpc::new(config.solana.rpc_url.clone()));
    let minter = SolMinter::with_parts(config.solana.clone(), rpc.clone(), Some((key(1), pk(1))))
        .unwrap()
        .with_store(db.clone());
    minter
        .verify_mint_authority_is_not_local_key()
        .await
        .unwrap();
    let program = Pubkey::from_base58(&config.solana.wbth_program).unwrap();
    let bridge = Pubkey::find_program_address(&[b"bridge"], &program)
        .unwrap()
        .0
        .to_base58();
    let multisig = fixture["multisig"].as_str().unwrap().to_string();
    let vault = fixture["vault"].as_str().unwrap().to_string();
    let original = rpc
        .inner
        .get_account_info(&multisig, "finalized")
        .await
        .unwrap()
        .unwrap();
    let mut cases = vec![];
    let mut a = original.clone();
    a.owner = pk(9);
    cases.push(("multisig owner", multisig.clone(), a));
    let mut a = original.clone();
    a.data[0] ^= 1;
    cases.push(("discriminator", multisig.clone(), a));
    let mut a = original.clone();
    a.data[8] ^= 1;
    cases.push(("multisig PDA", multisig.clone(), a));
    let mut a = original.clone();
    a.data[40..72].copy_from_slice(&pk(1).0);
    cases.push(("privileged config authority", multisig.clone(), a));
    let mut a = original.clone();
    a.data[72..74].copy_from_slice(&1u16.to_le_bytes());
    cases.push(("threshold one", multisig.clone(), a));
    let mut a = original.clone();
    a.data.truncate(20);
    cases.push(("truncated", multisig.clone(), a));
    let members = squads::state::MultisigState::parse(&original)
        .unwrap()
        .members;
    let offset = original
        .data
        .windows(33)
        .position(|w| w[..32] == members[0].0 .0 && w[32] == 7)
        .unwrap();
    let mut a = original.clone();
    a.data[offset + 32] = 1;
    cases.push(("member permissions", multisig.clone(), a));
    let original_bridge = rpc
        .inner
        .get_account_info(&bridge, "finalized")
        .await
        .unwrap()
        .unwrap();
    let mut a = original_bridge.clone();
    a.data[BRIDGE_MINT_AUTHORITY_OFFSET..BRIDGE_MINT_AUTHORITY_OFFSET + 32]
        .copy_from_slice(&pk(1).0);
    cases.push(("single-key bridge authority", bridge.clone(), a));
    let mut a = original_bridge.clone();
    a.owner = pk(8);
    cases.push(("bridge owner", bridge.clone(), a));
    let mut a = rpc
        .inner
        .get_account_info(&vault, "finalized")
        .await
        .unwrap()
        .unwrap();
    a.lamports = 0;
    cases.push(("vault rent", vault, a));
    let mut a = rpc
        .inner
        .get_account_info(&program.to_base58(), "finalized")
        .await
        .unwrap()
        .unwrap();
    a.owner = pk(5);
    cases.push(("program loader", program.to_base58(), a));
    for (label, address, account) in cases {
        rpc.account_overrides
            .lock()
            .unwrap()
            .insert(address, Some(account));
        assert!(
            minter
                .verify_mint_authority_is_not_local_key()
                .await
                .is_err(),
            "must reject {label}"
        );
        rpc.account_overrides.lock().unwrap().clear();
    }
    rpc.account_overrides
        .lock()
        .unwrap()
        .insert(multisig.clone(), None);
    assert!(minter
        .verify_mint_authority_is_not_local_key()
        .await
        .is_err());
    rpc.account_overrides.lock().unwrap().clear();
    let mut bad = config.solana.clone();
    bad.mint_threshold = 1;
    assert!(SolMinter::new(bad).is_err());
    let mut bad = config.solana.clone();
    bad.squads = None;
    bad.development_direct_mint = true;
    assert!(
        SolMinter::new(bad).is_err(),
        "federation cannot opt into direct fallback"
    );
    let mut bad = config.solana.clone();
    bad.mint_signers[2] = hex::encode(pk(12).0);
    let bad = SolMinter::with_parts(bad, rpc.clone(), Some((key(1), pk(1)))).unwrap();
    assert!(bad.verify_mint_authority_is_not_local_key().await.is_err());
    let nonmember =
        SolMinter::with_parts(config.solana.clone(), rpc.clone(), Some((key(12), pk(12)))).unwrap();
    assert!(nonmember
        .verify_mint_authority_is_not_local_key()
        .await
        .is_err());
    assert!(
        rpc.signatures.lock().unwrap().is_empty(),
        "custody rejection cannot broadcast"
    );
    println!("ENGINE PASS: custody owner/PDA/discriminator/permissions/threshold/authority/nonmember rejection matrix");
}
#[tokio::test]
#[ignore = "requires fresh real Squads+wbth local validator; SQUADS_ENGINE_TEST=1 localnet/run-squads.sh"]
async fn squads_engine_localnet() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("bth_bridge_service=warn")
        .with_test_writer()
        .try_init();
    let fixture: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            std::env::var("SQUADS_ENGINE_FIXTURE").expect("driver fixture required; no skip"),
        )
        .unwrap(),
    )
    .unwrap();
    let url = std::env::var("SQUADS_ENGINE_RPC").expect("local RPC required");
    assert!(url.starts_with("http://127.0.0.1:"));
    let rpc = HttpSolanaRpc::new(url.clone()).unwrap();
    let multisig = Pubkey::from_base58(fixture["multisig"].as_str().unwrap()).unwrap();
    let mint = fixture["mint"].as_str().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut configs = vec![];
    for n in [1, 2] {
        let mut config = BridgeConfig::default();
        config.solana.rpc_url = url.clone();
        config.solana.wbth_program = "CZDnzeywrqEM5ereWJmtYKUQ9uJXxX2PydqqKTQStxxE".into();
        config.solana.commitment = SolanaCommitment::Finalized;
        config.solana.mint_signers = (1..=3).map(|i| hex::encode(pk(i).0)).collect();
        config.solana.mint_threshold = 2;
        config.solana.squads = Some(SquadsConfig {
            history_page_size: 2,
            history_capacity: 4096,
            multisig: multisig.to_base58(),
            vault_index: 0,
            proposer: pk(1).to_base58(),
        });
        config.solana.development_direct_mint = false;
        let path = dir.path().join(format!("member{n}.seed"));
        std::fs::write(&path, hex::encode(key(n).to_bytes())).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        config.solana.keypair_file = Some(path.to_str().unwrap().into());
        config.bridge.db_path = dir
            .path()
            .join(format!("member{n}.db"))
            .to_str()
            .unwrap()
            .into();
        config.bridge.attestation_nonce_file = Some(
            dir.path()
                .join(format!("nonce{n}.json"))
                .to_str()
                .unwrap()
                .into(),
        );
        configs.push(config);
    }
    let mut order = BridgeOrder::new_mint(
        Chain::Solana,
        5_000_000_000_000,
        0,
        "confirmed-local-reserve".into(),
        fixture["recipient"].as_str().unwrap().into(),
    );
    order.source_tx = Some("local-confirmed-deposit".into());
    order.set_status(OrderStatus::DepositConfirmed);
    let mut db1 = Database::open(&configs[0].bridge.db_path).unwrap();
    db1.migrate().unwrap();
    db1.insert_order(&order).unwrap();
    custody_rejection_matrix(&configs[0], &db1, &fixture).await;
    let db2 = Database::open(&configs[1].bridge.db_path).unwrap();
    db2.migrate().unwrap();
    db2.insert_order(&order).unwrap();
    let controlled = Arc::new(ControlledRpc::new(url.clone()));
    let mut p1 = processor_with_rpc(
        &configs[0],
        &db1,
        std::slice::from_ref(&order),
        Some((controlled.clone(), 1)),
    );
    let p2 = processor(&configs[1], &db2, std::slice::from_ref(&order));
    for _ in 0..160 {
        tick(&p1).await;
        if let Some(row) = db1.solana_intent(&order.id).unwrap() {
            if let Some(index) = row.index.filter(|_| row.verified) {
                if let Some(p) = proposal(&rpc, multisig, index).await {
                    if p.approved.len() == 1 {
                        break;
                    }
                }
            }
        }
    }
    let first = db1
        .solana_intent(&order.id)
        .unwrap()
        .expect("proposer intent");
    assert!(first.verified, "{:?}", first);
    let index = first.index.unwrap();
    let raw = controlled.accepted_raw.lock().unwrap()[0].clone();
    let duplicate_broadcast = rpc.send_transaction(&raw).await;
    assert!(
        duplicate_broadcast.is_ok()
            || duplicate_broadcast == Err(crate::solana_rpc::ALREADY_PROCESSED_MARKER.into()),
        "{:?}",
        duplicate_broadcast
    );

    assert_eq!(
        proposal(&rpc, multisig, index)
            .await
            .unwrap()
            .approved
            .len(),
        1
    );
    assert_eq!(rpc.get_token_supply(mint, "finalized").await.unwrap(), 0);
    assert_eq!(
        db1.get_order(&order.id).unwrap().unwrap().status,
        OrderStatus::MintPending
    );
    // Simulate process restart: independent reopened SQLite connection and minter.
    drop(p1);
    drop(db1);
    db1 = Database::open(&configs[0].bridge.db_path).unwrap();
    db1.migrate().unwrap();
    p1 = processor(&configs[0], &db1, std::slice::from_ref(&order));
    tick(&p1).await;
    assert_eq!(
        db1.solana_intent(&order.id).unwrap().unwrap().index,
        Some(index)
    );
    db1.set_paused(true, Some("pause before quorum")).unwrap();
    for _ in 0..160 {
        tick(&p2).await;
        if db2
            .solana_intent(&order.id)
            .unwrap()
            .and_then(|r| r.action)
            .is_some_and(|a| a.kind == "approve")
        {
            break;
        }
    }
    db2.set_paused(true, Some("pause immediately after second vote"))
        .unwrap();
    for _ in 0..100 {
        if proposal(&rpc, multisig, index)
            .await
            .is_some_and(|p| p.status == 3)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(proposal(&rpc, multisig, index).await.unwrap().status, 3);
    let before1 = db1.solana_intent(&order.id).unwrap().unwrap().action;
    let before2 = db2.solana_intent(&order.id).unwrap().unwrap().action;
    for _ in 0..3 {
        tick(&p1).await;
        tick(&p2).await;
    }
    assert_eq!(rpc.get_token_supply(mint, "finalized").await.unwrap(), 0);
    assert_eq!(
        db1.solana_intent(&order.id).unwrap().unwrap().action,
        before1
    );
    assert_eq!(
        db2.solana_intent(&order.id).unwrap().unwrap().action,
        before2
    );
    drop(p1);
    p1 = processor(
        &configs[0],
        &Database::open(&configs[0].bridge.db_path).unwrap(),
        std::slice::from_ref(&order),
    );
    tick(&p1).await;
    assert_eq!(rpc.get_token_supply(mint, "finalized").await.unwrap(), 0);
    // Approved vault proposals remain executable even after a real config
    // transaction marks their index stale. Paused nodes must keep backing.
    advance_stale_index(&rpc, &context(&fixture, &order, index + 1, 1)).await;
    let multisig_account = rpc
        .get_account_info(&multisig.to_base58(), "finalized")
        .await
        .unwrap()
        .unwrap();
    assert!(
        squads::state::MultisigState::parse(&multisig_account)
            .unwrap()
            .stale_index
            >= index
    );
    tick(&p1).await;
    tick(&p2).await;
    for db in [&db1, &db2] {
        assert_eq!(
            db.get_order(&order.id).unwrap().unwrap().status,
            OrderStatus::MintPending
        );
        assert_eq!(db.locked_reserve_total().unwrap(), order.net_amount());
    }
    db1.set_paused(false, None).unwrap();
    for _ in 0..160 {
        tick(&p1).await;
        tick(&p2).await;
        if db1.get_order(&order.id).unwrap().unwrap().status == OrderStatus::Completed
            && db2.get_order(&order.id).unwrap().unwrap().status == OrderStatus::Completed
        {
            break;
        }
    }
    for db in [&db1, &db2] {
        let stored = db.get_order(&order.id).unwrap().unwrap();
        assert_eq!(
            stored.status,
            OrderStatus::Completed,
            "{:?}",
            db.solana_intent(&order.id).unwrap()
        );
        assert!(!stored.dest_tx.as_ref().unwrap().starts_with("squads:"));
        assert_eq!(
            stored.dest_tx,
            Some(db.get_mint_by_order(&order.id).unwrap().unwrap().dest_tx)
        );
        assert_eq!(db.locked_reserve_total().unwrap(), order.net_amount());
    }
    assert_eq!(
        db1.get_order(&order.id).unwrap().unwrap().dest_tx,
        db2.get_order(&order.id).unwrap().unwrap().dest_tx
    );
    for config in &configs {
        let conn = rusqlite::Connection::open(&config.bridge.db_path).unwrap();
        let detail:String=conn.query_row("SELECT details FROM audit_log WHERE order_id=?1 AND action='mint_confirmed' LIMIT 1",[order.id.to_string()],|r|r.get(0)).unwrap();
        assert!(detail.contains(
            db1.get_order(&order.id)
                .unwrap()
                .unwrap()
                .dest_tx
                .as_ref()
                .unwrap()
        ));
        assert!(!detail.contains("squads:"));
    }

    assert_eq!(
        rpc.get_token_supply(mint, "finalized").await.unwrap(),
        u128::from(order.net_amount())
    );
    for _ in 0..3 {
        tick(&p1).await;
        tick(&p2).await;
    }
    assert_eq!(
        rpc.get_token_supply(mint, "finalized").await.unwrap(),
        u128::from(order.net_amount())
    );
    let ata = rpc
        .get_account_info(fixture["ata"].as_str().unwrap(), "finalized")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ata.owner, crate::solana_rpc::TOKEN_PROGRAM_ID);
    assert_eq!(
        u64::from_le_bytes(ata.data[64..72].try_into().unwrap()),
        order.net_amount()
    );
    assert!(
        send_many(
            &rpc,
            1,
            vec![context(&fixture, &order, index, 1).build_vault_transaction_execute()]
        )
        .await
        .is_err(),
        "repeat execute must reject"
    );
    // A second order loses its create packet. Its expired contribution must
    // refresh at the SAME reserved index before any proposal has landed.
    use std::sync::atomic::Ordering::SeqCst;
    let mut rejected = BridgeOrder::new_mint(
        Chain::Solana,
        7_000_000_000_000,
        0,
        "reserve-rejected".into(),
        order.dest_address.clone(),
    );
    rejected.source_tx = Some("local-deposit-rejected".into());
    rejected.set_status(OrderStatus::DepositConfirmed);
    db1.insert_order(&rejected).unwrap();
    controlled.send_mode.store(2, SeqCst);
    let retry = processor_with_rpc(
        &configs[0],
        &db1,
        std::slice::from_ref(&rejected),
        Some((controlled.clone(), 1)),
    );
    for _ in 0..80 {
        tick(&retry).await;
        if db1
            .solana_intent(&rejected.id)
            .unwrap()
            .and_then(|r| r.action)
            .is_some()
        {
            break;
        }
    }
    let dropped = db1.solana_intent(&rejected.id).unwrap().unwrap();
    let old = dropped.action.as_ref().unwrap();
    for _ in 0..600 {
        if rpc.get_block_height().await.unwrap() > old.last_valid_height {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        rpc.get_block_height().await.unwrap() > old.last_valid_height,
        "blockhash must really expire"
    );
    controlled.send_mode.store(0, SeqCst);
    drop(retry);
    let retry = processor_with_rpc(
        &configs[0],
        &Database::open(&configs[0].bridge.db_path).unwrap(),
        std::slice::from_ref(&rejected),
        Some((controlled.clone(), 1)),
    );
    for _ in 0..160 {
        tick(&retry).await;
        if db1
            .solana_intent(&rejected.id)
            .unwrap()
            .is_some_and(|r| r.verified)
        {
            break;
        }
    }
    let refreshed = db1.solana_intent(&rejected.id).unwrap().unwrap();
    assert!(refreshed.verified);
    assert_eq!(refreshed.index, dropped.index);
    let rejected_index = refreshed.index.unwrap();
    let rejected_ctx = context(&fixture, &rejected, rejected_index, 1);
    assert!(
        send_many(&rpc, 1, vec![rejected_ctx.build_proposal_approve()])
            .await
            .is_err(),
        "duplicate member vote must reject"
    );
    // A funded nonmember cannot vote despite possessing its own transaction key.
    assert!(send_many(
        &rpc,
        12,
        vec![context(&fixture, &rejected, rejected_index, 12).build_proposal_approve()]
    )
    .await
    .is_err());
    for n in [2, 3] {
        let mut ix = context(&fixture, &rejected, rejected_index, n).build_proposal_approve();
        ix.data = instruction_tag("proposal_reject");
        ix.data.push(0);
        send(&rpc, n, ix).await;
    }
    assert_eq!(
        proposal(&rpc, multisig, rejected_index)
            .await
            .unwrap()
            .status,
        2
    );
    tick(&retry).await;
    assert_eq!(
        db1.get_order(&rejected.id).unwrap().unwrap().status,
        OrderStatus::MintPending
    );
    assert_eq!(
        rpc.get_token_supply(mint, "finalized").await.unwrap(),
        u128::from(order.net_amount())
    );
    assert_eq!(
        db1.locked_reserve_total().unwrap(),
        order.net_amount() + rejected.net_amount()
    );
    println!("ENGINE PASS: real expired contribution refresh, duplicate/nonmember rejection, rejected proposal retains reserve");
    // Competing unrelated order consumes a reserved index while our packet is
    // lost. Resolve positive finalized payload evidence, never RPC Unknown.
    let mut competing = BridgeOrder::new_mint(
        Chain::Solana,
        9_000_000_000_000,
        0,
        "reserve-competing".into(),
        order.dest_address.clone(),
    );
    competing.source_tx = Some("local-deposit-competing".into());
    competing.set_status(OrderStatus::DepositConfirmed);
    db1.insert_order(&competing).unwrap();
    controlled.send_mode.store(2, SeqCst);
    let competitor = processor_with_rpc(
        &configs[0],
        &db1,
        std::slice::from_ref(&competing),
        Some((controlled.clone(), 1)),
    );
    for _ in 0..80 {
        tick(&competitor).await;
        if db1
            .solana_intent(&competing.id)
            .unwrap()
            .and_then(|r| r.action)
            .is_some()
        {
            break;
        }
    }
    let reserved = db1.solana_intent(&competing.id).unwrap().unwrap();
    let candidate = reserved.index.unwrap();
    let old_action = reserved.action.as_ref().unwrap();
    let mut unrelated = competing.clone();
    unrelated.id = uuid::Uuid::new_v4();
    let unrelated_ctx = context(&fixture, &unrelated, candidate, 1);
    send_many(
        &rpc,
        1,
        vec![
            unrelated_ctx.build_vault_transaction_create(),
            unrelated_ctx.build_proposal_create(),
            unrelated_ctx.build_proposal_approve(),
        ],
    )
    .await
    .unwrap();
    tick(&competitor).await;
    assert_eq!(
        db1.solana_intent(&competing.id).unwrap().unwrap().index,
        Some(candidate)
    );
    for _ in 0..600 {
        if rpc.get_block_height().await.unwrap() > old_action.last_valid_height {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(rpc.get_block_height().await.unwrap() > old_action.last_valid_height);
    controlled.send_mode.store(1, SeqCst); // Accept replacement create, then lose its response.
    for _ in 0..160 {
        tick(&competitor).await;
        if db1
            .solana_intent(&competing.id)
            .unwrap()
            .is_some_and(|r| r.verified)
        {
            break;
        }
    }
    let bound = db1.solana_intent(&competing.id).unwrap().unwrap();
    assert!(bound.verified);
    assert_eq!(bound.index, Some(candidate + 1));
    assert_eq!(controlled.lost_responses.load(SeqCst), 2);
    assert_eq!(
        proposal(&rpc, multisig, candidate)
            .await
            .unwrap()
            .approved
            .len(),
        1
    );
    let ctx = context(&fixture, &competing, candidate + 1, 1);
    // A missing previously verified transaction can never clear the binding.
    controlled
        .account_overrides
        .lock()
        .unwrap()
        .insert(ctx.transaction_pda.to_base58(), None);
    tick(&competitor).await;
    assert_eq!(
        db1.solana_intent(&competing.id).unwrap().unwrap().index,
        bound.index
    );
    assert_eq!(
        db1.get_order(&competing.id).unwrap().unwrap().status,
        OrderStatus::MintPending
    );
    controlled.account_overrides.lock().unwrap().clear();
    // A corrupted/extra instruction with the same order id cannot match the
    // immutable full bundle. The engine must not approve or abandon it.
    let actual = rpc
        .get_account_info(&ctx.transaction_pda.to_base58(), "finalized")
        .await
        .unwrap()
        .unwrap();
    let mut altered = actual.clone();
    let payload_offset = altered
        .data
        .windows(ctx.inner.data.len())
        .position(|w| w == ctx.inner.data)
        .expect("stored exact instruction");
    altered.data[payload_offset + 8] ^= 1; // same order id, different amount
    controlled
        .account_overrides
        .lock()
        .unwrap()
        .insert(ctx.transaction_pda.to_base58(), Some(altered));
    tick(&competitor).await;
    assert_eq!(
        db1.solana_intent(&competing.id).unwrap().unwrap().index,
        bound.index
    );
    controlled.account_overrides.lock().unwrap().clear();
    // Deliberately create a duplicate canonical payload outside the engine.
    // An independent follower must flag ambiguity instead of splitting votes.
    let duplicate = context(&fixture, &competing, candidate + 2, 1);
    send_many(
        &rpc,
        1,
        vec![
            duplicate.build_vault_transaction_create(),
            duplicate.build_proposal_create(),
            duplicate.build_proposal_approve(),
        ],
    )
    .await
    .unwrap();
    db2.insert_order(&competing).unwrap();
    db2.set_paused(false, None).unwrap();
    let follower = processor(&configs[1], &db2, std::slice::from_ref(&competing));
    for _ in 0..3 {
        tick(&follower).await;
    }
    let follower_row = db2.solana_intent(&competing.id).unwrap().unwrap();
    assert_eq!(follower_row.index, None);
    assert!(follower_row.action.is_none());
    assert_eq!(
        proposal(&rpc, multisig, candidate + 1)
            .await
            .unwrap()
            .approved
            .len(),
        1
    );
    assert_eq!(
        proposal(&rpc, multisig, candidate + 2)
            .await
            .unwrap()
            .approved
            .len(),
        1
    );
    assert_eq!(
        db1.locked_reserve_total().unwrap(),
        order.net_amount() + rejected.net_amount() + competing.net_amount()
    );
    assert_eq!(
        db2.locked_reserve_total().unwrap(),
        order.net_amount() + competing.net_amount()
    );
    assert_eq!(
        rpc.get_token_supply(mint, "finalized").await.unwrap(),
        u128::from(order.net_amount())
    );
    println!("ENGINE PASS: competing index, accepted-send response loss, missing/mismatched binding, ambiguous proposals retain backing");
    // A restored durable binding may reconcile an already-landed execution
    // even while paused and after expected member configuration drifts.
    let restored = Database::open(dir.path().join("restored.db").to_str().unwrap()).unwrap();
    restored.migrate().unwrap();
    let mut pending_order = order.clone();
    pending_order.set_status(OrderStatus::MintPending);
    restored.insert_order(&pending_order).unwrap();
    restored
        .record_mint_submitted(
            &order.id,
            &hex::encode(order.order_id_bytes()),
            Chain::Solana,
            "squads:restored",
        )
        .unwrap();
    restored
        .record_locked_output(
            &format!("dep:{}", order.id),
            Chain::Solana,
            order.net_amount(),
            &order.id,
        )
        .unwrap();
    let original = db1.solana_intent(&order.id).unwrap().unwrap();
    let row = restored
        .claim_solana_intent(&order.id, &original.binding, &original.multisig)
        .unwrap();
    restored
        .update_solana_intent(&row, original.index, true, None)
        .unwrap();
    restored
        .set_paused(true, Some("restored kill switch"))
        .unwrap();
    let mut drift = configs[0].clone();
    drift.solana.mint_signers[2] = hex::encode(pk(12).0);
    let read_only = processor(&drift, &restored, std::slice::from_ref(&order));
    tick(&read_only).await;
    assert_eq!(
        restored.get_order(&order.id).unwrap().unwrap().status,
        OrderStatus::Completed
    );
    assert_eq!(
        restored.get_order(&order.id).unwrap().unwrap().dest_tx,
        db1.get_order(&order.id).unwrap().unwrap().dest_tx
    );
    // A late follower without the pre-execution binding cannot reconstruct it
    // from the emptied VaultTransaction. It explicitly holds for recovery.
    let late = Database::open(dir.path().join("late.db").to_str().unwrap()).unwrap();
    late.migrate().unwrap();
    late.insert_order(&order).unwrap();
    let late_processor = processor(&configs[1], &late, std::slice::from_ref(&order));
    for _ in 0..3 {
        tick(&late_processor).await;
    }
    assert_eq!(
        late.get_order(&order.id).unwrap().unwrap().status,
        OrderStatus::MintPending
    );
    assert!(late
        .solana_intent(&order.id)
        .unwrap()
        .unwrap()
        .index
        .is_none());
    assert_eq!(late.locked_reserve_total().unwrap(), order.net_amount());
    println!("ENGINE PASS: paused config-drift read-only completion, late follower historical gap retains backing");
    let mut receipts = vec![];
    for (signature, slot) in rpc
        .get_signatures_for_address(&multisig.to_base58(), None, "finalized")
        .await
        .unwrap()
    {
        let (logs, observed_slot) = rpc
            .get_transaction_logs(&signature, "finalized")
            .await
            .unwrap()
            .expect("finalized receipt");
        assert_eq!(slot, observed_slot);
        assert!(!logs.is_empty());
        receipts.push(serde_json::json!({"signature":signature,"slot":slot,"logs":logs}));
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut hashes = serde_json::Map::new();
    for path in [
        "Cargo.lock",
        "contracts/solana/package-lock.json",
        "contracts/solana/fixtures/squads-v4/idl.json",
        "bridge/core/src/config.rs",
        "bridge/service/src/mint/solana.rs",
        "bridge/service/src/mint/squads.rs",
        "bridge/service/src/mint/solana/squads_backend.rs",
        "bridge/service/src/mint/squads/state.rs",
        "bridge/service/src/db.rs",
        "bridge/service/src/db/solana_intents.rs",
        "bridge/service/src/engine.rs",
        "bridge/service/src/solana_rpc.rs",
        "bridge/service/src/squads_engine_tests.rs",
        "contracts/solana/localnet/squads.ts",
        "contracts/solana/localnet/run-squads.sh",
        "contracts/solana/target/deploy/wbth.so",
        "contracts/solana/fixtures/squads-v4/squads_multisig_program.so",
    ] {
        use sha2::Digest;
        hashes.insert(
            path.into(),
            hex::encode(sha2::Sha256::digest(
                std::fs::read(root.join(path)).unwrap(),
            ))
            .into(),
        );
    }
    let evidence = serde_json::json!({"schema":1,"kind":"actual-order-processor-local-validator","fixture":fixture,"source_sha256":hashes,"receipts":receipts,
        "completed_order":order.id,"execution_signature":db1.get_order(&order.id).unwrap().unwrap().dest_tx,
        "canonical_index":index,"rejected_index":rejected_index,"competing_candidate":candidate,"competing_resolved_index":bound.index,
        "supply":rpc.get_token_supply(mint,"finalized").await.unwrap().to_string(),"recipient_balance":order.net_amount().to_string(),
        "member1_locked_reserve":db1.locked_reserve_total().unwrap().to_string(),"member2_locked_reserve":db2.locked_reserve_total().unwrap().to_string(),
        "assertions":["strict custody rejection matrix","two independent member databases","distinct real on-chain votes","lost response after accepted send","approved stale proposal executable","pause at quorum and after restart","expired action refresh same index","actual executed proposal and marker event","duplicate broadcast idempotency, vote and execute rejection","nonmember rejection","rejected proposal retains backing","competing unrelated proposal","same order different amount rejected","missing verified account retained","ambiguous matching proposals retained","paused config drift read-only completion","late follower historical gap retained"]});
    std::fs::write(
        std::env::var("SQUADS_ENGINE_EVIDENCE").expect("engine evidence path required"),
        serde_json::to_vec_pretty(&evidence).unwrap(),
    )
    .unwrap();
    println!("ENGINE PASS: independent DBs, real federation attestations, one proposal, 2 votes, pause/restart, exactly-once completion with execution signature");
}

#[tokio::test]
#[ignore = "requires explicit pinned local validator driver"]
async fn squads_history_localnet() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("bth_bridge_service=warn")
        .with_test_writer()
        .try_init();
    let fixture: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            std::env::var("SQUADS_ENGINE_FIXTURE").expect("driver fixture required; no skip"),
        )
        .unwrap(),
    )
    .unwrap();
    let url = std::env::var("SQUADS_ENGINE_RPC").expect("local RPC required");
    assert!(url.starts_with("http://127.0.0.1:"));
    let rpc = HttpSolanaRpc::new(url.clone()).unwrap();
    let multisig = Pubkey::from_base58(fixture["multisig"].as_str().unwrap()).unwrap();
    let mint = fixture["mint"].as_str().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut configs = vec![];
    for n in [1, 2] {
        let mut config = BridgeConfig::default();
        config.solana.rpc_url = url.clone();
        config.solana.wbth_program = "CZDnzeywrqEM5ereWJmtYKUQ9uJXxX2PydqqKTQStxxE".into();
        config.solana.commitment = SolanaCommitment::Finalized;
        config.solana.mint_signers = (1..=3).map(|i| hex::encode(pk(i).0)).collect();
        config.solana.mint_threshold = 2;
        config.solana.squads = Some(SquadsConfig {
            history_page_size: 2,
            history_capacity: 4096,
            multisig: multisig.to_base58(),
            vault_index: 0,
            proposer: pk(1).to_base58(),
        });
        config.solana.development_direct_mint = false;
        let path = dir.path().join(format!("member{n}.seed"));
        std::fs::write(&path, hex::encode(key(n).to_bytes())).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        config.solana.keypair_file = Some(path.to_str().unwrap().into());
        config.bridge.db_path = dir
            .path()
            .join(format!("member{n}.db"))
            .to_str()
            .unwrap()
            .into();
        config.bridge.attestation_nonce_file = Some(
            dir.path()
                .join(format!("nonce{n}.json"))
                .to_str()
                .unwrap()
                .into(),
        );
        configs.push(config);
    }

    use crate::solana_rpc::{AccountMeta, Instruction, SYSTEM_PROGRAM_ID};
    let mut order = BridgeOrder::new_mint(
        Chain::Solana,
        1_000_000_000_000,
        0,
        "historical-confirmed-reserve".into(),
        fixture["recipient"].as_str().unwrap().into(),
    );
    order.source_tx = Some("local-history-confirmed-deposit".into());
    order.set_status(OrderStatus::DepositConfirmed);
    let db1 = Database::open(&configs[0].bridge.db_path).unwrap();
    db1.migrate().unwrap();
    db1.insert_order(&order).unwrap();
    let db2 = Database::open(&configs[1].bridge.db_path).unwrap();
    db2.migrate().unwrap();
    db2.insert_order(&order).unwrap();
    let p1 = processor(&configs[0], &db1, std::slice::from_ref(&order));
    let p2 = processor(&configs[1], &db2, std::slice::from_ref(&order));
    for _ in 0..160 {
        tick(&p1).await;
        if let Some(r) = db1.solana_intent(&order.id).unwrap() {
            if r.verified {
                break;
            }
        }
    }
    let index = db1
        .solana_intent(&order.id)
        .unwrap()
        .unwrap()
        .index
        .unwrap();
    let ctx = context(&fixture, &order, index, 1);
    let before = rpc
        .get_account_info(&ctx.transaction_pda.to_base58(), "finalized")
        .await
        .unwrap()
        .unwrap()
        .data;
    for _ in 0..160 {
        tick(&p1).await;
        tick(&p2).await;
        if db1.get_order(&order.id).unwrap().unwrap().status == OrderStatus::Completed
            && db2.get_order(&order.id).unwrap().unwrap().status == OrderStatus::Completed
        {
            break;
        }
    }
    assert_eq!(
        db1.get_order(&order.id).unwrap().unwrap().status,
        OrderStatus::Completed
    );
    assert_eq!(
        db2.get_order(&order.id).unwrap().unwrap().status,
        OrderStatus::Completed
    );
    assert_eq!(
        rpc.get_account_info(&ctx.transaction_pda.to_base58(), "finalized")
            .await
            .unwrap()
            .unwrap()
            .data,
        before
    );
    let execution = db1.get_mint_by_order(&order.id).unwrap().unwrap().dest_tx;
    let supply = rpc.get_token_supply(mint, "finalized").await.unwrap();
    assert_eq!(supply, order.net_amount() as u128);
    // A fresh independent member with no pre-execution binding must recover
    // from history, even while paused after its ordinary claim locks backing.
    let late_path = dir.path().join("history-late.db");
    let mut late = Database::open(late_path.to_str().unwrap()).unwrap();
    late.migrate().unwrap();
    late.insert_order(&order).unwrap();
    let latep = processor(&configs[1], &late, std::slice::from_ref(&order));
    tick(&latep).await;
    assert!(late
        .solana_intent(&order.id)
        .unwrap()
        .unwrap()
        .index
        .is_none());
    late.set_paused(true, Some("historical read-only recovery"))
        .unwrap();
    let backing = late.locked_reserve_total().unwrap();
    assert_eq!(backing, order.net_amount());
    let mut minter = SolMinter::new(configs[1].solana.clone())
        .unwrap()
        .with_store(late.clone());
    let mut pending_order = late.get_order(&order.id).unwrap().unwrap();
    for _ in 0..100 {
        minter
            .reconcile_squads_history(&pending_order)
            .await
            .unwrap();
        if late.get_order(&order.id).unwrap().unwrap().status == OrderStatus::Completed {
            break;
        }
    }
    assert_eq!(
        late.get_order(&order.id).unwrap().unwrap().status,
        OrderStatus::Completed,
        "{:?}",
        late.solana_history(&order.id).unwrap()
    );
    assert_eq!(
        late.get_mint_by_order(&order.id).unwrap().unwrap().dest_tx,
        execution
    );
    assert_eq!(late.locked_reserve_total().unwrap(), backing);
    // Close through the actual Squads program (collector fixed at creation).
    let close = Instruction {
        program_id: squads::SQUADS_V4_PROGRAM_ID,
        accounts: vec![
            AccountMeta::readonly(multisig),
            AccountMeta::writable(ctx.proposal_pda),
            AccountMeta::writable(ctx.transaction_pda),
            AccountMeta::writable(pk(1)),
            AccountMeta::readonly(SYSTEM_PROGRAM_ID),
        ],
        data: instruction_tag("vault_transaction_accounts_close"),
    };
    let close_sig = send_many(&rpc, 1, vec![close]).await.unwrap();
    assert!(rpc
        .get_account_info(&ctx.proposal_pda.to_base58(), "finalized")
        .await
        .unwrap()
        .is_none());
    assert!(rpc
        .get_account_info(&ctx.transaction_pda.to_base58(), "finalized")
        .await
        .unwrap()
        .is_none());
    let marker = ctx.inner.accounts[1].pubkey;
    let mut spam = vec![];
    for _ in 0..5 {
        let mut data = 2u32.to_le_bytes().to_vec();
        data.extend(1u64.to_le_bytes());
        spam.push(
            send_many(
                &rpc,
                1,
                vec![Instruction {
                    program_id: SYSTEM_PROGRAM_ID,
                    accounts: vec![
                        AccountMeta::writable_signer(pk(1)),
                        AccountMeta::writable(marker),
                    ],
                    data,
                }],
            )
            .await
            .unwrap(),
        );
    }
    // New DB again: no imported journal or pre-execution binding.
    drop(minter);
    drop(latep);
    drop(late);
    let closed_path = dir.path().join("history-closed.db");
    late = Database::open(closed_path.to_str().unwrap()).unwrap();
    late.migrate().unwrap();
    late.insert_order(&order).unwrap();
    let closedp = processor(&configs[1], &late, std::slice::from_ref(&order));
    tick(&closedp).await;
    drop(closedp);
    late.set_paused(true, Some("closed-account recovery"))
        .unwrap();
    pending_order = late.get_order(&order.id).unwrap().unwrap();
    // Post-execution real governance does not invalidate earlier execution.
    advance_stale_index(&rpc, &context(&fixture, &order, index + 1, 1)).await;
    let history_rpc = Arc::new(ControlledRpc::new(url.clone()));
    history_rpc
        .send_mode
        .store(2, std::sync::atomic::Ordering::SeqCst);
    history_rpc
        .history_null_once
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let mut drifted = configs[1].solana.clone();
    drifted.mint_threshold = 3;
    minter = SolMinter::with_parts(drifted.clone(), history_rpc.clone(), Some((key(2), pk(2))))
        .unwrap()
        .with_store(late.clone());
    *history_rpc.genesis_override.lock().unwrap() = Some(Pubkey([99; 32]).to_base58());
    assert!(minter
        .reconcile_squads_history(&pending_order)
        .await
        .unwrap_err()
        .to_string()
        .contains("network"));
    *history_rpc.genesis_override.lock().unwrap() = None;
    assert!(minter
        .reconcile_squads_history(&pending_order)
        .await
        .unwrap_err()
        .to_string()
        .contains("unavailable"));
    let unavailable: serde_json::Value =
        serde_json::from_str(&late.solana_history(&order.id).unwrap().unwrap().progress).unwrap();
    assert!(!unavailable["marker"]["queue"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        late.get_order(&order.id).unwrap().unwrap().status,
        OrderStatus::MintPending
    );
    assert_eq!(late.locked_reserve_total().unwrap(), backing);
    for _ in 0..3 {
        minter
            .reconcile_squads_history(&pending_order)
            .await
            .unwrap();
    }
    let saved = late.solana_history(&order.id).unwrap().unwrap();
    assert!(saved.progress.contains("before"));
    drop(minter);
    drop(late);
    late = Database::open(closed_path.to_str().unwrap()).unwrap();
    late.migrate().unwrap();
    assert_eq!(
        late.solana_history(&order.id).unwrap().unwrap().progress,
        saved.progress
    );
    minter = SolMinter::with_parts(drifted, history_rpc.clone(), Some((key(2), pk(2))))
        .unwrap()
        .with_store(late.clone());
    for _ in 0..150 {
        minter
            .reconcile_squads_history(&pending_order)
            .await
            .unwrap();
        if late.get_order(&order.id).unwrap().unwrap().status == OrderStatus::Completed {
            break;
        }
    }
    assert_eq!(
        late.get_order(&order.id).unwrap().unwrap().status,
        OrderStatus::Completed,
        "{:?}",
        late.solana_history(&order.id).unwrap()
    );
    assert_eq!(
        late.get_mint_by_order(&order.id).unwrap().unwrap().dest_tx,
        execution
    );
    assert_eq!(late.locked_reserve_total().unwrap(), backing);
    for _ in 0..3 {
        minter
            .reconcile_squads_history(&pending_order)
            .await
            .unwrap();
    }
    assert_eq!(
        rpc.get_token_supply(mint, "finalized").await.unwrap(),
        supply
    );
    assert_eq!(
        history_rpc
            .send_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert!(
        history_rpc
            .history_pages
            .load(std::sync::atomic::Ordering::SeqCst)
            > 2
    );
    // A later mint belongs to an unsupported governance epoch. Its successful
    // on-chain quorum is real, but deployment policy is not silently universal.
    let mut newer = order.clone();
    newer.id = uuid::Uuid::new_v4();
    newer.source_tx = Some("newer-confirmed-deposit".into());
    db1.insert_order(&newer).unwrap();
    db2.insert_order(&newer).unwrap();
    let newer1 = processor(&configs[0], &db1, std::slice::from_ref(&newer));
    let newer2 = processor(&configs[1], &db2, std::slice::from_ref(&newer));
    for _ in 0..160 {
        tick(&newer1).await;
        tick(&newer2).await;
        if db1.get_order(&newer.id).unwrap().unwrap().status == OrderStatus::Completed
            && db2.get_order(&newer.id).unwrap().unwrap().status == OrderStatus::Completed
        {
            break;
        }
    }
    assert_eq!(
        db1.get_order(&newer.id).unwrap().unwrap().status,
        OrderStatus::Completed
    );
    let epoch = Database::open(dir.path().join("unsupported-epoch.db").to_str().unwrap()).unwrap();
    epoch.migrate().unwrap();
    epoch.insert_order(&newer).unwrap();
    let epochp = processor(&configs[1], &epoch, std::slice::from_ref(&newer));
    tick(&epochp).await;
    epoch
        .set_paused(true, Some("unsupported governance epoch"))
        .unwrap();
    let epochminter = SolMinter::new(configs[1].solana.clone())
        .unwrap()
        .with_store(epoch.clone());
    let epochorder = epoch.get_order(&newer.id).unwrap().unwrap();
    for _ in 0..100 {
        epochminter
            .reconcile_squads_history(&epochorder)
            .await
            .unwrap();
    }
    assert_eq!(
        epoch.get_order(&newer.id).unwrap().unwrap().status,
        OrderStatus::MintPending
    );
    assert!(epoch
        .solana_history(&newer.id)
        .unwrap()
        .unwrap()
        .progress
        .contains("unsupported historical governance"));
    assert_eq!(epoch.locked_reserve_total().unwrap(), newer.net_amount());
    let mut wrong_source = pending_order.clone();
    wrong_source.source_tx = Some("different-confirmed-source".into());
    assert!(minter
        .reconcile_squads_history(&wrong_source)
        .await
        .is_err());
    let mut wrong_recipient = pending_order.clone();
    wrong_recipient.dest_address = pk(3).to_base58();
    assert!(minter
        .reconcile_squads_history(&wrong_recipient)
        .await
        .is_err());
    let mut stale_completed = pending_order.clone();
    stale_completed.status = OrderStatus::Completed;
    stale_completed.dest_tx = Some("wrong-completed-signature".into());
    assert!(minter
        .reconcile_squads_history(&stale_completed)
        .await
        .is_err());
    // Mixed-order real history must still recover the older supported mint.
    let mixed =
        Database::open(dir.path().join("mixed-order-history.db").to_str().unwrap()).unwrap();
    mixed.migrate().unwrap();
    mixed.insert_order(&order).unwrap();
    let mixedp = processor(&configs[1], &mixed, std::slice::from_ref(&order));
    tick(&mixedp).await;
    mixed
        .set_paused(true, Some("mixed-order recovery"))
        .unwrap();
    let mixedminter = SolMinter::new(configs[1].solana.clone())
        .unwrap()
        .with_store(mixed.clone());
    let mixedorder = mixed.get_order(&order.id).unwrap().unwrap();
    for _ in 0..150 {
        mixedminter
            .reconcile_squads_history(&mixedorder)
            .await
            .unwrap();
        if mixed.get_order(&order.id).unwrap().unwrap().status == OrderStatus::Completed {
            break;
        }
    }
    assert_eq!(
        mixed.get_order(&order.id).unwrap().unwrap().status,
        OrderStatus::Completed,
        "{:?}",
        mixed.solana_history(&order.id).unwrap()
    );
    assert_eq!(
        mixed.get_mint_by_order(&order.id).unwrap().unwrap().dest_tx,
        execution
    );
    assert_eq!(mixed.locked_reserve_total().unwrap(), backing);
    let final_supply = rpc.get_token_supply(mint, "finalized").await.unwrap();
    assert_eq!(final_supply, supply * 2);
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut hashes = serde_json::Map::new();
    for path in [
        "Cargo.lock",
        "bridge/core/src/config.rs",
        ".github/workflows/solana-contracts-ci.yml",
        "bridge/service/src/lib.rs",
        "bridge/service/src/mint/solana.rs",
        "bridge/service/src/mint/squads/state.rs",
        "bridge/service/tests/fixtures/squads-history-transactions.json",
        "bridge/service/src/db.rs",
        "bridge/service/src/db/solana_history.rs",
        "bridge/service/src/db/solana_intents.rs",
        "bridge/service/src/mint/solana/history.rs",
        "bridge/service/src/mint/solana/squads_backend.rs",
        "bridge/service/src/solana_rpc.rs",
        "bridge/service/src/solana_rpc/history.rs",
        "bridge/service/src/squads_engine_tests.rs",
        "bridge/service/src/main.rs",
        "contracts/solana/localnet/squads.ts",
        "contracts/solana/localnet/run-squads.sh",
        "contracts/solana/package-lock.json",
        "contracts/solana/target/deploy/wbth.so",
        "contracts/solana/fixtures/squads-v4/squads_multisig_program.so",
        "contracts/solana/fixtures/squads-v4/idl.json",
    ] {
        use sha2::Digest;
        hashes.insert(
            path.into(),
            hex::encode(sha2::Sha256::digest(
                std::fs::read(root.join(path)).unwrap(),
            ))
            .into(),
        );
    }
    let recipient_data = rpc
        .get_account_data(fixture["ata"].as_str().unwrap(), "finalized")
        .await
        .unwrap()
        .unwrap();
    let recipient_balance = u64::from_le_bytes(recipient_data[64..72].try_into().unwrap());
    assert_eq!(u128::from(recipient_balance), final_supply);
    let evidence = serde_json::json!({"mixed_order_recovered_signature":mixed.get_mint_by_order(&order.id).unwrap().unwrap().dest_tx,"recipient_balance":recipient_balance,"unsupported_epoch_execution":db1.get_mint_by_order(&newer.id).unwrap().unwrap().dest_tx,"unsupported_epoch_progress":epoch.solana_history(&newer.id).unwrap().unwrap().progress,"source_sha256":hashes,"final_supply":final_supply.to_string(),"unsupported_epoch_order":newer.id,"history_rpc_pages":history_rpc.history_pages.load(std::sync::atomic::Ordering::SeqCst),"recovery_send_calls":0,"null_transaction_fault":"one response withheld; durable queue retained then retried","fixture":fixture,"execution":execution,"close":close_sig,"marker_noise":spam,"supply":supply.to_string(),"locked_backing":backing,"progress":late.solana_history(&order.id).unwrap().unwrap().progress});
    std::fs::write(
        std::env::var("SQUADS_ENGINE_EVIDENCE").unwrap(),
        serde_json::to_vec_pretty(&evidence).unwrap(),
    )
    .unwrap();
    println!("HISTORY PASS: retained payload, live and closed accounts, paginated marker history, fresh independent DB, restart, paused read-only completion, unchanged supply/backing");
}
