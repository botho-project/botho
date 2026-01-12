// Copyright (c) 2024 Botho Foundation
//
//! RPC Integration Tests
//!
//! Tests the JSON-RPC server with real HTTP requests against a running server.
//! These tests verify:
//! - Endpoint correctness and response format
//! - Error handling for invalid inputs
//! - CORS behavior
//! - Transaction submission and status queries

use std::{net::SocketAddr, sync::Arc, time::Duration};

use reqwest::Client;
use serde_json::{json, Value};
use serial_test::serial;
use tempfile::TempDir;
use tokio::net::TcpListener;

use botho::{
    ledger::Ledger,
    mempool::Mempool,
    rpc::{RpcState, WsBroadcaster},
};

// ============================================================================
// Test Helpers
// ============================================================================

/// Spawn an RPC server on a random available port.
/// Returns the server task handle and the bound address.
async fn spawn_test_rpc_server() -> (TempDir, SocketAddr, tokio::task::JoinHandle<()>) {
    // Create temporary directory for ledger
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let ledger_path = temp_dir.path().join("ledger");

    // Initialize ledger
    let ledger = Ledger::open(&ledger_path).expect("Failed to create ledger");

    // Create mempool
    let mempool = Mempool::new();

    // Create WebSocket broadcaster (capacity of 100 events)
    let ws_broadcaster = Arc::new(WsBroadcaster::new(100));

    // Create RPC state
    let state = Arc::new(RpcState::new(
        ledger,
        mempool,
        None,                  // No wallet view key
        None,                  // No wallet spend key
        vec!["*".to_string()], // Allow all CORS origins for testing
        ws_broadcaster,
    ));

    // Find a random available port
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind");
    let addr = listener.local_addr().expect("Failed to get local addr");
    drop(listener);

    // Spawn RPC server
    let state_clone = state.clone();
    let handle = tokio::spawn(async move {
        if let Err(e) = botho::rpc::start_rpc_server(addr, state_clone).await {
            // Server stopped, this is expected during test teardown
            tracing::debug!("RPC server stopped: {}", e);
        }
    });

    // Wait for server to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    (temp_dir, addr, handle)
}

/// Create a JSON-RPC request body.
fn rpc_request(method: &str, params: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
        "id": 1
    })
}

/// Make an RPC call and return the response.
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

// ============================================================================
// Node Status Tests
// ============================================================================

#[tokio::test]
#[serial]
async fn test_node_get_status() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();

    let response = rpc_call(&client, addr, "node_getStatus", json!({})).await;

    // Verify success response
    assert_eq!(response["jsonrpc"], "2.0");
    assert!(
        response["error"].is_null(),
        "Unexpected error: {:?}",
        response["error"]
    );
    assert!(response["result"].is_object());

    let result = &response["result"];
    // Core fields
    assert!(result["version"].is_string());
    assert!(result["nodeVersion"].is_string());
    assert!(result["network"].is_string());
    assert!(result["uptimeSeconds"].is_number());
    assert!(result["chainHeight"].is_number());
    assert!(result["tipHash"].is_string());
    assert!(result["peerCount"].is_number());
    assert!(result["mempoolSize"].is_number());
    assert!(result["mintingActive"].is_boolean());

    // Extended fields (issue #307)
    assert!(result["scpPeerCount"].is_number());
    assert!(result["mintingThreads"].is_number());
    assert!(result["syncProgress"].is_number());
    assert!(result["synced"].is_boolean());
    assert!(result["totalTransactions"].is_number());

    // Verify version and nodeVersion match
    assert_eq!(result["version"], result["nodeVersion"]);

    // Verify syncProgress is 100 when synced
    assert_eq!(result["syncProgress"].as_f64().unwrap(), 100.0);
    assert!(result["synced"].as_bool().unwrap());
}

// ============================================================================
// Chain Info Tests
// ============================================================================

