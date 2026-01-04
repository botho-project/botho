// Copyright (c) 2024 Botho Foundation

//! Relay capacity measurement and utilities for circuit selection.
//!
//! This module re-exports the core capacity types from the gossip crate
//! and provides additional utilities for measuring and managing relay capacity.
//!
//! # Overview
//!
//! Every node in the Onion Gossip network can serve as a relay. Nodes advertise
//! their capacity through peer announcements, allowing circuit builders to make
//! informed decisions about which peers to select as hops.
//!
//! # Core Types (from gossip crate)
//!
//! - [`RelayCapacity`]: Core capacity metrics (bandwidth, uptime, NAT type, load)
//! - [`NatType`]: NAT classification affecting reachability
//!
//! # Utilities (this module)
//!
//! - [`NodeStats`]: Trait for measuring node statistics
//! - [`SimpleNodeStats`]: Basic implementation for testing
//! - Helper methods for capacity management
//!
//! # Example
//!
//! ```
//! use botho::network::privacy::capacity::{RelayCapacity, NatType, SimpleNodeStats, NodeStats};
//!
//! // Create stats for a well-connected node
//! let stats = SimpleNodeStats {
//!     bandwidth_bps: 10_000_000,
//!     uptime_ratio: 0.95,
//!     nat_type: NatType::Open,
//!     current_load: 0.2,
//! };
//!
//! // Measure capacity from stats
//! let capacity = RelayCapacity::measure(&stats);
//! let score = capacity.relay_score();
//! assert!(score > 0.7);
//! ```
//!
//! # References
//!
//! - Design doc: `docs/design/traffic-privacy-roadmap.md` (Section 1.6)
//! - Parent issue: #147 (Traffic Analysis Resistance - Phase 1)

// Re-export core types from gossip crate
pub use bth_gossip::{NatType, RelayCapacity};

/// Statistics for measuring relay capacity.
///
/// This trait abstracts the source of node statistics, allowing capacity
/// measurement to work with different stat collection implementations.
pub trait NodeStats {
    /// Get available upload bandwidth in bytes per second.
    fn available_bandwidth(&self) -> u64;

    /// Get uptime ratio over the last 24 hours (0.0 - 1.0).
    fn uptime_24h(&self) -> f64;

    /// Get detected NAT type.
    fn detected_nat_type(&self) -> NatType;

    /// Get current relay load (0.0 - 1.0).
    fn current_relay_load(&self) -> f64;
}

/// Extension trait for RelayCapacity with measurement utilities.
pub trait RelayCapacityExt {
    /// Measure current node capacity from statistics.
    fn measure<S: NodeStats>(stats: &S) -> Self;

    /// Check if this node has sufficient capacity to be a relay.
    fn is_viable_relay(&self) -> bool;

    /// Update current load based on active relay circuits.
    fn update_load(&mut self, active_circuits: u32, max_circuits: u32);
}

impl RelayCapacityExt for RelayCapacity {
    /// Measure current node capacity from statistics.
    ///
    /// This factory method creates a RelayCapacity by querying the provided
    /// stats implementation for current values.
    ///
    /// # Example
    ///
    /// ```
    /// use botho::network::privacy::capacity::{
    ///     RelayCapacity, RelayCapacityExt, SimpleNodeStats, NatType
    /// };
    ///
    /// let stats = SimpleNodeStats::new();
    /// let capacity = RelayCapacity::measure(&stats);
    /// ```
    fn measure<S: NodeStats>(stats: &S) -> Self {
        Self {
            bandwidth_bps: stats.available_bandwidth(),
            uptime_ratio: stats.uptime_24h(),
            nat_type: stats.detected_nat_type(),
            current_load: stats.current_relay_load(),
        }
    }

    /// Check if this node has sufficient capacity to be a relay.
    ///
    /// Returns `true` if the node meets minimum requirements:
    /// - At least 100 KB/s bandwidth
    /// - At least 10% uptime
    /// - Not completely overloaded
    fn is_viable_relay(&self) -> bool {
        self.bandwidth_bps >= 100_000 && self.uptime_ratio >= 0.1 && self.current_load < 0.95
    }

