// Copyright (c) 2024 Botho Foundation
//
//! End-to-end Faucet Workflow Integration Tests
//!
//! Tests the complete testnet workflow from faucet request through transaction
//! confirmation. Validates:
//! - Faucet dispenses correct amount (10 BTH)
//! - Rate limiting works as configured
//! - Transactions confirm within expected time
//! - Multi-node consensus agrees on faucet transactions
//! - Errors are user-friendly and actionable

mod common;

use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, SystemTime},
};

use reqwest::Client;
use serde_json::{json, Value};
use serial_test::serial;
use tempfile::TempDir;
use tokio::net::TcpListener;

use botho::{
    address::Address,
    block::{Block, BlockHeader, BlockLotterySummary, MintingTx},
    config::FaucetConfig,
    consensus::{BlockBuilder, LotteryFeeConfig},
    ledger::Ledger,
    mempool::Mempool,
    rpc::{FaucetState, RpcState, WsBroadcaster},
    transaction::PICOCREDITS_PER_CREDIT,
    wallet::Wallet,
};
use bth_account_keys::PublicAddress;
use bth_transaction_types::constants::Network;

// ============================================================================
// Test Helpers
// ============================================================================

/// Block reward minted per block during ledger funding (50 BTH).
const TEST_BLOCK_REWARD: u64 = 50 * PICOCREDITS_PER_CREDIT;

/// Trivial PoW difficulty for instant mining.
///
/// Must equal the chain's initial difficulty — block acceptance enforces
/// `header.difficulty == chain.difficulty` (audit cycle 6, C1).
const TRIVIAL_DIFFICULTY: u64 = u64::MAX;

/// Number of independent coinbase UTXOs minted to the faucet wallet.
///
/// The faucet skips UTXOs whose key image is pending in the mempool, so a
/// previously dispensed (still-unconfirmed) change output cannot be respent.
/// Funding the faucet with several distinct coinbase UTXOs therefore lets a
/// test make multiple sequential requests without mining between them — each
/// request simply consumes a different UTXO. The per-address daily limit (3)
/// is the largest sequential-success count any test needs, so 6 leaves slack.
const FAUCET_COINBASE_UTXOS: usize = 6;

/// Number of decoy outputs mined to a dedicated decoy wallet so CLSAG ring
/// signatures have a large enough confirmed pool (ring size 20, decoys need
/// 10 confirmations).
const DECOY_BLOCKS: usize = 30;

/// Default faucet configuration for tests
fn test_faucet_config() -> FaucetConfig {
    FaucetConfig {
        enabled: true,
        amount: 10_000_000_000_000, // 10 BTH in picocredits
        per_ip_hourly_limit: 5,
        per_address_daily_limit: 3,
        daily_limit: 1_000_000_000_000_000, // 1000 BTH
        cooldown_secs: 1,                   // Short cooldown for testing
    }
}

/// Create a deterministic wallet from a seed using valid 24-word BIP39
/// mnemonics, so tests are reproducible.
fn create_wallet(seed: u32) -> Wallet {
    let mnemonics = [
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art",
        "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo vote",
        "legal winner thank year wave sausage worth useful legal winner thank year wave sausage worth useful legal winner thank year wave sausage worth title",
        "letter advice cage absurd amount doctor acoustic avoid letter advice cage absurd amount doctor acoustic avoid letter advice cage absurd amount doctor acoustic bless",
    ];
    let idx = (seed as usize) % mnemonics.len();
    Wallet::from_mnemonic(mnemonics[idx]).expect("Failed to create wallet from mnemonic")
}

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
fn mine_block(ledger: &Ledger, minter_address: &PublicAddress) {
    let state = ledger.get_chain_state().expect("Failed to get chain state");
    let prev_block = ledger.get_tip().expect("Failed to get tip");
    let prev_hash = prev_block.hash();
    let height = state.height + 1;

    let minting_tx = create_mock_minting_tx(height, TEST_BLOCK_REWARD, minter_address, prev_hash);

    let block = Block {
        header: BlockHeader {
            version: 1,
            prev_block_hash: prev_hash,
            tx_root: [0u8; 32],
            timestamp: minting_tx.timestamp,
            height: minting_tx.block_height,
            difficulty: minting_tx.difficulty,
            nonce: minting_tx.nonce,
            minter_view_key: minting_tx.minter_view_key,
            minter_spend_key: minting_tx.minter_spend_key,
        },
        minting_tx,
        transactions: Vec::new(),
        lottery_outputs: Vec::new(),
        lottery_summary: BlockLotterySummary::default(),
    };

    let block = apply_lottery_to_block(block, ledger);
    ledger
        .add_block(&block)
        .expect("Failed to add funding block");
}

