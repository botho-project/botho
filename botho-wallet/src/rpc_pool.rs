//! Resilient RPC Client Pool
//!
//! Manages connections to multiple Botho nodes with:
//! - Automatic failover on errors
//! - Health-based node selection
//! - Response verification across multiple nodes for critical queries

use crate::discovery::NodeDiscovery;
use anyhow::{anyhow, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tracing::{debug, warn};

/// Timeout for RPC requests
const RPC_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum number of concurrent connections
const MAX_CONNECTIONS: usize = 5;

/// JSON-RPC request ID counter
static REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// JSON-RPC 2.0 request
#[derive(Debug, Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    method: String,
    params: Value,
    id: u64,
}

/// JSON-RPC 2.0 response
#[derive(Debug, Deserialize)]
struct JsonRpcResponse<T> {
    #[allow(dead_code)]
    jsonrpc: String,
    result: Option<T>,
    error: Option<JsonRpcError>,
    #[allow(dead_code)]
    id: u64,
}

/// JSON-RPC error
#[derive(Debug, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    pub data: Option<Value>,
}

/// Single RPC client connection
#[derive(Debug)]
struct RpcClient {
    addr: SocketAddr,
    client: reqwest::Client,
    base_url: String,
}

impl RpcClient {
    fn new(addr: SocketAddr) -> Self {
        let client = reqwest::Client::builder()
            .timeout(RPC_TIMEOUT)
            .build()
            .expect("Failed to create HTTP client");

        Self {
            addr,
            client,
            base_url: format!("http://{}", addr),
        }
    }

    async fn call<T: DeserializeOwned>(&self, method: &str, params: Value) -> Result<(T, u32)> {
        let id = REQUEST_ID.fetch_add(1, Ordering::SeqCst);
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            method: method.to_string(),
            params,
            id,
        };

        let start = Instant::now();

        let response = self
            .client
            .post(&self.base_url)
            .json(&request)
            .send()
            .await?;

        let latency = start.elapsed().as_millis() as u32;

        if !response.status().is_success() {
            return Err(anyhow!("HTTP error: {}", response.status()));
        }

        let json_response: JsonRpcResponse<T> = response.json().await?;

        if let Some(error) = json_response.error {
            return Err(anyhow!("RPC error {}: {}", error.code, error.message));
        }

        json_response
            .result
            .ok_or_else(|| anyhow!("Missing result in RPC response"))
            .map(|r| (r, latency))
    }
}

/// Pool of RPC connections with failover
pub struct RpcPool {
    /// Node discovery for finding new nodes
    discovery: NodeDiscovery,

    /// Active RPC clients
    clients: HashMap<SocketAddr, RpcClient>,

    /// Index of the primary (preferred) node
    primary_addr: Option<SocketAddr>,

    /// Minimum nodes to maintain connections to
    min_connections: usize,
}

impl RpcPool {
    /// Create a new RPC pool
    pub fn new(discovery: NodeDiscovery) -> Self {
        Self {
            discovery,
            clients: HashMap::new(),
            primary_addr: None,
            min_connections: 3,
        }
    }

    /// Initialize connections to nodes
    pub async fn connect(&mut self) -> Result<()> {
        let nodes = self.discovery.discover().await;

        if nodes.is_empty() {
            return Err(anyhow!("No nodes available"));
        }

        // Connect to the best nodes
        for addr in nodes.into_iter().take(MAX_CONNECTIONS) {
            let client = RpcClient::new(addr);

            // Verify the node is responsive
            match client.call::<NodeStatus>("node_getStatus", json!({})).await {
                Ok((status, latency)) => {
                    debug!("Connected to {} (v{}, height {})", addr, status.version, status.chain_height);
                    self.discovery.record_success(addr, latency, status.chain_height);
                    self.clients.insert(addr, client);

                    if self.primary_addr.is_none() {
                        self.primary_addr = Some(addr);
                    }
                }
                Err(e) => {
                    debug!("Failed to connect to {}: {}", addr, e);
                    self.discovery.record_failure(addr);
                }
            }
        }

        if self.clients.is_empty() {
            return Err(anyhow!("Failed to connect to any nodes"));
        }

        Ok(())
    }

    /// Execute an RPC call with automatic failover
    pub async fn call<T: DeserializeOwned>(&mut self, method: &str, params: Value) -> Result<T> {
        // Try primary node first
        if let Some(primary) = self.primary_addr {
            if let Some(client) = self.clients.get(&primary) {
                match client.call::<T>(method, params.clone()).await {
                    Ok((result, latency)) => {
                        self.discovery.record_success(primary, latency, 0);
                        return Ok(result);
                    }
                    Err(e) => {
                        warn!("Primary node {} failed: {}", primary, e);
                        self.discovery.record_failure(primary);
                    }
                }
            }
        }

        // Try other nodes
        let addrs: Vec<_> = self.clients.keys().cloned().collect();
        for addr in addrs {
            if Some(addr) == self.primary_addr {
                continue; // Already tried
            }

            if let Some(client) = self.clients.get(&addr) {
                match client.call::<T>(method, params.clone()).await {
                    Ok((result, latency)) => {
                        self.discovery.record_success(addr, latency, 0);
                        // Promote this node to primary
                        self.primary_addr = Some(addr);
                        return Ok(result);
                    }
                    Err(e) => {
                        debug!("Node {} failed: {}", addr, e);
                        self.discovery.record_failure(addr);
                    }
                }
            }
        }

        Err(anyhow!("All nodes failed"))
    }

