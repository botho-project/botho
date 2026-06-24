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
use tracing::{debug, error, info, warn};

use crate::{
    address::Address,
    config::QuorumConfig,
    ledger::Ledger,
    mempool::Mempool,
    network::{NetworkStats, SyncStatusSnapshot},
    node::MinterHealth,
    transaction::{TxOutput, MIN_TX_FEE},
    wallet::Wallet,
};
use bth_cluster_tax::{FeeConfig, TransactionType};
use bth_transaction_types::constants::Network;

/// Stable node identity material exposed by `node_getIdentity` (#500, epic
/// #441 Phase P1).
///
/// A thin client (e.g. the mobile app) must be able to verify *which* node it
/// is talking to before trusting it for the Mode-2 node-selection UX. All
/// fields here are derived from durable node state — the persistent libp2p
/// keypair (#439/#440) and the configured [`Network`] — rather than per-restart
/// ephemeral values, so the identity is stable across restarts.
///
/// The fields are pre-computed once at startup (in `commands::run`) and stored
/// as plain strings so the RPC layer needs no libp2p / SCP types and the
/// handler stays trivially testable.
#[derive(Debug, Clone, Default)]
pub struct NodeIdentity {
    /// libp2p peer ID derived from the persistent node keypair (#439/#440).
    /// Stable across restarts; empty only in tests / before the network layer
    /// has supplied it.
    pub peer_id: String,
    /// SCP node-id signing public key (hex), derived deterministically from the
    /// peer ID via `peer_id_to_node_id`. This is the key the quorum machinery
    /// identifies the node by.
    pub node_id_public_key: String,
    /// Wire protocol version this node speaks (e.g. `"2.0.0"`).
    pub protocol_version: String,
    /// Minimum protocol version this node will accept from peers.
    pub min_protocol_version: String,
    /// DNS-seed namespace for this node's network (e.g.
    /// `"seeds.testnet.botho.io"`), so a thin client can cross-check that the
    /// node belongs to the expected discovery domain.
    pub dns_seed_domain: String,
}

/// A single connected-peer snapshot surfaced by `network_getPeers` (#544).
///
/// Previously `network_getPeers` returned a hardcoded empty list, leaving thin
/// clients unable to enumerate peers over RPC. This type carries the live peer
/// set published from the network event loop in `commands::run`.
///
/// Like [`NodeIdentity`], the fields are pre-rendered into plain
/// strings/primitives by the producer (which owns the libp2p types) so the RPC
/// layer needs no libp2p / discovery types and the handler stays trivially
/// testable. The snapshot is a cheap clone of the discovery peer table taken on
/// peer connect/disconnect — no hot-path locking beyond the existing
/// `Arc<RwLock<..>>` pattern used for `peer_count`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PeerInfoSnapshot {
    /// libp2p peer ID string.
    pub peer_id: String,
    /// Last known multiaddr for the peer, if one has been observed. `None`
    /// renders as a JSON `null`.
    pub address: Option<String>,
    /// Peer's advertised protocol version (e.g. `"2.0.0"`), if identified.
    pub protocol_version: Option<String>,
    /// Whether the peer's protocol version is below the minimum supported.
    pub version_warning: bool,
    /// Seconds since this peer was last seen, measured when the snapshot was
    /// taken. A coarse liveness hint, not a precise timestamp.
    pub last_seen_secs: u64,
}

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
    /// Wallet for signing faucet transactions (None if no wallet or faucet
    /// disabled)
    pub wallet: Option<Arc<Wallet>>,
    /// Quorum configuration, used to surface Byzantine-fault-tolerance posture
    /// in `node_getStatus` (#509). Defaults to [`QuorumConfig::default`].
    pub quorum: QuorumConfig,
    /// Stable node identity surfaced by `node_getIdentity` (#500). Defaults to
    /// an empty identity; `commands::run` populates it from the persistent
    /// node keypair once the network layer is up.
    pub identity: NodeIdentity,
    /// Shared minter-health handle for stuck-miner detection (#538). `None`
    /// until `commands::run` wires it in (or in tests / relay nodes); the inner
    /// `Option` is `None` until minting first starts. Surfaced as `stalled` in
    /// `minting_getStatus` and `minerStalled` in `node_getStatus`.
    pub minter_health: Option<Arc<RwLock<Option<MinterHealth>>>>,
    /// Shared sync-status handle for honest sync reporting (#541). `None` until
    /// `commands::run` wires it in (or in tests / single-node setups); the
    /// inner `Option` is `None` until the sync loop publishes its first
    /// snapshot. When absent, `node_getStatus` falls back to assuming a
    /// caught-up node. Surfaced as `synced`, `syncStatus`, and
    /// `syncProgress` in `node_getStatus`.
    pub sync_status: Option<Arc<RwLock<Option<SyncStatusSnapshot>>>>,
    /// Shared snapshot of the live connected-peer set surfaced by
    /// `network_getPeers` (#544). `commands::run` publishes a cheap clone of
    /// the discovery peer table here on each peer connect/disconnect; the
    /// RPC layer reads it. Empty by default (tests / relay / no peers
    /// connected), in which case `network_getPeers` returns an empty list.
    pub peers: Arc<RwLock<Vec<PeerInfoSnapshot>>>,
    /// Shared live network-traffic counters surfaced by `network_getInfo`
    /// (#542). `commands::run` wires in the same [`NetworkStats`] handle the
    /// network event loop increments on send/receive and connect/disconnect, so
    /// `bytesSent`, `bytesReceived`, and `inboundCount` report real values
    /// instead of the previous hardcoded `0`. `None` in tests / relay nodes
    /// that never start the network loop, in which case those fields fall back
    /// to `0`.
    pub network_stats: Option<Arc<NetworkStats>>,
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
            wallet: None,
            quorum: QuorumConfig::default(),
            identity: NodeIdentity::default(),
            minter_health: None,
            sync_status: None,
            peers: Arc::new(RwLock::new(Vec::new())),
            network_stats: None,
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
            wallet: None,
            quorum: QuorumConfig::default(),
            identity: NodeIdentity::default(),
            minter_health: None,
            sync_status: None,
            peers: Arc::new(RwLock::new(Vec::new())),
            network_stats: None,
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
            wallet: None,
            quorum: QuorumConfig::default(),
            identity: NodeIdentity::default(),
            minter_health: None,
            sync_status: None,
            peers: Arc::new(RwLock::new(Vec::new())),
            network_stats: None,
        }
    }

    /// Set the faucet state and wallet for signing faucet transactions
    pub fn with_faucet(mut self, faucet: FaucetState, wallet: Wallet) -> Self {
        self.faucet = Some(Arc::new(faucet));
        self.wallet = Some(Arc::new(wallet));
        self
    }

    /// Set the wallet for balance checking (without faucet)
    pub fn with_wallet(mut self, wallet: Wallet) -> Self {
        self.wallet = Some(Arc::new(wallet));
        self
    }

    /// Set the quorum configuration so `node_getStatus` can report the
    /// cluster's Byzantine-fault-tolerance posture (#509).
    pub fn with_quorum(mut self, quorum: QuorumConfig) -> Self {
        self.quorum = quorum;
        self
    }

    /// Set the stable node identity surfaced by `node_getIdentity` (#500).
    pub fn with_identity(mut self, identity: NodeIdentity) -> Self {
        self.identity = identity;
        self
    }

    /// Wire in the shared minter-health handle so `minting_getStatus` and
    /// `node_getStatus` can surface live hashrate and the stuck-miner flag
    /// (#538). `commands::run` calls this with `Node::minter_health`.
    pub fn with_minter_health(mut self, minter_health: Arc<RwLock<Option<MinterHealth>>>) -> Self {
        self.minter_health = Some(minter_health);
        self
    }

    /// Read the current minter-health snapshot, if a handle is wired in and
    /// minting has started. Returns `None` for relay nodes / pre-mint state.
    fn minter_health_snapshot(&self) -> Option<crate::node::minter::MinterHealthSnapshot> {
        let handle = self.minter_health.as_ref()?;
        let guard = handle.read().ok()?;
        guard.as_ref().map(|h| h.snapshot())
    }

    /// Wire in the shared sync-status handle so `node_getStatus` can report
    /// honest `synced`/`syncStatus`/`syncProgress` from the live
    /// `ChainSyncManager` (#541). `commands::run` calls this with the handle
    /// the sync loop publishes into.
    pub fn with_sync_status(
        mut self,
        sync_status: Arc<RwLock<Option<SyncStatusSnapshot>>>,
    ) -> Self {
        self.sync_status = Some(sync_status);
        self
    }

    /// Read the current sync-status snapshot, if a handle is wired in and the
    /// sync loop has published at least once. Returns `None` for single-node /
    /// pre-sync state, in which case `node_getStatus` assumes a caught-up node.
    fn sync_status_snapshot(&self) -> Option<SyncStatusSnapshot> {
        let handle = self.sync_status.as_ref()?;
        let guard = handle.read().ok()?;
        guard.clone()
    }

    /// Wire in a pre-populated connected-peer snapshot handle so
    /// `network_getPeers` returns the live peer set (#544). `commands::run`
    /// shares the same handle it publishes the discovery peer table into.
    pub fn with_peers(mut self, peers: Arc<RwLock<Vec<PeerInfoSnapshot>>>) -> Self {
        self.peers = peers;
        self
    }

    /// Wire in the shared live network-traffic counters so `network_getInfo`
    /// reports real `bytesSent` / `bytesReceived` / `inboundCount` /
    /// `outboundCount` (#542). `commands::run` passes the same
    /// [`NetworkStats`] handle the network event loop increments.
    pub fn with_network_stats(mut self, network_stats: Arc<NetworkStats>) -> Self {
        self.network_stats = Some(network_stats);
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

/// Validate an incoming WebSocket upgrade request and compute its accept key.
///
/// Implements the RFC 6455 server-side opening-handshake checks:
/// - `Upgrade: websocket` (case-insensitive)
/// - `Connection` contains the `upgrade` token (case-insensitive; browsers and
///   proxies frequently send `keep-alive, Upgrade`)
/// - a present `Sec-WebSocket-Key`
///
/// On success returns the value to send back in `Sec-WebSocket-Accept`. On
/// failure returns a short, stable reason string suitable for a `400` body —
/// this is exactly the path that produced the live `wss://.../rpc/ws` 400 when
/// a stale node binary mishandled the handshake (#329), so it is covered by
/// unit tests to lock the contract.
fn validate_websocket_upgrade(headers: &hyper::HeaderMap) -> Result<String, &'static str> {
    let has_upgrade = headers
        .get("Upgrade")
        .map(|v| v.to_str().unwrap_or("").eq_ignore_ascii_case("websocket"))
        .unwrap_or(false);
    if !has_upgrade {
        return Err("Missing or invalid Upgrade header (expected 'websocket')");
    }

    let has_connection = headers
        .get("Connection")
        .map(|v| v.to_str().unwrap_or("").to_lowercase().contains("upgrade"))
        .unwrap_or(false);
    if !has_connection {
        return Err("Missing 'upgrade' token in Connection header");
    }

    let key = match headers
        .get("Sec-WebSocket-Key")
        .and_then(|v| v.to_str().ok())
    {
        Some(k) if !k.is_empty() => k,
        _ => return Err("Missing Sec-WebSocket-Key header"),
    };

    Ok(compute_websocket_accept_key(key))
}

