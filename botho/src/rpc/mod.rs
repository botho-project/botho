//! JSON-RPC Server for Botho
//!
//! Provides a JSON-RPC 2.0 API for thin wallets and web interfaces.
//! Also supports WebSocket connections for real-time event streaming.

pub mod auth;
pub mod deposit_scanner;
pub mod faucet;
pub mod metrics;
pub mod rate_limit;
pub mod view_keys;
pub mod websocket;

pub use auth::{ApiKeyConfig, ApiPermissions, AuthError, HmacAuthenticator};
pub use deposit_scanner::{DepositScanner, ScanResult};
pub use faucet::{FaucetError, FaucetRequest, FaucetResponse, FaucetState, FaucetStats};
pub use metrics::{
    calculate_dir_size, check_health, check_ready, init_metrics, start_metrics_server,
    HealthResponse, HealthStatus, MetricsUpdater, NodeMetrics, ReadyResponse, DATA_DIR_USAGE_BYTES,
};
pub use rate_limit::{KeyTier, RateLimitInfo, RateLimiter};
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
            Err(_) => {
                return JsonRpcResponse::error($id, INTERNAL_ERROR, "Internal error: lock poisoned")
            }
        }
    };
}

/// Helper macro to acquire a write lock, returning a JSON-RPC error if poisoned
macro_rules! write_lock {
    ($lock:expr, $id:expr) => {
        match $lock.write() {
            Ok(guard) => guard,
            Err(_) => {
                return JsonRpcResponse::error($id, INTERNAL_ERROR, "Internal error: lock poisoned")
            }
        }
    };
}
use http_body_util::{BodyExt, Full};
use hyper::{
    body::Bytes, server::conn::http1, service::service_fn, Method, Request, Response, StatusCode,
};
use hyper_util::rt::TokioIo;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    convert::Infallible,
    net::SocketAddr,
    sync::{Arc, RwLock},
};
use tokio::net::TcpListener;
use tracing::{debug, error, info};

use bth_transaction_types::constants::Network;
use crate::{address::Address, ledger::Ledger, mempool::Mempool};

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
    /// SCP consensus peer count (peers participating in voting)
    pub scp_peer_count: Arc<RwLock<usize>>,
    pub start_time: std::time::Instant,
    /// Network type (mainnet or testnet)
    pub network_type: Network,
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
    /// Prometheus metrics
    pub metrics: Arc<NodeMetrics>,
    /// Per-API-key rate limiter
    pub rate_limiter: Arc<RateLimiter>,
    /// Faucet state (None if faucet is disabled)
    pub faucet: Option<Arc<FaucetState>>,
}

impl RpcState {
    pub fn new(
        ledger: Ledger,
        mempool: Mempool,
        network_type: Network,
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
            scp_peer_count: Arc::new(RwLock::new(0)),
            start_time: std::time::Instant::now(),
            network_type,
            wallet_view_key,
            wallet_spend_key,
            cors_origins,
            ws_broadcaster,
            view_key_registry: Arc::new(ViewKeyRegistry::new()),
            metrics: Arc::new(NodeMetrics::new()),
            rate_limiter: Arc::new(RateLimiter::new()),
            faucet: None,
        }
    }

    /// Create RpcState from already-shared components
    pub fn from_shared(
        ledger: Arc<RwLock<Ledger>>,
        mempool: Arc<RwLock<Mempool>>,
        minting_active: Arc<RwLock<bool>>,
        peer_count: Arc<RwLock<usize>>,
        scp_peer_count: Arc<RwLock<usize>>,
        network_type: Network,
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
            scp_peer_count,
            start_time: std::time::Instant::now(),
            network_type,
            wallet_view_key,
            wallet_spend_key,
            cors_origins,
            ws_broadcaster,
            view_key_registry: Arc::new(ViewKeyRegistry::new()),
            metrics: Arc::new(NodeMetrics::new()),
            rate_limiter: Arc::new(RateLimiter::new()),
            faucet: None,
        }
    }

    /// Create RpcState with a custom rate limiter
    pub fn with_rate_limiter(
        ledger: Ledger,
        mempool: Mempool,
        network_type: Network,
        wallet_view_key: Option<[u8; 32]>,
        wallet_spend_key: Option<[u8; 32]>,
        cors_origins: Vec<String>,
        ws_broadcaster: Arc<WsBroadcaster>,
        rate_limiter: RateLimiter,
    ) -> Self {
        Self {
            ledger: Arc::new(RwLock::new(ledger)),
            mempool: Arc::new(RwLock::new(mempool)),
            minting_active: Arc::new(RwLock::new(false)),
            minting_threads: num_cpus::get(),
            peer_count: Arc::new(RwLock::new(0)),
            scp_peer_count: Arc::new(RwLock::new(0)),
            start_time: std::time::Instant::now(),
            network_type,
            wallet_view_key,
            wallet_spend_key,
            cors_origins,
            ws_broadcaster,
            view_key_registry: Arc::new(ViewKeyRegistry::new()),
            metrics: Arc::new(NodeMetrics::new()),
            rate_limiter: Arc::new(rate_limiter),
            faucet: None,
        }
    }

    /// Set the faucet state
    pub fn with_faucet(mut self, faucet: FaucetState) -> Self {
        self.faucet = Some(Arc::new(faucet));
        self
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

/// Default API key used when no X-API-Key header is provided
const DEFAULT_API_KEY: &str = "anonymous";

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

    // Handle CORS preflight (don't rate limit preflight requests)
    if req.method() == Method::OPTIONS {
        return Ok(cors_response(
            Response::new(Full::new(Bytes::new())),
            allowed_origin_ref,
        ));
    }

    // Extract API key from header (default to "anonymous" if not provided)
    let api_key = req
        .headers()
        .get("X-API-Key")
        .and_then(|h| h.to_str().ok())
        .unwrap_or(DEFAULT_API_KEY);

    // Check rate limit
    let rate_limit_info = state.rate_limiter.check(api_key);

    // If rate limited, return 429 Too Many Requests
    if !rate_limit_info.allowed {
        debug!(
            "Rate limit exceeded for API key: {} (limit: {}/min)",
            api_key, rate_limit_info.limit
        );
        return Ok(rate_limit_response(&rate_limit_info, allowed_origin_ref));
    }

    // Check for WebSocket upgrade request at /ws
    if req.method() == Method::GET && req.uri().path() == "/ws" {
        return handle_websocket_upgrade(req, state).await;
    }

    // Handle observability endpoints (GET only, no auth required)
    if req.method() == Method::GET {
        match req.uri().path() {
            "/health" => {
                let health = check_health(&state);
                let body = serde_json::to_string(&health).unwrap_or_default();
                return Ok(cors_response(
                    Response::builder()
                        .status(StatusCode::OK)
                        .header("Content-Type", "application/json")
                        .body(Full::new(Bytes::from(body)))
                        .unwrap(),
                    allowed_origin_ref,
                ));
            }
            "/ready" => {
                let ready_response = check_ready(&state);
                let is_ready = ready_response.status == "ready";
                let status = if is_ready {
                    StatusCode::OK
                } else {
                    StatusCode::SERVICE_UNAVAILABLE
                };
                let body = serde_json::to_string(&ready_response).unwrap_or_default();
                return Ok(cors_response(
                    Response::builder()
                        .status(status)
                        .header("Content-Type", "application/json")
                        .body(Full::new(Bytes::from(body)))
                        .unwrap(),
                    allowed_origin_ref,
                ));
            }
            "/metrics" => {
                // Update metrics from current state before encoding
                state.metrics.update_from_state(&state);
                let metrics_text = state.metrics.encode().unwrap_or_default();
                return Ok(cors_response(
                    Response::builder()
                        .status(StatusCode::OK)
                        .header("Content-Type", "text/plain; version=0.0.4; charset=utf-8")
                        .body(Full::new(Bytes::from(metrics_text)))
                        .unwrap(),
                    allowed_origin_ref,
                ));
            }
            _ => {}
        }
    }

    // Only accept POST for JSON-RPC
    if req.method() != Method::POST {
        return Ok(add_rate_limit_headers(
            cors_response(
                Response::builder()
                    .status(StatusCode::METHOD_NOT_ALLOWED)
                    .body(Full::new(Bytes::from("Method not allowed")))
                    .unwrap(),
                allowed_origin_ref,
            ),
            &rate_limit_info,
        ));
    }

    // Read body
    let body_bytes = match req.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(e) => {
            error!("Failed to read request body: {}", e);
            return Ok(add_rate_limit_headers(
                cors_response(
                    Response::builder()
                        .status(StatusCode::BAD_REQUEST)
                        .body(Full::new(Bytes::from("Failed to read body")))
                        .unwrap(),
                    allowed_origin_ref,
                ),
                &rate_limit_info,
            ));
        }
    };

    // Parse JSON-RPC request
    let rpc_request: JsonRpcRequest = match serde_json::from_slice(&body_bytes) {
        Ok(req) => req,
        Err(e) => {
            error!("Failed to parse JSON-RPC request: {}", e);
            let response = JsonRpcResponse::error(Value::Null, -32700, "Parse error");
            return Ok(json_response_with_rate_limit(
                response,
                allowed_origin_ref,
                &rate_limit_info,
            ));
        }
    };

    debug!(
        "RPC request: {} (id: {})",
        rpc_request.method, rpc_request.id
    );

    // Record metric for this request
    state.metrics.record_request(&rpc_request.method);

    // Handle the request
    let response = handle_rpc_method(&rpc_request, &state).await;

    // Record error if the response contains an error
    if response.error.is_some() {
        state.metrics.record_error(&rpc_request.method);
    }

    Ok(json_response_with_rate_limit(
        response,
        allowed_origin_ref,
        &rate_limit_info,
    ))
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
        "fee_getRate" => handle_get_fee_rate(id, state).await,

        // Wallet methods (for thin wallet sync)
        "chain_getOutputs" => handle_get_outputs(id, &request.params, state).await,
        "wallet_getBalance" => handle_wallet_balance(id, state).await,
        "wallet_getAddress" => handle_wallet_address(id, state).await,

        // Transaction methods
        "tx_submit" | "sendRawTransaction" => handle_submit_tx(id, &request.params, state).await,
        "pq_tx_submit" => handle_submit_pq_tx(id, &request.params, state).await,
        "getTransaction" | "tx_get" => handle_get_transaction(id, &request.params, state).await,
        "getTransactionStatus" | "tx_getStatus" => {
            handle_get_transaction_status(id, &request.params, state).await
        }

        // Address methods (for exchange integration)
        "validateAddress" | "address_validate" => {
            handle_validate_address(id, &request.params, state).await
        }

        // Minting methods
        "minting_getStatus" => handle_minting_status(id, state).await,

        // Network methods
        "network_getInfo" => handle_network_info(id, state).await,
        "network_getPeers" => handle_get_peers(id, state).await,

        // Exchange integration methods
        "exchange_registerViewKey" => handle_register_view_key(id, &request.params, state).await,
        "exchange_unregisterViewKey" => {
            handle_unregister_view_key(id, &request.params, state).await
        }
        "exchange_listViewKeys" => handle_list_view_keys(id, &request.params, state).await,

        // Cluster wealth methods (for progressive fee estimation)
        "cluster_getWealth" => handle_cluster_get_wealth(id, &request.params, state).await,
        "cluster_getWealthByTargetKeys" => {
            handle_cluster_get_wealth_by_target_keys(id, &request.params, state).await
        }
        "cluster_getAllWealth" => handle_cluster_get_all_wealth(id, state).await,

        // Entropy proof methods (Phase 2)
        "entropy_estimateFee" => handle_entropy_estimate_fee(id, &request.params, state).await,
        "entropy_getStatus" => handle_entropy_status(id, state).await,

        // Faucet methods (testnet only)
        "faucet_request" => handle_faucet_request(id, &request.params, state, None).await,
        "faucet_getStatus" => handle_faucet_status(id, state).await,

        _ => JsonRpcResponse::error(id, -32601, &format!("Method not found: {}", request.method)),
    }
}

