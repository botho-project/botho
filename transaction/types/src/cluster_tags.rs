// Copyright (c) 2018-2024 The MobileCoin Foundation

//! Cluster tag vector for progressive transaction fees.
//!
//! Each TxOut carries a tag vector indicating what fraction of its value
//! traces back to each cluster origin. This enables progressive fees where
//! clusters with concentrated wealth pay higher rates.
//!
//! See cluster-tax crate for the economic model and validation logic.

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use mc_crypto_digestible::Digestible;
#[cfg(feature = "prost")]
use prost::Message;
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

/// Maximum number of cluster tags stored per output.
/// Keeps output size bounded while preserving significant ancestry.
pub const MAX_CLUSTER_TAGS: usize = 16;

/// Minimum tag weight to store (weights below this become "background").
/// Represented as parts per million: 1000 = 0.1%.
pub const MIN_STORED_WEIGHT: u32 = 1000;

/// Scale factor for tag weights (1_000_000 = 100%).
pub const TAG_WEIGHT_SCALE: u32 = 1_000_000;

/// A cluster identifier derived from coin creation (e.g., mining rewards).
///
/// Not a group of accounts, but a lineage marker that fades through trade.
/// Cluster IDs are assigned when coins are minted and propagate through
/// the transaction graph with decay.
#[derive(Clone, Copy, Digestible, Eq, Hash, Ord, PartialEq, PartialOrd, Zeroize)]
#[cfg_attr(feature = "prost", derive(Message))]
#[cfg_attr(not(feature = "prost"), derive(Debug, Default))]
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
pub struct ClusterId(
    #[cfg_attr(feature = "prost", prost(fixed64, required, tag = "1"))]
    pub u64,
);

/// A single cluster tag entry: cluster ID and weight.
#[derive(Clone, Copy, Digestible, Eq, Hash, PartialEq, Zeroize)]
#[cfg_attr(feature = "prost", derive(Message))]
#[cfg_attr(not(feature = "prost"), derive(Debug, Default))]
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
pub struct ClusterTagEntry {
    /// The cluster this tag refers to.
    #[cfg_attr(feature = "prost", prost(message, required, tag = "1"))]
    pub cluster_id: ClusterId,

    /// Weight in parts per million (TAG_WEIGHT_SCALE = 100%).
    #[cfg_attr(feature = "prost", prost(fixed32, required, tag = "2"))]
    pub weight: u32,
}

/// Compact on-chain representation of a cluster tag vector.
///
/// This structure is serialized into TxOut to track coin ancestry.
/// Uses a variable-length list of entries, pruned to significant weights.
///
/// In Phase 1, tags are stored in plaintext. Ring signatures still hide
/// which input is the real one, providing some privacy.
///
/// In Phase 2 (future), tags would be committed using Pedersen commitments
/// with ZK proofs for inheritance validation.
#[derive(Clone, Digestible, Eq, Hash, PartialEq, Zeroize)]
#[cfg_attr(feature = "prost", derive(Message))]
#[cfg_attr(not(feature = "prost"), derive(Debug, Default))]
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[cfg(feature = "alloc")]
pub struct ClusterTagVector {
    /// List of cluster tag entries, sorted by weight descending.
    /// Maximum of MAX_CLUSTER_TAGS entries.
    #[cfg_attr(feature = "prost", prost(message, repeated, tag = "1"))]
    pub entries: Vec<ClusterTagEntry>,
}

#[cfg(feature = "alloc")]
impl ClusterTagVector {
    /// Create an empty tag vector (fully "background" - no cluster attribution).
    pub fn empty() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Create a tag vector fully attributed to one cluster.
    /// Used when minting new coins.
    pub fn single(cluster_id: ClusterId) -> Self {
        Self {
            entries: alloc::vec![ClusterTagEntry {
                cluster_id,
                weight: TAG_WEIGHT_SCALE,
            }],
        }
    }

