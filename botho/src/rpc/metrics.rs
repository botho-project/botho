//! Observability endpoints for Botho node
//!
//! Provides health, readiness, and Prometheus metrics endpoints for
//! production monitoring and load balancer integration.
//!
//! ## Metrics Exported
//!
//! ### Application Metrics
//! - `botho_peers_connected` - Number of connected peers (gauge)
//! - `botho_mempool_size` - Transactions in mempool (gauge)
//! - `botho_block_height` - Current block height (gauge)
//! - `botho_tps` - Transactions per second, 5-minute average (gauge)
//! - `botho_validation_latency_seconds` - Transaction validation latency
//!   (histogram)
//! - `botho_consensus_round_duration_seconds` - SCP consensus round duration
//!   (histogram)
//! - `botho_consensus_nominations_total` - Total SCP nominations (counter)
//! - `botho_scp_slot_stalled` - Current SCP slot stalled: active but not
//!   externalizing (gauge, 0/1)
//! - `botho_scp_slot_stall_seconds` - Seconds the active SCP slot has gone
//!   without externalizing, 0 while idle (gauge)
//! - `botho_gossip_failed_messages_total{kind}` - Gossipsub messages a peer
//!   failed to receive in time, from `SlowPeer` events; `kind` is `priority` or
//!   `non_priority` (counter, #1545)
//! - `botho_gossip_publish_errors_total{topic}` - Synchronous gossipsub
//!   `publish()` failures, e.g. `AllQueuesFull` (counter, #1545)
//! - `botho_node_loop_iteration_seconds` - Duration of one `'node_loop` select
//!   iteration including its handler (histogram, #1545)
//! - `botho_node_loop_delayed_total` - `'node_loop` iterations that took at
//!   least `NODE_LOOP_DELAY_WARN_THRESHOLD` (counter, #1545)
//!
//! ### System Metrics
//! - `botho_data_dir_usage_bytes` - Bytes used by the data directory (gauge)
//! - `botho_ledger_map_size_bytes` - LMDB map size of the ledger (gauge)
//! - `botho_ledger_used_bytes` - High-water mark of the ledger data file
//!   (gauge)
//! - `botho_ledger_map_full` - 1 once a block write hit MDB_MAP_FULL (gauge)
//!
//! ### Process Metrics (Linux only)
//! The following metrics are only available on Linux via the Prometheus process
//! collector:
//! - `process_resident_memory_bytes` - Process resident set size
//! - `process_virtual_memory_bytes` - Process virtual memory size
//! - `process_cpu_seconds_total` - Total CPU time used
//! - `process_open_fds` - Number of open file descriptors
//! - `process_start_time_seconds` - Process start time
//!
//! ## Usage
//!
//! ```bash
//! # Start node with metrics enabled
//! botho run --metrics-port 9090
//!
//! # Scrape metrics
//! curl http://localhost:9090/metrics
//! ```

use http_body_util::Full;
use hyper::{body::Bytes, server::conn::http1, service::service_fn, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use lazy_static::lazy_static;
#[cfg(all(feature = "process", target_os = "linux"))]
use prometheus::process_collector::ProcessCollector;
use prometheus::{
    CounterVec, Encoder, Gauge, Histogram, HistogramOpts, IntCounter, IntCounterVec, IntGauge,
    Opts, Registry, TextEncoder,
};
use serde::Serialize;
use std::{
    convert::Infallible,
    net::SocketAddr,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::net::TcpListener;
use tracing::{error, info, warn};

use super::RpcState;

// ============================================================================
// Per-instance NodeMetrics (for RpcState)
// ============================================================================

/// Prometheus metrics for the Botho node (per RpcState instance)
pub struct NodeMetrics {
    registry: Registry,
    /// Current blockchain height
    pub block_height: Gauge,
    /// Number of connected peers
    pub peer_count: Gauge,
    /// Number of transactions in mempool
    pub mempool_size: Gauge,
    /// Total RPC requests by method
    pub rpc_requests_total: CounterVec,
    /// Total RPC errors by method
    pub rpc_errors_total: CounterVec,
}

impl NodeMetrics {
    /// Create a new metrics registry with all metrics registered
    pub fn new() -> Self {
        let registry = Registry::new();

        let block_height =
            Gauge::with_opts(Opts::new("botho_block_height", "Current blockchain height"))
                .expect("metric can be created");

        let peer_count =
            Gauge::with_opts(Opts::new("botho_peer_count", "Number of connected peers"))
                .expect("metric can be created");

        let mempool_size = Gauge::with_opts(Opts::new(
            "botho_mempool_size",
            "Number of transactions in mempool",
        ))
        .expect("metric can be created");

        let rpc_requests_total = CounterVec::new(
            Opts::new("botho_rpc_requests_total", "Total RPC requests"),
            &["method"],
        )
        .expect("metric can be created");

        let rpc_errors_total = CounterVec::new(
            Opts::new("botho_rpc_errors_total", "Total RPC errors"),
            &["method"],
        )
        .expect("metric can be created");

        // Register all metrics
        registry
            .register(Box::new(block_height.clone()))
            .expect("collector can be registered");
        registry
            .register(Box::new(peer_count.clone()))
            .expect("collector can be registered");
        registry
            .register(Box::new(mempool_size.clone()))
            .expect("collector can be registered");
        registry
            .register(Box::new(rpc_requests_total.clone()))
            .expect("collector can be registered");
        registry
            .register(Box::new(rpc_errors_total.clone()))
            .expect("collector can be registered");

        Self {
            registry,
            block_height,
            peer_count,
            mempool_size,
            rpc_requests_total,
            rpc_errors_total,
        }
    }

    /// Record an RPC request
    pub fn record_request(&self, method: &str) {
        self.rpc_requests_total.with_label_values(&[method]).inc();
    }

    /// Record an RPC error
    pub fn record_error(&self, method: &str) {
        self.rpc_errors_total.with_label_values(&[method]).inc();
    }

    /// Update metrics from current state
    pub fn update_from_state(&self, state: &RpcState) {
        // Update block height
        if let Ok(ledger) = state.ledger.read() {
            if let Ok(chain_state) = ledger.get_chain_state() {
                self.block_height.set(chain_state.height as f64);
            }
        }

        // Update peer count
        if let Ok(peers) = state.peer_count.read() {
            self.peer_count.set(*peers as f64);
        }

        // Update mempool size
        if let Ok(mempool) = state.mempool.read() {
            self.mempool_size.set(mempool.len() as f64);
        }
    }

    /// Encode metrics in Prometheus text format
    pub fn encode(&self) -> Result<String, prometheus::Error> {
        let encoder = TextEncoder::new();
        let metric_families = self.registry.gather();
        let mut buffer = Vec::new();
        encoder.encode(&metric_families, &mut buffer)?;
        Ok(String::from_utf8(buffer).unwrap_or_default())
    }
}

impl Default for NodeMetrics {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Health & Readiness Checks
// ============================================================================

/// Health status of the node
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

impl HealthStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            HealthStatus::Healthy => "healthy",
            HealthStatus::Degraded => "degraded",
            HealthStatus::Unhealthy => "unhealthy",
        }
    }
}