/// Fund a faucet wallet in a fresh ledger.
///
/// Mines several coinbase blocks to the faucet wallet (independent spendable
/// UTXOs) and a larger batch of decoy blocks to a separate wallet so CLSAG
/// ring signatures have enough confirmed decoy outputs. The decoy blocks are
/// mined first so they comfortably exceed the 10-confirmation decoy minimum
/// by the time a faucet request builds a transaction.
fn fund_faucet_ledger(ledger: &Ledger) -> Wallet {
    let faucet_wallet = create_wallet(1);
    let faucet_address = faucet_wallet.default_address();

    // Decoy pool first (seed 0 is dedicated to decoys to avoid collisions).
    let decoy_address = create_wallet(0).default_address();
    for _ in 0..DECOY_BLOCKS {
        mine_block(ledger, &decoy_address);
    }

    // Independent coinbase UTXOs for the faucet wallet.
    for _ in 0..FAUCET_COINBASE_UTXOS {
        mine_block(ledger, &faucet_address);
    }

    faucet_wallet
}

/// Spawn an RPC server with a funded faucet using the default test config.
async fn spawn_faucet_rpc_server() -> (TempDir, SocketAddr, tokio::task::JoinHandle<()>) {
    spawn_faucet_rpc_server_with_config(test_faucet_config()).await
}

/// Spawn an RPC server with a funded faucet using a caller-supplied config.
///
/// All requests over the local HTTP loopback originate from `127.0.0.1`, and
/// the JSON-RPC handler treats every faucet request as coming from that single
/// IP. Tests that need to exercise concurrency or per-address limits in
/// isolation therefore tune `cooldown_secs` / `per_ip_hourly_limit` here so
/// the per-IP cooldown is not the (shared-IP) bottleneck.
async fn spawn_faucet_rpc_server_with_config(
    config: FaucetConfig,
) -> (TempDir, SocketAddr, tokio::task::JoinHandle<()>) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let ledger_path = temp_dir.path().join("ledger");

    let ledger = Ledger::open(&ledger_path).expect("Failed to create ledger");
    // RandomX genesis difficulty is real-hashrate sized; pin the chain to
    // the trivial target so test PoW solves in one hash and the C1
    // block-apply difficulty check accepts it.
    ledger.set_difficulty(TRIVIAL_DIFFICULTY).unwrap();
    let faucet_wallet = fund_faucet_ledger(&ledger);
    let mempool = Mempool::new();
    let ws_broadcaster = Arc::new(WsBroadcaster::new(100));

    let state = RpcState::new(
        ledger,
        mempool,
        Network::Testnet,
        None,
        None,
        vec!["*".to_string()],
        ws_broadcaster,
    )
    .with_faucet(FaucetState::new(config), faucet_wallet);

    let state = Arc::new(state);

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind");
    let addr = listener.local_addr().expect("Failed to get local addr");
    drop(listener);

    let state_clone = state.clone();
    let handle = tokio::spawn(async move {
        if let Err(e) = botho::rpc::start_rpc_server(addr, state_clone).await {
            tracing::debug!("RPC server stopped: {}", e);
        }
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    (temp_dir, addr, handle)
}

/// Create a JSON-RPC request body
fn rpc_request(method: &str, params: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
        "id": 1
    })
}

