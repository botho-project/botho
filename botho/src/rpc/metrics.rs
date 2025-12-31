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
    pub version: String,
    pub block_height: u64,
    pub peer_count: usize,
    pub synced: bool,
}

/// Check the health of the node
pub fn check_health(state: &RpcState) -> HealthResponse {
    let mut status = HealthStatus::Healthy;
    let mut block_height = 0u64;
    let mut peer_count = 0usize;
    let synced: bool;

    // Get block height
    match state.ledger.read() {
        Ok(ledger) => {
            if let Ok(chain_state) = ledger.get_chain_state() {
                block_height = chain_state.height;
            }
        }
        Err(_) => {
            status = HealthStatus::Unhealthy;
        }
    }

    // Get peer count
    match state.peer_count.read() {
        Ok(peers) => {
            peer_count = *peers;
        }
        Err(_) => {
            status = HealthStatus::Unhealthy;
        }
    }

    // Check if degraded (no peers but otherwise functional)
    if status == HealthStatus::Healthy && peer_count == 0 {
        status = HealthStatus::Degraded;
    }

    // For now, consider synced if we have any blocks
    // In production, this would compare against known network height
    synced = block_height > 0;

    HealthResponse {
        status,
        version: env!("CARGO_PKG_VERSION").to_string(),
        block_height,
        peer_count,
        synced,
    }
}

/// Check if the node is ready to accept requests
///
/// Returns true if the node is synced and healthy enough to serve requests.
/// This is used by load balancers to determine if traffic should be routed here.
pub fn check_ready(state: &RpcState) -> bool {
    let health = check_health(state);

    // Ready if healthy or degraded (degraded = functional but no peers)
    // and synced (has at least genesis block)
    matches!(health.status, HealthStatus::Healthy | HealthStatus::Degraded)
        && health.synced
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
}
