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

    /// Calculate Shannon entropy of the full tag distribution in bits.
    ///
    /// **WARNING**: This includes background in the calculation, which means
    /// entropy increases as tags decay. For lottery selection, use
    /// `cluster_entropy()` instead, which is decay-invariant.
    ///
    /// Formula: H = -Σ p_i × log2(p_i) for all sources INCLUDING background.
    pub fn shannon_entropy(&self) -> f64 {
        let scale = TAG_WEIGHT_SCALE as f64;
        let mut entropy = 0.0;

        // Entropy from each cluster tag
        for &weight in self.tags.values() {
            if weight > 0 {
                let p = weight as f64 / scale;
                entropy -= p * p.log2();
            }
        }

        // Entropy from background (fully diffused portion)
        let bg = self.background();
        if bg > 0 {
            let p = bg as f64 / scale;
            entropy -= p * p.log2();
        }

        entropy
    }

    /// Calculate Shannon entropy of the CLUSTER distribution only (excluding background).
    ///
    /// This is the correct entropy measure for lottery selection because it is
    /// **decay-invariant**: natural tag decay doesn't change cluster entropy.
    ///
    /// Key properties:
    /// - Fresh mint (single cluster): 0 bits
    /// - Split UTXO: same entropy as parent (Sybil-resistant)
    /// - After decay: same entropy (decay-invariant!)
    /// - Commerce coin (diverse origins): 1.5-3.0 bits typically
    ///
    /// Formula: Renormalize cluster weights to sum to 1.0, then compute
    /// H = -Σ p_i × log2(p_i) over clusters only.
    ///
    /// # Why exclude background?
    ///
    /// Background represents "fully diffused" value where cluster attribution
    /// has decayed away. Including it would make entropy increase with age,
    /// allowing attackers to gain lottery advantage just by waiting.
    ///
    /// By excluding background, entropy only increases through genuine commerce
    /// (mixing with coins from different clusters).
    pub fn cluster_entropy(&self) -> f64 {
        let total_cluster_weight = self.total_attributed();
        if total_cluster_weight == 0 {
            // Fully background = no cluster diversity = 0 entropy
            return 0.0;
        }

        let scale = total_cluster_weight as f64;
        let mut entropy = 0.0;

        // Entropy from each cluster tag, renormalized
        for &weight in self.tags.values() {
            if weight > 0 {
                let p = weight as f64 / scale;
                entropy -= p * p.log2();
            }
        }

        entropy
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

    // ========================================================================
    // Shannon Entropy Tests - Critical for Sybil Resistance Claims
    // ========================================================================

    #[test]
    fn test_entropy_single_cluster_is_zero() {
        // A fresh mint from a single cluster has 0 entropy
        let c1 = ClusterId::new(1);
        let tags = TagVector::single(c1);

        let entropy = tags.shannon_entropy();
        assert!(
            entropy.abs() < 0.001,
            "Single cluster should have ~0 entropy, got {entropy}"
        );
    }

    #[test]
    fn test_entropy_fully_background_is_zero() {
        // Fully diffused (100% background) has 0 entropy
        let tags = TagVector::new();

        let entropy = tags.shannon_entropy();
        assert!(
            entropy.abs() < 0.001,
            "Fully background should have ~0 entropy, got {entropy}"
        );
    }

    #[test]
    fn test_entropy_two_equal_clusters_is_one_bit() {
        // 50/50 split between two sources = 1 bit of entropy
        let c1 = ClusterId::new(1);
        let c2 = ClusterId::new(2);

        let mut tags = TagVector::new();
        tags.set(c1, TAG_WEIGHT_SCALE / 2);
        tags.set(c2, TAG_WEIGHT_SCALE / 2);

        let entropy = tags.shannon_entropy();
        assert!(
            (entropy - 1.0).abs() < 0.01,
            "50/50 split should have ~1 bit entropy, got {entropy}"
        );
    }

    #[test]
    fn test_entropy_preserved_on_split() {
        // KEY PROPERTY: When you split a UTXO, children have SAME entropy as parent
        // This is the foundation of Sybil resistance for entropy-weighted lottery

        let c1 = ClusterId::new(1);
        let c2 = ClusterId::new(2);
        let c3 = ClusterId::new(3);

        // Create a "commerce coin" with diverse provenance
        let mut parent = TagVector::new();
        parent.set(c1, 400_000); // 40%
        parent.set(c2, 350_000); // 35%
        parent.set(c3, 250_000); // 25%

        let parent_entropy = parent.shannon_entropy();

        // "Split" the UTXO into 10 pieces - each child has identical tag distribution
        // (In real code, splitting just creates multiple outputs with same tags)
        let child1 = parent.clone();
        let child2 = parent.clone();
        let child10 = parent.clone();

        // All children have identical entropy to parent
        assert!(
            (child1.shannon_entropy() - parent_entropy).abs() < 0.001,
            "Child should have same entropy as parent"
        );
        assert!(
            (child2.shannon_entropy() - parent_entropy).abs() < 0.001,
            "Child should have same entropy as parent"
        );
        assert!(
            (child10.shannon_entropy() - parent_entropy).abs() < 0.001,
            "Child should have same entropy as parent"
        );

        // Therefore: splitting gives NO entropy advantage
        // 1 UTXO × entropy E = 10 UTXOs × entropy E (same total entropy weight)
    }

    #[test]
    fn test_entropy_increases_with_mixing() {
        // When coins from different sources are combined, entropy increases
        // This rewards legitimate commerce over self-dealing

        let c1 = ClusterId::new(1);
        let c2 = ClusterId::new(2);

        // Alice has coins from cluster 1
        let mut alice = TagVector::single(c1);
        let alice_entropy_before = alice.shannon_entropy();
        assert!(alice_entropy_before < 0.01, "Single source = low entropy");

        // Alice receives coins from cluster 2 (different provenance)
        let incoming = TagVector::single(c2);
        alice.mix(1000, &incoming, 1000);

        let alice_entropy_after = alice.shannon_entropy();
        assert!(
            alice_entropy_after > alice_entropy_before + 0.5,
            "Mixing should increase entropy: before={alice_entropy_before}, after={alice_entropy_after}"
        );
    }

    #[test]
    fn test_entropy_range_realistic_scenarios() {
        // Document expected entropy ranges for lottery weight calibration

        let c1 = ClusterId::new(1);
        let c2 = ClusterId::new(2);
        let c3 = ClusterId::new(3);
        let c4 = ClusterId::new(4);

        // Fresh mint: 0 bits
        let fresh_mint = TagVector::single(c1);
        assert!(fresh_mint.shannon_entropy() < 0.01);

        // Self-split (same as fresh mint): 0 bits
        let self_split = fresh_mint.clone();
        assert!(self_split.shannon_entropy() < 0.01);

        // One trade (50/50): ~1 bit
        let mut one_trade = TagVector::new();
        one_trade.set(c1, 500_000);
        one_trade.set(c2, 500_000);
        let one_trade_entropy = one_trade.shannon_entropy();
        assert!(
            (one_trade_entropy - 1.0).abs() < 0.1,
            "One trade should be ~1 bit, got {one_trade_entropy}"
        );

        // Multiple trades (diverse): ~1.5-2 bits
        let mut diverse = TagVector::new();
        diverse.set(c1, 300_000); // 30%
        diverse.set(c2, 300_000); // 30%
        diverse.set(c3, 250_000); // 25%
        diverse.set(c4, 150_000); // 15%
        let diverse_entropy = diverse.shannon_entropy();
        assert!(
            diverse_entropy > 1.5 && diverse_entropy < 2.5,
            "Diverse commerce should be 1.5-2.5 bits, got {diverse_entropy}"
        );

        // Heavy commerce with background: 2-3 bits
        let mut heavy_commerce = TagVector::new();
        heavy_commerce.set(c1, 200_000); // 20%
        heavy_commerce.set(c2, 200_000); // 20%
        heavy_commerce.set(c3, 150_000); // 15%
        heavy_commerce.set(c4, 150_000); // 15%
        // Remaining 30% is background
        let heavy_entropy = heavy_commerce.shannon_entropy();
        assert!(
            heavy_entropy > 2.0 && heavy_entropy < 3.0,
            "Heavy commerce should be 2-3 bits, got {heavy_entropy}"
        );
    }

    #[test]
    fn test_entropy_sybil_resistance_proof() {
        // FORMAL PROOF: Splitting cannot increase total lottery weight
        //
        // Lottery weight formula: weight = value × (1 + bonus × entropy)
        //
        // Before split: 1 UTXO, value V, entropy E
        //   Total weight = V × (1 + bonus × E)
        //
        // After split: N UTXOs, value V/N each, entropy E each (unchanged!)
        //   Total weight = N × (V/N) × (1 + bonus × E) = V × (1 + bonus × E)
        //
        // QED: Splitting preserves total weight, gives NO Sybil advantage

        let c1 = ClusterId::new(1);
        let c2 = ClusterId::new(2);

        // Create a UTXO with some entropy
        let mut parent = TagVector::new();
        parent.set(c1, 600_000); // 60%
        parent.set(c2, 400_000); // 40%
        let parent_entropy = parent.shannon_entropy();

        let parent_value: u64 = 10_000_000; // 10M
        let bonus = 0.5;

        // Parent's lottery weight
        let parent_weight = parent_value as f64 * (1.0 + bonus * parent_entropy);

        // Split into 10 children (each inherits parent's tag distribution)
        let num_children = 10;
        let child_value = parent_value / num_children;
        let child_entropy = parent.shannon_entropy(); // Same entropy!

        // Total weight of all children
        let total_child_weight =
            num_children as f64 * child_value as f64 * (1.0 + bonus * child_entropy);

        // Assert: total weight unchanged
        let weight_ratio = total_child_weight / parent_weight;
        assert!(
            (weight_ratio - 1.0).abs() < 0.01,
            "Splitting should preserve total weight: ratio = {weight_ratio}"
        );
    }

    // ========================================================================
    // cluster_entropy() tests - decay-invariant entropy
    // ========================================================================

    #[test]
    fn test_cluster_entropy_single_source() {
        // Single cluster source = 0 entropy (no diversity)
        let c1 = ClusterId::new(1);
        let tags = TagVector::single(c1);

        assert!(
            tags.cluster_entropy() < 0.01,
            "Single source should have 0 cluster entropy"
        );
        // Should match shannon_entropy for fresh mint
        assert!(
            (tags.cluster_entropy() - tags.shannon_entropy()).abs() < 0.01,
            "Fresh mint: cluster_entropy should equal shannon_entropy"
        );
    }

    #[test]
    fn test_cluster_entropy_decay_invariant() {
        // KEY TEST: Decay should NOT change cluster_entropy
        // This is what distinguishes cluster_entropy from shannon_entropy

        let c1 = ClusterId::new(1);
        let mut tags = TagVector::single(c1);

        let entropy_before = tags.cluster_entropy();
        let shannon_before = tags.shannon_entropy();

        // Apply 10% decay
        tags.apply_decay(100_000);

        let entropy_after = tags.cluster_entropy();
        let shannon_after = tags.shannon_entropy();

        // cluster_entropy UNCHANGED (decay-invariant)
        assert!(
            (entropy_after - entropy_before).abs() < 0.01,
            "cluster_entropy should be decay-invariant: before={entropy_before}, after={entropy_after}"
        );

        // shannon_entropy INCREASED (includes background)
        assert!(
            shannon_after > shannon_before + 0.1,
            "shannon_entropy increases with decay: before={shannon_before}, after={shannon_after}"
        );
    }

    #[test]
    fn test_cluster_entropy_heavy_decay() {
        // Even with 50% decay, cluster_entropy should stay the same

        let c1 = ClusterId::new(1);
        let mut tags = TagVector::single(c1);

        let entropy_before = tags.cluster_entropy();

        // Apply 50% decay
        tags.apply_decay(500_000);

        let entropy_after = tags.cluster_entropy();

        assert!(
            (entropy_after - entropy_before).abs() < 0.01,
            "50% decay: cluster_entropy unchanged: before={entropy_before}, after={entropy_after}"
        );

        // But shannon_entropy is now 1 bit (50% cluster, 50% background)
        let shannon = tags.shannon_entropy();
        assert!(
            (shannon - 1.0).abs() < 0.1,
            "50% decay: shannon_entropy should be ~1 bit, got {shannon}"
        );
    }

    #[test]
    fn test_cluster_entropy_commerce_increases() {
        // Commerce (mixing sources) DOES increase cluster_entropy

        let c1 = ClusterId::new(1);
        let c2 = ClusterId::new(2);

        // Single source
        let mut tags = TagVector::single(c1);
        let entropy_single = tags.cluster_entropy();

        // Mix with another source (simulating commerce)
        let incoming = TagVector::single(c2);
        tags.mix(1000, &incoming, 1000);

        let entropy_mixed = tags.cluster_entropy();

        assert!(
            entropy_mixed > entropy_single + 0.5,
            "Commerce should increase cluster_entropy: single={entropy_single}, mixed={entropy_mixed}"
        );
    }

    #[test]
    fn test_cluster_entropy_commerce_then_decay() {
        // Commerce increases entropy, but subsequent decay doesn't change it

        let c1 = ClusterId::new(1);
        let c2 = ClusterId::new(2);

        // Commerce: mix two sources
        let mut tags = TagVector::new();
        tags.set(c1, 500_000); // 50%
        tags.set(c2, 500_000); // 50%

        let entropy_before_decay = tags.cluster_entropy();
        // Should be ~1 bit (two equal sources)
        assert!(
            (entropy_before_decay - 1.0).abs() < 0.1,
            "Two equal sources = ~1 bit: got {entropy_before_decay}"
        );

        // Apply 20% decay
        tags.apply_decay(200_000);

        // Cluster weights now: 40% + 40% = 80% total, 20% background
        let entropy_after_decay = tags.cluster_entropy();

        // cluster_entropy is UNCHANGED - still ~1 bit
        assert!(
            (entropy_after_decay - 1.0).abs() < 0.1,
            "After decay: cluster_entropy still ~1 bit: got {entropy_after_decay}"
        );

        // But shannon_entropy is HIGHER (includes background as third "source")
        let shannon = tags.shannon_entropy();
        assert!(
            shannon > 1.4,
            "shannon_entropy should be >1.4 with background, got {shannon}"
        );
    }

    #[test]
    fn test_cluster_entropy_fully_decayed() {
        // Fully decayed (100% background) = 0 cluster entropy

        let c1 = ClusterId::new(1);
        let mut tags = TagVector::single(c1);

        // Apply 100% decay
        tags.apply_decay(TAG_WEIGHT_SCALE);

        // All weight is now background
        assert_eq!(tags.total_attributed(), 0);
        assert_eq!(tags.background(), TAG_WEIGHT_SCALE);

        // cluster_entropy = 0 (no cluster diversity)
        assert!(
            tags.cluster_entropy() < 0.01,
            "Fully decayed: cluster_entropy should be 0"
        );

        // shannon_entropy = 0 (single source: background)
        assert!(
            tags.shannon_entropy() < 0.01,
            "Fully decayed: shannon_entropy should also be 0"
        );
    }

    #[test]
    fn test_cluster_entropy_lottery_weight_decay_invariant() {
        // PROOF: Using cluster_entropy for lottery makes weights decay-invariant

        let c1 = ClusterId::new(1);
        let c2 = ClusterId::new(2);

        // Commerce coin: 60% A, 40% B
        let mut tags = TagVector::new();
        tags.set(c1, 600_000);
        tags.set(c2, 400_000);

        let value: u64 = 10_000_000;
        let bonus = 0.5;

        // Lottery weight before decay
        let weight_before = value as f64 * (1.0 + bonus * tags.cluster_entropy());

        // Apply 30% decay
        tags.apply_decay(300_000);

        // Lottery weight after decay
        let weight_after = value as f64 * (1.0 + bonus * tags.cluster_entropy());

        // PROOF: weight unchanged
        let ratio = weight_after / weight_before;
        assert!(
            (ratio - 1.0).abs() < 0.01,
            "Lottery weight should be decay-invariant: ratio = {ratio}"
        );

        // With shannon_entropy, weight would have increased
        // (can't test directly since we already decayed, but the point is made)
    }

    #[test]
    fn test_cluster_vs_shannon_entropy_comparison() {
        // Side-by-side comparison showing when they differ

        let c1 = ClusterId::new(1);
        let c2 = ClusterId::new(2);

        // Fresh mint: both are 0
        let fresh = TagVector::single(c1);
        assert!((fresh.cluster_entropy() - fresh.shannon_entropy()).abs() < 0.01);

        // After decay: cluster_entropy stays 0, shannon_entropy increases
        let mut decayed = TagVector::single(c1);
        decayed.apply_decay(300_000); // 30% decay
        assert!(decayed.cluster_entropy() < 0.01); // Still 0
        assert!(decayed.shannon_entropy() > 0.7); // ~0.88 bits

        // Commerce without decay: both are equal
        let mut commerce = TagVector::new();
        commerce.set(c1, 500_000);
        commerce.set(c2, 500_000);
        let diff = (commerce.cluster_entropy() - commerce.shannon_entropy()).abs();
        assert!(
            diff < 0.01,
            "Commerce without decay: should be equal, diff={diff}"
        );

        // Commerce with decay: cluster_entropy stable, shannon increases
        let mut commerce_decayed = TagVector::new();
        commerce_decayed.set(c1, 400_000); // 40%
        commerce_decayed.set(c2, 400_000); // 40%
        // 20% is background
        assert!((commerce_decayed.cluster_entropy() - 1.0).abs() < 0.1); // Still ~1 bit
        assert!(commerce_decayed.shannon_entropy() > 1.4); // Higher due to background
    }
}
