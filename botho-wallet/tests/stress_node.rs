//! Offline signer against the production loopback RPC, mempool and ledger.
//! Fixture PoW is trivial; public consensus/network performance is not
//! asserted.
#![cfg(all(unix, feature = "pq"))]
use botho::{
    block::{Block, BlockHeader, BlockLotterySummary, MintingTx},
    consensus::{BlockBuilder, LotteryFeeConfig},
    ledger::Ledger,
    mempool::Mempool,
    rpc::{RpcState, WsBroadcaster},
    transaction::PICOCREDITS_PER_CREDIT,
};
use botho_wallet::stress::{execute, Request};
use bth_account_keys::PublicAddress;
use bth_transaction_types::constants::Network;
use serde_json::{json, Value};
use std::{
    os::unix::fs::PermissionsExt,
    sync::Arc,
    time::{Duration, SystemTime},
};
async fn rpc(url: &str, method: &str, params: Value) -> Value {
    let body: Value = reqwest::Client::new()
        .post(url)
        .json(&json!({"jsonrpc":"2.0",
        "id":1,"method":method,"params":params}))
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(body.get("error").is_none(), "{method}: {body}");
    body["result"].clone()
}
fn adapter(value: Value) -> Value {
    execute(serde_json::from_value::<Request>(value).unwrap()).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn receive_prepare_submit_restore_and_respend() {
    tokio::time::timeout(Duration::from_secs(240),async {
        let dir=tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(),std::fs::Permissions::from_mode(0o700)).unwrap();
        let a=dir.path().join("a.mnemonic"); let b=dir.path().join("b.mnemonic");
        let address_a=adapter(json!({"operation":"generate","wallet":a}))["address"].clone();
        let address_b=adapter(json!({"operation":"generate","wallet":b}))["address"].clone();
        let faucet_keys=botho_wallet::WalletKeys::generate().unwrap();
        let faucet_wallet=botho::wallet::Wallet::from_mnemonic(faucet_keys.mnemonic_phrase()).unwrap();
        let sink=botho_wallet::WalletKeys::generate().unwrap();
        let ledger=Ledger::open(&dir.path().join("ledger")).unwrap();
        ledger.set_difficulty(TRIVIAL_DIFFICULTY).unwrap();
        for height in 1..=220 {
            let address=if height==100 {faucet_keys.public_address()} else {sink.public_address()};
            mine_block(&ledger,&address,vec![]);
        }
        let state=Arc::new(RpcState::new(ledger,Mempool::new(),Network::Testnet,None,None,
            vec!["*".to_owned()],Arc::new(WsBroadcaster::new(100))).with_faucet(
                botho::rpc::FaucetState::new(botho::config::FaucetConfig {enabled:true,
                    amount:1_000_000_000_000, ..Default::default()}),faucet_wallet));
        let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr=listener.local_addr().unwrap(); drop(listener);
        let server_state=state.clone();
        let server=tokio::spawn(async move {botho::rpc::start_rpc_server(addr,server_state).await.unwrap()});
        let url=format!("http://{addr}/rpc");
        tokio::time::sleep(Duration::from_millis(100)).await;
        let grant=rpc(&url,"faucet_request",json!({"address":address_a})).await;
        let grant_hash: [u8;32]=hex::decode(grant["txHash"].as_str().unwrap()).unwrap().try_into().unwrap();
        let grant_tx=state.mempool.read().unwrap().get(&grant_hash).unwrap().clone();
        mine_block(&state.ledger.read().unwrap(),&sink.public_address(),vec![grant_tx]);
        state.mempool.write().unwrap().remove_tx(&grant_hash);
        for _ in 0..120 {mine_block(&state.ledger.read().unwrap(),&sink.public_address(),vec![]);}
        let mut fees=0u64;
        for (round,sender,recipient) in [(0,&a,&address_b),(1,&b,&address_a)] {
            let height=state.ledger.read().unwrap().get_chain_state().unwrap().height;
            let blocks=rpc(&url,"chain_getOutputs",json!({"start_height":0,"end_height":height})).await;
            let scan=adapter(json!({"operation":"scan","wallet":sender,"blocks":blocks}));
            let owned=scan["owned"].as_array().unwrap(); assert!(!owned.is_empty());
            let images:Vec<_>=owned.iter().map(|u|u["key_image"].clone()).collect();
            let spent=rpc(&url,"chain_areKeyImagesSpent",json!({"keyImages":images})).await;
            let restored=dir.path().join(format!("restore-{round}.mnemonic"));
            std::fs::copy(sender,&restored).unwrap();
            let check=adapter(json!({"operation":"restore_check","wallet":restored,
                "blocks":blocks,"expected_address":scan["address"]}));
            assert_eq!(check,scan);
            let artifact=dir.path().join(format!("payment-{round}.bin"));
            let prepared=adapter(json!({"operation":"prepare","wallet":restored,"blocks":blocks,
                "height":height,"selected":[owned[0]["id"]],"spent":spent,"reserved":[],
                "recipient":recipient,"allowed_recipients":[address_a,address_b],
                "amount":if round==0 {30_000_000_000u64} else {10_000_000_000u64},"base_rate":1,"artifact":artifact}));
            let bytes=std::fs::read(&artifact).unwrap();
            let inspected=adapter(json!({"operation":"inspect","artifact":artifact}));
            assert_eq!(inspected["hash"],prepared["hash"]);
            let tx:botho::transaction::Transaction=bincode::deserialize(&bytes).unwrap();
            // Preparing has not submitted anything.
            let before=rpc(&url,"getTransactionStatus",json!({"hash":prepared["hash"]})).await;
            assert_eq!(before["status"],"unknown");
            let submitted=rpc(&url,"tx_submit",json!({"tx_hex":hex::encode(bytes)})).await;
            assert_eq!(submitted["txHash"],prepared["hash"]);
            fees+=prepared["fee"].as_u64().unwrap();
            {
                let ledger=state.ledger.read().unwrap();
                mine_block(&ledger,&sink.public_address(),vec![tx.clone()]);
            }
            state.mempool.write().unwrap().remove_tx(&tx.hash());
            let receipt=rpc(&url,"getTransactionStatus",json!({"hash":prepared["hash"]})).await;
            assert_eq!(receipt["confirmed"],true);
            let spent_after=rpc(&url,"chain_areKeyImagesSpent",json!({"keyImages":prepared["key_images"]})).await;
            assert!(spent_after.as_array().unwrap().iter().all(|s|s["spent"]==true));
            let new_height=state.ledger.read().unwrap().get_chain_state().unwrap().height;
            let all=rpc(&url,"chain_getOutputs",json!({"start_height":0,"end_height":new_height})).await;
            let last=rpc(&url,"chain_getOutputs",json!({"start_height":height+1,"end_height":new_height})).await;
            let mut balance=0u64;
            for wallet in [&a,&b] {
                let full=adapter(json!({"operation":"scan","wallet":wallet,"blocks":all}));
                let previous=adapter(json!({"operation":"scan","wallet":wallet,"blocks":blocks}));
                let incremental=adapter(json!({"operation":"scan","wallet":wallet,"blocks":last}));
                let mut merged=previous["owned"].as_array().unwrap().clone();
                merged.extend(incremental["owned"].as_array().unwrap().clone());
                assert_eq!(json!(merged),full["owned"]);
                let all_images:Vec<_>=merged.iter().map(|u|u["key_image"].clone()).collect();
                let spent=rpc(&url,"chain_areKeyImagesSpent",json!({"keyImages":all_images})).await;
                for (o,s) in merged.iter().zip(spent.as_array().unwrap()) {
                    if s["spent"]==false {balance+=o["utxo"]["amount"].as_u64().unwrap();}
                }
            }
            // Coinbase reward after lottery diversion is read from the scanner.
            let initial=adapter(json!({"operation":"scan","wallet":a,"blocks":blocks}));
            let opening=initial["owned"][0]["utxo"]["amount"].as_u64().unwrap();
            assert_eq!(balance+fees,opening,"exact accounting after round {round}");
            if round==0 {for _ in 0..120 {mine_block(&state.ledger.read().unwrap(),&sink.public_address(),vec![]);}}
        }
        server.abort();
    }).await.expect("fixture exceeded four minute deadline");
}
const TEST_BLOCK_REWARD: u64 = 50 * PICOCREDITS_PER_CREDIT;
const TRIVIAL_DIFFICULTY: u64 = u64::MAX;
/// Create a minting transaction for testing with trivial PoW.
fn create_mock_minting_tx(
    height: u64,
    reward: u64,
    minter_address: &PublicAddress,
    prev_block_hash: [u8; 32],
) -> MintingTx {
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let mut minting_tx = MintingTx::new(
        height,
        reward,
        minter_address,
        prev_block_hash,
        TRIVIAL_DIFFICULTY,
        timestamp,
    );

    for nonce in 0..100_000 {
        minting_tx.nonce = nonce;
        if minting_tx.verify_pow() {
            break;
        }
    }

    minting_tx
}

