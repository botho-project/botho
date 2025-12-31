//! Age-Based Decay: A stateless alternative to AND-based decay.
//!
//! This module implements decay that uses UTXO age instead of per-UTXO state,
//! eliminating privacy concerns while achieving equivalent attack resistance.
//!
//! # Key Insight
//!
//! Instead of tracking `last_decay_block` and `decays_this_epoch`, we use the
//! fact that every UTXO has a creation block (already public). Decay only
//! applies when spending a UTXO that is at least `min_age` blocks old.
//!
//! # Properties
//!
//! - **Rapid wash trading**: Blocked (new outputs are too young to trigger decay)
//! - **Patient wash trading**: Bounded (max 1 decay per `min_age` blocks)
//! - **No passive decay**: Correct (only decays on spend)
//! - **Privacy**: No additional metadata leaked!
//!
//! # Epoch Cap Emergence
//!
//! With `min_age = 720 blocks` (~2 hours at 10s/block):
//! - Max decays per day: 8640 / 720 = 12
//! - Max decays per week: 60480 / 720 = 84
//!
//! This matches the AND-based epoch cap naturally!

use crate::tag::{TagVector, TagWeight, TAG_WEIGHT_SCALE};

/// Configuration for age-based decay.
#[derive(Clone, Debug)]
pub struct AgeDecayConfig {
    /// Minimum age (in blocks) for a UTXO to trigger decay when spent.
    /// Default: 720 blocks (~2 hours at 10s/block)
    pub min_age_blocks: u64,

    /// Decay rate per eligible spend (parts per million).
    /// E.g., 50_000 = 5% decay per eligible spend.
    pub decay_rate: TagWeight,
}

impl Default for AgeDecayConfig {
    fn default() -> Self {
        Self {
            min_age_blocks: 720, // ~2 hours
            decay_rate: 50_000,  // 5%
        }
    }
}

impl AgeDecayConfig {
    /// Create a new config with custom parameters.
    pub fn new(min_age_hours: f64, decay_percent: f64) -> Self {
        Self {
            min_age_blocks: (min_age_hours * 360.0) as u64, // 360 blocks/hour at 10s
            decay_rate: (decay_percent / 100.0 * TAG_WEIGHT_SCALE as f64) as TagWeight,
        }
    }

    /// Check if a UTXO is eligible for decay based on its age.
    pub fn is_eligible(&self, utxo_creation_block: u64, current_block: u64) -> bool {
        current_block.saturating_sub(utxo_creation_block) >= self.min_age_blocks
    }

    /// Calculate maximum decays possible in a time period.
    pub fn max_decays_in_blocks(&self, blocks: u64) -> u64 {
        blocks / self.min_age_blocks
    }

    /// Calculate tag remaining after maximum decay over a period.
    pub fn min_tag_remaining_after_blocks(&self, blocks: u64) -> f64 {
        let max_decays = self.max_decays_in_blocks(blocks);
        let decay_fraction = self.decay_rate as f64 / TAG_WEIGHT_SCALE as f64;
        (1.0 - decay_fraction).powi(max_decays as i32)
    }
}

/// Apply age-based decay to tags when spending a UTXO.
///
/// Returns `true` if decay was applied, `false` if UTXO was too young.
pub fn apply_age_decay(
    tags: &mut TagVector,
    utxo_creation_block: u64,
    current_block: u64,
    config: &AgeDecayConfig,
) -> bool {
    if !config.is_eligible(utxo_creation_block, current_block) {
        return false;
    }

    tags.apply_decay(config.decay_rate);
    true
}

/// Information about decay eligibility for ring signature verification.
#[derive(Clone, Debug)]
pub struct RingDecayInfo {
    /// For each ring member, whether it's eligible for decay.
    pub member_eligibility: Vec<bool>,
}

impl RingDecayInfo {
    /// Create decay info for a ring.
    pub fn new(
        ring_creation_blocks: &[u64],
        current_block: u64,
        config: &AgeDecayConfig,
    ) -> Self {
        let member_eligibility = ring_creation_blocks
            .iter()
            .map(|&creation_block| config.is_eligible(creation_block, current_block))
            .collect();

        Self { member_eligibility }
    }

    /// Check if all ring members are eligible (simplest case for ZK proof).
    pub fn all_eligible(&self) -> bool {
        self.member_eligibility.iter().all(|&e| e)
    }

    /// Check if no ring members are eligible (simplest case for ZK proof).
    pub fn none_eligible(&self) -> bool {
        self.member_eligibility.iter().all(|&e| !e)
    }

