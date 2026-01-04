// Copyright (c) 2024 Botho Foundation

//! Circuit hop selection with diversity requirements.
//!
//! This module implements secure peer selection for onion gossip circuits.
//! The selection algorithm ensures:
//!
//! - **Subnet Diversity**: No two hops in the same /16 subnet
//! - **Weighted Selection**: Higher relay scores increase selection probability
//! - **Minimum Thresholds**: Only peers meeting quality requirements are
//!   considered
//! - **Non-determinism**: Random selection prevents prediction by adversaries
//!
//! # Security Properties
//!
//! - Adversary controlling one subnet cannot control entire circuit
//! - High-score nodes preferred but not guaranteed (prevents gaming)
//! - Selection is non-deterministic (prevents prediction)
//!
//! # Example
//!
//! ```
//! use botho::network::privacy::{
//!     CircuitSelector, SelectionConfig, RelayPeerInfo, RelayCapacity, NatType,
//! };
//! use libp2p::PeerId;
//! use std::net::Ipv4Addr;
//!
//! // Create selector with default config
//! let selector = CircuitSelector::new(SelectionConfig::default());
//!
//! // Create some test peers
//! let peers = vec![
//!     RelayPeerInfo::new(
//!         PeerId::random(),
//!         Some(Ipv4Addr::new(10, 0, 1, 1)),
//!         RelayCapacity::default(),
//!     ),
//!     RelayPeerInfo::new(
//!         PeerId::random(),
//!         Some(Ipv4Addr::new(10, 1, 1, 1)),
//!         RelayCapacity::default(),
//!     ),
//!     RelayPeerInfo::new(
//!         PeerId::random(),
//!         Some(Ipv4Addr::new(10, 2, 1, 1)),
//!         RelayCapacity::default(),
//!     ),
//! ];
//!
//! // Select 3 diverse hops
//! let hops = selector.select_diverse_hops(&peers, 3).unwrap();
//! assert_eq!(hops.len(), 3);
//! ```
//!
//! # References
//!
//! - Design doc: `docs/design/traffic-privacy-roadmap.md` (Section 1.3)
//! - Tor path selection: <https://spec.torproject.org/path-spec>

use bth_gossip::RelayCapacity;
use libp2p::PeerId;
use rand::{seq::SliceRandom, Rng};
use std::{collections::HashSet, net::Ipv4Addr};
use thiserror::Error;

/// Errors that can occur during circuit hop selection.
#[derive(Debug, Error)]
pub enum SelectionError {
    /// Not enough peers available to build a circuit.
    #[error("insufficient peers: need {needed}, have {available}")]
    InsufficientPeers {
        /// Number of peers needed
        needed: usize,
        /// Number of peers available
        available: usize,
    },

    /// Not enough peers with different subnets.
    #[error("insufficient diversity: need {needed} unique subnets, found {found}")]
    InsufficientDiversity {
        /// Number of diverse hops needed
        needed: usize,
        /// Number of unique subnets found
        found: usize,
    },

    /// No peers meet the minimum relay score threshold.
    #[error("no peers meet minimum relay score {min_score}")]
    NoQualifiedPeers {
        /// Minimum score required
        min_score: f64,
    },
}

/// Peer information for relay selection.
///
/// Combines peer identity with relay capacity and network location
/// information needed for diverse hop selection.
#[derive(Debug, Clone)]
pub struct RelayPeerInfo {
    /// The peer's libp2p identity.
    pub peer_id: PeerId,

    /// The peer's IP address (if known).
    ///
    /// Used for subnet diversity checking. If unknown, the peer
    /// will be treated as having a unique subnet.
    pub ip_addr: Option<Ipv4Addr>,

    /// The peer's relay capacity.
    pub relay_capacity: RelayCapacity,
}

impl RelayPeerInfo {
    /// Create new relay peer info.
    pub fn new(peer_id: PeerId, ip_addr: Option<Ipv4Addr>, relay_capacity: RelayCapacity) -> Self {
        Self {
            peer_id,
            ip_addr,
            relay_capacity,
        }
    }

    /// Get the relay score for this peer.
    #[inline]
    pub fn relay_score(&self) -> f64 {
        self.relay_capacity.relay_score()
    }