    /// Get the total weight attributed to explicit clusters.
    /// The remainder (TAG_WEIGHT_SCALE - total) is "background".
    pub fn total_weight(&self) -> u32 {
        self.entries
            .iter()
            .map(|e| e.weight)
            .sum::<u32>()
            .min(TAG_WEIGHT_SCALE)
    }

    /// Get the background weight (unattributed portion).
    pub fn background_weight(&self) -> u32 {
        TAG_WEIGHT_SCALE.saturating_sub(self.total_weight())
    }

    /// Get the weight for a specific cluster.
    pub fn get_weight(&self, cluster_id: ClusterId) -> u32 {
        self.entries
            .iter()
            .find(|e| e.cluster_id == cluster_id)
            .map(|e| e.weight)
            .unwrap_or(0)
    }

    /// Check if this tag vector has any cluster attribution.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Number of cluster entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Create from a list of (cluster_id, weight) pairs.
    /// Filters out weights below MIN_STORED_WEIGHT, sorts by weight descending,
    /// and truncates to MAX_CLUSTER_TAGS.
    pub fn from_pairs(pairs: &[(ClusterId, u32)]) -> Self {
        let mut entries: Vec<ClusterTagEntry> = pairs
            .iter()
            .filter(|(_, w)| *w >= MIN_STORED_WEIGHT)
            .map(|(cluster_id, weight)| ClusterTagEntry {
                cluster_id: *cluster_id,
                weight: *weight,
            })
            .collect();

        // Sort by weight descending
        entries.sort_by(|a, b| b.weight.cmp(&a.weight));

        // Truncate to max
        entries.truncate(MAX_CLUSTER_TAGS);

        Self { entries }
    }

    /// Compute inherited tags from multiple inputs weighted by their values.
    ///
    /// This is the core inheritance computation for cluster tags:
    /// - Each input contributes proportionally to its value
    /// - An optional decay rate reduces weights (expressed as parts per TAG_WEIGHT_SCALE)
    /// - Weights below MIN_STORED_WEIGHT are pruned
    /// - Result is sorted by weight descending
    ///
    /// # Arguments
    /// * `inputs` - List of (ClusterTagVector, value) pairs for each input
    /// * `decay_rate` - Decay to apply (0 = no decay, TAG_WEIGHT_SCALE = full decay)
    ///
    /// # Returns
    /// A new ClusterTagVector representing the weighted merge with decay applied.
    pub fn merge_weighted(inputs: &[(ClusterTagVector, u64)], decay_rate: u32) -> Self {
        let total_value: u64 = inputs.iter().map(|(_, v)| *v).sum();
        if total_value == 0 {
            return Self::empty();
        }

        // Accumulate weighted contributions per cluster
        let mut cluster_weights: Vec<(ClusterId, u64)> = Vec::new();

        for (tags, value) in inputs {
            for entry in &tags.entries {
                // Contribution = (input_value / total_value) * entry_weight
                // Use u128 to avoid overflow
                let contribution =
                    ((*value as u128) * (entry.weight as u128)) / (total_value as u128);

                // Find or insert this cluster
                if let Some(existing) = cluster_weights
                    .iter_mut()
                    .find(|(id, _)| *id == entry.cluster_id)
                {
                    existing.1 = existing.1.saturating_add(contribution as u64);
                } else {
                    cluster_weights.push((entry.cluster_id, contribution as u64));
                }
            }
        }

        // Apply decay: new_weight = weight * (TAG_WEIGHT_SCALE - decay_rate) / TAG_WEIGHT_SCALE
        let retention = TAG_WEIGHT_SCALE.saturating_sub(decay_rate);
        let pairs: Vec<(ClusterId, u32)> = cluster_weights
            .into_iter()
            .map(|(id, w)| {
                let decayed = ((w as u64) * (retention as u64)) / (TAG_WEIGHT_SCALE as u64);
                (id, decayed as u32)
            })
            .collect();

        Self::from_pairs(&pairs)
    }