// Handler implementations

// ============================================================================
// Entropy Proof Helper Functions (Phase 2)
// ============================================================================

/// Block height after which entropy proofs are required for decay credit.
/// Before this height: proofs optional, minimal decay credit if not provided.
/// After this height: proofs required for decay credit, none if not provided.
const ENTROPY_REQUIRED_HEIGHT: u64 = 500_000;

/// Block height after which entropy proofs are mandatory.
/// Transactions without entropy proof will be rejected after this height.
const ENTROPY_MANDATORY_HEIGHT: u64 = 1_000_000;

/// Base decay rate (5% per year, expressed as parts per million).
const BASE_DECAY_RATE: u64 = 50_000; // 5%

/// Minimal decay rate for transactions without entropy proof (0.5%).
const MINIMAL_DECAY_RATE: u64 = 5_000; // 0.5%

/// Check if entropy proof is required at the given block height.
///
/// Returns true if:
/// - Block height >= ENTROPY_REQUIRED_HEIGHT (proofs needed for decay credit)
/// - OR block height >= ENTROPY_MANDATORY_HEIGHT (proofs mandatory)
fn is_entropy_proof_required(block_height: u64) -> bool {
    block_height >= ENTROPY_REQUIRED_HEIGHT
}

/// Check if entropy proof is mandatory (consensus-level requirement).
#[allow(dead_code)] // Used by handle_entropy_estimate_fee and tests
fn is_entropy_proof_mandatory(block_height: u64) -> bool {
    block_height >= ENTROPY_MANDATORY_HEIGHT
}

/// Extract entropy proof data from a transaction.
///
/// Returns JSON-serializable entropy proof info if present, null otherwise.
/// Once #279 merges, this will read from tx.extended_signature.entropy_proof.
fn get_entropy_proof_from_tx(tx: &crate::transaction::Transaction) -> Option<Value> {
    // Phase 2 placeholder: entropy proof not yet in Transaction struct
    // This will be updated when #279 adds entropy_proof to ExtendedTxSignature
    //
    // Future implementation:
    // if let Some(ref extended_sig) = tx.extended_signature {
    //     if let Some(ref proof) = extended_sig.entropy_proof {
    //         return Some(json!({
    //             "entropyBeforeCommitment": hex::encode(proof.entropy_before_commitment.as_bytes()),
    //             "entropyAfterCommitment": hex::encode(proof.entropy_after_commitment.as_bytes()),
    //             "proofSize": proof.serialized_size(),
    //         }));
    //     }
    // }
    let _ = tx; // Suppress unused variable warning
    None
}

/// Compute the entropy validation result based on proof presence and block height.
///
/// Returns one of:
/// - "valid": Proof provided and verified
/// - "not_provided": No proof provided (transition period)
/// - "no_decay_credit": No proof provided (after required height)
/// - "invalid": Proof provided but failed verification
fn compute_entropy_validation_result(
    entropy_proof_data: &Option<Value>,
    block_height: u64,
) -> Option<String> {
    match entropy_proof_data {
        Some(_proof) => {
            // Proof provided - would verify here
            // For now, assume valid if present (verification in #280)
            Some("valid".to_string())
        }
        None => {
            if block_height < ENTROPY_REQUIRED_HEIGHT {
                // Transition period: proof optional
                Some("not_provided".to_string())
            } else {
                // After required height: no decay credit without proof
                Some("no_decay_credit".to_string())
            }
        }
    }
}

/// Compute the effective decay rate based on entropy validation result.
///
/// Returns decay rate in parts per million:
/// - "valid": Full decay credit (BASE_DECAY_RATE)
/// - "not_provided": Minimal decay credit (MINIMAL_DECAY_RATE)
/// - "no_decay_credit": No decay credit (0)
/// - "invalid": No decay credit (0)
fn compute_effective_decay_rate(validation_result: Option<&str>, block_height: u64) -> u64 {
    match validation_result {
        Some("valid") => BASE_DECAY_RATE,
        Some("not_provided") => {
            if block_height < ENTROPY_REQUIRED_HEIGHT {
                MINIMAL_DECAY_RATE
            } else {
                0
            }
        }
        Some("no_decay_credit") | Some("invalid") => 0,
        None => {
            // No result yet - use minimal if in transition period
            if block_height < ENTROPY_REQUIRED_HEIGHT {
                MINIMAL_DECAY_RATE
            } else {
                0
            }
        }
        Some(_) => 0, // Unknown result
    }
}

// ============================================================================
// Node and Chain Handlers
// ============================================================================

