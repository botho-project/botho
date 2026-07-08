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
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

use botho::{
    ledger::Ledger,
    mempool::Mempool,
    rpc::{RpcState, WsBroadcaster},
};
use bth_transaction_types::constants::Network;

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
        Network::Testnet,      // Use testnet for tests
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
    // totalMined is a u128 picocredit value emitted as a decimal string (PR #342)
    // to avoid JS 2^53 precision loss; verify it is a well-formed unsigned integer.
    assert!(result["totalMined"].is_string());
    assert!(result["totalMined"]
        .as_str()
        .unwrap()
        .parse::<u128>()
        .is_ok());
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

#[tokio::test]
#[serial]
async fn test_get_block_by_hash_known() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();

    // The genesis block always exists in a freshly opened ledger. Its hash is
    // reported as the tip hash via getChainInfo. Use that to exercise the
    // known-hash path of getBlockByHash.
    let chain_info = rpc_call(&client, addr, "getChainInfo", json!({})).await;
    assert!(chain_info["error"].is_null());
    let tip_hash = chain_info["result"]["tipHash"]
        .as_str()
        .expect("tipHash should be a hex string")
        .to_string();

    let response = rpc_call(&client, addr, "getBlockByHash", json!({ "hash": tip_hash })).await;

    assert!(
        response["error"].is_null(),
        "Unexpected error: {:?}",
        response["error"]
    );
    let result = &response["result"];
    assert!(result["height"].is_number());
    assert!(result["hash"].is_string());
    assert!(result["prevHash"].is_string());
    assert!(result["timestamp"].is_number());
    // The returned block's hash must match the requested hash.
    assert_eq!(result["hash"].as_str().unwrap(), tip_hash);

    // #696: the enriched explorer fields ride along on the by-hash path too.
    assert!(result["transactions"].is_array());
    assert!(result["totalFees"].is_number());
    assert!(result["lottery"].is_object());
}

/// #696: `getBlockByHeight` carries the additive explorer fields —
/// `transactions` (per-tx hash/fee/ringSize), `totalFees`, and the `lottery`
/// summary — while leaving the original header shape untouched. Exercised
/// against the genesis block, which always exists in a fresh ledger, so the
/// new fields must render as explicit empty/zero defaults (never be omitted).
#[tokio::test]
#[serial]
async fn test_get_block_by_height_explorer_fields() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();

    let response = rpc_call(&client, addr, "getBlockByHeight", json!({"height": 0})).await;
    assert!(
        response["error"].is_null(),
        "genesis block should exist: {:?}",
        response["error"]
    );
    let result = &response["result"];

    // Original (pre-#696) shape intact — additive-only contract.
    assert!(result["height"].is_number());
    assert!(result["hash"].is_string());
    assert!(result["prevHash"].is_string());
    assert!(result["timestamp"].is_number());
    assert!(result["difficulty"].is_number());
    assert!(result["nonce"].is_number());
    assert!(result["txCount"].is_number());
    assert!(result["mintingReward"].is_number());

    // New: per-tx structure. Genesis has no transfer txs -> empty array.
    let txs = result["transactions"]
        .as_array()
        .expect("transactions must be an array");
    assert!(txs.is_empty());

    // New: block fee total.
    assert_eq!(result["totalFees"], json!(0));

    // New: lottery summary with the full field set, zeroed for genesis.
    let lottery = &result["lottery"];
    assert!(lottery.is_object());
    assert_eq!(lottery["totalFees"], json!(0));
    assert_eq!(lottery["poolDistributed"], json!(0));
    assert_eq!(lottery["amountBurned"], json!(0));
    assert!(lottery["lotterySeed"].is_string());
    assert_eq!(lottery["payoutCount"], json!(0));
    assert_eq!(lottery["payoutTotal"], json!(0));
}

#[tokio::test]
#[serial]
async fn test_get_block_by_hash_unknown() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();

    // A 32-byte all-zeros hash will not match any real block.
    let unknown_hash = "0".repeat(64);
    let response = rpc_call(
        &client,
        addr,
        "getBlockByHash",
        json!({ "hash": unknown_hash }),
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
async fn test_get_block_by_hash_invalid_hash() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();

    // Too-short hash should be rejected as invalid params.
    let response = rpc_call(&client, addr, "getBlockByHash", json!({ "hash": "abcd" })).await;

    assert!(response["error"].is_object());
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("Invalid hash"));
}

#[tokio::test]
#[serial]
async fn test_get_block_by_hash_missing_param() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;
    let client = Client::new();

    let response = rpc_call(&client, addr, "getBlockByHash", json!({})).await;

    assert!(response["error"].is_object());
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("Missing hash"));
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