    /// Execute an RPC call and verify across multiple nodes
    ///
    /// Used for critical queries where we want to detect lying nodes.
    pub async fn call_verified<T>(&mut self, method: &str, params: Value) -> Result<T>
    where
        T: DeserializeOwned + PartialEq + Clone,
    {
        let mut results: Vec<(SocketAddr, T)> = Vec::new();

        let addrs: Vec<_> = self.clients.keys().cloned().collect();
        for addr in addrs {
            if let Some(client) = self.clients.get(&addr) {
                match client.call::<T>(method, params.clone()).await {
                    Ok((result, latency)) => {
                        self.discovery.record_success(addr, latency, 0);
                        results.push((addr, result));
                    }
                    Err(e) => {
                        debug!("Node {} failed during verification: {}", addr, e);
                        self.discovery.record_failure(addr);
                    }
                }
            }
        }

        if results.is_empty() {
            return Err(anyhow!("No nodes responded"));
        }

        // Find majority result
        let mut counts: HashMap<usize, usize> = HashMap::new();
        for (i, (_, result)) in results.iter().enumerate() {
            for (j, (_, other)) in results.iter().enumerate() {
                if i != j && result == other {
                    *counts.entry(i).or_default() += 1;
                }
            }
        }

        // Return the result with most matches, or first if all different
        let best_idx = counts
            .into_iter()
            .max_by_key(|(_, count)| *count)
            .map(|(idx, _)| idx)
            .unwrap_or(0);

        Ok(results.remove(best_idx).1)
    }

    /// Get node status
    pub async fn get_node_status(&mut self) -> Result<NodeStatus> {
        self.call("node_getStatus", json!({})).await
    }

    /// Get chain info
    pub async fn get_chain_info(&mut self) -> Result<ChainInfo> {
        self.call("getChainInfo", json!({})).await
    }

    /// Get outputs in a block range (for wallet sync)
    pub async fn get_outputs(&mut self, start_height: u64, end_height: u64) -> Result<Vec<BlockOutputs>> {
        self.call(
            "chain_getOutputs",
            json!({
                "start_height": start_height,
                "end_height": end_height
            }),
        )
        .await
    }

    /// Submit a signed transaction
    pub async fn submit_transaction(&mut self, tx_hex: &str) -> Result<String> {
        let result: SubmitTxResult = self
            .call("tx_submit", json!({ "tx_hex": tx_hex }))
            .await?;
        Ok(result.tx_hash)
    }

    /// Get fee estimate
    pub async fn estimate_fee(&mut self, priority: &str) -> Result<u64> {
        let result: FeeEstimate = self
            .call("tx_estimateFee", json!({ "priority": priority }))
            .await?;
        Ok(result.recommended_fee)
    }

    /// Get connected peers from a node (for gossip discovery)
    pub async fn get_peers(&mut self) -> Result<Vec<SocketAddr>> {
        let result: PeersResult = self.call("network_getPeers", json!({})).await?;
        Ok(result.peers)
    }

    /// Get mutable reference to discovery
    pub fn discovery_mut(&mut self) -> &mut NodeDiscovery {
        &mut self.discovery
    }

    /// Get reference to discovery
    pub fn discovery(&self) -> &NodeDiscovery {
        &self.discovery
    }

    /// Get number of connected clients
    pub fn connected_count(&self) -> usize {
        self.clients.len()
    }

    /// Ensure we have enough connections
    pub async fn maintain_connections(&mut self) -> Result<()> {
        // Remove dead clients
        let dead: Vec<_> = self
            .clients
            .keys()
            .filter(|addr| {
                self.discovery
                    .get_health(addr)
                    .map(|h| h.failures >= 3)
                    .unwrap_or(false)
            })
            .cloned()
            .collect();

        for addr in dead {
            self.clients.remove(&addr);
            if self.primary_addr == Some(addr) {
                self.primary_addr = None;
            }
        }

        // Add new connections if needed
        if self.clients.len() < self.min_connections {
            let best_nodes = self.discovery.get_best_nodes(MAX_CONNECTIONS);

            for addr in best_nodes {
                if self.clients.contains_key(&addr) {
                    continue;
                }

                let client = RpcClient::new(addr);
                if let Ok((status, latency)) = client.call::<NodeStatus>("node_getStatus", json!({})).await {
                    self.discovery.record_success(addr, latency, status.chain_height);
                    self.clients.insert(addr, client);

                    if self.primary_addr.is_none() {
                        self.primary_addr = Some(addr);
                    }
                }
            }
        }

        Ok(())
    }
}

// Response types for RPC calls

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct NodeStatus {
    pub version: String,
    pub network: String,
    pub uptime_seconds: u64,
    pub sync_status: String,
    pub chain_height: u64,
    pub tip_hash: String,
    pub peer_count: usize,
    pub mempool_size: usize,
    pub minting_active: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ChainInfo {
    pub height: u64,
    pub tip_hash: String,
    pub difficulty: u64,
    pub total_mined: u64,
    pub mempool_size: usize,
    pub mempool_fees: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BlockOutputs {
    pub height: u64,
    pub outputs: Vec<TxOutput>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TxOutput {
    pub tx_hash: String,
    pub output_index: u32,
    pub public_key: String,
    pub amount_commitment: String,
}

#[derive(Debug, Deserialize)]
struct SubmitTxResult {
    tx_hash: String,
}

#[derive(Debug, Deserialize)]
struct FeeEstimate {
    #[allow(dead_code)]
    minimum_fee: u64,
    recommended_fee: u64,
    #[allow(dead_code)]
    high_priority_fee: u64,
}

#[derive(Debug, Deserialize)]
struct PeersResult {
    peers: Vec<SocketAddr>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rpc_pool_new() {
        let discovery = NodeDiscovery::new();
        let pool = RpcPool::new(discovery);
        assert_eq!(pool.connected_count(), 0);
    }
}