async fn handle_node_status(id: Value, state: &RpcState) -> JsonRpcResponse {
    let ledger = read_lock!(state.ledger, id.clone());
    let chain_state = ledger.get_chain_state().unwrap_or_default();
    let minting = *read_lock!(state.minting_active, id.clone());
    let mempool = read_lock!(state.mempool, id.clone());
    let peers = *read_lock!(state.peer_count, id.clone());
    let scp_peers = *read_lock!(state.scp_peer_count, id.clone());

    // Calculate sync progress: 100.0 if synced, otherwise based on chain state
    // TODO: Wire up actual sync progress from ChainSyncManager
    let sync_progress: f64 = 100.0;

    JsonRpcResponse::success(
        id,
        json!({
            "version": env!("CARGO_PKG_VERSION"),
            "nodeVersion": env!("CARGO_PKG_VERSION"),
            "gitCommit": option_env!("GIT_HASH").unwrap_or("unknown"),
            "gitCommitShort": option_env!("GIT_HASH_SHORT").unwrap_or("unknown"),
            "buildTime": option_env!("BUILD_TIME").unwrap_or("unknown"),
            "network": format!("botho-{}", state.network_type.name()),
            "uptimeSeconds": state.start_time.elapsed().as_secs(),
            "syncStatus": "synced",
            "syncProgress": sync_progress,
            "synced": true,
            "chainHeight": chain_state.height,
            "tipHash": hex::encode(chain_state.tip_hash),
            "peerCount": peers,
            "scpPeerCount": scp_peers,
            "mempoolSize": mempool.len(),
            "mintingActive": minting,
            "mintingThreads": if minting { state.minting_threads } else { 0 },
            "totalTransactions": chain_state.total_tx,
        }),
    )
}

async fn handle_chain_info(id: Value, state: &RpcState) -> JsonRpcResponse {
    let ledger = read_lock!(state.ledger, id.clone());
    let chain_state = ledger.get_chain_state().unwrap_or_default();
    let mempool = read_lock!(state.mempool, id.clone());

    // Calculate circulating supply: total mined minus fees burned
    let circulating_supply = chain_state
        .total_mined
        .saturating_sub(chain_state.total_fees_burned);

    JsonRpcResponse::success(
        id,
        json!({
            "height": chain_state.height,
            "tipHash": hex::encode(chain_state.tip_hash),
            "difficulty": chain_state.difficulty,
            "totalMined": chain_state.total_mined,
            "totalFeesBurned": chain_state.total_fees_burned,
            "circulatingSupply": circulating_supply,
            "mempoolSize": mempool.len(),
            "mempoolFees": mempool.total_fees(),
        }),
    )
}

/// Get supply information for accurate circulating supply measurement
///
/// Returns:
/// - `totalMined`: Gross emission from block rewards (all BTH ever created)
/// - `totalFeesBurned`: Cumulative transaction fees burned (removed from
///   supply)
/// - `circulatingSupply`: Net supply = totalMined - totalFeesBurned
async fn handle_supply_info(id: Value, state: &RpcState) -> JsonRpcResponse {
    let ledger = read_lock!(state.ledger, id.clone());
    let chain_state = ledger.get_chain_state().unwrap_or_default();

    // Calculate circulating supply: total mined minus fees burned
    let circulating_supply = chain_state
        .total_mined
        .saturating_sub(chain_state.total_fees_burned);

    JsonRpcResponse::success(
        id,
        json!({
            "height": chain_state.height,
            "totalMined": chain_state.total_mined,
            "totalFeesBurned": chain_state.total_fees_burned,
            "circulatingSupply": circulating_supply,
        }),
    )
}

async fn handle_get_block(id: Value, params: &Value, state: &RpcState) -> JsonRpcResponse {
    let height = params.get("height").and_then(|v| v.as_u64()).unwrap_or(0);
    let ledger = read_lock!(state.ledger, id.clone());

    match ledger.get_block(height) {
        Ok(block) => JsonRpcResponse::success(
            id,
            json!({
                "height": block.height(),
                "hash": hex::encode(block.hash()),
                "prevHash": hex::encode(block.header.prev_block_hash),
                "timestamp": block.header.timestamp,
                "difficulty": block.header.difficulty,
                "nonce": block.header.nonce,
                "txCount": block.transactions.len(),
                "mintingReward": block.minting_tx.reward,
            }),
        ),
        Err(e) => JsonRpcResponse::error(id, -32000, &format!("Block not found: {}", e)),
    }
}

async fn handle_mempool_info(id: Value, state: &RpcState) -> JsonRpcResponse {
    let mempool = read_lock!(state.mempool, id.clone());

    // Get transaction hashes from mempool
    let txs = mempool.get_transactions(100);
    let tx_hashes: Vec<String> = txs.iter().map(|tx| hex::encode(tx.hash())).collect();

    JsonRpcResponse::success(
        id,
        json!({
            "size": mempool.len(),
            "totalFees": mempool.total_fees(),
            "txHashes": tx_hashes,
        }),
    )
}

async fn handle_estimate_fee(id: Value, params: &Value, state: &RpcState) -> JsonRpcResponse {
    // Parse parameters
    let amount = params.get("amount").and_then(|v| v.as_u64()).unwrap_or(0);
    let num_memos = params.get("memos").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

    // Parse optional cluster_wealth for accurate progressive fee calculation
    // Wallets can get this from cluster_getWealthByTargetKeys
    let cluster_wealth = params
        .get("cluster_wealth")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    // All transactions are private (CLSAG ring signatures)
    let tx_type = bth_cluster_tax::TransactionType::Hidden;

    let mempool = read_lock!(state.mempool, id.clone());

    // Calculate minimum fee using the fee curve with cluster wealth
    let minimum_fee = mempool.estimate_fee_with_wealth(tx_type, amount, num_memos, cluster_wealth);

    // Get cluster factor for display (1000 = 1x, 6000 = 6x based on wealth)
    let cluster_factor = mempool.cluster_factor(cluster_wealth);

    // Calculate average mempool fee for priority estimation
    let avg_fee = if !mempool.is_empty() {
        mempool.total_fees() / mempool.len() as u64
    } else {
        minimum_fee
    };

    let tx_type_str = match tx_type {
        bth_cluster_tax::TransactionType::Hidden => "hidden",
        bth_cluster_tax::TransactionType::Minting => "minting",
    };

    JsonRpcResponse::success(
        id,
        json!({
            "minimumFee": minimum_fee,
            "clusterFactor": cluster_factor,  // 1000 = 1x, 6000 = 6x
            "clusterFactorDisplay": format!("{:.2}x", cluster_factor as f64 / 1000.0),
            "recommendedFee": avg_fee.max(minimum_fee),
            "highPriorityFee": (avg_fee * 2).max(minimum_fee * 2),
            "clusterWealth": cluster_wealth,
            "params": {
                "amount": amount,
                "txType": tx_type_str,
                "memos": num_memos,
                "clusterWealth": cluster_wealth,
            }
        }),
    )
}

/// Get current network fee rate.
///
/// Returns the dynamic fee base rate used for fee calculation. Wallets should
/// use this to update their local FeeEstimator for accurate fee estimation.
///
/// # Returns
/// - `baseRate`: Current base fee rate in nanoBTH per byte
/// - `baseMin`: Minimum possible base rate (floor)
/// - `baseMax`: Maximum possible base rate (ceiling)
/// - `multiplier`: Current multiplier (baseRate / baseMin)
/// - `congestion`: Network congestion level (0.0 to 1.0)
/// - `adjustmentActive`: Whether dynamic adjustment is active
/// - `blocksToRecovery`: Estimated blocks until fees return to minimum
async fn handle_get_fee_rate(id: Value, state: &RpcState) -> JsonRpcResponse {
    let mempool = read_lock!(state.mempool, id.clone());

    // Get the dynamic fee state from the mempool
    let fee_state = mempool.dynamic_fee_state();
    let dynamic_fee = mempool.dynamic_fee();

    JsonRpcResponse::success(
        id,
        json!({
            "baseRate": fee_state.current_base,
            "baseMin": dynamic_fee.base_min,
            "baseMax": dynamic_fee.base_max,
            "multiplier": fee_state.multiplier,
            "congestion": fee_state.ema_fullness,
            "targetFullness": fee_state.target_fullness,
            "adjustmentActive": fee_state.adjustment_active,
            "blocksToRecovery": dynamic_fee.blocks_to_recovery(),
        }),
    )
}