    /// Get the /16 subnet prefix for diversity checking.
    ///
    /// Returns the first two octets of the IPv4 address as a u16,
    /// or `None` if no IP address is known.
    ///
    /// # Example
    ///
    /// For IP `192.168.1.100`, returns `Some(49320)` (0xC0A8 = 192.168).
    pub fn subnet_prefix(&self) -> Option<u16> {
        self.ip_addr.map(|ip| {
            let octets = ip.octets();
            ((octets[0] as u16) << 8) | (octets[1] as u16)
        })
    }
}

/// Configuration for circuit hop selection.
#[derive(Debug, Clone)]
pub struct SelectionConfig {
    /// Minimum relay score required for selection (0.0 - 1.0).
    pub min_relay_score: f64,

    /// Maximum selection attempts before giving up.
    pub max_attempts: usize,

    /// Whether to allow peers without known IP addresses.
    ///
    /// If true, peers without IP addresses are treated as having
    /// unique subnets for diversity purposes.
    pub allow_unknown_ip: bool,

    /// Whether to strictly enforce subnet diversity.
    ///
    /// If false, will fall back to selecting without diversity
    /// when insufficient diverse peers are available.
    pub strict_diversity: bool,
}

impl Default for SelectionConfig {
    fn default() -> Self {
        Self {
            min_relay_score: 0.2,
            max_attempts: 100,
            allow_unknown_ip: true,
            strict_diversity: true,
        }
    }
}

/// Circuit hop selector implementing diversity requirements.
///
/// Selects relay hops using weighted random selection with subnet
/// diversity constraints. The selector ensures:
///
/// 1. All selected hops meet minimum relay score
/// 2. No two hops share the same /16 subnet
/// 3. Higher-score peers are more likely to be selected
/// 4. Selection is non-deterministic to prevent prediction
pub struct CircuitSelector {
    config: SelectionConfig,
}

impl CircuitSelector {
    /// Create a new circuit selector with the given configuration.
    pub fn new(config: SelectionConfig) -> Self {
        Self { config }
    }

    /// Create a new selector with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(SelectionConfig::default())
    }

    /// Get the selector configuration.
    pub fn config(&self) -> &SelectionConfig {
        &self.config
    }

    /// Select diverse hops for a circuit.
    ///
    /// # Arguments
    ///
    /// * `peers` - Available peers to select from
    /// * `count` - Number of hops to select
    ///
    /// # Returns
    ///
    /// A vector of selected peer IDs on success, or an error if
    /// selection requirements cannot be met.
    ///
    /// # Algorithm
    ///
    /// 1. Filter peers by minimum relay score
    /// 2. Repeatedly: a. Filter out already-selected peers and used subnets b.
    ///    Select one peer using weighted random selection c. Add to selected
    ///    list and mark subnet as used
    /// 3. Return selected peers or error if insufficient diversity
    pub fn select_diverse_hops(
        &self,
        peers: &[RelayPeerInfo],
        count: usize,
    ) -> Result<Vec<PeerId>, SelectionError> {
        // Filter by minimum relay score
        let qualified: Vec<_> = peers
            .iter()
            .filter(|p| p.relay_score() >= self.config.min_relay_score)
            .collect();

        if qualified.is_empty() {
            return Err(SelectionError::NoQualifiedPeers {
                min_score: self.config.min_relay_score,
            });
        }

        if qualified.len() < count {
            return Err(SelectionError::InsufficientPeers {
                needed: count,
                available: qualified.len(),
            });
        }

        let mut selected = Vec::with_capacity(count);
        let mut used_subnets: HashSet<Option<u16>> = HashSet::new();
        let mut used_peer_ids: HashSet<PeerId> = HashSet::new();
        let mut rng = rand::thread_rng();
        let mut attempts = 0;

        while selected.len() < count && attempts < self.config.max_attempts {
            attempts += 1;

            // Find candidates not already selected and not in used subnets
            let candidates: Vec<&RelayPeerInfo> = qualified
                .iter()
                .filter(|p| !used_peer_ids.contains(&p.peer_id))
                .filter(|p| {
                    let subnet = p.subnet_prefix();
                    // Allow unknown IPs if configured, otherwise require diversity
                    if subnet.is_none() && self.config.allow_unknown_ip {
                        true // Unknown IPs treated as unique
                    } else {
                        !used_subnets.contains(&subnet)
                    }
                })
                .copied()
                .collect();

            if candidates.is_empty() {
                break;
            }

            // Weighted random selection by relay score
            if let Some(peer) = weighted_random_select(&candidates, &mut rng) {
                let subnet = peer.subnet_prefix();
                used_subnets.insert(subnet);
                used_peer_ids.insert(peer.peer_id);
                selected.push(peer.peer_id);
            }
        }

        // Check if we got enough hops
        if selected.len() < count {
            if self.config.strict_diversity {
                return Err(SelectionError::InsufficientDiversity {
                    needed: count,
                    found: selected.len(),
                });
            }

            // Non-strict mode: try to fill remaining slots without diversity
            let remaining = count - selected.len();
            let additional: Vec<&RelayPeerInfo> = qualified
                .iter()
                .filter(|p| !used_peer_ids.contains(&p.peer_id))
                .copied()
                .collect();

            for peer in additional.into_iter().take(remaining) {
                used_peer_ids.insert(peer.peer_id);
                selected.push(peer.peer_id);
            }

            if selected.len() < count {
                return Err(SelectionError::InsufficientPeers {
                    needed: count,
                    available: selected.len(),
                });
            }
        }

        Ok(selected)
    }

    /// Check if two peers are in the same /16 subnet.
    pub fn same_subnet(a: &RelayPeerInfo, b: &RelayPeerInfo) -> bool {
        match (a.subnet_prefix(), b.subnet_prefix()) {
            (Some(a_subnet), Some(b_subnet)) => a_subnet == b_subnet,
            // If either is unknown, treat as different subnets
            _ => false,
        }
    }

    /// Check if a set of peers are all in different subnets.
    pub fn are_diverse(peers: &[&RelayPeerInfo]) -> bool {
        let mut seen_subnets: HashSet<u16> = HashSet::new();

        for peer in peers {
            if let Some(subnet) = peer.subnet_prefix() {
                if !seen_subnets.insert(subnet) {
                    return false; // Duplicate subnet
                }
            }
            // Unknown IPs are treated as unique
        }

        true
    }
}

