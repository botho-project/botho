//! JSON-RPC Server for Botho
//!
//! Provides a JSON-RPC 2.0 API for thin wallets and web interfaces.
//! Also supports WebSocket connections for real-time event streaming.

pub mod auth;
pub mod deposit_scanner;
pub mod view_keys;
pub mod websocket;

pub use auth::{ApiKeyConfig, ApiPermissions, AuthError, HmacAuthenticator};
pub use deposit_scanner::{DepositScanner, ScanResult};
pub use view_keys::{RegistryError, ViewKeyInfo, ViewKeyRegistry};
pub use websocket::WsBroadcaster;

use anyhow::Result;

/// JSON-RPC internal error code
const INTERNAL_ERROR: i32 = -32603;

/// Helper macro to acquire a read lock, returning a JSON-RPC error if poisoned
macro_rules! read_lock {
    ($lock:expr, $id:expr) => {
        match $lock.read() {
            Ok(guard) => guard,
            Err(_) => return JsonRpcResponse::error($id, INTERNAL_ERROR, "Internal error: lock poisoned"),
        }
    };
}

/// Helper macro to acquire a write lock, returning a JSON-RPC error if poisoned
macro_rules! write_lock {
    ($lock:expr, $id:expr) => {
        match $lock.write() {
            Ok(guard) => guard,
            Err(_) => return JsonRpcResponse::error($id, INTERNAL_ERROR, "Internal error: lock poisoned"),
        }
    };
}
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use tokio::net::TcpListener;
use tracing::{debug, error, info};

use crate::address::Address;
use crate::ledger::Ledger;
use crate::mempool::Mempool;

/// JSON-RPC request
#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub method: String,
    pub params: Value,
    pub id: Value,
}

/// JSON-RPC response
#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
    pub id: Value,
}

/// JSON-RPC error
#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcResponse {
    pub fn success(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            result: Some(result),
            error: None,
            id,
        }
    }

    pub fn error(id: Value, code: i32, message: &str) -> Self {
        Self {
            jsonrpc: "2.0",
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.to_string(),
                data: None,
            }),
            id,
        }
    }
}

/// Shared RPC state
pub struct RpcState {
    pub ledger: Arc<RwLock<Ledger>>,
    pub mempool: Arc<RwLock<Mempool>>,
    pub minting_active: Arc<RwLock<bool>>,
    pub minting_threads: usize,
    pub peer_count: Arc<RwLock<usize>>,
    pub start_time: std::time::Instant,
    /// Wallet view key (None if running in relay mode)
    pub wallet_view_key: Option<[u8; 32]>,
    /// Wallet spend key (None if running in relay mode)
    pub wallet_spend_key: Option<[u8; 32]>,
    /// Allowed CORS origins (e.g., ["http://localhost", "http://127.0.0.1"])
    /// If contains "*", all origins are allowed (insecure)
    pub cors_origins: Vec<String>,
    /// WebSocket event broadcaster
    pub ws_broadcaster: Arc<WsBroadcaster>,
    /// View key registry for exchange deposit notifications
    pub view_key_registry: Arc<ViewKeyRegistry>,
}

impl RpcState {
    pub fn new(
        ledger: Ledger,
        mempool: Mempool,
        wallet_view_key: Option<[u8; 32]>,
        wallet_spend_key: Option<[u8; 32]>,
        cors_origins: Vec<String>,
        ws_broadcaster: Arc<WsBroadcaster>,
    ) -> Self {
        Self {
            ledger: Arc::new(RwLock::new(ledger)),
            mempool: Arc::new(RwLock::new(mempool)),
            minting_active: Arc::new(RwLock::new(false)),
            minting_threads: num_cpus::get(),
            peer_count: Arc::new(RwLock::new(0)),
            start_time: std::time::Instant::now(),
            wallet_view_key,
            wallet_spend_key,
            cors_origins,
            ws_broadcaster,
            view_key_registry: Arc::new(ViewKeyRegistry::new()),
        }
    }

    /// Create RpcState from already-shared components
    pub fn from_shared(
        ledger: Arc<RwLock<Ledger>>,
        mempool: Arc<RwLock<Mempool>>,
        minting_active: Arc<RwLock<bool>>,
        peer_count: Arc<RwLock<usize>>,
        wallet_view_key: Option<[u8; 32]>,
        wallet_spend_key: Option<[u8; 32]>,
        cors_origins: Vec<String>,
        ws_broadcaster: Arc<WsBroadcaster>,
    ) -> Self {
        Self {
            ledger,
            mempool,
            minting_active,
            minting_threads: num_cpus::get(),
            peer_count,
            start_time: std::time::Instant::now(),
            wallet_view_key,
            wallet_spend_key,
            cors_origins,
            ws_broadcaster,
            view_key_registry: Arc::new(ViewKeyRegistry::new()),
        }
    }
}