/// Make an RPC call and return the response
async fn rpc_call(client: &Client, addr: SocketAddr, method: &str, params: Value) -> Value {
    let url = format!("http://{}", addr);
    let body = rpc_request(method, params);

    let response = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .expect("Failed to send request");

    response
        .json::<Value>()
        .await
        .expect("Failed to parse response")
}

/// Generate a valid testnet recipient address string for the faucet.
///
/// Builds a deterministic, *distinct* wallet for each `seed` (so per-address
/// rate-limit tests see independent addresses) and formats its default
/// subaddress as a `tbotho://1/...` testnet address. The faucet parses this on
/// the Testnet network.
fn test_address(seed: u8) -> String {
    // Deterministic 32 bytes of entropy keyed by `seed`, distinct per seed.
    let entropy: [u8; 32] = std::array::from_fn(|i| seed.wrapping_add(i as u8).wrapping_mul(31));
    let mnemonic = bip39::Mnemonic::from_entropy(&entropy, bip39::Language::English)
        .expect("Failed to build mnemonic from entropy");
    let wallet =
        Wallet::from_mnemonic(mnemonic.phrase()).expect("Failed to create wallet from mnemonic");
    Address::classical(wallet.default_address(), Network::Testnet).to_address_string()
}

// ============================================================================
// Scenario 1: Faucet Request Flow
// ============================================================================

#[tokio::test]
#[serial]
async fn test_faucet_dispenses_correct_amount() {
    let (_temp_dir, addr, _handle) = spawn_faucet_rpc_server().await;
    let client = Client::new();

    let address = test_address(1);
    let response = rpc_call(
        &client,
        addr,
        "faucet_request",
        json!({ "address": address }),
    )
    .await;

    // Verify success response
    assert!(
        response["error"].is_null(),
        "Faucet request failed: {:?}",
        response["error"]
    );

    let result = &response["result"];
    assert_eq!(result["success"], true);

    // Verify amount is 10 BTH (10_000_000_000_000 picocredits)
    let amount: u64 = result["amount"]
        .as_str()
        .expect("amount should be a string")
        .parse()
        .expect("amount should be numeric");
    assert_eq!(amount, 10_000_000_000_000, "Faucet should dispense 10 BTH");

    // Verify formatted amount
    let formatted = result["amountFormatted"].as_str().unwrap();
    assert!(
        formatted.contains("10.000000") && formatted.contains("BTH"),
        "Formatted amount should show '10.000000 BTH', got: {}",
        formatted
    );

    // Verify transaction hash is present
    assert!(
        result["txHash"].is_string() && !result["txHash"].as_str().unwrap().is_empty(),
        "Transaction hash should be present"
    );
}

#[tokio::test]
#[serial]
async fn test_faucet_returns_tx_hash() {
    let (_temp_dir, addr, _handle) = spawn_faucet_rpc_server().await;
    let client = Client::new();

    let response = rpc_call(
        &client,
        addr,
        "faucet_request",
        json!({ "address": test_address(2) }),
    )
    .await;

    assert!(response["error"].is_null());
    let result = &response["result"];

    // Tx hash should be a 64-character hex string
    let tx_hash = result["txHash"].as_str().expect("txHash should be string");
    assert_eq!(tx_hash.len(), 64, "Transaction hash should be 64 hex chars");
    assert!(
        tx_hash.chars().all(|c| c.is_ascii_hexdigit()),
        "Transaction hash should be hex"
    );
}

// ============================================================================
// Scenario 2: Rate Limiting Validation
// ============================================================================

#[tokio::test]
#[serial]
async fn test_faucet_cooldown_between_requests() {
    let (_temp_dir, addr, _handle) = spawn_faucet_rpc_server().await;
    let client = Client::new();

    let address = test_address(3);

    // First request should succeed
    let response1 = rpc_call(
        &client,
        addr,
        "faucet_request",
        json!({ "address": address }),
    )
    .await;
    assert!(response1["error"].is_null(), "First request should succeed");

    // Immediate second request should be rate limited
    let response2 = rpc_call(
        &client,
        addr,
        "faucet_request",
        json!({ "address": address }),
    )
    .await;

    assert!(
        response2["error"].is_object(),
        "Second immediate request should be rate limited"
    );

    let error = &response2["error"];
    let message = error["message"].as_str().unwrap();

    // Verify error message includes retry time
    assert!(
        message.contains("wait") || message.contains("cooldown") || message.contains("seconds"),
        "Error should indicate cooldown/wait time, got: {}",
        message
    );
}

