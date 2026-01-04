// Copyright (c) 2024 Botho Foundation

//! Peer Exchange (PEX) protocol for decentralized peer discovery.
//!
//! PEX enables nodes to share known peer addresses with each other,
//! reducing reliance on centralized bootstrap nodes. Security measures
//! prevent eclipse attacks and spam.

use libp2p::{Multiaddr, PeerId};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    net::IpAddr,
    time::{Duration, Instant},
};
use tracing::{debug, warn};

/// Maximum peers to share per PEX message
pub const MAX_PEX_PEERS: usize = 8;

/// Minimum interval between PEX broadcasts (seconds)
pub const PEX_INTERVAL_SECS: u64 = 300;

/// Maximum age for a peer entry to be shared (24 hours)
pub const MAX_PEER_AGE_SECS: u64 = 86400;

/// Maximum PEX messages per peer per hour
pub const MAX_PEX_PER_HOUR: u32 = 12;

/// Maximum peers from the same /24 subnet
pub const MAX_PEERS_PER_SUBNET: usize = 3;

/// Size of the PEX message for DoS protection
pub const MAX_PEX_MESSAGE_SIZE: usize = 4096;

/// PEX message containing peer addresses to share
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PexMessage {
    /// Peer addresses being shared
    pub entries: Vec<PexEntry>,
    /// Sender's timestamp (unix epoch seconds)
    pub timestamp: u64,
}

impl PexMessage {
    /// Create a new PEX message with the current timestamp
    pub fn new(entries: Vec<PexEntry>) -> Self {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        Self { entries, timestamp }
    }

    /// Validate this message's structure
    pub fn is_valid(&self) -> bool {
        // Check entry count
        if self.entries.len() > MAX_PEX_PEERS {
            return false;
        }

        // Check timestamp is not in the future (with 5 min tolerance)
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        if self.timestamp > now + 300 {
            return false;
        }

        true
    }
}

/// Entry in a PEX message representing a known peer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PexEntry {
    /// Multiaddr of the peer (as string for serialization)
    pub addr: String,
    /// Unix timestamp when this peer was last successfully connected
    pub last_seen: u64,
}

impl PexEntry {
    /// Create a new PEX entry
    pub fn new(addr: Multiaddr, last_seen: u64) -> Self {
        Self {
            addr: addr.to_string(),
            last_seen,
        }
    }

    /// Parse the address string back to a Multiaddr
    pub fn multiaddr(&self) -> Option<Multiaddr> {
        self.addr.parse().ok()
    }

    /// Check if this entry is stale (older than MAX_PEER_AGE_SECS)
    pub fn is_stale(&self, current_time: u64) -> bool {
        current_time.saturating_sub(self.last_seen) > MAX_PEER_AGE_SECS
    }
}

/// Filters addresses for PEX sharing and validation
pub struct PexFilter {
    /// Whitelisted subnets (for testing)
    whitelisted_subnets: HashSet<String>,
}

impl Default for PexFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl PexFilter {
    /// Create a new PEX filter
    pub fn new() -> Self {
        Self {
            whitelisted_subnets: HashSet::new(),
        }
    }

    /// Create a filter with whitelisted subnets (for testing)
    pub fn with_whitelist(subnets: Vec<String>) -> Self {
        Self {
            whitelisted_subnets: subnets.into_iter().collect(),
        }
    }

    /// Check if an address is shareable via PEX
    ///
    /// Returns false for:
    /// - Loopback addresses (127.x.x.x, ::1)
    /// - Private/local addresses (10.x.x.x, 192.168.x.x, etc.)
    /// - Link-local addresses
    /// - Addresses without peer ID
    pub fn is_shareable(&self, addr: &Multiaddr) -> bool {
        // Must have a peer ID component
        if !self.has_peer_id(addr) {
            debug!(?addr, "Rejecting address without peer ID");
            return false;
        }

        // Extract IP and check if it's public
        match self.extract_ip(addr) {
            Some(ip) => {
                if self.is_private_or_local(&ip) {
                    // Check whitelist for testing
                    let subnet = self.ip_to_subnet(&ip);
                    if self.whitelisted_subnets.contains(&subnet) {
                        return true;
                    }
                    debug!(?addr, "Rejecting private/local address");
                    return false;
                }
                true
            }
            None => {
                debug!(?addr, "Rejecting address without IP");
                false
            }
        }
    }