/// Start the RPC server
pub async fn start_rpc_server(addr: SocketAddr, state: Arc<RpcState>) -> Result<()> {
    let listener = TcpListener::bind(addr).await?;
    info!("RPC server listening on {} (WebSocket: /ws)", addr);

    loop {
        let (stream, _) = listener.accept().await?;
        let io = TokioIo::new(stream);
        let state = state.clone();

        tokio::spawn(async move {
            let service = service_fn(|req| handle_request(req, state.clone()));

            // Use with_upgrades() to support WebSocket connections
            if let Err(err) = http1::Builder::new()
                .serve_connection(io, service)
                .with_upgrades()
                .await
            {
                error!("Error serving connection: {:?}", err);
            }
        });
    }
}

async fn handle_request(
    req: Request<hyper::body::Incoming>,
    state: Arc<RpcState>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    // Extract Origin header for CORS checking
    let request_origin = req
        .headers()
        .get("Origin")
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string());

    // Check if origin is allowed
    let allowed_origin = check_cors_origin(request_origin.as_deref(), &state.cors_origins);
    let allowed_origin_ref = allowed_origin.as_deref();

    // Handle CORS preflight
    if req.method() == Method::OPTIONS {
        return Ok(cors_response(Response::new(Full::new(Bytes::new())), allowed_origin_ref));
    }

    // Check for WebSocket upgrade request at /ws
    if req.method() == Method::GET && req.uri().path() == "/ws" {
        return handle_websocket_upgrade(req, state).await;
    }

    // Only accept POST for JSON-RPC
    if req.method() != Method::POST {
        return Ok(cors_response(
            Response::builder()
                .status(StatusCode::METHOD_NOT_ALLOWED)
                .body(Full::new(Bytes::from("Method not allowed")))
                .unwrap(),
            allowed_origin_ref,
        ));
    }

    // Read body
    let body_bytes = match req.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(e) => {
            error!("Failed to read request body: {}", e);
            return Ok(cors_response(
                Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(Full::new(Bytes::from("Failed to read body")))
                    .unwrap(),
                allowed_origin_ref,
            ));
        }
    };

    // Parse JSON-RPC request
    let rpc_request: JsonRpcRequest = match serde_json::from_slice(&body_bytes) {
        Ok(req) => req,
        Err(e) => {
            error!("Failed to parse JSON-RPC request: {}", e);
            let response = JsonRpcResponse::error(Value::Null, -32700, "Parse error");
            return Ok(json_response(response, allowed_origin_ref));
        }
    };

    debug!("RPC request: {} (id: {})", rpc_request.method, rpc_request.id);

    // Handle the request
    let response = handle_rpc_method(&rpc_request, &state).await;

    Ok(json_response(response, allowed_origin_ref))
}

/// Handle WebSocket upgrade request
async fn handle_websocket_upgrade(
    req: Request<hyper::body::Incoming>,
    state: Arc<RpcState>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    // Check for required WebSocket headers
    let has_upgrade = req
        .headers()
        .get("Upgrade")
        .map(|v| v.to_str().unwrap_or("").eq_ignore_ascii_case("websocket"))
        .unwrap_or(false);

    let has_connection = req
        .headers()
        .get("Connection")
        .map(|v| v.to_str().unwrap_or("").to_lowercase().contains("upgrade"))
        .unwrap_or(false);

    let sec_websocket_key = req.headers().get("Sec-WebSocket-Key").cloned();

    if !has_upgrade || !has_connection || sec_websocket_key.is_none() {
        return Ok(Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(Full::new(Bytes::from("Missing WebSocket headers")))
            .unwrap());
    }

    // Calculate the accept key
    let key = sec_websocket_key.unwrap();
    let accept_key = compute_websocket_accept_key(key.to_str().unwrap_or(""));

    // Spawn task to handle the WebSocket connection after upgrade
    let broadcaster = state.ws_broadcaster.clone();
    tokio::spawn(async move {
        match hyper::upgrade::on(req).await {
            Ok(upgraded) => {
                websocket::handle_websocket(upgraded, broadcaster).await;
            }
            Err(e) => {
                error!("WebSocket upgrade failed: {}", e);
            }
        }
    });

    // Return 101 Switching Protocols
    Ok(Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header("Upgrade", "websocket")
        .header("Connection", "Upgrade")
        .header("Sec-WebSocket-Accept", accept_key)
        .body(Full::new(Bytes::new()))
        .unwrap())
}

/// Compute the Sec-WebSocket-Accept header value
fn compute_websocket_accept_key(key: &str) -> String {
    use sha1::{Digest, Sha1};
    const WEBSOCKET_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

    let mut hasher = Sha1::new();
    hasher.update(key.as_bytes());
    hasher.update(WEBSOCKET_GUID.as_bytes());
    let result = hasher.finalize();

    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(result)
}

