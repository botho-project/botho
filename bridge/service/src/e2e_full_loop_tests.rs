// Copyright (c) 2024 The Botho Foundation

//! Orchestrated full-loop bridge e2e (#993): the single
//! **wrap → mint-wBTH → burn → release-BTH** round trip driven end to end
//! through the REAL bridge engine ([`OrderProcessor::process_pending_orders`]),
//! with BOTH chains live — a local Botho node AND a local Hardhat/Anvil node.
//!
//! Where `fork_tests.rs` covers only the Ethereum leg against a synthetic
//! order (hand-walking order status and stopping the burn at the release
//! gate) and `bth_fork_tests.rs` covers only the BTH transport in isolation,
//! this test closes the loop: it inserts one mint order and one burn order and
//! lets the production [`OrderProcessor`] walk them
//! `DepositConfirmed → … → Completed` (mint) and `BurnConfirmed → … →
//! Released` (release), driving the REAL [`EthMinter`] (Safe-wrapped
//! `bridgeMint`), the REAL [`BthReleaser`] (CLSAG reserve spend to a fresh
//! stealth output), and the REAL [`FederationAttestationProvider`] (t-of-n
//! EIP-712 for the mint, t-of-n Ed25519 for the release).
//!
//! ## The four assertions (the demonstration's proof)
//!
//! 1. **wBTH minted == BTH locked** — `wbth.balanceOf(user)` and
//!    `wbth.totalSupply()` equal the order's net amount, which equals the
//!    reserve ledger's locked backing (ADR 0003 factor-1, exact).
//! 2. **Burn releases the correct BTH to a fresh stealth output** — the live
//!    [`BthReleaser`] pays `net_amount` to a one-time output (ADR 0004) that
//!    the user's own view key scans back off the live node, distinct from the
//!    burn transaction on the EVM side.
//! 3. **Proof-of-reserves invariant holds across the loop** — the
//!    [`Reconciler`] reports `drift == 0` (`Σ wBTH == locked reserve`) after
//!    the mint and again after the release, with the live BTH node actually
//!    consulted for the custody leg, and returns to `0/0` once unwrapped.
//! 4. **Attestation authorized by the federation** — the mint does NOT proceed
//!    on a single signer (the engine leaves it `DepositConfirmed` until a
//!    second Safe-owner envelope arrives) and the release does NOT proceed on a
//!    single Ed25519 signer (left `BurnConfirmed` until the peer envelope
//!    arrives); both cross the configured threshold before any value moves.
//!
//! ## Running
//!
//! This test is `#[ignore]`d AND **self-skips** unless both nodes are
//! provided — it never claims a live path it could not exercise (the same
//! discipline as `bth_fork_tests.rs`). The hermetic driver boots both:
//!
//! ```text
//! ./scripts/bridge-e2e-full-loop.sh
//! ```
//!
//! or manually, with a funded factor-1 reserve on a local Botho node and a
//! local Hardhat node up:
//!
//! ```text
//! BRIDGE_FORK_RPC_URL=http://127.0.0.1:8545 \
//! BRIDGE_BTH_RPC_URL=http://127.0.0.1:27201 \
//! BRIDGE_BTH_RESERVE_VIEW_KEY=/path/reserve.view.hex \
//! BRIDGE_BTH_RESERVE_SPEND_KEY=/path/reserve.spend.hex \
//! BRIDGE_BTH_RESERVE_PQ_SEED=/path/reserve.pq_seed.hex \
//! BRIDGE_BTH_RESERVE_ADDRESS=<reserve bth address> \
//! BRIDGE_BTH_USER_ADDRESS=<user bth address> \
//! BRIDGE_BTH_USER_VIEW_KEY=/path/user.view.hex \
//! BRIDGE_BTH_USER_SPEND_KEY=/path/user.spend.hex \
//! BRIDGE_BTH_USER_PQ_SEED=/path/user.pq_seed.hex \
//!   cargo test -p bth-bridge-service -- --ignored full_loop_
//! ```
//!
//! The `*_PQ_SEED` files are the 64-byte BIP39 seeds the reserve/user derive
//! their ML-KEM-768 secrets from (issue #972); on the protocol-6.0.0 hybrid
//! chain they are required for the wallet to detect outputs paid to it. The
//! hermetic driver provisions all of the above at runtime via
//! `botho-testnet gen-bridge-keys` (no committed secret).
//!
//! The Ethereum half swaps `local (31337) → Sepolia-fork → live Sepolia`
//! purely by pointing `BRIDGE_FORK_RPC_URL` at a fork/live RPC (+ a funded
//! relayer key for live) — no test-logic change (companion #992/#866).