/// Apply the lottery fee-split / draw to a block so it satisfies
/// `validate_block_lottery` in `add_block` (mirrors the production proposer
/// path). For our funding blocks there are no fees, but the lottery emission
/// share still has to be accounted for.
fn apply_lottery_to_block(block: Block, ledger: &Ledger) -> Block {
    let total_fees: u64 = block.transactions.iter().map(|tx| tx.fee).sum();
    let emission_share = block.minting_tx.lottery_emission_share();
    let lottery_config = LotteryFeeConfig::default();

    let stored_pool = ledger.get_lottery_pool().unwrap_or(0);
    let candidates = ledger
        .get_lottery_validation_candidates(
            block.height(),
            &block.header.prev_block_hash,
            &lottery_config.draw_config,
        )
        .unwrap_or_default();

    if total_fees == 0 && emission_share == 0 && stored_pool == 0 {
        return block;
    }

    let utxo_lookup = |utxo_id: &[u8; 36]| ledger.get_utxo_by_id(utxo_id).ok().flatten();
    BlockBuilder::apply_lottery(
        block,
        &candidates,
        stored_pool,
        utxo_lookup,
        &lottery_config,
    )
}

/// Mine a single block crediting `minter_address` and append it to the ledger.
fn mine_block(
    ledger: &Ledger,
    minter_address: &PublicAddress,
    transactions: Vec<botho::transaction::Transaction>,
) {
    let state = ledger.get_chain_state().expect("Failed to get chain state");
    let prev_block = ledger.get_tip().expect("Failed to get tip");
    let prev_hash = prev_block.hash();
    let height = state.height + 1;

    let minting_tx = create_mock_minting_tx(height, TEST_BLOCK_REWARD, minter_address, prev_hash);

    let block = Block {
        header: BlockHeader {
            version: 1,
            prev_block_hash: prev_hash,
            tx_root: Block::compute_tx_root(&transactions),
            timestamp: minting_tx.timestamp,
            height: minting_tx.block_height,
            difficulty: minting_tx.difficulty,
            nonce: minting_tx.nonce,
            minter_view_key: minting_tx.minter_view_key,
            minter_spend_key: minting_tx.minter_spend_key,
        },
        minting_tx,
        transactions,
        lottery_outputs: Vec::new(),
        lottery_summary: BlockLotterySummary::default(),
    };

    let block = apply_lottery_to_block(block, ledger);
    ledger
        .add_block(&block)
        .expect("Failed to add funding block");
}