    /// Validate that the tag vector is well-formed.
    ///
    /// Returns true if:
    /// - Total weight <= TAG_WEIGHT_SCALE
    /// - No duplicate cluster IDs
    /// - Entries are sorted by weight descending
    /// - All weights >= MIN_STORED_WEIGHT
    /// - At most MAX_CLUSTER_TAGS entries
    pub fn is_valid(&self) -> bool {
        if self.entries.len() > MAX_CLUSTER_TAGS {
            return false;
        }

        let mut total: u32 = 0;
        let mut prev_weight: u32 = u32::MAX;

        for (i, entry) in self.entries.iter().enumerate() {
            // Check weight is valid
            if entry.weight < MIN_STORED_WEIGHT {
                return false;
            }

            // Check sorted descending
            if entry.weight > prev_weight {
                return false;
            }
            prev_weight = entry.weight;

            // Check no duplicates
            for other in self.entries.iter().skip(i + 1) {
                if entry.cluster_id == other.cluster_id {
                    return false;
                }
            }

            // Accumulate total
            total = total.saturating_add(entry.weight);
        }

        total <= TAG_WEIGHT_SCALE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn test_empty_tag_vector() {
        let tags = ClusterTagVector::empty();
        assert!(tags.is_empty());
        assert_eq!(tags.total_weight(), 0);
        assert_eq!(tags.background_weight(), TAG_WEIGHT_SCALE);
        assert!(tags.is_valid());
    }

    #[test]
    fn test_single_cluster() {
        let cluster = ClusterId(42);
        let tags = ClusterTagVector::single(cluster);

        assert_eq!(tags.len(), 1);
        assert_eq!(tags.get_weight(cluster), TAG_WEIGHT_SCALE);
        assert_eq!(tags.total_weight(), TAG_WEIGHT_SCALE);
        assert_eq!(tags.background_weight(), 0);
        assert!(tags.is_valid());
    }

    #[test]
    fn test_from_pairs() {
        let pairs = [
            (ClusterId(1), 500_000), // 50%
            (ClusterId(2), 300_000), // 30%
            (ClusterId(3), 100),     // Below threshold, should be filtered
        ];

        let tags = ClusterTagVector::from_pairs(&pairs);

        assert_eq!(tags.len(), 2);
        assert_eq!(tags.get_weight(ClusterId(1)), 500_000);
        assert_eq!(tags.get_weight(ClusterId(2)), 300_000);
        assert_eq!(tags.get_weight(ClusterId(3)), 0); // Filtered out
        assert_eq!(tags.total_weight(), 800_000);
        assert_eq!(tags.background_weight(), 200_000);
        assert!(tags.is_valid());
    }

    #[test]
    fn test_sorted_by_weight() {
        let pairs = [
            (ClusterId(1), 100_000),
            (ClusterId(2), 500_000),
            (ClusterId(3), 300_000),
        ];

        let tags = ClusterTagVector::from_pairs(&pairs);

        // Should be sorted by weight descending
        assert_eq!(tags.entries[0].cluster_id, ClusterId(2));
        assert_eq!(tags.entries[1].cluster_id, ClusterId(3));
        assert_eq!(tags.entries[2].cluster_id, ClusterId(1));
        assert!(tags.is_valid());
    }

    #[test]
    fn test_validation_rejects_duplicates() {
        let mut tags = ClusterTagVector::empty();
        tags.entries.push(ClusterTagEntry {
            cluster_id: ClusterId(1),
            weight: 500_000,
        });
        tags.entries.push(ClusterTagEntry {
            cluster_id: ClusterId(1), // Duplicate!
            weight: 300_000,
        });

        assert!(!tags.is_valid());
    }

    #[test]
    fn test_validation_rejects_overflow() {
        let mut tags = ClusterTagVector::empty();
        tags.entries.push(ClusterTagEntry {
            cluster_id: ClusterId(1),
            weight: 600_000,
        });
        tags.entries.push(ClusterTagEntry {
            cluster_id: ClusterId(2),
            weight: 500_000, // Total > 100%
        });

        assert!(!tags.is_valid());
    }

    #[test]
    fn test_validation_rejects_unsorted() {
        let mut tags = ClusterTagVector::empty();
        tags.entries.push(ClusterTagEntry {
            cluster_id: ClusterId(1),
            weight: 300_000,
        });
        tags.entries.push(ClusterTagEntry {
            cluster_id: ClusterId(2),
            weight: 500_000, // Higher than previous - not sorted!
        });

        assert!(!tags.is_valid());
    }

    #[test]
    fn test_merge_weighted_single_input() {
        let tags = ClusterTagVector::single(ClusterId(1));
        let inputs = vec![(tags, 1000u64)];

        let result = ClusterTagVector::merge_weighted(&inputs, 0);
        assert_eq!(result.len(), 1);
        assert_eq!(result.get_weight(ClusterId(1)), TAG_WEIGHT_SCALE);
        assert!(result.is_valid());
    }

    #[test]
    fn test_merge_weighted_equal_inputs() {
        // Two inputs with same value, different clusters
        let tags1 = ClusterTagVector::single(ClusterId(1));
        let tags2 = ClusterTagVector::single(ClusterId(2));
        let inputs = vec![(tags1, 1000u64), (tags2, 1000u64)];

        let result = ClusterTagVector::merge_weighted(&inputs, 0);
        assert_eq!(result.len(), 2);
        // Each should have 50% weight
        assert_eq!(result.get_weight(ClusterId(1)), 500_000);
        assert_eq!(result.get_weight(ClusterId(2)), 500_000);
        assert!(result.is_valid());
    }

    #[test]
    fn test_merge_weighted_unequal_inputs() {
        // Input 1 is worth 3x as much as input 2
        let tags1 = ClusterTagVector::single(ClusterId(1));
        let tags2 = ClusterTagVector::single(ClusterId(2));
        let inputs = vec![(tags1, 3000u64), (tags2, 1000u64)];

        let result = ClusterTagVector::merge_weighted(&inputs, 0);
        assert_eq!(result.len(), 2);
        // Cluster 1 should have 75%, cluster 2 should have 25%
        assert_eq!(result.get_weight(ClusterId(1)), 750_000);
        assert_eq!(result.get_weight(ClusterId(2)), 250_000);
        assert!(result.is_valid());
    }

    #[test]
    fn test_merge_weighted_with_decay() {
        let tags = ClusterTagVector::single(ClusterId(1));
        let inputs = vec![(tags, 1000u64)];

        // 10% decay
        let decay = 100_000;
        let result = ClusterTagVector::merge_weighted(&inputs, decay);

        // Should have 90% of original
        assert_eq!(result.get_weight(ClusterId(1)), 900_000);
        assert!(result.is_valid());
    }

    #[test]
    fn test_merge_weighted_combines_same_cluster() {
        // Two inputs that both have cluster 1
        let tags1 = ClusterTagVector::single(ClusterId(1));
        let tags2 = ClusterTagVector::single(ClusterId(1));
        let inputs = vec![(tags1, 1000u64), (tags2, 1000u64)];

        let result = ClusterTagVector::merge_weighted(&inputs, 0);
        assert_eq!(result.len(), 1);
        // Contributions combine to 100%
        assert_eq!(result.get_weight(ClusterId(1)), TAG_WEIGHT_SCALE);
        assert!(result.is_valid());
    }

    #[test]
    fn test_merge_weighted_empty() {
        let inputs: Vec<(ClusterTagVector, u64)> = vec![];
        let result = ClusterTagVector::merge_weighted(&inputs, 0);
        assert!(result.is_empty());
        assert!(result.is_valid());
    }

    #[test]
    fn test_merge_weighted_prunes_small_weights() {
        // Input with tiny weight that should be pruned after decay
        let mut tags = ClusterTagVector::empty();
        tags.entries.push(ClusterTagEntry {
            cluster_id: ClusterId(1),
            weight: MIN_STORED_WEIGHT, // Exactly at threshold
        });

        let inputs = vec![(tags, 1000u64)];

        // 50% decay should push it below MIN_STORED_WEIGHT
        let result = ClusterTagVector::merge_weighted(&inputs, 500_000);
        assert!(result.is_empty());
    }
}