// ============================================================================
// WebSocket Upgrade Tests (#329)
// ============================================================================
//
// These tests exercise the `/ws` endpoint end-to-end through `start_rpc_server`
// using a raw TCP socket so the full RFC 6455 opening handshake is verified.
// They guard against the regression where `wss://seed.botho.io/rpc/ws` returned
// HTTP 400 instead of `101 Switching Protocols` (issue #329).

/// Compute the expected `Sec-WebSocket-Accept` value for a client key.
fn expected_accept_key(client_key: &str) -> String {
    use base64::Engine;
    use sha1::{Digest, Sha1};
    const WEBSOCKET_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
    let mut hasher = Sha1::new();
    hasher.update(client_key.as_bytes());
    hasher.update(WEBSOCKET_GUID.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(hasher.finalize())
}

/// Encode a client->server masked text frame (payload < 126 bytes).
fn client_text_frame(payload: &[u8]) -> Vec<u8> {
    assert!(
        payload.len() < 126,
        "test helper only supports short frames"
    );
    let mask: [u8; 4] = [0x12, 0x34, 0x56, 0x78];
    let mut frame = vec![0x81, 0x80 | (payload.len() as u8)];
    frame.extend_from_slice(&mask);
    frame.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    frame
}

/// Read the HTTP response head (up to and including the blank line).
async fn read_http_head(stream: &mut TcpStream) -> String {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut byte))
            .await
            .expect("timed out reading handshake response")
            .expect("read error");
        if n == 0 {
            break;
        }
        buf.push(byte[0]);
        if buf.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

#[tokio::test]
#[serial]
async fn test_websocket_upgrade_returns_101() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;

    let client_key = "dGhlIHNhbXBsZSBub25jZQ==";
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    let req = format!(
        "GET /ws HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\n\
         Connection: Upgrade\r\nSec-WebSocket-Key: {client_key}\r\n\
         Sec-WebSocket-Version: 13\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).await.expect("write req");

    let head = read_http_head(&mut stream).await;
    assert!(
        head.starts_with("HTTP/1.1 101"),
        "expected 101 Switching Protocols, got:\n{head}"
    );
    assert!(
        head.to_lowercase().contains("upgrade: websocket"),
        "missing Upgrade header:\n{head}"
    );
    // Header names are case-insensitive (hyper emits them lowercased) but the
    // accept-key VALUE is case-sensitive base64, so match it verbatim.
    let accept = expected_accept_key(client_key);
    assert!(
        head.lines().any(|line| {
            let line = line.trim();
            line.to_lowercase().starts_with("sec-websocket-accept:") && line.ends_with(&accept)
        }),
        "missing/incorrect Sec-WebSocket-Accept (expected {accept}):\n{head}"
    );
}

#[tokio::test]
#[serial]
async fn test_websocket_subscribe_roundtrip() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;

    let client_key = "x3JJHMbDL1EzLkh9GBhXDw==";
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    let req = format!(
        "GET /ws HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\n\
         Connection: keep-alive, Upgrade\r\nSec-WebSocket-Key: {client_key}\r\n\
         Sec-WebSocket-Version: 13\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).await.expect("write req");

    let head = read_http_head(&mut stream).await;
    assert!(
        head.starts_with("HTTP/1.1 101"),
        "expected 101 Switching Protocols, got:\n{head}"
    );

    // Send a subscribe frame and expect a `subscribed` confirmation back.
    let sub = br#"{"type":"subscribe","events":["blocks","peers"]}"#;
    stream
        .write_all(&client_text_frame(sub))
        .await
        .expect("write subscribe frame");

    // Read a server text frame (unmasked). header[0]=0x81 (fin+text),
    // header[1]=payload length (<126 here).
    let mut header = [0u8; 2];
    tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut header))
        .await
        .expect("timed out reading server frame")
        .expect("read frame header");
    assert_eq!(header[0] & 0x0F, 0x1, "expected a text frame");
    let len = (header[1] & 0x7F) as usize;
    assert!(len < 126, "subscribe reply unexpectedly large");
    let mut payload = vec![0u8; len];
    stream
        .read_exact(&mut payload)
        .await
        .expect("read frame payload");

    let msg: Value = serde_json::from_slice(&payload).expect("server frame is JSON");
    assert_eq!(msg["type"], "subscribed");
    let events: Vec<String> = msg["events"]
        .as_array()
        .expect("events array")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!(events.contains(&"blocks".to_string()));
    assert!(events.contains(&"peers".to_string()));
}

// ============================================================================
// Operator read-surface Tests (#707, P4.2)
// ============================================================================

use std::sync::RwLock as StdRwLock;

use botho::{
    consensus::QuorumGateSnapshot,
    rpc::auth::{mint_operator_read_token, DEFAULT_OPERATOR_TOKEN_TTL_SECONDS},
};