    /// Update current load based on active relay circuits.
    ///
    /// # Arguments
    ///
    /// * `active_circuits` - Number of circuits currently being relayed
    /// * `max_circuits` - Maximum circuits this node can handle
    fn update_load(&mut self, active_circuits: u32, max_circuits: u32) {
        if max_circuits == 0 {
            self.current_load = 1.0;
        } else {
            self.current_load = (active_circuits as f64 / max_circuits as f64).min(1.0);
        }
    }
}

/// Extension trait for NatType with utility methods.
pub trait NatTypeExt {
    /// Check if this NAT type allows good relay performance.
    fn is_relay_friendly(&self) -> bool;
}

impl NatTypeExt for NatType {
    /// Check if this NAT type allows good relay performance.
    fn is_relay_friendly(&self) -> bool {
        matches!(self, NatType::Open | NatType::FullCone | NatType::Restricted)
    }
}

/// Simple implementation of NodeStats for testing and basic usage.
#[derive(Debug, Clone)]
pub struct SimpleNodeStats {
    /// Available bandwidth in bytes/sec
    pub bandwidth_bps: u64,
    /// Uptime ratio (0.0 - 1.0)
    pub uptime_ratio: f64,
    /// Detected NAT type
    pub nat_type: NatType,
    /// Current load (0.0 - 1.0)
    pub current_load: f64,
}

impl SimpleNodeStats {
    /// Create new stats with default values.
    pub fn new() -> Self {
        Self {
            bandwidth_bps: 1_000_000,
            uptime_ratio: 1.0,
            nat_type: NatType::Unknown,
            current_load: 0.0,
        }
    }
}

impl Default for SimpleNodeStats {
    fn default() -> Self {
        Self::new()
    }
}

impl NodeStats for SimpleNodeStats {
    fn available_bandwidth(&self) -> u64 {
        self.bandwidth_bps
    }

    fn uptime_24h(&self) -> f64 {
        self.uptime_ratio
    }

    fn detected_nat_type(&self) -> NatType {
        self.nat_type
    }

