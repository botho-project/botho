//! Node Discovery
//!
//! Multi-layer node discovery inspired by Bitcoin's approach:
//! 1. DNS seeds (primary) - maintained by trusted community members
//! 2. Hardcoded bootstrap nodes (fallback) - updated with releases
//! 3. Peer exchange (gossip) - learn peers from connected nodes

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// DNS seed hostnames that resolve to active Botho nodes
const DNS_SEEDS: &[&str] = &[
    "seed.botho.network",
    "seed2.botho.network",
    "seed.botho-nodes.org",
];

/// Hardcoded bootstrap nodes as fallback (geographically distributed)
const BOOTSTRAP_NODES: &[&str] = &[
    // These would be updated with each release
    "127.0.0.1:8545", // Local development
];

/// Default RPC port for Botho nodes
const DEFAULT_PORT: u16 = 8545;

/// Timeout for probing nodes
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Health status for a node
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NodeHealth {
    /// Last time we successfully communicated with this node
    pub last_seen: Option<u64>, // Unix timestamp
    /// Round-trip latency in milliseconds
    pub latency_ms: u32,
    /// Number of consecutive failures
    pub failures: u32,
    /// Last known block height
    pub block_height: u64,
    /// Node version string
    pub version: Option<String>,
}

impl NodeHealth {
    /// Check if this node is considered healthy
    pub fn is_healthy(&self) -> bool {
        self.failures < 3
    }

    /// Score this node for selection (higher is better)
    pub fn score(&self) -> u32 {
        if self.failures >= 3 {
            return 0;
        }

        let mut score = 100u32;

        // Prefer lower latency
        score = score.saturating_sub(self.latency_ms / 10);

        // Prefer recently seen nodes
        if let Some(last_seen) = self.last_seen {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let age_minutes = (now.saturating_sub(last_seen)) / 60;
            score = score.saturating_sub(age_minutes as u32);
        }

        // Prefer higher block height (more synced)
        score = score.saturating_add((self.block_height / 1000) as u32);

        score
    }
}

/// Node discovery manager
#[derive(Debug)]
pub struct NodeDiscovery {
    /// DNS seed hostnames
    dns_seeds: Vec<String>,

    /// Hardcoded bootstrap node addresses
    bootstrap_nodes: Vec<SocketAddr>,

    /// Discovered peers from gossip
    known_peers: HashSet<SocketAddr>,

    /// Health tracking for each node
    node_health: HashMap<SocketAddr, NodeHealth>,

    /// Last full discovery timestamp
    last_discovery: Option<Instant>,
}

impl Default for NodeDiscovery {
    fn default() -> Self {
        Self::new()
    }
}

impl NodeDiscovery {
    /// Create a new node discovery instance
    pub fn new() -> Self {
        let bootstrap_nodes = BOOTSTRAP_NODES
            .iter()
            .filter_map(|s| s.parse().ok())
            .collect();

        Self {
            dns_seeds: DNS_SEEDS.iter().map(|s| s.to_string()).collect(),
            bootstrap_nodes,
            known_peers: HashSet::new(),
            node_health: HashMap::new(),
            last_discovery: None,
        }
    }

    /// Add a custom bootstrap node
    pub fn add_bootstrap_node(&mut self, addr: SocketAddr) {
        self.bootstrap_nodes.push(addr);
    }

    /// Discover nodes using layered fallback strategy
    pub async fn discover(&mut self) -> Vec<SocketAddr> {
        let mut nodes = Vec::new();

        // 1. Try DNS seeds first (most up-to-date)
        info!("Querying DNS seeds...");
        for seed in &self.dns_seeds {
            match self.resolve_dns_seed(seed).await {
                Ok(addrs) => {
                    debug!("DNS seed {} returned {} addresses", seed, addrs.len());
                    nodes.extend(addrs);
                }
                Err(e) => {
                    debug!("DNS seed {} failed: {}", seed, e);
                }
            }
        }

        // 2. Fall back to hardcoded bootstrap nodes if DNS failed
        if nodes.is_empty() {
            info!("DNS seeds unavailable, using bootstrap nodes");
            nodes.extend(self.bootstrap_nodes.iter().cloned());
        }

        // 3. Include known good peers from previous sessions
        let healthy_peers: Vec<_> = self
            .known_peers
            .iter()
            .filter(|addr| {
                self.node_health
                    .get(*addr)
                    .map(|h| h.is_healthy())
                    .unwrap_or(true)
            })
            .cloned()
            .collect();

        debug!("Adding {} healthy known peers", healthy_peers.len());
        nodes.extend(healthy_peers);

        // Deduplicate
        let unique: HashSet<_> = nodes.into_iter().collect();
        let mut result: Vec<_> = unique.into_iter().collect();

        // Sort by health score
        result.sort_by(|a, b| {
            let score_a = self.node_health.get(a).map(|h| h.score()).unwrap_or(50);
            let score_b = self.node_health.get(b).map(|h| h.score()).unwrap_or(50);
            score_b.cmp(&score_a)
        });

        self.last_discovery = Some(Instant::now());

        info!("Discovered {} unique nodes", result.len());
        result
    }