/// Spawn an RPC server with the operator read surface enabled under `secret`.
/// If `gate` is provided, it is published into the shared gate handle so
/// `operator_getQuorumInfo` can surface per-peer classification.
async fn spawn_operator_rpc_server(
    secret: Option<&str>,
    gate: Option<QuorumGateSnapshot>,
) -> (TempDir, SocketAddr, tokio::task::JoinHandle<()>) {
    let temp_dir = TempDir::new().expect("temp dir");
    let ledger = Ledger::open(&temp_dir.path().join("ledger")).expect("ledger");
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
    )
    .with_operator_read_token_secret(secret.map(str::to_string));

    // A gate handle is always wired; it stays `None` (unevaluated) unless a
    // snapshot is supplied — exercising the anti-#541 "null until evaluated"
    // contract.
    let gate_handle = Arc::new(StdRwLock::new(gate));
    state = state.with_quorum_gate_status(gate_handle);

    let state = Arc::new(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    drop(listener);

    let state_clone = state.clone();
    let handle = tokio::spawn(async move {
        let _ = botho::rpc::start_rpc_server(addr, state_clone).await;
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    (temp_dir, addr, handle)
}

/// A valid token for `secret` expiring in the default TTL.
fn valid_operator_token(secret: &str) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    mint_operator_read_token(secret, now + DEFAULT_OPERATOR_TOKEN_TTL_SECONDS)
}

/// Acceptance criterion 1: with NO `[rpc.operator]` config, the operator RPCs
/// return a clean "not enabled" error (and the node otherwise behaves as
/// today).
#[tokio::test]
#[serial]
async fn test_operator_disabled_without_config() {
    let (_temp_dir, addr, _handle) = spawn_operator_rpc_server(None, None).await;
    let client = Client::new();

    for method in ["operator_getQuorumInfo", "operator_getAuditLog"] {
        // Even WITH a plausible token, no secret ⇒ feature off.
        let resp = rpc_call(
            &client,
            addr,
            method,
            json!({ "token": "op.9999999999.deadbeef" }),
        )
        .await;
        assert!(
            resp["result"].is_null(),
            "{method} should not succeed when disabled"
        );
        assert_eq!(
            resp["error"]["code"].as_i64(),
            Some(-32020),
            "{method} not-enabled code"
        );
        let msg = resp["error"]["message"].as_str().unwrap_or_default();
        assert!(msg.contains("not enabled"), "{method}: {msg}");
    }

    // The node still serves its normal public surface unchanged.
    let status = rpc_call(&client, addr, "node_getStatus", json!({})).await;
    assert!(status["result"].is_object());
}

/// Missing / expired / tampered tokens are all rejected with the SAME generic
/// reason (no leak of which check failed).
#[tokio::test]
#[serial]
async fn test_operator_token_rejections_are_generic() {
    let secret = "integration-operator-secret";
    let (_temp_dir, addr, _handle) = spawn_operator_rpc_server(Some(secret), None).await;
    let client = Client::new();

    // (a) Missing token.
    let missing = rpc_call(&client, addr, "operator_getQuorumInfo", json!({})).await;
    // (b) Malformed token.
    let malformed = rpc_call(
        &client,
        addr,
        "operator_getQuorumInfo",
        json!({ "token": "not-a-token" }),
    )
    .await;
    // (c) Expired-but-validly-signed token.
    let expired_tok = mint_operator_read_token(secret, 1_000_000_000); // year 2001
    let expired = rpc_call(
        &client,
        addr,
        "operator_getQuorumInfo",
        json!({ "token": expired_tok }),
    )
    .await;
    // (d) Tampered token (valid shape, wrong signature).
    let good = valid_operator_token(secret);
    let mut parts: Vec<&str> = good.split('.').collect();
    let forged_exp = format!("op.{}.{}", 9_999_999_999u64, parts.pop().unwrap());
    let tampered = rpc_call(
        &client,
        addr,
        "operator_getQuorumInfo",
        json!({ "token": forged_exp }),
    )
    .await;
    // (e) Token signed with a DIFFERENT secret.
    let wrong_secret_tok = valid_operator_token("some-other-secret");
    let wrong_secret = rpc_call(
        &client,
        addr,
        "operator_getQuorumInfo",
        json!({ "token": wrong_secret_tok }),
    )
    .await;

    for (label, resp) in [
        ("missing", &missing),
        ("malformed", &malformed),
        ("expired", &expired),
        ("tampered", &tampered),
        ("wrong-secret", &wrong_secret),
    ] {
        assert!(resp["result"].is_null(), "{label} must not succeed");
        assert_eq!(
            resp["error"]["code"].as_i64(),
            Some(-32021),
            "{label} must use the generic rejection code"
        );
        // Every rejection reason is byte-for-byte identical (no oracle).
        assert_eq!(
            resp["error"]["message"].as_str(),
            Some("operator token invalid or expired"),
            "{label} reason must be generic"
        );
    }
}

/// A valid token unlocks `operator_getQuorumInfo`; with no gate evaluation yet
/// the per-peer classification is `null` (anti-#541), never fabricated.
#[tokio::test]
#[serial]
async fn test_operator_quorum_info_per_peer_null_until_evaluated() {
    let secret = "integration-operator-secret-2";
    let (_temp_dir, addr, _handle) = spawn_operator_rpc_server(Some(secret), None).await;
    let client = Client::new();
    let token = valid_operator_token(secret);

    let resp = rpc_call(
        &client,
        addr,
        "operator_getQuorumInfo",
        json!({ "token": token }),
    )
    .await;
    assert!(
        resp["error"].is_null(),
        "valid token must succeed: {:?}",
        resp["error"]
    );
    let result = &resp["result"];
    // Configured quorum contents are present.
    assert!(result["quorum"].is_object());
    assert!(result["quorum"]["members"].is_array());
    // Per-peer classification is null until the gate runs — NOT an empty map.
    assert!(
        result["perPeer"].is_null(),
        "perPeer must be null pre-evaluation"
    );
    assert!(result["gate"].is_null(), "gate must be null pre-evaluation");
}

/// After a REAL gate evaluation publishes a snapshot, `operator_getQuorumInfo`
/// surfaces the exact per-peer classification the gate produced.
#[tokio::test]
#[serial]
async fn test_operator_quorum_info_per_peer_after_evaluation() {
    let secret = "integration-operator-secret-3";
    let snapshot = QuorumGateSnapshot {
        curated_members: 1,
        auto_members: 1,
        suppressed_peers: 1,
        max_auto_members: 8,
        intersection_refused: false,
        curated_peer_ids: vec!["12D3KooWCurated".to_string()],
        auto_peer_ids: vec!["12D3KooWAuto".to_string()],
        suppressed_peer_ids: vec!["12D3KooWSuppressed".to_string()],
    };
    let (_temp_dir, addr, _handle) = spawn_operator_rpc_server(Some(secret), Some(snapshot)).await;
    let client = Client::new();
    let token = valid_operator_token(secret);

    let resp = rpc_call(
        &client,
        addr,
        "operator_getQuorumInfo",
        json!({ "token": token }),
    )
    .await;
    let per_peer = &resp["result"]["perPeer"];
    assert_eq!(per_peer["curated"][0], "12D3KooWCurated");
    assert_eq!(per_peer["auto"][0], "12D3KooWAuto");
    assert_eq!(per_peer["suppressed"][0], "12D3KooWSuppressed");
    assert_eq!(resp["result"]["gate"]["intersectionRefused"], json!(false));
}

/// `operator_getAuditLog` is present-but-empty in P4.2 (writes are #709).
#[tokio::test]
#[serial]
async fn test_operator_audit_log_empty_but_present() {
    let secret = "integration-operator-secret-4";
    let (_temp_dir, addr, _handle) = spawn_operator_rpc_server(Some(secret), None).await;
    let client = Client::new();
    let token = valid_operator_token(secret);

    let resp = rpc_call(
        &client,
        addr,
        "operator_getAuditLog",
        json!({ "token": token }),
    )
    .await;
    assert!(resp["error"].is_null());
    assert!(resp["result"]["entries"].is_array());
    assert_eq!(resp["result"]["entries"].as_array().unwrap().len(), 0);
    assert_eq!(resp["result"]["count"], json!(0));
}

/// The token grants READS ONLY: there is no operator write RPC. A plausible
/// write method name must be an unknown method, even WITH a valid token.
#[tokio::test]
#[serial]
async fn test_operator_token_grants_no_write_method() {
    let secret = "integration-operator-secret-5";
    let (_temp_dir, addr, _handle) = spawn_operator_rpc_server(Some(secret), None).await;
    let client = Client::new();
    let token = valid_operator_token(secret);

    // #709's write method must NOT exist in this build.
    let resp = rpc_call(
        &client,
        addr,
        "operator_submitAction",
        json!({ "token": token }),
    )
    .await;
    assert!(resp["result"].is_null());
    // Method-not-found (-32601), i.e. no write surface is reachable at all.
    assert_eq!(resp["error"]["code"].as_i64(), Some(-32601));
}

#[tokio::test]
#[serial]
async fn test_websocket_plain_get_returns_400() {
    let (_temp_dir, addr, _handle) = spawn_test_rpc_server().await;

    // A GET to /ws without the upgrade headers must be rejected with 400,
    // not silently treated as a socket.
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    let req = format!("GET /ws HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).await.expect("write req");

    let head = read_http_head(&mut stream).await;
    assert!(
        head.starts_with("HTTP/1.1 400"),
        "expected 400 Bad Request for non-upgrade GET, got:\n{head}"
    );
}