#[tokio::test]
#[serial]
async fn test_get_chain_info() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();

    let response = rpc_call(&client, addr, "getChainInfo", json!({})).await;

    assert!(response["error"].is_null());
    let result = &response["result"];

    assert!(result["height"].is_number());
    assert!(result["tipHash"].is_string());
    assert!(result["difficulty"].is_number());
    assert!(result["totalMined"].is_number());
    assert!(result["mempoolSize"].is_number());
    assert!(result["mempoolFees"].is_number());
}

#[tokio::test]
#[serial]
async fn test_get_block_by_height() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();

    // Height 0 is the genesis block
    let response = rpc_call(&client, addr, "getBlockByHeight", json!({"height": 0})).await;

    // Should get block or error if no genesis exists yet
    // In a fresh ledger, there may be no block at height 0
    if response["error"].is_null() {
        let result = &response["result"];
        assert!(result["height"].is_number());
        assert!(result["hash"].is_string());
        assert!(result["prevHash"].is_string());
        assert!(result["timestamp"].is_number());
    } else {
        // Block not found is acceptable for empty ledger
        assert!(response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("not found"));
    }
}

#[tokio::test]
#[serial]
async fn test_get_block_invalid_height() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();

    // Very high block height should return error
    let response = rpc_call(
        &client,
        addr,
        "getBlockByHeight",
        json!({"height": 999999999}),
    )
    .await;

    assert!(response["error"].is_object());
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("not found"));
}

// ============================================================================
// Mempool Tests
// ============================================================================

#[tokio::test]
#[serial]
async fn test_get_mempool_info() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();

    let response = rpc_call(&client, addr, "getMempoolInfo", json!({})).await;

    assert!(response["error"].is_null());
    let result = &response["result"];

    assert!(result["size"].is_number());
    assert!(result["totalFees"].is_number());
    assert!(result["txHashes"].is_array());
}

#[tokio::test]
#[serial]
async fn test_estimate_fee() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();

    // Estimate fee for a 10 BTH private transaction
    let response = rpc_call(
        &client,
        addr,
        "estimateFee",
        json!({
            "amount": 10_000_000_000_000u64,  // 10 BTH in picocredits
            "txType": "hidden",
            "memos": 1
        }),
    )
    .await;

    assert!(
        response["error"].is_null(),
        "Unexpected error: {:?}",
        response["error"]
    );
    let result = &response["result"];

    assert!(result["minimumFee"].is_number());
    assert!(result["clusterFactor"].is_number());
    assert!(result["recommendedFee"].is_number());
    assert!(result["highPriorityFee"].is_number());

    // Verify fee parameters were echoed back
    assert_eq!(result["params"]["amount"], 10_000_000_000_000u64);
    assert_eq!(result["params"]["txType"], "hidden");
    assert_eq!(result["params"]["memos"], 1);
}

// ============================================================================
// Transaction Tests
// ============================================================================

#[tokio::test]
#[serial]
async fn test_submit_tx_invalid_hex() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();

    // Submit garbage hex
    let response = rpc_call(
        &client,
        addr,
        "tx_submit",
        json!({
            "tx_hex": "not_valid_hex!"
        }),
    )
    .await;

    assert!(response["error"].is_object());
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("Invalid hex"));
}

#[tokio::test]
#[serial]
async fn test_submit_tx_invalid_format() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();

    // Submit valid hex but not a valid transaction
    let response = rpc_call(
        &client,
        addr,
        "tx_submit",
        json!({
            "tx_hex": "deadbeef"
        }),
    )
    .await;

    assert!(response["error"].is_object());
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("Invalid transaction"));
}

#[tokio::test]
#[serial]
async fn test_submit_tx_missing_param() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();

    // Missing tx_hex parameter
    let response = rpc_call(&client, addr, "tx_submit", json!({})).await;

    assert!(response["error"].is_object());
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("Missing tx_hex"));
}

#[tokio::test]
#[serial]
async fn test_get_transaction_not_found() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();

    // Query for a non-existent transaction
    let fake_hash = "0".repeat(64);
    let response = rpc_call(
        &client,
        addr,
        "getTransaction",
        json!({
            "tx_hash": fake_hash
        }),
    )
    .await;

    assert!(response["error"].is_object());
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("not found"));
}

