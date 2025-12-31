//! Tag vectors representing cluster attribution for coins.

use crate::cluster::ClusterId;
use std::collections::HashMap;

/// Weight of a tag, represented as fixed-point fraction.
///
/// Scale: 1_000_000 = 100% (one million = full attribution).
/// This gives 6 decimal places of precision.
pub type TagWeight = u32;

/// Scale factor for tag weights. 1_000_000 represents 100%.
pub const TAG_WEIGHT_SCALE: TagWeight = 1_000_000;

/// Sparse vector of cluster tags for an account or UTXO.
///
/// Maps cluster IDs to weights indicating what fraction of the value
/// traces back to that cluster's origin. Weights sum to at most
/// TAG_WEIGHT_SCALE, with any remainder representing "background" (fully
/// diffused) attribution.
#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TagVector {
    /// Sparse map of cluster -> weight.
    tags: HashMap<ClusterId, TagWeight>,
}

impl TagVector {
    /// Minimum tag weight to retain (prune smaller tags to background).
    /// 0.01% = 100 in our scale.
    pub const PRUNE_THRESHOLD: TagWeight = 100;

    /// Maximum number of tags to track per vector.
    pub const MAX_TAGS: usize = 32;

    /// Create an empty tag vector (fully background).
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a tag vector fully attributed to one cluster.
    pub fn single(cluster: ClusterId) -> Self {
        let mut tags = HashMap::new();
        tags.insert(cluster, TAG_WEIGHT_SCALE);
        Self { tags }
    }

    /// Get the weight for a specific cluster.
    pub fn get(&self, cluster: ClusterId) -> TagWeight {
        self.tags.get(&cluster).copied().unwrap_or(0)
    }

    /// Set the weight for a specific cluster.
    pub fn set(&mut self, cluster: ClusterId, weight: TagWeight) {
        if weight >= Self::PRUNE_THRESHOLD {
            self.tags.insert(cluster, weight);
        } else {
            self.tags.remove(&cluster);
        }
    }

    /// Total attributed weight (sum of all cluster tags).
    /// The remainder (TAG_WEIGHT_SCALE - total) is "background".
    pub fn total_attributed(&self) -> TagWeight {
        self.tags.values().sum::<TagWeight>().min(TAG_WEIGHT_SCALE)
    }

    /// Background (fully diffused) weight.
    pub fn background(&self) -> TagWeight {
        TAG_WEIGHT_SCALE.saturating_sub(self.total_attributed())
    }