#[tokio::test]
#[serial]
async fn test_faucet_rate_limit_includes_retry_time() {
    let (_temp_dir, addr, _handle) = spawn_faucet_rpc_server().await;
    let client = Client::new();

    // Make first request
    let response1 = rpc_call(
        &client,
        addr,
        "faucet_request",
        json!({ "address": test_address(4) }),
    )
    .await;
    assert!(response1["error"].is_null());

    // Try again immediately
    let response2 = rpc_call(
        &client,
        addr,
        "faucet_request",
        json!({ "address": test_address(4) }),
    )
    .await;

    // Error should include retry_after_secs in data
    let error = &response2["error"];
    let message = error["message"].as_str().unwrap();

    // The message should contain the number of seconds to wait
    assert!(
        message.chars().any(|c| c.is_ascii_digit()),
        "Error message should include retry time in seconds: {}",
        message
    );
}

#[tokio::test]
#[serial]
async fn test_faucet_request_succeeds_after_cooldown() {
    let (_temp_dir, addr, _handle) = spawn_faucet_rpc_server().await;
    let client = Client::new();

    let address = test_address(5);

    // First request
    let response1 = rpc_call(
        &client,
        addr,
        "faucet_request",
        json!({ "address": address }),
    )
    .await;
    assert!(response1["error"].is_null());

    // Wait for cooldown (test config has 1 second cooldown)
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Second request should succeed after cooldown
    let response2 = rpc_call(
        &client,
        addr,
        "faucet_request",
        json!({ "address": address }),
    )
    .await;
    assert!(
        response2["error"].is_null(),
        "Request after cooldown should succeed: {:?}",
        response2["error"]
    );
}

#[tokio::test]
#[serial]
async fn test_faucet_per_address_limit() {
    let (_temp_dir, addr, _handle) = spawn_faucet_rpc_server().await;
    let client = Client::new();

    let address = test_address(6);

    // Make requests up to the per-address daily limit (3 in test config)
    for i in 0..3 {
        // Wait for cooldown between requests
        if i > 0 {
            tokio::time::sleep(Duration::from_secs(2)).await;
        }

        let response = rpc_call(
            &client,
            addr,
            "faucet_request",
            json!({ "address": address }),
        )
        .await;
        assert!(
            response["error"].is_null(),
            "Request {} should succeed: {:?}",
            i + 1,
            response["error"]
        );
    }

    // Wait for cooldown
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Fourth request to same address should be rate limited
    let response = rpc_call(
        &client,
        addr,
        "faucet_request",
        json!({ "address": address }),
    )
    .await;

    assert!(
        response["error"].is_object(),
        "Fourth request to same address should be rate limited"
    );

    let message = response["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("address") || message.contains("today") || message.contains("limit"),
        "Error should indicate per-address limit: {}",
        message
    );
}

// ============================================================================
// Scenario 3: Error Handling
// ============================================================================

#[tokio::test]
#[serial]
async fn test_faucet_invalid_address_format() {
    let (_temp_dir, addr, _handle) = spawn_faucet_rpc_server().await;
    let client = Client::new();

    let response = rpc_call(
        &client,
        addr,
        "faucet_request",
        json!({ "address": "invalid_address_format" }),
    )
    .await;

    assert!(
        response["error"].is_object(),
        "Invalid address should return error"
    );

    let message = response["error"]["message"].as_str().unwrap();
    assert!(
        message.to_lowercase().contains("address")
            || message.to_lowercase().contains("invalid")
            || message.to_lowercase().contains("format"),
        "Error should mention invalid address: {}",
        message
    );
}

