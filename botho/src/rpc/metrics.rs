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
//! - `botho_validation_latency_seconds` - Transaction validation latency (histogram)
//! - `botho_consensus_round_duration_seconds` - SCP consensus round duration (histogram)
//! - `botho_consensus_nominations_total` - Total SCP nominations (counter)
//!
//! ### System Metrics
//! - `botho_data_dir_usage_bytes` - Bytes used by the data directory (gauge)
//! - `process_resident_memory_bytes` - Process resident set size (from process collector)
//! - `process_virtual_memory_bytes` - Process virtual memory size (from process collector)
//! - `process_cpu_seconds_total` - Total CPU time used (from process collector)
//! - `process_open_fds` - Number of open file descriptors (from process collector)
//! - `process_start_time_seconds` - Process start time (from process collector)
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
use hyper::body::Bytes;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use lazy_static::lazy_static;
use prometheus::{
    process_collector::ProcessCollector, Counter, CounterVec, Encoder, Gauge, Histogram,
    HistogramOpts, IntCounter, IntGauge, Opts, Registry, TextEncoder,
};
use serde::Serialize;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::{error, info};

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

        let block_height = Gauge::with_opts(Opts::new(
            "botho_block_height",
            "Current blockchain height",
        ))
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

    // ============================================================================
    // System Metrics
    // ============================================================================

    /// Total bytes used by the node's data directory.
    pub static ref DATA_DIR_USAGE_BYTES: IntGauge = IntGauge::new(
        "botho_data_dir_usage_bytes",
        "Total bytes used by the node's data directory"
    ).expect("Failed to create data_dir_usage_bytes metric");

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

    // Register system metrics
    REGISTRY
        .register(Box::new(DATA_DIR_USAGE_BYTES.clone()))
        .expect("Failed to register data_dir_usage_bytes");

    // Register process collector for memory metrics
    // This provides: process_resident_memory_bytes, process_virtual_memory_bytes,
    // process_cpu_seconds_total, process_open_fds, process_start_time_seconds
    let process_collector = ProcessCollector::for_self();
    REGISTRY
        .register(Box::new(process_collector))
        .expect("Failed to register process collector");

    info!("Prometheus metrics initialized");
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
    pub fn set_total_minted(&self, total: u64) {
        TOTAL_MINTED.set(total as i64);
    }

    /// Update the total fees burned metric.
    pub fn set_total_fees_burned(&self, total: u64) {
        TOTAL_FEES_BURNED.set(total as i64);
    }

    /// Update the minting active metric.
    pub fn set_minting_active(&self, active: bool) {
        MINTING_ACTIVE.set(if active { 1 } else { 0 });
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