/// Health check response
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: HealthStatus,
    pub uptime_seconds: u64,
}

/// Readiness check response
#[derive(Debug, Serialize)]
pub struct ReadyResponse {
    pub status: &'static str,
    pub synced: bool,
    pub peers: usize,
    pub block_height: u64,
}

/// Check the health of the node
///
/// Returns a simple health status indicating if the node is alive.
/// This endpoint is used by load balancers and Kubernetes liveness probes.
pub fn check_health(state: &RpcState) -> HealthResponse {
    HealthResponse {
        status: HealthStatus::Healthy,
        uptime_seconds: state.start_time.elapsed().as_secs(),
    }
}

/// Check if the node is ready to accept requests
///
/// Returns readiness status with sync state, peer count, and block height.
/// This is used by load balancers and Kubernetes readiness probes.
///
/// The node is considered ready if:
/// - It has at least one block (synced)
/// - It has at least one peer connected
pub fn check_ready(state: &RpcState) -> ReadyResponse {
    let mut block_height = 0u64;
    let mut peers = 0usize;

    // Get block height
    if let Ok(ledger) = state.ledger.read() {
        if let Ok(chain_state) = ledger.get_chain_state() {
            block_height = chain_state.height;
        }
    }

    // Get peer count
    if let Ok(peer_count) = state.peer_count.read() {
        peers = *peer_count;
    }

    // Consider synced if we have any blocks
    // In production, this could compare against known network height
    let synced = block_height > 0;

    // Ready if synced and has peers
    let is_ready = synced && peers > 0;
    let status = if is_ready { "ready" } else { "not_ready" };

    ReadyResponse {
        status,
        synced,
        peers,
        block_height,
    }
}

// ============================================================================
// Global Prometheus Metrics (for standalone metrics server)
// ============================================================================