#[tokio::test]
#[serial]
async fn test_faucet_missing_address_param() {
    let (_temp_dir, addr, _handle) = spawn_faucet_rpc_server().await;
    let client = Client::new();

    let response = rpc_call(&client, addr, "faucet_request", json!({})).await;

    assert!(
        response["error"].is_object(),
        "Missing address should return error"
    );

    let message = response["error"]["message"].as_str().unwrap();
    assert!(
        message.to_lowercase().contains("address") || message.to_lowercase().contains("missing"),
        "Error should mention missing address: {}",
        message
    );
}

#[tokio::test]
#[serial]
async fn test_faucet_error_messages_are_user_friendly() {
    let (_temp_dir, addr, _handle) = spawn_faucet_rpc_server().await;
    let client = Client::new();

    // Trigger rate limit to check error message quality
    let address = test_address(7);

    rpc_call(
        &client,
        addr,
        "faucet_request",
        json!({ "address": address }),
    )
    .await;

    let response = rpc_call(
        &client,
        addr,
        "faucet_request",
        json!({ "address": address }),
    )
    .await;

    let error = &response["error"];
    let message = error["message"].as_str().unwrap();

    // Error messages should be clear and actionable
    assert!(
        !message.contains("panic") && !message.contains("unwrap") && !message.contains("internal"),
        "Error message should not expose implementation details: {}",
        message
    );

    // Should contain helpful information
    assert!(
        message.contains("wait")
            || message.contains("seconds")
            || message.contains("Try again")
            || message.contains("cooldown"),
        "Error should provide actionable guidance: {}",
        message
    );
}

// ============================================================================
// Scenario 4: Faucet Stats Endpoint
// ============================================================================

#[tokio::test]
#[serial]
async fn test_faucet_stats_endpoint() {
    let (_temp_dir, addr, _handle) = spawn_faucet_rpc_server().await;
    let client = Client::new();

    // Make a faucet request first
    rpc_call(
        &client,
        addr,
        "faucet_request",
        json!({ "address": test_address(8) }),
    )
    .await;

    // Check faucet stats
    let response = rpc_call(&client, addr, "faucet_getStatus", json!({})).await;

    assert!(
        response["error"].is_null(),
        "Stats request should succeed: {:?}",
        response["error"]
    );

    let result = &response["result"];
    assert_eq!(result["enabled"], true);
    assert!(result["amountPerRequest"].is_number());
    assert!(result["dailyDispensed"].is_number());
    assert!(result["dailyLimit"].is_number());

    // Daily dispensed should reflect the request we made
    let dispensed: u64 = result["dailyDispensed"].as_u64().unwrap_or(0);
    assert!(
        dispensed >= 10_000_000_000_000,
        "Daily dispensed should reflect at least one request"
    );
}

// ============================================================================
// Scenario 5: Faucet Disabled
// ============================================================================