    /// Validate a received PEX entry
    pub fn is_valid_entry(&self, entry: &PexEntry, current_time: u64) -> bool {
        // Check if stale
        if entry.is_stale(current_time) {
            debug!(addr = %entry.addr, "Rejecting stale PEX entry");
            return false;
        }

        // Parse and validate the address
        match entry.multiaddr() {
            Some(addr) => self.is_shareable(&addr),
            None => {
                debug!(addr = %entry.addr, "Rejecting unparseable address");
                false
            }
        }
    }

    /// Check if a Multiaddr has a peer ID component
    fn has_peer_id(&self, addr: &Multiaddr) -> bool {
        addr.iter()
            .any(|p| matches!(p, libp2p::multiaddr::Protocol::P2p(_)))
    }

    /// Extract IP address from a Multiaddr
    fn extract_ip(&self, addr: &Multiaddr) -> Option<IpAddr> {
        for protocol in addr.iter() {
            match protocol {
                libp2p::multiaddr::Protocol::Ip4(ip) => return Some(IpAddr::V4(ip)),
                libp2p::multiaddr::Protocol::Ip6(ip) => return Some(IpAddr::V6(ip)),
                _ => continue,
            }
        }
        None
    }

    /// Check if an IP is private, local, or loopback
    fn is_private_or_local(&self, ip: &IpAddr) -> bool {
        match ip {
            IpAddr::V4(ipv4) => {
                ipv4.is_loopback()
                    || ipv4.is_private()
                    || ipv4.is_link_local()
                    || ipv4.is_broadcast()
                    || ipv4.is_documentation()
                    || ipv4.is_unspecified()
            }
            IpAddr::V6(ipv6) => {
                ipv6.is_loopback() || ipv6.is_unspecified()
                    // Note: is_unicast_link_local and is_unique_local are unstable
                    // Check common private prefixes manually
                    || ipv6.segments()[0] == 0xfe80  // Link-local
                    || ipv6.segments()[0] & 0xfe00 == 0xfc00 // Unique local
                                                             // (fc00::/7)
            }
        }
    }

    /// Convert IP to /24 subnet string for tracking
    pub fn ip_to_subnet(&self, ip: &IpAddr) -> String {
        match ip {
            IpAddr::V4(ipv4) => {
                let octets = ipv4.octets();
                format!("{}.{}.{}.0/24", octets[0], octets[1], octets[2])
            }
            IpAddr::V6(ipv6) => {
                // Use /48 for IPv6
                let segments = ipv6.segments();
                format!("{:x}:{:x}:{:x}::/48", segments[0], segments[1], segments[2])
            }
        }
    }

    /// Extract subnet from a Multiaddr
    pub fn addr_to_subnet(&self, addr: &Multiaddr) -> Option<String> {
        self.extract_ip(addr).map(|ip| self.ip_to_subnet(&ip))
    }
}

/// Rate limiter for PEX messages
pub struct PexRateLimiter {
    /// Timestamp of PEX messages per peer (ring buffer of last hour)
    peer_pex_times: HashMap<PeerId, Vec<Instant>>,
    /// Maximum PEX messages per peer per hour
    max_per_hour: u32,
}

impl Default for PexRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl PexRateLimiter {
    /// Create a new rate limiter
    pub fn new() -> Self {
        Self {
            peer_pex_times: HashMap::new(),
            max_per_hour: MAX_PEX_PER_HOUR,
        }
    }

    /// Create with custom max rate
    pub fn with_max_rate(max_per_hour: u32) -> Self {
        Self {
            peer_pex_times: HashMap::new(),
            max_per_hour,
        }
    }

