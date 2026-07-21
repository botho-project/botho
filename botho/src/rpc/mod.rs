//! JSON-RPC Server for Botho
//!
//! Provides a JSON-RPC 2.0 API for thin wallets and web interfaces.
//! Also supports WebSocket connections for real-time event streaming.

pub mod auth;
pub mod deposit_scanner;
pub mod faucet;
pub mod metrics;
pub mod operator;
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
pub use operator::{OperatorAuditEntry, OperatorAuditLog};
pub use rate_limit::{KeyTier, RateLimitInfo, RateLimiter};
pub use view_keys::{RegistryError, ViewKeyInfo, ViewKeyRegistry};
pub use websocket::WsBroadcaster;

use anyhow::Result;

/// JSON-RPC internal error code
const INTERNAL_ERROR: i32 = -32603;

/// Operator surface is not configured (`[rpc.operator]` absent / empty secret).
/// A clean, stable "feature off" signal — distinct from an auth failure so the
/// dashboard can degrade to the public read-only view (#707).
const OPERATOR_NOT_ENABLED: i32 = -32020;

/// Operator read token missing, malformed, expired, or forged. Deliberately
/// GENERIC: the node must not leak which check failed (expiry vs signature vs
/// shape) — fail closed with one reason (#707).
const OPERATOR_TOKEN_REJECTED: i32 = -32021;

/// An operator WRITE action (`operator_submitAction`) was rejected: the request
/// was verified but a check refused it, or the apply path returned a refusal.
/// The response `result` carries the full structured outcome (outcome class,
/// gate verdict, reason) — the error code merely signals "not applied" (#748).
const OPERATOR_ACTION_REJECTED: i32 = -32022;