lazy_static! {
    /// Global Prometheus registry for all metrics.
    pub static ref REGISTRY: Registry = Registry::new();

    // ============================================================================
    // Gauges (current values)
    // ============================================================================

    /// Number of connected peers.
    pub static ref PEERS_CONNECTED: IntGauge = IntGauge::new(
        "botho_peers_connected",
        "Number of connected peers"
    ).expect("Failed to create peers_connected metric");

    /// Number of transactions in the mempool.
    pub static ref MEMPOOL_SIZE_GLOBAL: IntGauge = IntGauge::new(
        "botho_mempool_size",
        "Number of transactions in the mempool"
    ).expect("Failed to create mempool_size metric");

    /// Current block height.
    pub static ref BLOCK_HEIGHT_GLOBAL: IntGauge = IntGauge::new(
        "botho_block_height",
        "Current block height"
    ).expect("Failed to create block_height metric");

    /// Transactions per second (5-minute rolling average).
    pub static ref TPS: prometheus::Gauge = prometheus::Gauge::new(
        "botho_tps",
        "Transactions per second (5-minute rolling average)"
    ).expect("Failed to create tps metric");

    /// Current mining difficulty.
    pub static ref DIFFICULTY: IntGauge = IntGauge::new(
        "botho_difficulty",
        "Current mining difficulty"
    ).expect("Failed to create difficulty metric");

    /// Total minted supply (in atomic units).
    pub static ref TOTAL_MINTED: IntGauge = IntGauge::new(
        "botho_total_minted",
        "Total minted supply in atomic units"
    ).expect("Failed to create total_minted metric");

    /// Total fees burned (in atomic units).
    pub static ref TOTAL_FEES_BURNED: IntGauge = IntGauge::new(
        "botho_total_fees_burned",
        "Total fees burned in atomic units"
    ).expect("Failed to create total_fees_burned metric");

    /// Whether minting is active (1) or not (0).
    pub static ref MINTING_ACTIVE: IntGauge = IntGauge::new(
        "botho_minting_active",
        "Whether minting is active (1) or not (0)"
    ).expect("Failed to create minting_active metric");

    /// Whether the current SCP slot is stalled (1) or not (0) — active
    /// nomination/ballot state with no externalization past the stall
    /// threshold (#653, epic #532 Phase 0). Updated from the live
    /// `ScpSlotSnapshot` on every consensus tick.
    pub static ref SCP_SLOT_STALLED: IntGauge = IntGauge::new(
        "botho_scp_slot_stalled",
        "Whether the current SCP slot is stalled: active but not externalizing (1) or not (0)"
    ).expect("Failed to create scp_slot_stalled metric");

    /// Seconds the current ACTIVE SCP slot has gone without an
    /// externalization (0 while idle). Advances while a round stays jammed,
    /// so operators can quantify a stall, not just detect it (#653).
    pub static ref SCP_SLOT_STALL_SECONDS: IntGauge = IntGauge::new(
        "botho_scp_slot_stall_seconds",
        "Seconds the active SCP slot has gone without externalizing (0 while idle)"
    ).expect("Failed to create scp_slot_stall_seconds metric");

    /// Gossip messages dropped by the per-peer rate limiter, by message type
    /// (#1547). Replaces the per-message `warn!` as the flood signal.
    pub static ref GOSSIP_RATE_LIMITED: prometheus::IntCounterVec = prometheus::IntCounterVec::new(
        Opts::new(
            "botho_gossip_rate_limited_total",
            "Gossip messages dropped by the per-peer rate limiter, by message type"
        ),
        &["message_type"]
    ).expect("Failed to create gossip_rate_limited metric");

    // ============================================================================
    // System Metrics
    // ============================================================================

    /// Total bytes used by the node's data directory.
    pub static ref DATA_DIR_USAGE_BYTES: IntGauge = IntGauge::new(
        "botho_data_dir_usage_bytes",
        "Total bytes used by the node's data directory"
    ).expect("Failed to create data_dir_usage_bytes metric");

    /// LMDB map size of the ledger (#1535): the most `data.mdb` can hold.
    pub static ref LEDGER_MAP_SIZE_BYTES: IntGauge = IntGauge::new(
        "botho_ledger_map_size_bytes",
        "LMDB map size of the ledger: the most the ledger data file can hold"
    ).expect("Failed to create ledger_map_size_bytes metric");

    /// High-water mark of the ledger's LMDB data file (#1535). Writes fail
    /// with MDB_MAP_FULL once this reaches `botho_ledger_map_size_bytes`.
    pub static ref LEDGER_USED_BYTES: IntGauge = IntGauge::new(
        "botho_ledger_used_bytes",
        "High-water mark of the ledger LMDB data file; writes fail once it reaches the map size"
    ).expect("Failed to create ledger_used_bytes metric");

    /// 1 once a block write has failed with MDB_MAP_FULL (#1535): the node
    /// cannot store blocks until restarted with a larger map.
    pub static ref LEDGER_MAP_FULL: IntGauge = IntGauge::new(
        "botho_ledger_map_full",
        "Whether a ledger block write has failed with MDB_MAP_FULL since startup (1) or not (0)"
    ).expect("Failed to create ledger_map_full metric");

    // ============================================================================
    // Histograms (latency measurements)
    // ============================================================================

    /// Transaction validation latency in seconds.
    pub static ref VALIDATION_LATENCY: Histogram = Histogram::with_opts(
        HistogramOpts::new(
            "botho_validation_latency_seconds",
            "Transaction validation latency in seconds"
        ).buckets(vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5])
    ).expect("Failed to create validation_latency metric");

    /// SCP consensus round duration in seconds.
    pub static ref CONSENSUS_ROUND_DURATION: Histogram = Histogram::with_opts(
        HistogramOpts::new(
            "botho_consensus_round_duration_seconds",
            "SCP consensus round duration in seconds"
        ).buckets(vec![0.1, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0])
    ).expect("Failed to create consensus_round_duration metric");

    /// Block processing latency in seconds.
    pub static ref BLOCK_PROCESSING_LATENCY: Histogram = Histogram::with_opts(
        HistogramOpts::new(
            "botho_block_processing_seconds",
            "Block processing latency in seconds"
        ).buckets(vec![0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0])
    ).expect("Failed to create block_processing metric");

    // ============================================================================
    // Counters (cumulative values)
    // ============================================================================

    /// Total SCP nominations sent.
    pub static ref CONSENSUS_NOMINATIONS: IntCounter = IntCounter::new(
        "botho_consensus_nominations_total",
        "Total SCP nominations sent"
    ).expect("Failed to create consensus_nominations metric");

    /// Total transactions processed.
    pub static ref TRANSACTIONS_PROCESSED: IntCounter = IntCounter::new(
        "botho_transactions_processed_total",
        "Total transactions processed"
    ).expect("Failed to create transactions_processed metric");

    /// Total blocks processed.
    pub static ref BLOCKS_PROCESSED: IntCounter = IntCounter::new(
        "botho_blocks_processed_total",
        "Total blocks processed"
    ).expect("Failed to create blocks_processed metric");

    /// Total validation failures.
    pub static ref VALIDATION_FAILURES: IntCounter = IntCounter::new(
        "botho_validation_failures_total",
        "Total transaction validation failures"
    ).expect("Failed to create validation_failures metric");

    // ============================================================================
    // Gossip delivery and event-loop health (#1545)
    // ============================================================================

    /// Gossipsub messages that a peer failed to receive in time, as reported
    /// by `gossipsub::Event::SlowPeer` at each heartbeat. A non-zero
    /// `non_priority` count means queued publishes (blocks, txs, SCP) expired
    /// in the connection handler before they were sent; in w12 this lost a
    /// validator's Commit/Externalize and halted consensus (#1544/#1545).
    pub static ref GOSSIP_FAILED_MESSAGES: IntCounterVec = IntCounterVec::new(
        Opts::new(
            "botho_gossip_failed_messages_total",
            "Gossipsub messages a slow peer failed to receive in time, by queue kind"
        ),
        &["kind"]
    ).expect("Failed to create gossip_failed_messages metric");

    /// Synchronous gossipsub `publish()` failures (`AllQueuesFull`,
    /// `NoPeersSubscribedToTopic`, ...), labelled by topic.
    pub static ref GOSSIP_PUBLISH_ERRORS: IntCounterVec = IntCounterVec::new(
        Opts::new(
            "botho_gossip_publish_errors_total",
            "Gossipsub publish() calls that returned an error, by topic"
        ),
        &["topic"]
    ).expect("Failed to create gossip_publish_errors metric");

    /// Duration of one `'node_loop` select iteration, including its handler.
    /// Idle iterations are bounded by the loop's shortest tick, so the upper
    /// buckets only fill when a handler blocks the loop.
    pub static ref NODE_LOOP_ITERATION_SECONDS: Histogram = Histogram::with_opts(
        HistogramOpts::new(
            "botho_node_loop_iteration_seconds",
            "Duration of one node event-loop iteration including its handler"
        ).buckets(vec![0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0])
    ).expect("Failed to create node_loop_iteration_seconds metric");

    /// `'node_loop` iterations that took at least
    /// [`NODE_LOOP_DELAY_WARN_THRESHOLD`] (each also logs `node_loop_delayed`).
    pub static ref NODE_LOOP_DELAYED: IntCounter = IntCounter::new(
        "botho_node_loop_delayed_total",
        "Node event-loop iterations that exceeded the delay warning threshold"
    ).expect("Failed to create node_loop_delayed metric");
}