#[tokio::test]
#[serial]
async fn test_get_transaction_invalid_hash() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();

    // Invalid hash format (too short)
    let response = rpc_call(
        &client,
        addr,
        "getTransaction",
        json!({
            "tx_hash": "abcd"
        }),
    )
    .await;

    assert!(response["error"].is_object());
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("Invalid tx_hash"));
}

#[tokio::test]
#[serial]
async fn test_get_transaction_status_unknown() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();

    // Query status for non-existent transaction
    let fake_hash = "0".repeat(64);
    let response = rpc_call(
        &client,
        addr,
        "getTransactionStatus",
        json!({
            "tx_hash": fake_hash
        }),
    )
    .await;

    // Should return status "unknown" not an error
    assert!(response["error"].is_null());
    let result = &response["result"];
    assert_eq!(result["status"], "unknown");
    assert_eq!(result["confirmations"], 0);
    assert_eq!(result["confirmed"], false);
}

// ============================================================================
// Address Validation Tests
// ============================================================================

#[tokio::test]
#[serial]
async fn test_validate_address_missing_param() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();

    let response = rpc_call(&client, addr, "validateAddress", json!({})).await;

    assert!(response["error"].is_object());
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("Missing address"));
}

#[tokio::test]
#[serial]
async fn test_validate_address_invalid() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();

    let response = rpc_call(
        &client,
        addr,
        "validateAddress",
        json!({
            "address": "not_a_valid_address"
        }),
    )
    .await;

    // Invalid address returns success with valid: false
    assert!(response["error"].is_null());
    let result = &response["result"];
    assert_eq!(result["valid"], false);
    assert!(result["error"].is_string());
}

// ============================================================================
// Network Info Tests
// ============================================================================

#[tokio::test]
#[serial]
async fn test_get_network_info() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();

    let response = rpc_call(&client, addr, "network_getInfo", json!({})).await;

    assert!(response["error"].is_null());
    let result = &response["result"];

    assert!(result["peerCount"].is_number());
    assert!(result["uptimeSeconds"].is_number());
}

#[tokio::test]
#[serial]
async fn test_get_peers() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();

    let response = rpc_call(&client, addr, "network_getPeers", json!({})).await;

    assert!(response["error"].is_null());
    let result = &response["result"];
    assert!(result["peers"].is_array());
}

// ============================================================================
// Minting Status Tests
// ============================================================================

#[tokio::test]
#[serial]
async fn test_minting_get_status() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();

    let response = rpc_call(&client, addr, "minting_getStatus", json!({})).await;

    assert!(response["error"].is_null());
    let result = &response["result"];

    assert!(result["active"].is_boolean());
    assert!(result["threads"].is_number());
    assert!(result["currentDifficulty"].is_number());
    assert!(result["uptimeSeconds"].is_number());
}

// ============================================================================
// Error Handling Tests
// ============================================================================

#[tokio::test]
#[serial]
async fn test_method_not_found() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();

    let response = rpc_call(&client, addr, "nonexistent_method", json!({})).await;

    assert!(response["error"].is_object());
    assert_eq!(response["error"]["code"], -32601);
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("Method not found"));
}

#[tokio::test]
#[serial]
async fn test_parse_error_invalid_json() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();
    let url = format!("http://{}", addr);

    let response = client
        .post(&url)
        .header("Content-Type", "application/json")
        .body("{ invalid json }")
        .send()
        .await
        .expect("Failed to send request");

    let json: Value = response
        .json::<Value>()
        .await
        .expect("Failed to parse response");
    assert!(json["error"].is_object());
    assert_eq!(json["error"]["code"], -32700);
    assert!(json["error"]["message"]
        .as_str()
        .unwrap()
        .contains("Parse error"));
}