/// Perform weighted random selection from candidates.
///
/// Each candidate is selected with probability proportional to their
/// relay score. This prevents deterministic selection which could be
/// gamed by adversaries.
fn weighted_random_select<'a, R: Rng>(
    candidates: &[&'a RelayPeerInfo],
    rng: &mut R,
) -> Option<&'a RelayPeerInfo> {
    if candidates.is_empty() {
        return None;
    }

    // Calculate total weight
    let total_weight: f64 = candidates.iter().map(|p| p.relay_score()).sum();

    if total_weight <= 0.0 {
        // Fallback to uniform random if all weights are zero
        return candidates.choose(rng).copied();
    }

    // Generate random value in [0, total_weight)
    let mut value = rng.gen_range(0.0..total_weight);

    // Find the selected candidate
    for candidate in candidates {
        value -= candidate.relay_score();
        if value <= 0.0 {
            return Some(candidate);
        }
    }

    // Fallback to last candidate (shouldn't happen with proper weights)
    candidates.last().copied()
}

/// Extract IPv4 address from endpoint URI.
///
/// Supports common endpoint formats:
/// - `mcp://host:port`
/// - `tcp://ip:port`
/// - `ip:port`
///
/// Returns `None` if the endpoint doesn't contain an IPv4 address.
pub fn extract_ipv4_from_endpoint(endpoint: &str) -> Option<Ipv4Addr> {
    // Strip protocol prefix if present
    let host_part = endpoint
        .strip_prefix("mcp://")
        .or_else(|| endpoint.strip_prefix("tcp://"))
        .or_else(|| endpoint.strip_prefix("udp://"))
        .or_else(|| endpoint.strip_prefix("quic://"))
        .unwrap_or(endpoint);

    // Extract host (before port)
    let host = host_part.split(':').next()?;

    // Try to parse as IPv4
    host.parse::<Ipv4Addr>().ok()
}

