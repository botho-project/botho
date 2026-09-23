//! Positive isolated chain/RPC/thin-wallet acceptance. No public network,
//! faucet substitute, consensus/index rewrite, or legacy lottery recovery.
use botho::{
    block::{Block, BlockHeader, BlockLotterySummary, MintingTx, MINTING_OUTPUT_INDEX},
    consensus::{BlockBuilder, LotteryFeeConfig},
    ledger::Ledger,
    mempool::Mempool,
    rpc::{RpcState, WsBroadcaster},
    transaction::{Transaction, UtxoId, MIN_RING_SIZE, PICOCREDITS_PER_CREDIT as COIN},
};
use botho_wallet::{
    rpc_pool::BlockOutputs,
    transaction::{to_tx_hex, OwnedUtxo, TransactionBuilder, WalletScanner},
    WalletKeys,
};
use bth_account_keys::PublicAddress;
use bth_crypto_ring_signature::KeyImage;
use bth_transaction_types::constants::Network;
use bth_util_from_random::OsRng;
use serde_json::{json, Value};
use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

fn mine(ledger: &Ledger, address: &PublicAddress, transactions: Vec<Transaction>) -> Block {
    let tip = ledger.get_tip().unwrap();
    let mut mint = MintingTx::new(
        tip.height() + 1,
        50 * COIN,
        address,
        tip.hash(),
        u64::MAX,
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    );
    for nonce in 0..100_000 {
        mint.nonce = nonce;
        if mint.verify_pow() {
            break;
        }
    }
    assert!(mint.verify_pow());
    let block = Block {
        header: BlockHeader {
            version: 1,
            prev_block_hash: tip.hash(),
            tx_root: Block::compute_tx_root(&transactions),
            timestamp: mint.timestamp,
            height: mint.block_height,
            difficulty: mint.difficulty,
            nonce: mint.nonce,
            minter_view_key: mint.minter_view_key,
            minter_spend_key: mint.minter_spend_key,
        },
        minting_tx: mint,
        transactions,
        lottery_outputs: vec![],
        lottery_summary: BlockLotterySummary::default(),
    };
    let config = LotteryFeeConfig::default();
    let candidates = ledger
        .get_lottery_validation_candidates(
            block.height(),
            &block.header.prev_block_hash,
            &config.draw_config,
        )
        .unwrap();
    let block = BlockBuilder::apply_lottery(
        block,
        &candidates,
        ledger.get_lottery_pool().unwrap(),
        |id| ledger.get_utxo_by_id(id).unwrap(),
        &config,
    );
    ledger.add_block(&block).unwrap();
    block
}
async fn rpc(url: &str, method: &str, params: Value) -> Value {
    let body: Value = reqwest::Client::new()
        .post(url)
        .json(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(body.get("error").is_none(), "{method}: {body}");
    body["result"].clone()
}
async fn outputs(url: &str, end: u64) -> (Value, Vec<BlockOutputs>) {
    let raw = rpc(
        url,
        "chain_getOutputs",
        json!({"start_height":0,"end_height":end}),
    )
    .await;
    let decoded = serde_json::from_value(raw.clone()).unwrap();
    (raw, decoded)
}
fn image(keys: &WalletKeys, output: &OwnedUtxo) -> String {
    hex::encode(
        KeyImage::from(
            &output
                .recover_spend_key(keys)
                .expect("actual wallet recovery"),
        )
        .as_bytes(),
    )
}
struct Server(tokio::task::JoinHandle<()>);
impl Drop for Server {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hybrid_coinbase_rpc_scan_spend_filter_restore() {
    let began = Instant::now();
    let dir = tempfile::tempdir().unwrap();
    let miner = WalletKeys::generate().unwrap();
    let recipient = WalletKeys::generate().unwrap();
    let sink = WalletKeys::generate().unwrap();
    let ledger = Ledger::open(dir.path()).unwrap();
    let mut reward = None;
    for h in 1..=220 {
        let b = mine(
            &ledger,
            &if h == 100 {
                miner.public_address()
            } else {
                sink.public_address()
            },
            vec![],
        );
        if h == 100 {
            reward = Some(b);
        }
    }
    let reward = reward.unwrap();
    let actual = ledger
        .get_utxo(&UtxoId::new(reward.hash(), 0))
        .unwrap()
        .unwrap();
    let state = Arc::new(RpcState::new(
        ledger,
        Mempool::new(),
        Network::Testnet,
        None,
        None,
        vec!["*".into()],
        Arc::new(WsBroadcaster::new(100)),
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let s = state.clone();
    let _server = Server(tokio::spawn(async move {
        botho::rpc::start_rpc_server(addr, s).await.unwrap();
    }));
    tokio::time::sleep(Duration::from_millis(100)).await;
    let url = format!("http://{addr}");
    let (raw, blocks) = outputs(&url, 220).await;
    let row = &raw
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["height"] == 100)
        .unwrap()["outputs"][0];
    assert_eq!(row["txHash"], hex::encode(reward.minting_tx.hash()));
    assert_eq!(row["outputIndex"], u32::MAX);
    assert_eq!(row["coinbase"], true);
    assert_eq!(row["cryptoOutputIndex"], MINTING_OUTPUT_INDEX);
    assert_eq!(
        row["ledgerOutpoint"],
        json!({"txHash":hex::encode(reward.hash()),"outputIndex":0})
    );
    assert_eq!(row["targetKey"], hex::encode(actual.output.target_key));
    assert_eq!(row["publicKey"], hex::encode(actual.output.public_key));
    assert_eq!(
        row["kemCiphertext"],
        hex::encode(actual.output.kem_ciphertext.as_ref().unwrap())
    );
    let scanner = WalletScanner::new(&miner);
    let found = scanner.scan_outputs(&blocks);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].output_index, u32::MAX);
    assert!(found[0].coinbase);
    assert_eq!(found[0].crypto_output_index, Some(0));
    // Old node shape: retain its existing coinbase discriminator, omit additive
    // fields.
    let mut old = raw.clone();
    for b in old.as_array_mut().unwrap() {
        for o in b["outputs"].as_array_mut().unwrap() {
            o.as_object_mut().unwrap().remove("cryptoOutputIndex");
            o.as_object_mut().unwrap().remove("ledgerOutpoint");
        }
    }
    let old_found =
        scanner.scan_outputs(&serde_json::from_value::<Vec<BlockOutputs>>(old).unwrap());
    assert_eq!(old_found.len(), 1);
    assert_eq!(old_found[0].id(), found[0].id());
    assert_eq!(image(&miner, &old_found[0]), image(&miner, &found[0]));
    // Ambiguous old caches are loadable but require a fresh RPC scan, not guessing.
    let mut old_cache = serde_json::to_value(&found[0]).unwrap();
    old_cache
        .as_object_mut()
        .unwrap()
        .remove("crypto_output_index");
    old_cache.as_object_mut().unwrap().remove("coinbase");
    assert!(serde_json::from_value::<OwnedUtxo>(old_cache)
        .unwrap()
        .recover_spend_key(&miner)
        .is_none());
    let key_image = image(&miner, &found[0]);
    let spent = rpc(
        &url,
        "chain_areKeyImagesSpent",
        json!({"keyImages":[key_image]}),
    )
    .await;
    assert_eq!(spent[0]["spent"], false);
    let mut selected = found[0].clone();
    let mut expected_recipient = 0u64;
    let mut total_fees = 0u64;
    for round in 0..2 {
        let height = state
            .ledger
            .read()
            .unwrap()
            .get_chain_state()
            .unwrap()
            .height;
        let (_, blocks) = outputs(&url, height).await;
        // Recreate keys and cached owned record before actual signing, including
        // nonzero change index on round2.
        let restored = WalletKeys::from_mnemonic(miner.mnemonic_phrase()).unwrap();
        selected = serde_json::from_slice(&serde_json::to_vec(&selected).unwrap()).unwrap();
        if round == 1 {
            assert_eq!(selected.output_index, 1);
            assert_eq!(selected.crypto_output_index, Some(1));
            assert!(!selected.coinbase);
        }
        let age = height - selected.created_at;
        let (min_age, max_age) = botho_wallet::decoy_selection::age_similarity_band(age);
        let eligible: Vec<_> = blocks
            .into_iter()
            .filter(|b| {
                b.height >= height.saturating_sub(max_age)
                    && b.height <= height.saturating_sub(min_age).saturating_add(1)
            })
            .collect();
        let decoys = botho_wallet::ring_builder::select_rpc_decoy_pool(
            &eligible,
            &[selected.target_key],
            MIN_RING_SIZE - 1,
            min_age,
            max_age,
            &mut OsRng,
        )
        .unwrap();
        let total = selected.amount;
        let selected_image = image(&restored, &selected);
        let builder = TransactionBuilder::new(restored, vec![selected.clone()], height);
        let transfer = builder
            .build_signed_transaction(
                &recipient.public_address(),
                COIN,
                10 * COIN,
                vec![selected.clone()],
                total,
                vec![decoys],
            )
            .unwrap();
        assert_eq!(
            hex::encode(transfer.transaction.key_images()[0]),
            selected_image
        );
        assert_eq!(
            transfer
                .transaction
                .outputs
                .iter()
                .map(|o| o.amount)
                .sum::<u64>()
                + transfer.actual_fee,
            total
        );
        let submission = rpc(
            &url,
            "tx_submit",
            json!({"tx_hex":to_tx_hex(&transfer.transaction).unwrap()}),
        )
        .await;
        assert_eq!(
            submission["txHash"],
            hex::encode(transfer.transaction.hash())
        );
        assert!(state
            .mempool
            .read()
            .unwrap()
            .get_transactions(10)
            .iter()
            .any(|t| t.hash() == transfer.transaction.hash()));
        let accepted = mine(
            &state.ledger.read().unwrap(),
            &sink.public_address(),
            vec![transfer.transaction.clone()],
        );
        state
            .mempool
            .write()
            .unwrap()
            .remove_confirmed(&[transfer.transaction.clone()]);
        let spent = rpc(
            &url,
            "chain_areKeyImagesSpent",
            json!({"keyImages":[selected_image]}),
        )
        .await;
        assert_eq!(spent[0]["spent"], true);
        total_fees += transfer.actual_fee;
        expected_recipient += COIN;
        let (_, blocks) = outputs(&url, accepted.height()).await;
        let scan = WalletScanner::new(&miner).scan_outputs(&blocks);
        let images: Vec<_> = scan.iter().map(|o| image(&miner, o)).collect();
        let states = rpc(&url, "chain_areKeyImagesSpent", json!({"keyImages":images})).await;
        let spendable: Vec<_> = scan
            .into_iter()
            .zip(states.as_array().unwrap())
            .filter(|(_, s)| s["spent"] == false)
            .map(|(o, _)| o)
            .collect();
        assert_eq!(spendable.len(), 1);
        selected = spendable[0].clone();
        assert_eq!(
            selected.amount + expected_recipient + total_fees,
            actual.output.amount
        );
        let received = WalletScanner::new(&recipient).scan_outputs(&blocks);
        assert_eq!(
            received.iter().map(|o| o.amount).sum::<u64>(),
            expected_recipient
        );
        eprintln!("COINBASE_RPC_ACCEPTED round={} height={} tx={} image={} change={} recipient={} fees={}",round+1,accepted.height(),hex::encode(transfer.transaction.hash()),selected_image,selected.amount,expected_recipient,total_fees);
        if round == 0 {
            for _ in 0..120 {
                mine(
                    &state.ledger.read().unwrap(),
                    &sink.public_address(),
                    vec![],
                );
            }
        }
    }
    eprintln!(
        "COINBASE_RPC_PASS elapsed_ms={} canonical={} legacy={} index={}",
        began.elapsed().as_millis(),
        hex::encode(reward.hash()),
        hex::encode(reward.minting_tx.hash()),
        u32::MAX
    );
}