    /// Check if a peer is allowed to send a PEX message
    pub fn check_rate(&mut self, peer_id: &PeerId) -> bool {
        let now = Instant::now();
        let one_hour_ago = now - Duration::from_secs(3600);

        // Get or create entry for this peer
        let times = self.peer_pex_times.entry(*peer_id).or_default();

        // Remove old entries
        times.retain(|t| *t > one_hour_ago);

        // Check if under limit
        if times.len() >= self.max_per_hour as usize {
            warn!(%peer_id, "PEX rate limit exceeded");
            return false;
        }

        // Record this message
        times.push(now);
        true
    }

    /// Clean up old entries (call periodically)
    pub fn cleanup(&mut self) {
        let one_hour_ago = Instant::now() - Duration::from_secs(3600);

        self.peer_pex_times.retain(|_, times| {
            times.retain(|t| *t > one_hour_ago);
            !times.is_empty()
        });
    }

    /// Get the number of tracked peers
    pub fn tracked_peers(&self) -> usize {
        self.peer_pex_times.len()
    }
}

/// Tracks peer address sources for eclipse attack prevention
pub struct PexSourceTracker {
    /// How each peer was discovered
    peer_sources: HashMap<PeerId, PeerSource>,
    /// Count of peers per subnet (for diversity)
    subnet_counts: HashMap<String, usize>,
    /// Maximum peers per subnet
    max_per_subnet: usize,
    /// PEX filter for subnet extraction
    filter: PexFilter,
}

/// How a peer was discovered
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerSource {
    /// From bootstrap/seed nodes (trusted)
    Bootstrap,
    /// Discovered via PEX (less trusted)
    Pex,
    /// Direct user configuration
    Manual,
}

impl Default for PexSourceTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl PexSourceTracker {
    /// Create a new source tracker
    pub fn new() -> Self {
        Self {
            peer_sources: HashMap::new(),
            subnet_counts: HashMap::new(),
            max_per_subnet: MAX_PEERS_PER_SUBNET,
            filter: PexFilter::new(),
        }
    }

    /// Create with custom subnet limit
    pub fn with_subnet_limit(max_per_subnet: usize) -> Self {
        Self {
            peer_sources: HashMap::new(),
            subnet_counts: HashMap::new(),
            max_per_subnet,
            filter: PexFilter::new(),
        }
    }

    /// Record a peer with its discovery source
    pub fn record_peer(&mut self, peer_id: PeerId, addr: &Multiaddr, source: PeerSource) {
        // Track source
        self.peer_sources.insert(peer_id, source);

        // Track subnet
        if let Some(subnet) = self.filter.addr_to_subnet(addr) {
            *self.subnet_counts.entry(subnet).or_insert(0) += 1;
        }
    }

    /// Check if we should connect to a PEX-discovered peer
    ///
    /// Returns false if:
    /// - Too many peers from the same subnet (eclipse prevention)
    /// - Already have too many PEX-discovered peers
    pub fn should_connect(&self, addr: &Multiaddr) -> bool {
        // Check subnet diversity
        if let Some(subnet) = self.filter.addr_to_subnet(addr) {
            let current = self.subnet_counts.get(&subnet).copied().unwrap_or(0);
            if current >= self.max_per_subnet {
                debug!(%subnet, current, max = self.max_per_subnet, "Subnet limit reached");
                return false;
            }
        }

        true
    }

    /// Get the source of a peer
    pub fn get_source(&self, peer_id: &PeerId) -> Option<PeerSource> {
        self.peer_sources.get(peer_id).copied()
    }

    /// Remove a peer from tracking
    pub fn remove_peer(&mut self, peer_id: &PeerId, addr: &Multiaddr) {
        self.peer_sources.remove(peer_id);

        if let Some(subnet) = self.filter.addr_to_subnet(addr) {
            if let Some(count) = self.subnet_counts.get_mut(&subnet) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    self.subnet_counts.remove(&subnet);
                }
            }
        }
    }

    /// Count peers by source
    pub fn count_by_source(&self, source: PeerSource) -> usize {
        self.peer_sources.values().filter(|s| **s == source).count()
    }

    /// Get subnet counts for debugging
    pub fn subnet_counts(&self) -> &HashMap<String, usize> {
        &self.subnet_counts
    }
}