/// Extract subnet prefix from endpoint URI.
///
/// Returns the /16 subnet prefix as a u16, or `None` if no IPv4
/// address can be extracted.
pub fn extract_subnet_from_endpoint(endpoint: &str) -> Option<u16> {
    extract_ipv4_from_endpoint(endpoint).map(|ip| {
        let octets = ip.octets();
        ((octets[0] as u16) << 8) | (octets[1] as u16)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bth_gossip::NatType;

    fn make_peer(ip: Ipv4Addr, score: f64) -> RelayPeerInfo {
        RelayPeerInfo::new(
            PeerId::random(),
            Some(ip),
            RelayCapacity {
                bandwidth_bps: (score * 10_000_000.0) as u64,
                uptime_ratio: score,
                nat_type: NatType::Open,
                current_load: 0.0,
            },
        )
    }

    fn make_peer_no_ip(score: f64) -> RelayPeerInfo {
        RelayPeerInfo::new(
            PeerId::random(),
            None,
            RelayCapacity {
                bandwidth_bps: (score * 10_000_000.0) as u64,
                uptime_ratio: score,
                nat_type: NatType::Open,
                current_load: 0.0,
            },
        )
    }

    #[test]
    fn test_relay_capacity_score() {
        // High bandwidth, high uptime, open NAT, no load
        let high = RelayCapacity {
            bandwidth_bps: 10_000_000,
            uptime_ratio: 1.0,
            nat_type: NatType::Open,
            current_load: 0.0,
        };
        assert!(high.relay_score() > 0.8);

        // Low bandwidth, low uptime, symmetric NAT, high load
        let low = RelayCapacity {
            bandwidth_bps: 100_000,
            uptime_ratio: 0.1,
            nat_type: NatType::Symmetric,
            current_load: 0.9,
        };
        assert!(low.relay_score() < 0.2);
        assert!(low.relay_score() >= 0.1); // Minimum guaranteed

        // Default has 1MB/s, 0.5 uptime, Unknown NAT, no load
        // Score: 0.04 (bw) + 0.15 (uptime) + 0.0 (NAT) = 0.19
        let default = RelayCapacity::default();
        assert!(default.relay_score() > 0.1);
    }

    #[test]
    fn test_subnet_prefix() {
        let peer = make_peer(Ipv4Addr::new(192, 168, 1, 100), 0.5);
        let subnet = peer.subnet_prefix().unwrap();
        // 192.168 = 0xC0A8
        assert_eq!(subnet, 0xC0A8);

        let peer2 = make_peer(Ipv4Addr::new(10, 0, 1, 1), 0.5);
        let subnet2 = peer2.subnet_prefix().unwrap();
        // 10.0 = 0x0A00
        assert_eq!(subnet2, 0x0A00);
    }

    #[test]
    fn test_same_subnet() {
        let peer1 = make_peer(Ipv4Addr::new(192, 168, 1, 1), 0.5);
        let peer2 = make_peer(Ipv4Addr::new(192, 168, 2, 2), 0.5);
        let peer3 = make_peer(Ipv4Addr::new(10, 0, 1, 1), 0.5);

        // Same /16 subnet (192.168.x.x)
        assert!(CircuitSelector::same_subnet(&peer1, &peer2));

        // Different subnets
        assert!(!CircuitSelector::same_subnet(&peer1, &peer3));
    }

    #[test]
    fn test_select_diverse_hops_success() {
        let selector = CircuitSelector::with_defaults();

        let peers = vec![
            make_peer(Ipv4Addr::new(10, 0, 1, 1), 0.8),
            make_peer(Ipv4Addr::new(10, 1, 1, 1), 0.7),
            make_peer(Ipv4Addr::new(10, 2, 1, 1), 0.9),
            make_peer(Ipv4Addr::new(10, 3, 1, 1), 0.6),
        ];

        let result = selector.select_diverse_hops(&peers, 3);
        assert!(result.is_ok());

        let hops = result.unwrap();
        assert_eq!(hops.len(), 3);

        // All hops should be unique
        let unique: HashSet<_> = hops.iter().collect();
        assert_eq!(unique.len(), 3);
    }

    #[test]
    fn test_select_insufficient_peers() {
        let selector = CircuitSelector::with_defaults();

        let peers = vec![
            make_peer(Ipv4Addr::new(10, 0, 1, 1), 0.8),
            make_peer(Ipv4Addr::new(10, 1, 1, 1), 0.7),
        ];

        let result = selector.select_diverse_hops(&peers, 3);
        assert!(matches!(
            result,
            Err(SelectionError::InsufficientPeers { .. })
        ));
    }

    #[test]
    fn test_select_insufficient_diversity() {
        let selector = CircuitSelector::with_defaults();

        // All peers in same /16 subnet
        let peers = vec![
            make_peer(Ipv4Addr::new(10, 0, 1, 1), 0.8),
            make_peer(Ipv4Addr::new(10, 0, 2, 2), 0.7),
            make_peer(Ipv4Addr::new(10, 0, 3, 3), 0.9),
        ];

        let result = selector.select_diverse_hops(&peers, 3);
        assert!(matches!(
            result,
            Err(SelectionError::InsufficientDiversity { .. })
        ));
    }

    #[test]
    fn test_select_no_qualified_peers() {
        let config = SelectionConfig {
            min_relay_score: 0.99, // Very high threshold
            ..Default::default()
        };
        let selector = CircuitSelector::new(config);

        let peers = vec![
            make_peer(Ipv4Addr::new(10, 0, 1, 1), 0.5),
            make_peer(Ipv4Addr::new(10, 1, 1, 1), 0.4),
        ];

        let result = selector.select_diverse_hops(&peers, 2);
        assert!(matches!(
            result,
            Err(SelectionError::NoQualifiedPeers { .. })
        ));
    }

    #[test]
    fn test_select_with_unknown_ips() {
        let config = SelectionConfig {
            allow_unknown_ip: true,
            ..Default::default()
        };
        let selector = CircuitSelector::new(config);

        let peers = vec![
            make_peer_no_ip(0.8),
            make_peer_no_ip(0.7),
            make_peer_no_ip(0.9),
        ];

        // Should succeed since unknown IPs are treated as unique
        let result = selector.select_diverse_hops(&peers, 3);
        assert!(result.is_ok());
    }

    #[test]
    fn test_non_strict_diversity() {
        let config = SelectionConfig {
            strict_diversity: false,
            ..Default::default()
        };
        let selector = CircuitSelector::new(config);

        // All in same subnet
        let peers = vec![
            make_peer(Ipv4Addr::new(10, 0, 1, 1), 0.8),
            make_peer(Ipv4Addr::new(10, 0, 2, 2), 0.7),
            make_peer(Ipv4Addr::new(10, 0, 3, 3), 0.9),
        ];

        // Should succeed in non-strict mode
        let result = selector.select_diverse_hops(&peers, 3);
        assert!(result.is_ok());
    }

    #[test]
    fn test_weighted_selection_bias() {
        // Test that higher scores lead to higher selection probability
        let high_score = make_peer(Ipv4Addr::new(10, 0, 1, 1), 0.9);
        let low_score = make_peer(Ipv4Addr::new(10, 1, 1, 1), 0.1);

        let candidates = vec![&high_score, &low_score];

        let mut high_count = 0;
        let mut rng = rand::thread_rng();

        for _ in 0..1000 {
            if let Some(selected) = weighted_random_select(&candidates, &mut rng) {
                if selected.peer_id == high_score.peer_id {
                    high_count += 1;
                }
            }
        }

        // High score peer should be selected more often
        // With ~0.83 vs ~0.27 scores (based on relay_score calculation),
        // expect ~75% selection rate for high score peer.
        // Use threshold of 600 (well below expected ~755) to avoid flaky failures.
        assert!(
            high_count > 600,
            "High score peer selected {} times (expected >600)",
            high_count
        );
    }

    #[test]
    fn test_are_diverse() {
        let peer1 = make_peer(Ipv4Addr::new(10, 0, 1, 1), 0.5);
        let peer2 = make_peer(Ipv4Addr::new(10, 1, 1, 1), 0.5);
        let peer3 = make_peer(Ipv4Addr::new(10, 0, 2, 2), 0.5);

        // Diverse peers (different /16 subnets)
        assert!(CircuitSelector::are_diverse(&[&peer1, &peer2]));

        // Not diverse (same /16 subnet)
        assert!(!CircuitSelector::are_diverse(&[&peer1, &peer3]));
    }

    #[test]
    fn test_extract_ipv4_from_endpoint() {
        assert_eq!(
            extract_ipv4_from_endpoint("mcp://192.168.1.100:8443"),
            Some(Ipv4Addr::new(192, 168, 1, 100))
        );

        assert_eq!(
            extract_ipv4_from_endpoint("tcp://10.0.0.1:5000"),
            Some(Ipv4Addr::new(10, 0, 0, 1))
        );

        assert_eq!(
            extract_ipv4_from_endpoint("172.16.0.1:8080"),
            Some(Ipv4Addr::new(172, 16, 0, 1))
        );

        // Hostname instead of IP
        assert_eq!(extract_ipv4_from_endpoint("mcp://example.com:8443"), None);

        // IPv6 (not supported by this function)
        assert_eq!(extract_ipv4_from_endpoint("[::1]:8080"), None);
    }

    #[test]
    fn test_extract_subnet_from_endpoint() {
        // 192.168 = 0xC0A8
        assert_eq!(
            extract_subnet_from_endpoint("mcp://192.168.1.100:8443"),
            Some(0xC0A8)
        );

        // 10.0 = 0x0A00
        assert_eq!(
            extract_subnet_from_endpoint("tcp://10.0.5.1:5000"),
            Some(0x0A00)
        );

        assert_eq!(extract_subnet_from_endpoint("mcp://example.com:8443"), None);
    }

    #[test]
    fn test_selection_is_nondeterministic() {
        let selector = CircuitSelector::with_defaults();

        let peers: Vec<_> = (0..10)
            .map(|i| make_peer(Ipv4Addr::new(10, i, 1, 1), 0.5))
            .collect();

        // Run selection multiple times and check we get different results
        let mut selections: HashSet<Vec<PeerId>> = HashSet::new();

        for _ in 0..50 {
            if let Ok(hops) = selector.select_diverse_hops(&peers, 3) {
                selections.insert(hops);
            }
        }

        // With 10 peers and 3 hops, there are C(10,3) = 120 combinations
        // With 50 runs, we should see several different selections
        assert!(
            selections.len() > 10,
            "Expected more diversity in selections, got {} unique",
            selections.len()
        );
    }

    #[test]
    fn test_nat_type_scoring() {
        let open = RelayCapacity {
            bandwidth_bps: 1_000_000,
            uptime_ratio: 0.5,
            nat_type: NatType::Open,
            current_load: 0.0,
        };
        let full_cone = RelayCapacity {
            bandwidth_bps: 1_000_000,
            uptime_ratio: 0.5,
            nat_type: NatType::FullCone,
            current_load: 0.0,
        };
        let restricted = RelayCapacity {
            bandwidth_bps: 1_000_000,
            uptime_ratio: 0.5,
            nat_type: NatType::Restricted,
            current_load: 0.0,
        };
        let symmetric = RelayCapacity {
            bandwidth_bps: 1_000_000,
            uptime_ratio: 0.5,
            nat_type: NatType::Symmetric,
            current_load: 0.0,
        };

        // Open NAT should score highest
        assert!(open.relay_score() > full_cone.relay_score());
        assert!(full_cone.relay_score() > restricted.relay_score());
        assert!(restricted.relay_score() > symmetric.relay_score());
    }

    #[test]
    fn test_load_penalty() {
        let no_load = RelayCapacity {
            bandwidth_bps: 5_000_000,
            uptime_ratio: 0.8,
            nat_type: NatType::Open,
            current_load: 0.0,
        };
        let half_load = RelayCapacity {
            bandwidth_bps: 5_000_000,
            uptime_ratio: 0.8,
            nat_type: NatType::Open,
            current_load: 0.5,
        };
        let full_load = RelayCapacity {
            bandwidth_bps: 5_000_000,
            uptime_ratio: 0.8,
            nat_type: NatType::Open,
            current_load: 1.0,
        };

        assert!(no_load.relay_score() > half_load.relay_score());
        assert!(half_load.relay_score() > full_load.relay_score());
    }

    #[test]
    fn test_minimum_score_guarantee() {
        // Even with worst parameters, minimum score is 0.1
        let worst = RelayCapacity {
            bandwidth_bps: 0,
            uptime_ratio: 0.0,
            nat_type: NatType::Symmetric,
            current_load: 1.0,
        };
        assert!((worst.relay_score() - 0.1).abs() < 0.01);
    }
}