/// Kind label for [`GOSSIP_FAILED_MESSAGES`]: priority-queue failures.
pub const GOSSIP_FAILED_KIND_PRIORITY: &str = "priority";
/// Kind label for [`GOSSIP_FAILED_MESSAGES`]: non-priority-queue failures
/// (publishes and forwards that expired before being sent).
pub const GOSSIP_FAILED_KIND_NON_PRIORITY: &str = "non_priority";

/// Record the per-peer failed-message counts from a gossipsub `SlowPeer`
/// event.
pub fn record_gossip_failed_messages(priority: usize, non_priority: usize) {
    if priority > 0 {
        GOSSIP_FAILED_MESSAGES
            .with_label_values(&[GOSSIP_FAILED_KIND_PRIORITY])
            .inc_by(priority as u64);
    }
    if non_priority > 0 {
        GOSSIP_FAILED_MESSAGES
            .with_label_values(&[GOSSIP_FAILED_KIND_NON_PRIORITY])
            .inc_by(non_priority as u64);
    }
}

/// Record one failed gossipsub `publish()` on `topic`.
pub fn record_gossip_publish_error(topic: &str) {
    GOSSIP_PUBLISH_ERRORS.with_label_values(&[topic]).inc();
}

/// A `'node_loop` iteration at least this long logs `node_loop_delayed` at
/// `warn!` and increments [`NODE_LOOP_DELAYED`]. Gossipsub abandons queued
/// publishes after `GOSSIPSUB_PUBLISH_QUEUE_DURATION`; a loop that stops
/// polling the swarm for this long is already starving outbound gossip.
pub const NODE_LOOP_DELAY_WARN_THRESHOLD: Duration = Duration::from_secs(5);

/// Record one `'node_loop` iteration of length `elapsed`. Returns `true` when
/// the iteration crossed `threshold` (and was logged and counted as delayed).
pub fn record_node_loop_iteration(elapsed: Duration, threshold: Duration) -> bool {
    NODE_LOOP_ITERATION_SECONDS.observe(elapsed.as_secs_f64());
    if elapsed >= threshold {
        NODE_LOOP_DELAYED.inc();
        warn!(
            event = "node_loop_delayed",
            elapsed_ms = elapsed.as_millis() as u64,
            "Node event loop iteration delayed; swarm was not polled"
        );
        true
    } else {
        false
    }
}

/// RAII timer for one `'node_loop` select iteration including its handler.
///
/// Create it as the first statement of the loop body; dropping it (at the end
/// of the iteration, or on `continue`/`break`) records the elapsed time via
/// [`record_node_loop_iteration`]. Always on: not gated behind any feature.
pub struct NodeLoopTimer {
    start: Instant,
    threshold: Duration,
}

impl NodeLoopTimer {
    /// Start timing with the default [`NODE_LOOP_DELAY_WARN_THRESHOLD`].
    pub fn start() -> Self {
        Self::with_threshold(NODE_LOOP_DELAY_WARN_THRESHOLD)
    }

    /// Start timing with a custom delay threshold (used by tests).
    pub fn with_threshold(threshold: Duration) -> Self {
        Self {
            start: Instant::now(),
            threshold,
        }
    }
}

impl Drop for NodeLoopTimer {
    fn drop(&mut self) {
        record_node_loop_iteration(self.start.elapsed(), self.threshold);
    }
}