/// Complete PEX manager combining all components
pub struct PexManager {
    /// Address filter
    pub filter: PexFilter,
    /// Rate limiter
    pub rate_limiter: PexRateLimiter,
    /// Source tracker for eclipse prevention
    pub source_tracker: PexSourceTracker,
    /// Last time we broadcast PEX
    last_broadcast: Option<Instant>,
    /// Minimum interval between broadcasts
    broadcast_interval: Duration,
}

impl Default for PexManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PexManager {
    /// Create a new PEX manager
    pub fn new() -> Self {
        Self {
            filter: PexFilter::new(),
            rate_limiter: PexRateLimiter::new(),
            source_tracker: PexSourceTracker::new(),
            last_broadcast: None,
            broadcast_interval: Duration::from_secs(PEX_INTERVAL_SECS),
        }
    }

    /// Check if we should broadcast PEX now
    pub fn should_broadcast(&self) -> bool {
        match self.last_broadcast {
            Some(last) => last.elapsed() >= self.broadcast_interval,
            None => true,
        }
    }

    /// Record that we broadcast PEX
    pub fn record_broadcast(&mut self) {
        self.last_broadcast = Some(Instant::now());
    }

    /// Process an incoming PEX message
    ///
    /// Returns list of valid addresses to potentially connect to
    pub fn process_incoming(&mut self, peer_id: &PeerId, message: &PexMessage) -> Vec<Multiaddr> {
        // Check rate limit
        if !self.rate_limiter.check_rate(peer_id) {
            return vec![];
        }

        // Validate message structure
        if !message.is_valid() {
            warn!(%peer_id, "Invalid PEX message structure");
            return vec![];
        }

        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let mut valid_addrs = Vec::new();

        for entry in &message.entries {
            // Validate entry
            if !self.filter.is_valid_entry(entry, current_time) {
                continue;
            }

            // Parse address
            if let Some(addr) = entry.multiaddr() {
                // Check if we should connect (subnet diversity)
                if self.source_tracker.should_connect(&addr) {
                    valid_addrs.push(addr);
                }
            }
        }

        debug!(
            %peer_id,
            received = message.entries.len(),
            valid = valid_addrs.len(),
            "Processed PEX message"
        );

        valid_addrs
    }