#[tokio::test]
#[serial]
async fn test_method_not_allowed() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();
    let url = format!("http://{}", addr);

    // GET requests should be rejected (only POST allowed for JSON-RPC)
    let response = client
        .get(&url)
        .send()
        .await
        .expect("Failed to send request");

    assert_eq!(response.status().as_u16(), 405); // METHOD_NOT_ALLOWED
}

// ============================================================================
// Wallet Endpoint Tests
// ============================================================================

#[tokio::test]
#[serial]
async fn test_wallet_get_balance() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();

    let response = rpc_call(&client, addr, "wallet_getBalance", json!({})).await;

    assert!(response["error"].is_null());
    let result = &response["result"];

    // Fresh wallet should have zero balance
    assert!(result["confirmed"].is_number());
    assert!(result["pending"].is_number());
    assert!(result["total"].is_number());
}

#[tokio::test]
#[serial]
async fn test_wallet_get_address() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();

    let response = rpc_call(&client, addr, "wallet_getAddress", json!({})).await;

    assert!(response["error"].is_null());
    let result = &response["result"];

    // We didn't configure a wallet, so hasWallet should be false
    assert_eq!(result["hasWallet"], false);
}

// ============================================================================
// Exchange Integration Endpoint Tests
// ============================================================================

#[tokio::test]
#[serial]
async fn test_exchange_register_view_key_missing_params() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();

    // Missing required parameters
    let response = rpc_call(&client, addr, "exchange_registerViewKey", json!({})).await;

    assert!(response["error"].is_object());
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("Missing"));
}

#[tokio::test]
#[serial]
async fn test_exchange_register_view_key_invalid_key() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();

    // Invalid key format (wrong length)
    let response = rpc_call(
        &client,
        addr,
        "exchange_registerViewKey",
        json!({
            "id": "test-registration",
            "view_private_key": "abcd",  // Too short
            "spend_public_key": "0".repeat(64)
        }),
    )
    .await;

    assert!(response["error"].is_object());
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("64 hex"));
}

#[tokio::test]
#[serial]
async fn test_exchange_list_view_keys() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();

    // List view keys (should be empty initially)
    let response = rpc_call(
        &client,
        addr,
        "exchange_listViewKeys",
        json!({
            "api_key_id": "default"
        }),
    )
    .await;

    assert!(response["error"].is_null());
    let result = &response["result"];

    assert_eq!(result["count"], 0);
    assert!(result["view_keys"].is_array());
}

// ============================================================================
// Method Alias Tests
// ============================================================================

#[tokio::test]
#[serial]
async fn test_method_aliases() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();

    // Test that aliased methods work
    let methods = vec![
        ("tx_submit", "sendRawTransaction"),
        ("tx_get", "getTransaction"),
        ("tx_getStatus", "getTransactionStatus"),
        ("tx_estimateFee", "estimateFee"),
        ("address_validate", "validateAddress"),
    ];

    for (alias1, alias2) in methods {
        // Both aliases should work (though they may return errors due to missing
        // params)
        let response1 = rpc_call(&client, addr, alias1, json!({})).await;
        let response2 = rpc_call(&client, addr, alias2, json!({})).await;

        // They should both respond (not "method not found")
        // The error code should be -32602 (invalid params) or similar, not -32601
        // (method not found)
        if response1["error"].is_object() {
            assert_ne!(
                response1["error"]["code"], -32601,
                "Alias {} not found",
                alias1
            );
        }
        if response2["error"].is_object() {
            assert_ne!(
                response2["error"]["code"], -32601,
                "Alias {} not found",
                alias2
            );
        }
    }
}

// ============================================================================
// Concurrent Request Tests
// ============================================================================

#[tokio::test]
#[serial]
async fn test_concurrent_requests() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();

    // Send multiple concurrent requests
    let futures: Vec<_> = (0..10)
        .map(|_| {
            let client = client.clone();
            async move { rpc_call(&client, addr, "node_getStatus", json!({})).await }
        })
        .collect();

    let results = futures::future::join_all(futures).await;

    // All requests should succeed
    for result in results {
        assert!(
            result["error"].is_null(),
            "Concurrent request failed: {:?}",
            result
        );
        assert!(result["result"]["chainHeight"].is_number());
    }
}