async fn handle_rpc_method(request: &JsonRpcRequest, state: &RpcState) -> JsonRpcResponse {
    let id = request.id.clone();

    match request.method.as_str() {
        // Node methods
        "node_getStatus" => handle_node_status(id, state).await,

        // Chain methods
        "getChainInfo" => handle_chain_info(id, state).await,
        "getSupplyInfo" => handle_supply_info(id, state).await,
        "getBlockByHeight" => handle_get_block(id, &request.params, state).await,
        "getMempoolInfo" => handle_mempool_info(id, state).await,
        "estimateFee" | "tx_estimateFee" => handle_estimate_fee(id, &request.params, state).await,

        // Wallet methods (for thin wallet sync)
        "chain_getOutputs" => handle_get_outputs(id, &request.params, state).await,
        "wallet_getBalance" => handle_wallet_balance(id, state).await,
        "wallet_getAddress" => handle_wallet_address(id, state).await,

        // Transaction methods
        "tx_submit" | "sendRawTransaction" => handle_submit_tx(id, &request.params, state).await,
        "pq_tx_submit" => handle_submit_pq_tx(id, &request.params, state).await,
        "getTransaction" | "tx_get" => handle_get_transaction(id, &request.params, state).await,
        "getTransactionStatus" | "tx_getStatus" => handle_get_transaction_status(id, &request.params, state).await,

        // Address methods (for exchange integration)
        "validateAddress" | "address_validate" => handle_validate_address(id, &request.params, state).await,

        // Minting methods
        "minting_getStatus" => handle_minting_status(id, state).await,

        // Network methods
        "network_getInfo" => handle_network_info(id, state).await,
        "network_getPeers" => handle_get_peers(id, state).await,

        // Exchange integration methods
        "exchange_registerViewKey" => handle_register_view_key(id, &request.params, state).await,
        "exchange_unregisterViewKey" => handle_unregister_view_key(id, &request.params, state).await,
        "exchange_listViewKeys" => handle_list_view_keys(id, &request.params, state).await,

        _ => JsonRpcResponse::error(id, -32601, &format!("Method not found: {}", request.method)),
    }
}

// Handler implementations

async fn handle_node_status(id: Value, state: &RpcState) -> JsonRpcResponse {
    let ledger = read_lock!(state.ledger, id.clone());
    let chain_state = ledger.get_chain_state().unwrap_or_default();
    let minting = *read_lock!(state.minting_active, id.clone());
    let mempool = read_lock!(state.mempool, id.clone());
    let peers = *read_lock!(state.peer_count, id.clone());

    JsonRpcResponse::success(id, json!({
        "version": env!("CARGO_PKG_VERSION"),
        "network": "botho-mainnet",
        "uptimeSeconds": state.start_time.elapsed().as_secs(),
        "syncStatus": "synced",
        "chainHeight": chain_state.height,
        "tipHash": hex::encode(chain_state.tip_hash),
        "peerCount": peers,
        "mempoolSize": mempool.len(),
        "mintingActive": minting,
    }))
}

async fn handle_chain_info(id: Value, state: &RpcState) -> JsonRpcResponse {
    let ledger = read_lock!(state.ledger, id.clone());
    let chain_state = ledger.get_chain_state().unwrap_or_default();
    let mempool = read_lock!(state.mempool, id.clone());

    // Calculate circulating supply: total mined minus fees burned
    let circulating_supply = chain_state
        .total_mined
        .saturating_sub(chain_state.total_fees_burned);

    JsonRpcResponse::success(id, json!({
        "height": chain_state.height,
        "tipHash": hex::encode(chain_state.tip_hash),
        "difficulty": chain_state.difficulty,
        "totalMined": chain_state.total_mined,
        "totalFeesBurned": chain_state.total_fees_burned,
        "circulatingSupply": circulating_supply,
        "mempoolSize": mempool.len(),
        "mempoolFees": mempool.total_fees(),
    }))
}

/// Get supply information for accurate circulating supply measurement
///
/// Returns:
/// - `totalMined`: Gross emission from block rewards (all BTH ever created)
/// - `totalFeesBurned`: Cumulative transaction fees burned (removed from supply)
/// - `circulatingSupply`: Net supply = totalMined - totalFeesBurned
async fn handle_supply_info(id: Value, state: &RpcState) -> JsonRpcResponse {
    let ledger = read_lock!(state.ledger, id.clone());
    let chain_state = ledger.get_chain_state().unwrap_or_default();

    // Calculate circulating supply: total mined minus fees burned
    let circulating_supply = chain_state
        .total_mined
        .saturating_sub(chain_state.total_fees_burned);

    JsonRpcResponse::success(id, json!({
        "height": chain_state.height,
        "totalMined": chain_state.total_mined,
        "totalFeesBurned": chain_state.total_fees_burned,
        "circulatingSupply": circulating_supply,
    }))
}