use alloy::{
    network::{EthereumWallet, TransactionBuilder},
    primitives::{Address, U256},
    providers::{DynProvider, Provider, ProviderBuilder},
    rpc::types::TransactionRequest,
    signers::local::PrivateKeySigner,
    sol,
    sol_types::{SolCall, SolValue},
};
use bth_bridge_core::{
    attestation::{sign_attestation_ed25519, AttestationKind},
    BridgeConfig, BridgeOrder, BthConfig, Chain, GasPriceStrategy, OrderStatus,
};
use chrono::Utc;
use ed25519_dalek::SigningKey;
use std::{collections::HashMap, sync::Arc, time::Duration};
use uuid::Uuid;

use crate::{
    attestation::{sign_attestation_secp256k1, AttestationProvider, FederationAttestationProvider},
    db::Database,
    engine::OrderProcessor,
    mint::{ethereum::EthMinter, Minter},
    release::{bth::BthReleaser, Releaser},
    reserve::Reconciler,
    watchers::{
        bth::{BthChainClient, NodeBthClient},
        ethereum::{with_tx_ordinals, AlloyEthClient, EthChainClient},
    },
};

sol! {
    #[allow(missing_docs)]
    interface IWrappedBTHFullLoop {
        function balanceOf(address account) external view returns (uint256);
        function totalSupply() external view returns (uint256);
        function bridgeBurn(uint256 amount, string calldata bthAddress) external;
    }
}

/// Well-known dev accounts of `npx hardhat node` / `anvil` (mnemonic
/// "test test test test test test test test test test test junk"). Test ETH
/// on chain 31337 only — not secrets. Mirrors `fork_tests.rs`.
const DEV_KEYS: [&str; 4] = [
    // 0: contract deployer + relayer EOA (pays gas, holds no authority)
    "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
    // 1: Safe owner 1 (local mint attestation signer)
    "59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d",
    // 2: Safe owner 2 (peer federation member, envelope injected)
    "5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a",
    // 3: the bridging user (mint recipient / burner)
    "7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6",
];

fn eth_rpc_url() -> String {
    std::env::var("BRIDGE_FORK_RPC_URL").unwrap_or_else(|_| "http://127.0.0.1:8545".to_string())
}

fn dev_signer(index: usize) -> PrivateKeySigner {
    DEV_KEYS[index].parse().expect("valid dev key")
}

/// Read a Hardhat artifact's creation bytecode (shared shape with
/// `fork_tests.rs`; duplicated to keep changes additive per #992 coordination).
fn artifact_bytecode(rel_path: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../contracts/ethereum/artifacts")
        .join(rel_path);
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read contract artifact {} ({}); run \
             `npm install && npx hardhat compile` in contracts/ethereum first",
            path.display(),
            e
        )
    });
    let json: serde_json::Value = serde_json::from_str(&raw).expect("artifact is JSON");
    let hex_code = json["bytecode"].as_str().expect("artifact has bytecode");
    hex::decode(hex_code.trim_start_matches("0x")).expect("bytecode is hex")
}

async fn deploy(provider: &DynProvider, bytecode: Vec<u8>, ctor_args: Vec<u8>) -> Address {
    let mut code = bytecode;
    code.extend_from_slice(&ctor_args);
    let tx = TransactionRequest::default().with_deploy_code(code);
    let receipt = provider
        .send_transaction(tx)
        .await
        .expect("deploy tx accepted")
        .get_receipt()
        .await
        .expect("deploy tx mined");
    assert!(receipt.status(), "deploy reverted");
    receipt
        .contract_address
        .expect("deploy receipt has address")
}

