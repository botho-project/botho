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
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
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
    addr: Option<SocketAddr>,
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
            addr: Some(addr),
            client,
            base_url: format!("http://{}", addr),
        }
    }

    fn pinned(endpoint: &str) -> Result<Self> {
        let url = reqwest::Url::parse(endpoint)?;
        anyhow::ensure!(
            matches!(url.scheme(), "http" | "https") && url.host_str().is_some(),
            "RPC endpoint must be an absolute HTTP(S) URL"
        );
        anyhow::ensure!(
            url.username().is_empty() && url.password().is_none() && url.fragment().is_none(),
            "RPC endpoint must not contain userinfo or a fragment"
        );
        let client = reqwest::Client::builder()
            .timeout(RPC_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            addr: None,
            client,
            base_url: url.to_string(),
        })
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
    /// Explicit endpoint mode never discovers, fails over, or repopulates
    /// clients.
    pinned: Option<(RpcClient, String)>,
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
            pinned: None,
            discovery,
            clients: HashMap::new(),
            primary_addr: None,
            min_connections: 3,
        }
    }

    /// Connect only to the selected endpoint and require its reported network.
    /// This checks trusted-node metadata, not authenticated chain headers.
    /// Existing discovery-based CLI pools retain their previous behavior.
    pub async fn connect_endpoint(endpoint: &str, expected_network: &str) -> Result<Self> {
        anyhow::ensure!(
            matches!(expected_network, "botho-mainnet" | "botho-testnet"),
            "Unrecognized wallet network"
        );
        let mut pool = Self::new(NodeDiscovery::new());
        pool.pinned = Some((RpcClient::pinned(endpoint)?, expected_network.to_owned()));
        pool.connect().await?;
        Ok(pool)
    }

    /// Initialize connections to nodes
    pub async fn connect(&mut self) -> Result<()> {
        if let Some((client, network)) = &self.pinned {
            let (status, _) = client
                .call::<NodeStatus>("node_getStatus", json!({}))
                .await?;
            anyhow::ensure!(
                status.network == *network,
                "RPC node reported a different network"
            );
            return Ok(());
        }
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
                    debug!(
                        "Connected to {} (v{}, height {})",
                        addr, status.version, status.chain_height
                    );
                    self.discovery
                        .record_success(addr, latency, status.chain_height);
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
        if let Some((client, _)) = &self.pinned {
            return client.call(method, params).await.map(|(result, _)| result);
        }
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
        anyhow::ensure!(
            self.pinned.is_none(),
            "A pinned endpoint cannot provide multi-node verification"
        );
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
    pub async fn get_outputs(
        &mut self,
        start_height: u64,
        end_height: u64,
    ) -> Result<Vec<BlockOutputs>> {
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
        let result: SubmitTxResult = self.call("tx_submit", json!({ "tx_hex": tx_hex })).await?;
        Ok(result.tx_hash)
    }

    /// Get fee estimate
    pub async fn estimate_fee(&mut self, priority: &str) -> Result<u64> {
        let result: FeeEstimate = self
            .call("tx_estimateFee", json!({ "priority": priority }))
            .await?;
        Ok(result.recommended_fee)
    }

    /// Get current network fee rate.
    ///
    /// Returns the dynamic base fee rate from the network, including congestion
    /// information. Wallets should use this to update their local FeeEstimator.
    pub async fn get_fee_rate(&mut self) -> Result<NetworkFeeRate> {
        self.call("fee_getRate", json!({})).await
    }

    /// Get connected peers from a node (for gossip discovery)
    pub async fn get_peers(&mut self) -> Result<Vec<SocketAddr>> {
        let result: PeersResult = self.call("network_getPeers", json!({})).await?;
        Ok(result.peers)
    }

    /// Request testnet coins from the faucet
    ///
    /// Sends a faucet_request RPC with the wallet address and returns
    /// the transaction hash and amount dispensed.
    pub async fn faucet_request(&mut self, address: &str) -> Result<FaucetRequestResult> {
        self.call("faucet_request", json!({ "address": address }))
            .await
    }

    // ========================================================================
    // Exchange Integration Methods
    // ========================================================================

    /// Get transaction by hash (for exchange integration)
    ///
    /// Returns transaction info including status, block height, and
    /// confirmations.
    pub async fn get_transaction(&mut self, tx_hash: &str) -> Result<TransactionInfo> {
        self.call("getTransaction", json!({ "tx_hash": tx_hash }))
            .await
    }

    /// Get transaction status (lightweight version)
    ///
    /// Returns just the confirmation status without full transaction details.
    pub async fn get_transaction_status(&mut self, tx_hash: &str) -> Result<TransactionStatus> {
        self.call("getTransactionStatus", json!({ "tx_hash": tx_hash }))
            .await
    }

    /// Validate a Botho address
    ///
    /// Returns address info including network and type (classical/quantum).
    pub async fn validate_address(&mut self, address: &str) -> Result<AddressValidation> {
        self.call("validateAddress", json!({ "address": address }))
            .await
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
        if self.pinned.is_some() {
            1
        } else {
            self.clients.len()
        }
    }

    /// Ensure we have enough connections
    pub async fn maintain_connections(&mut self) -> Result<()> {
        if self.pinned.is_some() {
            return self.connect().await;
        }
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
                if let Ok((status, latency)) =
                    client.call::<NodeStatus>("node_getStatus", json!({})).await
                {
                    self.discovery
                        .record_success(addr, latency, status.chain_height);
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
#[serde(rename_all = "camelCase")]
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
#[serde(rename_all = "camelCase")]
pub struct ChainInfo {
    pub height: u64,
    pub tip_hash: String,
    pub difficulty: u64,
    /// Total picocredits mined. The node emits this as a decimal string
    /// because the u128 value exceeds JS's 2^53 safe-integer limit
    /// (botho/src/rpc/mod.rs, `getChainInfo` handler). Callers that need a
    /// numeric value should parse it (e.g. `.parse::<u128>()`).
    pub total_mined: String,
    pub mempool_size: usize,
    pub mempool_fees: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BlockOutputs {
    pub height: u64,
    pub outputs: Vec<TxOutput>,
}

/// Canonical ledger location, separate from the legacy RPC identifier.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LedgerOutpoint {
    pub tx_hash: String,
    pub output_index: u32,
}

/// Resolve only the supported coinbase/ordinary index contracts. Old nodes
/// already mark coinbases explicitly; a bare MAX is not proof of coinbase.
pub(crate) fn crypto_output_index(
    index: u32,
    coinbase: bool,
    explicit: Option<u32>,
) -> Option<u32> {
    if coinbase {
        if index != u32::MAX || explicit.is_some_and(|value| value != 0) {
            return None;
        }
        Some(0)
    } else if index == u32::MAX || explicit.is_some_and(|value| value != index) {
        None
    } else {
        Some(index)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TxOutput {
    // Node emits `txHash` (botho/src/rpc/mod.rs, chain_getOutputs handler).
    pub tx_hash: String,
    // Node emits `outputIndex`.
    pub output_index: u32,
    /// Optional derivation index; absent on older nodes and legacy lottery
    /// rows.
    #[serde(default)]
    pub crypto_output_index: Option<u32>,
    /// Explicit existing RPC discriminator, needed for old-node compatibility.
    #[serde(default)]
    pub coinbase: bool,
    /// Additive canonical ledger identity. Never substitutes for crypto index.
    #[serde(default)]
    pub ledger_outpoint: Option<LedgerOutpoint>,
    /// One-time target key (stealth spend key)
    pub target_key: String,
    /// Ephemeral public key (for DH derivation)
    pub public_key: String,
    /// Amount commitment (or plaintext amount)
    pub amount_commitment: String,
    /// Cluster tags for progressive fee calculation.
    /// Array of [cluster_id, weight] pairs where weight is parts per million.
    #[serde(default)]
    pub cluster_tags: Vec<[u64; 2]>,
    /// Unified ML-KEM-768 ciphertext (1088 bytes, hex-encoded), emitted by the
    /// node as `kemCiphertext` on the single hybrid scan path (issue #970).
    /// `None` for a classical/legacy KEM-less output. The scanner decapsulates
    /// this with the wallet's derived ML-KEM secret to detect hybrid outputs;
    /// there is no longer a separate `is_pq_output` flag — presence of the
    /// ciphertext IS the hybrid marker.
    #[serde(default)]
    pub kem_ciphertext: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubmitTxResult {
    tx_hash: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
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

/// Result of a faucet request
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FaucetRequestResult {
    /// Whether the request was successful
    pub success: bool,
    /// Transaction hash (only if successful)
    #[serde(default)]
    pub tx_hash: String,
    /// Amount dispensed in picocredits (as string)
    #[serde(default)]
    pub amount: String,
    /// Formatted amount (e.g., "10.000000 BTH")
    #[serde(default)]
    pub amount_formatted: String,
}

/// Network fee rate information returned by fee_getRate.
///
/// Wallets should cache this information and refresh periodically
/// to ensure accurate fee estimation.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkFeeRate {
    /// Current base fee rate in picocredits per byte.
    pub base_rate: u64,

    /// Minimum possible base rate (floor).
    pub base_min: u64,

    /// Maximum possible base rate (ceiling).
    pub base_max: u64,

    /// Current multiplier (base_rate / base_min).
    pub multiplier: f64,

    /// Network congestion level (0.0 to 1.0).
    pub congestion: f64,

    /// Target block fullness threshold.
    pub target_fullness: f64,

    /// Whether dynamic fee adjustment is active.
    pub adjustment_active: bool,

    /// Estimated blocks until fees return to minimum.
    pub blocks_to_recovery: usize,
}

// ============================================================================
// Exchange Integration Response Types
// ============================================================================

/// Transaction information returned by getTransaction
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionInfo {
    /// Transaction hash (hex)
    pub tx_hash: String,
    /// Transaction status: "pending", "confirmed", or "unknown"
    pub status: String,
    /// Block height (null if pending)
    pub block_height: Option<u64>,
    /// Number of confirmations (0 if pending)
    pub confirmations: u64,
    /// Whether the transaction is in the mempool
    pub in_mempool: bool,
    /// Transaction type: "simple" or "ring"
    #[serde(rename = "type")]
    pub tx_type: Option<String>,
    /// Transaction fee in picocredits
    pub fee: Option<u64>,
    /// Number of outputs
    pub output_count: Option<usize>,
    /// Total output amount in picocredits
    pub total_output: Option<u64>,
    /// Block height when transaction was created
    pub created_at_height: Option<u64>,
}

/// Transaction status returned by getTransactionStatus
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionStatus {
    /// Transaction hash (hex)
    pub tx_hash: String,
    /// Transaction status: "pending", "confirmed", or "unknown"
    pub status: String,
    /// Number of confirmations (0 if pending or unknown)
    pub confirmations: u64,
    /// Whether the transaction is confirmed (at least 1 confirmation)
    pub confirmed: bool,
}

/// Address validation result returned by validateAddress
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddressValidation {
    /// Whether the address is valid
    pub valid: bool,
    /// The address (canonical form if valid, original if invalid)
    pub address: String,
    /// Network name: "Mainnet" or "Testnet" (only if valid)
    pub network: Option<String>,
    /// Address type: "classical" or "quantum" (only if valid)
    #[serde(rename = "type")]
    pub address_type: Option<String>,
    /// Whether this is a quantum-safe address (only if valid)
    pub is_quantum: Option<bool>,
    /// Error message (only if invalid)
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_endpoint_rejects_ambiguous_urls() {
        for endpoint in [
            "file:///tmp/rpc",
            "http://user:password@localhost/rpc",
            "https://localhost/rpc#fragment",
            "localhost:17101",
        ] {
            assert!(RpcClient::pinned(endpoint).is_err(), "{endpoint}");
        }
        let client = RpcClient::pinned("https://example.invalid/custom/rpc?route=wallet").unwrap();
        assert_eq!(
            client.base_url,
            "https://example.invalid/custom/rpc?route=wallet"
        );
        assert!(client.addr.is_none());
    }

    // One disposable loopback response, never a live node or transaction
    // submission.
    async fn fixture_endpoint(response: String) -> (String, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = vec![0; 8192];
            let count = stream.read(&mut bytes).await.unwrap();
            let request = String::from_utf8_lossy(&bytes[..count]).into_owned();
            stream.write_all(response.as_bytes()).await.unwrap();
            request
        });
        (format!("http://{addr}/explicit/rpc"), task)
    }

    fn status_response(network: &str) -> String {
        let body = json!({"jsonrpc":"2.0", "id":1, "result": {
            "version":"fixture", "network":network, "uptimeSeconds":0,
            "syncStatus":"synced", "chainHeight":0, "tipHash":"00",
            "peerCount":0,"mempoolSize":0,"mintingActive":false
        }})
        .to_string();
        format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len())
    }

    #[tokio::test]
    async fn pinned_endpoint_matches_network_and_never_fails_over() {
        let (url, server) = fixture_endpoint(status_response("botho-testnet")).await;
        let mut pool = RpcPool::connect_endpoint(&url, "botho-testnet")
            .await
            .unwrap();
        let request = server.await.unwrap();
        assert!(request.starts_with("POST /explicit/rpc HTTP/1.1"));
        assert_eq!(pool.connected_count(), 1);
        assert!(pool.clients.is_empty());
        assert!(pool.get_node_status().await.is_err()); // fixture listener is gone
        assert!(pool.connect().await.is_err());
        assert!(pool.maintain_connections().await.is_err());
        assert!(pool.clients.is_empty());
        assert!(pool
            .call_verified::<Value>("node_getStatus", json!({}))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn pinned_endpoint_rejects_other_reported_network() {
        let (url, server) = fixture_endpoint(status_response("botho-mainnet")).await;
        assert!(RpcPool::connect_endpoint(&url, "botho-testnet")
            .await
            .is_err());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn pinned_endpoint_does_not_follow_redirects() {
        let (url, server) = fixture_endpoint("HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/never\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into()).await;
        let error = RpcPool::connect_endpoint(&url, "botho-testnet")
            .await
            .err()
            .unwrap();
        assert!(error.to_string().contains("302"));
        server.await.unwrap();
    }

    #[test]
    fn test_rpc_pool_new() {
        let discovery = NodeDiscovery::new();
        let pool = RpcPool::new(discovery);
        assert_eq!(pool.connected_count(), 0);
    }

    // ---------------------------------------------------------------------
    // RPC wire-format regression guards (#610)
    //
    // The node emits camelCase JSON (see botho/src/rpc/mod.rs handlers).
    // These fixtures mirror the *actual* shape each handler emits so that a
    // future field-casing drift is caught at `cargo test` time rather than
    // live against the testnet ("Failed to connect to any nodes"). This is
    // the wallet edition of the #541-#544 hardcoded-observability bug class.
    // ---------------------------------------------------------------------

    use serde_json::json;

    /// Mirrors `node_getStatus` (botho/src/rpc/mod.rs). Wallet only reads a
    /// subset of the emitted fields; extra fields must be ignored gracefully.
    #[test]
    fn test_node_status_deserializes_from_node_json() {
        let fixture = json!({
            "version": "3.0.0",
            "nodeVersion": "3.0.0",
            "gitCommit": "deadbeef",
            "gitCommitShort": "deadbee",
            "buildTime": "unknown",
            "network": "botho-testnet",
            "uptimeSeconds": 3600u64,
            "syncStatus": "synced",
            "syncProgress": 1.0,
            "synced": true,
            "chainHeight": 204u64,
            "tipHash": "0000000000000000000000000000000000000000000000000000000000000000",
            "peerCount": 2usize,
            "scpPeerCount": 2usize,
            "mempoolSize": 0usize,
            "mintingActive": true,
            "mintingThreads": 4u64,
            "totalTransactions": 17u64,
            "quorumFaultTolerant": false,
            "quorumDegenerate": true,
            "minerStalled": false,
        });

        let status: NodeStatus =
            serde_json::from_value(fixture).expect("NodeStatus should deserialize camelCase JSON");
        assert_eq!(status.version, "3.0.0");
        assert_eq!(status.network, "botho-testnet");
        assert_eq!(status.uptime_seconds, 3600);
        assert_eq!(status.sync_status, "synced");
        assert_eq!(status.chain_height, 204);
        assert_eq!(
            status.tip_hash,
            "0000000000000000000000000000000000000000000000000000000000000000"
        );
        assert_eq!(status.peer_count, 2);
        assert_eq!(status.mempool_size, 0);
        assert!(status.minting_active);
    }

    /// Mirrors `getChainInfo` (botho/src/rpc/mod.rs). `totalMined` is a
    /// decimal string (u128 > JS safe-integer), not a number.
    #[test]
    fn test_chain_info_deserializes_with_string_total_mined() {
        let fixture = json!({
            "height": 204u64,
            "tipHash": "1111111111111111111111111111111111111111111111111111111111111111",
            "difficulty": 12345u64,
            "totalMined": "611000000000000000000",
            "totalFeesBurned": "1000",
            "circulatingSupply": "610999999999999999000",
            "mempoolSize": 3usize,
            "mempoolFees": 5000u64,
        });

        let info: ChainInfo =
            serde_json::from_value(fixture).expect("ChainInfo should deserialize camelCase JSON");
        assert_eq!(info.height, 204);
        assert_eq!(
            info.tip_hash,
            "1111111111111111111111111111111111111111111111111111111111111111"
        );
        assert_eq!(info.difficulty, 12345);
        // total_mined is a decimal string that round-trips exactly and parses
        // into the full-precision u128 value.
        assert_eq!(info.total_mined, "611000000000000000000");
        assert_eq!(
            info.total_mined.parse::<u128>().unwrap(),
            611_000_000_000_000_000_000u128
        );
        assert_eq!(info.mempool_size, 3);
        assert_eq!(info.mempool_fees, 5000);
    }

    /// Mirrors a regular transaction output emitted by `chain_getOutputs`
    /// (botho/src/rpc/mod.rs). Exercises both the newly-renamed fields
    /// (`txHash`/`outputIndex`) and the previously-working stealth-key fields.
    #[test]
    fn test_tx_output_deserializes_from_node_json() {
        let fixture = json!({
            "txHash": "aabbccddeeff00112233445566778899aabbccddeeff00112233445566778899",
            "outputIndex": 0u32,
            "targetKey": "deadbeef",
            "publicKey": "cafebabe",
            "amountCommitment": "0100000000000000",
            "clusterTags": [[1u64, 1_000_000u64], [2u64, 500_000u64]],
        });

        let out: TxOutput =
            serde_json::from_value(fixture).expect("TxOutput should deserialize camelCase JSON");
        assert_eq!(
            out.tx_hash,
            "aabbccddeeff00112233445566778899aabbccddeeff00112233445566778899"
        );
        assert_eq!(out.output_index, 0);
        assert_eq!(out.target_key, "deadbeef");
        assert_eq!(out.public_key, "cafebabe");
        assert_eq!(out.amount_commitment, "0100000000000000");
        assert_eq!(out.cluster_tags, vec![[1, 1_000_000], [2, 500_000]]);
        // The unified ML-KEM ciphertext field defaults to None when absent.
        assert!(out.kem_ciphertext.is_none());
    }

    /// Mirrors the coinbase output shape emitted by `chain_getOutputs`
    /// (`outputIndex` == u32::MAX, no explicit `clusterTags` guaranteed).
    #[test]
    fn test_tx_output_coinbase_shape_deserializes() {
        let fixture = json!({
            "txHash": "0011223344556677889900112233445566778899001122334455667788990011",
            "outputIndex": u32::MAX,
            "targetKey": "aa",
            "publicKey": "bb",
            "amountCommitment": "cc",
            "clusterTags": [],
            "coinbase": true,
        });

        let out: TxOutput = serde_json::from_value(fixture)
            .expect("coinbase TxOutput should deserialize camelCase JSON");
        assert_eq!(out.output_index, u32::MAX);
        assert!(out.coinbase);
        assert_eq!(out.crypto_output_index, None);
        assert_eq!(
            crypto_output_index(out.output_index, out.coinbase, out.crypto_output_index),
            Some(0)
        );
        assert!(out.ledger_outpoint.is_none());
        assert!(out.cluster_tags.is_empty());
    }

    #[test]
    fn additive_output_metadata_keeps_legacy_decoder_and_lottery_identity() {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct LegacyId {
            tx_hash: String,
            output_index: u32,
        }
        let mut row = json!({"txHash":"aa", "outputIndex":u32::MAX,
            "targetKey":"bb", "publicKey":"cc", "amountCommitment":"dd",
            "coinbase":true, "cryptoOutputIndex":0,
            "ledgerOutpoint":{"txHash":"ee", "outputIndex":0}});
        let legacy: LegacyId = serde_json::from_value(row.clone()).unwrap();
        let decoded: TxOutput = serde_json::from_value(row.clone()).unwrap();
        assert_eq!(legacy.tx_hash, decoded.tx_hash);
        assert_eq!(legacy.output_index, decoded.output_index);
        assert_eq!(
            decoded.ledger_outpoint.unwrap(),
            LedgerOutpoint {
                tx_hash: "ee".into(),
                output_index: 0
            }
        );
        row.as_object_mut().unwrap().remove("coinbase");
        row.as_object_mut().unwrap().remove("cryptoOutputIndex");
        row["lottery"] = json!(true);
        row["outputIndex"] = json!(3);
        row["ledgerOutpoint"] = json!({"txHash":"aa", "outputIndex":3});
        let lottery: TxOutput = serde_json::from_value(row).unwrap();
        assert!(!lottery.coinbase);
        assert_eq!(lottery.output_index, 3);
        assert_eq!(lottery.crypto_output_index, None);
        assert_eq!(
            crypto_output_index(
                lottery.output_index,
                lottery.coinbase,
                lottery.crypto_output_index
            ),
            Some(3)
        );
    }

    #[test]
    fn crypto_index_contract_preserves_identity_and_requires_coinbase() {
        assert_eq!(crypto_output_index(u32::MAX, true, None), Some(0));
        assert_eq!(crypto_output_index(u32::MAX, true, Some(0)), Some(0));
        assert_eq!(crypto_output_index(u32::MAX, false, None), None);
        assert_eq!(crypto_output_index(3, false, None), Some(3));
        assert_eq!(crypto_output_index(3, false, Some(3)), Some(3));
        assert_eq!(crypto_output_index(3, false, Some(0)), None);
        assert_eq!(crypto_output_index(u32::MAX, true, Some(1)), None);
        assert_eq!(crypto_output_index(1, true, Some(0)), None);
        // Legacy lottery index is retained, not reinterpreted as a source index.
        assert_eq!(crypto_output_index(2, false, None), Some(2));
    }

    /// Mirrors `tx_submit` success (botho/src/rpc/mod.rs).
    #[test]
    fn test_submit_tx_result_deserializes_from_node_json() {
        let fixture = json!({
            "txHash": "abcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcd0",
        });

        let result: SubmitTxResult =
            serde_json::from_value(fixture).expect("SubmitTxResult should deserialize txHash");
        assert_eq!(
            result.tx_hash,
            "abcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcd0"
        );
    }

    /// Mirrors `estimateFee` (botho/src/rpc/mod.rs). Only a subset of the
    /// emitted fields is consumed; extras must be ignored.
    #[test]
    fn test_fee_estimate_deserializes_from_node_json() {
        let fixture = json!({
            "minimumFee": 1000u64,
            "clusterFactor": 1000u64,
            "clusterFactorDisplay": "1.00x",
            "recommendedFee": 2000u64,
            "highPriorityFee": 4000u64,
            // clusterWealth is now a STRING (u128 pico, #626); FeeEstimate does
            // not consume it, but the fixture mirrors the real string contract.
            "clusterWealth": "0",
            "params": {
                "amount": 100u64,
                "txType": "transfer",
                "memos": 0u64,
            },
        });

        let fee: FeeEstimate =
            serde_json::from_value(fixture).expect("FeeEstimate should deserialize camelCase JSON");
        assert_eq!(fee.minimum_fee, 1000);
        assert_eq!(fee.recommended_fee, 2000);
        assert_eq!(fee.high_priority_fee, 4000);
    }
}