async fn handle_get_block(id: Value, params: &Value, state: &RpcState) -> JsonRpcResponse {
    let height = params.get("height").and_then(|v| v.as_u64()).unwrap_or(0);
    let ledger = read_lock!(state.ledger, id.clone());

    match ledger.get_block(height) {
        Ok(block) => JsonRpcResponse::success(id, json!({
            "height": block.height(),
            "hash": hex::encode(block.hash()),
            "prevHash": hex::encode(block.header.prev_block_hash),
            "timestamp": block.header.timestamp,
            "difficulty": block.header.difficulty,
            "nonce": block.header.nonce,
            "txCount": block.transactions.len(),
            "mintingReward": block.minting_tx.reward,
        })),
        Err(e) => JsonRpcResponse::error(id, -32000, &format!("Block not found: {}", e)),
    }
}

async fn handle_mempool_info(id: Value, state: &RpcState) -> JsonRpcResponse {
    let mempool = read_lock!(state.mempool, id.clone());

    // Get transaction hashes from mempool
    let txs = mempool.get_transactions(100);
    let tx_hashes: Vec<String> = txs.iter().map(|tx| hex::encode(tx.hash())).collect();

    JsonRpcResponse::success(id, json!({
        "size": mempool.len(),
        "totalFees": mempool.total_fees(),
        "txHashes": tx_hashes,
    }))
}

async fn handle_estimate_fee(id: Value, params: &Value, state: &RpcState) -> JsonRpcResponse {
    // Parse parameters
    let amount = params.get("amount").and_then(|v| v.as_u64()).unwrap_or(0);
    let is_private = params.get("private").and_then(|v| v.as_bool()).unwrap_or(true);
    let num_memos = params.get("memos").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

    let mempool = read_lock!(state.mempool, id.clone());

    // Calculate minimum fee using the fee curve
    let minimum_fee = mempool.estimate_fee(is_private, amount, num_memos);

    // Get fee rate in basis points for display
    let fee_rate_bps = mempool.fee_rate_bps(is_private);

    // Calculate average mempool fee for priority estimation
    let avg_fee = if mempool.len() > 0 {
        mempool.total_fees() / mempool.len() as u64
    } else {
        minimum_fee
    };

    JsonRpcResponse::success(id, json!({
        "minimumFee": minimum_fee,
        "feeRateBps": fee_rate_bps,
        "recommendedFee": avg_fee.max(minimum_fee),
        "highPriorityFee": (avg_fee * 2).max(minimum_fee * 2),
        "params": {
            "amount": amount,
            "private": is_private,
            "memos": num_memos,
        }
    }))
}

async fn handle_get_outputs(id: Value, params: &Value, state: &RpcState) -> JsonRpcResponse {
    let start_height = params.get("start_height").and_then(|v| v.as_u64()).unwrap_or(0);
    let end_height = params.get("end_height").and_then(|v| v.as_u64()).unwrap_or(start_height + 100);

    let ledger = read_lock!(state.ledger, id.clone());
    let mut blocks = Vec::new();

    for height in start_height..=end_height {
        if let Ok(block) = ledger.get_block(height) {
            let mut outputs = Vec::new();
            for tx in &block.transactions {
                for (idx, output) in tx.outputs.iter().enumerate() {
                    outputs.push(json!({
                        "txHash": hex::encode(tx.hash()),
                        "outputIndex": idx,
                        "targetKey": hex::encode(output.target_key),
                        "publicKey": hex::encode(output.public_key),
                        "amountCommitment": hex::encode(output.amount.to_le_bytes()),
                    }));
                }
            }
            blocks.push(json!({
                "height": height,
                "outputs": outputs,
            }));
        }
    }

    JsonRpcResponse::success(id, json!(blocks))
}

async fn handle_wallet_balance(id: Value, _state: &RpcState) -> JsonRpcResponse {
    // For now, return placeholder values
    // A full implementation would scan UTXOs for the wallet address
    // This requires iterating through all blocks which is expensive
    // The thin wallet should sync locally instead

    JsonRpcResponse::success(id, json!({
        "confirmed": 0,
        "pending": 0,
        "total": 0,
        "utxoCount": 0,
    }))
}

async fn handle_wallet_address(id: Value, state: &RpcState) -> JsonRpcResponse {
    // Return null keys if running in relay mode (no wallet)
    let view_key = state.wallet_view_key
        .map(|k| hex::encode(&k))
        .unwrap_or_default();
    let spend_key = state.wallet_spend_key
        .map(|k| hex::encode(&k))
        .unwrap_or_default();

    JsonRpcResponse::success(id, json!({
        "viewKey": view_key,
        "spendKey": spend_key,
        "hasWallet": state.wallet_view_key.is_some(),
    }))
}

