//! Cluster identification and wealth tracking.

use std::collections::HashMap;

/// Unique identifier for a cluster (coin lineage).
///
/// Each coin-creation event (mining reward, initial distribution) spawns a new
/// cluster. The ID is typically derived from the hash of the originating transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ClusterId(pub u64);

impl ClusterId {
    /// Create a new cluster ID.
    pub fn new(id: u64) -> Self {
        Self(id)
    }
}

/// Tracks the total wealth attributed to each cluster across all accounts.
///
/// Cluster wealth W_{C_k} = Σ_i (balance_i × tag_i(k))
///
/// This is the key input to the progressive fee function: clusters with more
/// concentrated wealth have higher fee rates.
#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ClusterWealth {
    /// Map from cluster ID to total tagged wealth.
    wealths: HashMap<ClusterId, u64>,
}

impl ClusterWealth {
    /// Create empty cluster wealth state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the total wealth attributed to a cluster.
    pub fn get(&self, cluster: ClusterId) -> u64 {
        self.wealths.get(&cluster).copied().unwrap_or(0)
    }

    /// Update cluster wealth by a signed delta.
    ///
    /// Positive delta: wealth flowing into the cluster.
    /// Negative delta: wealth flowing out (via decay or transfer).
    pub fn apply_delta(&mut self, cluster: ClusterId, delta: i64) {
        let current = self.get(cluster) as i64;
        let new_value = (current + delta).max(0) as u64;

        if new_value > 0 {
            self.wealths.insert(cluster, new_value);
        } else {
            self.wealths.remove(&cluster);
        }
    }

    /// Set the wealth for a cluster directly.
    pub fn set(&mut self, cluster: ClusterId, wealth: u64) {
        if wealth > 0 {
            self.wealths.insert(cluster, wealth);
        } else {
            self.wealths.remove(&cluster);
        }
    }

    /// Iterate over all clusters with non-zero wealth.
    pub fn iter(&self) -> impl Iterator<Item = (ClusterId, u64)> + '_ {
        self.wealths.iter().map(|(&k, &v)| (k, v))
    }

    /// Number of active clusters.
    pub fn len(&self) -> usize {
        self.wealths.len()
    }

    /// Returns true if no clusters have wealth.
    pub fn is_empty(&self) -> bool {
        self.wealths.is_empty()
    }

    /// Total wealth across all clusters (useful for sanity checks).
    pub fn total(&self) -> u64 {
        self.wealths.values().sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cluster_wealth_deltas() {
        let mut cw = ClusterWealth::new();
        let c1 = ClusterId::new(1);
        let c2 = ClusterId::new(2);

        // Initial state
        assert_eq!(cw.get(c1), 0);

        // Add wealth
        cw.apply_delta(c1, 1000);
        assert_eq!(cw.get(c1), 1000);

        // Reduce wealth
        cw.apply_delta(c1, -300);
        assert_eq!(cw.get(c1), 700);

        // Can't go negative
        cw.apply_delta(c1, -1000);
        assert_eq!(cw.get(c1), 0);
        assert!(!cw.wealths.contains_key(&c1));

        // Multiple clusters
        cw.apply_delta(c1, 500);
        cw.apply_delta(c2, 1500);
        assert_eq!(cw.total(), 2000);
        assert_eq!(cw.len(), 2);
    }
}
