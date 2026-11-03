// Copyright (c) 2018-2024 The Botho Foundation

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

use bth_crypto_digestible::Digestible;
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

/// A cluster identifier derived from coin creation (e.g., minting rewards).
///
/// Not a group of accounts, but a lineage marker that fades through trade.
/// Cluster IDs are assigned when coins are minted and propagate through
/// the transaction graph with decay.
#[derive(Clone, Copy, Digestible, Eq, Hash, Ord, PartialEq, PartialOrd, Zeroize)]
#[cfg_attr(feature = "prost", derive(Message))]
#[cfg_attr(not(feature = "prost"), derive(Debug, Default))]
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
pub struct ClusterId(#[cfg_attr(feature = "prost", prost(fixed64, required, tag = "1"))] pub u64);

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

/// Decay state for AND-based decay mechanism.
///
/// This tracks when decay was last applied and how many decays have occurred
/// in the current epoch. Required for rate-limiting and epoch-capping decay
/// to resist wash trading attacks.
#[derive(Clone, Copy, Digestible, Eq, Hash, PartialEq, Zeroize)]
#[cfg_attr(feature = "prost", derive(Message))]
#[cfg_attr(not(feature = "prost"), derive(Debug, Default))]
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
pub struct DecayState {
    /// Block height when decay was last applied.
    /// Used for rate-limiting: decay only occurs if sufficient blocks have
    /// passed.
    #[cfg_attr(feature = "prost", prost(fixed64, required, tag = "1"))]
    pub last_decay_block: u64,

    /// Number of decays applied in the current epoch.
    /// Used for epoch-capping: limits total decay per time period.
    #[cfg_attr(feature = "prost", prost(fixed32, required, tag = "2"))]
    pub decays_this_epoch: u32,

    /// Start block of the current epoch.
    /// When current_block >= epoch_start_block + epoch_blocks, epoch resets.
    #[cfg_attr(feature = "prost", prost(fixed64, required, tag = "3"))]
    pub epoch_start_block: u64,
}

impl DecayState {
    /// Create a new decay state at the given block height.
    pub fn new(block: u64) -> Self {
        Self {
            last_decay_block: block,
            decays_this_epoch: 0,
            epoch_start_block: block,
        }
    }

    /// Create a decay state representing "never decayed" for migration.
    /// Old UTXOs without decay state are treated as eligible for first decay.
    pub fn never_decayed(creation_block: u64) -> Self {
        Self {
            last_decay_block: creation_block,
            decays_this_epoch: 0,
            epoch_start_block: creation_block,
        }
    }
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

    /// Decay state tracking for AND-based decay mechanism.
    /// Optional for backward compatibility with existing UTXOs.
    /// UTXOs without decay state are treated as "never decayed".
    #[cfg_attr(feature = "prost", prost(message, optional, tag = "2"))]
    pub decay_state: Option<DecayState>,
}

#[cfg(feature = "alloc")]
impl ClusterTagVector {
    /// Create an empty tag vector (fully "background" - no cluster
    /// attribution).
    pub fn empty() -> Self {
        Self {
            entries: Vec::new(),
            decay_state: None,
        }
    }

    /// Create an empty tag vector at a specific block height.
    pub fn empty_at_block(block: u64) -> Self {
        Self {
            entries: Vec::new(),
            decay_state: Some(DecayState::new(block)),
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
            decay_state: None,
        }
    }

    /// Create a tag vector fully attributed to one cluster at a specific block
    /// height. Used when minting new coins with decay tracking.
    pub fn single_at_block(cluster_id: ClusterId, block: u64) -> Self {
        Self {
            entries: alloc::vec![ClusterTagEntry {
                cluster_id,
                weight: TAG_WEIGHT_SCALE,
            }],
            decay_state: Some(DecayState::new(block)),
        }
    }

    /// Get the decay state, or create a default "never decayed" state.
    pub fn decay_state_or_default(&self, creation_block: u64) -> DecayState {
        self.decay_state
            .unwrap_or_else(|| DecayState::never_decayed(creation_block))
    }

    /// Get the last decay block, or the creation block if never decayed.
    pub fn last_decay_block(&self, creation_block: u64) -> u64 {
        self.decay_state
            .map(|s| s.last_decay_block)
            .unwrap_or(creation_block)
    }

    /// Get the number of decays this epoch.
    pub fn decays_this_epoch(&self) -> u32 {
        self.decay_state.map(|s| s.decays_this_epoch).unwrap_or(0)
    }

    /// Get the epoch start block.
    pub fn epoch_start_block(&self, creation_block: u64) -> u64 {
        self.decay_state
            .map(|s| s.epoch_start_block)
            .unwrap_or(creation_block)
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

    /// Calculate Shannon entropy of the full tag distribution including
    /// background.
    ///
    /// WARNING: This includes background in the entropy calculation, which
    /// means entropy INCREASES with age as tags decay to background. This
    /// is NOT suitable for lottery selection because it would give old
    /// coins an unfair advantage.
    ///
    /// For lottery weight calculation, use `cluster_entropy()` instead.
    pub fn shannon_entropy(&self) -> f64 {
        let scale = TAG_WEIGHT_SCALE as f64;
        let mut entropy = 0.0;

        // Entropy from each cluster tag
        for entry in &self.entries {
            if entry.weight > 0 {
                let p = entry.weight as f64 / scale;
                entropy -= p * libm::log2(p);
            }
        }

        // Entropy from background
        let bg = self.background_weight();
        if bg > 0 {
            let p = bg as f64 / scale;
            entropy -= p * libm::log2(p);
        }

        entropy
    }

    /// Calculate Shannon entropy of the CLUSTER distribution only (excluding
    /// background).
    ///
    /// This is the correct entropy measure for lottery selection because it is
    /// **decay-invariant**: natural tag decay doesn't change cluster entropy.
    ///
    /// The calculation renormalizes cluster weights to sum to 1.0, effectively
    /// ignoring the background portion. This ensures:
    /// - Fresh mint (100% one cluster): 0 bits
    /// - After decay (90% one cluster, 10% bg): still 0 bits
    /// - Commerce (50% A, 50% B): 1 bit
    /// - Commerce after decay (40% A, 40% B, 20% bg): still 1 bit
    pub fn cluster_entropy(&self) -> f64 {
        let total_cluster = self.total_weight();
        if total_cluster == 0 {
            // Fully background = no cluster diversity = 0 entropy
            return 0.0;
        }

        let scale = total_cluster as f64;
        let mut entropy = 0.0;

        // Entropy from each cluster tag, renormalized
        for entry in &self.entries {
            if entry.weight > 0 {
                let p = entry.weight as f64 / scale;
                entropy -= p * libm::log2(p);
            }
        }

        entropy
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
        entries.sort_by_key(|entry| core::cmp::Reverse(entry.weight));

        // Truncate to max
        entries.truncate(MAX_CLUSTER_TAGS);

        Self {
            entries,
            decay_state: None,
        }
    }

    /// Create from pairs with decay state at a specific block.
    pub fn from_pairs_at_block(pairs: &[(ClusterId, u32)], block: u64) -> Self {
        let mut result = Self::from_pairs(pairs);
        result.decay_state = Some(DecayState::new(block));
        result
    }

    /// Compute inherited tags from multiple inputs weighted by their values.
    ///
    /// This is the core inheritance computation for cluster tags:
    /// - Each input contributes proportionally to its value
    /// - An optional decay rate reduces weights (expressed as parts per
    ///   TAG_WEIGHT_SCALE)
    /// - Weights below MIN_STORED_WEIGHT are pruned
    /// - Result is sorted by weight descending
    ///
    /// Note: This is the legacy hop-based decay method. For AND-based decay
    /// with rate limiting, use `merge_weighted_with_and_decay()` instead.
    ///
    /// # Arguments
    /// * `inputs` - List of (ClusterTagVector, value) pairs for each input
    /// * `decay_rate` - Decay to apply (0 = no decay, TAG_WEIGHT_SCALE = full
    ///   decay)
    ///
    /// # Returns
    /// A new ClusterTagVector representing the weighted merge with decay
    /// applied.
    ///
    /// # Overflow behaviour (#1858)
    ///
    /// This function never panics. The value sum is taken in `u128` and the
    /// decay multiply is done in `u128` and clamped to `u32::MAX`. For any
    /// inputs whose tags pass [`ClusterTagVector::is_valid`] and whose values
    /// sum to at most `u64::MAX`, every accumulated weight is at most
    /// `TAG_WEIGHT_SCALE`, so neither the widening nor the clamp is ever
    /// reached and the output is bit-identical to the pre-#1858 `u64`
    /// arithmetic. Only malformed (over-100%) tags, which `is_valid` rejects,
    /// can reach the clamp; before #1858 those panicked or wrapped.
    pub fn merge_weighted(inputs: &[(ClusterTagVector, u64)], decay_rate: u32) -> Self {
        let total_value: u128 = inputs.iter().map(|(_, v)| *v as u128).sum();
        if total_value == 0 {
            return Self::empty();
        }

        // Accumulate weighted contributions per cluster
        let mut cluster_weights: Vec<(ClusterId, u64)> = Vec::new();

        for (tags, value) in inputs {
            for entry in &tags.entries {
                // Contribution = (input_value / total_value) * entry_weight
                // Use u128 to avoid overflow
                let contribution = ((*value as u128) * (entry.weight as u128)) / total_value;

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

        // Apply decay: new_weight = weight * (TAG_WEIGHT_SCALE - decay_rate) /
        // TAG_WEIGHT_SCALE
        let retention = TAG_WEIGHT_SCALE.saturating_sub(decay_rate);
        let pairs: Vec<(ClusterId, u32)> = cluster_weights
            .into_iter()
            .map(|(id, w)| (id, Self::decay_weight(w, retention)))
            .collect();

        Self::from_pairs(&pairs)
    }

    /// Apply `retention` (parts per `TAG_WEIGHT_SCALE`) to an accumulated
    /// weight without overflow: the multiply is done in `u128` and the
    /// result is clamped to `u32::MAX` rather than truncated by `as u32`
    /// (#1858). Monotone non-decreasing in `w`.
    #[inline]
    fn decay_weight(w: u64, retention: u32) -> u32 {
        let decayed = (w as u128) * (retention as u128) / (TAG_WEIGHT_SCALE as u128);
        decayed.min(u32::MAX as u128) as u32
    }

    /// Compute inherited tags with AND-based decay (rate-limited and
    /// epoch-capped).
    ///
    /// This implements the AND-based decay mechanism that requires BOTH
    /// conditions:
    /// 1. A transfer must occur (hop condition - implicit in calling this
    ///    method)
    /// 2. Sufficient time must have passed since last decay (rate limiting)
    /// 3. Epoch decay cap not exceeded (epoch capping)
    ///
    /// # Arguments
    /// * `inputs` - List of (ClusterTagVector, value, creation_block) for each
    ///   input
    /// * `current_block` - Current block height
    /// * `decay_rate_per_hop` - Decay to apply if eligible (parts per
    ///   TAG_WEIGHT_SCALE)
    /// * `min_blocks_between_decays` - Minimum blocks between decay events
    /// * `max_decays_per_epoch` - Maximum decays per epoch (0 = unlimited)
    /// * `epoch_blocks` - Epoch length in blocks
    ///
    /// # Returns
    /// A tuple of (new ClusterTagVector, decay_applied: bool).
    pub fn merge_weighted_with_and_decay(
        inputs: &[(ClusterTagVector, u64, u64)], // (tags, value, creation_block)
        current_block: u64,
        decay_rate_per_hop: u32,
        min_blocks_between_decays: u64,
        max_decays_per_epoch: u32,
        epoch_blocks: u64,
    ) -> (Self, bool) {
        // u128 so the sum cannot overflow (#1858); see `merge_weighted`.
        let total_value: u128 = inputs.iter().map(|(_, v, _)| *v as u128).sum();
        if total_value == 0 {
            return (Self::empty_at_block(current_block), false);
        }

        // Accumulate weighted contributions per cluster
        let mut cluster_weights: Vec<(ClusterId, u64)> = Vec::new();

        // Also track the combined decay state from inputs
        let mut combined_last_decay_block: u64 = 0;
        let mut combined_decays_this_epoch: u32 = 0;
        let mut combined_epoch_start_block: u64 = u64::MAX;

        for (tags, value, creation_block) in inputs {
            // Get decay state for this input
            let decay_state = tags.decay_state_or_default(*creation_block);

            // Weight-average the decay state from inputs
            combined_last_decay_block = combined_last_decay_block.max(decay_state.last_decay_block);
            combined_decays_this_epoch = combined_decays_this_epoch.saturating_add(
                ((decay_state.decays_this_epoch as u128) * (*value as u128) / total_value.max(1))
                    as u32,
            );
            combined_epoch_start_block =
                combined_epoch_start_block.min(decay_state.epoch_start_block);

            for entry in &tags.entries {
                let contribution = ((*value as u128) * (entry.weight as u128)) / total_value;

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

        // Handle edge case of empty epoch start
        if combined_epoch_start_block == u64::MAX {
            combined_epoch_start_block = current_block;
        }

        // Check if we're in a new epoch
        let mut epoch_start = combined_epoch_start_block;
        let mut decays_this_epoch = combined_decays_this_epoch;

        if current_block.saturating_sub(epoch_start) >= epoch_blocks {
            epoch_start = current_block;
            decays_this_epoch = 0;
        }

        // Check AND conditions for decay eligibility
        let never_decayed = combined_last_decay_block == 0 && decays_this_epoch == 0;
        let time_eligible = never_decayed
            || current_block.saturating_sub(combined_last_decay_block) >= min_blocks_between_decays;
        let epoch_cap_ok = max_decays_per_epoch == 0 || decays_this_epoch < max_decays_per_epoch;

        let decay_applies = time_eligible && epoch_cap_ok;

        // Apply decay if eligible
        let pairs: Vec<(ClusterId, u32)> = if decay_applies {
            let retention = TAG_WEIGHT_SCALE.saturating_sub(decay_rate_per_hop);
            cluster_weights
                .into_iter()
                .map(|(id, w)| (id, Self::decay_weight(w, retention)))
                .collect()
        } else {
            cluster_weights
                .into_iter()
                .map(|(id, w)| (id, w.min(u32::MAX as u64) as u32))
                .collect()
        };

        // Build the result with updated decay state
        let mut result = Self::from_pairs(&pairs);
        result.decay_state = Some(DecayState {
            last_decay_block: if decay_applies {
                current_block
            } else {
                combined_last_decay_block
            },
            decays_this_epoch: if decay_applies {
                // saturating (#1858): an input decay state carrying
                // u32::MAX decays with an uncapped epoch must not panic.
                decays_this_epoch.saturating_add(1)
            } else {
                decays_this_epoch
            },
            epoch_start_block: epoch_start,
        });

        (result, decay_applies)
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

    // ========================================================================
    // Decay State Tests
    // ========================================================================

    #[test]
    fn test_decay_state_new() {
        let state = DecayState::new(1000);
        assert_eq!(state.last_decay_block, 1000);
        assert_eq!(state.decays_this_epoch, 0);
        assert_eq!(state.epoch_start_block, 1000);
    }

    #[test]
    fn test_decay_state_never_decayed() {
        let state = DecayState::never_decayed(500);
        assert_eq!(state.last_decay_block, 500);
        assert_eq!(state.decays_this_epoch, 0);
        assert_eq!(state.epoch_start_block, 500);
    }

    #[test]
    fn test_single_at_block() {
        let cluster = ClusterId(1);
        let tags = ClusterTagVector::single_at_block(cluster, 1000);

        assert_eq!(tags.get_weight(cluster), TAG_WEIGHT_SCALE);
        assert!(tags.decay_state.is_some());

        let state = tags.decay_state.unwrap();
        assert_eq!(state.last_decay_block, 1000);
        assert_eq!(state.decays_this_epoch, 0);
        assert_eq!(state.epoch_start_block, 1000);
    }

    #[test]
    fn test_decay_state_or_default() {
        // With decay state present
        let tags = ClusterTagVector::single_at_block(ClusterId(1), 1000);
        let state = tags.decay_state_or_default(0);
        assert_eq!(state.last_decay_block, 1000);

        // Without decay state (legacy UTXO)
        let legacy = ClusterTagVector::single(ClusterId(1));
        let state = legacy.decay_state_or_default(500);
        assert_eq!(state.last_decay_block, 500); // Falls back to creation_block
    }

    // ========================================================================
    // AND-Based Decay Tests
    // ========================================================================

    #[test]
    fn test_and_decay_first_transfer_always_decays() {
        // First transfer should always trigger decay (never decayed before)
        let tags = ClusterTagVector::single_at_block(ClusterId(1), 0);
        let inputs = vec![(tags, 1000u64, 0u64)];

        let (result, decay_applied) = ClusterTagVector::merge_weighted_with_and_decay(
            &inputs, 100,    // current_block
            50_000, // 5% decay
            360,    // min_blocks_between (1 hour)
            12,     // max_decays_per_epoch
            8_640,  // epoch_blocks (1 day)
        );

        assert!(decay_applied, "First transfer should always decay");
        // 95% of 1_000_000 = 950_000
        assert_eq!(result.get_weight(ClusterId(1)), 950_000);

        let state = result.decay_state.unwrap();
        assert_eq!(state.last_decay_block, 100);
        assert_eq!(state.decays_this_epoch, 1);
    }

    #[test]
    fn test_and_decay_rate_limiting() {
        // Second transfer too soon should NOT trigger decay
        let mut tags = ClusterTagVector::single_at_block(ClusterId(1), 0);
        tags.decay_state = Some(DecayState {
            last_decay_block: 100,
            decays_this_epoch: 1,
            epoch_start_block: 0,
        });

        let inputs = vec![(tags, 1000u64, 0u64)];

        // Only 50 blocks since last decay (< 360 min)
        let (result, decay_applied) = ClusterTagVector::merge_weighted_with_and_decay(
            &inputs, 150,    // current_block
            50_000, // 5% decay
            360,    // min_blocks_between (1 hour)
            12,     // max_decays_per_epoch
            8_640,  // epoch_blocks
        );

        assert!(!decay_applied, "Rate limiting should block decay");
        assert_eq!(result.get_weight(ClusterId(1)), TAG_WEIGHT_SCALE);

        let state = result.decay_state.unwrap();
        assert_eq!(state.last_decay_block, 100); // Unchanged
        assert_eq!(state.decays_this_epoch, 1); // Unchanged
    }

    #[test]
    fn test_and_decay_after_rate_limit_expires() {
        // Transfer after rate limit expires should trigger decay
        let mut tags = ClusterTagVector::single_at_block(ClusterId(1), 0);
        tags.decay_state = Some(DecayState {
            last_decay_block: 100,
            decays_this_epoch: 1,
            epoch_start_block: 0,
        });

        let inputs = vec![(tags, 1000u64, 0u64)];

        // 400 blocks since last decay (>= 360 min)
        let (result, decay_applied) = ClusterTagVector::merge_weighted_with_and_decay(
            &inputs, 500,    // current_block
            50_000, // 5% decay
            360,    // min_blocks_between
            12,     // max_decays_per_epoch
            8_640,  // epoch_blocks
        );

        assert!(decay_applied, "Decay should apply after rate limit expires");
        assert_eq!(result.get_weight(ClusterId(1)), 950_000);

        let state = result.decay_state.unwrap();
        assert_eq!(state.last_decay_block, 500);
        assert_eq!(state.decays_this_epoch, 2);
    }

    #[test]
    fn test_and_decay_epoch_cap() {
        // Epoch cap should prevent decay even if rate limit allows
        let mut tags = ClusterTagVector::single_at_block(ClusterId(1), 0);
        tags.decay_state = Some(DecayState {
            last_decay_block: 100,
            decays_this_epoch: 12, // Already at cap
            epoch_start_block: 0,
        });

        let inputs = vec![(tags, 1000u64, 0u64)];

        let (result, decay_applied) = ClusterTagVector::merge_weighted_with_and_decay(
            &inputs, 500,    // current_block
            50_000, // 5% decay
            360,    // min_blocks_between
            12,     // max_decays_per_epoch (at cap)
            8_640,  // epoch_blocks
        );

        assert!(!decay_applied, "Epoch cap should block decay");
        assert_eq!(result.get_weight(ClusterId(1)), TAG_WEIGHT_SCALE);
    }

    #[test]
    fn test_and_decay_epoch_reset() {
        // New epoch should reset decay counter
        let mut tags = ClusterTagVector::single_at_block(ClusterId(1), 0);
        tags.decay_state = Some(DecayState {
            last_decay_block: 100,
            decays_this_epoch: 12, // At cap
            epoch_start_block: 0,
        });

        let inputs = vec![(tags, 1000u64, 0u64)];

        // 9000 blocks later (> 8640 epoch), new epoch starts
        let (result, decay_applied) = ClusterTagVector::merge_weighted_with_and_decay(
            &inputs, 9100,   // current_block (new epoch)
            50_000, // 5% decay
            360,    // min_blocks_between
            12,     // max_decays_per_epoch
            8_640,  // epoch_blocks
        );

        assert!(decay_applied, "New epoch should allow decay");
        assert_eq!(result.get_weight(ClusterId(1)), 950_000);

        let state = result.decay_state.unwrap();
        assert_eq!(state.epoch_start_block, 9100); // Reset to current
        assert_eq!(state.decays_this_epoch, 1); // Reset counter
    }

    #[test]
    fn test_and_decay_multi_input_combines_state() {
        // Multiple inputs should combine their decay states
        let mut tags1 = ClusterTagVector::single_at_block(ClusterId(1), 0);
        tags1.decay_state = Some(DecayState {
            last_decay_block: 100,
            decays_this_epoch: 2,
            epoch_start_block: 0,
        });

        let mut tags2 = ClusterTagVector::single_at_block(ClusterId(2), 0);
        tags2.decay_state = Some(DecayState {
            last_decay_block: 200, // More recent
            decays_this_epoch: 4,
            epoch_start_block: 0,
        });

        let inputs = vec![(tags1, 1000u64, 0u64), (tags2, 1000u64, 0u64)];

        let (result, decay_applied) = ClusterTagVector::merge_weighted_with_and_decay(
            &inputs, 600,    // current_block (>= 200 + 360)
            50_000, // 5% decay
            360,    // min_blocks_between
            12,     // max_decays_per_epoch
            8_640,  // epoch_blocks
        );

        assert!(decay_applied);
        // Both clusters should be at 50% each, then 5% decay applied
        assert_eq!(result.get_weight(ClusterId(1)), 475_000); // 50% * 95%
        assert_eq!(result.get_weight(ClusterId(2)), 475_000);

        let state = result.decay_state.unwrap();
        // last_decay_block should be max of inputs
        assert_eq!(state.last_decay_block, 600);
    }

    #[test]
    fn test_and_decay_backward_compatibility() {
        // Legacy UTXOs without decay state should work
        let legacy = ClusterTagVector::single(ClusterId(1)); // No decay_state

        let inputs = vec![(legacy, 1000u64, 500u64)]; // creation_block = 500

        let (result, decay_applied) = ClusterTagVector::merge_weighted_with_and_decay(
            &inputs, 1000,   // current_block
            50_000, // 5% decay
            360,    // min_blocks_between
            12,     // max_decays_per_epoch
            8_640,  // epoch_blocks
        );

        // Should treat legacy as "never decayed" and apply first decay
        assert!(decay_applied);
        assert_eq!(result.get_weight(ClusterId(1)), 950_000);
        assert!(result.decay_state.is_some());
    }

    #[test]
    fn test_and_decay_wash_trading_resistance() {
        // Simulate 100 rapid wash trades - should only get 1 decay
        let mut tags = ClusterTagVector::single_at_block(ClusterId(1), 0);
        let mut total_decays = 0;

        for block in 0..100 {
            let inputs = vec![(tags.clone(), 1000u64, 0u64)];

            let (new_tags, decay_applied) = ClusterTagVector::merge_weighted_with_and_decay(
                &inputs, block, 50_000, // 5% decay
                360,    // min_blocks_between (1 hour)
                12,     // max_decays_per_epoch
                8_640,  // epoch_blocks
            );

            if decay_applied {
                total_decays += 1;
            }
            tags = new_tags;
        }

        // Only the first transfer should have decayed
        assert_eq!(
            total_decays, 1,
            "Rapid wash trading should only allow 1 decay"
        );
        assert_eq!(
            tags.get_weight(ClusterId(1)),
            950_000,
            "Should be 95% (one 5% decay)"
        );
    }

    // ------------------------------------------------------------------
    // #1858: overflow-safe `merge_weighted` / `merge_weighted_with_and_decay`
    // ------------------------------------------------------------------

    /// The pre-#1858 `merge_weighted`, verbatim (u64 value sum, u64 decay
    /// multiply, `as u32` truncation). Kept as a test-only reference so the
    /// bit-identical claim for `is_valid()` inputs is checked, not assumed.
    fn legacy_merge_weighted(
        inputs: &[(ClusterTagVector, u64)],
        decay_rate: u32,
    ) -> ClusterTagVector {
        let total_value: u64 = inputs.iter().map(|(_, v)| *v).sum();
        if total_value == 0 {
            return ClusterTagVector::empty();
        }
        let mut cluster_weights: Vec<(ClusterId, u64)> = Vec::new();
        for (tags, value) in inputs {
            for entry in &tags.entries {
                let contribution =
                    ((*value as u128) * (entry.weight as u128)) / (total_value as u128);
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
        let retention = TAG_WEIGHT_SCALE.saturating_sub(decay_rate);
        let pairs: Vec<(ClusterId, u32)> = cluster_weights
            .into_iter()
            .map(|(id, w)| {
                let decayed = (w * (retention as u64)) / (TAG_WEIGHT_SCALE as u64);
                (id, decayed as u32)
            })
            .collect();
        ClusterTagVector::from_pairs(&pairs)
    }

    /// The pre-#1858 `merge_weighted_with_and_decay`, verbatim.
    fn legacy_merge_weighted_with_and_decay(
        inputs: &[(ClusterTagVector, u64, u64)],
        current_block: u64,
        decay_rate_per_hop: u32,
        min_blocks_between_decays: u64,
        max_decays_per_epoch: u32,
        epoch_blocks: u64,
    ) -> (ClusterTagVector, bool) {
        let total_value: u64 = inputs.iter().map(|(_, v, _)| *v).sum();
        if total_value == 0 {
            return (ClusterTagVector::empty_at_block(current_block), false);
        }
        let mut cluster_weights: Vec<(ClusterId, u64)> = Vec::new();
        let mut combined_last_decay_block: u64 = 0;
        let mut combined_decays_this_epoch: u32 = 0;
        let mut combined_epoch_start_block: u64 = u64::MAX;
        for (tags, value, creation_block) in inputs {
            let decay_state = tags.decay_state_or_default(*creation_block);
            combined_last_decay_block = combined_last_decay_block.max(decay_state.last_decay_block);
            combined_decays_this_epoch = combined_decays_this_epoch.saturating_add(
                (decay_state.decays_this_epoch as u64 * *value / total_value.max(1)) as u32,
            );
            combined_epoch_start_block =
                combined_epoch_start_block.min(decay_state.epoch_start_block);
            for entry in &tags.entries {
                let contribution =
                    ((*value as u128) * (entry.weight as u128)) / (total_value as u128);
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
        if combined_epoch_start_block == u64::MAX {
            combined_epoch_start_block = current_block;
        }
        let mut epoch_start = combined_epoch_start_block;
        let mut decays_this_epoch = combined_decays_this_epoch;
        if current_block.saturating_sub(epoch_start) >= epoch_blocks {
            epoch_start = current_block;
            decays_this_epoch = 0;
        }
        let never_decayed = combined_last_decay_block == 0 && decays_this_epoch == 0;
        let time_eligible = never_decayed
            || current_block.saturating_sub(combined_last_decay_block) >= min_blocks_between_decays;
        let epoch_cap_ok = max_decays_per_epoch == 0 || decays_this_epoch < max_decays_per_epoch;
        let decay_applies = time_eligible && epoch_cap_ok;
        let pairs: Vec<(ClusterId, u32)> = if decay_applies {
            let retention = TAG_WEIGHT_SCALE.saturating_sub(decay_rate_per_hop);
            cluster_weights
                .into_iter()
                .map(|(id, w)| {
                    let decayed = (w * (retention as u64)) / (TAG_WEIGHT_SCALE as u64);
                    (id, decayed as u32)
                })
                .collect()
        } else {
            cluster_weights
                .into_iter()
                .map(|(id, w)| (id, w as u32))
                .collect()
        };
        let mut result = ClusterTagVector::from_pairs(&pairs);
        result.decay_state = Some(DecayState {
            last_decay_block: if decay_applies {
                current_block
            } else {
                combined_last_decay_block
            },
            decays_this_epoch: if decay_applies {
                decays_this_epoch + 1
            } else {
                decays_this_epoch
            },
            epoch_start_block: epoch_start,
        });
        (result, decay_applies)
    }

    /// `count` copies of `(cluster, weight)`: the #1840/#1858 malformed
    /// vector (no dedup, no 16-entry cap, total far above 100%).
    fn repeated(cluster: u64, weight: u32, count: usize) -> ClusterTagVector {
        ClusterTagVector {
            entries: vec![
                ClusterTagEntry {
                    cluster_id: ClusterId(cluster),
                    weight,
                };
                count
            ],
            decay_state: None,
        }
    }

    fn pairs_of(tags: &ClusterTagVector) -> Vec<(u64, u32)> {
        tags.entries
            .iter()
            .map(|e| (e.cluster_id.0, e.weight))
            .collect()
    }

    /// #1858 (Lean `mergeWeighted_overflow_panics`): 4,295 entries of
    /// `u32::MAX` at rate 0, and 4,522 at the wallet rate 50,000, used to
    /// overflow the u64 decay multiply. They now saturate to `u32::MAX`.
    #[test]
    fn merge_weighted_saturates_instead_of_panicking() {
        for (count, rate) in [(4_295usize, 0u32), (4_522, 50_000), (100_000, 0)] {
            let out = ClusterTagVector::merge_weighted(&[(repeated(7, u32::MAX, count), 1)], rate);
            assert_eq!(
                pairs_of(&out),
                vec![(7, u32::MAX)],
                "count {count} rate {rate}"
            );
            let (out, applied) = ClusterTagVector::merge_weighted_with_and_decay(
                &[(repeated(7, u32::MAX, count), 1, 0)],
                10,
                rate,
                360,
                12,
                8_640,
            );
            assert!(applied);
            assert_eq!(pairs_of(&out), vec![(7, u32::MAX)]);
        }
    }

    /// The exact overflow boundary, pinned against the pre-fix reference:
    /// 4,294 entries did not overflow the multiply, 4,295 did.
    #[test]
    fn merge_weighted_overflow_boundary_4294_vs_4295() {
        let below = repeated(7, u32::MAX, 4_294);
        let at = repeated(7, u32::MAX, 4_295);
        // w * 10^6 fits u64 at 4,294 and not at 4,295.
        let w = |n: u64| n * u32::MAX as u64;
        assert!(w(4_294).checked_mul(TAG_WEIGHT_SCALE as u64).is_some());
        assert!(w(4_295).checked_mul(TAG_WEIGHT_SCALE as u64).is_none());
        // Old code did not panic at 4,294 (it wrapped through `as u32`).
        let _ = legacy_merge_weighted(&[(below.clone(), 1)], 0);
        // New code: both sides saturate identically.
        for v in [below, at] {
            let out = ClusterTagVector::merge_weighted(&[(v, 1)], 0);
            assert_eq!(pairs_of(&out), vec![(7, u32::MAX)]);
        }
    }

    /// The pre-fix reference really did panic at 4,295 (this is what the
    /// node hit, #1858); keeps the boundary test honest.
    #[test]
    #[should_panic(expected = "overflow")]
    fn legacy_merge_weighted_panics_at_4295() {
        let _ = legacy_merge_weighted(&[(repeated(7, u32::MAX, 4_295), 1)], 0);
    }

    /// Lean `mergeWeighted_decay_not_antitone`: the old `as u32` wrapped, so
    /// more input weight could yield LESS output weight. With the clamp the
    /// output is monotone non-decreasing in the input weight.
    #[test]
    fn merge_weighted_decay_is_monotone_past_u32() {
        // Old code: 2 entries of u32::MAX wrap to u32::MAX - 1 < u32::MAX.
        let one = legacy_merge_weighted(&[(repeated(7, u32::MAX, 1), 1)], 0);
        let two = legacy_merge_weighted(&[(repeated(7, u32::MAX, 2), 1)], 0);
        assert!(two.get_weight(ClusterId(7)) < one.get_weight(ClusterId(7)));

        for rate in [0u32, 50_000, 500_000] {
            let mut prev = 0u32;
            for count in [1usize, 2, 3, 17, 1_000, 4_294, 4_295, 4_522, 10_000] {
                let out =
                    ClusterTagVector::merge_weighted(&[(repeated(7, u32::MAX, count), 1)], rate);
                let w = out.get_weight(ClusterId(7));
                assert!(w >= prev, "rate {rate}: count {count} gave {w} < {prev}");
                prev = w;
            }
        }
        // And for weights that cross u32::MAX only after accumulation.
        let mut prev = 0u32;
        for count in 1..=40usize {
            let out = ClusterTagVector::merge_weighted(&[(repeated(7, 200_000_000, count), 1)], 0);
            let w = out.get_weight(ClusterId(7));
            assert!(w >= prev, "count {count} gave {w} < {prev}");
            prev = w;
        }
        assert_eq!(prev, u32::MAX);
    }

    /// The non-decay branch of `merge_weighted_with_and_decay` clamps too
    /// (old: `w as u32`).
    #[test]
    fn merge_weighted_with_and_decay_no_decay_branch_clamps() {
        let mut tags = repeated(7, u32::MAX, 2);
        // Decayed at block 100: not time-eligible at block 101.
        tags.decay_state = Some(DecayState {
            last_decay_block: 100,
            decays_this_epoch: 1,
            epoch_start_block: 100,
        });
        let (out, applied) = ClusterTagVector::merge_weighted_with_and_decay(
            &[(tags.clone(), 1, 0)],
            101,
            50_000,
            360,
            12,
            8_640,
        );
        assert!(!applied);
        assert_eq!(pairs_of(&out), vec![(7, u32::MAX)]);
        // The old code wrapped to u32::MAX - 1.
        let (old, _) =
            legacy_merge_weighted_with_and_decay(&[(tags, 1, 0)], 101, 50_000, 360, 12, 8_640);
        assert_eq!(pairs_of(&old), vec![(7, u32::MAX - 1)]);
    }

    /// The value sum is taken in u128: two inputs of `u64::MAX` used to
    /// panic on the `u64` sum.
    #[test]
    fn merge_weighted_total_value_overflow_does_not_panic() {
        let inputs = [
            (ClusterTagVector::single(ClusterId(1)), u64::MAX),
            (ClusterTagVector::single(ClusterId(2)), u64::MAX),
        ];
        let out = ClusterTagVector::merge_weighted(&inputs, 0);
        assert_eq!(pairs_of(&out), vec![(1, 500_000), (2, 500_000)]);
        assert!(out.is_valid());

        let inputs3 = [
            (ClusterTagVector::single(ClusterId(1)), u64::MAX, 0),
            (ClusterTagVector::single(ClusterId(2)), u64::MAX, 0),
        ];
        let (out, _) =
            ClusterTagVector::merge_weighted_with_and_decay(&inputs3, 10, 0, 360, 12, 8_640);
        assert_eq!(pairs_of(&out), vec![(1, 500_000), (2, 500_000)]);
    }

    #[test]
    #[should_panic(expected = "overflow")]
    fn legacy_merge_weighted_panics_on_value_sum_overflow() {
        let inputs = [
            (ClusterTagVector::single(ClusterId(1)), u64::MAX),
            (ClusterTagVector::single(ClusterId(2)), u64::MAX),
        ];
        let _ = legacy_merge_weighted(&inputs, 0);
    }

    /// Honest-wallet outputs are pinned: a table of valid inputs with their
    /// exact expected merged vectors, identical to the pre-fix reference and
    /// themselves `is_valid()`.
    #[test]
    fn merge_weighted_valid_inputs_unchanged() {
        let s = ClusterTagVector::single;
        let fp = |p: &[(u64, u32)]| {
            ClusterTagVector::from_pairs(
                &p.iter()
                    .map(|(c, w)| (ClusterId(*c), *w))
                    .collect::<Vec<_>>(),
            )
        };
        #[allow(clippy::type_complexity)]
        let table: Vec<(Vec<(ClusterTagVector, u64)>, u32, Vec<(u64, u32)>)> = vec![
            // single input
            (vec![(s(ClusterId(1)), 1000)], 0, vec![(1, 1_000_000)]),
            // equal
            (
                vec![(s(ClusterId(1)), 100), (s(ClusterId(2)), 100)],
                0,
                vec![(1, 500_000), (2, 500_000)],
            ),
            // unequal
            (
                vec![(s(ClusterId(1)), 300), (s(ClusterId(2)), 100)],
                0,
                vec![(1, 750_000), (2, 250_000)],
            ),
            // shared cluster (stable tie order: 1 before 3)
            (
                vec![
                    (fp(&[(1, 600_000), (2, 400_000)]), 100),
                    (fp(&[(1, 200_000), (3, 800_000)]), 100),
                ],
                0,
                vec![(1, 400_000), (3, 400_000), (2, 200_000)],
            ),
            // rounding
            (
                vec![(s(ClusterId(1)), 1), (s(ClusterId(2)), 2)],
                0,
                vec![(2, 666_666), (1, 333_333)],
            ),
            // wallet decay 5%
            (vec![(s(ClusterId(1)), 1000)], 50_000, vec![(1, 950_000)]),
            (
                vec![(s(ClusterId(1)), 300), (s(ClusterId(2)), 100)],
                50_000,
                vec![(1, 712_500), (2, 237_500)],
            ),
            // empty / zero value
            (vec![(ClusterTagVector::empty(), 5)], 0, vec![]),
            (vec![(s(ClusterId(1)), 0)], 0, vec![]),
        ];
        for (inputs, rate, expected) in table {
            let new = ClusterTagVector::merge_weighted(&inputs, rate);
            let old = legacy_merge_weighted(&inputs, rate);
            assert_eq!(
                pairs_of(&new),
                expected,
                "inputs {:?}",
                pairs_of(&inputs[0].0)
            );
            assert_eq!(new, old);
            assert!(new.is_valid());
        }
    }

    mod merge_weighted_reference_equivalence {
        use super::*;
        use proptest::prelude::*;

        /// A random `is_valid()` tag vector: up to 16 distinct clusters drawn
        /// from a small id space (so inputs share clusters), each weight in
        /// [MIN_STORED_WEIGHT, 62_500] so the total is at most 100%.
        fn valid_tags() -> impl Strategy<Value = ClusterTagVector> {
            proptest::collection::btree_map(0u64..24, MIN_STORED_WEIGHT..=62_500u32, 0..=16)
                .prop_map(|m| {
                    let pairs: Vec<(ClusterId, u32)> =
                        m.into_iter().map(|(c, w)| (ClusterId(c), w)).collect();
                    ClusterTagVector::from_pairs(&pairs)
                })
        }

        /// A valid vector with arbitrary (not just small) weights summing to
        /// at most 100%, e.g. one dominant cluster.
        fn valid_tags_wide() -> impl Strategy<Value = ClusterTagVector> {
            proptest::collection::btree_map(0u64..24, MIN_STORED_WEIGHT..=TAG_WEIGHT_SCALE, 0..=4)
                .prop_map(|m| {
                    let mut left = TAG_WEIGHT_SCALE;
                    let mut pairs = Vec::new();
                    for (c, w) in m {
                        let w = w.min(left);
                        left -= w;
                        pairs.push((ClusterId(c), w));
                    }
                    ClusterTagVector::from_pairs(&pairs)
                })
        }

        fn any_valid() -> impl Strategy<Value = ClusterTagVector> {
            prop_oneof![valid_tags(), valid_tags_wide()]
        }

        /// Values whose sum over at most 8 inputs fits u64 (the honest case).
        fn value() -> impl Strategy<Value = u64> {
            prop_oneof![0u64..=1_000_000, 0u64..=(u64::MAX / 8)]
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(2_000))]

            /// For every `is_valid()` input whose value sum fits u64, the
            /// overflow-safe `merge_weighted` is bit-identical to the pre-fix
            /// reference, and its output is itself `is_valid()`.
            #[test]
            fn merge_weighted_matches_legacy_on_valid_inputs(
                inputs in proptest::collection::vec((any_valid(), value()), 0..=8),
                rate in prop_oneof![Just(0u32), Just(50_000u32), 0u32..=TAG_WEIGHT_SCALE, any::<u32>()],
            ) {
                prop_assert!(inputs.iter().all(|(t, _)| t.is_valid()));
                let new = ClusterTagVector::merge_weighted(&inputs, rate);
                let old = legacy_merge_weighted(&inputs, rate);
                prop_assert_eq!(&new, &old);
                prop_assert!(new.is_valid());
            }

            /// Same for `merge_weighted_with_and_decay`, over random decay
            /// states. Values are capped at 2^59 so the pre-fix
            /// `decays_this_epoch as u64 * value` (<= 12 * 2^59) cannot
            /// overflow in the reference either.
            #[test]
            fn merge_weighted_with_and_decay_matches_legacy_on_valid_inputs(
                inputs in proptest::collection::vec(
                    (
                        any_valid(),
                        0u64..=(1u64 << 59),
                        0u64..20_000,
                        proptest::option::of((0u64..20_000, 0u32..=12, 0u64..20_000)),
                    ),
                    0..=8,
                ),
                current_block in 0u64..40_000,
                rate in prop_oneof![Just(50_000u32), 0u32..=TAG_WEIGHT_SCALE],
                min_between in 0u64..1_000,
                max_per_epoch in 0u32..=12,
                epoch_blocks in 1u64..20_000,
            ) {
                let inputs: Vec<(ClusterTagVector, u64, u64)> = inputs
                    .into_iter()
                    .map(|(mut t, v, created, ds)| {
                        t.decay_state = ds.map(|(l, d, e)| DecayState {
                            last_decay_block: l,
                            decays_this_epoch: d,
                            epoch_start_block: e,
                        });
                        (t, v, created)
                    })
                    .collect();
                let new = ClusterTagVector::merge_weighted_with_and_decay(
                    &inputs, current_block, rate, min_between, max_per_epoch, epoch_blocks,
                );
                let old = legacy_merge_weighted_with_and_decay(
                    &inputs, current_block, rate, min_between, max_per_epoch, epoch_blocks,
                );
                prop_assert_eq!(&new, &old);
                prop_assert!(new.0.is_valid());
            }

            /// No input, valid or not, makes the new code panic.
            #[test]
            fn merge_weighted_never_panics(
                entries in proptest::collection::vec(
                    proptest::collection::vec((0u64..4, any::<u32>()), 0..64),
                    0..4,
                ),
                values in proptest::collection::vec(any::<u64>(), 4),
                states in proptest::collection::vec(
                    proptest::option::of((any::<u64>(), any::<u32>(), any::<u64>())),
                    4,
                ),
                rate in any::<u32>(),
                current_block in any::<u64>(),
                min_between in any::<u64>(),
                max_per_epoch in prop_oneof![Just(0u32), any::<u32>()],
                epoch_blocks in any::<u64>(),
            ) {
                let inputs: Vec<(ClusterTagVector, u64)> = entries
                    .into_iter()
                    .zip(values)
                    .map(|(es, v)| {
                        (
                            ClusterTagVector {
                                entries: es
                                    .into_iter()
                                    .map(|(c, w)| ClusterTagEntry { cluster_id: ClusterId(c), weight: w })
                                    .collect(),
                                decay_state: None,
                            },
                            v,
                        )
                    })
                    .collect();
                let _ = ClusterTagVector::merge_weighted(&inputs, rate);
                let with_blocks: Vec<_> = inputs
                    .into_iter()
                    .zip(states)
                    .map(|((mut t, v), ds)| {
                        t.decay_state = ds.map(|(l, d, e)| DecayState {
                            last_decay_block: l,
                            decays_this_epoch: d,
                            epoch_start_block: e,
                        });
                        (t, v, 0u64)
                    })
                    .collect();
                let _ = ClusterTagVector::merge_weighted_with_and_decay(
                    &with_blocks, current_block, rate, min_between, max_per_epoch, epoch_blocks,
                );
            }
        }
    }
}