async fn handle_submit_tx(id: Value, params: &Value, state: &RpcState) -> JsonRpcResponse {
    let tx_hex = match params.get("tx_hex").and_then(|v| v.as_str()) {
        Some(hex) => hex,
        None => return JsonRpcResponse::error(id, -32602, "Missing tx_hex parameter"),
    };

    let tx_bytes = match hex::decode(tx_hex) {
        Ok(bytes) => bytes,
        Err(_) => return JsonRpcResponse::error(id, -32602, "Invalid hex encoding"),
    };

    let tx: crate::transaction::Transaction = match bincode::deserialize(&tx_bytes) {
        Ok(tx) => tx,
        Err(e) => return JsonRpcResponse::error(id, -32602, &format!("Invalid transaction: {}", e)),
    };

    let ledger = read_lock!(state.ledger, id.clone());
    let mut mempool = write_lock!(state.mempool, id.clone());

    match mempool.add_tx(tx, &ledger) {
        Ok(hash) => JsonRpcResponse::success(id, json!({
            "txHash": hex::encode(hash),
        })),
        Err(e) => JsonRpcResponse::error(id, -32000, &format!("Failed to add transaction: {}", e)),
    }
}

/// Handle quantum-private transaction submission
#[cfg(feature = "pq")]
async fn handle_submit_pq_tx(id: Value, params: &Value, _state: &RpcState) -> JsonRpcResponse {
    use crate::transaction_pq::QuantumPrivateTransaction;

    let tx_hex = match params.get("tx_hex").and_then(|v| v.as_str()) {
        Some(hex) => hex,
        None => return JsonRpcResponse::error(id, -32602, "Missing tx_hex parameter"),
    };

    let tx_bytes = match hex::decode(tx_hex) {
        Ok(bytes) => bytes,
        Err(_) => return JsonRpcResponse::error(id, -32602, "Invalid hex encoding"),
    };

    let tx: QuantumPrivateTransaction = match bincode::deserialize(&tx_bytes) {
        Ok(tx) => tx,
        Err(e) => return JsonRpcResponse::error(id, -32602, &format!("Invalid PQ transaction: {}", e)),
    };

    // Validate structure
    if let Err(e) = tx.is_valid_structure() {
        return JsonRpcResponse::error(id, -32602, &format!("Invalid PQ transaction structure: {}", e));
    }

    // Validate fee
    if !tx.has_sufficient_fee() {
        return JsonRpcResponse::error(id, -32602, &format!(
            "Insufficient fee: {} < {} required",
            tx.fee,
            tx.minimum_fee()
        ));
    }

    // TODO: Full validation requires checking:
    // 1. UTXO existence in ledger
    // 2. Classical signature verification
    // 3. PQ signature verification against stored pq_signing_pubkey
    // 4. Adding to mempool (requires PQ-aware mempool)
    //
    // For now, return success with the transaction hash
    // The transaction will be validated when included in a block
    let tx_hash = tx.hash();

    info!("Received PQ transaction: {}", hex::encode(&tx_hash[..8]));

    JsonRpcResponse::success(id, json!({
        "txHash": hex::encode(tx_hash),
        "type": "quantum-private",
        "size": tx.estimated_size(),
    }))
}

/// Fallback for non-PQ builds
#[cfg(not(feature = "pq"))]
async fn handle_submit_pq_tx(id: Value, _params: &Value, _state: &RpcState) -> JsonRpcResponse {
    JsonRpcResponse::error(id, -32601, "PQ transactions not enabled in this build")
}

/// Get a transaction by hash (for exchange integration)
///
/// Returns transaction details including block height, confirmations, and status.
async fn handle_get_transaction(id: Value, params: &Value, state: &RpcState) -> JsonRpcResponse {
    // Parse tx_hash parameter
    let tx_hash_hex = match params.get("tx_hash").or_else(|| params.get("hash")).and_then(|v| v.as_str()) {
        Some(hex) => hex,
        None => return JsonRpcResponse::error(id, -32602, "Missing tx_hash parameter"),
    };

    let tx_hash: [u8; 32] = match hex::decode(tx_hash_hex) {
        Ok(bytes) if bytes.len() == 32 => {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            arr
        }
        _ => return JsonRpcResponse::error(id, -32602, "Invalid tx_hash: expected 32-byte hex string"),
    };

    let ledger = read_lock!(state.ledger, id.clone());

    // First check mempool
    let mempool = read_lock!(state.mempool, id.clone());
    if mempool.contains(&tx_hash) {
        return JsonRpcResponse::success(id, json!({
            "txHash": tx_hash_hex,
            "status": "pending",
            "blockHeight": null,
            "confirmations": 0,
            "inMempool": true,
        }));
    }
    drop(mempool);

    // Look up in blockchain
    match ledger.get_transaction(&tx_hash) {
        Ok(Some((tx, block_height, confirmations))) => {
            let tx_type = if tx.is_private() { "ring" } else { "simple" };
            let output_count = tx.outputs.len();
            let total_output: u64 = tx.outputs.iter().map(|o| o.amount).sum();

            JsonRpcResponse::success(id, json!({
                "txHash": tx_hash_hex,
                "status": "confirmed",
                "blockHeight": block_height,
                "confirmations": confirmations,
                "inMempool": false,
                "type": tx_type,
                "fee": tx.fee,
                "outputCount": output_count,
                "totalOutput": total_output,
                "createdAtHeight": tx.created_at_height,
            }))
        }
        Ok(None) => {
            JsonRpcResponse::error(id, -32000, "Transaction not found")
        }
        Err(e) => {
            JsonRpcResponse::error(id, -32000, &format!("Failed to get transaction: {}", e))
        }
    }
}