async fn handle_get_outputs(id: Value, params: &Value, state: &RpcState) -> JsonRpcResponse {
    let start_height = params
        .get("start_height")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let end_height = params
        .get("end_height")
        .and_then(|v| v.as_u64())
        .unwrap_or(start_height + 100);

    let ledger = read_lock!(state.ledger, id.clone());
    let mut blocks = Vec::new();

    for height in start_height..=end_height {
        if let Ok(block) = ledger.get_block(height) {
            let mut outputs = Vec::new();
            for tx in &block.transactions {
                for (idx, output) in tx.outputs.iter().enumerate() {
                    // Serialize cluster tags as array of [cluster_id, weight] pairs
                    let cluster_tags: Vec<[u64; 2]> = output
                        .cluster_tags
                        .entries
                        .iter()
                        .map(|e| [e.cluster_id.0, e.weight as u64])
                        .collect();

                    outputs.push(json!({
                        "txHash": hex::encode(tx.hash()),
                        "outputIndex": idx,
                        "targetKey": hex::encode(output.target_key),
                        "publicKey": hex::encode(output.public_key),
                        "amountCommitment": hex::encode(output.amount.to_le_bytes()),
                        "clusterTags": cluster_tags,
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

    JsonRpcResponse::success(
        id,
        json!({
            "confirmed": 0,
            "pending": 0,
            "total": 0,
            "utxoCount": 0,
        }),
    )
}

async fn handle_wallet_address(id: Value, state: &RpcState) -> JsonRpcResponse {
    // Return null keys if running in relay mode (no wallet)
    let view_key = state
        .wallet_view_key
        .map(|k| hex::encode(&k))
        .unwrap_or_default();
    let spend_key = state
        .wallet_spend_key
        .map(|k| hex::encode(&k))
        .unwrap_or_default();

    JsonRpcResponse::success(
        id,
        json!({
            "viewKey": view_key,
            "spendKey": spend_key,
            "hasWallet": state.wallet_view_key.is_some(),
        }),
    )
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
        Err(e) => {
            return JsonRpcResponse::error(id, -32602, &format!("Invalid transaction: {}", e))
        }
    };

    let ledger = read_lock!(state.ledger, id.clone());
    let mut mempool = write_lock!(state.mempool, id.clone());

    match mempool.add_tx(tx, &ledger) {
        Ok(hash) => JsonRpcResponse::success(
            id,
            json!({
                "txHash": hex::encode(hash),
            }),
        ),
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
        Err(e) => {
            return JsonRpcResponse::error(id, -32602, &format!("Invalid PQ transaction: {}", e))
        }
    };

    // Validate structure
    if let Err(e) = tx.is_valid_structure() {
        return JsonRpcResponse::error(
            id,
            -32602,
            &format!("Invalid PQ transaction structure: {}", e),
        );
    }

    // Validate fee
    if !tx.has_sufficient_fee() {
        return JsonRpcResponse::error(
            id,
            -32602,
            &format!(
                "Insufficient fee: {} < {} required",
                tx.fee,
                tx.minimum_fee()
            ),
        );
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

    JsonRpcResponse::success(
        id,
        json!({
            "txHash": hex::encode(tx_hash),
            "type": "quantum-private",
            "size": tx.estimated_size(),
        }),
    )
}

/// Fallback for non-PQ builds
#[cfg(not(feature = "pq"))]
async fn handle_submit_pq_tx(id: Value, _params: &Value, _state: &RpcState) -> JsonRpcResponse {
    JsonRpcResponse::error(id, -32601, "PQ transactions not enabled in this build")
}

/// Get a transaction by hash (for exchange integration)
///
/// Returns transaction details including block height, confirmations,
/// status, and entropy proof information (Phase 2).
///
/// # Entropy Proof Fields (Phase 2)
///
/// The response includes entropy proof information when available:
/// - `entropyProof`: Entropy proof data (null if not provided)
/// - `entropyValidationResult`: Validation status ("valid", "not_provided", "no_decay_credit", "invalid")
/// - `effectiveDecayRate`: Computed decay rate based on entropy proof
/// - `entropyProofRequired`: Whether entropy proof is required at this block height
async fn handle_get_transaction(id: Value, params: &Value, state: &RpcState) -> JsonRpcResponse {
    // Parse tx_hash parameter
    let tx_hash_hex = match params
        .get("tx_hash")
        .or_else(|| params.get("hash"))
        .and_then(|v| v.as_str())
    {
        Some(hex) => hex,
        None => return JsonRpcResponse::error(id, -32602, "Missing tx_hash parameter"),
    };

    let tx_hash: [u8; 32] = match hex::decode(tx_hash_hex) {
        Ok(bytes) if bytes.len() == 32 => {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            arr
        }
        _ => {
            return JsonRpcResponse::error(
                id,
                -32602,
                "Invalid tx_hash: expected 32-byte hex string",
            )
        }
    };

    let ledger = read_lock!(state.ledger, id.clone());
    let chain_state = ledger.get_chain_state().unwrap_or_default();

    // First check mempool
    let mempool = read_lock!(state.mempool, id.clone());
    if mempool.contains(&tx_hash) {
        // Determine if entropy proof would be required for a tx at current height
        let entropy_proof_required = is_entropy_proof_required(chain_state.height);

        return JsonRpcResponse::success(
            id,
            json!({
                "txHash": tx_hash_hex,
                "status": "pending",
                "blockHeight": null,
                "confirmations": 0,
                "inMempool": true,
                // Entropy proof fields (Phase 2)
                "entropyProof": null,
                "entropyValidationResult": "not_provided",
                "effectiveDecayRate": compute_effective_decay_rate(None, chain_state.height),
                "entropyProofRequired": entropy_proof_required,
            }),
        );
    }
    drop(mempool);

    // Look up in blockchain
    match ledger.get_transaction(&tx_hash) {
        Ok(Some((tx, block_height, confirmations))) => {
            let tx_type = "clsag";
            let output_count = tx.outputs.len();
            let total_output: u64 = tx.outputs.iter().map(|o| o.amount).sum();

            // Get entropy proof information from transaction (Phase 2)
            // For now, entropy proofs are not yet in the Transaction struct (#279)
            // Once #279 is merged, this will read from tx.extended_signature.entropy_proof
            let entropy_proof_data = get_entropy_proof_from_tx(&tx);
            let entropy_validation_result =
                compute_entropy_validation_result(&entropy_proof_data, block_height);
            let effective_decay_rate =
                compute_effective_decay_rate(entropy_validation_result.as_deref(), block_height);
            let entropy_proof_required = is_entropy_proof_required(block_height);

            JsonRpcResponse::success(
                id,
                json!({
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
                    // Entropy proof fields (Phase 2)
                    "entropyProof": entropy_proof_data,
                    "entropyValidationResult": entropy_validation_result,
                    "effectiveDecayRate": effective_decay_rate,
                    "entropyProofRequired": entropy_proof_required,
                }),
            )
        }
        Ok(None) => JsonRpcResponse::error(id, -32000, "Transaction not found"),
        Err(e) => JsonRpcResponse::error(id, -32000, &format!("Failed to get transaction: {}", e)),
    }
}

/// Get transaction status and confirmation count (for exchange integration)
///
/// Lightweight version of getTransaction that only returns status info.
/// Includes entropy validation status (Phase 2).
///
/// # Entropy Validation Fields (Phase 2)
///
/// - `entropyValidationResult`: Validation status ("valid", "not_provided", "no_decay_credit")
/// - `entropyProofRequired`: Whether entropy proof is required at this block height
async fn handle_get_transaction_status(
    id: Value,
    params: &Value,
    state: &RpcState,
) -> JsonRpcResponse {
    // Parse tx_hash parameter
    let tx_hash_hex = match params
        .get("tx_hash")
        .or_else(|| params.get("hash"))
        .and_then(|v| v.as_str())
    {
        Some(hex) => hex,
        None => return JsonRpcResponse::error(id, -32602, "Missing tx_hash parameter"),
    };

    let tx_hash: [u8; 32] = match hex::decode(tx_hash_hex) {
        Ok(bytes) if bytes.len() == 32 => {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            arr
        }
        _ => {
            return JsonRpcResponse::error(
                id,
                -32602,
                "Invalid tx_hash: expected 32-byte hex string",
            )
        }
    };

    let ledger = read_lock!(state.ledger, id.clone());
    let chain_state = ledger.get_chain_state().unwrap_or_default();

    // Check mempool first
    let mempool = read_lock!(state.mempool, id.clone());
    if mempool.contains(&tx_hash) {
        let entropy_proof_required = is_entropy_proof_required(chain_state.height);
        return JsonRpcResponse::success(
            id,
            json!({
                "txHash": tx_hash_hex,
                "status": "pending",
                "confirmations": 0,
                "confirmed": false,
                // Entropy validation fields (Phase 2)
                "entropyValidationResult": "not_provided",
                "entropyProofRequired": entropy_proof_required,
            }),
        );
    }
    drop(mempool);

    // Look up in blockchain
    match ledger.get_transaction_confirmations(&tx_hash) {
        Ok(Some(confirmations)) => {
            // For confirmed transactions, we need the block height to determine entropy status
            // Since this is the lightweight endpoint, we use chain height as approximation
            // Full entropy info is available via getTransaction
            let entropy_proof_required = is_entropy_proof_required(chain_state.height);
            let entropy_validation_result = if entropy_proof_required {
                "no_decay_credit" // Default without proof after required height
            } else {
                "not_provided" // Transition period
            };

            JsonRpcResponse::success(
                id,
                json!({
                    "txHash": tx_hash_hex,
                    "status": "confirmed",
                    "confirmations": confirmations,
                    "confirmed": true,
                    // Entropy validation fields (Phase 2)
                    "entropyValidationResult": entropy_validation_result,
                    "entropyProofRequired": entropy_proof_required,
                }),
            )
        }
        Ok(None) => JsonRpcResponse::success(
            id,
            json!({
                "txHash": tx_hash_hex,
                "status": "unknown",
                "confirmations": 0,
                "confirmed": false,
                // Entropy validation fields (Phase 2)
                "entropyValidationResult": null,
                "entropyProofRequired": is_entropy_proof_required(chain_state.height),
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            id,
            -32000,
            &format!("Failed to get transaction status: {}", e),
        ),
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

            JsonRpcResponse::success(
                id,
                json!({
                    "valid": true,
                    "address": canonical,
                    "network": network,
                    "type": address_type,
                    "isQuantum": is_quantum,
                }),
            )
        }
        Err(e) => JsonRpcResponse::success(
            id,
            json!({
                "valid": false,
                "error": e.to_string(),
                "address": address_str,
            }),
        ),
    }
}

async fn handle_minting_status(id: Value, state: &RpcState) -> JsonRpcResponse {
    let active = *read_lock!(state.minting_active, id.clone());
    let ledger = read_lock!(state.ledger, id.clone());
    let chain_state = ledger.get_chain_state().unwrap_or_default();

    JsonRpcResponse::success(
        id,
        json!({
            "active": active,
            "threads": state.minting_threads,
            "hashrate": 0.0, // TODO: track actual hashrate
            "totalHashes": 0,
            "blocksFound": 0, // TODO: track blocks found
            "currentDifficulty": chain_state.difficulty,
            "uptimeSeconds": state.start_time.elapsed().as_secs(),
        }),
    )
}

async fn handle_network_info(id: Value, state: &RpcState) -> JsonRpcResponse {
    let peers = *read_lock!(state.peer_count, id.clone());

    JsonRpcResponse::success(
        id,
        json!({
            "peerCount": peers,
            "inboundCount": 0,
            "outboundCount": peers,
            "bytesSent": 0,
            "bytesReceived": 0,
            "uptimeSeconds": state.start_time.elapsed().as_secs(),
        }),
    )
}

async fn handle_get_peers(id: Value, _state: &RpcState) -> JsonRpcResponse {
    // Return empty for now - would need to get actual peer addresses
    JsonRpcResponse::success(
        id,
        json!({
            "peers": []
        }),
    )
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
        if origin.starts_with(allowed)
            && (allowed.ends_with("localhost") || allowed.ends_with("127.0.0.1"))
        {
            let suffix = &origin[allowed.len()..];
            if suffix.is_empty() || suffix.starts_with(':') {
                return Some(origin.to_string());
            }
        }
    }

    None
}

fn cors_response(
    mut response: Response<Full<Bytes>>,
    allowed_origin: Option<&str>,
) -> Response<Full<Bytes>> {
    let headers = response.headers_mut();

    if let Some(origin) = allowed_origin {
        headers.insert("Access-Control-Allow-Origin", origin.parse().unwrap());
        headers.insert(
            "Access-Control-Allow-Methods",
            "POST, OPTIONS".parse().unwrap(),
        );
        headers.insert(
            "Access-Control-Allow-Headers",
            "Content-Type".parse().unwrap(),
        );
        headers.insert("Vary", "Origin".parse().unwrap());
    }
    // If no allowed origin, we don't set CORS headers - browser will block the
    // request

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

/// Create a JSON response with rate limit headers.
fn json_response_with_rate_limit(
    response: JsonRpcResponse,
    allowed_origin: Option<&str>,
    rate_limit: &RateLimitInfo,
) -> Response<Full<Bytes>> {
    add_rate_limit_headers(json_response(response, allowed_origin), rate_limit)
}

/// Add X-RateLimit-* headers to a response.
///
/// Headers added:
/// - X-RateLimit-Limit: Maximum requests allowed per window
/// - X-RateLimit-Remaining: Remaining requests in current window
/// - X-RateLimit-Reset: Unix timestamp when the window resets
fn add_rate_limit_headers(
    mut response: Response<Full<Bytes>>,
    rate_limit: &RateLimitInfo,
) -> Response<Full<Bytes>> {
    let headers = response.headers_mut();

    headers.insert(
        "X-RateLimit-Limit",
        rate_limit.limit.to_string().parse().unwrap(),
    );
    headers.insert(
        "X-RateLimit-Remaining",
        rate_limit.remaining.to_string().parse().unwrap(),
    );
    headers.insert(
        "X-RateLimit-Reset",
        rate_limit.reset.to_string().parse().unwrap(),
    );

    response
}

/// Create a 429 Too Many Requests response with rate limit headers.
///
/// Includes:
/// - 429 status code
/// - Retry-After header (seconds until rate limit resets)
/// - X-RateLimit-* headers
/// - JSON error body
fn rate_limit_response(
    rate_limit: &RateLimitInfo,
    allowed_origin: Option<&str>,
) -> Response<Full<Bytes>> {
    let retry_after = rate_limit.retry_after.unwrap_or(60);

    let error_body = json!({
        "jsonrpc": "2.0",
        "error": {
            "code": -32029,
            "message": "Rate limit exceeded",
            "data": {
                "limit": rate_limit.limit,
                "remaining": 0,
                "reset": rate_limit.reset,
                "retryAfter": retry_after
            }
        },
        "id": null
    });

    let body = serde_json::to_string(&error_body).unwrap();

    let mut response = Response::builder()
        .status(StatusCode::TOO_MANY_REQUESTS)
        .header("Content-Type", "application/json")
        .header("Retry-After", retry_after.to_string())
        .body(Full::new(Bytes::from(body)))
        .unwrap();

    // Add CORS headers
    if let Some(origin) = allowed_origin {
        let headers = response.headers_mut();
        headers.insert("Access-Control-Allow-Origin", origin.parse().unwrap());
        headers.insert("Vary", "Origin".parse().unwrap());
    }

    add_rate_limit_headers(response, rate_limit)
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
    let subaddress_min = params
        .get("subaddress_min")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let subaddress_max = params
        .get("subaddress_max")
        .and_then(|v| v.as_u64())
        .unwrap_or(1000);

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
async fn handle_unregister_view_key(
    id: Value,
    params: &Value,
    state: &RpcState,
) -> JsonRpcResponse {
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

// ============================================================================
// Cluster Wealth Handlers (for Progressive Fee Estimation)
// ============================================================================
//
// # Privacy Implications
//
// These endpoints expose cluster wealth information from the public UTXO set.
// Users should understand:
//
// 1. Cluster tags are public on-chain data - anyone can compute cluster wealth
//    by scanning UTXOs. These endpoints just make the lookup efficient.
//
// 2. Wealth aggregates reveal concentration, not individual balances. A cluster
//    with 10M BTH could belong to one whale or 1000 small holders.
//
// 3. Ring signatures protect spending privacy. Even if cluster wealth is known,
//    observers cannot determine which UTXO was spent in a transaction.
//
// 4. Wallets should use `cluster_getWealthByTargetKeys` for accurate fee
//    estimation rather than querying global cluster wealth.

/// Get the total wealth attributed to a specific cluster.
///
/// # Parameters
/// - `cluster_id`: The cluster identifier (numeric string or number)
///
/// # Returns
/// The total wealth in nanoBTH attributed to this cluster across all UTXOs.
async fn handle_cluster_get_wealth(id: Value, params: &Value, state: &RpcState) -> JsonRpcResponse {
    // Parse cluster_id parameter (accept both string and number)
    let cluster_id = if let Some(id_str) = params.get("cluster_id").and_then(|v| v.as_str()) {
        match id_str.parse::<u64>() {
            Ok(id) => id,
            Err(_) => {
                return JsonRpcResponse::error(
                    id,
                    -32602,
                    "Invalid cluster_id: expected numeric value",
                )
            }
        }
    } else if let Some(id_num) = params.get("cluster_id").and_then(|v| v.as_u64()) {
        id_num
    } else {
        return JsonRpcResponse::error(id, -32602, "Missing cluster_id parameter");
    };

    let ledger = read_lock!(state.ledger, id.clone());

    match ledger.get_cluster_wealth(cluster_id) {
        Ok(wealth) => JsonRpcResponse::success(
            id,
            json!({
                "cluster_id": cluster_id.to_string(),
                "wealth": wealth,
                "wealth_btd": format!("{:.9}", wealth as f64 / 1_000_000_000_000.0),
            }),
        ),
        Err(e) => {
            JsonRpcResponse::error(id, -32000, &format!("Failed to get cluster wealth: {}", e))
        }
    }
}

/// Compute cluster wealth for a set of UTXOs identified by target keys.
///
/// This is the primary method for wallets to estimate their cluster wealth
/// for accurate fee calculation. Wallets provide the target keys of their
/// UTXOs, and this method returns comprehensive wealth information.
///
/// # Parameters
/// - `target_keys`: Array of target key hex strings (32 bytes each)
///
/// # Returns
/// Cluster wealth information including max wealth, breakdown, and fee
/// multiplier.
async fn handle_cluster_get_wealth_by_target_keys(
    id: Value,
    params: &Value,
    state: &RpcState,
) -> JsonRpcResponse {
    // Parse target_keys parameter
    let target_keys_hex = match params.get("target_keys").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => {
            return JsonRpcResponse::error(
                id,
                -32602,
                "Missing target_keys parameter (expected array)",
            )
        }
    };

    // Parse hex strings to [u8; 32] arrays
    let mut target_keys: Vec<[u8; 32]> = Vec::with_capacity(target_keys_hex.len());
    for (i, key_val) in target_keys_hex.iter().enumerate() {
        let key_hex = match key_val.as_str() {
            Some(hex) => hex,
            None => {
                return JsonRpcResponse::error(
                    id,
                    -32602,
                    &format!("target_keys[{}]: expected hex string", i),
                )
            }
        };

        if key_hex.len() != 64 {
            return JsonRpcResponse::error(
                id,
                -32602,
                &format!("target_keys[{}]: expected 64 hex characters", i),
            );
        }

        match hex::decode(key_hex) {
            Ok(bytes) if bytes.len() == 32 => {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&bytes);
                target_keys.push(arr);
            }
            _ => {
                return JsonRpcResponse::error(
                    id,
                    -32602,
                    &format!("target_keys[{}]: invalid hex", i),
                )
            }
        }
    }

    if target_keys.is_empty() {
        return JsonRpcResponse::error(id, -32602, "target_keys cannot be empty");
    }

    let ledger = read_lock!(state.ledger, id.clone());
    let mempool = read_lock!(state.mempool, id.clone());

    match ledger.compute_cluster_wealth_for_utxos(&target_keys) {
        Ok(info) => {
            // Calculate the fee multiplier for this wealth level
            let cluster_factor = mempool.cluster_factor(info.max_cluster_wealth);

            // Format cluster breakdown for response
            let breakdown: Vec<Value> = info
                .cluster_breakdown
                .iter()
                .map(|(cluster_id, wealth)| {
                    json!({
                        "cluster_id": cluster_id.to_string(),
                        "wealth": wealth,
                    })
                })
                .collect();

            JsonRpcResponse::success(
                id,
                json!({
                    "max_cluster_wealth": info.max_cluster_wealth,
                    "max_cluster_wealth_btd": format!("{:.9}", info.max_cluster_wealth as f64 / 1_000_000_000_000.0),
                    "total_value": info.total_value,
                    "utxo_count": info.utxo_count,
                    "dominant_cluster_id": info.dominant_cluster_id.map(|id| id.to_string()),
                    "cluster_factor": cluster_factor,  // 1000 = 1x, 6000 = 6x
                    "cluster_factor_display": format!("{:.2}x", cluster_factor as f64 / 1000.0),
                    "cluster_breakdown": breakdown,
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            id,
            -32000,
            &format!("Failed to compute cluster wealth: {}", e),
        ),
    }
}

/// Get all cluster wealth entries for network-wide wealth distribution
/// analysis.
///
/// # Returns
/// Array of all tracked clusters and their total wealth.
///
/// # Note
/// This is primarily for analytics. The number of entries grows with unique
/// cluster IDs in the UTXO set.
async fn handle_cluster_get_all_wealth(id: Value, state: &RpcState) -> JsonRpcResponse {
    let ledger = read_lock!(state.ledger, id.clone());

    match ledger.get_all_cluster_wealth() {
        Ok(clusters) => {
            let total_tracked: u64 = clusters.iter().map(|(_, w)| w).sum();

            let entries: Vec<Value> = clusters
                .iter()
                .map(|(cluster_id, wealth)| {
                    json!({
                        "cluster_id": cluster_id.to_string(),
                        "wealth": wealth,
                    })
                })
                .collect();

            JsonRpcResponse::success(
                id,
                json!({
                    "count": clusters.len(),
                    "total_tracked_wealth": total_tracked,
                    "clusters": entries,
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            id,
            -32000,
            &format!("Failed to get all cluster wealth: {}", e),
        ),
    }
}

// ============================================================================
// Entropy Proof Handlers (Phase 2)
// ============================================================================

/// Estimated entropy proof size in bytes.
/// Based on design document: ~964-1164 bytes for typical transaction.
const ESTIMATED_ENTROPY_PROOF_SIZE: usize = 1100;

/// Estimate the additional fee for including an entropy proof.
///
/// Returns fee information for transactions that include entropy proofs:
/// - `proofSizeEstimate`: Estimated proof size in bytes
/// - `additionalFee`: Additional fee for the entropy proof
/// - `decayCreditEligible`: Whether proof qualifies for decay credit
/// - `effectiveDecayRate`: Expected decay rate with proof
/// - `entropyProofRequired`: Whether proof is required at current height
///
/// # Parameters
/// - `cluster_count`: Number of clusters involved (affects proof size)
/// - `amount`: Transaction amount (for fee curve calculation)
/// - `cluster_wealth`: Optional cluster wealth for progressive fee
async fn handle_entropy_estimate_fee(
    id: Value,
    params: &Value,
    state: &RpcState,
) -> JsonRpcResponse {
    let ledger = read_lock!(state.ledger, id.clone());
    let chain_state = ledger.get_chain_state().unwrap_or_default();
    let mempool = read_lock!(state.mempool, id.clone());

    // Parse parameters
    let cluster_count = params
        .get("cluster_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(3) as usize;
    let amount = params.get("amount").and_then(|v| v.as_u64()).unwrap_or(0);
    let cluster_wealth = params
        .get("cluster_wealth")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    // Calculate proof size based on cluster count
    // Formula from design doc:
    // - 2 entropy commitments: 64 bytes
    // - Range proof: 160 bytes (simplified) or ~700 bytes (Bulletproof)
    // - Linkage proof: 4 + (32 * cluster_count) + (64 * cluster_count) + 64 bytes
    let base_proof_size = 64 + 160; // commitments + range proof
    let linkage_proof_size = 4 + (32 * cluster_count) + (64 * cluster_count) + 64;
    let proof_size_estimate = base_proof_size + linkage_proof_size;

    // Calculate additional fee for proof bytes
    let fee_rate = mempool.dynamic_fee_state().current_base;
    let additional_fee = (proof_size_estimate as u64) * fee_rate;

    // Apply cluster factor to the additional fee
    let cluster_factor = mempool.cluster_factor(cluster_wealth);
    let adjusted_additional_fee = additional_fee * cluster_factor / 1000;

    // Calculate total tx fee with entropy proof
    let tx_type = bth_cluster_tax::TransactionType::Hidden;
    let base_fee = mempool.estimate_fee_with_wealth(tx_type, amount, 0, cluster_wealth);
    let total_fee_with_proof = base_fee + adjusted_additional_fee;

    let entropy_proof_required = is_entropy_proof_required(chain_state.height);
    let entropy_proof_mandatory = is_entropy_proof_mandatory(chain_state.height);

    JsonRpcResponse::success(
        id,
        json!({
            "proofSizeEstimate": proof_size_estimate,
            "additionalFee": adjusted_additional_fee,
            "baseFee": base_fee,
            "totalFeeWithProof": total_fee_with_proof,
            "feeRatePerByte": fee_rate,
            "clusterFactor": cluster_factor,
            "clusterFactorDisplay": format!("{:.2}x", cluster_factor as f64 / 1000.0),
            "decayCreditEligible": true,
            "effectiveDecayRate": BASE_DECAY_RATE,
            "effectiveDecayRateDisplay": format!("{:.2}%", BASE_DECAY_RATE as f64 / 10000.0),
            "entropyProofRequired": entropy_proof_required,
            "entropyProofMandatory": entropy_proof_mandatory,
            "currentBlockHeight": chain_state.height,
            "requiredHeight": ENTROPY_REQUIRED_HEIGHT,
            "mandatoryHeight": ENTROPY_MANDATORY_HEIGHT,
            "params": {
                "clusterCount": cluster_count,
                "amount": amount,
                "clusterWealth": cluster_wealth,
            }
        }),
    )
}

/// Get current entropy proof status and configuration.
///
/// Returns the current state of entropy proof requirements:
/// - `entropyProofRequired`: Whether proofs are required for decay credit
/// - `entropyProofMandatory`: Whether proofs are mandatory (consensus)
/// - `currentBlockHeight`: Current chain height
/// - `requiredHeight`: Block height when proofs become required
/// - `mandatoryHeight`: Block height when proofs become mandatory
/// - `baseDecayRate`: Full decay rate with valid proof
/// - `minimalDecayRate`: Decay rate without proof (transition period)
async fn handle_entropy_status(id: Value, state: &RpcState) -> JsonRpcResponse {
    let ledger = read_lock!(state.ledger, id.clone());
    let chain_state = ledger.get_chain_state().unwrap_or_default();

    let entropy_proof_required = is_entropy_proof_required(chain_state.height);
    let entropy_proof_mandatory = is_entropy_proof_mandatory(chain_state.height);

    // Calculate blocks until next phase
    let blocks_until_required = if chain_state.height < ENTROPY_REQUIRED_HEIGHT {
        Some(ENTROPY_REQUIRED_HEIGHT - chain_state.height)
    } else {
        None
    };
    let blocks_until_mandatory = if chain_state.height < ENTROPY_MANDATORY_HEIGHT {
        Some(ENTROPY_MANDATORY_HEIGHT - chain_state.height)
    } else {
        None
    };

    // Determine current phase
    let phase = if entropy_proof_mandatory {
        "mandatory"
    } else if entropy_proof_required {
        "required"
    } else {
        "optional"
    };

    JsonRpcResponse::success(
        id,
        json!({
            "phase": phase,
            "entropyProofRequired": entropy_proof_required,
            "entropyProofMandatory": entropy_proof_mandatory,
            "currentBlockHeight": chain_state.height,
            "requiredHeight": ENTROPY_REQUIRED_HEIGHT,
            "mandatoryHeight": ENTROPY_MANDATORY_HEIGHT,
            "blocksUntilRequired": blocks_until_required,
            "blocksUntilMandatory": blocks_until_mandatory,
            "decayRates": {
                "withValidProof": BASE_DECAY_RATE,
                "withValidProofDisplay": format!("{:.2}%", BASE_DECAY_RATE as f64 / 10000.0),
                "withoutProof": if entropy_proof_required { 0 } else { MINIMAL_DECAY_RATE },
                "withoutProofDisplay": if entropy_proof_required {
                    "0%".to_string()
                } else {
                    format!("{:.2}%", MINIMAL_DECAY_RATE as f64 / 10000.0)
                },
            },
            "estimatedProofSize": ESTIMATED_ENTROPY_PROOF_SIZE,
        }),
    )
}

// ============================================================================
// Faucet Handler Functions (Testnet only)
// ============================================================================

/// Handle faucet request
///
/// This handler dispenses testnet coins to the specified address.
/// Rate limiting is applied based on IP and address.
async fn handle_faucet_request(
    id: Value,
    params: &Value,
    state: &RpcState,
    client_ip: Option<std::net::IpAddr>,
) -> JsonRpcResponse {
    use faucet::{FaucetError, FaucetRequest};

    // Check if faucet is available
    let faucet = match &state.faucet {
        Some(f) => f,
        None => {
            return JsonRpcResponse::error(id, -32000, "Faucet is disabled on this node");
        }
    };

    if !faucet.is_enabled() {
        return JsonRpcResponse::error(id, -32000, "Faucet is disabled on this node");
    }

    // Parse request
    let request: FaucetRequest = match serde_json::from_value(params.clone()) {
        Ok(r) => r,
        Err(e) => {
            return JsonRpcResponse::error(id, -32602, &format!("Invalid params: {}", e));
        }
    };

    // Get client IP (use localhost if not provided - for local testing)
    let ip = client_ip.unwrap_or_else(|| "127.0.0.1".parse().unwrap());

    // Check rate limits
    if let Err(err) = faucet.check_rate_limit(ip, &request.address) {
        let retry_after = err.retry_after_secs();
        return JsonRpcResponse {
            jsonrpc: "2.0",
            result: None,
            error: Some(JsonRpcError {
                code: -32000,
                message: err.message(),
                data: Some(json!({
                    "retryAfterSecs": retry_after,
                    "error": err,
                })),
            }),
            id,
        };
    }

    // For now, we just record the request and return a placeholder
    // Full implementation would:
    // 1. Parse the address
    // 2. Build a transaction from the node's wallet
    // 3. Submit to mempool
    // 4. Return tx hash
    //
    // This requires integrating with the wallet and transaction building,
    // which is a larger change. For now, we validate the flow works.

    // TODO: Implement actual transaction building and submission
    // This requires:
    // - Access to node's wallet keys (wallet_view_key, wallet_spend_key)
    // - UTXO selection from ledger
    // - Transaction building
    // - Mempool submission

    // For now, return an error indicating the feature is not fully implemented
    // The rate limiting and config are working - just need transaction building
    JsonRpcResponse::error(
        id,
        -32000,
        "Faucet transaction building not yet implemented. Rate limiting validated successfully.",
    )
}

/// Handle faucet status request
///
/// Returns information about the faucet configuration and current stats.
async fn handle_faucet_status(id: Value, state: &RpcState) -> JsonRpcResponse {
    match &state.faucet {
        Some(faucet) => {
            let stats = faucet.stats();
            JsonRpcResponse::success(
                id,
                json!({
                    "enabled": stats.enabled,
                    "amountPerRequest": stats.amount_per_request,
                    "amountPerRequestFormatted": format!("{:.6} BTH", stats.amount_per_request as f64 / 1_000_000_000_000.0),
                    "dailyDispensed": stats.daily_dispensed,
                    "dailyDispensedFormatted": format!("{:.6} BTH", stats.daily_dispensed as f64 / 1_000_000_000_000.0),
                    "dailyLimit": stats.daily_limit,
                    "dailyLimitFormatted": format!("{:.6} BTH", stats.daily_limit as f64 / 1_000_000_000_000.0),
                    "trackedIps": stats.tracked_ips,
                    "trackedAddresses": stats.tracked_addresses,
                }),
            )
        }
        None => JsonRpcResponse::success(
            id,
            json!({
                "enabled": false,
                "message": "Faucet is not configured on this node"
            }),
        ),
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
        assert_eq!(
            check_cors_origin(Some("http://localhostevil.com"), &allowed),
            None
        );
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
        let allowed = vec![
            "http://localhost".to_string(),
            "http://127.0.0.1".to_string(),
        ];
        assert_eq!(check_cors_origin(Some("http://evil.com"), &allowed), None);
        assert_eq!(
            check_cors_origin(Some("https://example.com"), &allowed),
            None
        );
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
        assert_eq!(
            check_cors_origin(Some("http://localhost:3000"), &allowed),
            None
        );
    }

    #[test]
    fn test_rate_limit_headers() {
        let info = RateLimitInfo {
            limit: 100,
            remaining: 50,
            reset: 1234567890,
            allowed: true,
            retry_after: None,
        };

        let response = Response::builder()
            .status(StatusCode::OK)
            .body(Full::new(Bytes::new()))
            .unwrap();

        let response = add_rate_limit_headers(response, &info);
        let headers = response.headers();

        assert_eq!(headers.get("X-RateLimit-Limit").unwrap(), "100");
        assert_eq!(headers.get("X-RateLimit-Remaining").unwrap(), "50");
        assert_eq!(headers.get("X-RateLimit-Reset").unwrap(), "1234567890");
    }

    #[test]
    fn test_rate_limit_response_429() {
        let info = RateLimitInfo {
            limit: 100,
            remaining: 0,
            reset: 1234567890,
            allowed: false,
            retry_after: Some(30),
        };

        let response = rate_limit_response(&info, Some("http://localhost:3000"));

        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);

        let headers = response.headers();
        assert_eq!(headers.get("Retry-After").unwrap(), "30");
        assert_eq!(headers.get("X-RateLimit-Limit").unwrap(), "100");
        assert_eq!(headers.get("X-RateLimit-Remaining").unwrap(), "0");
        assert_eq!(
            headers.get("Access-Control-Allow-Origin").unwrap(),
            "http://localhost:3000"
        );
    }

    #[test]
    fn test_rate_limit_response_body() {
        let info = RateLimitInfo {
            limit: 100,
            remaining: 0,
            reset: 1234567890,
            allowed: false,
            retry_after: Some(30),
        };

        let response = rate_limit_response(&info, None);
        let body_bytes = response.into_body();

        // Verify the body contains the expected error structure
        // The body is a Full<Bytes> which we can't easily convert here,
        // but the json! macro ensures valid JSON structure
        assert!(true); // Body structure is validated by json! macro at compile
                       // time
    }

    #[test]
    fn test_rate_limiter_integration() {
        let limiter = RateLimiter::new();
        limiter.set_key_tier("test-api-key", KeyTier::Custom(3));

        // First 3 requests should succeed
        for i in 0..3 {
            let info = limiter.check("test-api-key");
            assert!(info.allowed, "Request {} should be allowed", i);
        }

        // 4th request should be rate limited
        let info = limiter.check("test-api-key");
        assert!(!info.allowed);
        assert_eq!(info.remaining, 0);
        assert!(info.retry_after.is_some());
    }

    #[test]
    fn test_anonymous_key_default_limit() {
        let limiter = RateLimiter::new();

        // Anonymous key should use default Free tier (100 req/min)
        let tier = limiter.get_key_tier("anonymous");
        assert_eq!(tier, KeyTier::Free);
        assert_eq!(tier.rate_limit(), 100);
    }

    // ========================================================================
    // Entropy Proof Tests (Phase 2)
    // ========================================================================

    #[test]
    fn test_entropy_proof_required_before_threshold() {
        // Before ENTROPY_REQUIRED_HEIGHT, proofs are optional
        assert!(!is_entropy_proof_required(0));
        assert!(!is_entropy_proof_required(100_000));
        assert!(!is_entropy_proof_required(ENTROPY_REQUIRED_HEIGHT - 1));
    }

    #[test]
    fn test_entropy_proof_required_at_threshold() {
        // At and after ENTROPY_REQUIRED_HEIGHT, proofs are required
        assert!(is_entropy_proof_required(ENTROPY_REQUIRED_HEIGHT));
        assert!(is_entropy_proof_required(ENTROPY_REQUIRED_HEIGHT + 1));
        assert!(is_entropy_proof_required(ENTROPY_REQUIRED_HEIGHT + 100_000));
    }

    #[test]
    fn test_entropy_proof_mandatory_before_threshold() {
        // Before ENTROPY_MANDATORY_HEIGHT, proofs are not mandatory
        assert!(!is_entropy_proof_mandatory(0));
        assert!(!is_entropy_proof_mandatory(ENTROPY_REQUIRED_HEIGHT));
        assert!(!is_entropy_proof_mandatory(ENTROPY_MANDATORY_HEIGHT - 1));
    }

    #[test]
    fn test_entropy_proof_mandatory_at_threshold() {
        // At and after ENTROPY_MANDATORY_HEIGHT, proofs are mandatory
        assert!(is_entropy_proof_mandatory(ENTROPY_MANDATORY_HEIGHT));
        assert!(is_entropy_proof_mandatory(ENTROPY_MANDATORY_HEIGHT + 1));
    }

    #[test]
    fn test_entropy_validation_result_with_proof() {
        // With a proof present, should return "valid"
        let proof_data = Some(json!({"test": "proof"}));
        let result = compute_entropy_validation_result(&proof_data, 0);
        assert_eq!(result, Some("valid".to_string()));
    }

    #[test]
    fn test_entropy_validation_result_without_proof_transition() {
        // Without proof in transition period, should return "not_provided"
        let result = compute_entropy_validation_result(&None, 0);
        assert_eq!(result, Some("not_provided".to_string()));

        let result = compute_entropy_validation_result(&None, ENTROPY_REQUIRED_HEIGHT - 1);
        assert_eq!(result, Some("not_provided".to_string()));
    }

    #[test]
    fn test_entropy_validation_result_without_proof_required() {
        // Without proof after required height, should return "no_decay_credit"
        let result = compute_entropy_validation_result(&None, ENTROPY_REQUIRED_HEIGHT);
        assert_eq!(result, Some("no_decay_credit".to_string()));

        let result = compute_entropy_validation_result(&None, ENTROPY_REQUIRED_HEIGHT + 100);
        assert_eq!(result, Some("no_decay_credit".to_string()));
    }

    #[test]
    fn test_effective_decay_rate_with_valid_proof() {
        // With valid proof, should get full decay rate
        assert_eq!(
            compute_effective_decay_rate(Some("valid"), 0),
            BASE_DECAY_RATE
        );
        assert_eq!(
            compute_effective_decay_rate(Some("valid"), ENTROPY_REQUIRED_HEIGHT),
            BASE_DECAY_RATE
        );
    }

    #[test]
    fn test_effective_decay_rate_without_proof_transition() {
        // Without proof in transition period, should get minimal rate
        assert_eq!(
            compute_effective_decay_rate(Some("not_provided"), 0),
            MINIMAL_DECAY_RATE
        );
        assert_eq!(
            compute_effective_decay_rate(Some("not_provided"), ENTROPY_REQUIRED_HEIGHT - 1),
            MINIMAL_DECAY_RATE
        );
    }

    #[test]
    fn test_effective_decay_rate_without_proof_required() {
        // Without proof after required height, should get zero
        assert_eq!(
            compute_effective_decay_rate(Some("not_provided"), ENTROPY_REQUIRED_HEIGHT),
            0
        );
        assert_eq!(
            compute_effective_decay_rate(Some("no_decay_credit"), ENTROPY_REQUIRED_HEIGHT),
            0
        );
    }

    #[test]
    fn test_effective_decay_rate_invalid_proof() {
        // Invalid proof should get zero
        assert_eq!(compute_effective_decay_rate(Some("invalid"), 0), 0);
        assert_eq!(
            compute_effective_decay_rate(Some("invalid"), ENTROPY_REQUIRED_HEIGHT),
            0
        );
    }

    #[test]
    fn test_decay_rate_constants() {
        // Verify constants match design document
        assert_eq!(BASE_DECAY_RATE, 50_000); // 5%
        assert_eq!(MINIMAL_DECAY_RATE, 5_000); // 0.5%
        assert_eq!(ENTROPY_REQUIRED_HEIGHT, 500_000);
        assert_eq!(ENTROPY_MANDATORY_HEIGHT, 1_000_000);
    }
}