async fn call_u256(provider: &DynProvider, to: Address, input: Vec<u8>) -> U256 {
    let ret = provider
        .call(TransactionRequest::default().with_to(to).with_input(input))
        .await
        .expect("eth_call succeeds");
    U256::abi_decode(&ret).expect("uint256 return")
}

/// The live BTH-node environment for the full loop. `None` when any piece is
/// absent — the test then self-skips (never a false green).
struct BthEnv {
    rpc_url: String,
    reserve_view_key: String,
    reserve_spend_key: String,
    /// Reserve ML-KEM/ML-DSA BIP39 seed file (`bth.pq_seed_file`, issue #972).
    /// Required on the protocol-6.0.0 hybrid chain: every value output carries
    /// an ML-KEM ciphertext, so the reserve can only detect (and therefore
    /// spend) its own outputs when it holds the matching ML-KEM secret. `None`
    /// leaves a classical-only reserve (hybrid deposits warned, not detected).
    reserve_pq_seed: Option<String>,
    reserve_address: String,
    user_address: String,
    user_view_key: String,
    user_spend_key: String,
    /// User ML-KEM/ML-DSA BIP39 seed file, for scanning back the released
    /// hybrid stealth output (assertion 2) on the 6.0.0 chain.
    user_pq_seed: Option<String>,
    /// Amount to wrap (picocredits). The driver must fund the reserve with at
    /// least this much spendable factor-1 balance. Default 1 BTH.
    amount: u64,
}

fn non_empty(var: &str) -> Option<String> {
    std::env::var(var).ok().filter(|s| !s.is_empty())
}

fn bth_env() -> Option<BthEnv> {
    Some(BthEnv {
        rpc_url: non_empty("BRIDGE_BTH_RPC_URL")?,
        reserve_view_key: non_empty("BRIDGE_BTH_RESERVE_VIEW_KEY")?,
        reserve_spend_key: non_empty("BRIDGE_BTH_RESERVE_SPEND_KEY")?,
        // The PQ seeds are optional to preserve the classical-key skip contract,
        // but required for the loop to detect hybrid outputs on the 6.0.0 chain.
        reserve_pq_seed: non_empty("BRIDGE_BTH_RESERVE_PQ_SEED"),
        reserve_address: non_empty("BRIDGE_BTH_RESERVE_ADDRESS")?,
        user_address: non_empty("BRIDGE_BTH_USER_ADDRESS")?,
        user_view_key: non_empty("BRIDGE_BTH_USER_VIEW_KEY")?,
        user_spend_key: non_empty("BRIDGE_BTH_USER_SPEND_KEY")?,
        user_pq_seed: non_empty("BRIDGE_BTH_USER_PQ_SEED"),
        amount: non_empty("BRIDGE_BTH_AMOUNT")
            .and_then(|s| s.parse().ok())
            .unwrap_or(1_000_000_000_000),
    })
}