/// Get transaction status and confirmation count (for exchange integration)
///
/// Lightweight version of getTransaction that only returns status info.
async fn handle_get_transaction_status(id: Value, params: &Value, state: &RpcState) -> JsonRpcResponse {
    // Parse tx_hash parameter
    let tx_hash_hex = match params.get("tx_hash").or_else(|| params.get("hash")).and_then(|v| v.as_str()) {
        Some(hex) => hex,
        None => return JsonRpcResponse::error(id, -32602, "Missing tx_hash parameter"),
    };

    let tx_hash: [u8; 32] = match hex::decode(tx_hash_hex) {
        Ok(bytes) if bytes.len() == 32 => {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            arr
        }
        _ => return JsonRpcResponse::error(id, -32602, "Invalid tx_hash: expected 32-byte hex string"),
    };

    // Check mempool first
    let mempool = read_lock!(state.mempool, id.clone());
    if mempool.contains(&tx_hash) {
        return JsonRpcResponse::success(id, json!({
            "txHash": tx_hash_hex,
            "status": "pending",
            "confirmations": 0,
            "confirmed": false,
        }));
    }
    drop(mempool);

    // Look up in blockchain
    let ledger = read_lock!(state.ledger, id.clone());
    match ledger.get_transaction_confirmations(&tx_hash) {
        Ok(Some(confirmations)) => {
            JsonRpcResponse::success(id, json!({
                "txHash": tx_hash_hex,
                "status": "confirmed",
                "confirmations": confirmations,
                "confirmed": true,
            }))
        }
        Ok(None) => {
            JsonRpcResponse::success(id, json!({
                "txHash": tx_hash_hex,
                "status": "unknown",
                "confirmations": 0,
                "confirmed": false,
            }))
        }
        Err(e) => {
            JsonRpcResponse::error(id, -32000, &format!("Failed to get transaction status: {}", e))
        }
    }
}

/// Validate an address (for exchange integration)
///
/// Parses and validates a Botho address, returning its properties.
async fn handle_validate_address(id: Value, params: &Value, _state: &RpcState) -> JsonRpcResponse {
    // Parse address parameter
    let address_str = match params.get("address").and_then(|v| v.as_str()) {
        Some(addr) => addr,
        None => return JsonRpcResponse::error(id, -32602, "Missing address parameter"),
    };

    // Try to parse the address
    match Address::parse(address_str) {
        Ok(addr) => {
            let network = addr.network.display_name();
            let is_quantum = addr.is_quantum();
            let address_type = if is_quantum { "quantum" } else { "classical" };

            // Get the canonical form
            let canonical = addr.to_address_string();

            JsonRpcResponse::success(id, json!({
                "valid": true,
                "address": canonical,
                "network": network,
                "type": address_type,
                "isQuantum": is_quantum,
            }))
        }
        Err(e) => {
            JsonRpcResponse::success(id, json!({
                "valid": false,
                "error": e.to_string(),
                "address": address_str,
            }))
        }
    }
}

async fn handle_minting_status(id: Value, state: &RpcState) -> JsonRpcResponse {
    let active = *read_lock!(state.minting_active, id.clone());
    let ledger = read_lock!(state.ledger, id.clone());
    let chain_state = ledger.get_chain_state().unwrap_or_default();

    JsonRpcResponse::success(id, json!({
        "active": active,
        "threads": state.minting_threads,
        "hashrate": 0.0, // TODO: track actual hashrate
        "totalHashes": 0,
        "blocksFound": 0, // TODO: track blocks found
        "currentDifficulty": chain_state.difficulty,
        "uptimeSeconds": state.start_time.elapsed().as_secs(),
    }))
}

async fn handle_network_info(id: Value, state: &RpcState) -> JsonRpcResponse {
    let peers = *read_lock!(state.peer_count, id.clone());

    JsonRpcResponse::success(id, json!({
        "peerCount": peers,
        "inboundCount": 0,
        "outboundCount": peers,
        "bytesSent": 0,
        "bytesReceived": 0,
        "uptimeSeconds": state.start_time.elapsed().as_secs(),
    }))
}

