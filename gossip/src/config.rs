// Copyright (c) 2024 Botho Foundation

//! Configuration for the gossip service.

use crate::store::PeerStoreConfig;
use libp2p::Multiaddr;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Network identifier for protocol version matching.
/// Peers with different network IDs will be disconnected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum NetworkId {
    /// Production mainnet
    Mainnet,
    /// Test network (default during beta)
    #[default]
    Testnet,
}


impl NetworkId {
    /// Get the protocol version string for this network.
    /// Format: /botho/{network}/1.0.0
    pub fn protocol_version(&self) -> String {
        match self {
            NetworkId::Mainnet => "/botho/mainnet/1.0.0".to_string(),
            NetworkId::Testnet => "/botho/testnet/1.0.0".to_string(),
        }
    }

    /// Check if a protocol version matches this network.
    pub fn matches_protocol(&self, protocol: &str) -> bool {
        protocol == self.protocol_version()
    }
}

impl std::fmt::Display for NetworkId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetworkId::Mainnet => write!(f, "mainnet"),
            NetworkId::Testnet => write!(f, "testnet"),
        }
    }
}

/// Configuration for the gossip service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GossipConfig {
    /// Network identifier (mainnet or testnet)
    /// Peers with mismatched network IDs are disconnected.
    #[serde(default)]
    pub network_id: NetworkId,

    /// Port to listen on for libp2p connections
    pub listen_port: u16,

    /// Bootstrap peers to connect to on startup (libp2p multiaddrs)
    pub bootstrap_peers: Vec<Multiaddr>,

    /// How often to broadcast our own announcement (seconds)
    pub announce_interval_secs: u64,

    /// How often to request topology updates from peers (seconds)
    pub sync_interval_secs: u64,

    /// How often to run peer exchange (seconds)
    pub peer_exchange_interval_secs: u64,

    /// Maximum number of peers to connect to
    pub max_connections: usize,

    /// Maximum number of announcements to request in one batch
    pub max_batch_size: u32,

    /// Peer store configuration
    pub store_config: PeerStoreConfig,

    /// Enable gossipsub for push-based announcement propagation
    pub enable_gossipsub: bool,

    /// Enable Kademlia DHT for peer discovery
    pub enable_kademlia: bool,

    /// Timeout for request-response operations
    pub request_timeout_secs: u64,

    /// Whether to accept announcements from unknown peers
    pub accept_unknown_peers: bool,

    /// Minimum number of peers before we consider ourselves bootstrapped
    pub min_peers_for_bootstrap: usize,

    /// Per-peer rate limiting configuration
    pub rate_limit: PeerRateLimitConfig,
}

/// Configuration for per-peer gossipsub rate limiting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerRateLimitConfig {
    /// Maximum messages per second per peer before throttling (global fallback)
    pub max_messages_per_second: u32,

    /// Maximum messages in burst window before throttling
    pub burst_limit: u32,

    /// Burst window duration in milliseconds
    pub burst_window_ms: u64,

    /// Number of rate limit violations before disconnecting peer
    pub disconnect_threshold: u32,

    /// Whether to enable per-peer rate limiting
    pub enabled: bool,

    /// Per-message-type rate limits
    pub message_limits: MessageTypeLimits,
}

/// Rate limits per message type (messages per minute).
///
/// These limits protect against flooding attacks on specific message types.
/// Each message type has different network impact and expected frequency.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageTypeLimits {
    /// Maximum transaction announcements per minute per peer.
    /// Transactions are frequent but lightweight.
    pub transactions_per_minute: u32,

    /// Maximum block announcements per minute per peer.
    /// Blocks are infrequent but important.
    pub blocks_per_minute: u32,

    /// Maximum consensus messages per minute per peer.
    /// SCP messages are critical for consensus but should be bounded.
    pub consensus_per_minute: u32,

    /// Maximum topology/announcement messages per minute per peer.
    /// Discovery messages are periodic and bounded.
    pub announcements_per_minute: u32,
}

impl Default for MessageTypeLimits {
    fn default() -> Self {
        Self {
            // From audit requirements:
            // - Transaction announcements: 100/min
            // - Block announcements: 10/min
            // - Consensus messages: 50/min
            transactions_per_minute: 100,
            blocks_per_minute: 10,
            consensus_per_minute: 50,
            announcements_per_minute: 20,
        }
    }
}

impl Default for PeerRateLimitConfig {
    fn default() -> Self {
        Self {
            max_messages_per_second: 10,
            burst_limit: 50,
            burst_window_ms: 5000, // 5 second window
            disconnect_threshold: 3,
            enabled: true,
            message_limits: MessageTypeLimits::default(),
        }
    }
}

impl Default for GossipConfig {
    fn default() -> Self {
        Self {
            network_id: NetworkId::default(),
            listen_port: 7100,
            bootstrap_peers: Vec::new(),
            announce_interval_secs: 300,      // 5 minutes
            sync_interval_secs: 60,           // 1 minute
            peer_exchange_interval_secs: 120, // 2 minutes
            max_connections: 50,
            max_batch_size: 100,
            store_config: PeerStoreConfig::default(),
            enable_gossipsub: true,
            enable_kademlia: true,
            request_timeout_secs: 30,
            accept_unknown_peers: true,
            min_peers_for_bootstrap: 3,
            rate_limit: PeerRateLimitConfig::default(),
        }
    }
}

impl GossipConfig {
    /// Create a new config with the given listen port.
    pub fn with_port(port: u16) -> Self {
        Self {
            listen_port: port,
            ..Default::default()
        }
    }