/// Initialize all metrics and register them with the global registry.
///
/// This function should be called once at node startup.
pub fn init_metrics() {
    // Register gauges
    REGISTRY
        .register(Box::new(PEERS_CONNECTED.clone()))
        .expect("Failed to register peers_connected");
    REGISTRY
        .register(Box::new(MEMPOOL_SIZE_GLOBAL.clone()))
        .expect("Failed to register mempool_size");
    REGISTRY
        .register(Box::new(BLOCK_HEIGHT_GLOBAL.clone()))
        .expect("Failed to register block_height");
    REGISTRY
        .register(Box::new(TPS.clone()))
        .expect("Failed to register tps");
    REGISTRY
        .register(Box::new(DIFFICULTY.clone()))
        .expect("Failed to register difficulty");
    REGISTRY
        .register(Box::new(TOTAL_MINTED.clone()))
        .expect("Failed to register total_minted");
    REGISTRY
        .register(Box::new(TOTAL_FEES_BURNED.clone()))
        .expect("Failed to register total_fees_burned");
    REGISTRY
        .register(Box::new(MINTING_ACTIVE.clone()))
        .expect("Failed to register minting_active");
    REGISTRY
        .register(Box::new(SCP_SLOT_STALLED.clone()))
        .expect("Failed to register scp_slot_stalled");
    REGISTRY
        .register(Box::new(SCP_SLOT_STALL_SECONDS.clone()))
        .expect("Failed to register scp_slot_stall_seconds");
    REGISTRY
        .register(Box::new(GOSSIP_RATE_LIMITED.clone()))
        .expect("Failed to register gossip_rate_limited");

    // Register histograms
    REGISTRY
        .register(Box::new(VALIDATION_LATENCY.clone()))
        .expect("Failed to register validation_latency");
    REGISTRY
        .register(Box::new(CONSENSUS_ROUND_DURATION.clone()))
        .expect("Failed to register consensus_round_duration");
    REGISTRY
        .register(Box::new(BLOCK_PROCESSING_LATENCY.clone()))
        .expect("Failed to register block_processing_latency");

    // Register counters
    REGISTRY
        .register(Box::new(CONSENSUS_NOMINATIONS.clone()))
        .expect("Failed to register consensus_nominations");
    REGISTRY
        .register(Box::new(TRANSACTIONS_PROCESSED.clone()))
        .expect("Failed to register transactions_processed");
    REGISTRY
        .register(Box::new(BLOCKS_PROCESSED.clone()))
        .expect("Failed to register blocks_processed");
    REGISTRY
        .register(Box::new(VALIDATION_FAILURES.clone()))
        .expect("Failed to register validation_failures");

    // Register gossip delivery and event-loop health metrics (#1545)
    register_loop_and_gossip_metrics(&REGISTRY);

    // Register system metrics
    REGISTRY
        .register(Box::new(DATA_DIR_USAGE_BYTES.clone()))
        .expect("Failed to register data_dir_usage_bytes");
    REGISTRY
        .register(Box::new(LEDGER_MAP_SIZE_BYTES.clone()))
        .expect("Failed to register ledger_map_size_bytes");
    REGISTRY
        .register(Box::new(LEDGER_USED_BYTES.clone()))
        .expect("Failed to register ledger_used_bytes");
    REGISTRY
        .register(Box::new(LEDGER_MAP_FULL.clone()))
        .expect("Failed to register ledger_map_full");

    // Register process collector for memory metrics (Linux only)
    // This provides: process_resident_memory_bytes, process_virtual_memory_bytes,
    // process_cpu_seconds_total, process_open_fds, process_start_time_seconds
    #[cfg(all(feature = "process", target_os = "linux"))]
    {
        let process_collector = ProcessCollector::for_self();
        REGISTRY
            .register(Box::new(process_collector))
            .expect("Failed to register process collector");
    }

    // Register privacy layer metrics
    crate::network::privacy::register_privacy_metrics(&REGISTRY);

    info!("Prometheus metrics initialized");
}

/// Register the gossip-delivery and node-loop metrics (#1545) with `registry`.
fn register_loop_and_gossip_metrics(registry: &Registry) {
    registry
        .register(Box::new(GOSSIP_FAILED_MESSAGES.clone()))
        .expect("Failed to register gossip_failed_messages");
    registry
        .register(Box::new(GOSSIP_PUBLISH_ERRORS.clone()))
        .expect("Failed to register gossip_publish_errors");
    registry
        .register(Box::new(NODE_LOOP_ITERATION_SECONDS.clone()))
        .expect("Failed to register node_loop_iteration_seconds");
    registry
        .register(Box::new(NODE_LOOP_DELAYED.clone()))
        .expect("Failed to register node_loop_delayed");
}

/// Start the Prometheus metrics HTTP server.
///
/// This spawns a simple HTTP server that responds to GET requests on `/metrics`
/// with Prometheus-formatted metrics data.
pub async fn start_metrics_server(addr: SocketAddr) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    info!(
        "Prometheus metrics server listening on http://{}/metrics",
        addr
    );

    loop {
        let (stream, _) = listener.accept().await?;
        let io = TokioIo::new(stream);

        tokio::spawn(async move {
            let service = service_fn(handle_metrics_request);

            if let Err(err) = http1::Builder::new().serve_connection(io, service).await {
                error!("Error serving metrics connection: {:?}", err);
            }
        });
    }
}

/// Handle HTTP requests to the metrics endpoint.
async fn handle_metrics_request(
    req: Request<hyper::body::Incoming>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    // Only respond to GET /metrics
    if req.method() != hyper::Method::GET {
        return Ok(Response::builder()
            .status(StatusCode::METHOD_NOT_ALLOWED)
            .body(Full::new(Bytes::from("Method not allowed")))
            .unwrap());
    }

    let path = req.uri().path();
    if path != "/metrics" && path != "/" {
        return Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Full::new(Bytes::from("Not found. Try /metrics")))
            .unwrap());
    }

    // Gather and encode metrics
    let encoder = TextEncoder::new();
    let metric_families = REGISTRY.gather();
    let mut buffer = Vec::new();

    if let Err(e) = encoder.encode(&metric_families, &mut buffer) {
        error!("Failed to encode metrics: {}", e);
        return Ok(Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Full::new(Bytes::from(format!(
                "Failed to encode metrics: {}",
                e
            ))))
            .unwrap());
    }

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", encoder.format_type())
        .body(Full::new(Bytes::from(buffer)))
        .unwrap())
}

/// Shared metrics state that can be updated from various components.
///
/// This provides a convenient interface for updating metrics from
/// components that don't want to use the global statics directly.
#[derive(Clone)]
pub struct MetricsUpdater {
    _inner: Arc<()>, // Placeholder for potential future state
}

impl Default for MetricsUpdater {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsUpdater {
    /// Create a new metrics updater.
    pub fn new() -> Self {
        Self {
            _inner: Arc::new(()),
        }
    }