async fn handle_get_peers(id: Value, _state: &RpcState) -> JsonRpcResponse {
    // Return empty for now - would need to get actual peer addresses
    JsonRpcResponse::success(id, json!({
        "peers": []
    }))
}

/// Check if the given origin is allowed based on the CORS configuration.
/// Returns the origin to echo back if allowed, or None if denied.
fn check_cors_origin(request_origin: Option<&str>, allowed_origins: &[String]) -> Option<String> {
    let origin = request_origin?;

    // Check for wildcard - allows all origins
    if allowed_origins.iter().any(|o| o == "*") {
        return Some(origin.to_string());
    }

    // Check if origin matches any allowed origin (prefix match for localhost:port)
    for allowed in allowed_origins {
        if origin == allowed {
            return Some(origin.to_string());
        }
        // Allow localhost with any port (e.g., "http://localhost" matches "http://localhost:3000")
        if origin.starts_with(allowed) && (allowed.ends_with("localhost") || allowed.ends_with("127.0.0.1")) {
            let suffix = &origin[allowed.len()..];
            if suffix.is_empty() || suffix.starts_with(':') {
                return Some(origin.to_string());
            }
        }
    }

    None
}

fn cors_response(mut response: Response<Full<Bytes>>, allowed_origin: Option<&str>) -> Response<Full<Bytes>> {
    let headers = response.headers_mut();

    if let Some(origin) = allowed_origin {
        headers.insert("Access-Control-Allow-Origin", origin.parse().unwrap());
        headers.insert("Access-Control-Allow-Methods", "POST, OPTIONS".parse().unwrap());
        headers.insert("Access-Control-Allow-Headers", "Content-Type".parse().unwrap());
        headers.insert("Vary", "Origin".parse().unwrap());
    }
    // If no allowed origin, we don't set CORS headers - browser will block the request

    response
}

fn json_response(response: JsonRpcResponse, allowed_origin: Option<&str>) -> Response<Full<Bytes>> {
    let body = serde_json::to_string(&response).unwrap();
    cors_response(
        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(Full::new(Bytes::from(body)))
            .unwrap(),
        allowed_origin,
    )
}

// ============================================================================
// Exchange Integration Handlers
// ============================================================================

/// Register a view key for deposit notifications.
///
/// # Parameters
/// - `id`: Unique identifier for this registration
/// - `view_private_key`: 64-character hex string (32 bytes)
/// - `spend_public_key`: 64-character hex string (32 bytes)
/// - `subaddress_min`: Minimum subaddress index to scan (default: 0)
/// - `subaddress_max`: Maximum subaddress index to scan (default: 1000)
/// - `api_key_id`: API key making this registration (for auth tracking)
async fn handle_register_view_key(id: Value, params: &Value, state: &RpcState) -> JsonRpcResponse {
    use bth_crypto_keys::{RistrettoPrivate, RistrettoPublic};

    // Parse registration ID
    let reg_id = match params.get("id").and_then(|v| v.as_str()) {
        Some(id) => id.to_string(),
        None => return JsonRpcResponse::error(id, -32602, "Missing 'id' parameter"),
    };

    // Parse view private key
    let view_key_hex = match params.get("view_private_key").and_then(|v| v.as_str()) {
        Some(hex) => hex,
        None => return JsonRpcResponse::error(id, -32602, "Missing 'view_private_key' parameter"),
    };

    if view_key_hex.len() != 64 {
        return JsonRpcResponse::error(id, -32602, "view_private_key must be 64 hex characters");
    }

    let view_private_bytes: [u8; 32] = match hex::decode(view_key_hex) {
        Ok(bytes) if bytes.len() == 32 => bytes.try_into().unwrap(),
        _ => return JsonRpcResponse::error(id, -32602, "Invalid view_private_key hex"),
    };

    let view_private = match RistrettoPrivate::try_from(&view_private_bytes[..]) {
        Ok(k) => k,
        Err(_) => return JsonRpcResponse::error(id, -32602, "Invalid view private key format"),
    };

    // Parse spend public key
    let spend_key_hex = match params.get("spend_public_key").and_then(|v| v.as_str()) {
        Some(hex) => hex,
        None => return JsonRpcResponse::error(id, -32602, "Missing 'spend_public_key' parameter"),
    };

    if spend_key_hex.len() != 64 {
        return JsonRpcResponse::error(id, -32602, "spend_public_key must be 64 hex characters");
    }

    let spend_public_bytes: [u8; 32] = match hex::decode(spend_key_hex) {
        Ok(bytes) if bytes.len() == 32 => bytes.try_into().unwrap(),
        _ => return JsonRpcResponse::error(id, -32602, "Invalid spend_public_key hex"),
    };

    let spend_public = match RistrettoPublic::try_from(&spend_public_bytes[..]) {
        Ok(k) => k,
        Err(_) => return JsonRpcResponse::error(id, -32602, "Invalid spend public key format"),
    };

    // Parse subaddress range
    let subaddress_min = params.get("subaddress_min").and_then(|v| v.as_u64()).unwrap_or(0);
    let subaddress_max = params.get("subaddress_max").and_then(|v| v.as_u64()).unwrap_or(1000);

    // API key ID (in production, this would come from auth middleware)
    let api_key_id = params
        .get("api_key_id")
        .and_then(|v| v.as_str())
        .unwrap_or("default")
        .to_string();

    // Register
    match state.view_key_registry.register(
        reg_id.clone(),
        view_private,
        spend_public,
        subaddress_min,
        subaddress_max,
        api_key_id,
    ) {
        Ok(()) => JsonRpcResponse::success(
            id,
            json!({
                "registered": true,
                "id": reg_id,
                "subaddress_min": subaddress_min,
                "subaddress_max": subaddress_max,
                "subaddress_count": subaddress_max - subaddress_min + 1,
            }),
        ),
        Err(e) => JsonRpcResponse::error(id, -32000, &format!("Registration failed: {}", e)),
    }
}