    /// Add bootstrap peers.
    pub fn with_bootstrap_peers(mut self, peers: Vec<Multiaddr>) -> Self {
        self.bootstrap_peers = peers;
        self
    }

    /// Set the announce interval.
    pub fn with_announce_interval(mut self, secs: u64) -> Self {
        self.announce_interval_secs = secs;
        self
    }

    /// Get the announce interval as a Duration.
    pub fn announce_interval(&self) -> Duration {
        Duration::from_secs(self.announce_interval_secs)
    }

    /// Get the sync interval as a Duration.
    pub fn sync_interval(&self) -> Duration {
        Duration::from_secs(self.sync_interval_secs)
    }

    /// Get the peer exchange interval as a Duration.
    pub fn peer_exchange_interval(&self) -> Duration {
        Duration::from_secs(self.peer_exchange_interval_secs)
    }

    /// Get the request timeout as a Duration.
    pub fn request_timeout(&self) -> Duration {
        Duration::from_secs(self.request_timeout_secs)
    }

    /// Check if we have any bootstrap peers configured.
    pub fn has_bootstrap_peers(&self) -> bool {
        !self.bootstrap_peers.is_empty()
    }

    /// Get the listen multiaddr.
    pub fn listen_multiaddr(&self) -> Multiaddr {
        format!("/ip4/0.0.0.0/tcp/{}", self.listen_port)
            .parse()
            .expect("valid multiaddr")
    }
}

/// Builder for GossipConfig.
#[derive(Debug, Default)]
pub struct GossipConfigBuilder {
    config: GossipConfig,
}

impl GossipConfigBuilder {
    /// Create a new builder with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the network ID.
    pub fn network_id(mut self, id: NetworkId) -> Self {
        self.config.network_id = id;
        self
    }

    /// Set the listen port.
    pub fn listen_port(mut self, port: u16) -> Self {
        self.config.listen_port = port;
        self
    }

    /// Add a bootstrap peer.
    pub fn add_bootstrap_peer(mut self, peer: Multiaddr) -> Self {
        self.config.bootstrap_peers.push(peer);
        self
    }

    /// Set bootstrap peers.
    pub fn bootstrap_peers(mut self, peers: Vec<Multiaddr>) -> Self {
        self.config.bootstrap_peers = peers;
        self
    }

    /// Set the announce interval in seconds.
    pub fn announce_interval_secs(mut self, secs: u64) -> Self {
        self.config.announce_interval_secs = secs;
        self
    }

    /// Set the sync interval in seconds.
    pub fn sync_interval_secs(mut self, secs: u64) -> Self {
        self.config.sync_interval_secs = secs;
        self
    }

    /// Set max connections.
    pub fn max_connections(mut self, max: usize) -> Self {
        self.config.max_connections = max;
        self
    }

    /// Enable or disable gossipsub.
    pub fn enable_gossipsub(mut self, enable: bool) -> Self {
        self.config.enable_gossipsub = enable;
        self
    }

    /// Enable or disable Kademlia.
    pub fn enable_kademlia(mut self, enable: bool) -> Self {
        self.config.enable_kademlia = enable;
        self
    }

    /// Set the store config.
    pub fn store_config(mut self, config: PeerStoreConfig) -> Self {
        self.config.store_config = config;
        self
    }

    /// Build the config.
    pub fn build(self) -> GossipConfig {
        self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_network_id_default() {
        assert_eq!(NetworkId::default(), NetworkId::Testnet);
    }

    #[test]
    fn test_network_id_protocol_version() {
        assert_eq!(
            NetworkId::Mainnet.protocol_version(),
            "/botho/mainnet/1.0.0"
        );
        assert_eq!(
            NetworkId::Testnet.protocol_version(),
            "/botho/testnet/1.0.0"
        );
    }

    #[test]
    fn test_network_id_matches_protocol() {
        assert!(NetworkId::Mainnet.matches_protocol("/botho/mainnet/1.0.0"));
        assert!(!NetworkId::Mainnet.matches_protocol("/botho/testnet/1.0.0"));
        assert!(NetworkId::Testnet.matches_protocol("/botho/testnet/1.0.0"));
        assert!(!NetworkId::Testnet.matches_protocol("/botho/mainnet/1.0.0"));
        // Old protocol version should not match either
        assert!(!NetworkId::Testnet.matches_protocol("/botho/1.0.0"));
    }

    #[test]
    fn test_default_config() {
        let config = GossipConfig::default();
        assert_eq!(config.network_id, NetworkId::Testnet);
        assert_eq!(config.listen_port, 7100);
        assert!(config.enable_gossipsub);
        assert!(config.enable_kademlia);
        assert!(config.bootstrap_peers.is_empty());
    }

    #[test]
    fn test_config_builder() {
        let config = GossipConfigBuilder::new()
            .listen_port(8000)
            .announce_interval_secs(600)
            .max_connections(100)
            .enable_kademlia(false)
            .build();

        assert_eq!(config.listen_port, 8000);
        assert_eq!(config.announce_interval_secs, 600);
        assert_eq!(config.max_connections, 100);
        assert!(!config.enable_kademlia);
    }

    #[test]
    fn test_listen_multiaddr() {
        let config = GossipConfig::with_port(7200);
        let addr = config.listen_multiaddr();
        assert_eq!(addr.to_string(), "/ip4/0.0.0.0/tcp/7200");
    }

    #[test]
    fn test_intervals() {
        let config = GossipConfig::default();
        assert_eq!(config.announce_interval(), Duration::from_secs(300));
        assert_eq!(config.sync_interval(), Duration::from_secs(60));
    }
}
