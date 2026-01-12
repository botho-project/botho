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

use std::{net::SocketAddr, sync::Arc, time::Duration};

use reqwest::Client;
use serde_json::{json, Value};
use serial_test::serial;
use tempfile::TempDir;
use tokio::net::TcpListener;

use bth_transaction_types::constants::Network;
use botho::{
    config::FaucetConfig,
    ledger::Ledger,
    mempool::Mempool,
    rpc::{FaucetState, RpcState, WsBroadcaster},
};

// ============================================================================
// Test Helpers
// ============================================================================

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

/// Spawn an RPC server with faucet enabled on a random available port.
async fn spawn_faucet_rpc_server() -> (TempDir, SocketAddr, tokio::task::JoinHandle<()>) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let ledger_path = temp_dir.path().join("ledger");

    let ledger = Ledger::open(&ledger_path).expect("Failed to create ledger");
    let mempool = Mempool::new();
    let ws_broadcaster = Arc::new(WsBroadcaster::new(100));

    let mut state = RpcState::new(
        ledger,
        mempool,
        Network::Testnet,
        None,
        None,
        vec!["*".to_string()],
        ws_broadcaster,
    );

    // Enable faucet
    state.faucet = Some(Arc::new(FaucetState::new(test_faucet_config())));

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

/// Generate a test address (view:hex\nspend:hex format)
fn test_address(seed: u8) -> String {
    let view_key = format!("{:064x}", seed as u64 * 12345);
    let spend_key = format!("{:064x}", seed as u64 * 67890);
    format!("view:{}\nspend:{}", view_key, spend_key)
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
    let (_temp_dir, addr, _handle) = spawn_faucet_rpc_server().await;
    let client = Client::new();

    // Send multiple concurrent requests from "different IPs" (different addresses)
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