/// Unregister a view key.
///
/// # Parameters
/// - `id`: Registration ID to remove
/// - `api_key_id`: API key that registered this key (for authorization)
async fn handle_unregister_view_key(id: Value, params: &Value, state: &RpcState) -> JsonRpcResponse {
    let reg_id = match params.get("id").and_then(|v| v.as_str()) {
        Some(id) => id,
        None => return JsonRpcResponse::error(id, -32602, "Missing 'id' parameter"),
    };

    let api_key_id = params
        .get("api_key_id")
        .and_then(|v| v.as_str())
        .unwrap_or("default");

    match state.view_key_registry.unregister(reg_id, api_key_id) {
        Ok(()) => JsonRpcResponse::success(
            id,
            json!({
                "unregistered": true,
                "id": reg_id,
            }),
        ),
        Err(e) => JsonRpcResponse::error(id, -32000, &format!("Unregistration failed: {}", e)),
    }
}

/// List view keys registered by an API key.
///
/// # Parameters
/// - `api_key_id`: API key to list registrations for
async fn handle_list_view_keys(id: Value, params: &Value, state: &RpcState) -> JsonRpcResponse {
    let api_key_id = params
        .get("api_key_id")
        .and_then(|v| v.as_str())
        .unwrap_or("default");

    match state.view_key_registry.list_by_api_key(api_key_id) {
        Ok(keys) => JsonRpcResponse::success(
            id,
            json!({
                "count": keys.len(),
                "view_keys": keys,
            }),
        ),
        Err(e) => JsonRpcResponse::error(id, -32000, &format!("List failed: {}", e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cors_wildcard_allows_any_origin() {
        let allowed = vec!["*".to_string()];
        assert_eq!(
            check_cors_origin(Some("http://evil.com"), &allowed),
            Some("http://evil.com".to_string())
        );
        assert_eq!(
            check_cors_origin(Some("http://localhost:3000"), &allowed),
            Some("http://localhost:3000".to_string())
        );
    }

    #[test]
    fn test_cors_localhost_allows_any_port() {
        let allowed = vec!["http://localhost".to_string()];
        assert_eq!(
            check_cors_origin(Some("http://localhost"), &allowed),
            Some("http://localhost".to_string())
        );
        assert_eq!(
            check_cors_origin(Some("http://localhost:3000"), &allowed),
            Some("http://localhost:3000".to_string())
        );
        assert_eq!(
            check_cors_origin(Some("http://localhost:8080"), &allowed),
            Some("http://localhost:8080".to_string())
        );
        // But not a different host
        assert_eq!(check_cors_origin(Some("http://localhostevil.com"), &allowed), None);
    }

    #[test]
    fn test_cors_127_allows_any_port() {
        let allowed = vec!["http://127.0.0.1".to_string()];
        assert_eq!(
            check_cors_origin(Some("http://127.0.0.1:7101"), &allowed),
            Some("http://127.0.0.1:7101".to_string())
        );
    }

    #[test]
    fn test_cors_denies_unlisted_origins() {
        let allowed = vec!["http://localhost".to_string(), "http://127.0.0.1".to_string()];
        assert_eq!(check_cors_origin(Some("http://evil.com"), &allowed), None);
        assert_eq!(check_cors_origin(Some("https://example.com"), &allowed), None);
    }

    #[test]
    fn test_cors_no_origin_header() {
        let allowed = vec!["http://localhost".to_string()];
        // No Origin header means no CORS needed (same-origin request)
        assert_eq!(check_cors_origin(None, &allowed), None);
    }

    #[test]
    fn test_cors_empty_allowed_list() {
        // Empty list should deny all origins
        let allowed: Vec<String> = vec![];
        assert_eq!(check_cors_origin(Some("http://localhost:3000"), &allowed), None);
    }
}
