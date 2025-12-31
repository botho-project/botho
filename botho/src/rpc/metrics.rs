//! Observability endpoints for Botho node
//!
//! Provides health, readiness, and Prometheus metrics endpoints for
//! production monitoring and load balancer integration.

use prometheus::{
    Counter, CounterVec, Encoder, Gauge, Opts, Registry, TextEncoder,
};
use serde::Serialize;
use std::sync::Arc;

use super::RpcState;

/// Prometheus metrics for the Botho node
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

        let block_height = Gauge::with_opts(
            Opts::new("botho_block_height", "Current blockchain height")
        ).expect("metric can be created");

        let peer_count = Gauge::with_opts(
            Opts::new("botho_peer_count", "Number of connected peers")
        ).expect("metric can be created");

        let mempool_size = Gauge::with_opts(
            Opts::new("botho_mempool_size", "Number of transactions in mempool")
        ).expect("metric can be created");

        let rpc_requests_total = CounterVec::new(
            Opts::new("botho_rpc_requests_total", "Total RPC requests"),
            &["method"],
        ).expect("metric can be created");

        let rpc_errors_total = CounterVec::new(
            Opts::new("botho_rpc_errors_total", "Total RPC errors"),
            &["method"],
        ).expect("metric can be created");

        // Register all metrics
        registry.register(Box::new(block_height.clone())).expect("collector can be registered");
        registry.register(Box::new(peer_count.clone())).expect("collector can be registered");
        registry.register(Box::new(mempool_size.clone())).expect("collector can be registered");
        registry.register(Box::new(rpc_requests_total.clone())).expect("collector can be registered");
        registry.register(Box::new(rpc_errors_total.clone())).expect("collector can be registered");

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
}
