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
        // These caps must comfortably exceed the message rate that honest
        // multi-node SCP consensus produces, while remaining far below a
        // genuine flood. A single SCP slot is not one message: nomination
        // rounds plus the ballot protocol (PREPARE / CONFIRM / EXTERNALIZE),
        // each potentially rebroadcast on timeout, plus a per-slot minting tx.
        // A conservative bound is ~10-30 consensus messages per slot per peer
        // during active rounds. At the *default* slot duration (20s) that is
        // tiny, but the same binary is run with `BOTHO_SLOT_DURATION_SECS=1`
        // in tests, where it is ~600-1800 consensus msgs/min/peer. The old
        // fixed caps (50/min consensus, 100/min tx) silently dropped honest
        // SCP/minting traffic, kept nodes in solo mode, and caused forks
        // (issue #413). We size the defaults for the fast-slot, multi-peer
        // case and rely on `for_slot_duration` to scale further when the
        // effective slot duration / quorum size is known.
        Self::for_slot_duration(DEFAULT_SLOT_DURATION_SECS, DEFAULT_QUORUM_PEERS)
    }
}

/// Default slot duration (seconds) assumed when sizing rate limits without an
/// explicit value. Matches `ConsensusConfig::default().slot_duration`.
const DEFAULT_SLOT_DURATION_SECS: u64 = 20;

/// Default quorum size assumed when sizing rate limits without an explicit
/// value. Small testnets run 2-of-2; we leave generous headroom.
const DEFAULT_QUORUM_PEERS: u32 = 8;

/// Estimated number of consensus (SCP) messages a single peer broadcasts per
/// slot during active nomination + ballot rounds (with rebroadcast headroom).
const CONSENSUS_MSGS_PER_SLOT: u32 = 30;

/// Estimated number of transaction (incl. minting tx) messages a single peer
/// broadcasts per slot.
const TX_MSGS_PER_SLOT: u32 = 8;

/// Multiplicative safety headroom applied to derived rate limits so transient
/// bursts (e.g. several rebroadcasts colliding) never trip honest traffic.
const RATE_LIMIT_HEADROOM: u32 = 4;

impl MessageTypeLimits {
    /// Derive per-type per-minute caps from the effective slot duration and the
    /// quorum size. The consensus/transaction caps scale with `peers` and with
    /// `60 / slot_secs`, so a faster slot or a larger quorum raises the ceiling
    /// instead of silently dropping honest consensus traffic.
    ///
    /// The result is still a *flood ceiling*: a genuinely abusive peer sending
    /// orders of magnitude more than the honest cadence is still caught.
    pub fn for_slot_duration(slot_secs: u64, peers: u32) -> Self {
        let slot_secs = slot_secs.max(1);
        // Slots per minute at this cadence: a 1s slot yields 60, a 20s slot
        // yields 3. Floored at 1 so very long slots still get a sane cap.
        let slots_per_min = (60 / slot_secs).max(1) as u32;
        let peers = peers.max(1);

        // consensus_per_minute = msgs_per_slot * slots_per_min * peers * headroom
        let consensus_per_minute = CONSENSUS_MSGS_PER_SLOT
            .saturating_mul(slots_per_min)
            .saturating_mul(peers)
            .saturating_mul(RATE_LIMIT_HEADROOM);
        let transactions_per_minute = TX_MSGS_PER_SLOT
            .saturating_mul(slots_per_min)
            .saturating_mul(peers)
            .saturating_mul(RATE_LIMIT_HEADROOM);

        Self {
            transactions_per_minute,
            blocks_per_minute: 60,
            consensus_per_minute,
            announcements_per_minute: 60,
        }
    }
}

impl Default for PeerRateLimitConfig {
    fn default() -> Self {
        Self::for_slot_duration(DEFAULT_SLOT_DURATION_SECS, DEFAULT_QUORUM_PEERS)
    }
}

impl PeerRateLimitConfig {
    /// Build a rate-limit config sized for the given effective slot duration
    /// and quorum size.
    ///
    /// Both the per-type caps *and* the global gates
    /// (`max_messages_per_second`, `burst_limit`) are derived together. The
    /// original bug (issue #413) was that the global gates (10 msg/s, 50
    /// msg/5s) trip *before* the per-type caps, so raising only the
    /// per-type caps still dropped honest SCP/minting traffic. Here the
    /// global gates are scaled from the same slot-rate model with extra
    /// headroom so they sit above the honest aggregate rate while
    /// still tripping for a true flood.
    pub fn for_slot_duration(slot_secs: u64, peers: u32) -> Self {
        let limits = MessageTypeLimits::for_slot_duration(slot_secs, peers);

        let peers = peers.max(1);

        // Aggregate honest rate across all types, per minute, then convert to a
        // per-second global ceiling with generous headroom.
        let aggregate_per_min = limits
            .consensus_per_minute
            .saturating_add(limits.transactions_per_minute)
            .saturating_add(limits.blocks_per_minute)
            .saturating_add(limits.announcements_per_minute);

        // A full slot's worth of consensus+tx messages from all peers can
        // legitimately cluster inside a single second (e.g. an active ballot
        // round with rebroadcasts), even at long slot durations. The global
        // per-second gate must sit above that instantaneous cluster, not just
        // above the time-averaged aggregate, otherwise it trips before the
        // per-type caps (the core #413 failure). Take the larger of:
        //   - the per-slot instantaneous cluster across peers, with headroom
        //   - the time-averaged aggregate per second, with headroom
        let per_slot_cluster = CONSENSUS_MSGS_PER_SLOT
            .saturating_add(TX_MSGS_PER_SLOT)
            .saturating_mul(peers)
            .saturating_mul(RATE_LIMIT_HEADROOM);
        let averaged_per_second = ((aggregate_per_min / 60) + 1).saturating_mul(2);
        let max_messages_per_second = per_slot_cluster.max(averaged_per_second).max(50);
        // Burst limit over a 5s window: 5s of the per-second ceiling, with
        // headroom for rebroadcast collisions.
        let burst_limit = max_messages_per_second.saturating_mul(5).max(250);

        Self {
            max_messages_per_second,
            burst_limit,
            burst_window_ms: 5000, // 5 second window
            // Raised from 3: a busy honest peer can momentarily exceed a cap
            // during a rebroadcast storm. Combined with violation decay (see
            // `PeerRateState::decay_violations`) this avoids permanently
            // blacklisting an honest peer after a transient burst, while a
            // sustained abuser still accrues violations faster than they decay.
            disconnect_threshold: 10,
            enabled: true,
            message_limits: limits,
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