    /// Update the peer count metric.
    pub fn set_peer_count(&self, count: usize) {
        PEERS_CONNECTED.set(count as i64);
    }

    /// Update the mempool size metric.
    pub fn set_mempool_size(&self, size: usize) {
        MEMPOOL_SIZE_GLOBAL.set(size as i64);
    }

    /// Update the block height metric.
    pub fn set_block_height(&self, height: u64) {
        BLOCK_HEIGHT_GLOBAL.set(height as i64);
    }

    /// Update the TPS metric.
    pub fn set_tps(&self, tps: f64) {
        TPS.set(tps);
    }

    /// Update the difficulty metric.
    pub fn set_difficulty(&self, difficulty: u64) {
        DIFFICULTY.set(difficulty as i64);
    }

    /// Update the total minted metric.
    ///
    /// `total` is a u128 picocredit accumulator (#333); the Prometheus gauge
    /// is `i64`, so the stored sample saturates at `i64::MAX`. This is a
    /// display-only metric — exact accounting lives in chain state.
    pub fn set_total_minted(&self, total: u128) {
        TOTAL_MINTED.set(total.min(i64::MAX as u128) as i64);
    }

    /// Update the total fees burned metric. Saturates to `i64::MAX` like
    /// `set_total_minted` (display-only metric).
    pub fn set_total_fees_burned(&self, total: u128) {
        TOTAL_FEES_BURNED.set(total.min(i64::MAX as u128) as i64);
    }

    /// Update the minting active metric.
    pub fn set_minting_active(&self, active: bool) {
        MINTING_ACTIVE.set(if active { 1 } else { 0 });
    }

    /// Update the SCP slot-stalled metric (#653). Called from the consensus
    /// tick with the live `ScpSlotSnapshot.slot_stalled` verdict.
    pub fn set_scp_slot_stalled(&self, stalled: bool) {
        SCP_SLOT_STALLED.set(if stalled { 1 } else { 0 });
    }

    /// Update the SCP slot stall-duration metric (#653). Called from the
    /// consensus tick with the live `ScpSlotSnapshot.stall_seconds`.
    pub fn set_scp_slot_stall_seconds(&self, seconds: u64) {
        SCP_SLOT_STALL_SECONDS.set(seconds.min(i64::MAX as u64) as i64);
    }

    /// Record a validation latency observation.
    pub fn observe_validation_latency(&self, seconds: f64) {
        VALIDATION_LATENCY.observe(seconds);
    }

    /// Record a consensus round duration observation.
    pub fn observe_consensus_round(&self, seconds: f64) {
        CONSENSUS_ROUND_DURATION.observe(seconds);
    }

    /// Record a block processing latency observation.
    pub fn observe_block_processing(&self, seconds: f64) {
        BLOCK_PROCESSING_LATENCY.observe(seconds);
    }

    /// Increment the consensus nominations counter.
    pub fn inc_consensus_nominations(&self) {
        CONSENSUS_NOMINATIONS.inc();
    }

    /// Increment the transactions processed counter.
    pub fn inc_transactions_processed(&self) {
        TRANSACTIONS_PROCESSED.inc();
    }

    /// Add to the transactions processed counter.
    pub fn add_transactions_processed(&self, count: u64) {
        TRANSACTIONS_PROCESSED.inc_by(count);
    }

    /// Increment the blocks processed counter.
    pub fn inc_blocks_processed(&self) {
        BLOCKS_PROCESSED.inc();
    }

    /// Increment the validation failures counter.
    pub fn inc_validation_failures(&self) {
        VALIDATION_FAILURES.inc();
    }

    /// Update the data directory usage metric.
    pub fn set_data_dir_usage(&self, bytes: u64) {
        DATA_DIR_USAGE_BYTES.set(bytes as i64);
    }

    /// Update the ledger LMDB map-usage metrics (#1535).
    pub fn set_ledger_storage(&self, usage: &crate::ledger::LedgerStorageUsage) {
        LEDGER_MAP_SIZE_BYTES.set(usage.map_size_bytes.min(i64::MAX as u64) as i64);
        LEDGER_USED_BYTES.set(usage.used_bytes.min(i64::MAX as u64) as i64);
        LEDGER_MAP_FULL.set(i64::from(usage.map_full));
    }
}

/// Calculate the total size of a directory recursively.
///
/// Returns the total bytes used by all files in the directory tree.
/// Symbolic links are not followed to avoid counting the same files twice.
pub fn calculate_dir_size(path: &Path) -> std::io::Result<u64> {
    let mut total = 0u64;

    if !path.is_dir() {
        return Ok(0);
    }

    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;

        if metadata.is_file() {
            total += metadata.len();
        } else if metadata.is_dir() {
            // Recursively calculate subdirectory size
            total += calculate_dir_size(&entry.path())?;
        }
        // Symbolic links are intentionally skipped
    }

    Ok(total)
}

/// Prometheus label for a gossip message type.
pub fn gossip_message_type_label(message_type: bth_gossip::GossipMessageType) -> &'static str {
    use bth_gossip::GossipMessageType as T;
    match message_type {
        T::Transaction => "transaction",
        T::Block => "block",
        T::Consensus => "consensus",
        T::Announcement => "announcement",
        T::PeerExchange => "peer_exchange",
        T::Other => "other",
    }
}