#[tokio::test]
#[serial]
async fn test_faucet_disabled_returns_clear_error() {
    // Spawn RPC server without faucet enabled
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let ledger_path = temp_dir.path().join("ledger");

    let ledger = Ledger::open(&ledger_path).expect("Failed to create ledger");
    // RandomX genesis difficulty is real-hashrate sized; pin the chain to
    // the trivial target so test PoW solves in one hash and the C1
    // block-apply difficulty check accepts it.
    ledger.set_difficulty(TRIVIAL_DIFFICULTY).unwrap();
    let mempool = Mempool::new();
    let ws_broadcaster = Arc::new(WsBroadcaster::new(100));

    // Create state WITHOUT faucet
    let state = Arc::new(RpcState::new(
        ledger,
        mempool,
        Network::Testnet,
        None,
        None,
        vec!["*".to_string()],
        ws_broadcaster,
    ));

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind");
    let addr = listener.local_addr().expect("Failed to get local addr");
    drop(listener);

    let state_clone = state.clone();
    let _handle = tokio::spawn(async move {
        let _ = botho::rpc::start_rpc_server(addr, state_clone).await;
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    let client = Client::new();
    let response = rpc_call(
        &client,
        addr,
        "faucet_request",
        json!({ "address": test_address(9) }),
    )
    .await;

    assert!(
        response["error"].is_object(),
        "Faucet request should fail when disabled"
    );

    let message = response["error"]["message"].as_str().unwrap();
    assert!(
        message.to_lowercase().contains("disabled")
            || message.to_lowercase().contains("not available")
            || message.to_lowercase().contains("not enabled"),
        "Error should clearly indicate faucet is disabled: {}",
        message
    );
}

// ============================================================================
// Integration with Transaction Flow
// ============================================================================

#[tokio::test]
#[serial]
async fn test_faucet_transaction_in_mempool() {
    let (_temp_dir, addr, _handle) = spawn_faucet_rpc_server().await;
    let client = Client::new();

    // Request from faucet
    let faucet_response = rpc_call(
        &client,
        addr,
        "faucet_request",
        json!({ "address": test_address(10) }),
    )
    .await;

    assert!(faucet_response["error"].is_null());
    let tx_hash = faucet_response["result"]["txHash"]
        .as_str()
        .expect("txHash should be present");

    // Check mempool for the transaction
    let mempool_response = rpc_call(&client, addr, "getMempoolInfo", json!({})).await;

    assert!(mempool_response["error"].is_null());
    let tx_hashes = mempool_response["result"]["txHashes"]
        .as_array()
        .expect("txHashes should be array");

    // Transaction should be in mempool
    let tx_in_mempool = tx_hashes.iter().any(|h| h.as_str() == Some(tx_hash));
    assert!(
        tx_in_mempool,
        "Faucet transaction {} should be in mempool",
        tx_hash
    );
}

#[tokio::test]
#[serial]
async fn test_faucet_transaction_status_pending() {
    let (_temp_dir, addr, _handle) = spawn_faucet_rpc_server().await;
    let client = Client::new();

    // Request from faucet
    let faucet_response = rpc_call(
        &client,
        addr,
        "faucet_request",
        json!({ "address": test_address(11) }),
    )
    .await;

    assert!(faucet_response["error"].is_null());
    let tx_hash = faucet_response["result"]["txHash"].as_str().unwrap();

    // Check transaction status
    let status_response = rpc_call(
        &client,
        addr,
        "getTransactionStatus",
        json!({ "tx_hash": tx_hash }),
    )
    .await;

    assert!(status_response["error"].is_null());
    let status = status_response["result"]["status"].as_str().unwrap();

    // Should be pending (in mempool) or confirmed
    assert!(
        status == "pending" || status == "confirmed",
        "Transaction status should be 'pending' or 'confirmed', got: {}",
        status
    );
}

// ============================================================================
// Concurrent Request Tests
// ============================================================================

#[tokio::test]
#[serial]
async fn test_faucet_handles_concurrent_requests() {
    // All loopback requests share IP 127.0.0.1, so the per-IP cooldown would
    // otherwise reject concurrent requests. Disable the cooldown and raise the
    // per-IP hourly limit so the test genuinely exercises concurrent dispense
    // handling to distinct addresses rather than the cooldown path.
    let config = FaucetConfig {
        cooldown_secs: 0,
        per_ip_hourly_limit: 100,
        ..test_faucet_config()
    };
    let (_temp_dir, addr, _handle) = spawn_faucet_rpc_server_with_config(config).await;
    let client = Client::new();

    // Send multiple concurrent requests to distinct addresses.
    let futures: Vec<_> = (0..5)
        .map(|i| {
            let client = client.clone();
            async move {
                rpc_call(
                    &client,
                    addr,
                    "faucet_request",
                    json!({ "address": test_address(100 + i) }),
                )
                .await
            }
        })
        .collect();

    let results = futures::future::join_all(futures).await;

    // All requests to different addresses should succeed
    for (i, result) in results.iter().enumerate() {
        assert!(
            result["error"].is_null(),
            "Request {} should succeed: {:?}",
            i,
            result["error"]
        );
    }
}