// ============================================================================
// Response Format Tests
// ============================================================================

#[tokio::test]
#[serial]
async fn test_response_format() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();

    let response = rpc_call(&client, addr, "node_getStatus", json!({})).await;

    // Verify JSON-RPC 2.0 compliance
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 1);

    // Either result or error should be present, not both
    let has_result = !response["result"].is_null();
    let has_error = !response["error"].is_null();
    assert!(
        has_result != has_error,
        "Response should have exactly one of result or error"
    );
}

// ============================================================================
// Observability Endpoint Tests
// ============================================================================

#[tokio::test]
#[serial]
async fn test_health_endpoint() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();
    let url = format!("http://{}/health", addr);

    let response = client
        .get(&url)
        .send()
        .await
        .expect("Failed to send request");

    assert_eq!(response.status().as_u16(), 200);

    let json: Value = response.json().await.expect("Failed to parse JSON");

    // Verify health response structure (HealthResponse: status, uptime_seconds)
    assert!(json["status"].is_string());
    assert!(json["uptime_seconds"].is_number());

    // Status should be one of the valid values
    let status = json["status"].as_str().unwrap();
    assert!(
        status == "healthy" || status == "degraded" || status == "unhealthy",
        "Invalid status: {}",
        status
    );
}

#[tokio::test]
#[serial]
async fn test_ready_endpoint() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();
    let url = format!("http://{}/ready", addr);

    let response = client
        .get(&url)
        .send()
        .await
        .expect("Failed to send request");

    // Status should be 200 or 503
    let status = response.status().as_u16();
    assert!(
        status == 200 || status == 503,
        "Unexpected status code: {}",
        status
    );

    let json: Value = response.json().await.expect("Failed to parse JSON");

    // Verify ready response structure (ReadyResponse: status, synced, peers,
    // block_height)
    assert!(json["status"].is_string());
    assert!(json["synced"].is_boolean());
    assert!(json["peers"].is_number());
    assert!(json["block_height"].is_number());

    // status should be "ready" or "not_ready" matching the HTTP status code
    let is_ready = json["status"].as_str().unwrap() == "ready";
    assert_eq!(is_ready, status == 200);
}

#[tokio::test]
#[serial]
async fn test_metrics_endpoint() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();
    let url = format!("http://{}/metrics", addr);

    let response = client
        .get(&url)
        .send()
        .await
        .expect("Failed to send request");

    assert_eq!(response.status().as_u16(), 200);

    // Check content type is Prometheus format
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        content_type.contains("text/plain"),
        "Expected text/plain content type, got: {}",
        content_type
    );

    let body = response.text().await.expect("Failed to get body");

    // Verify Prometheus metrics are present
    assert!(
        body.contains("botho_block_height"),
        "Missing botho_block_height metric"
    );
    assert!(
        body.contains("botho_peer_count"),
        "Missing botho_peer_count metric"
    );
    assert!(
        body.contains("botho_mempool_size"),
        "Missing botho_mempool_size metric"
    );
}

#[tokio::test]
#[serial]
async fn test_metrics_after_rpc_calls() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();

    // Make some RPC calls to generate metrics
    rpc_call(&client, addr, "node_getStatus", json!({})).await;
    rpc_call(&client, addr, "node_getStatus", json!({})).await;
    rpc_call(&client, addr, "getChainInfo", json!({})).await;

    // Now check metrics endpoint
    let url = format!("http://{}/metrics", addr);
    let response = client
        .get(&url)
        .send()
        .await
        .expect("Failed to send request");

    let body = response.text().await.expect("Failed to get body");

    // Should see RPC request counters
    assert!(
        body.contains("botho_rpc_requests_total"),
        "Missing botho_rpc_requests_total metric"
    );
    assert!(
        body.contains("node_getStatus"),
        "Missing node_getStatus in metrics"
    );
}