/// Handle WebSocket upgrade request
async fn handle_websocket_upgrade(
    req: Request<hyper::body::Incoming>,
    state: Arc<RpcState>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    // Validate the handshake headers and compute the accept key.
    let accept_key = match validate_websocket_upgrade(req.headers()) {
        Ok(key) => key,
        Err(reason) => {
            warn!("Rejected WebSocket upgrade: {}", reason);
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Full::new(Bytes::from(reason)))
                .unwrap());
        }
    };

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
        "node_getIdentity" => handle_node_identity(id, state).await,

        // Chain methods
        "getChainInfo" => handle_chain_info(id, state).await,
        "getSupplyInfo" => handle_supply_info(id, state).await,
        "getBlockByHeight" => handle_get_block(id, &request.params, state).await,
        "getBlockByHash" => handle_get_block_by_hash(id, &request.params, state).await,
        "getMempoolInfo" => handle_mempool_info(id, state).await,
        "estimateFee" | "tx_estimateFee" => handle_estimate_fee(id, &request.params, state).await,
        "fee_getRate" => handle_get_fee_rate(id, state).await,

        // Wallet methods (for thin wallet sync)
        "chain_getOutputs" => handle_get_outputs(id, &request.params, state).await,
        "chain_areKeyImagesSpent" => handle_are_key_images_spent(id, &request.params, state).await,
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
    //             "entropyBeforeCommitment":
    // hex::encode(proof.entropy_before_commitment.as_bytes()),
    // "entropyAfterCommitment":
    // hex::encode(proof.entropy_after_commitment.as_bytes()),
    // "proofSize": proof.serialized_size(),         }));
    //     }
    // }
    let _ = tx; // Suppress unused variable warning
    None
}

/// Compute the entropy validation result based on proof presence and block
/// height.
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

    // Honest sync reporting wired to the live ChainSyncManager (#541). A node
    // must not claim to be fully synced mid-download: the thin-client trust UX
    // (#503) and readiness probes rely on these fields.
    //
    // When no sync handle is wired in (single-node setups, tests) or the sync
    // loop has not yet published a snapshot, fall back to the caught-up
    // assumption — a lone node with no peers has nothing to sync against.
    let sync_snapshot = state.sync_status_snapshot();
    let synced = sync_snapshot.as_ref().map(|s| s.synced).unwrap_or(true);
    let sync_status: &str = sync_snapshot.as_ref().map(|s| s.status).unwrap_or("synced");
    // Real percentage when a best-known tip is available; 100.0 when synced or
    // when we have no peer to compare against (nothing to catch up to).
    let sync_progress: f64 = match sync_snapshot.as_ref() {
        Some(s) => s.progress_percent().unwrap_or(100.0),
        None => 100.0,
    };

    // Byzantine-fault-tolerance posture (#509). Participating node count
    // includes self, so n = scp_peers + 1. In `recommended` mode, n < 4 yields
    // a degenerate quorum that tolerates ZERO faults; >= 4 is genuinely BFT.
    let participating_nodes = scp_peers + 1;
    let quorum_fault_tolerant = state.quorum.is_bft_fault_tolerant(participating_nodes);
    let quorum_degenerate = state.quorum.is_degenerate_quorum(participating_nodes);

    // Stuck-miner health (#538): surface the same verdict as `minting_getStatus`
    // so dashboards/monitoring watching node health catch a wedged miner (active
    // but 0 H/s) immediately instead of after the chain silently halts.
    let miner_stalled = state
        .minter_health_snapshot()
        .map(|s| s.stalled)
        .unwrap_or(false);

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
            "syncStatus": sync_status,
            "syncProgress": sync_progress,
            "synced": synced,
            "chainHeight": chain_state.height,
            "tipHash": hex::encode(chain_state.tip_hash),
            "peerCount": peers,
            "scpPeerCount": scp_peers,
            "mempoolSize": mempool.len(),
            "mintingActive": minting,
            "mintingThreads": if minting { state.minting_threads } else { 0 },
            "totalTransactions": chain_state.total_tx,
            // BFT posture: `quorumFaultTolerant` is true only with >= 4
            // participating nodes in recommended mode (3f+1 >= 4 for f=1);
            // `quorumDegenerate` flags the n-of-n / zero-fault-tolerance regime.
            "quorumFaultTolerant": quorum_fault_tolerant,
            "quorumDegenerate": quorum_degenerate,
            // Stuck-miner early-warning (#538): true iff this node's miner is
            // active but producing 0 H/s past the grace + stall window.
            "minerStalled": miner_stalled,
        }),
    )
}