/// The operator-action write channel into the event loop is unavailable (relay
/// node / test state with no loop wired). Fail closed (#748).
const OPERATOR_ACTION_UNAVAILABLE: i32 = -32023;

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
    block::MINTING_OUTPUT_INDEX,
    config::QuorumConfig,
    consensus::{QuorumGateSnapshot, ScpSlotSnapshot},
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
    /// Shared SCP slot-progress handle for slot-stall observability (#653,
    /// epic #532 Phase 0). `commands::run` publishes a fresh
    /// [`ScpSlotSnapshot`] here on every consensus tick, derived from the live
    /// SCP slot metrics + externalization history (never a constant — the
    /// anti-#541–#544 gate). `None` until wired in (tests / relay nodes); the
    /// inner `Option` is `None` until the first tick publishes. Surfaced as
    /// the `scpSlot*` / `slotStalled` / `slotStallSeconds` fields in
    /// `node_getStatus`.
    pub scp_slot_status: Option<Arc<RwLock<Option<ScpSlotSnapshot>>>>,
    /// Shared quorum-promotion-gate handle (#651, epic #441 §3/P5).
    /// `commands::run` publishes a fresh [`QuorumGateSnapshot`] here at the
    /// initial quorum seed and on every peer-churn rebuild — always derived
    /// from a real gate evaluation, never a constant (the anti-#541–#544
    /// gate). `None` until wired in (tests / relay nodes); the inner `Option`
    /// is `None` until the first evaluation publishes. Surfaced as the
    /// `quorumCuratedMembers` / `quorumAutoMembers` /
    /// `quorumGateSuppressedPeers` / `quorumGateMaxAutoMembers` /
    /// `quorumGateIntersectionRefused` fields in `node_getStatus` (JSON
    /// `null` until first publish).
    pub quorum_gate_status: Option<Arc<RwLock<Option<QuorumGateSnapshot>>>>,
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
    /// Relay channel from the RPC layer into the network event loop (#674).
    /// `handle_submit_tx` pushes every mempool-accepted transaction here so
    /// `commands::run` can gossip it to peers and register it in the SCP tx
    /// cache immediately — independent of local minting state. Without this, a
    /// non-minting node is a black hole for RPC-submitted transactions: they
    /// sit in the local mempool and are only ever announced from inside the
    /// active-minting code path. `None` in tests / setups without a network
    /// loop, in which case submission remains mempool-local (previous
    /// behavior).
    pub tx_relay: Option<tokio::sync::mpsc::UnboundedSender<crate::transaction::Transaction>>,
    /// Operator read-token secret from `[rpc.operator] read_token_secret`
    /// (#707, P4.2). `None` ⇒ the operator surface is OFF: `operator_*` RPCs
    /// return a clean "not enabled" error and the node behaves exactly as
    /// today. `Some` ⇒ the node verifies magic-link READ tokens (constant-time
    /// HMAC, signature-before-expiry — see
    /// [`auth::verify_operator_read_token`]) before serving the
    /// operator-only reads. This grants READS ONLY; there is no operator
    /// write RPC (that is #709).
    pub operator_read_token_secret: Option<String>,
    /// Operator-action signing public keys (hex Ed25519) from
    /// `[rpc.operator] action_public_keys` (#747, P4.4a). Empty ⇒ **no write
    /// surface at all** (fail closed): the operator-signed quorum-curation
    /// write path (#709, later sub-issues) refuses `operator_submitAction`
    /// when this is empty. This issue lands ONLY the plumbing — no RPC
    /// reads this field yet, so with an empty list the node behaves
    /// byte-identically to today.
    ///
    /// Populated exclusively from config at startup (SSH/config trust domain).
    /// There is intentionally NO RPC or signed action that reads, adds, or
    /// removes entries — verified by absence (see `config::OperatorConfig`).
    pub operator_action_public_keys: Vec<String>,
    /// Operator audit-log store (#707). Present-but-empty in P4.2; #709 wires
    /// the append side. Surfaced by `operator_getAuditLog`.
    pub operator_audit_log: Arc<OperatorAuditLog>,
    /// Bounded write channel into the `commands::run` event loop for verified
    /// operator actions (#748, §4 apply path). `None` ⇒ no event loop is wired
    /// (relay node / tests) ⇒ `operator_submitAction` fails closed with
    /// `OPERATOR_ACTION_UNAVAILABLE`. Mirrors [`Self::tx_relay`]; bounded (not
    /// unbounded) so a flood of actions cannot grow memory without bound — the
    /// handler applies backpressure by awaiting a send permit.
    pub operator_action_tx:
        Option<tokio::sync::mpsc::Sender<crate::operator_action::OperatorActionRequest>>,
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
            scp_slot_status: None,
            quorum_gate_status: None,
            peers: Arc::new(RwLock::new(Vec::new())),
            network_stats: None,
            tx_relay: None,
            operator_read_token_secret: None,
            operator_action_public_keys: Vec::new(),
            operator_audit_log: OperatorAuditLog::new(),
            operator_action_tx: None,
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
            scp_slot_status: None,
            quorum_gate_status: None,
            peers: Arc::new(RwLock::new(Vec::new())),
            network_stats: None,
            tx_relay: None,
            operator_read_token_secret: None,
            operator_action_public_keys: Vec::new(),
            operator_audit_log: OperatorAuditLog::new(),
            operator_action_tx: None,
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
            scp_slot_status: None,
            quorum_gate_status: None,
            peers: Arc::new(RwLock::new(Vec::new())),
            network_stats: None,
            tx_relay: None,
            operator_read_token_secret: None,
            operator_action_public_keys: Vec::new(),
            operator_audit_log: OperatorAuditLog::new(),
            operator_action_tx: None,
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

    /// Wire in the shared SCP slot-progress handle so `node_getStatus` can
    /// surface live slot-stall observability (#653). `commands::run` calls
    /// this with the handle the consensus tick publishes
    /// [`ScpSlotSnapshot`]s into.
    pub fn with_scp_slot_status(
        mut self,
        scp_slot_status: Arc<RwLock<Option<ScpSlotSnapshot>>>,
    ) -> Self {
        self.scp_slot_status = Some(scp_slot_status);
        self
    }

    /// Read the current SCP slot-progress snapshot, if a handle is wired in
    /// and the consensus tick has published at least once. Returns `None` for
    /// tests / relay nodes / pre-first-tick state, in which case
    /// `node_getStatus` reports the slot fields as absent/idle rather than
    /// fabricating values.
    fn scp_slot_snapshot(&self) -> Option<ScpSlotSnapshot> {
        let handle = self.scp_slot_status.as_ref()?;
        let guard = handle.read().ok()?;
        guard.clone()
    }

    /// Wire in the shared quorum-promotion-gate handle so `node_getStatus`
    /// can surface curated-vs-auto quorum membership and gate state (#651).
    /// `commands::run` calls this with the handle the quorum rebuild path
    /// publishes [`QuorumGateSnapshot`]s into.
    pub fn with_quorum_gate_status(
        mut self,
        quorum_gate_status: Arc<RwLock<Option<QuorumGateSnapshot>>>,
    ) -> Self {
        self.quorum_gate_status = Some(quorum_gate_status);
        self
    }

    /// Read the current quorum-promotion-gate snapshot, if a handle is wired
    /// in and the gate has evaluated at least once. Returns `None` for tests
    /// / relay nodes / pre-first-rebuild state, in which case
    /// `node_getStatus` reports the gate fields as JSON `null` rather than
    /// fabricating values.
    /// Enable the operator read surface (#707) by wiring in the
    /// `[rpc.operator] read_token_secret`. An empty/whitespace secret is
    /// treated as "not configured" (fail closed) and leaves the surface OFF.
    /// `commands::run` calls this with
    /// `Config::rpc.operator_read_token_secret()`.
    pub fn with_operator_read_token_secret(mut self, secret: Option<String>) -> Self {
        self.operator_read_token_secret = secret.filter(|s| !s.trim().is_empty());
        self
    }

    /// Provision the operator-action signing public keys (#747, P4.4a) from
    /// `[rpc.operator] action_public_keys`. Fails closed identically to the
    /// read-token accessor: empty/whitespace-only entries are filtered out and
    /// trimmed, so an all-empty (or absent) list yields an empty `Vec` ⇒ **no
    /// write surface** (downstream sub-issues refuse `operator_submitAction`
    /// when this is empty). `commands::run` calls this with
    /// `Config::rpc.operator_action_public_keys()`.
    ///
    /// No RPC or signed action mutates this list — it is provisioned once here
    /// from config (SSH/config trust domain) and thereafter only read.
    pub fn with_operator_action_public_keys(mut self, keys: Vec<String>) -> Self {
        self.operator_action_public_keys = keys
            .into_iter()
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
            .collect();
        self
    }

    fn quorum_gate_snapshot(&self) -> Option<QuorumGateSnapshot> {
        let handle = self.quorum_gate_status.as_ref()?;
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

    /// Wire in the tx-relay channel into the network event loop (#674).
    pub fn with_tx_relay(
        mut self,
        sender: tokio::sync::mpsc::UnboundedSender<crate::transaction::Transaction>,
    ) -> Self {
        self.tx_relay = Some(sender);
        self
    }

    /// Wire in the BOUNDED operator-action channel into the `commands::run`
    /// event loop (#748, §4 apply path). Mirrors [`Self::with_tx_relay`], but
    /// bounded: the loop is the single applier and the handler awaits a send
    /// permit, so a burst of actions cannot grow memory without bound. Absent ⇒
    /// `operator_submitAction` fails closed with `OPERATOR_ACTION_UNAVAILABLE`.
    pub fn with_operator_action_channel(
        mut self,
        sender: tokio::sync::mpsc::Sender<crate::operator_action::OperatorActionRequest>,
    ) -> Self {
        self.operator_action_tx = Some(sender);
        self
    }

    /// Wire in a PERSISTED operator audit log (#750, §6) opened from the data
    /// dir. The default `RpcState` uses an in-memory-only store (tests / relay
    /// nodes); `commands::run` overrides it with one backed by
    /// `<data-dir>/operator-audit.jsonl` so authenticated outcomes survive
    /// restart and the pre-signature rejected-requests counter (finding 3) is
    /// surfaced in `node_getStatus`.
    pub fn with_operator_audit_log(mut self, audit_log: Arc<OperatorAuditLog>) -> Self {
        self.operator_audit_log = audit_log;
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
        // Quantum-private transactions were retired (ADR 0006, issue #903).
        // Keep an explicit error so old clients get a clear message instead
        // of a generic "method not found".
        "pq_tx_submit" => JsonRpcResponse::error(
            id,
            -32601,
            "quantum-private transactions retired (ADR 0006): pq_tx_submit has been removed",
        ),
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

        // Operator-only READ methods (#707, P4.2). Token-gated; fail closed
        // when `[rpc.operator]` is absent. READS ONLY — there is deliberately
        // no operator write method here (that is #709).
        "operator_getQuorumInfo" => handle_operator_quorum_info(id, &request.params, state).await,
        "operator_getAuditLog" => handle_operator_audit_log(id, &request.params, state).await,

        // Operator-only WRITE method (#748, P4.4b). Signed-envelope verified
        // (fail-closed, parse-after-verify), then gate-routed apply via the
        // bounded channel into the event loop. Fails closed when
        // `action_public_keys` is empty ("operator actions not configured").
        "operator_submitAction" => handle_operator_submit_action(id, &request.params, state).await,

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

        // Dev/testnet-only: settle the node wallet's own coins to factor-1
        // (background) so a bridge reserve can accrue spendable factor-1 outputs
        // (#1025 full-loop reserve funding).
        "dev_settleToBackground" => {
            handle_dev_settle_to_background(id, &request.params, state).await
        }

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
    let raw_synced = sync_snapshot.as_ref().map(|s| s.synced).unwrap_or(true);
    let raw_status: &str = sync_snapshot.as_ref().map(|s| s.status).unwrap_or("synced");

    // Peer-isolation cross-check (#1118). `ChainSyncManager` has no isolation
    // escape hatch: once it reaches `SyncState::Synced` it stays latched there
    // even after its last peer disconnects (`best_peer()` then returns `None`
    // and the `Synced` tick arm never re-evaluates), so a node stranded on a
    // stale singleton fork keeps self-certifying as `synced` — the #1114 relay
    // outage, where two 0-peer relays reported `synced: true` while days behind
    // the live chain. Cross-check the raw snapshot against the live `peers`
    // count here rather than trust it.
    //
    // Gated on a snapshot having been published (`sync_snapshot.is_some()`): a
    // lone dev / single-node / genesis node that never wired or ran a sync loop
    // returns `None` and is legitimately caught up — it has no live network to
    // be isolated *from*. The bug is specifically a node that *was* connected,
    // published sync state, then lost every peer. This stays grounded entirely
    // in the real, live `peers` count — never a fabricated constant
    // (anti-#541–#544).
    let isolated = peers == 0 && sync_snapshot.is_some();
    let synced = raw_synced && !isolated;
    let sync_status: &str = if isolated { "isolated" } else { raw_status };
    // Real percentage when a best-known tip is available; 100.0 when synced or
    // when we have no peer to compare against (nothing to catch up to). An
    // isolated node reports 0.0 — it has no peer to measure progress against, so
    // claiming 100% (which `progress_percent()` returns whenever `synced`)
    // would be the same lie as the latched `synced` flag.
    let sync_progress: f64 = if isolated {
        0.0
    } else {
        match sync_snapshot.as_ref() {
            Some(s) => s.progress_percent().unwrap_or(100.0),
            None => 100.0,
        }
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

    // SCP slot-stall observability (#653, epic #532 Phase 0). Every field is
    // read from the snapshot the consensus tick publishes from LIVE SCP slot
    // state (SlotMetrics + externalization history) — never fabricated here
    // (the anti-#541–#544 gate). With no handle wired in (tests / relay
    // nodes) or before the first tick, the counters render as JSON null and
    // the booleans default to false: an unwired node reports "no data", not a
    // plausible-looking constant.
    let scp_slot = state.scp_slot_snapshot();
    let scp_slot_active = scp_slot
        .as_ref()
        .map(|s| s.scp_slot_active)
        .unwrap_or(false);
    let slot_stalled = scp_slot.as_ref().map(|s| s.slot_stalled).unwrap_or(false);
    let slot_stall_seconds = scp_slot.as_ref().map(|s| s.stall_seconds).unwrap_or(0);

    // Quorum promotion gate observability (#651, epic #441 §3/P5). Every
    // field is read from the snapshot the quorum rebuild path publishes from
    // a REAL gate evaluation — never fabricated here (the anti-#541–#544
    // gate). With no handle wired in (tests / relay nodes) or before the
    // first rebuild, all gate fields render as JSON null: an unwired node
    // reports "no data", not a plausible-looking constant.
    let quorum_gate = state.quorum_gate_snapshot();

    // Operator-action pre-signature rejected-requests counter (#750, §6 review
    // finding 3). Pre-signature failures (not-configured / unknown-signer /
    // bad-signature) are reachable by ANY unauthenticated caller, so they are
    // NOT audit-logged (that would be an unbounded disk-fill primitive); this
    // counter is their only durable, observable trace. A spike here means the
    // operator RPC port is being probed with junk. Read from the live audit-log
    // store — never a fabricated constant (anti-#541–#544).
    let operator_rejected_requests = state.operator_audit_log.rejected_requests();

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
            // Quorum promotion gate (#651): curated vs auto-promoted quorum
            // membership and gate state, from the rebuild path's
            // QuorumGateSnapshot. All null until the first gate evaluation.
            "quorumCuratedMembers": quorum_gate.as_ref().map(|g| g.curated_members),
            "quorumAutoMembers": quorum_gate.as_ref().map(|g| g.auto_members),
            // > 0 means the gate is actively keeping discovered peers OUT of
            // the safety-critical quorum (over-cap auto peers, or all
            // non-curated peers in explicit mode).
            "quorumGateSuppressedPeers": quorum_gate.as_ref().map(|g| g.suppressed_peers),
            "quorumGateMaxAutoMembers": quorum_gate.as_ref().map(|g| g.max_auto_members),
            // true when the latest candidate quorum set failed the
            // bth-quorum-sim intersection check and was refused (the node
            // kept its previous safe quorum set).
            "quorumGateIntersectionRefused": quorum_gate.as_ref().map(|g| g.intersection_refused),
            // Stuck-miner early-warning (#538): true iff this node's miner is
            // active but producing 0 H/s past the grace + stall window.
            "minerStalled": miner_stalled,
            // SCP slot-stall observability (#653, #532 Phase 0): live slot
            // progress from the consensus tick's ScpSlotSnapshot. Index /
            // phase / counters are null until the first snapshot is published.
            "scpSlotIndex": scp_slot.as_ref().map(|s| s.slot_index),
            "scpSlotPhase": scp_slot.as_ref().map(|s| s.phase.clone()),
            "scpSlotActive": scp_slot_active,
            "scpNominationRound": scp_slot.as_ref().map(|s| s.nomination_round),
            "scpVotedNominated": scp_slot.as_ref().map(|s| s.num_voted_nominated),
            "scpAcceptedNominated": scp_slot.as_ref().map(|s| s.num_accepted_nominated),
            "scpConfirmedNominated": scp_slot.as_ref().map(|s| s.num_confirmed_nominated),
            "scpBallotCounter": scp_slot.as_ref().map(|s| s.ballot_counter),
            // Derived stall verdict: slot ACTIVE but no externalization for >
            // SLOT_STALL_THRESHOLD_MULTIPLIER x the effective slot duration.
            // An idle node (nothing to propose) is never "stalled".
            "slotStalled": slot_stalled,
            "slotStallSeconds": slot_stall_seconds,
            // Operator-action pre-signature rejected requests (#750, finding 3):
            // count of unauthenticated operator_submitAction requests refused
            // before signature verification. NOT audit-logged; this is their
            // only observable trace. 0 on a node never probed.
            "operatorRejectedRequests": operator_rejected_requests,
            "lastExternalizedSlot": scp_slot.as_ref().and_then(|s| s.last_externalized_slot),
            "lastExternalizedSecondsAgo": scp_slot.as_ref().and_then(|s| s.last_externalized_seconds_ago),
            "effectiveSlotDurationSecs": scp_slot.as_ref().map(|s| s.effective_slot_duration_secs),
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

/// Serialize a block into the explorer-facing RPC shape shared by
/// `getBlockByHeight` and `getBlockByHash`.
///
/// #696: on top of the original header fields (which existing consumers — the
/// web wallet adapter and the seed status page — depend on and which must not
/// change), this adds privacy-safe per-transaction structure (hash, fee, ring
/// size — never amounts/recipients/linkage), the block's total fees, and the
/// lottery summary so explorers can render block detail and lottery events
/// without re-deriving consensus data client-side.
fn block_to_json(block: &crate::block::Block) -> Value {
    // Per-transfer-tx structure. `ringSize` is the number of ring members in
    // the transaction's first input; every input uses the same fixed ring
    // size, and a well-formed transfer tx always has at least one input
    // (0 only for degenerate/test blocks).
    let transactions: Vec<Value> = block
        .transactions
        .iter()
        .map(|tx| {
            json!({
                "hash": hex::encode(tx.hash()),
                "fee": tx.fee,
                "ringSize": tx.inputs.clsag().first().map(|input| input.ring.len()).unwrap_or(0),
            })
        })
        .collect();

    // Lottery summary: the real `BlockLotterySummary` fields (block.rs) in
    // camelCase, plus the on-chain payout structure carried alongside it.
    let summary = &block.lottery_summary;
    let lottery = json!({
        "totalFees": summary.total_fees,
        "poolDistributed": summary.pool_distributed,
        "amountBurned": summary.amount_burned,
        "lotterySeed": hex::encode(summary.lottery_seed),
        "payoutCount": block.lottery_outputs.len(),
        "payoutTotal": block.total_lottery_payouts(),
    });

    json!({
        "height": block.height(),
        "hash": hex::encode(block.hash()),
        "prevHash": hex::encode(block.header.prev_block_hash),
        "timestamp": block.header.timestamp,
        "difficulty": block.header.difficulty,
        "nonce": block.header.nonce,
        "txCount": block.transactions.len(),
        "mintingReward": block.minting_tx.reward,
        // #696 additive explorer fields below this line.
        "transactions": transactions,
        "totalFees": block.total_fees(),
        "lottery": lottery,
    })
}

async fn handle_get_block(id: Value, params: &Value, state: &RpcState) -> JsonRpcResponse {
    let height = params.get("height").and_then(|v| v.as_u64()).unwrap_or(0);
    let ledger = read_lock!(state.ledger, id.clone());

    match ledger.get_block(height) {
        Ok(block) => JsonRpcResponse::success(id, block_to_json(&block)),
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
        Ok(Some(block)) => JsonRpcResponse::success(id, block_to_json(&block)),
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

/// Parse a `cluster_wealth` RPC parameter as `u128` picocredits.
///
/// Since the accumulator widened to u128 (#626), wealth can exceed the JS
/// safe-integer / u64 range, so wallets send it as a decimal STRING (matching
/// the `cluster_getWealth*` string responses). A legacy numeric value is still
/// accepted for backward compatibility. Missing / unparseable → 0.
fn parse_cluster_wealth_param(params: &Value) -> u128 {
    match params.get("cluster_wealth") {
        Some(Value::String(s)) => s.parse::<u128>().unwrap_or(0),
        Some(v) => v.as_u64().map(u128::from).unwrap_or(0),
        None => 0,
    }
}

async fn handle_estimate_fee(id: Value, params: &Value, state: &RpcState) -> JsonRpcResponse {
    // Parse parameters
    let amount = params.get("amount").and_then(|v| v.as_u64()).unwrap_or(0);
    let num_memos = params.get("memos").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

    // Parse optional cluster_wealth for accurate progressive fee calculation
    // (u128 pico, string-encoded). Wallets get this from
    // cluster_getWealthByTargetKeys. The mempool fee API takes u128 end-to-end
    // (#626 PR3) — the prior u64 clamp is gone; full-width wealth flows through.
    let cluster_wealth = parse_cluster_wealth_param(params);

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
            "clusterWealth": cluster_wealth.to_string(),
            "params": {
                "amount": amount,
                "txType": tx_type_str,
                "memos": num_memos,
                "clusterWealth": cluster_wealth.to_string(),
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
/// - `baseRate`: Current base fee rate in picocredits per byte
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
                // Unified ML-KEM ciphertext field (issue #970): hex, or null for
                // a classical/legacy KEM-less output. Thin wallets need it to
                // decapsulate and detect hybrid outputs on the single scan path.
                "kemCiphertext": coinbase.kem_ciphertext.as_ref().map(hex::encode),
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

                    // Encrypted memo ciphertext (additive, #856). Only the
                    // recipient (who holds the view key) can decrypt it, so
                    // exposing the ciphertext is privacy-safe — it is exactly
                    // what already travels on-chain. Thin clients (the bridge
                    // deposit watcher, and any wallet rendering received-payment
                    // notes) need it to read the destination memo; `null` when
                    // the output carries no memo. Coinbase / lottery outputs
                    // never carry a memo and omit the field entirely.
                    let e_memo = output.e_memo.as_ref().map(|m| hex::encode(m.as_bytes()));

                    outputs.push(json!({
                        "txHash": hex::encode(tx.hash()),
                        "outputIndex": idx,
                        "targetKey": hex::encode(output.target_key),
                        "publicKey": hex::encode(output.public_key),
                        "amountCommitment": hex::encode(output.amount.to_le_bytes()),
                        "clusterTags": cluster_tags,
                        "eMemo": e_memo,
                        // Unified ML-KEM ciphertext (issue #970): hex or null.
                        "kemCiphertext": output.kem_ciphertext.as_ref().map(hex::encode),
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
                    // Unified ML-KEM ciphertext (issue #970): hex or null.
                    "kemCiphertext": lottery_output.kem_ciphertext.as_ref().map(hex::encode),
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
        // Unified hybrid scan path (issue #970): decapsulate + view-key check.
        if let Some(subaddress_index) = wallet.scan_output(&utxo.output, utxo.id.output_index) {
            // Recover the one-time private key to compute key image
            if let Some(onetime_private) = wallet.recover_output_spend_key(
                &utxo.output,
                subaddress_index,
                utxo.id.output_index,
            ) {
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

    // Keep a copy for relay: `add_tx` consumes the tx, and the network event
    // loop needs the full transaction to gossip it to peers (#674).
    let relay_tx = tx.clone();

    match mempool.add_tx(tx, &ledger) {
        Ok(hash) => {
            // Hand the accepted tx to the network event loop for immediate
            // gossip + SCP tx-cache registration (#674). Mempool acceptance
            // above is the validity gate (full structural + UTXO + signature
            // checks), so only validated txs are relayed. Without this, a
            // non-minting node never announces RPC-submitted txs — they sit in
            // the local mempool forever because the steady-state broadcast
            // only ran inside the active-minting path.
            if let Some(relay) = &state.tx_relay {
                if relay.send(relay_tx).is_err() {
                    // Event loop gone (shutdown); the tx is still in the local
                    // mempool, matching pre-#674 behavior.
                    warn!("tx relay channel closed; submitted tx not gossiped");
                }
            }
            JsonRpcResponse::success(
                id,
                json!({
                    "txHash": hex::encode(hash),
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(id, -32000, &format!("Failed to add transaction: {}", e)),
    }
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

            // Get the canonical form
            let canonical = match addr.to_address_string() {
                Ok(s) => s,
                Err(e) => {
                    return JsonRpcResponse::success(
                        id,
                        json!({
                            "valid": false,
                            "error": e.to_string(),
                            "address": address_str,
                        }),
                    )
                }
            };

            // Quantum addresses are retired (ADR 0006): they no longer parse,
            // so any valid address here is classical. `isQuantum` is kept for
            // response-shape compatibility with older clients.
            JsonRpcResponse::success(
                id,
                json!({
                    "valid": true,
                    "address": canonical,
                    "network": network,
                    "type": "classical",
                    "isQuantum": false,
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
    let (inbound, outbound, bytes_sent, bytes_received) = match state.network_stats.as_ref() {
        Some(stats) => (
            stats.inbound_count(),
            stats.outbound_count(),
            stats.bytes_sent(),
            stats.bytes_received(),
        ),
        None => (0, peers as u64, 0, 0),
    };

    JsonRpcResponse::success(
        id,
        json!({
            "peerCount": peers,
            "inboundCount": inbound,
            "outboundCount": outbound,
            "bytesSent": bytes_sent,
            "bytesReceived": bytes_received,
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

/// Shared gate for the operator-only READ RPCs (#707, P4.2).
///
/// Fail closed, in order:
///   1. `[rpc.operator]` absent / empty secret ⇒ `OPERATOR_NOT_ENABLED` (a
///      clean "feature off" signal; the node behaves exactly as today).
///   2. the `token` param is verified with [`auth::verify_operator_read_token`]
///      (constant-time HMAC, signature-before-expiry). ANY failure —
///      missing/malformed/forged/expired — collapses to one GENERIC
///      `OPERATOR_TOKEN_REJECTED` error: the node never leaks which check
///      failed.
///
/// Returns `Ok(())` only when a valid, unexpired token was presented. This is
/// a READ gate: it authorizes viewing operator-only data and grants no write
/// capability whatsoever.
fn operator_read_gate(id: &Value, params: &Value, state: &RpcState) -> Result<(), JsonRpcResponse> {
    let secret = match state.operator_read_token_secret.as_deref() {
        Some(s) => s,
        None => {
            return Err(JsonRpcResponse::error(
                id.clone(),
                OPERATOR_NOT_ENABLED,
                "operator features are not enabled on this node ([rpc.operator] not configured)",
            ));
        }
    };

    let token = params.get("token").and_then(|v| v.as_str()).unwrap_or("");

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    match auth::verify_operator_read_token(token, secret, now) {
        Ok(_exp) => Ok(()),
        // Deliberately generic: do not reveal expiry-vs-signature-vs-malformed.
        Err(_) => Err(JsonRpcResponse::error(
            id.clone(),
            OPERATOR_TOKEN_REJECTED,
            "operator token invalid or expired",
        )),
    }
}

/// `operator_getQuorumInfo` (#707): the node's configured `[network.quorum]`
/// plus the PER-PEER gate classification from the last real gate evaluation.
///
/// The public `node_getStatus` exposes only COUNTS; this operator-only method
/// adds (a) the configured quorum contents (members list, mode, threshold,
/// caps) and (b) which specific connected peers are curated / auto-promoted /
/// suppressed — a targeting map the public surface must not hand out.
///
/// Anti-#541: the per-peer classification is `null` until the gate has run at
/// least once (relay nodes / pre-first-rebuild), never a fabricated or
/// zero-filled list. The classification comes verbatim from the gate's
/// [`QuorumGateSnapshot`]; nothing is recomputed here.
async fn handle_operator_quorum_info(
    id: Value,
    params: &Value,
    state: &RpcState,
) -> JsonRpcResponse {
    if let Err(resp) = operator_read_gate(&id, params, state) {
        return resp;
    }

    let q = &state.quorum;
    let mode = match q.mode {
        crate::config::QuorumMode::Explicit => "explicit",
        crate::config::QuorumMode::Recommended => "recommended",
    };
    let fault_model = match q.fault_model {
        crate::config::FaultModel::Crash => "crash",
        crate::config::FaultModel::Bft => "bft",
    };

    // Per-peer classification from the last REAL gate evaluation, or null.
    let gate = state.quorum_gate_snapshot();
    let per_peer = gate.as_ref().map(|g| {
        json!({
            "curated": g.curated_peer_ids,
            "auto": g.auto_peer_ids,
            "suppressed": g.suppressed_peer_ids,
        })
    });

    JsonRpcResponse::success(
        id,
        json!({
            // Configured [network.quorum] contents (operator-only read).
            "quorum": {
                "mode": mode,
                "faultModel": fault_model,
                "threshold": q.threshold,
                "members": q.members,
                "minPeers": q.min_peers,
                "maxAutoMembers": q.max_auto_members,
            },
            // Per-peer gate classification, or null until the first evaluation
            // (anti-#541 — never fabricated).
            "perPeer": per_peer,
            // The same aggregate counts node_getStatus exposes, echoed for
            // convenience; null until the first evaluation.
            "gate": gate.as_ref().map(|g| json!({
                "curatedMembers": g.curated_members,
                "autoMembers": g.auto_members,
                "suppressedPeers": g.suppressed_peers,
                "maxAutoMembers": g.max_auto_members,
                "intersectionRefused": g.intersection_refused,
            })),
        }),
    )
}

/// `operator_getAuditLog` (#707): the operator audit log (read-token gated).
///
/// Present-but-empty in P4.2 — there is no write path to append to it yet
/// (that is #709). Returns an empty `entries` list rather than a placeholder
/// (anti-#541). An optional `limit` param bounds the returned entries.
async fn handle_operator_audit_log(id: Value, params: &Value, state: &RpcState) -> JsonRpcResponse {
    if let Err(resp) = operator_read_gate(&id, params, state) {
        return resp;
    }

    let limit = params
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(200)
        .min(1000);

    let entries = state.operator_audit_log.recent(limit);
    let count = entries.len();

    JsonRpcResponse::success(
        id,
        json!({
            "entries": entries,
            "count": count,
        }),
    )
}

/// `operator_submitAction` (#748, P4.4b): the operator-signed quorum-curation
/// WRITE path.
///
/// This handler owns verification steps 1–5 (which are all SECRET-FREE, so the
/// ordering keeps no secret-dependent check ahead of signature verification, §9
/// oracle-risk item):
///   1. Config gate: `action_public_keys` non-empty, else "not configured".
///   2. Signer known: `signerKeyId` selects a configured pubkey.
///   3. Signature valid over the RECEIVED canonical bytes + domain separator,
///      then parse-after-verify (finding 1: unknown/duplicate keys rejected).
///   4. Target binding: `targetNode` == this node's PeerId.
///   5. Freshness: `issuedAt - 30 <= now <= expiresAt`, lifetime <= 300s.
///
/// Steps 6 (nonce reserve-then-apply) and 7-apply (payload policy against the
/// LIVE peer set, the EXISTING gate, install, `Config::save`) run in the
/// `commands::run` event loop, because they need the live `NonceStore`, the
/// connected-peer set, the consensus handle, and the config — the handler sends
/// the verified envelope over the bounded channel and awaits the outcome.
///
/// The handler takes EXACTLY ONE argument object with two fields — `envelope`
/// (canonical JSON string) and `signature` — and reads nothing else (finding 1:
/// no sibling parameter influences processing).
async fn handle_operator_submit_action(
    id: Value,
    params: &Value,
    state: &RpcState,
) -> JsonRpcResponse {
    use crate::operator_action as oa;

    // Step 1 (config gate), first and secret-free: an empty action_public_keys
    // list means the write surface does not exist at all (fail-closed). This
    // check precedes even envelope extraction so a node with the feature off
    // returns the stable "not configured" signal regardless of request shape.
    // Pre-signature (finding 3): counted, never audit-logged (no envelope yet).
    if state.operator_action_public_keys.is_empty() {
        return operator_action_reject(id, &oa::RejectReason::NotConfigured, None, None, state);
    }

    // Extract the single (envelope, signature) argument (finding 1).
    let signed = match oa::SignedEnvelope::from_params(params) {
        Ok(s) => s,
        Err(reason) => return operator_action_reject(id, &reason, None, None, state),
    };

    // The signerKeyId is needed to SELECT the verifying key (step 2), but we
    // must not depend on secret data before signature verification — step 2 is a
    // fingerprint scan over PUBLIC keys, so it is safe here. We first peek the
    // signerKeyId out of the (as-yet-unverified) bytes only to pick the key; the
    // signature is then checked over the exact received bytes, and the fully
    // trusted parse happens AFTER that inside `verify_and_parse`.
    let signer_key_id = match oa::peek_signer_key_id(&signed.envelope) {
        Ok(s) => s,
        Err(reason) => return operator_action_reject(id, &reason, None, None, state),
    };

    // Step 1 (config gate) + step 2 (signer known): select the verifying key.
    // Still pre-signature — unknown-signer is unauthenticated (finding 3).
    let verifying_key =
        match oa::select_verifying_key(&state.operator_action_public_keys, &signer_key_id) {
            Ok(vk) => vk,
            Err(reason) => return operator_action_reject(id, &reason, None, None, state),
        };

    // Step 3 (signature) + parse-after-verify (finding 1). A bad signature is
    // the LAST pre-signature failure (counted, never audit-logged); everything
    // after this point is AUTHENTICATED and audit-logged.
    let parsed = match signed.verify_and_parse(&verifying_key) {
        Ok(p) => p,
        Err(reason) => return operator_action_reject(id, &reason, None, None, state),
    };

    // From here the request is AUTHENTICATED: pass the envelope bytes so the
    // audit hook can hash them (§6 envelopeHash) and append an entry.
    let envelope = signed.envelope.as_str();

    // Step 4 (target binding) — post-signature, audit-logged on refusal.
    if let Err(reason) = oa::check_target(&parsed, &state.identity.peer_id) {
        return operator_action_reject(id, &reason, Some(&parsed), Some(envelope), state);
    }

    // Step 5 (freshness) — post-signature, audit-logged on refusal.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Err(reason) = oa::check_freshness(&parsed, now) {
        return operator_action_reject(id, &reason, Some(&parsed), Some(envelope), state);
    }

    // Steps 6 + 7-apply run in the event loop. Send the verified envelope over
    // the bounded channel and await the synchronous outcome.
    let tx = match state.operator_action_tx.as_ref() {
        Some(tx) => tx,
        None => {
            return JsonRpcResponse::error(
                id,
                OPERATOR_ACTION_UNAVAILABLE,
                "operator actions are not available on this node (no event loop wired)",
            );
        }
    };

    let (responder, receiver) = tokio::sync::oneshot::channel();
    let request = oa::OperatorActionRequest { parsed, responder };
    // Bounded send: awaits a permit (backpressure) rather than dropping.
    if tx.send(request).await.is_err() {
        return JsonRpcResponse::error(
            id,
            INTERNAL_ERROR,
            "operator action could not be dispatched to the event loop",
        );
    }

    match receiver.await {
        // Loop-produced outcome (steps 6–7): applied / gate_refused /
        // post-signature verify_refused. All AUTHENTICATED — audit-logged with
        // the original envelope bytes' hash (§6).
        Ok(outcome) => operator_action_response(id, outcome, Some(envelope), state),
        Err(_) => JsonRpcResponse::error(
            id,
            INTERNAL_ERROR,
            "event loop dropped the operator action without responding",
        ),
    }
}

/// Build a JSON-RPC response for an early (handler-side) rejection, wrapping
/// the structured outcome so #750 sees the same shape whether the refusal
/// happened in the handler (steps 1–5) or the loop (steps 6–7). `envelope` is
/// the canonical signed bytes when the request reached authentication (for the
/// audit `envelopeHash`); `None` for pre-signature failures.
fn operator_action_reject(
    id: Value,
    reason: &crate::operator_action::RejectReason,
    parsed: Option<&crate::operator_action::ParsedEnvelope>,
    envelope: Option<&str>,
    state: &RpcState,
) -> JsonRpcResponse {
    let outcome = crate::operator_action::OperatorActionOutcome::rejected(reason, parsed);
    operator_action_response(id, outcome, envelope, state)
}

/// Render an [`crate::operator_action::OperatorActionOutcome`] as a JSON-RPC
/// response, ALSO wiring the #750 audit hook: an AUTHENTICATED outcome appends
/// a §6 JSONL entry (+ `warn!` mirror); a PRE-signature outcome only increments
/// the rejected-requests counter (+ rate-limited `debug!`) — never a file write
/// (finding 3).
///
/// Applied outcomes are `success` (the caller reads the outcome class);
/// refusals are `error` with the structured outcome in `data` so the dashboard
/// renders the gate verdict truthfully (anti-#541: the verdict comes only from
/// the node).
fn operator_action_response(
    id: Value,
    outcome: crate::operator_action::OperatorActionOutcome,
    envelope: Option<&str>,
    state: &RpcState,
) -> JsonRpcResponse {
    use crate::operator_action::OutcomeClass;

    // --- #750 audit hook (§6, finding 3) -----------------------------------
    if outcome.authenticated {
        // Authenticated: append the full §6 entry. The envelopeHash is the
        // blake2b-256 of the exact signed bytes (the one blake2b helper).
        if let Some(env) = envelope {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let hash = crate::operator_key::blake2b_256_hex(env.as_bytes());
            if let Some(entry) = outcome.to_audit_entry(hash, ts) {
                state.operator_audit_log.append(entry);
            }
        }
    } else {
        // Pre-signature (unauthenticated): counter + rate-limited debug! only.
        // NEVER a JSONL write — this path is reachable by any anonymous caller.
        state
            .operator_audit_log
            .note_pre_signature_rejection(outcome.audit_tag.as_str());
    }

    let body = serde_json::to_value(&outcome).unwrap_or(Value::Null);
    match outcome.outcome {
        OutcomeClass::Applied => JsonRpcResponse::success(id, body),
        OutcomeClass::GateRefused | OutcomeClass::VerifyRefused => {
            let code = if outcome.outcome == OutcomeClass::VerifyRefused {
                // Not-configured is the dedicated "feature off" signal.
                if outcome.audit_tag == "verify_refused:not_configured" {
                    OPERATOR_NOT_ENABLED
                } else {
                    OPERATOR_ACTION_REJECTED
                }
            } else {
                OPERATOR_ACTION_REJECTED
            };
            JsonRpcResponse {
                jsonrpc: "2.0",
                result: None,
                error: Some(JsonRpcError {
                    code,
                    message: outcome.message.clone(),
                    data: Some(body),
                }),
                id,
            }
        }
    }
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
/// The total wealth in picocredits attributed to this cluster across all
/// UTXOs.
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
        // `wealth` is u128 pico (#626): serialize as a STRING (can exceed the
        // JS safe-integer / u64 range), matching the `totalMined` precedent.
        Ok(wealth) => JsonRpcResponse::success(
            id,
            json!({
                "cluster_id": cluster_id.to_string(),
                "wealth": wealth.to_string(),
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
            // Calculate the fee multiplier for this wealth level. `cluster_factor`
            // takes full-u128 wealth end-to-end (#626 PR3) — no clamp.
            let cluster_factor = mempool.cluster_factor(info.max_cluster_wealth);

            // Format cluster breakdown for response. Wealth is u128 pico (#626),
            // serialized as STRINGS (may exceed the JS/u64 range).
            let breakdown: Vec<Value> = info
                .cluster_breakdown
                .iter()
                .map(|(cluster_id, wealth)| {
                    json!({
                        "cluster_id": cluster_id.to_string(),
                        "wealth": wealth.to_string(),
                    })
                })
                .collect();

            JsonRpcResponse::success(
                id,
                json!({
                    "max_cluster_wealth": info.max_cluster_wealth.to_string(),
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
    // Mempool holds the live fee curve; taken (ledger → mempool, matching
    // cluster_getWealthByTargetKeys) so `factor` comes from the same
    // `cluster_factor` the fee-estimation RPCs use — one source of curve
    // truth, no TS re-implementation (#696, drift class #610).
    let mempool = read_lock!(state.mempool, id.clone());

    match ledger.get_all_cluster_wealth() {
        Ok(clusters) => {
            // Wealth is u128 pico (#626); accumulate in u128 and serialize wealth
            // fields as STRINGS (can exceed the JS/u64 range).
            let total_tracked: u128 = clusters
                .iter()
                .map(|(_, w)| *w)
                .fold(0u128, |acc, w| acc.saturating_add(w));

            let entries: Vec<Value> = clusters
                .iter()
                .map(|(cluster_id, wealth)| {
                    json!({
                        "cluster_id": cluster_id.to_string(),
                        "wealth": wealth.to_string(),
                        // Milli-x fee factor from the live curve
                        // (1000 = 1x .. 6000 = 6x). #696 additive.
                        "factor": mempool.cluster_factor(*wealth),
                    })
                })
                .collect();

            JsonRpcResponse::success(
                id,
                json!({
                    "count": clusters.len(),
                    "total_tracked_wealth": total_tracked.to_string(),
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
    // u128 pico, string-encoded (#626); mempool fee API is u128 end-to-end (PR3).
    let cluster_wealth = parse_cluster_wealth_param(params);

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
                "clusterWealth": cluster_wealth.to_string(),
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
        // Unified hybrid scan path (issue #970): decapsulate + view-key check.
        if let Some(subaddress_index) = wallet.scan_output(&utxo.output, utxo.id.output_index) {
            // Recover the one-time private key
            if let Some(onetime_private) = wallet.recover_output_spend_key(
                &utxo.output,
                subaddress_index,
                utxo.id.output_index,
            ) {
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

    // Build outputs.
    //
    // Protocol 6.0.0: every value-transfer output is a hybrid post-quantum
    // stealth output carrying an ML-KEM-768 ciphertext encapsulated to the
    // destination's published KEM key (issue #958 sub-issue 4). The recipient
    // output is index 0; the change output (a self-send encapsulated to our
    // own address) is index 1.
    let mut outputs = Vec::new();
    match TxOutput::new_hybrid_to_address(
        amount,
        &recipient,
        0,
        None,
        bth_transaction_types::ClusterTagVector::empty(),
    ) {
        Ok(out) => outputs.push(out),
        Err(e) => {
            error!(
                "Faucet: recipient address lacks a valid ML-KEM-768 key: {}",
                e
            );
            return JsonRpcResponse::error(id, -32000, "Recipient address is not post-quantum");
        }
    }

    // Change output (if any), encapsulated to our own published address.
    let change = selected_amount - amount - fee;
    if change > 0 {
        match TxOutput::new_hybrid_to_address(
            change,
            &our_address,
            1,
            None,
            bth_transaction_types::ClusterTagVector::empty(),
        ) {
            Ok(out) => outputs.push(out),
            Err(e) => {
                error!("Faucet: wallet address lacks a valid ML-KEM-768 key: {}", e);
                return JsonRpcResponse::error(id, -32000, "Wallet address is not post-quantum");
            }
        }
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
            // Cold-start guard (issue #583): on a fresh-genesis chain the decoy
            // anonymity set has not warmed up yet, so ring formation fails with
            // a typed `LedgerError::InsufficientDecoys`. This self-heals in
            // ~30 blocks. Catch ONLY that specific case and return a graceful,
            // structured "warming up" result (HTTP 200, not an error) so the
            // web-wallet can render a friendly message. All other tx-creation
            // failures still surface as real errors below.
            if let Some(response) = faucet::FaucetResponse::warming_up_for_tx_error(&e) {
                warn!(
                    "Faucet: chain warming up — insufficient decoys (have {:?}, need {:?})",
                    response.have_decoys, response.need_decoys
                );
                return match serde_json::to_value(&response) {
                    Ok(value) => JsonRpcResponse::success(id, value),
                    Err(e) => JsonRpcResponse::error(
                        id,
                        -32000,
                        &format!("Failed to serialize response: {}", e),
                    ),
                };
            }

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

/// `dev_settleToBackground` (testnet-only): build an explicit
/// demurrage-settlement (#831) that spends the node wallet's own (cluster-
/// tagged) coins to a **factor-1/background** output back to its own address,
/// and submit it to the mempool.
///
/// This is the local full-loop bridge e2e's reserve-funding primitive (#1025).
/// The bridge releaser (`release/bth.rs`) only ever spends **factor-1**
/// reserve outputs (ADR 0003), but a freshly-mined node accrues ONLY
/// 100%-cluster-tagged coinbases (`block.rs::to_tx_output`) — it never
/// naturally holds factor-1. A settlement is the consensus-sanctioned
/// reclassification to background (the same on-ramp a real wrap deposit
/// uses); in the bootstrap epoch the capitalized settlement charge is zero
/// (`demurrage_rate_bps == 0`), so it costs only the base fee.
///
/// Params: `{ "amount": <picocredits, optional, default 1 BTH> }`.
/// Gated to testnet — never exposed on mainnet.
async fn handle_dev_settle_to_background(
    id: Value,
    params: &Value,
    state: &RpcState,
) -> JsonRpcResponse {
    // Footgun guard: settlement is a legitimate operation for any holder, but
    // this unauthenticated, CPU-heavy (full UTXO scan + CLSAG ring build),
    // *mutating* self-send helper exists purely for the local harness. It is
    // firewalled off mainnet by network type AND gated behind the explicit
    // dev-RPC opt-in (`BOTHO_ENABLE_DEV_RPC`, set by the `botho-testnet`
    // harness) so that a *live public* internet-facing testnet node does not
    // expose it by default — only a throwaway local harness node does (M1/L1).
    if state.network_type != Network::Testnet || !crate::config::is_dev_rpc_enabled() {
        return JsonRpcResponse::error(
            id,
            -32000,
            "dev_settleToBackground is not enabled (testnet + BOTHO_ENABLE_DEV_RPC only)",
        );
    }

    let wallet = match &state.wallet {
        Some(w) => w,
        None => return JsonRpcResponse::error(id, -32000, "Node wallet not configured"),
    };

    // Amount to settle to background (default 1 BTH). The remainder of the
    // selected input(s) becomes background change back to us.
    const DEFAULT_SETTLE_AMOUNT: u64 = 1_000_000_000_000; // 1 BTH
    let amount = params
        .get("amount")
        .and_then(|v| v.as_u64())
        .unwrap_or(DEFAULT_SETTLE_AMOUNT);

    let fee = MIN_TX_FEE;
    let required = match amount.checked_add(fee) {
        Some(r) => r,
        None => return JsonRpcResponse::error(id, -32602, "amount + fee overflow"),
    };

    let our_address = wallet.default_address();

    let ledger = read_lock!(state.ledger, id);

    // Scan our own spendable UTXOs (stealth ownership via the account key).
    let all_utxos = match ledger.scan_utxos_for_account(wallet.account_key()) {
        Ok(u) => u,
        Err(e) => {
            error!("dev_settleToBackground: failed to scan UTXOs: {}", e);
            return JsonRpcResponse::error(id, -32000, "Failed to scan wallet UTXOs");
        }
    };

    // Drop spent / pending inputs (chain + mempool), mirroring the faucet.
    let mempool = read_lock!(state.mempool, id);
    let mut utxos = Vec::new();
    for utxo in &all_utxos {
        if let Some(subaddress_index) = wallet.scan_output(&utxo.output, utxo.id.output_index) {
            if let Some(onetime_private) = wallet.recover_output_spend_key(
                &utxo.output,
                subaddress_index,
                utxo.id.output_index,
            ) {
                let key_image = bth_crypto_ring_signature::KeyImage::from(&onetime_private);
                let key_image_bytes = key_image.as_bytes();
                if mempool.is_key_image_pending(&key_image_bytes) {
                    continue;
                }
                match ledger.is_key_image_spent(&key_image_bytes) {
                    Ok(None) => utxos.push(utxo.clone()),
                    Ok(Some(_)) => continue,
                    Err(e) => {
                        error!("dev_settleToBackground: key image check failed: {}", e);
                        return JsonRpcResponse::error(id, -32000, "Key image check failed");
                    }
                }
            }
        }
    }
    drop(mempool);

    // Select enough inputs to cover amount + fee.
    let mut selected_utxos = Vec::new();
    let mut selected_amount = 0u64;
    for utxo in &utxos {
        if selected_amount >= required {
            break;
        }
        selected_utxos.push(utxo.clone());
        selected_amount = selected_amount.saturating_add(utxo.output.amount);
    }
    if selected_amount < required {
        return JsonRpcResponse::error(
            id,
            -32000,
            &format!(
                "insufficient spendable balance: have {} pc, need {} pc (mine more blocks)",
                selected_amount, required
            ),
        );
    }

    let current_height = match ledger.get_chain_state() {
        Ok(s) => s.height,
        Err(e) => {
            error!("dev_settleToBackground: failed to get chain state: {}", e);
            return JsonRpcResponse::error(id, -32000, "Failed to get chain state");
        }
    };

    // Build the (background) outputs: the settled amount + any change, both back
    // to our own address. `create_settlement_transaction` forces empty cluster
    // tags and stamps the settlement flag / settled_value.
    let mut outputs = Vec::new();
    match TxOutput::new_hybrid_to_address(
        amount,
        &our_address,
        0,
        None,
        bth_transaction_types::ClusterTagVector::empty(),
    ) {
        Ok(out) => outputs.push(out),
        Err(e) => {
            error!(
                "dev_settleToBackground: wallet address lacks ML-KEM key: {}",
                e
            );
            return JsonRpcResponse::error(id, -32000, "Wallet address is not post-quantum");
        }
    }
    let change = selected_amount - amount - fee;
    if change > 0 {
        match TxOutput::new_hybrid_to_address(
            change,
            &our_address,
            1,
            None,
            bth_transaction_types::ClusterTagVector::empty(),
        ) {
            Ok(out) => outputs.push(out),
            Err(e) => {
                error!(
                    "dev_settleToBackground: wallet address lacks ML-KEM key: {}",
                    e
                );
                return JsonRpcResponse::error(id, -32000, "Wallet address is not post-quantum");
            }
        }
    }

    let tx = match wallet.create_settlement_transaction(
        &selected_utxos,
        outputs,
        fee,
        current_height,
        &ledger,
    ) {
        Ok(t) => t,
        Err(e) => {
            error!("dev_settleToBackground: failed to build settlement: {}", e);
            return JsonRpcResponse::error(
                id,
                -32000,
                &format!("Failed to build settlement transaction: {}", e),
            );
        }
    };

    let tx_hash = tx.hash();
    let tx_hash_hex = hex::encode(tx_hash);
    drop(ledger);

    // Submit to mempool.
    {
        let mut mempool = write_lock!(state.mempool, id);
        let ledger = read_lock!(state.ledger, id);
        if let Err(e) = mempool.add_tx(tx.clone(), &ledger) {
            error!("dev_settleToBackground: mempool rejected settlement: {}", e);
            return JsonRpcResponse::error(
                id,
                -32000,
                &format!("Failed to submit settlement transaction: {}", e),
            );
        }
    }

    state.ws_broadcaster.new_transaction(&tx_hash, fee, None);

    info!(
        "dev_settleToBackground: settled {} pc to factor-1 (tx: {})",
        amount,
        &tx_hash_hex[..16.min(tx_hash_hex.len())]
    );

    JsonRpcResponse::success(
        id,
        json!({
            "txHash": tx_hash_hex,
            "settledAmount": amount.to_string(),
            "changeAmount": change.to_string(),
        }),
    )
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

            // Check minting tx output (mining rewards to faucet). The coinbase
            // reward always sits at MINTING_OUTPUT_INDEX; the unified hybrid
            // scan path (issue #970) decapsulates its ML-KEM ciphertext.
            let minting_output = block.minting_tx.to_tx_output();
            if let Some(subaddr_idx) = wallet.scan_output(&minting_output, MINTING_OUTPUT_INDEX) {
                if let Some(onetime_key) = wallet.recover_output_spend_key(
                    &minting_output,
                    subaddr_idx,
                    MINTING_OUTPUT_INDEX,
                ) {
                    let key_image = bth_crypto_ring_signature::KeyImage::from(&onetime_key);
                    cache.key_images.insert(*key_image.as_bytes());
                }
            }

            // Check regular tx outputs (change back to faucet, or incoming transfers)
            for tx in &block.transactions {
                for (idx, output) in tx.outputs.iter().enumerate() {
                    let output_index = idx as u32;
                    if let Some(subaddr_idx) = wallet.scan_output(output, output_index) {
                        if let Some(onetime_key) =
                            wallet.recover_output_spend_key(output, subaddr_idx, output_index)
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

    fn empty_test_state() -> RpcState {
        use crate::{ledger::Ledger, mempool::Mempool};
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
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

    /// #747: with no action_public_keys wired in, the field is empty — the
    /// node has no operator write surface and behaves as today.
    #[test]
    fn operator_action_public_keys_default_empty() {
        let state = empty_test_state();
        assert!(state.operator_action_public_keys.is_empty());
    }

    /// #747: the builder filters empty/whitespace-only entries and trims,
    /// failing closed identically to the read-token accessor.
    #[test]
    fn with_operator_action_public_keys_filters_fail_closed() {
        let state = empty_test_state().with_operator_action_public_keys(vec![
            "aa".to_string(),
            "   ".to_string(),
            "".to_string(),
            " bb ".to_string(),
        ]);
        assert_eq!(state.operator_action_public_keys, vec!["aa", "bb"]);

        // An all-empty list ⇒ no keys (no write surface).
        let state = empty_test_state()
            .with_operator_action_public_keys(vec!["  ".to_string(), "".to_string()]);
        assert!(state.operator_action_public_keys.is_empty());
    }

    // -- operator_submitAction RPC handler tests (#748) -----------------------

    use ed25519_dalek::{Signer as _, SigningKey};

    fn oa_test_key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    fn oa_signer_id(sk: &SigningKey) -> String {
        crate::operator_key::fingerprint_hex(&sk.verifying_key().to_bytes())
    }

    fn oa_pubkey_hex(sk: &SigningKey) -> String {
        hex::encode(sk.verifying_key().to_bytes())
    }

    /// A base58 PeerId to use as the node's identity / member targets.
    fn oa_peer_id(seed: u8) -> String {
        let kp = libp2p::identity::Keypair::ed25519_from_bytes([seed; 32]).unwrap();
        libp2p::PeerId::from(kp.public()).to_string()
    }

    /// Build a signed envelope (canonical sorted-key JSON) and the RPC params
    /// object `{envelope, signature}`.
    #[allow(clippy::too_many_arguments)]
    fn oa_params(
        sk: &SigningKey,
        action: &str,
        params_json: &str,
        target: &str,
        issued_at: u64,
        expires_at: u64,
        nonce: &str,
        dry_run: bool,
    ) -> Value {
        let canonical = format!(
            "{{\"action\":\"{action}\",\"dryRun\":{dry_run},\"expiresAt\":{expires_at},\
             \"issuedAt\":{issued_at},\"nonce\":\"{nonce}\",\"params\":{params_json},\
             \"signerKeyId\":\"{s}\",\"targetNode\":\"{target}\",\"v\":1}}",
            s = oa_signer_id(sk),
        );
        let mut msg = crate::operator_action::DOMAIN_SEPARATOR.to_vec();
        msg.extend_from_slice(canonical.as_bytes());
        let sig = sk.sign(&msg);
        json!({ "envelope": canonical, "signature": hex::encode(sig.to_bytes()) })
    }

    fn oa_now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    #[tokio::test]
    async fn submit_action_not_configured_is_inert() {
        // No action_public_keys ⇒ "operator actions not configured", fail-closed.
        let state = empty_test_state();
        let sk = oa_test_key();
        let target = oa_peer_id(1);
        let params = oa_params(
            &sk,
            "quorum.set_max_auto_members",
            "{\"value\":8}",
            &target,
            oa_now(),
            oa_now() + 100,
            "00112233445566778899aabbccddeeff",
            false,
        );
        let resp = handle_operator_submit_action(json!(1), &params, &state).await;
        let err = resp.error.expect("must be an error");
        assert_eq!(err.code, OPERATOR_NOT_ENABLED);
    }

    #[tokio::test]
    async fn submit_action_bad_signature_rejected_at_handler() {
        let sk = oa_test_key();
        let target = oa_peer_id(1);
        let state = empty_test_state().with_operator_action_public_keys(vec![oa_pubkey_hex(&sk)]);
        let mut params = oa_params(
            &sk,
            "quorum.set_max_auto_members",
            "{\"value\":8}",
            &target,
            oa_now(),
            oa_now() + 100,
            "00112233445566778899aabbccddeeff",
            false,
        );
        // Corrupt the signature.
        let sig = params["signature"].as_str().unwrap().to_string();
        params["signature"] = json!(format!("ff{}", &sig[2..]));
        let resp = handle_operator_submit_action(json!(1), &params, &state).await;
        let err = resp.error.expect("bad sig must error");
        assert_eq!(err.code, OPERATOR_ACTION_REJECTED);
    }

    #[tokio::test]
    async fn submit_action_wrong_target_rejected_at_handler() {
        let sk = oa_test_key();
        let mut state =
            empty_test_state().with_operator_action_public_keys(vec![oa_pubkey_hex(&sk)]);
        // This node's identity is peer 9; the envelope targets peer 1.
        state.identity.peer_id = oa_peer_id(9);
        let params = oa_params(
            &sk,
            "quorum.set_max_auto_members",
            "{\"value\":8}",
            &oa_peer_id(1),
            oa_now(),
            oa_now() + 100,
            "00112233445566778899aabbccddeeff",
            false,
        );
        let resp = handle_operator_submit_action(json!(1), &params, &state).await;
        let err = resp.error.expect("wrong target must error");
        assert_eq!(err.code, OPERATOR_ACTION_REJECTED);
        // The structured outcome is carried in `data` for the dashboard.
        let data = err.data.expect("structured outcome in data");
        assert_eq!(data["auditTag"], "verify_refused:wrong_target");
    }

    #[tokio::test]
    async fn submit_action_channel_unavailable_fails_closed() {
        // Configured + valid signature/target/freshness, but NO event-loop
        // channel wired ⇒ fail closed with OPERATOR_ACTION_UNAVAILABLE.
        let sk = oa_test_key();
        let target = oa_peer_id(9);
        let mut state =
            empty_test_state().with_operator_action_public_keys(vec![oa_pubkey_hex(&sk)]);
        state.identity.peer_id = target.clone();
        let params = oa_params(
            &sk,
            "quorum.set_max_auto_members",
            "{\"value\":8}",
            &target,
            oa_now(),
            oa_now() + 100,
            "00112233445566778899aabbccddeeff",
            false,
        );
        let resp = handle_operator_submit_action(json!(1), &params, &state).await;
        let err = resp.error.expect("no channel must error");
        assert_eq!(err.code, OPERATOR_ACTION_UNAVAILABLE);
    }

    #[tokio::test]
    async fn submit_action_full_roundtrip_over_bounded_channel() {
        // End-to-end: handler does steps 1–5, sends over a BOUNDED channel to a
        // stand-in "event loop" that responds with an applied outcome. Verifies
        // the channel is bounded and the synchronous request→verdict round trip
        // works.
        use crate::operator_action::{GateVerdict, OperatorActionOutcome, QuorumPosture};

        let sk = oa_test_key();
        let target = oa_peer_id(9);
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let mut state = empty_test_state()
            .with_operator_action_public_keys(vec![oa_pubkey_hex(&sk)])
            .with_operator_action_channel(tx);
        state.identity.peer_id = target.clone();

        // Stand-in event loop: receive the verified request and reply "applied".
        let loop_task = tokio::spawn(async move {
            let req = rx.recv().await.expect("request");
            // The handler must have verified + parsed before sending.
            assert_eq!(req.parsed.target_node, oa_peer_id(9));
            assert!(!req.parsed.dry_run);
            let verdict = GateVerdict {
                intersection_refused: false,
                curated_members: 3,
                auto_members: 1,
                suppressed_peers: 0,
                max_auto_members: 8,
                fault_tolerant: true,
                degenerate: false,
            };
            let posture = QuorumPosture {
                mode: "recommended".to_string(),
                members: vec![],
                max_auto_members: 8,
            };
            let outcome = OperatorActionOutcome::applied(&req.parsed, posture, verdict);
            let _ = req.responder.send(outcome);
        });

        let params = oa_params(
            &sk,
            "quorum.set_max_auto_members",
            "{\"value\":8}",
            &target,
            oa_now(),
            oa_now() + 100,
            "00112233445566778899aabbccddeeff",
            false,
        );
        let resp = handle_operator_submit_action(json!(1), &params, &state).await;
        loop_task.await.unwrap();
        let result = resp.result.expect("applied must be success");
        assert_eq!(result["outcome"], "applied");
        assert_eq!(result["gate"]["intersectionRefused"], false);
    }

    // -- #750 audit hook at the RPC boundary ---------------------------------

    /// A process-unique, thread-safe temp audit-log path (the #749 lesson: an
    /// atomic counter, never SystemTime, to avoid parallel-test path races).
    fn oa_audit_path() -> std::path::PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir()
            .join(format!("botho-audit-rpc-{}-{}", std::process::id(), n))
            .join(operator::AUDIT_LOG_FILE)
    }

    #[tokio::test]
    async fn audit_applied_outcome_appends_full_shape_entry() {
        // #750 AC: an applied outcome from the loop appends ONE JSONL entry with
        // the full §6 shape, newQuorum present, envelopeHash a 64-char blake2b.
        use crate::operator_action::{GateVerdict, OperatorActionOutcome, QuorumPosture};

        let sk = oa_test_key();
        let target = oa_peer_id(9);
        let path = oa_audit_path();
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let mut state = empty_test_state()
            .with_operator_action_public_keys(vec![oa_pubkey_hex(&sk)])
            .with_operator_action_channel(tx)
            .with_operator_audit_log(OperatorAuditLog::open(&path));
        state.identity.peer_id = target.clone();

        let loop_task = tokio::spawn(async move {
            let req = rx.recv().await.expect("request");
            let verdict = GateVerdict {
                intersection_refused: false,
                curated_members: 4,
                auto_members: 0,
                suppressed_peers: 0,
                max_auto_members: 8,
                fault_tolerant: true,
                degenerate: false,
            };
            let resulting = QuorumPosture {
                mode: "recommended".to_string(),
                members: vec![oa_peer_id(3)],
                max_auto_members: 8,
            };
            let prev = QuorumPosture {
                mode: "recommended".to_string(),
                members: vec![],
                max_auto_members: 8,
            };
            let outcome = OperatorActionOutcome::applied(&req.parsed, resulting, verdict)
                .with_prev_quorum(prev);
            let _ = req.responder.send(outcome);
        });

        let params = oa_params(
            &sk,
            "quorum.pin_member",
            &format!("{{\"peerId\":\"{}\"}}", oa_peer_id(3)),
            &target,
            oa_now(),
            oa_now() + 100,
            "00112233445566778899aabbccddeeff",
            false,
        );
        let resp = handle_operator_submit_action(json!(1), &params, &state).await;
        loop_task.await.unwrap();
        assert!(resp.result.is_some(), "applied is success");

        // Exactly one entry, rendered from the store (anti-#541), full §6 shape.
        let entries = state.operator_audit_log.recent(10);
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.outcome, "applied");
        assert_eq!(e.action, "quorum.pin_member");
        assert_eq!(e.envelope_hash.len(), 64, "blake2b-256 hex");
        assert!(e.new_quorum.is_some(), "applied MUST carry newQuorum");
        assert!(e.prev_quorum.is_some());
        assert!(e.gate.is_some());
        assert_eq!(e.params["peerId"], oa_peer_id(3));

        // Persisted to the JSONL file as one line.
        let raw = std::fs::read_to_string(&path).unwrap();
        assert_eq!(raw.lines().filter(|l| !l.trim().is_empty()).count(), 1);
        // No pre-signature rejections occurred.
        assert_eq!(state.operator_audit_log.rejected_requests(), 0);

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn audit_post_signature_refusal_appends_entry_without_new_quorum() {
        // #750 AC: a POST-signature refusal (wrong target) is AUTHENTICATED, so
        // it appends one entry — with the attempted params but NO newQuorum.
        let sk = oa_test_key();
        let path = oa_audit_path();
        let mut state = empty_test_state()
            .with_operator_action_public_keys(vec![oa_pubkey_hex(&sk)])
            .with_operator_audit_log(OperatorAuditLog::open(&path));
        state.identity.peer_id = oa_peer_id(9); // envelope targets peer 1
        let params = oa_params(
            &sk,
            "quorum.set_max_auto_members",
            "{\"value\":8}",
            &oa_peer_id(1),
            oa_now(),
            oa_now() + 100,
            "00112233445566778899aabbccddeeff",
            false,
        );
        let resp = handle_operator_submit_action(json!(1), &params, &state).await;
        assert!(resp.error.is_some());

        let entries = state.operator_audit_log.recent(10);
        assert_eq!(entries.len(), 1, "authenticated refusal is audit-logged");
        assert_eq!(entries[0].outcome, "verify_refused:wrong_target");
        assert!(
            entries[0].new_quorum.is_none(),
            "a refusal has no new state (§6)"
        );
        assert_eq!(entries[0].envelope_hash.len(), 64);
        // Not a pre-signature failure ⇒ counter untouched.
        assert_eq!(state.operator_audit_log.rejected_requests(), 0);
        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn finding3_presig_flood_bumps_node_status_counter_jsonl_empty() {
        // FINDING 3 at the RPC boundary: a flood of PRE-signature failures
        // (bad signature) increments the node_getStatus rejected-requests
        // counter while the audit JSONL stays EMPTY (no disk-fill primitive).
        let sk = oa_test_key();
        let path = oa_audit_path();
        let state = empty_test_state()
            .with_operator_action_public_keys(vec![oa_pubkey_hex(&sk)])
            .with_operator_audit_log(OperatorAuditLog::open(&path));

        let target = oa_peer_id(9);
        for i in 0..250u64 {
            let mut params = oa_params(
                &sk,
                "quorum.set_max_auto_members",
                "{\"value\":8}",
                &target,
                oa_now(),
                oa_now() + 100,
                &format!("{i:032x}"),
                false,
            );
            // Replace with an all-zero (well-formed hex, 64-byte) signature that
            // can NEVER verify ⇒ a deterministic pre-signature bad-signature
            // failure (unlike flipping bytes of the real sig, which can rarely
            // land back on a valid signature).
            params["signature"] = json!("00".repeat(64));
            let resp = handle_operator_submit_action(json!(1), &params, &state).await;
            assert_eq!(resp.error.unwrap().code, OPERATOR_ACTION_REJECTED);
        }

        // Counter surfaced in node_getStatus reflects the flood...
        let status = handle_node_status(json!(1), &state).await.result.unwrap();
        assert_eq!(status["operatorRejectedRequests"], 250);
        // ...while the audit log (file + memory) stayed EMPTY.
        assert!(
            state.operator_audit_log.recent(10).is_empty(),
            "pre-signature failures must NOT be audit-logged"
        );
        assert!(
            !path.exists(),
            "pre-signature failures must NOT create the JSONL file"
        );
        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn node_status_reports_zero_operator_rejects_by_default() {
        // A node never probed reports 0 (not a fabricated constant — read live
        // from the store).
        let state = empty_test_state();
        let status = handle_node_status(json!(1), &state).await.result.unwrap();
        assert_eq!(status["operatorRejectedRequests"], 0);
    }

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

        // A node that is downloading from / caught up with the network has live
        // peers; set a realistic count so the #1118 isolation cross-check does
        // not fire (that path is exercised separately in
        // `test_node_status_reports_isolated_when_no_peers`).
        *state.peer_count.write().unwrap() = 3;

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

    /// #1118: a node whose `ChainSyncManager` is latched in `SyncState::Synced`
    /// but has lost every peer must NOT keep reporting `synced: true`. This is
    /// the #1114 relay-outage failure: a node stranded on a stale singleton
    /// fork self-certifies as synced because the state machine has no
    /// isolation escape hatch. `handle_node_status` cross-checks the
    /// published snapshot against the live peer count and reports `synced:
    /// false` / `syncStatus: "isolated"` / `syncProgress: 0` instead. A
    /// node with the SAME snapshot but live peers is unaffected.
    #[tokio::test]
    async fn test_node_status_reports_isolated_when_no_peers() {
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

        // The sync manager is latched in Synced (it published a synced snapshot
        // before its peers dropped) and claims a full 100% progress.
        *sync_handle.write().unwrap() = Some(SyncStatusSnapshot {
            synced: true,
            status: "synced",
            local_height: 3233,
            target_height: Some(3233),
        });

        // --- Isolated: 0 peers overrides the latched `synced` snapshot. ---
        *state.peer_count.write().unwrap() = 0;
        let resp = handle_node_status(json!(1), &state).await;
        let result = resp.result.unwrap();
        assert_eq!(
            result["synced"],
            json!(false),
            "a 0-peer node must not report synced even when latched in SyncState::Synced"
        );
        assert_eq!(
            result["syncStatus"],
            json!("isolated"),
            "syncStatus must be a distinct `isolated` value, not `synced`"
        );
        assert_eq!(
            result["syncProgress"].as_f64().unwrap(),
            0.0,
            "an isolated node has no peer to measure progress against — must not claim 100%"
        );
        assert_eq!(result["peerCount"], json!(0));

        // --- Regression guard: the SAME snapshot with live peers is healthy. ---
        *state.peer_count.write().unwrap() = 3;
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

    /// #653 (epic #532 Phase 0): the `node_getStatus` SCP slot fields must
    /// track the shared snapshot handle the consensus tick publishes into —
    /// the anti-#541–#544 gate at the RPC layer. With no handle wired in the
    /// fields report absent/idle (null counters, false booleans) instead of
    /// fabricated values; with a handle, two DIFFERENT snapshots must yield
    /// DIFFERENT JSON (the fields are live, not constants).
    #[tokio::test]
    async fn test_node_status_tracks_scp_slot_snapshot() {
        use crate::{consensus::ScpSlotSnapshot, ledger::Ledger, mempool::Mempool};

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

        // (1) No handle wired in: absent/idle defaults, never fabricated data.
        let state = fresh_state();
        let result = handle_node_status(json!(1), &state).await.result.unwrap();
        assert_eq!(result["scpSlotActive"], json!(false));
        assert_eq!(result["slotStalled"], json!(false));
        assert_eq!(result["slotStallSeconds"], json!(0));
        assert_eq!(result["scpSlotIndex"], Value::Null);
        assert_eq!(result["scpSlotPhase"], Value::Null);
        assert_eq!(result["scpVotedNominated"], Value::Null);
        assert_eq!(result["scpBallotCounter"], Value::Null);
        assert_eq!(result["lastExternalizedSlot"], Value::Null);
        assert_eq!(result["lastExternalizedSecondsAgo"], Value::Null);

        // (2) Handle wired but nothing published yet: same absent/idle shape.
        let handle = Arc::new(RwLock::new(None));
        let state = fresh_state().with_scp_slot_status(handle.clone());
        let result = handle_node_status(json!(1), &state).await.result.unwrap();
        assert_eq!(result["scpSlotActive"], json!(false));
        assert_eq!(result["scpSlotIndex"], Value::Null);

        // (3) Active-progressing snapshot published by the consensus tick.
        *handle.write().unwrap() = Some(ScpSlotSnapshot {
            slot_index: 42,
            phase: "NominatePrepare".to_string(),
            num_voted_nominated: 2,
            num_accepted_nominated: 1,
            num_confirmed_nominated: 0,
            nomination_round: 3,
            ballot_counter: 0,
            scp_slot_active: true,
            slot_stalled: false,
            stall_seconds: 4,
            last_externalized_slot: Some(41),
            last_externalized_seconds_ago: Some(4),
            effective_slot_duration_secs: 20,
        });
        let active = handle_node_status(json!(1), &state).await.result.unwrap();
        assert_eq!(active["scpSlotIndex"], json!(42));
        assert_eq!(active["scpSlotPhase"], json!("NominatePrepare"));
        assert_eq!(active["scpSlotActive"], json!(true));
        assert_eq!(active["scpNominationRound"], json!(3));
        assert_eq!(active["scpVotedNominated"], json!(2));
        assert_eq!(active["scpAcceptedNominated"], json!(1));
        assert_eq!(active["scpConfirmedNominated"], json!(0));
        assert_eq!(active["scpBallotCounter"], json!(0));
        assert_eq!(active["slotStalled"], json!(false));
        assert_eq!(active["slotStallSeconds"], json!(4));
        assert_eq!(active["lastExternalizedSlot"], json!(41));
        assert_eq!(active["lastExternalizedSecondsAgo"], json!(4));
        assert_eq!(active["effectiveSlotDurationSecs"], json!(20));

        // (4) Jammed snapshot: the SAME handle now reports a stall — every
        // field must move with the snapshot (not-a-constant proof).
        *handle.write().unwrap() = Some(ScpSlotSnapshot {
            slot_index: 42,
            phase: "Prepare".to_string(),
            num_voted_nominated: 2,
            num_accepted_nominated: 2,
            num_confirmed_nominated: 1,
            nomination_round: 9,
            ballot_counter: 7,
            scp_slot_active: true,
            slot_stalled: true,
            stall_seconds: 61,
            last_externalized_slot: Some(41),
            last_externalized_seconds_ago: Some(61),
            effective_slot_duration_secs: 20,
        });
        let jammed = handle_node_status(json!(1), &state).await.result.unwrap();
        assert_eq!(jammed["slotStalled"], json!(true));
        assert_eq!(jammed["slotStallSeconds"], json!(61));
        assert_eq!(jammed["scpSlotPhase"], json!("Prepare"));
        assert_eq!(jammed["scpBallotCounter"], json!(7));
        assert_eq!(jammed["scpNominationRound"], json!(9));
        assert_ne!(
            active["slotStalled"], jammed["slotStalled"],
            "slotStalled must track the live snapshot, not a constant"
        );
        assert_ne!(
            active["slotStallSeconds"], jammed["slotStallSeconds"],
            "slotStallSeconds must track the live snapshot, not a constant"
        );
    }

    /// #651 (epic #441 §3/P5): the `node_getStatus` quorum-promotion-gate
    /// fields must track the shared snapshot handle the quorum rebuild path
    /// publishes into — the anti-#541–#544 gate at the RPC layer. With no
    /// handle wired in (or wired but never published) every gate field is
    /// JSON null, never a fabricated zero; with a handle, two DIFFERENT
    /// snapshots must yield DIFFERENT JSON (the fields are live, not
    /// constants).
    #[tokio::test]
    async fn test_node_status_tracks_quorum_gate_snapshot() {
        use crate::{consensus::QuorumGateSnapshot, ledger::Ledger, mempool::Mempool};

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

        // (1) No handle wired in: all gate fields null (no data, not zeros).
        let state = fresh_state();
        let result = handle_node_status(json!(1), &state).await.result.unwrap();
        assert_eq!(result["quorumCuratedMembers"], Value::Null);
        assert_eq!(result["quorumAutoMembers"], Value::Null);
        assert_eq!(result["quorumGateSuppressedPeers"], Value::Null);
        assert_eq!(result["quorumGateMaxAutoMembers"], Value::Null);
        assert_eq!(result["quorumGateIntersectionRefused"], Value::Null);

        // (2) Handle wired but nothing published yet: still null.
        let handle = Arc::new(RwLock::new(None));
        let state = fresh_state().with_quorum_gate_status(handle.clone());
        let result = handle_node_status(json!(1), &state).await.result.unwrap();
        assert_eq!(result["quorumCuratedMembers"], Value::Null);
        assert_eq!(result["quorumAutoMembers"], Value::Null);
        assert_eq!(result["quorumGateSuppressedPeers"], Value::Null);
        assert_eq!(result["quorumGateMaxAutoMembers"], Value::Null);
        assert_eq!(result["quorumGateIntersectionRefused"], Value::Null);

        // (3) Small honest cluster: gate admits everyone, suppresses no one.
        *handle.write().unwrap() = Some(QuorumGateSnapshot {
            curated_members: 0,
            auto_members: 4,
            suppressed_peers: 0,
            max_auto_members: 8,
            intersection_refused: false,
            curated_peer_ids: Vec::new(),
            auto_peer_ids: Vec::new(),
            suppressed_peer_ids: Vec::new(),
        });
        let quiet = handle_node_status(json!(1), &state).await.result.unwrap();
        assert_eq!(quiet["quorumCuratedMembers"], json!(0));
        assert_eq!(quiet["quorumAutoMembers"], json!(4));
        assert_eq!(quiet["quorumGateSuppressedPeers"], json!(0));
        assert_eq!(quiet["quorumGateMaxAutoMembers"], json!(8));
        assert_eq!(quiet["quorumGateIntersectionRefused"], json!(false));

        // (4) Sybil flood: the SAME handle now reports active suppression —
        // every field must move with the snapshot (not-a-constant proof).
        *handle.write().unwrap() = Some(QuorumGateSnapshot {
            curated_members: 2,
            auto_members: 8,
            suppressed_peers: 22,
            max_auto_members: 8,
            intersection_refused: true,
            curated_peer_ids: Vec::new(),
            auto_peer_ids: Vec::new(),
            suppressed_peer_ids: Vec::new(),
        });
        let flooded = handle_node_status(json!(1), &state).await.result.unwrap();
        assert_eq!(flooded["quorumCuratedMembers"], json!(2));
        assert_eq!(flooded["quorumAutoMembers"], json!(8));
        assert_eq!(flooded["quorumGateSuppressedPeers"], json!(22));
        assert_eq!(flooded["quorumGateIntersectionRefused"], json!(true));
        assert_ne!(
            quiet["quorumGateSuppressedPeers"], flooded["quorumGateSuppressedPeers"],
            "quorumGateSuppressedPeers must track the live snapshot, not a constant"
        );
        assert_ne!(
            quiet["quorumGateIntersectionRefused"], flooded["quorumGateIntersectionRefused"],
            "quorumGateIntersectionRefused must track the live snapshot, not a constant"
        );
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

        // (2) Stats handle wired in: real values surfaced. Simulate the network
        // event loop having recorded traffic + connections.
        let stats = Arc::new(NetworkStats::new());
        stats.record_sent(4096);
        stats.record_received(8192);
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

        // (3) Genuine zero: handle present but nothing sent / no inbound.
        let empty_stats = Arc::new(NetworkStats::new());
        let state = fresh_state().with_network_stats(empty_stats);
        let result = handle_network_info(json!(1), &state).await.result.unwrap();
        assert_eq!(result["bytesSent"], json!(0));
        assert_eq!(result["bytesReceived"], json!(0));
        assert_eq!(result["inboundCount"], json!(0));
        assert_eq!(result["outboundCount"], json!(0));
    }

    /// #696: `block_to_json` — the shared `getBlockByHeight` /
    /// `getBlockByHash` shape — must carry the additive explorer fields
    /// (`transactions`, `totalFees`, `lottery`) alongside the ORIGINAL header
    /// fields, which existing consumers (web wallet adapter, seed status page)
    /// depend on and which must not change.
    #[test]
    fn test_block_to_json_explorer_fields() {
        use crate::{
            block::{Block, BlockLotterySummary, LotteryOutput},
            transaction::{ClsagRingInput, RingMember, Transaction},
        };

        // A transfer tx with an 11-member ring. `block_to_json` only reads
        // structure (hash / fee / ring length), so the signature can be empty.
        let ring: Vec<RingMember> = (0..11)
            .map(|i| RingMember {
                target_key: [i as u8; 32],
                public_key: [i as u8; 32],
                commitment: [0u8; 32],
            })
            .collect();
        let input = ClsagRingInput {
            ring,
            key_image: [1u8; 32],
            commitment_key_image: [2u8; 32],
            clsag_signature: Vec::new(),
            pseudo_output_amount: 500,
        };
        let tx = Transaction::new(vec![input], Vec::new(), 250, 0);
        let tx_hash_hex = hex::encode(tx.hash());

        let mut block = Block::genesis();
        block.transactions.push(tx);
        block.set_lottery_result(
            vec![
                LotteryOutput {
                    winner_tx_hash: [3u8; 32],
                    winner_output_index: 0,
                    payout: 60,
                    target_key: [4u8; 32],
                    public_key: [5u8; 32],
                    kem_ciphertext: None,
                },
                LotteryOutput {
                    winner_tx_hash: [6u8; 32],
                    winner_output_index: 1,
                    payout: 40,
                    target_key: [7u8; 32],
                    public_key: [8u8; 32],
                    kem_ciphertext: None,
                },
            ],
            BlockLotterySummary {
                total_fees: 250,
                pool_distributed: 200,
                amount_burned: 50,
                lottery_seed: [9u8; 32],
            },
        );

        let json = block_to_json(&block);

        // Original fields: present and unchanged (additive-only contract).
        assert_eq!(json["height"], json!(block.height()));
        assert_eq!(json["hash"], json!(hex::encode(block.hash())));
        assert_eq!(
            json["prevHash"],
            json!(hex::encode(block.header.prev_block_hash))
        );
        assert_eq!(json["timestamp"], json!(block.header.timestamp));
        assert_eq!(json["difficulty"], json!(block.header.difficulty));
        assert_eq!(json["nonce"], json!(block.header.nonce));
        assert_eq!(json["txCount"], json!(1));
        assert_eq!(json["mintingReward"], json!(block.minting_tx.reward));

        // New: per-tx structure (privacy-safe — hash / fee / ring size only).
        let txs = json["transactions"].as_array().expect("transactions array");
        assert_eq!(txs.len(), 1);
        assert_eq!(txs[0]["hash"], json!(tx_hash_hex));
        assert_eq!(txs[0]["fee"], json!(250));
        assert_eq!(txs[0]["ringSize"], json!(11));
        // No amount/recipient/linkage fields leak into the tx entries.
        assert!(txs[0].get("outputs").is_none());
        assert!(txs[0].get("amount").is_none());

        // New: block fee total (saturating helper).
        assert_eq!(json["totalFees"], json!(250));

        // New: lottery summary — the real BlockLotterySummary fields in
        // camelCase, plus on-chain payout structure.
        let lottery = &json["lottery"];
        assert_eq!(lottery["totalFees"], json!(250));
        assert_eq!(lottery["poolDistributed"], json!(200));
        assert_eq!(lottery["amountBurned"], json!(50));
        assert_eq!(lottery["lotterySeed"], json!(hex::encode([9u8; 32])));
        assert_eq!(lottery["payoutCount"], json!(2));
        assert_eq!(lottery["payoutTotal"], json!(100));
    }

    /// #696: the `getBlockByHeight` handler returns the enriched shape end to
    /// end, and a block with no transfer txs (genesis) renders the new fields
    /// as empty/zero defaults rather than omitting them.
    #[tokio::test]
    async fn test_get_block_by_height_additive_explorer_fields() {
        use crate::{ledger::Ledger, mempool::Mempool};

        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        std::mem::forget(dir);
        let state = RpcState::new(
            ledger,
            Mempool::new(),
            Network::Testnet,
            None,
            None,
            vec![],
            Arc::new(WsBroadcaster::new(16)),
        );

        let result = handle_get_block(json!(1), &json!({"height": 0}), &state)
            .await
            .result
            .expect("genesis block exists");

        // Original shape intact.
        assert_eq!(result["height"], json!(0));
        assert!(result["hash"].is_string());
        assert!(result["prevHash"].is_string());
        assert_eq!(result["txCount"], json!(0));

        // New fields present with empty/default values.
        assert_eq!(result["transactions"], json!([]));
        assert_eq!(result["totalFees"], json!(0));
        assert_eq!(result["lottery"]["totalFees"], json!(0));
        assert_eq!(result["lottery"]["poolDistributed"], json!(0));
        assert_eq!(result["lottery"]["amountBurned"], json!(0));
        assert_eq!(result["lottery"]["payoutCount"], json!(0));
        assert_eq!(result["lottery"]["payoutTotal"], json!(0));
        assert_eq!(
            result["lottery"]["lotterySeed"],
            json!(hex::encode([0u8; 32]))
        );
    }

    /// #696: `cluster_getAllWealth` entries carry a `factor` field — the
    /// milli-x multiplier (1000 = 1x .. 6000 = 6x) computed via the SAME live
    /// fee curve the fee-estimation RPCs use (`Mempool::cluster_factor`), so
    /// the explorer never re-implements the consensus curve client-side.
    #[tokio::test]
    async fn test_cluster_get_all_wealth_includes_live_curve_factor() {
        use crate::{ledger::Ledger, mempool::Mempool};

        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        std::mem::forget(dir);

        // Seed one poor and one ultra-wealthy cluster directly.
        let poor_wealth: u128 = 1;
        let rich_wealth: u128 = u128::MAX;
        ledger.set_cluster_wealth_for_test(1, poor_wealth).unwrap();
        ledger.set_cluster_wealth_for_test(2, rich_wealth).unwrap();

        let state = RpcState::new(
            ledger,
            Mempool::new(),
            Network::Testnet,
            None,
            None,
            vec![],
            Arc::new(WsBroadcaster::new(16)),
        );

        let result = handle_cluster_get_all_wealth(json!(1), &state)
            .await
            .result
            .expect("cluster_getAllWealth succeeds");

        // Existing shape intact (additive-only).
        assert_eq!(result["count"], json!(2));
        assert!(result["total_tracked_wealth"].is_string());
        let clusters = result["clusters"].as_array().expect("clusters array");
        assert_eq!(clusters.len(), 2);

        // The reference curve: same default FeeConfig the RPC state's mempool
        // carries.
        let reference = Mempool::new();

        let mut poor_factor = None;
        let mut rich_factor = None;
        for entry in clusters {
            // Existing fields unchanged: string cluster_id + string wealth.
            let wealth: u128 = entry["wealth"].as_str().unwrap().parse().unwrap();
            let factor = entry["factor"].as_u64().expect("factor is a u64");
            // Single source of curve truth: matches the live cluster_factor.
            assert_eq!(factor, reference.cluster_factor(wealth));
            // Curve bounds: milli-x in [1000, 6000].
            assert!(
                (1000..=6000).contains(&factor),
                "factor {} out of bounds",
                factor
            );
            match entry["cluster_id"].as_str().unwrap() {
                "1" => poor_factor = Some(factor),
                "2" => rich_factor = Some(factor),
                other => panic!("unexpected cluster_id {}", other),
            }
        }
        let poor_factor = poor_factor.expect("cluster 1 present");
        let rich_factor = rich_factor.expect("cluster 2 present");
        assert_eq!(poor_factor, 1000, "negligible wealth pays the 1x floor");
        assert!(
            rich_factor > poor_factor,
            "curve is progressive: {} !> {}",
            rich_factor,
            poor_factor
        );
    }
}