    /// Check if eligibility is mixed (requires more complex ZK proof).
    pub fn mixed_eligibility(&self) -> bool {
        !self.all_eligible() && !self.none_eligible()
    }

    /// Count eligible members.
    pub fn eligible_count(&self) -> usize {
        self.member_eligibility.iter().filter(|&&e| e).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ClusterId;

    #[test]
    fn test_age_eligibility() {
        let config = AgeDecayConfig::default(); // 720 blocks min age

        // Young UTXO - not eligible
        assert!(!config.is_eligible(100, 200)); // 100 blocks old

        // Old enough UTXO - eligible
        assert!(config.is_eligible(100, 900)); // 800 blocks old

        // Exactly at threshold
        assert!(config.is_eligible(100, 820)); // 720 blocks old
    }

    #[test]
    fn test_rapid_wash_trading_blocked() {
        let config = AgeDecayConfig::default();
        let cluster = ClusterId::new(1);

        // Start with an old UTXO
        let mut tags = TagVector::single(cluster);
        let old_utxo_block = 0;
        let mut current_block = 1000; // UTXO is 1000 blocks old

        // First spend - should decay (UTXO is old enough)
        let decayed = apply_age_decay(&mut tags, old_utxo_block, current_block, &config);
        assert!(decayed, "First spend should decay");

        // Rapid subsequent spends - output is new, shouldn't decay
        for i in 1..100 {
            current_block += 1;
            let new_utxo_block = current_block - 1; // Created 1 block ago
            let mut new_tags = tags.clone();
            let decayed = apply_age_decay(&mut new_tags, new_utxo_block, current_block, &config);
            assert!(!decayed, "Rapid spend {} should not decay", i);
        }

        // Only one decay applied
        let expected = TAG_WEIGHT_SCALE - 50_000; // 95%
        assert_eq!(tags.get(cluster), expected);
    }

    #[test]
    fn test_patient_wash_trading_bounded() {
        let config = AgeDecayConfig::default();
        let cluster = ClusterId::new(1);

        let mut tags = TagVector::single(cluster);
        let mut last_creation_block = 0u64;
        let mut current_block = 1000u64;
        let mut total_decays = 0;

        // Patient attacker: wait min_age between each spend
        for _ in 0..20 {
            if apply_age_decay(&mut tags, last_creation_block, current_block, &config) {
                total_decays += 1;
                last_creation_block = current_block; // New UTXO created
            }
            current_block += 720; // Wait exactly min_age
        }

        // Should get ~20 decays (one per min_age period)
        assert_eq!(total_decays, 20);

        // Tag should be 0.95^20 ≈ 35.8%
        let expected = (0.95_f64.powi(20) * TAG_WEIGHT_SCALE as f64) as TagWeight;
        let actual = tags.get(cluster);
        assert!(
            (actual as i64 - expected as i64).abs() < 5000,
            "Expected ~{}, got {}", expected, actual
        );
    }

    #[test]
    fn test_max_decays_calculation() {
        let config = AgeDecayConfig::default(); // 720 blocks min age

        // Per day (8640 blocks)
        assert_eq!(config.max_decays_in_blocks(8_640), 12);

        // Per week
        assert_eq!(config.max_decays_in_blocks(60_480), 84);

        // Min remaining after 1 week of max decay
        let min_remaining = config.min_tag_remaining_after_blocks(60_480);
        assert!(
            (min_remaining - 0.0135).abs() < 0.01,
            "Expected ~1.35%, got {:.2}%", min_remaining * 100.0
        );
    }

    #[test]
    fn test_ring_decay_info() {
        let config = AgeDecayConfig::default();
        let current_block = 10_000;

        // Ring with mixed ages
        let creation_blocks = vec![
            9_500,  // 500 blocks old - not eligible
            8_000,  // 2000 blocks old - eligible
            9_900,  // 100 blocks old - not eligible
            5_000,  // 5000 blocks old - eligible
        ];

        let info = RingDecayInfo::new(&creation_blocks, current_block, &config);

        assert!(!info.all_eligible());
        assert!(!info.none_eligible());
        assert!(info.mixed_eligibility());
        assert_eq!(info.eligible_count(), 2);
        assert_eq!(info.member_eligibility, vec![false, true, false, true]);
    }

    #[test]
    fn test_ring_all_eligible() {
        let config = AgeDecayConfig::default();
        let current_block = 10_000;

        // Ring where all members are old enough
        let creation_blocks = vec![1_000, 2_000, 3_000, 4_000];

        let info = RingDecayInfo::new(&creation_blocks, current_block, &config);

        assert!(info.all_eligible());
        assert!(!info.mixed_eligibility());
    }
}