/// Drive `process_pending_orders` up to `ticks` times, stopping as soon as the
/// order reaches `target`. Returns the final observed status.
async fn drive_until(
    processor: &OrderProcessor,
    db: &Database,
    order_id: &Uuid,
    target: OrderStatus,
    ticks: usize,
) -> OrderStatus {
    let mut last = OrderStatus::AwaitingDeposit;
    for _ in 0..ticks {
        processor
            .process_pending_orders()
            .await
            .expect("processor tick");
        last = db
            .get_order(order_id)
            .expect("get_order")
            .expect("order present")
            .status;
        if last == target {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    last
}

/// wrap → mint → burn → release, all through the real engine, both chains live.
#[tokio::test]
#[ignore = "requires a local Botho node + hardhat/anvil (run scripts/bridge-e2e-full-loop.sh)"]
async fn full_loop_wrap_mint_burn_release() {
    let Some(bth) = bth_env() else {
        eprintln!(
            "SKIP: set BRIDGE_BTH_RPC_URL, BRIDGE_BTH_RESERVE_VIEW_KEY, \
             BRIDGE_BTH_RESERVE_SPEND_KEY, BRIDGE_BTH_RESERVE_ADDRESS, \
             BRIDGE_BTH_USER_ADDRESS, BRIDGE_BTH_USER_VIEW_KEY, \
             BRIDGE_BTH_USER_SPEND_KEY to run the full-loop e2e (a funded \
             factor-1 reserve on a live Botho node is required)"
        );
        return;
    };

    let url: alloy::transports::http::reqwest::Url = eth_rpc_url().parse().expect("valid RPC url");
    let deployer = dev_signer(0);
    let owner1 = dev_signer(1);
    let owner2 = dev_signer(2);
    let user = dev_signer(3);
    let user_addr = user.address();

    let deploy_provider = ProviderBuilder::new()
        .wallet(EthereumWallet::from(deployer.clone()))
        .connect_http(url.clone())
        .erased();

    let chain_id = match deploy_provider.get_chain_id().await {
        Ok(id) => id,
        Err(e) => {
            eprintln!(
                "SKIP: no Ethereum node reachable at {} ({e})",
                eth_rpc_url()
            );
            return;
        }
    };

    // ---- Deploy the 2-of-2 validator Safe and the wBTH token -------------
    let mut owners = vec![owner1.address(), owner2.address()];
    owners.sort();
    let safe_addr = deploy(
        &deploy_provider,
        artifact_bytecode("contracts/test/SafeStub.sol/SafeStub.json"),
        (owners.clone(), U256::from(2u8)).abi_encode_params(),
    )
    .await;
    let wbth_addr = deploy(
        &deploy_provider,
        artifact_bytecode("contracts/WrappedBTH.sol/WrappedBTH.json"),
        // admin / MINTER (the Safe!) / pauser
        (deployer.address(), safe_addr, deployer.address()).abi_encode_params(),
    )
    .await;

    // ---- Federation keys: secp256k1 (mint) + Ed25519 (release) -----------
    let ed_local = SigningKey::from_bytes(&[0x11u8; 32]); // this node
    let ed_peer = SigningKey::from_bytes(&[0x22u8; 32]); // peer validator

    let dir = tempfile::tempdir().expect("tempdir");
    let relayer_key_path = dir.path().join("relayer.hex");
    std::fs::write(&relayer_key_path, DEV_KEYS[0]).unwrap();
    let owner1_key_path = dir.path().join("owner1.hex");
    std::fs::write(&owner1_key_path, DEV_KEYS[1]).unwrap();
    let ed_local_key_path = dir.path().join("ed_local.hex");
    std::fs::write(&ed_local_key_path, hex::encode(ed_local.to_bytes())).unwrap();

    // ---- One BridgeConfig wiring BOTH chains -----------------------------
    let mut config = BridgeConfig::default();
    config.ethereum.rpc_url = eth_rpc_url();
    config.ethereum.chain_id = chain_id;
    config.ethereum.wbth_contract = format!("{:#x}", wbth_addr);
    config.ethereum.safe_address = Some(format!("{:#x}", safe_addr));
    config.ethereum.private_key_file = Some(relayer_key_path.to_string_lossy().into_owned());
    config.ethereum.confirmations_required = 1;
    config.ethereum.gas_price_strategy = GasPriceStrategy::Fixed(3);
    config.ethereum.mint_signers = vec![
        format!("{:#x}", owner1.address()),
        format!("{:#x}", owner2.address()),
    ];
    config.ethereum.mint_threshold = 2;

    config.bth = BthConfig {
        rpc_url: bth.rpc_url.clone(),
        ws_url: String::new(),
        view_key_file: Some(bth.reserve_view_key.clone()),
        spend_key_file: Some(bth.reserve_spend_key.clone()),
        // On protocol 6.0.0 every value output is hybrid, so the reserve needs
        // its ML-KEM secret to detect (and spend) its own factor-1 outputs
        // (issue #972). The driver provisions this from the node's own seed.
        pq_seed_file: bth.reserve_pq_seed.clone(),
        confirmations_required: 0,
        reserve_address: Some(bth.reserve_address.clone()),
        release_signers: vec![
            hex::encode(ed_local.verifying_key().to_bytes()),
            hex::encode(ed_peer.verifying_key().to_bytes()),
        ],
        release_threshold: 2,
        release_confirmations_required: 0,
    };

    config.bridge.db_path = dir.path().join("bridge.db").to_string_lossy().into_owned();
    config.bridge.attestation_secp256k1_key_file =
        Some(owner1_key_path.to_string_lossy().into_owned());
    config.bridge.attestation_ed25519_key_file =
        Some(ed_local_key_path.to_string_lossy().into_owned());
    // Drive the reconciler directly; no long-lived HTTP surface in the test.
    config.reserve.api_listen = String::new();

    // ---- Real engine wiring: minters, releaser, attestation, reconciler --
    let db = Database::open_in_memory().expect("db");
    db.migrate().expect("migrate");

    let eth_minter = EthMinter::new(config.ethereum.clone()).expect("eth minter builds");
    let mut minters: HashMap<Chain, Arc<dyn Minter>> = HashMap::new();
    minters.insert(Chain::Ethereum, Arc::new(eth_minter));

    let releaser = BthReleaser::new(config.bth.clone()).expect("bth releaser builds");

    let provider = Arc::new(
        FederationAttestationProvider::from_config(&config)
            .expect("valid federation config")
            .expect("federation configured (both eth mint + bth release)"),
    );

    let processor = OrderProcessor::new(
        config.clone(),
        db.clone(),
        minters,
        Some(Arc::new(releaser) as Arc<dyn Releaser>),
        provider.clone() as Arc<dyn AttestationProvider>,
    );

    let reconciler = Reconciler::from_config(&config, db.clone());

    // =====================================================================
    // WRAP leg: a confirmed BTH deposit → engine drives the wBTH mint.
    // (The BthWatcher produces the DepositConfirmed state from a real
    // on-chain deposit; here we insert that state and let the engine mint.)
    // =====================================================================
    let mut mint_order = BridgeOrder::new_mint(
        Chain::Ethereum,
        bth.amount,
        0, // keep the peg exact (fee accounting is covered by unit tests)
        bth.reserve_address.clone(),
        format!("{:#x}", user_addr),
    );
    mint_order.source_tx = Some("bth_deposit_confirmed_by_watcher".to_string());
    mint_order.set_status(OrderStatus::DepositConfirmed);
    db.insert_order(&mint_order).expect("insert mint order");

    // Assertion 4a — a SINGLE Safe-owner signature must NOT authorize a mint:
    // the engine self-attests owner1 (1/2), fails the threshold, and leaves
    // the order DepositConfirmed. No wBTH exists yet.
    processor
        .process_pending_orders()
        .await
        .expect("tick (below-threshold mint)");
    assert_eq!(
        db.get_order(&mint_order.id).unwrap().unwrap().status,
        OrderStatus::DepositConfirmed,
        "single signer must not authorize a mint (fail-safe)"
    );
    let pre_mint_supply = call_u256(
        &deploy_provider,
        wbth_addr,
        IWrappedBTHFullLoop::totalSupplyCall {}.abi_encode(),
    )
    .await;
    assert_eq!(pre_mint_supply, U256::ZERO, "no wBTH before the threshold");

    // Peer (owner 2) submits its EIP-712 envelope, bound to the fresh Safe
    // nonce (0) — the same #858 transport, injected directly here.
    let now = Utc::now().timestamp().max(0) as u64;
    let mint_kind = AttestationKind::MintWbth {
        dest_chain: Chain::Ethereum,
        dest_address: mint_order.dest_address.clone(),
        amount: mint_order.net_amount(),
        order_id: mint_order.id,
        source_tx: mint_order.source_tx.clone().unwrap(),
        safe_nonce: Some(0),
    };
    let mint_envelope = sign_attestation_secp256k1(
        &mint_kind,
        &owner2,
        chain_id,
        safe_addr,
        wbth_addr,
        &Uuid::new_v4().simple().to_string(),
        now,
        now + 300,
    )
    .expect("peer mint envelope signs");
    assert!(
        provider
            .submit_attestation(&mint_envelope, &mint_order)
            .accepted,
        "peer mint attestation must be accepted"
    );

    // Assertion 4b — now at threshold (2/2), the engine mints through the
    // Safe: DepositConfirmed → MintPending → Completed.
    let final_mint = drive_until(&processor, &db, &mint_order.id, OrderStatus::Completed, 40).await;
    assert_eq!(
        final_mint,
        OrderStatus::Completed,
        "engine must drive the mint to Completed"
    );
    assert!(final_mint.is_terminal());

    // Assertion 1 — wBTH minted == BTH locked (exact factor-1 peg, ADR 0003).
    let net = mint_order.net_amount();
    let balance = call_u256(
        &deploy_provider,
        wbth_addr,
        IWrappedBTHFullLoop::balanceOfCall { account: user_addr }.abi_encode(),
    )
    .await;
    assert_eq!(balance, U256::from(net), "wBTH balance == locked BTH");
    let supply = call_u256(
        &deploy_provider,
        wbth_addr,
        IWrappedBTHFullLoop::totalSupplyCall {}.abi_encode(),
    )
    .await;
    assert_eq!(supply, U256::from(net), "wBTH supply == locked BTH");

    // Assertion 3a — proof-of-reserves after mint: Σ(wBTH) == locked, zero
    // drift, and the live BTH node was actually consulted for custody.
    let proof_after_mint = reconciler.reconcile_once().await.expect("reconcile (mint)");
    assert_eq!(proof_after_mint.eth_supply, Some(net));
    assert_eq!(proof_after_mint.locked_reserve, net);
    assert_eq!(proof_after_mint.drift, 0, "Σ wBTH == locked reserve");
    assert!(proof_after_mint.in_tolerance, "peg within tolerance");
    assert!(
        proof_after_mint.reserve_balance_checked,
        "the live BTH reserve balance must be checked (custody leg)"
    );

    // =====================================================================
    // UNWRAP leg: user burns wBTH → engine drives the reserve release.
    // =====================================================================
    let user_provider = ProviderBuilder::new()
        .wallet(EthereumWallet::from(user.clone()))
        .connect_http(url)
        .erased();
    let burn_receipt = user_provider
        .send_transaction(
            TransactionRequest::default().with_to(wbth_addr).with_input(
                IWrappedBTHFullLoop::bridgeBurnCall {
                    amount: U256::from(net),
                    bthAddress: bth.user_address.clone(),
                }
                .abi_encode(),
            ),
        )
        .await
        .expect("burn tx accepted")
        .get_receipt()
        .await
        .expect("burn tx mined");
    assert!(burn_receipt.status(), "bridgeBurn reverted");

    // Detect the burn with the SAME transport the Ethereum watcher runs
    // (eth_getLogs + decode), then create the order the watcher would.
    let eth_client = AlloyEthClient::new(&config.ethereum).expect("watcher client");
    let tip = eth_client.latest_block().await.expect("eth tip");
    let events = eth_client.burn_events(0, tip).await.expect("burn scan");
    let ordered = with_tx_ordinals(events);
    let (event, _ordinal) = ordered
        .iter()
        .find(|(e, _)| e.tx_hash == format!("{:#x}", burn_receipt.transaction_hash))
        .expect("watcher sees the burn");
    assert_eq!(event.amount, net, "burn amount is exact picocredits");
    assert_eq!(event.bth_address, bth.user_address);

    let mut burn_order = BridgeOrder::new_burn(
        Chain::Ethereum,
        event.amount,
        0,
        event.from.clone(),
        event.bth_address.clone(),
        event.tx_hash.clone(),
    );
    burn_order.set_status(OrderStatus::BurnConfirmed);
    db.insert_order(&burn_order).expect("insert burn order");

    // Assertion 4c — a SINGLE Ed25519 signature must NOT authorize a release:
    // the engine self-attests the local key (1/2) and leaves the order
    // BurnConfirmed. No reserve funds move.
    processor
        .process_pending_orders()
        .await
        .expect("tick (below-threshold release)");
    assert_eq!(
        db.get_order(&burn_order.id).unwrap().unwrap().status,
        OrderStatus::BurnConfirmed,
        "single signer must not authorize a release (fail-safe)"
    );

    // Peer validator submits its Ed25519 release envelope (bound to this
    // exact order id / amount / recipient).
    let release_kind = FederationAttestationProvider::release_kind_for_test(&burn_order);
    let release_envelope = sign_attestation_ed25519(
        &release_kind,
        &ed_peer,
        &Uuid::new_v4().simple().to_string(),
        now,
        now + 300,
    )
    .expect("peer release envelope signs");
    assert!(
        provider
            .submit_attestation(&release_envelope, &burn_order)
            .accepted,
        "peer release attestation must be accepted"
    );

    // Assertion 4d — now at threshold (2/2), the engine releases through the
    // live BthReleaser: BurnConfirmed → ReleasePending → Released.
    let final_release =
        drive_until(&processor, &db, &burn_order.id, OrderStatus::Released, 60).await;
    assert_eq!(
        final_release,
        OrderStatus::Released,
        "engine must drive the release to Released (requires a funded factor-1 reserve)"
    );
    assert!(final_release.is_terminal());

    // Assertion 2 — the burn released the correct BTH to a FRESH stealth
    // output that the USER's own view key scans back off the live node
    // (ADR 0004 one-time output), and it is distinct from the EVM burn tx.
    let user_client = NodeBthClient::new(BthConfig {
        rpc_url: bth.rpc_url.clone(),
        ws_url: String::new(),
        view_key_file: Some(bth.user_view_key.clone()),
        spend_key_file: Some(bth.user_spend_key.clone()),
        // The user scans back a hybrid stealth output (ADR 0004) on the 6.0.0
        // chain, so it likewise needs its ML-KEM secret to see the release.
        pq_seed_file: bth.user_pq_seed.clone(),
        confirmations_required: 0,
        reserve_address: Some(bth.user_address.clone()),
        release_signers: Vec::new(),
        release_threshold: 0,
        release_confirmations_required: 0,
    })
    .expect("user scan client");
    let user_tip = user_client.tip_height().await.expect("bth tip");
    let scan_start = user_tip.saturating_sub(200);
    let mut released_output = None;
    for height in scan_start..=user_tip {
        if let Some(block) = user_client.block_at(height).await.expect("scan block") {
            if let Some(dep) = block.deposits.iter().find(|d| d.amount == net) {
                released_output = Some(dep.clone());
                break;
            }
        }
    }
    let released_output =
        released_output.expect("user must scan back the fresh released stealth output");
    assert_eq!(
        released_output.amount, net,
        "released amount == net burn amount"
    );
    assert_ne!(
        released_output.tx_hash,
        burn_order.source_tx.clone().unwrap(),
        "the BTH release output is a fresh on-chain tx, unlinkable to the EVM burn"
    );

    // Assertion 3b — proof-of-reserves after the unwrap: wBTH supply back to
    // zero, backing unlocked, zero drift. The loop closed with the peg exact.
    let final_supply = call_u256(
        &deploy_provider,
        wbth_addr,
        IWrappedBTHFullLoop::totalSupplyCall {}.abi_encode(),
    )
    .await;
    assert_eq!(final_supply, U256::ZERO, "sum(mints) - sum(burns) == 0");

    let proof_after_release = reconciler
        .reconcile_once()
        .await
        .expect("reconcile (release)");
    assert_eq!(proof_after_release.eth_supply, Some(0), "wBTH burned");
    assert_eq!(
        proof_after_release.locked_reserve, 0,
        "backing unlocked after release"
    );
    assert_eq!(
        proof_after_release.drift, 0,
        "peg exact after the round trip"
    );

    eprintln!(
        "full loop OK: wrapped {net} pc → wBTH minted (2/2 EIP-712) → burned → \
         released to a fresh stealth output (2/2 Ed25519); peg returned to 0"
    );
}