    /// Resolve a DNS seed to IP addresses
    async fn resolve_dns_seed(&self, seed: &str) -> Result<Vec<SocketAddr>> {
        let host = format!("{}:{}", seed, DEFAULT_PORT);

        let addrs: Vec<SocketAddr> = tokio::time::timeout(
            PROBE_TIMEOUT,
            tokio::net::lookup_host(&host),
        )
        .await??
        .collect();

        Ok(addrs)
    }

    /// Add peers discovered from a connected node
    pub fn add_peers(&mut self, peers: &[SocketAddr]) {
        for peer in peers {
            self.known_peers.insert(*peer);
        }
    }

    /// Record a successful connection to a node
    pub fn record_success(&mut self, addr: SocketAddr, latency_ms: u32, block_height: u64) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let health = self.node_health.entry(addr).or_default();
        health.last_seen = Some(now);
        health.latency_ms = latency_ms;
        health.block_height = block_height;
        health.failures = 0;

        self.known_peers.insert(addr);
    }

    /// Record a failed connection attempt
    pub fn record_failure(&mut self, addr: SocketAddr) {
        let health = self.node_health.entry(addr).or_default();
        health.failures = health.failures.saturating_add(1);

        if health.failures >= 10 {
            warn!("Removing unreliable node: {}", addr);
            self.known_peers.remove(&addr);
            self.node_health.remove(&addr);
        }
    }

    /// Get the best nodes to connect to
    pub fn get_best_nodes(&self, count: usize) -> Vec<SocketAddr> {
        let mut nodes: Vec<_> = self.known_peers.iter().cloned().collect();

        nodes.sort_by(|a, b| {
            let score_a = self.node_health.get(a).map(|h| h.score()).unwrap_or(50);
            let score_b = self.node_health.get(b).map(|h| h.score()).unwrap_or(50);
            score_b.cmp(&score_a)
        });

        nodes.truncate(count);
        nodes
    }

    /// Get node health info
    pub fn get_health(&self, addr: &SocketAddr) -> Option<&NodeHealth> {
        self.node_health.get(addr)
    }

    /// Get all known peers
    pub fn known_peers(&self) -> &HashSet<SocketAddr> {
        &self.known_peers
    }

    /// Serialize discovery state for persistence
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let state = DiscoveryState {
            known_peers: self.known_peers.iter().map(|a| a.to_string()).collect(),
            node_health: self
                .node_health
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        };
        Ok(bincode::serialize(&state)?)
    }

    /// Deserialize discovery state
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        let state: DiscoveryState = bincode::deserialize(data)?;

        let known_peers = state
            .known_peers
            .iter()
            .filter_map(|s| s.parse().ok())
            .collect();

        let node_health = state
            .node_health
            .iter()
            .filter_map(|(k, v)| k.parse().ok().map(|addr| (addr, v.clone())))
            .collect();

        Ok(Self {
            dns_seeds: DNS_SEEDS.iter().map(|s| s.to_string()).collect(),
            bootstrap_nodes: BOOTSTRAP_NODES
                .iter()
                .filter_map(|s| s.parse().ok())
                .collect(),
            known_peers,
            node_health,
            last_discovery: None,
        })
    }
}

/// Serializable discovery state
#[derive(Serialize, Deserialize)]
struct DiscoveryState {
    known_peers: Vec<String>,
    node_health: HashMap<String, NodeHealth>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_health_score() {
        let mut health = NodeHealth::default();
        assert!(health.score() > 0);

        health.failures = 3;
        assert_eq!(health.score(), 0);
    }

    #[test]
    fn test_discovery_new() {
        let discovery = NodeDiscovery::new();
        assert!(!discovery.dns_seeds.is_empty());
    }

    #[test]
    fn test_record_success_and_failure() {
        let mut discovery = NodeDiscovery::new();
        let addr: SocketAddr = "127.0.0.1:8545".parse().unwrap();

        discovery.record_success(addr, 50, 1000);
        assert!(discovery.known_peers.contains(&addr));
        assert_eq!(discovery.node_health.get(&addr).unwrap().failures, 0);

        discovery.record_failure(addr);
        assert_eq!(discovery.node_health.get(&addr).unwrap().failures, 1);
    }
}