    fn current_relay_load(&self) -> f64 {
        self.current_load
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_relay_score_perfect_node() {
        let capacity = RelayCapacity {
            bandwidth_bps: 10_000_000,
            uptime_ratio: 1.0,
            nat_type: NatType::Open,
            current_load: 0.0,
        };

        let score = capacity.relay_score();
        // 0.4 (bandwidth) + 0.3 (uptime) + 0.2 (NAT) = 0.9
        assert!((score - 0.9).abs() < 0.01);
    }

    #[test]
    fn test_relay_score_minimum() {
        let capacity = RelayCapacity {
            bandwidth_bps: 0,
            uptime_ratio: 0.0,
            nat_type: NatType::Symmetric,
            current_load: 1.0,
        };

        let score = capacity.relay_score();
        assert!((score - 0.1).abs() < 0.01);
    }

    #[test]
    fn test_relay_score_load_penalty() {
        let capacity_no_load = RelayCapacity {
            bandwidth_bps: 10_000_000,
            uptime_ratio: 1.0,
            nat_type: NatType::Open,
            current_load: 0.0,
        };

        let capacity_full_load = RelayCapacity {
            bandwidth_bps: 10_000_000,
            uptime_ratio: 1.0,
            nat_type: NatType::Open,
            current_load: 1.0,
        };

        let score_no_load = capacity_no_load.relay_score();
        let score_full_load = capacity_full_load.relay_score();

        // Full load should reduce score by 50%
        assert!((score_full_load - score_no_load * 0.5).abs() < 0.01);
    }

    #[test]
    fn test_nat_type_bonus() {
        assert!((NatType::Open.bonus() - 0.2).abs() < 0.01);
        assert!((NatType::FullCone.bonus() - 0.15).abs() < 0.01);
        assert!((NatType::Restricted.bonus() - 0.1).abs() < 0.01);
        assert!((NatType::Symmetric.bonus() - 0.0).abs() < 0.01);
        assert!((NatType::Unknown.bonus() - 0.0).abs() < 0.01);
    }

    #[test]
    fn test_nat_type_relay_friendly() {
        assert!(NatType::Open.is_relay_friendly());
        assert!(NatType::FullCone.is_relay_friendly());
        assert!(NatType::Restricted.is_relay_friendly());
        assert!(!NatType::Symmetric.is_relay_friendly());
        assert!(!NatType::Unknown.is_relay_friendly());
    }

    #[test]
    fn test_is_viable_relay() {
        let viable = RelayCapacity {
            bandwidth_bps: 1_000_000,
            uptime_ratio: 0.5,
            nat_type: NatType::Open,
            current_load: 0.5,
        };
        assert!(viable.is_viable_relay());

        let low_bandwidth = RelayCapacity {
            bandwidth_bps: 50_000,
            uptime_ratio: 1.0,
            nat_type: NatType::Open,
            current_load: 0.0,
        };
        assert!(!low_bandwidth.is_viable_relay());

        let low_uptime = RelayCapacity {
            bandwidth_bps: 1_000_000,
            uptime_ratio: 0.05,
            nat_type: NatType::Open,
            current_load: 0.0,
        };
        assert!(!low_uptime.is_viable_relay());

        let overloaded = RelayCapacity {
            bandwidth_bps: 10_000_000,
            uptime_ratio: 1.0,
            nat_type: NatType::Open,
            current_load: 0.99,
        };
        assert!(!overloaded.is_viable_relay());
    }

    #[test]
    fn test_update_load() {
        let mut capacity = RelayCapacity::default();

        capacity.update_load(5, 10);
        assert!((capacity.current_load - 0.5).abs() < 0.01);

        capacity.update_load(10, 10);
        assert!((capacity.current_load - 1.0).abs() < 0.01);

        capacity.update_load(0, 10);
        assert!((capacity.current_load - 0.0).abs() < 0.01);

        // Edge case: max_circuits = 0
        capacity.update_load(5, 0);
        assert!((capacity.current_load - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_measure_from_stats() {
        let stats = SimpleNodeStats {
            bandwidth_bps: 5_000_000,
            uptime_ratio: 0.8,
            nat_type: NatType::FullCone,
            current_load: 0.3,
        };

        let capacity = RelayCapacity::measure(&stats);

        assert_eq!(capacity.bandwidth_bps, 5_000_000);
        assert!((capacity.uptime_ratio - 0.8).abs() < 0.01);
        assert_eq!(capacity.nat_type, NatType::FullCone);
        assert!((capacity.current_load - 0.3).abs() < 0.01);
    }

    #[test]
    fn test_relay_capacity_serialization() {
        let capacity = RelayCapacity {
            bandwidth_bps: 10_000_000,
            uptime_ratio: 0.95,
            nat_type: NatType::Open,
            current_load: 0.2,
        };

        let json = serde_json::to_string(&capacity).unwrap();
        let parsed: RelayCapacity = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.bandwidth_bps, capacity.bandwidth_bps);
        assert!((parsed.uptime_ratio - capacity.uptime_ratio).abs() < 0.01);
        assert_eq!(parsed.nat_type, capacity.nat_type);
        assert!((parsed.current_load - capacity.current_load).abs() < 0.01);
    }

    #[test]
    fn test_nat_type_serialization() {
        for nat_type in [
            NatType::Open,
            NatType::FullCone,
            NatType::Restricted,
            NatType::Symmetric,
            NatType::Unknown,
        ] {
            let json = serde_json::to_string(&nat_type).unwrap();
            let parsed: NatType = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, nat_type);
        }
    }

    #[test]
    fn test_relay_capacity_default() {
        let capacity = RelayCapacity::default();

        assert_eq!(capacity.bandwidth_bps, 1_000_000);
        assert!((capacity.uptime_ratio - 0.5).abs() < 0.01);
        assert_eq!(capacity.nat_type, NatType::Unknown);
        assert!((capacity.current_load - 0.0).abs() < 0.01);
    }
}