    /// Iterate over all (cluster, weight) pairs.
    pub fn iter(&self) -> impl Iterator<Item = (ClusterId, TagWeight)> + '_ {
        self.tags.iter().map(|(&k, &v)| (k, v))
    }

    /// Number of tracked clusters.
    pub fn len(&self) -> usize {
        self.tags.len()
    }

    /// Returns true if fully background (no cluster attribution).
    pub fn is_empty(&self) -> bool {
        self.tags.is_empty()
    }

    /// Apply decay to all tags, moving decayed mass to background.
    ///
    /// decay_factor is in parts per million (e.g., 50_000 = 5% decay).
    pub fn apply_decay(&mut self, decay_factor: TagWeight) {
        if decay_factor == 0 {
            return;
        }

        let decay_fraction = decay_factor.min(TAG_WEIGHT_SCALE);

        for weight in self.tags.values_mut() {
            // new_weight = weight * (1 - decay_fraction / SCALE)
            let decay_amount =
                (*weight as u64 * decay_fraction as u64 / TAG_WEIGHT_SCALE as u64) as TagWeight;
            *weight = weight.saturating_sub(decay_amount);
        }

        self.prune();
    }

    /// Mix another tag vector into this one with given weights.
    ///
    /// Used when receiving coins: the receiver's tags become a weighted
    /// average of their existing tags and the incoming tags.
    ///
    /// - `self_value`: current value held
    /// - `incoming`: tag vector of incoming coins
    /// - `incoming_value`: value of incoming coins
    pub fn mix(&mut self, self_value: u64, incoming: &TagVector, incoming_value: u64) {
        let total_value = self_value + incoming_value;
        if total_value == 0 {
            return;
        }

        // Collect all cluster IDs from both vectors
        let mut all_clusters: Vec<ClusterId> = self.tags.keys().copied().collect();
        for &cluster in incoming.tags.keys() {
            if !self.tags.contains_key(&cluster) {
                all_clusters.push(cluster);
            }
        }

        // Compute weighted average for each cluster
        for cluster in all_clusters {
            let self_weight = self.get(cluster) as u64;
            let incoming_weight = incoming.get(cluster) as u64;

            // new_weight = (self_value * self_weight + incoming_value * incoming_weight) /
            // total_value
            let numerator = self_value * self_weight + incoming_value * incoming_weight;
            let new_weight = (numerator / total_value) as TagWeight;

            self.set(cluster, new_weight);
        }

        self.prune();
    }

    /// Remove tags below threshold and enforce MAX_TAGS limit.
    fn prune(&mut self) {
        // Remove below-threshold tags
        self.tags.retain(|_, &mut w| w >= Self::PRUNE_THRESHOLD);

        // If still too many, keep only top weights
        if self.tags.len() > Self::MAX_TAGS {
            let mut entries: Vec<_> = self.tags.drain().collect();
            entries.sort_by(|a, b| b.1.cmp(&a.1)); // Sort descending by weight

            self.tags = entries.into_iter().take(Self::MAX_TAGS).collect();
        }
    }

    /// Scale all weights by a factor (used when splitting outputs).
    ///
    /// factor is in parts per million.
    pub fn scale(&mut self, factor: TagWeight) {
        for weight in self.tags.values_mut() {
            *weight = (*weight as u64 * factor as u64 / TAG_WEIGHT_SCALE as u64) as TagWeight;
        }
        self.prune();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_cluster() {
        let c1 = ClusterId::new(1);
        let tags = TagVector::single(c1);

        assert_eq!(tags.get(c1), TAG_WEIGHT_SCALE);
        assert_eq!(tags.background(), 0);
        assert_eq!(tags.total_attributed(), TAG_WEIGHT_SCALE);
    }

    #[test]
    fn test_decay() {
        let c1 = ClusterId::new(1);
        let mut tags = TagVector::single(c1);

        // 10% decay
        tags.apply_decay(100_000);

        // Should have ~90% remaining
        let remaining = tags.get(c1);
        assert!(
            remaining >= 890_000 && remaining <= 910_000,
            "Expected ~900000, got {remaining}"
        );
        assert!(tags.background() > 0);
    }

    #[test]
    fn test_mix() {
        let c1 = ClusterId::new(1);
        let c2 = ClusterId::new(2);

        let mut receiver = TagVector::single(c1); // 100% c1
        let incoming = TagVector::single(c2); // 100% c2

        // Mix equal values
        receiver.mix(1000, &incoming, 1000);

        // Should now be ~50% each
        let w1 = receiver.get(c1);
        let w2 = receiver.get(c2);

        assert!(
            w1 >= 490_000 && w1 <= 510_000,
            "Expected ~500000 for c1, got {w1}"
        );
        assert!(
            w2 >= 490_000 && w2 <= 510_000,
            "Expected ~500000 for c2, got {w2}"
        );
    }

    #[test]
    fn test_prune_threshold() {
        let c1 = ClusterId::new(1);
        let mut tags = TagVector::new();

        // Set weight below threshold
        tags.set(c1, TagVector::PRUNE_THRESHOLD - 1);
        assert_eq!(tags.get(c1), 0); // Should be pruned

        // Set weight at threshold
        tags.set(c1, TagVector::PRUNE_THRESHOLD);
        assert_eq!(tags.get(c1), TagVector::PRUNE_THRESHOLD); // Should be kept
    }
}