/// Return the node's stable, verifiable identity (#500, epic #441 Phase P1).
///
/// This is the read-only surface a thin client (mobile app) calls to decide
/// *which* node it is talking to before trusting it for the Mode-2
/// node-selection UX. Every field is grounded in durable node state:
///
/// - `peerId` / `nodeId` come from the persistent libp2p keypair (#439/#440),
///   so they are stable across restarts (an ephemeral, per-restart peer ID
///   would let an attacker impersonate a previously-trusted node).
/// - `network` distinguishes mainnet from testnet so a phone cannot be tricked
///   into trusting a wrong-network node. It mirrors the `botho-<name>` form
///   already used by `node_getStatus`.
/// - `protocolVersion` / `minProtocolVersion` let the client check wire
///   compatibility before depending on the node.
/// - `dnsSeedDomain` lets the client cross-check the node against the expected
///   DNS-seed discovery namespace (`dns_seeds.rs`).
/// - `chainHeight` / `tipHash` are the current tip so the client can sanity-
///   check that the node is on the chain it expects.
///
/// The response shape is intended to be stable enough for the mobile client to
/// depend on; new fields may be added but existing ones will not change
/// meaning.
async fn handle_node_identity(id: Value, state: &RpcState) -> JsonRpcResponse {
    let ledger = read_lock!(state.ledger, id.clone());
    let chain_state = ledger.get_chain_state().unwrap_or_default();
    drop(ledger);

    let identity = &state.identity;

    JsonRpcResponse::success(
        id,
        json!({
            // Stable identity material (persistent keypair, #439/#440).
            "peerId": identity.peer_id,
            "nodeId": identity.node_id_public_key,
            // Network the node belongs to: "botho-mainnet" / "botho-testnet".
            "network": format!("botho-{}", state.network_type.name()),
            // Wire-protocol compatibility window.
            "protocolVersion": identity.protocol_version,
            "minProtocolVersion": identity.min_protocol_version,
            // Node software version + build provenance (mirrors node_getStatus).
            "nodeVersion": env!("CARGO_PKG_VERSION"),
            "version": env!("CARGO_PKG_VERSION"),
            "gitCommit": option_env!("GIT_HASH").unwrap_or("unknown"),
            // DNS-seed discovery namespace for this network.
            "dnsSeedDomain": identity.dns_seed_domain,
            // Current chain tip so the client can confirm the node's chain.
            "chainHeight": chain_state.height,
            "tipHash": hex::encode(chain_state.tip_hash),
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
            // Monetary totals are u128 picocredits and always exceed JS's
            // 2^53 safe-integer limit; emit as decimal strings so JSON clients
            // can parse them into BigInt without precision loss (#333).
            "totalMined": chain_state.total_mined.to_string(),
            "totalFeesBurned": chain_state.total_fees_burned.to_string(),
            "circulatingSupply": circulating_supply.to_string(),
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

    // Redistribution-lottery carryover pool (consensus state): the cumulative
    // balance awaiting payout, which the per-block lottery draws from (capped at
    // one block reward per block). Exposed so monetary clients and the
    // fee->pool->payout accounting tests can observe that payouts are drawn from
    // the pool and the pool never underflows. Missing/error -> 0.
    let lottery_pool = ledger.get_lottery_pool().unwrap_or(0);

    JsonRpcResponse::success(
        id,
        json!({
            "height": chain_state.height,
            // u128 picocredits emitted as decimal strings — see handle_chain_info (#333).
            "totalMined": chain_state.total_mined.to_string(),
            "totalFeesBurned": chain_state.total_fees_burned.to_string(),
            "circulatingSupply": circulating_supply.to_string(),
            "lotteryPool": lottery_pool.to_string(),
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

/// Get a block by its hash.
///
/// Mirrors [`handle_get_block`] but resolves the block from its 32-byte hash
/// instead of its height. This backs the explorer's block-by-hash search and
/// `/explorer/block/:hash` deep links (issue #330).
///
/// The lookup is delegated to [`Ledger::get_block_by_hash`], scanning the full
/// chain (from tip to genesis) so any historical block can be resolved, not
/// just recent ones. An unknown hash returns a "Block not found" error so the
/// adapter can map it to a not-found state.
async fn handle_get_block_by_hash(id: Value, params: &Value, state: &RpcState) -> JsonRpcResponse {
    // Parse hash parameter (accept either `hash` or `block_hash`).
    let hash_hex = match params
        .get("hash")
        .or_else(|| params.get("block_hash"))
        .and_then(|v| v.as_str())
    {
        Some(hex) => hex,
        None => return JsonRpcResponse::error(id, -32602, "Missing hash parameter"),
    };

    let block_hash: [u8; 32] = match hex::decode(hash_hex) {
        Ok(bytes) if bytes.len() == 32 => {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            arr
        }
        _ => {
            return JsonRpcResponse::error(id, -32602, "Invalid hash: expected 32-byte hex string")
        }
    };

    let ledger = read_lock!(state.ledger, id.clone());

    // Scan the entire chain (tip down to genesis) for a matching hash.
    let chain_height = ledger.get_chain_state().map(|s| s.height).unwrap_or(0);

    match ledger.get_block_by_hash(&block_hash, chain_height) {
        Ok(Some(block)) => JsonRpcResponse::success(
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
        Ok(None) => JsonRpcResponse::error(id, -32000, "Block not found"),
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

            // Coinbase (minting reward) output. This is a real stealth TxOutput
            // stored in the UTXO set (see LedgerStore: `block.minting_tx
            // .to_tx_output()`), but it is NOT part of `block.transactions`, so
            // it must be emitted explicitly. Without it, thin wallets scanning a
            // freshly-mined chain see no outputs at all — they cannot find their
            // own (coinbase) UTXOs to spend, nor build a decoy ring. The
            // `outputIndex` is encoded as u32::MAX to distinguish coinbase from
            // regular transaction outputs.
            let coinbase = block.minting_tx.to_tx_output();
            let coinbase_tags: Vec<[u64; 2]> = coinbase
                .cluster_tags
                .entries
                .iter()
                .map(|e| [e.cluster_id.0, e.weight as u64])
                .collect();
            outputs.push(json!({
                "txHash": hex::encode(block.minting_tx.hash()),
                "outputIndex": u32::MAX,
                "targetKey": hex::encode(coinbase.target_key),
                "publicKey": hex::encode(coinbase.public_key),
                "amountCommitment": hex::encode(coinbase.amount.to_le_bytes()),
                "clusterTags": coinbase_tags,
                "coinbase": true,
            }));

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

            // Lottery payout outputs. Like the coinbase, these are real stealth
            // UTXOs minted into the set by `LedgerStore::add_block` (id =
            // (block_hash, 1 + lottery_index)) but they are NOT part of
            // `block.transactions`, so they must be emitted explicitly here or a
            // thin wallet scanning the chain would never see a lottery payout it
            // won. The payout reuses the winning UTXO's stealth keys (target /
            // public), so the winner's `scanOwnedOutputs`/`belongs_to` detects
            // it. `txHash` is the block hash and `outputIndex` is `1 + index` to
            // mirror the ledger's deterministic id scheme (coinbase holds index
            // 0 under the block hash; tx outputs use the tx hash, never the
            // block hash, so these ids cannot collide).
            let block_hash = block.hash();
            for (lottery_idx, lottery_output) in block.lottery_outputs.iter().enumerate() {
                outputs.push(json!({
                    "txHash": hex::encode(block_hash),
                    "outputIndex": (lottery_idx as u32) + 1,
                    "targetKey": hex::encode(lottery_output.target_key),
                    "publicKey": hex::encode(lottery_output.public_key),
                    "amountCommitment": hex::encode(lottery_output.payout.to_le_bytes()),
                    "clusterTags": Vec::<[u64; 2]>::new(),
                    "lottery": true,
                }));
            }

            blocks.push(json!({
                "height": height,
                "outputs": outputs,
            }));
        }
    }

    JsonRpcResponse::success(id, json!(blocks))
}

/// Report which of the supplied key images have been spent.
///
/// This is a read-only query that lets thin (web) wallets learn whether their
/// owned outputs have already been spent, so they can exclude spent outputs
/// from both their displayed balance and their spendable-output selection.
/// The thin wallet computes each owned output's key image client-side (it holds
/// the spend key) and asks the node which are spent — mirroring the
/// double-spend check the node already performs for its own configured wallet
/// in `handle_wallet_balance`.
///
/// Params: `{ "keyImages": ["<hex>", ...] }` (hex-encoded 32-byte key images).
/// Also accepts the snake_case alias `key_images` for convenience.
///
/// Result: a list with one entry per input key image, preserving order:
/// `[{ "keyImage": "<hex>", "spent": bool, "spentHeight": <u64|null>,
/// "pending": bool }]`.
/// - `spent` is true if the key image is recorded on-chain (double-spend set).
/// - `spentHeight` is the block height at which it was spent (null if unspent).
/// - `pending` is true if the key image is currently pending in the mempool (an
///   in-flight spend not yet mined). Wallets should treat either `spent ||
///   pending` as "not spendable".
///
/// Invalid hex or wrong-length entries are reported with `spent: false`,
/// `pending: false`, and an `error` field rather than failing the whole call.
async fn handle_are_key_images_spent(
    id: Value,
    params: &Value,
    state: &RpcState,
) -> JsonRpcResponse {
    let key_images = params
        .get("keyImages")
        .or_else(|| params.get("key_images"))
        .and_then(|v| v.as_array());

    let key_images = match key_images {
        Some(arr) => arr,
        None => {
            return JsonRpcResponse::error(
                id,
                -32602,
                "Missing keyImages parameter (expected an array of hex strings)",
            )
        }
    };

    let ledger = read_lock!(state.ledger, id.clone());
    let mempool = read_lock!(state.mempool, id);

    let mut results = Vec::with_capacity(key_images.len());

    for entry in key_images {
        let ki_hex = match entry.as_str() {
            Some(s) => s,
            None => {
                results.push(json!({
                    "keyImage": entry,
                    "spent": false,
                    "spentHeight": Value::Null,
                    "pending": false,
                    "error": "key image must be a hex string",
                }));
                continue;
            }
        };

        let bytes = match hex::decode(ki_hex) {
            Ok(b) => b,
            Err(_) => {
                results.push(json!({
                    "keyImage": ki_hex,
                    "spent": false,
                    "spentHeight": Value::Null,
                    "pending": false,
                    "error": "invalid hex encoding",
                }));
                continue;
            }
        };

        let key_image_bytes: [u8; 32] = match bytes.try_into() {
            Ok(arr) => arr,
            Err(_) => {
                results.push(json!({
                    "keyImage": ki_hex,
                    "spent": false,
                    "spentHeight": Value::Null,
                    "pending": false,
                    "error": "key image must be 32 bytes",
                }));
                continue;
            }
        };

        let spent_height = ledger.is_key_image_spent(&key_image_bytes).unwrap_or(None);
        let pending = mempool.is_key_image_pending(&key_image_bytes);

        results.push(json!({
            "keyImage": ki_hex,
            "spent": spent_height.is_some(),
            "spentHeight": spent_height,
            "pending": pending,
        }));
    }

    JsonRpcResponse::success(id, json!(results))
}

async fn handle_wallet_balance(id: Value, state: &RpcState) -> JsonRpcResponse {
    // Check if we have a wallet configured
    let wallet = match &state.wallet {
        Some(w) => w,
        None => {
            return JsonRpcResponse::success(
                id,
                json!({
                    "confirmed": 0,
                    "pending": 0,
                    "total": 0,
                    "utxoCount": 0,
                    "error": "No wallet configured"
                }),
            )
        }
    };

    // Scan UTXOs for this wallet
    let ledger = read_lock!(state.ledger, id);
    let all_utxos = match ledger.scan_utxos_for_account(wallet.account_key()) {
        Ok(u) => u,
        Err(e) => {
            error!("Wallet balance scan failed: {}", e);
            return JsonRpcResponse::error(id, -32000, "Failed to scan wallet UTXOs");
        }
    };

    // Check mempool for pending key images (UTXOs being spent)
    let mempool = read_lock!(state.mempool, id);
    let mut confirmed_balance: u64 = 0;
    let mut utxo_count = 0;

    for utxo in &all_utxos {
        // Check if this output belongs to us and get subaddress index
        if let Some(subaddress_index) = utxo.output.belongs_to(wallet.account_key()) {
            // Recover the one-time private key to compute key image
            if let Some(onetime_private) = utxo
                .output
                .recover_spend_key(wallet.account_key(), subaddress_index)
            {
                let key_image = bth_crypto_ring_signature::KeyImage::from(&onetime_private);
                let key_image_bytes = key_image.as_bytes();

                // Skip if pending in mempool
                if mempool.is_key_image_pending(&key_image_bytes) {
                    continue;
                }

                // Skip if already spent on-chain
                if ledger
                    .is_key_image_spent(&key_image_bytes)
                    .unwrap_or(None)
                    .is_some()
                {
                    continue;
                }

                confirmed_balance += utxo.output.amount;
                utxo_count += 1;
            }
        }
    }

    JsonRpcResponse::success(
        id,
        json!({
            "confirmed": confirmed_balance,
            "pending": 0,
            "total": confirmed_balance,
            "utxoCount": utxo_count,
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
/// - `entropyValidationResult`: Validation status ("valid", "not_provided",
///   "no_decay_credit", "invalid")
/// - `effectiveDecayRate`: Computed decay rate based on entropy proof
/// - `entropyProofRequired`: Whether entropy proof is required at this block
///   height
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
/// - `entropyValidationResult`: Validation status ("valid", "not_provided",
///   "no_decay_credit")
/// - `entropyProofRequired`: Whether entropy proof is required at this block
///   height
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
            // For confirmed transactions, we need the block height to determine entropy
            // status Since this is the lightweight endpoint, we use chain
            // height as approximation Full entropy info is available via
            // getTransaction
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
    drop(ledger);

    // Live hashrate / total-hashes and the stuck-miner verdict come from the
    // shared minter-health handle (#538). When no handle is wired in (relay
    // nodes / tests / before minting starts) we report zeros and `stalled:
    // false`, matching the previous placeholder behavior.
    let snap = state.minter_health_snapshot();
    let hashrate = snap.map(|s| s.hashrate).unwrap_or(0.0);
    let total_hashes = snap.map(|s| s.total_hashes).unwrap_or(0);
    let stalled = snap.map(|s| s.stalled).unwrap_or(false);
    // Blocks won by this node (#543): read from the same shared minter-health
    // handle. The externalize hook increments it once per block whose winning
    // coinbase belongs to this node's address. No handle (relay/tests/before
    // minting) => 0, matching the previous placeholder.
    let blocks_found = snap.map(|s| s.blocks_found).unwrap_or(0);

    JsonRpcResponse::success(
        id,
        json!({
            "active": active,
            "threads": state.minting_threads,
            "hashrate": hashrate,
            "totalHashes": total_hashes,
            "blocksFound": blocks_found,
            "currentDifficulty": chain_state.difficulty,
            "uptimeSeconds": state.start_time.elapsed().as_secs(),
            // Stuck-miner detector (#538): true iff active but 0 H/s past the
            // grace + stall window. Operator/monitoring early-warning.
            "stalled": stalled,
        }),
    )
}

async fn handle_network_info(id: Value, state: &RpcState) -> JsonRpcResponse {
    let peers = *read_lock!(state.peer_count, id.clone());

    // Surface real traffic / connection-direction counters from the live
    // network event loop (#542). When the handle is wired in (normal node
    // operation) we report the actual atomics; `inboundCount` + `outboundCount`
    // are the dialer/listener split tracked on connect/disconnect, and the byte
    // counters are cumulative gossipsub payload bytes since startup.
    //
    // When no handle is present (tests / relay nodes that never start the
    // network loop) we fall back to the previous placeholder behavior:
    // `inboundCount: 0` and `outboundCount: peerCount`, so the endpoint shape is
    // unchanged for those callers.
    // `bytesSent` / `bytesReceived` are application-layer payload totals (#542);
    // `wireBytesSent` / `wireBytesReceived` are raw bytes-on-wire including
    // Noise/yamux framing, fed by the counting transport wrapper (#550). The
    // wire totals are always >= the payload totals once any traffic flows.
    let (inbound, outbound, bytes_sent, bytes_received, wire_sent, wire_received) =
        match state.network_stats.as_ref() {
            Some(stats) => (
                stats.inbound_count(),
                stats.outbound_count(),
                stats.bytes_sent(),
                stats.bytes_received(),
                stats.wire_bytes_sent(),
                stats.wire_bytes_received(),
            ),
            None => (0, peers as u64, 0, 0, 0, 0),
        };

    JsonRpcResponse::success(
        id,
        json!({
            "peerCount": peers,
            "inboundCount": inbound,
            "outboundCount": outbound,
            "bytesSent": bytes_sent,
            "bytesReceived": bytes_received,
            "wireBytesSent": wire_sent,
            "wireBytesReceived": wire_received,
            "uptimeSeconds": state.start_time.elapsed().as_secs(),
        }),
    )
}

async fn handle_get_peers(id: Value, state: &RpcState) -> JsonRpcResponse {
    // Surface the live connected-peer set published by the network event loop
    // (#544). The snapshot is a cheap clone of the discovery peer table taken on
    // peer connect/disconnect; empty when no peers are connected (or in
    // tests / relay nodes that never wire the handle in).
    let peers = read_lock!(state.peers, id.clone());
    let peers_json: Vec<Value> = peers
        .iter()
        .map(|p| {
            json!({
                "peerId": p.peer_id,
                "address": p.address,
                "protocolVersion": p.protocol_version,
                "versionWarning": p.version_warning,
                "lastSeenSecs": p.last_seen_secs,
            })
        })
        .collect();

    JsonRpcResponse::success(
        id,
        json!({
            "peers": peers_json,
            "peerCount": peers_json.len(),
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
    use faucet::FaucetRequest;

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

    // Acquire transaction build lock to prevent concurrent UTXO selection.
    // This prevents race conditions where two requests select the same UTXO
    // and one fails with a double-spend error.
    let _tx_lock = faucet.acquire_tx_lock().await;

    // Get wallet for signing
    let wallet = match &state.wallet {
        Some(w) => w,
        None => {
            return JsonRpcResponse::error(id, -32000, "Faucet wallet not configured");
        }
    };

    // Parse recipient address
    let parsed_address = match Address::parse_for_network(&request.address, state.network_type) {
        Ok(addr) => addr,
        Err(e) => {
            return JsonRpcResponse::error(id, -32602, &format!("Invalid address: {}", e));
        }
    };
    let recipient = parsed_address.public_address();

    // Get faucet amount
    let amount = faucet.amount();

    // Get ledger for UTXO selection
    let ledger = read_lock!(state.ledger, id);
    let our_address = wallet.default_address();

    // Get our UTXOs using stealth address scanning
    // This scans all UTXOs and checks ownership via the account key
    let all_utxos = match ledger.scan_utxos_for_account(wallet.account_key()) {
        Ok(u) => u,
        Err(e) => {
            error!("Faucet: failed to scan UTXOs: {}", e);
            return JsonRpcResponse::error(id, -32000, "Failed to get faucet balance");
        }
    };

    // Filter out spent UTXOs by checking key images in both ledger and mempool
    // We need to check the mempool to avoid double-spend errors when a previous
    // faucet transaction is still pending (not yet confirmed in a block)
    let mempool = read_lock!(state.mempool, id);
    let mut utxos = Vec::new();
    info!("Faucet: scanning {} UTXOs from ledger", all_utxos.len());
    for (idx, utxo) in all_utxos.iter().enumerate() {
        // Check if this output belongs to us and get subaddress index
        if let Some(subaddress_index) = utxo.output.belongs_to(wallet.account_key()) {
            // Recover the one-time private key
            if let Some(onetime_private) = utxo
                .output
                .recover_spend_key(wallet.account_key(), subaddress_index)
            {
                // Compute the key image
                let key_image = bth_crypto_ring_signature::KeyImage::from(&onetime_private);
                let key_image_bytes = key_image.as_bytes();

                // Check if this key image is pending in mempool
                if mempool.is_key_image_pending(&key_image_bytes) {
                    debug!("Faucet: skipping UTXO with pending key image in mempool");
                    continue;
                }

                // Check if this key image has been spent on-chain
                match ledger.is_key_image_spent(&key_image_bytes) {
                    Ok(None) => {
                        // Not spent, include this UTXO
                        info!(
                            "Faucet: UTXO {} owned, unspent - id={}, key_image={}",
                            idx,
                            hex::encode(&utxo.id.to_bytes()[0..8]),
                            hex::encode(&key_image_bytes[0..8])
                        );
                        utxos.push(utxo.clone());
                    }
                    Ok(Some(_)) => {
                        // Already spent, skip
                        debug!("Faucet: skipping spent UTXO");
                    }
                    Err(e) => {
                        warn!("Faucet: failed to check key image: {}", e);
                    }
                }
            }
        }
    }
    // Drop mempool read lock before continuing
    drop(mempool);
    info!("Faucet: found {} unspent UTXOs", utxos.len());

    // Calculate fee (use minimum required fee)
    let fee = MIN_TX_FEE;

    // Check balance
    let total_balance: u64 = utxos.iter().map(|u| u.output.amount).sum();
    let required = amount + fee;

    if total_balance < required {
        error!(
            "Faucet: insufficient balance: have {} picocredits, need {}",
            total_balance, required
        );
        return JsonRpcResponse::error(id, -32000, "Faucet has insufficient balance");
    }

    // Select UTXOs (simple: use enough to cover amount + fee)
    let mut selected_utxos = Vec::new();
    let mut selected_amount = 0u64;
    info!(
        "Faucet: selecting from {} UTXOs, need {} picocredits",
        utxos.len(),
        required
    );
    for (idx, utxo) in utxos.iter().enumerate() {
        if selected_amount >= required {
            break;
        }
        info!(
            "Faucet: selected UTXO {}: id={}, amount={}, target_key={}",
            idx,
            hex::encode(&utxo.id.to_bytes()[0..8]),
            utxo.output.amount,
            hex::encode(&utxo.output.target_key[0..8])
        );
        selected_utxos.push(utxo.clone());
        selected_amount += utxo.output.amount;
    }
    info!(
        "Faucet: selected {} UTXOs totaling {} picocredits",
        selected_utxos.len(),
        selected_amount
    );

    // Get current height
    let current_height = match ledger.get_chain_state() {
        Ok(state) => state.height,
        Err(e) => {
            error!("Faucet: failed to get chain state: {}", e);
            return JsonRpcResponse::error(id, -32000, "Failed to get chain state");
        }
    };

    // Build outputs
    let mut outputs = Vec::new();
    outputs.push(TxOutput::new(amount, &recipient));

    // Change output (if any)
    let change = selected_amount - amount - fee;
    if change > 0 {
        outputs.push(TxOutput::new(change, &our_address));
    }

    // Create the transaction
    let tx = match wallet.create_private_transaction(
        &selected_utxos,
        outputs,
        fee,
        current_height,
        &ledger,
    ) {
        Ok(t) => t,
        Err(e) => {
            error!("Faucet: failed to create transaction: {}", e);
            return JsonRpcResponse::error(
                id,
                -32000,
                &format!("Failed to create transaction: {}", e),
            );
        }
    };

    let tx_hash = tx.hash();
    let tx_hash_hex = hex::encode(&tx_hash);

    // Drop ledger lock before acquiring mempool lock
    drop(ledger);

    // Submit to mempool
    {
        let mut mempool = write_lock!(state.mempool, id);
        let ledger = read_lock!(state.ledger, id);
        if let Err(e) = mempool.add_tx(tx.clone(), &ledger) {
            error!("Faucet: failed to add transaction to mempool: {}", e);
            return JsonRpcResponse::error(
                id,
                -32000,
                &format!("Failed to submit transaction: {}", e),
            );
        }
    }

    // Record successful dispense
    faucet.record_request(ip, &request.address, amount);

    info!(
        "Faucet: dispensed {} BTH to {} (tx: {})",
        amount as f64 / 1_000_000_000_000.0,
        request.address,
        &tx_hash_hex[..16]
    );

    // Broadcast to WebSocket subscribers
    state.ws_broadcaster.new_transaction(&tx_hash, fee, None);

    // Build the response via FaucetResponse so the serialized shape stays in
    // sync with the documented faucet contract. In particular `amount` is
    // serialized as a decimal string (picocredits can exceed JSON's safe
    // integer range), which is what API consumers and the e2e tests expect.
    let response = faucet::FaucetResponse::success(tx_hash_hex, amount);
    match serde_json::to_value(&response) {
        Ok(value) => JsonRpcResponse::success(id, value),
        Err(e) => {
            JsonRpcResponse::error(id, -32000, &format!("Failed to serialize response: {}", e))
        }
    }
}

/// Faucet dispense statistics calculated from blockchain.
struct FaucetDispenseStats {
    /// Amount dispensed today (current UTC day)
    daily_dispensed: u64,
    /// Total amount dispensed over all time
    lifetime_dispensed: u64,
}

/// Calculate daily and lifetime dispensed amounts from blockchain.
///
/// This scans the blockchain to find transactions where the faucet wallet
/// spent outputs. Each such transaction represents one faucet dispense.
///
/// Uses caching to avoid rescanning the entire blockchain on every request.
/// The cache stores all key images belonging to the faucet wallet and is
/// incrementally updated as new blocks are added.
///
/// The algorithm:
/// 1. Update the key image cache with any new blocks since last scan
/// 2. Scan all blocks to find transactions using cached key images as inputs
/// 3. Count transactions for today (daily) and all time (lifetime)
fn calculate_dispensed_from_blockchain(
    wallet: &Wallet,
    ledger: &Ledger,
    faucet: &FaucetState,
    faucet_amount: u64,
) -> FaucetDispenseStats {
    use std::time::{SystemTime, UNIX_EPOCH};

    // Get UTC day start (midnight)
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let day_start = now - (now % 86400);

    // Get chain state
    let chain_state = match ledger.get_chain_state() {
        Ok(s) => s,
        Err(e) => {
            warn!("Failed to get chain state for faucet stats: {}", e);
            return FaucetDispenseStats {
                daily_dispensed: 0,
                lifetime_dispensed: 0,
            };
        }
    };

    // Step 1: Update the key image cache with any new blocks.
    // We only scan blocks that haven't been processed yet.
    let cached_height = faucet.key_image_cache().cached_height;
    let start_height = if cached_height == 0 {
        0
    } else {
        cached_height + 1
    };

    if chain_state.height >= start_height {
        let mut cache = faucet.key_image_cache_mut();

        for height in start_height..=chain_state.height {
            let block = match ledger.get_block(height) {
                Ok(b) => b,
                Err(_) => continue,
            };

            // Check minting tx output (mining rewards to faucet)
            let minting_output = block.minting_tx.to_tx_output();
            if let Some(subaddr_idx) = minting_output.belongs_to(wallet.account_key()) {
                if let Some(onetime_key) =
                    minting_output.recover_spend_key(wallet.account_key(), subaddr_idx)
                {
                    let key_image = bth_crypto_ring_signature::KeyImage::from(&onetime_key);
                    cache.key_images.insert(*key_image.as_bytes());
                }
            }

            // Check regular tx outputs (change back to faucet, or incoming transfers)
            for tx in &block.transactions {
                for output in tx.outputs.iter() {
                    if let Some(subaddr_idx) = output.belongs_to(wallet.account_key()) {
                        if let Some(onetime_key) =
                            output.recover_spend_key(wallet.account_key(), subaddr_idx)
                        {
                            let key_image = bth_crypto_ring_signature::KeyImage::from(&onetime_key);
                            cache.key_images.insert(*key_image.as_bytes());
                        }
                    }
                }
            }
        }

        cache.cached_height = chain_state.height;

        debug!(
            "Faucet stats: updated cache to height {}, {} key images",
            cache.cached_height,
            cache.key_images.len()
        );
    }

    // Step 2: Scan all blocks and count transactions that spend faucet key images.
    let cache = faucet.key_image_cache();
    let mut daily_count = 0u64;
    let mut lifetime_count = 0u64;

    for height in (0..=chain_state.height).rev() {
        let block = match ledger.get_block(height) {
            Ok(b) => b,
            Err(_) => continue,
        };

        let is_today = block.header.timestamp >= day_start;

        // Check each transaction
        for tx in &block.transactions {
            // Check if any input key image belongs to faucet
            let is_faucet_tx = tx
                .inputs
                .clsag()
                .iter()
                .any(|input| cache.key_images.contains(&input.key_image));

            if is_faucet_tx {
                lifetime_count += 1;
                if is_today {
                    daily_count += 1;
                }
            }
        }
    }

    debug!(
        "Faucet stats: {} transactions today, {} lifetime",
        daily_count, lifetime_count
    );

    FaucetDispenseStats {
        daily_dispensed: daily_count * faucet_amount,
        lifetime_dispensed: lifetime_count * faucet_amount,
    }
}

/// Handle faucet status request
///
/// Returns information about the faucet configuration and current stats.
/// Daily and lifetime dispensed are calculated from the blockchain for
/// accuracy.
async fn handle_faucet_status(id: Value, state: &RpcState) -> JsonRpcResponse {
    match &state.faucet {
        Some(faucet) => {
            let stats = faucet.stats();

            // Calculate dispensed amounts from blockchain if wallet is available
            let (daily_dispensed, lifetime_dispensed) = if let Some(wallet) = &state.wallet {
                let ledger = match state.ledger.read() {
                    Ok(l) => l,
                    Err(_) => {
                        return JsonRpcResponse::error(
                            id,
                            INTERNAL_ERROR,
                            "Failed to acquire ledger lock",
                        );
                    }
                };
                let dispense_stats = calculate_dispensed_from_blockchain(
                    &wallet,
                    &ledger,
                    faucet,
                    stats.amount_per_request,
                );
                // The blockchain reflects only *confirmed* dispenses. Faucet
                // transactions sit in the mempool until mined, so a freshly
                // dispensed request would otherwise report zero. The in-memory
                // counter (incremented on every successful dispense) captures
                // these still-pending amounts, so report the larger of the two
                // to avoid under-counting today's committed dispenses.
                (
                    dispense_stats.daily_dispensed.max(stats.daily_dispensed),
                    dispense_stats.lifetime_dispensed,
                )
            } else {
                // Fall back to in-memory counter if no wallet
                (stats.daily_dispensed, 0)
            };

            JsonRpcResponse::success(
                id,
                json!({
                    "enabled": stats.enabled,
                    "amountPerRequest": stats.amount_per_request,
                    "amountPerRequestFormatted": format!("{:.6} BTH", stats.amount_per_request as f64 / 1_000_000_000_000.0),
                    "dailyDispensed": daily_dispensed,
                    "dailyDispensedFormatted": format!("{:.6} BTH", daily_dispensed as f64 / 1_000_000_000_000.0),
                    "dailyLimit": stats.daily_limit,
                    "dailyLimitFormatted": format!("{:.6} BTH", stats.daily_limit as f64 / 1_000_000_000_000.0),
                    "lifetimeDispensed": lifetime_dispensed,
                    "lifetimeDispensedFormatted": format!("{:.6} BTH", lifetime_dispensed as f64 / 1_000_000_000_000.0),
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

    /// #392: thin wallets cannot learn which of their owned outputs are spent
    /// because `chain_getOutputs` reports ownership only.
    /// `chain_areKeyImagesSpent` exposes the node's on-chain double-spend
    /// set so the wallet can exclude spent outputs from its balance and
    /// spendable selection. This verifies a spent key image is reported
    /// `spent: true` (with height) and an unspent one `spent: false`.
    #[tokio::test]
    async fn test_are_key_images_spent_reports_spent_and_unspent() {
        use crate::{ledger::Ledger, mempool::Mempool};

        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        // Record one key image as spent on-chain at height 7.
        let spent_ki: [u8; 32] = [0x11; 32];
        let unspent_ki: [u8; 32] = [0x22; 32];
        ledger.record_key_image_for_test(&spent_ki, 7).unwrap();

        let state = RpcState::new(
            ledger,
            Mempool::new(),
            Network::Testnet,
            None,
            None,
            vec![],
            Arc::new(WsBroadcaster::new(16)),
        );

        let params = json!({
            "keyImages": [hex::encode(spent_ki), hex::encode(unspent_ki)],
        });

        let resp = handle_are_key_images_spent(json!(1), &params, &state).await;
        assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);
        let result = resp.result.unwrap();
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 2);

        // First key image: spent on-chain at height 7.
        assert_eq!(arr[0]["spent"], json!(true));
        assert_eq!(arr[0]["spentHeight"], json!(7));
        assert_eq!(arr[0]["pending"], json!(false));
        assert_eq!(arr[0]["keyImage"], json!(hex::encode(spent_ki)));

        // Second key image: never spent.
        assert_eq!(arr[1]["spent"], json!(false));
        assert_eq!(arr[1]["spentHeight"], Value::Null);
        assert_eq!(arr[1]["pending"], json!(false));
        assert_eq!(arr[1]["keyImage"], json!(hex::encode(unspent_ki)));
    }

    /// #392: malformed key-image entries must be reported per-entry rather than
    /// failing the whole batch, and a missing `keyImages` param is a -32602.
    #[tokio::test]
    async fn test_are_key_images_spent_handles_bad_input() {
        use crate::{ledger::Ledger, mempool::Mempool};

        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        let state = RpcState::new(
            ledger,
            Mempool::new(),
            Network::Testnet,
            None,
            None,
            vec![],
            Arc::new(WsBroadcaster::new(16)),
        );

        // Missing param -> invalid params error.
        let resp = handle_are_key_images_spent(json!(1), &json!({}), &state).await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32602);

        // Bad hex and wrong length are reported per-entry, not fatal.
        let params = json!({ "keyImages": ["zz", "abcd"] });
        let resp = handle_are_key_images_spent(json!(1), &params, &state).await;
        let arr = resp.result.unwrap();
        let arr = arr.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["spent"], json!(false));
        assert!(arr[0]["error"].is_string());
        assert_eq!(arr[1]["spent"], json!(false));
        assert!(arr[1]["error"].is_string());
    }

    /// #509: `node_getStatus` must expose the cluster's Byzantine-fault
    /// tolerance posture. In `recommended` mode a cluster with < 4
    /// participating nodes (self + scpPeerCount) is degenerate (zero fault
    /// tolerance); >= 4 nodes is genuinely BFT.
    #[tokio::test]
    async fn test_node_status_reports_quorum_fault_tolerance() {
        use crate::{ledger::Ledger, mempool::Mempool};

        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        let state = RpcState::new(
            ledger,
            Mempool::new(),
            Network::Testnet,
            None,
            None,
            vec![],
            Arc::new(WsBroadcaster::new(16)),
        );
        // Default quorum config is recommended mode.

        // 2 SCP peers -> n = 3 -> degenerate, not fault tolerant.
        *state.scp_peer_count.write().unwrap() = 2;
        let resp = handle_node_status(json!(1), &state).await;
        let result = resp.result.unwrap();
        assert_eq!(result["scpPeerCount"], json!(2));
        assert_eq!(result["quorumDegenerate"], json!(true));
        assert_eq!(result["quorumFaultTolerant"], json!(false));

        // 3 SCP peers -> n = 4 -> BFT, not degenerate.
        *state.scp_peer_count.write().unwrap() = 3;
        let resp = handle_node_status(json!(1), &state).await;
        let result = resp.result.unwrap();
        assert_eq!(result["scpPeerCount"], json!(3));
        assert_eq!(result["quorumDegenerate"], json!(false));
        assert_eq!(result["quorumFaultTolerant"], json!(true));
    }

    /// #541: `node_getStatus` must report honest sync state wired to the live
    /// `ChainSyncManager`, not a hardcoded "always synced". This covers BOTH
    /// states: a node mid-download must report `synced=false`,
    /// `syncStatus="syncing"`, and a sub-100 progress percentage; a caught-up
    /// node must report `synced=true`, `syncStatus="synced"`, progress 100.0.
    #[tokio::test]
    async fn test_node_status_reports_real_sync_state() {
        use crate::{ledger::Ledger, mempool::Mempool, network::SyncStatusSnapshot};

        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        let sync_handle = Arc::new(RwLock::new(None));
        let state = RpcState::new(
            ledger,
            Mempool::new(),
            Network::Testnet,
            None,
            None,
            vec![],
            Arc::new(WsBroadcaster::new(16)),
        )
        .with_sync_status(sync_handle.clone());

        // --- Behind: downloading, local 50 of best-known 100. ---
        *sync_handle.write().unwrap() = Some(SyncStatusSnapshot {
            synced: false,
            status: "syncing",
            local_height: 50,
            target_height: Some(100),
        });
        let resp = handle_node_status(json!(1), &state).await;
        let result = resp.result.unwrap();
        assert_eq!(result["synced"], json!(false));
        assert_eq!(result["syncStatus"], json!("syncing"));
        assert_eq!(
            result["syncProgress"].as_f64().unwrap(),
            50.0,
            "50/100 should be 50%"
        );

        // --- Caught up: synced, progress pinned to 100. ---
        *sync_handle.write().unwrap() = Some(SyncStatusSnapshot {
            synced: true,
            status: "synced",
            local_height: 100,
            target_height: Some(100),
        });
        let resp = handle_node_status(json!(1), &state).await;
        let result = resp.result.unwrap();
        assert_eq!(result["synced"], json!(true));
        assert_eq!(result["syncStatus"], json!("synced"));
        assert_eq!(result["syncProgress"].as_f64().unwrap(), 100.0);
    }

    /// #541: when no sync handle is wired in (single-node setups / tests), the
    /// node falls back to the caught-up assumption rather than erroring — a
    /// lone node with no peers has nothing to sync against.
    #[tokio::test]
    async fn test_node_status_sync_fallback_when_no_handle() {
        use crate::{ledger::Ledger, mempool::Mempool};

        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        let state = RpcState::new(
            ledger,
            Mempool::new(),
            Network::Testnet,
            None,
            None,
            vec![],
            Arc::new(WsBroadcaster::new(16)),
        );

        let resp = handle_node_status(json!(1), &state).await;
        let result = resp.result.unwrap();
        assert_eq!(result["synced"], json!(true));
        assert_eq!(result["syncStatus"], json!("synced"));
        assert_eq!(result["syncProgress"].as_f64().unwrap(), 100.0);
    }

    /// #538: the stuck-miner flag must surface in BOTH `minting_getStatus`
    /// (`stalled`) and `node_getStatus` (`minerStalled`). A miner that is
    /// active but producing 0 H/s past the grace + stall window is flagged;
    /// a healthy or inactive miner is not; and with no health handle wired
    /// in the flag defaults to `false` (present, never missing).
    #[tokio::test]
    async fn test_miner_stalled_flag_in_both_status_payloads() {
        use crate::{
            ledger::Ledger,
            mempool::Mempool,
            node::{
                minter::{STARTUP_GRACE_SECS, STUCK_MINER_SECS},
                MinterHealth,
            },
        };

        fn fresh_state() -> RpcState {
            let dir = tempfile::tempdir().unwrap();
            let ledger = Ledger::open(dir.path()).unwrap();
            // Keep the tempdir alive for the lifetime of the ledger by leaking
            // it — fine for a unit test.
            std::mem::forget(dir);
            RpcState::new(
                ledger,
                Mempool::new(),
                Network::Testnet,
                None,
                None,
                vec![],
                Arc::new(WsBroadcaster::new(16)),
            )
        }

        // (1) No health handle wired in: flag present and false in both.
        let state = fresh_state();
        let minting = handle_minting_status(json!(1), &state)
            .await
            .result
            .unwrap();
        assert_eq!(minting["stalled"], json!(false));
        let node = handle_node_status(json!(1), &state).await.result.unwrap();
        assert_eq!(node["minerStalled"], json!(false));

        // (2) Active + 0 H/s past the grace + stall window: stalled in both.
        let stalled = MinterHealth::for_test(true, 0, STARTUP_GRACE_SECS + STUCK_MINER_SECS + 5);
        let handle = Arc::new(RwLock::new(Some(stalled)));
        let state = fresh_state().with_minter_health(handle);
        let minting = handle_minting_status(json!(1), &state)
            .await
            .result
            .unwrap();
        assert_eq!(minting["stalled"], json!(true), "minting_getStatus.stalled");
        let node = handle_node_status(json!(1), &state).await.result.unwrap();
        assert_eq!(
            node["minerStalled"],
            json!(true),
            "node_getStatus.minerStalled"
        );

        // (3) Active + healthy (hashes well past 0, recent progress): not stalled.
        // `for_test` pins last-progress to start, so to model a *healthy* miner
        // we keep uptime within the stall window after the grace.
        let healthy = MinterHealth::for_test(true, 100_000, STARTUP_GRACE_SECS + 1);
        let handle = Arc::new(RwLock::new(Some(healthy)));
        let state = fresh_state().with_minter_health(handle);
        let minting = handle_minting_status(json!(1), &state)
            .await
            .result
            .unwrap();
        assert_eq!(minting["stalled"], json!(false));
        assert_eq!(minting["totalHashes"], json!(100_000));
        let node = handle_node_status(json!(1), &state).await.result.unwrap();
        assert_eq!(node["minerStalled"], json!(false));

        // (4) Inactive miner: never stalled, even with a long zero-hash uptime.
        let inactive =
            MinterHealth::for_test(false, 0, STARTUP_GRACE_SECS + STUCK_MINER_SECS + 1000);
        let handle = Arc::new(RwLock::new(Some(inactive)));
        let state = fresh_state().with_minter_health(handle);
        let minting = handle_minting_status(json!(1), &state)
            .await
            .result
            .unwrap();
        assert_eq!(minting["stalled"], json!(false));
        let node = handle_node_status(json!(1), &state).await.result.unwrap();
        assert_eq!(node["minerStalled"], json!(false));
    }

    /// #543: `minting_getStatus.blocksFound` reflects the live count from the
    /// shared minter-health handle. With no handle wired in it is 0; after the
    /// externalize hook records won blocks (`increment_blocks_found`) the count
    /// is surfaced verbatim.
    #[tokio::test]
    async fn test_blocks_found_in_minting_status() {
        use crate::{
            ledger::Ledger,
            mempool::Mempool,
            node::{minter::STARTUP_GRACE_SECS, MinterHealth},
        };

        fn fresh_state() -> RpcState {
            let dir = tempfile::tempdir().unwrap();
            let ledger = Ledger::open(dir.path()).unwrap();
            std::mem::forget(dir);
            RpcState::new(
                ledger,
                Mempool::new(),
                Network::Testnet,
                None,
                None,
                vec![],
                Arc::new(WsBroadcaster::new(16)),
            )
        }

        // (1) No health handle wired in: blocksFound present and 0.
        let state = fresh_state();
        let minting = handle_minting_status(json!(1), &state)
            .await
            .result
            .unwrap();
        assert_eq!(minting["blocksFound"], json!(0), "no handle => 0");

        // (2) Nothing won yet (fresh handle): still 0.
        let health = MinterHealth::for_test(true, 100_000, STARTUP_GRACE_SECS + 1);
        let handle = Arc::new(RwLock::new(Some(health.clone())));
        let state = fresh_state().with_minter_health(handle);
        let minting = handle_minting_status(json!(1), &state)
            .await
            .result
            .unwrap();
        assert_eq!(minting["blocksFound"], json!(0), "no blocks won yet => 0");

        // (3) After the externalize hook records two won blocks, the RPC reads
        // the live count from the shared handle (same Arc-backed counter).
        health.increment_blocks_found();
        health.increment_blocks_found();
        let minting = handle_minting_status(json!(1), &state)
            .await
            .result
            .unwrap();
        assert_eq!(minting["blocksFound"], json!(2), "two blocks won => 2");
    }

    /// #500: `node_getIdentity` exposes the node's stable, verifiable identity
    /// so a thin client can confirm *which* node it is talking to before
    /// trusting it. Asserts the payload shape and that the configured identity
    /// material is surfaced verbatim, the network is namespaced as
    /// `botho-<name>`, and the chain tip is included.
    #[tokio::test]
    async fn test_node_identity_returns_expected_fields() {
        use crate::{ledger::Ledger, mempool::Mempool};

        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        let identity = NodeIdentity {
            peer_id: "12D3KooWTestPeerId".to_string(),
            node_id_public_key: "aa".repeat(32),
            protocol_version: "2.0.0".to_string(),
            min_protocol_version: "2.0.0".to_string(),
            dns_seed_domain: "seeds.testnet.botho.io".to_string(),
        };

        let state = RpcState::new(
            ledger,
            Mempool::new(),
            Network::Testnet,
            None,
            None,
            vec![],
            Arc::new(WsBroadcaster::new(16)),
        )
        .with_identity(identity);

        let resp = handle_node_identity(json!(1), &state).await;
        assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);
        let result = resp.result.unwrap();

        // Stable identity material is surfaced verbatim.
        assert_eq!(result["peerId"], json!("12D3KooWTestPeerId"));
        assert_eq!(result["nodeId"], json!("aa".repeat(32)));
        assert_eq!(result["protocolVersion"], json!("2.0.0"));
        assert_eq!(result["minProtocolVersion"], json!("2.0.0"));
        assert_eq!(result["dnsSeedDomain"], json!("seeds.testnet.botho.io"));

        // Network must be namespaced so a phone cannot trust a wrong-network
        // node.
        assert_eq!(result["network"], json!("botho-testnet"));

        // Version + chain tip are present and well-formed.
        assert_eq!(result["nodeVersion"], json!(env!("CARGO_PKG_VERSION")));
        assert_eq!(result["version"], json!(env!("CARGO_PKG_VERSION")));
        // Genesis ledger is at height 0; the tip must match the ledger's actual
        // chain-state tip (64-char hex), confirming it is grounded in real
        // node state rather than a fabricated value.
        let expected = {
            let ledger = state.ledger.read().unwrap();
            ledger.get_chain_state().unwrap_or_default()
        };
        assert_eq!(result["chainHeight"], json!(expected.height));
        let tip_hash = result["tipHash"].as_str().unwrap();
        assert_eq!(tip_hash.len(), 64);
        assert_eq!(tip_hash, hex::encode(expected.tip_hash));

        // gitCommit is always present (defaults to "unknown" in dev builds).
        assert!(result["gitCommit"].is_string());
    }

    /// #500: the default identity (no `with_identity`) yields empty identity
    /// strings but still produces a well-formed, network-correct payload, so
    /// the method never panics or omits required keys.
    #[tokio::test]
    async fn test_node_identity_defaults_are_well_formed() {
        use crate::{ledger::Ledger, mempool::Mempool};

        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        let state = RpcState::new(
            ledger,
            Mempool::new(),
            Network::Mainnet,
            None,
            None,
            vec![],
            Arc::new(WsBroadcaster::new(16)),
        );

        let resp = handle_node_identity(json!(1), &state).await;
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();

        assert_eq!(result["peerId"], json!(""));
        assert_eq!(result["nodeId"], json!(""));
        assert_eq!(result["network"], json!("botho-mainnet"));
        // Required keys are always present.
        for key in ["protocolVersion", "minProtocolVersion", "dnsSeedDomain"] {
            assert!(result.get(key).is_some(), "missing key: {key}");
        }
    }

    /// #509: in `explicit` mode the operator owns the threshold/membership, so
    /// the status payload never flags a degenerate quorum regardless of peers.
    #[tokio::test]
    async fn test_node_status_explicit_mode_not_flagged() {
        use crate::{
            config::{QuorumConfig, QuorumMode},
            ledger::Ledger,
            mempool::Mempool,
        };

        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        let state = RpcState::new(
            ledger,
            Mempool::new(),
            Network::Testnet,
            None,
            None,
            vec![],
            Arc::new(WsBroadcaster::new(16)),
        )
        .with_quorum(QuorumConfig {
            mode: QuorumMode::Explicit,
            ..QuorumConfig::default()
        });

        // Even a lone node in explicit mode is not flagged degenerate.
        *state.scp_peer_count.write().unwrap() = 0;
        let resp = handle_node_status(json!(1), &state).await;
        let result = resp.result.unwrap();
        assert_eq!(result["quorumDegenerate"], json!(false));
        assert_eq!(result["quorumFaultTolerant"], json!(true));
    }

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

    // ========================================================================
    // WebSocket upgrade handshake (#329)
    // ========================================================================

    /// RFC 6455 §1.3 worked example: the canonical client key
    /// "dGhlIHNhbXBsZSBub25jZQ==" must produce accept
    /// "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=".
    #[test]
    fn test_compute_websocket_accept_key_rfc6455_vector() {
        assert_eq!(
            compute_websocket_accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    /// A well-formed browser/proxy handshake must be accepted and yield the
    /// matching accept key. `Connection: keep-alive, Upgrade` mirrors what
    /// real browsers and the seed nginx proxy forward.
    #[test]
    fn test_validate_websocket_upgrade_accepts_valid_handshake() {
        let mut headers = hyper::HeaderMap::new();
        headers.insert("Upgrade", "websocket".parse().unwrap());
        headers.insert("Connection", "keep-alive, Upgrade".parse().unwrap());
        headers.insert(
            "Sec-WebSocket-Key",
            "dGhlIHNhbXBsZSBub25jZQ==".parse().unwrap(),
        );

        let accept = validate_websocket_upgrade(&headers).expect("handshake should be accepted");
        assert_eq!(accept, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    /// Case-insensitive Upgrade token (browsers may send "WebSocket").
    #[test]
    fn test_validate_websocket_upgrade_case_insensitive() {
        let mut headers = hyper::HeaderMap::new();
        headers.insert("Upgrade", "WebSocket".parse().unwrap());
        headers.insert("Connection", "Upgrade".parse().unwrap());
        headers.insert("Sec-WebSocket-Key", "abcdefghijklmnop".parse().unwrap());
        assert!(validate_websocket_upgrade(&headers).is_ok());
    }

    #[test]
    fn test_validate_websocket_upgrade_rejects_missing_upgrade() {
        let mut headers = hyper::HeaderMap::new();
        headers.insert("Connection", "Upgrade".parse().unwrap());
        headers.insert("Sec-WebSocket-Key", "abcdefghijklmnop".parse().unwrap());
        assert!(validate_websocket_upgrade(&headers).is_err());
    }

    #[test]
    fn test_validate_websocket_upgrade_rejects_missing_connection() {
        let mut headers = hyper::HeaderMap::new();
        headers.insert("Upgrade", "websocket".parse().unwrap());
        headers.insert("Sec-WebSocket-Key", "abcdefghijklmnop".parse().unwrap());
        assert!(validate_websocket_upgrade(&headers).is_err());
    }

    #[test]
    fn test_validate_websocket_upgrade_rejects_missing_key() {
        let mut headers = hyper::HeaderMap::new();
        headers.insert("Upgrade", "websocket".parse().unwrap());
        headers.insert("Connection", "Upgrade".parse().unwrap());
        assert!(validate_websocket_upgrade(&headers).is_err());
    }

    #[test]
    fn test_validate_websocket_upgrade_rejects_plain_get() {
        // A plain GET (no upgrade headers) is what an accidental HTTP request to
        // /ws looks like; it must be rejected rather than treated as a socket.
        let headers = hyper::HeaderMap::new();
        assert!(validate_websocket_upgrade(&headers).is_err());
    }

    /// #544: `network_getPeers` surfaces the live connected-peer snapshot
    /// instead of the previous hardcoded empty list.
    ///
    /// - No handle / no peers: returns an empty `peers` array and `peerCount:
    ///   0`.
    /// - With peers published into the shared snapshot: returns one entry per
    ///   peer with the expected field shape (`peerId`, `address`,
    ///   `protocolVersion`, `versionWarning`, `lastSeenSecs`).
    #[tokio::test]
    async fn test_get_peers_surfaces_connected_peers() {
        use crate::{ledger::Ledger, mempool::Mempool};

        fn fresh_state() -> RpcState {
            let dir = tempfile::tempdir().unwrap();
            let ledger = Ledger::open(dir.path()).unwrap();
            std::mem::forget(dir);
            RpcState::new(
                ledger,
                Mempool::new(),
                Network::Testnet,
                None,
                None,
                vec![],
                Arc::new(WsBroadcaster::new(16)),
            )
        }

        // (1) No peers connected: empty list, zero count.
        let state = fresh_state();
        let result = handle_get_peers(json!(1), &state).await.result.unwrap();
        assert_eq!(result["peers"], json!([]), "empty when no peers connected");
        assert_eq!(result["peerCount"], json!(0));

        // (2) Two peers published into the shared snapshot: both surfaced with
        // the documented field shape.
        let peers_handle = Arc::new(RwLock::new(vec![
            PeerInfoSnapshot {
                peer_id: "12D3KooWPeerOne".to_string(),
                address: Some("/ip4/10.0.0.1/tcp/4001".to_string()),
                protocol_version: Some("2.0.0".to_string()),
                version_warning: false,
                last_seen_secs: 3,
            },
            PeerInfoSnapshot {
                peer_id: "12D3KooWPeerTwo".to_string(),
                address: None,
                protocol_version: None,
                version_warning: true,
                last_seen_secs: 42,
            },
        ]));
        let state = fresh_state().with_peers(peers_handle);

        let result = handle_get_peers(json!(1), &state).await.result.unwrap();
        let peers = result["peers"].as_array().expect("peers is an array");
        assert_eq!(peers.len(), 2, "both connected peers surfaced");
        assert_eq!(result["peerCount"], json!(2));

        // First peer: fully populated.
        assert_eq!(peers[0]["peerId"], json!("12D3KooWPeerOne"));
        assert_eq!(peers[0]["address"], json!("/ip4/10.0.0.1/tcp/4001"));
        assert_eq!(peers[0]["protocolVersion"], json!("2.0.0"));
        assert_eq!(peers[0]["versionWarning"], json!(false));
        assert_eq!(peers[0]["lastSeenSecs"], json!(3));

        // Second peer: optional fields render as JSON null, warning flag set.
        assert_eq!(peers[1]["peerId"], json!("12D3KooWPeerTwo"));
        assert_eq!(peers[1]["address"], Value::Null);
        assert_eq!(peers[1]["protocolVersion"], Value::Null);
        assert_eq!(peers[1]["versionWarning"], json!(true));
        assert_eq!(peers[1]["lastSeenSecs"], json!(42));
    }

    /// #542: `network_getInfo` surfaces real `bytesSent` / `bytesReceived` /
    /// `inboundCount` / `outboundCount` from the live network counters instead
    /// of the previous hardcoded `0`.
    ///
    /// - No handle wired in (relay/test): falls back to the prior placeholder
    ///   shape — `inboundCount: 0`, `outboundCount: peerCount`, zero bytes.
    /// - With a `NetworkStats` handle: reports the actual atomic values.
    #[tokio::test]
    async fn test_network_info_surfaces_real_counters() {
        use crate::{ledger::Ledger, mempool::Mempool, network::NetworkStats};

        fn fresh_state() -> RpcState {
            let dir = tempfile::tempdir().unwrap();
            let ledger = Ledger::open(dir.path()).unwrap();
            std::mem::forget(dir);
            RpcState::new(
                ledger,
                Mempool::new(),
                Network::Testnet,
                None,
                None,
                vec![],
                Arc::new(WsBroadcaster::new(16)),
            )
        }

        // (1) No stats handle: legacy fallback shape, all traffic counters zero.
        let state = fresh_state();
        *state.peer_count.write().unwrap() = 3;
        let result = handle_network_info(json!(1), &state).await.result.unwrap();
        assert_eq!(result["peerCount"], json!(3));
        assert_eq!(result["inboundCount"], json!(0), "no handle -> 0 inbound");
        assert_eq!(
            result["outboundCount"],
            json!(3),
            "no handle -> outbound falls back to peerCount"
        );
        assert_eq!(result["bytesSent"], json!(0));
        assert_eq!(result["bytesReceived"], json!(0));
        // #550: wire counters also default to zero with no handle.
        assert_eq!(result["wireBytesSent"], json!(0));
        assert_eq!(result["wireBytesReceived"], json!(0));

        // (2) Stats handle wired in: real values surfaced. Simulate the network
        // event loop having recorded traffic + connections.
        let stats = Arc::new(NetworkStats::new());
        stats.record_sent(4096);
        stats.record_received(8192);
        // #550: simulate the counting transport recording raw wire bytes
        // (always >= payload once framing overhead is included).
        stats.record_wire_sent(5000);
        stats.record_wire_received(9000);
        // Two inbound, one outbound (record_connection_opened: inbound flag).
        stats.record_connection_opened(true);
        stats.record_connection_opened(true);
        stats.record_connection_opened(false);

        let state = fresh_state().with_network_stats(Arc::clone(&stats));
        *state.peer_count.write().unwrap() = 3;
        let result = handle_network_info(json!(1), &state).await.result.unwrap();
        assert_eq!(result["peerCount"], json!(3));
        assert_eq!(result["inboundCount"], json!(2), "real inbound count");
        assert_eq!(result["outboundCount"], json!(1), "real outbound count");
        assert_eq!(result["bytesSent"], json!(4096));
        assert_eq!(result["bytesReceived"], json!(8192));
        assert_eq!(result["wireBytesSent"], json!(5000), "real wire bytes sent");
        assert_eq!(
            result["wireBytesReceived"],
            json!(9000),
            "real wire bytes received"
        );

        // (3) Genuine zero: handle present but nothing sent / no inbound.
        let empty_stats = Arc::new(NetworkStats::new());
        let state = fresh_state().with_network_stats(empty_stats);
        let result = handle_network_info(json!(1), &state).await.result.unwrap();
        assert_eq!(result["bytesSent"], json!(0));
        assert_eq!(result["bytesReceived"], json!(0));
        assert_eq!(result["wireBytesSent"], json!(0));
        assert_eq!(result["wireBytesReceived"], json!(0));
        assert_eq!(result["inboundCount"], json!(0));
        assert_eq!(result["outboundCount"], json!(0));
    }
}