    /// Prepare peers for PEX broadcast
    ///
    /// Takes list of (peer_id, address, last_seen) and returns a PEX message
    pub fn prepare_broadcast(
        &self,
        peers: impl IntoIterator<Item = (PeerId, Multiaddr, u64)>,
    ) -> Option<PexMessage> {
        let mut entries: Vec<PexEntry> = peers
            .into_iter()
            .filter(|(_, addr, _)| self.filter.is_shareable(addr))
            .map(|(_, addr, last_seen)| PexEntry::new(addr, last_seen))
            .collect();

        // Sort by recency (most recent first)
        entries.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));

        // Limit to MAX_PEX_PEERS
        entries.truncate(MAX_PEX_PEERS);

        if entries.is_empty() {
            None
        } else {
            Some(PexMessage::new(entries))
        }
    }

    /// Periodic cleanup
    pub fn cleanup(&mut self) {
        self.rate_limiter.cleanup();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // PexEntry tests
    // ========================================================================

    #[test]
    fn test_pex_entry_new() {
        let addr: Multiaddr = "/ip4/1.2.3.4/tcp/9000".parse().unwrap();
        let entry = PexEntry::new(addr.clone(), 1000);

        assert_eq!(entry.addr, addr.to_string());
        assert_eq!(entry.last_seen, 1000);
    }

    #[test]
    fn test_pex_entry_multiaddr() {
        let addr: Multiaddr = "/ip4/1.2.3.4/tcp/9000".parse().unwrap();
        let entry = PexEntry::new(addr.clone(), 1000);

        assert_eq!(entry.multiaddr(), Some(addr));
    }

    #[test]
    fn test_pex_entry_invalid_multiaddr() {
        let entry = PexEntry {
            addr: "not-a-valid-addr".to_string(),
            last_seen: 1000,
        };

        assert!(entry.multiaddr().is_none());
    }

    #[test]
    fn test_pex_entry_is_stale() {
        let entry = PexEntry {
            addr: "/ip4/1.2.3.4/tcp/9000".to_string(),
            last_seen: 1000,
        };

        // Entry from time 1000, current time 1000 + MAX_PEER_AGE_SECS = not stale
        assert!(!entry.is_stale(1000 + MAX_PEER_AGE_SECS));

        // Entry from time 1000, current time 1000 + MAX_PEER_AGE_SECS + 1 = stale
        assert!(entry.is_stale(1000 + MAX_PEER_AGE_SECS + 1));
    }

    // ========================================================================
    // PexMessage tests
    // ========================================================================

    #[test]
    fn test_pex_message_new() {
        let entries = vec![PexEntry {
            addr: "/ip4/1.2.3.4/tcp/9000".to_string(),
            last_seen: 1000,
        }];

        let msg = PexMessage::new(entries.clone());

        assert_eq!(msg.entries.len(), 1);
        assert!(msg.timestamp > 0);
    }

    #[test]
    fn test_pex_message_is_valid_empty() {
        let msg = PexMessage::new(vec![]);
        assert!(msg.is_valid());
    }

    #[test]
    fn test_pex_message_is_valid_too_many_entries() {
        let entries: Vec<PexEntry> = (0..MAX_PEX_PEERS + 1)
            .map(|i| PexEntry {
                addr: format!("/ip4/1.2.3.{}/tcp/9000", i),
                last_seen: 1000,
            })
            .collect();

        let msg = PexMessage {
            entries,
            timestamp: 0,
        };

        assert!(!msg.is_valid());
    }

    #[test]
    fn test_pex_message_is_valid_future_timestamp() {
        let future_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
            + 600; // 10 minutes in future

        let msg = PexMessage {
            entries: vec![],
            timestamp: future_time,
        };

        assert!(!msg.is_valid());
    }

    // ========================================================================
    // PexFilter tests
    // ========================================================================

    #[test]
    fn test_filter_rejects_loopback_ipv4() {
        let filter = PexFilter::new();
        let peer_id = PeerId::random();
        let addr: Multiaddr = format!("/ip4/127.0.0.1/tcp/9000/p2p/{}", peer_id)
            .parse()
            .unwrap();

        assert!(!filter.is_shareable(&addr));
    }

    #[test]
    fn test_filter_rejects_loopback_ipv6() {
        let filter = PexFilter::new();
        let peer_id = PeerId::random();
        let addr: Multiaddr = format!("/ip6/::1/tcp/9000/p2p/{}", peer_id)
            .parse()
            .unwrap();

        assert!(!filter.is_shareable(&addr));
    }

    #[test]
    fn test_filter_rejects_private_10() {
        let filter = PexFilter::new();
        let peer_id = PeerId::random();
        let addr: Multiaddr = format!("/ip4/10.0.0.1/tcp/9000/p2p/{}", peer_id)
            .parse()
            .unwrap();

        assert!(!filter.is_shareable(&addr));
    }

    #[test]
    fn test_filter_rejects_private_172() {
        let filter = PexFilter::new();
        let peer_id = PeerId::random();
        let addr: Multiaddr = format!("/ip4/172.16.0.1/tcp/9000/p2p/{}", peer_id)
            .parse()
            .unwrap();

        assert!(!filter.is_shareable(&addr));
    }

    #[test]
    fn test_filter_rejects_private_192() {
        let filter = PexFilter::new();
        let peer_id = PeerId::random();
        let addr: Multiaddr = format!("/ip4/192.168.1.1/tcp/9000/p2p/{}", peer_id)
            .parse()
            .unwrap();

        assert!(!filter.is_shareable(&addr));
    }

    #[test]
    fn test_filter_rejects_no_peer_id() {
        let filter = PexFilter::new();
        let addr: Multiaddr = "/ip4/8.8.8.8/tcp/9000".parse().unwrap();

        assert!(!filter.is_shareable(&addr));
    }

    #[test]
    fn test_filter_accepts_public_ipv4() {
        let filter = PexFilter::new();
        let peer_id = PeerId::random();
        let addr: Multiaddr = format!("/ip4/8.8.8.8/tcp/9000/p2p/{}", peer_id)
            .parse()
            .unwrap();

        assert!(filter.is_shareable(&addr));
    }

    #[test]
    fn test_filter_accepts_public_ipv6() {
        let filter = PexFilter::new();
        let peer_id = PeerId::random();
        let addr: Multiaddr = format!("/ip6/2001:4860:4860::8888/tcp/9000/p2p/{}", peer_id)
            .parse()
            .unwrap();

        assert!(filter.is_shareable(&addr));
    }

    #[test]
    fn test_filter_whitelist() {
        let filter = PexFilter::with_whitelist(vec!["192.168.1.0/24".to_string()]);
        let peer_id = PeerId::random();
        let addr: Multiaddr = format!("/ip4/192.168.1.1/tcp/9000/p2p/{}", peer_id)
            .parse()
            .unwrap();

        assert!(filter.is_shareable(&addr));
    }

    #[test]
    fn test_filter_ip_to_subnet_v4() {
        let filter = PexFilter::new();
        let ip: IpAddr = "192.168.1.100".parse().unwrap();

        assert_eq!(filter.ip_to_subnet(&ip), "192.168.1.0/24");
    }

    #[test]
    fn test_filter_ip_to_subnet_v6() {
        let filter = PexFilter::new();
        let ip: IpAddr = "2001:4860:4860::8888".parse().unwrap();

        assert_eq!(filter.ip_to_subnet(&ip), "2001:4860:4860::/48");
    }

    #[test]
    fn test_filter_valid_entry() {
        let filter = PexFilter::new();
        let peer_id = PeerId::random();
        let current_time = 2000u64;

        let entry = PexEntry {
            addr: format!("/ip4/8.8.8.8/tcp/9000/p2p/{}", peer_id),
            last_seen: current_time - 1000, // 1000 seconds ago
        };

        assert!(filter.is_valid_entry(&entry, current_time));
    }

    #[test]
    fn test_filter_rejects_stale_entry() {
        let filter = PexFilter::new();
        let peer_id = PeerId::random();
        let current_time = MAX_PEER_AGE_SECS + 2000;

        let entry = PexEntry {
            addr: format!("/ip4/8.8.8.8/tcp/9000/p2p/{}", peer_id),
            last_seen: 1000, // Very old
        };

        assert!(!filter.is_valid_entry(&entry, current_time));
    }

    // ========================================================================
    // PexRateLimiter tests
    // ========================================================================

    #[test]
    fn test_rate_limiter_allows_first_message() {
        let mut limiter = PexRateLimiter::new();
        let peer = PeerId::random();

        assert!(limiter.check_rate(&peer));
    }

    #[test]
    fn test_rate_limiter_allows_up_to_limit() {
        let mut limiter = PexRateLimiter::with_max_rate(3);
        let peer = PeerId::random();

        assert!(limiter.check_rate(&peer));
        assert!(limiter.check_rate(&peer));
        assert!(limiter.check_rate(&peer));
        assert!(!limiter.check_rate(&peer));
    }

    #[test]
    fn test_rate_limiter_independent_peers() {
        let mut limiter = PexRateLimiter::with_max_rate(1);
        let peer1 = PeerId::random();
        let peer2 = PeerId::random();

        assert!(limiter.check_rate(&peer1));
        assert!(!limiter.check_rate(&peer1));
        assert!(limiter.check_rate(&peer2)); // Different peer
    }

    #[test]
    fn test_rate_limiter_cleanup() {
        let mut limiter = PexRateLimiter::new();
        let peer = PeerId::random();

        limiter.check_rate(&peer);
        assert_eq!(limiter.tracked_peers(), 1);

        limiter.cleanup();
        // Still tracked because not old enough
        assert_eq!(limiter.tracked_peers(), 1);
    }

    // ========================================================================
    // PexSourceTracker tests
    // ========================================================================

    #[test]
    fn test_source_tracker_record_peer() {
        let mut tracker = PexSourceTracker::new();
        let peer = PeerId::random();
        let addr: Multiaddr = "/ip4/8.8.8.8/tcp/9000".parse().unwrap();

        tracker.record_peer(peer, &addr, PeerSource::Bootstrap);

        assert_eq!(tracker.get_source(&peer), Some(PeerSource::Bootstrap));
    }

    #[test]
    fn test_source_tracker_count_by_source() {
        let mut tracker = PexSourceTracker::new();
        let addr: Multiaddr = "/ip4/8.8.8.8/tcp/9000".parse().unwrap();

        tracker.record_peer(PeerId::random(), &addr, PeerSource::Bootstrap);
        tracker.record_peer(PeerId::random(), &addr, PeerSource::Pex);
        tracker.record_peer(PeerId::random(), &addr, PeerSource::Pex);

        assert_eq!(tracker.count_by_source(PeerSource::Bootstrap), 1);
        assert_eq!(tracker.count_by_source(PeerSource::Pex), 2);
    }

    #[test]
    fn test_source_tracker_subnet_limit() {
        let mut tracker = PexSourceTracker::with_subnet_limit(2);

        // Add two peers from same subnet
        let addr1: Multiaddr = "/ip4/8.8.8.1/tcp/9000".parse().unwrap();
        let addr2: Multiaddr = "/ip4/8.8.8.2/tcp/9000".parse().unwrap();
        let addr3: Multiaddr = "/ip4/8.8.8.3/tcp/9000".parse().unwrap();

        tracker.record_peer(PeerId::random(), &addr1, PeerSource::Pex);
        assert!(tracker.should_connect(&addr2));

        tracker.record_peer(PeerId::random(), &addr2, PeerSource::Pex);
        assert!(!tracker.should_connect(&addr3)); // Limit reached
    }

    #[test]
    fn test_source_tracker_different_subnets() {
        let mut tracker = PexSourceTracker::with_subnet_limit(1);

        let addr1: Multiaddr = "/ip4/8.8.8.1/tcp/9000".parse().unwrap();
        let addr2: Multiaddr = "/ip4/9.9.9.1/tcp/9000".parse().unwrap();

        tracker.record_peer(PeerId::random(), &addr1, PeerSource::Pex);
        assert!(tracker.should_connect(&addr2)); // Different subnet
    }

    #[test]
    fn test_source_tracker_remove_peer() {
        let mut tracker = PexSourceTracker::with_subnet_limit(1);
        let peer = PeerId::random();
        let addr: Multiaddr = "/ip4/8.8.8.1/tcp/9000".parse().unwrap();
        let addr2: Multiaddr = "/ip4/8.8.8.2/tcp/9000".parse().unwrap();

        tracker.record_peer(peer, &addr, PeerSource::Pex);
        assert!(!tracker.should_connect(&addr2)); // Limit reached

        tracker.remove_peer(&peer, &addr);
        assert!(tracker.should_connect(&addr2)); // Space freed
        assert!(tracker.get_source(&peer).is_none());
    }

    // ========================================================================
    // PexManager tests
    // ========================================================================

    #[test]
    fn test_manager_should_broadcast_initially() {
        let manager = PexManager::new();
        assert!(manager.should_broadcast());
    }

    #[test]
    fn test_manager_record_broadcast() {
        let mut manager = PexManager::new();
        manager.record_broadcast();
        assert!(!manager.should_broadcast());
    }

    #[test]
    fn test_manager_process_incoming_empty() {
        let mut manager = PexManager::new();
        let peer = PeerId::random();
        let msg = PexMessage::new(vec![]);

        let result = manager.process_incoming(&peer, &msg);
        assert!(result.is_empty());
    }

    #[test]
    fn test_manager_process_incoming_valid() {
        let mut manager = PexManager::new();
        let sender = PeerId::random();
        let target = PeerId::random();

        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let msg = PexMessage {
            entries: vec![PexEntry {
                addr: format!("/ip4/8.8.8.8/tcp/9000/p2p/{}", target),
                last_seen: current_time - 100,
            }],
            timestamp: current_time,
        };

        let result = manager.process_incoming(&sender, &msg);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_manager_process_incoming_rate_limited() {
        let mut manager = PexManager::new();
        manager.rate_limiter = PexRateLimiter::with_max_rate(1);

        let sender = PeerId::random();
        let msg = PexMessage::new(vec![]);

        // First message allowed
        manager.process_incoming(&sender, &msg);

        // Second message rate limited
        let result = manager.process_incoming(&sender, &msg);
        assert!(result.is_empty());
    }

    #[test]
    fn test_manager_prepare_broadcast_empty() {
        let manager = PexManager::new();
        let peers: Vec<(PeerId, Multiaddr, u64)> = vec![];

        assert!(manager.prepare_broadcast(peers).is_none());
    }

    #[test]
    fn test_manager_prepare_broadcast_filters_private() {
        let manager = PexManager::new();
        let peer = PeerId::random();
        let addr: Multiaddr = format!("/ip4/192.168.1.1/tcp/9000/p2p/{}", peer)
            .parse()
            .unwrap();

        let peers = vec![(peer, addr, 1000)];
        assert!(manager.prepare_broadcast(peers).is_none());
    }

    #[test]
    fn test_manager_prepare_broadcast_valid() {
        let manager = PexManager::new();
        let peer = PeerId::random();
        let addr: Multiaddr = format!("/ip4/8.8.8.8/tcp/9000/p2p/{}", peer)
            .parse()
            .unwrap();

        let peers = vec![(peer, addr, 1000)];
        let msg = manager.prepare_broadcast(peers);

        assert!(msg.is_some());
        assert_eq!(msg.unwrap().entries.len(), 1);
    }

    #[test]
    fn test_manager_prepare_broadcast_limits_entries() {
        let manager = PexManager::new();

        let peers: Vec<(PeerId, Multiaddr, u64)> = (0..20)
            .map(|i| {
                let peer = PeerId::random();
                let addr: Multiaddr = format!("/ip4/8.8.{}.1/tcp/9000/p2p/{}", i, peer)
                    .parse()
                    .unwrap();
                (peer, addr, 1000 + i as u64)
            })
            .collect();

        let msg = manager.prepare_broadcast(peers).unwrap();
        assert_eq!(msg.entries.len(), MAX_PEX_PEERS);
    }

    #[test]
    fn test_manager_prepare_broadcast_sorts_by_recency() {
        let manager = PexManager::new();

        let peer1 = PeerId::random();
        let peer2 = PeerId::random();
        let addr1: Multiaddr = format!("/ip4/8.8.8.1/tcp/9000/p2p/{}", peer1)
            .parse()
            .unwrap();
        let addr2: Multiaddr = format!("/ip4/8.8.8.2/tcp/9000/p2p/{}", peer2)
            .parse()
            .unwrap();

        let peers = vec![(peer1, addr1, 1000), (peer2, addr2.clone(), 2000)];

        let msg = manager.prepare_broadcast(peers).unwrap();

        // Most recent (2000) should be first
        assert_eq!(msg.entries[0].addr, addr2.to_string());
    }

    #[test]
    fn test_default_impls() {
        // Ensure all Default impls work
        let _ = PexFilter::default();
        let _ = PexRateLimiter::default();
        let _ = PexSourceTracker::default();
        let _ = PexManager::default();
    }
}