/// Count one gossip message dropped by the rate limiter (#1547).
pub fn record_gossip_rate_limited(message_type: bth_gossip::GossipMessageType) {
    GOSSIP_RATE_LIMITED
        .with_label_values(&[gossip_message_type_label(message_type)])
        .inc();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_creation() {
        let metrics = NodeMetrics::new();

        // Record some requests
        metrics.record_request("node_getStatus");
        metrics.record_request("node_getStatus");
        metrics.record_request("getChainInfo");
        metrics.record_error("invalid_method");

        // Encode and verify output contains our metrics
        let output = metrics.encode().unwrap();
        assert!(output.contains("botho_block_height"));
        assert!(output.contains("botho_peer_count"));
        assert!(output.contains("botho_mempool_size"));
        assert!(output.contains("botho_rpc_requests_total"));
        assert!(output.contains("node_getStatus"));
    }

    #[test]
    fn test_health_status_serialization() {
        assert_eq!(HealthStatus::Healthy.as_str(), "healthy");
        assert_eq!(HealthStatus::Degraded.as_str(), "degraded");
        assert_eq!(HealthStatus::Unhealthy.as_str(), "unhealthy");
    }

    #[test]
    fn test_health_response_format() {
        let response = HealthResponse {
            status: HealthStatus::Healthy,
            uptime_seconds: 12345,
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"status\":\"healthy\""));
        assert!(json.contains("\"uptime_seconds\":12345"));
    }

    #[test]
    fn test_ready_response_format_ready() {
        let response = ReadyResponse {
            status: "ready",
            synced: true,
            peers: 5,
            block_height: 12345,
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"status\":\"ready\""));
        assert!(json.contains("\"synced\":true"));
        assert!(json.contains("\"peers\":5"));
        assert!(json.contains("\"block_height\":12345"));
    }

    #[test]
    fn test_ready_response_format_not_ready() {
        let response = ReadyResponse {
            status: "not_ready",
            synced: false,
            peers: 0,
            block_height: 0,
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"status\":\"not_ready\""));
        assert!(json.contains("\"synced\":false"));
        assert!(json.contains("\"peers\":0"));
        assert!(json.contains("\"block_height\":0"));
    }

    #[test]
    fn test_metrics_registration() {
        // This test verifies that metrics can be created without panicking
        // We can't easily test the full registration in a unit test because
        // the global registry can only register each metric once
        assert!(PEERS_CONNECTED.get() >= 0);
        assert!(MEMPOOL_SIZE_GLOBAL.get() >= 0);
        assert!(BLOCK_HEIGHT_GLOBAL.get() >= 0);
    }

    #[test]
    fn test_metrics_updater() {
        let updater = MetricsUpdater::new();

        updater.set_peer_count(5);
        assert_eq!(PEERS_CONNECTED.get(), 5);

        updater.set_mempool_size(100);
        assert_eq!(MEMPOOL_SIZE_GLOBAL.get(), 100);

        updater.set_block_height(12345);
        assert_eq!(BLOCK_HEIGHT_GLOBAL.get(), 12345);

        updater.set_minting_active(true);
        assert_eq!(MINTING_ACTIVE.get(), 1);

        updater.set_minting_active(false);
        assert_eq!(MINTING_ACTIVE.get(), 0);
    }

    /// #653: the SCP slot-stall gauges must track the values the consensus
    /// tick pushes — both directions (set and clear), so a stall showing on
    /// the dashboard is live state, not a latched constant.
    #[test]
    fn test_scp_slot_stall_gauges() {
        let updater = MetricsUpdater::new();

        updater.set_scp_slot_stalled(true);
        updater.set_scp_slot_stall_seconds(61);
        assert_eq!(SCP_SLOT_STALLED.get(), 1);
        assert_eq!(SCP_SLOT_STALL_SECONDS.get(), 61);

        updater.set_scp_slot_stalled(false);
        updater.set_scp_slot_stall_seconds(0);
        assert_eq!(SCP_SLOT_STALLED.get(), 0);
        assert_eq!(SCP_SLOT_STALL_SECONDS.get(), 0);
    }

    /// #1545: an iteration under the threshold is observed but not counted
    /// as delayed; one at/over the threshold is both.
    #[test]
    fn test_node_loop_iteration_threshold() {
        let threshold = Duration::from_millis(100);
        let hist_before = NODE_LOOP_ITERATION_SECONDS.get_sample_count();
        let delayed_before = NODE_LOOP_DELAYED.get();

        assert!(!record_node_loop_iteration(
            Duration::from_millis(10),
            threshold
        ));
        assert!(record_node_loop_iteration(
            Duration::from_millis(100),
            threshold
        ));
        assert!(record_node_loop_iteration(
            Duration::from_secs(8),
            threshold
        ));

        // Other tests may record concurrently, so assert lower bounds.
        assert!(NODE_LOOP_ITERATION_SECONDS.get_sample_count() >= hist_before + 3);
        assert!(NODE_LOOP_DELAYED.get() >= delayed_before + 2);
    }

    /// #1545: the RAII timer records on drop, and a zero threshold makes any
    /// iteration count as delayed without sleeping.
    #[test]
    fn test_node_loop_timer_records_on_drop() {
        let hist_before = NODE_LOOP_ITERATION_SECONDS.get_sample_count();
        let delayed_before = NODE_LOOP_DELAYED.get();
        {
            let _timer = NodeLoopTimer::with_threshold(Duration::ZERO);
        }
        assert!(NODE_LOOP_ITERATION_SECONDS.get_sample_count() > hist_before);
        assert!(NODE_LOOP_DELAYED.get() > delayed_before);
        assert_eq!(NODE_LOOP_DELAY_WARN_THRESHOLD, Duration::from_secs(5));
    }

    #[test]
    fn test_gossip_counters() {
        let pri = || {
            GOSSIP_FAILED_MESSAGES
                .with_label_values(&[GOSSIP_FAILED_KIND_PRIORITY])
                .get()
        };
        let non_pri = || {
            GOSSIP_FAILED_MESSAGES
                .with_label_values(&[GOSSIP_FAILED_KIND_NON_PRIORITY])
                .get()
        };
        let (p0, n0) = (pri(), non_pri());
        record_gossip_failed_messages(2, 7);
        assert!(pri() >= p0 + 2);
        assert!(non_pri() >= n0 + 7);

        let errs = || {
            GOSSIP_PUBLISH_ERRORS
                .with_label_values(&["test-topic"])
                .get()
        };
        let e0 = errs();
        record_gossip_publish_error("test-topic");
        assert_eq!(errs(), e0 + 1);
    }

    /// #1545: the new metric names appear in the `/metrics` text encoding.
    #[test]
    fn test_loop_and_gossip_metrics_encoded() {
        // A private registry: the global REGISTRY can register each metric
        // only once per process.
        let registry = Registry::new();
        register_loop_and_gossip_metrics(&registry);
        record_gossip_failed_messages(1, 1);
        record_gossip_publish_error("scp");
        record_node_loop_iteration(Duration::from_millis(1), Duration::from_secs(5));

        let mut buffer = Vec::new();
        TextEncoder::new()
            .encode(&registry.gather(), &mut buffer)
            .unwrap();
        let text = String::from_utf8(buffer).unwrap();
        for name in [
            "botho_gossip_failed_messages_total",
            "botho_gossip_publish_errors_total",
            "botho_node_loop_iteration_seconds",
            "botho_node_loop_delayed_total",
        ] {
            assert!(text.contains(name), "{name} missing from /metrics:\n{text}");
        }
    }

    #[test]
    fn test_histogram_observations() {
        let updater = MetricsUpdater::new();

        // These should not panic
        updater.observe_validation_latency(0.05);
        updater.observe_consensus_round(1.5);
        updater.observe_block_processing(0.25);
    }

    #[test]
    fn test_counter_increments() {
        let updater = MetricsUpdater::new();

        let before = CONSENSUS_NOMINATIONS.get();
        updater.inc_consensus_nominations();
        assert_eq!(CONSENSUS_NOMINATIONS.get(), before + 1);

        let before = TRANSACTIONS_PROCESSED.get();
        updater.add_transactions_processed(10);
        assert_eq!(TRANSACTIONS_PROCESSED.get(), before + 10);
    }

    /// #1547: the rate-limit counter increments per drop, per message type,
    /// and encodes under its Prometheus name.
    #[test]
    fn test_gossip_rate_limited_counter() {
        use bth_gossip::GossipMessageType;

        let tx = GOSSIP_RATE_LIMITED.with_label_values(&["transaction"]);
        let before = tx.get();
        record_gossip_rate_limited(GossipMessageType::Transaction);
        record_gossip_rate_limited(GossipMessageType::Transaction);
        assert!(tx.get() >= before + 2);

        let consensus = GOSSIP_RATE_LIMITED.with_label_values(&["consensus"]);
        let before = consensus.get();
        record_gossip_rate_limited(GossipMessageType::Consensus);
        assert!(consensus.get() > before);

        // Encode through a scratch registry. `init_metrics` registers into the
        // global REGISTRY, which can only be done once per process.
        let registry = Registry::new();
        registry
            .register(Box::new(GOSSIP_RATE_LIMITED.clone()))
            .unwrap();
        let mut buf = Vec::new();
        TextEncoder::new()
            .encode(&registry.gather(), &mut buf)
            .unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("botho_gossip_rate_limited_total{message_type=\"transaction\"}"));
        assert!(out.contains("botho_gossip_rate_limited_total{message_type=\"consensus\"}"));
    }

    #[test]
    fn test_gossip_message_type_labels_are_distinct() {
        use bth_gossip::GossipMessageType as T;
        let labels: std::collections::HashSet<_> = [
            T::Transaction,
            T::Block,
            T::Consensus,
            T::Announcement,
            T::PeerExchange,
            T::Other,
        ]
        .into_iter()
        .map(gossip_message_type_label)
        .collect();
        assert_eq!(labels.len(), 6);
    }

    #[test]
    fn test_data_dir_usage_metric() {
        let updater = MetricsUpdater::new();

        updater.set_data_dir_usage(1024 * 1024 * 100); // 100 MB
        assert_eq!(DATA_DIR_USAGE_BYTES.get(), 104857600);

        updater.set_data_dir_usage(0);
        assert_eq!(DATA_DIR_USAGE_BYTES.get(), 0);
    }

    #[test]
    fn test_calculate_dir_size_empty() {
        let temp_dir = std::env::temp_dir().join("botho_test_empty_dir");
        let _ = std::fs::create_dir_all(&temp_dir);

        let size = calculate_dir_size(&temp_dir).unwrap();
        assert_eq!(size, 0);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_calculate_dir_size_with_files() {
        let temp_dir = std::env::temp_dir().join("botho_test_files_dir");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        // Create a test file with known content
        let test_file = temp_dir.join("test.txt");
        std::fs::write(&test_file, "hello world").unwrap(); // 11 bytes

        let size = calculate_dir_size(&temp_dir).unwrap();
        assert_eq!(size, 11);

        // Clean up
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_calculate_dir_size_nested() {
        let temp_dir = std::env::temp_dir().join("botho_test_nested_dir");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(temp_dir.join("subdir")).unwrap();

        // Create files in root and subdirectory
        std::fs::write(temp_dir.join("root.txt"), "12345").unwrap(); // 5 bytes
        std::fs::write(temp_dir.join("subdir/nested.txt"), "abcdefghij").unwrap(); // 10 bytes

        let size = calculate_dir_size(&temp_dir).unwrap();
        assert_eq!(size, 15);

        // Clean up
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_calculate_dir_size_nonexistent() {
        let path = std::path::Path::new("/nonexistent/path/that/does/not/exist");
        let size = calculate_dir_size(path).unwrap();
        // Returns 0 for non-directories
        assert_eq!(size, 0);
    }
}
