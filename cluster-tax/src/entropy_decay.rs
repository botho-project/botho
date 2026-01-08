//! Entropy-Weighted Decay: Patient Attack Resistance via Commerce Detection.
//!
//! This module implements decay that is proportional to the entropy increase
//! between input and output tags, effectively blocking patient wash trading
//! attacks while rewarding genuine commerce.
//!
//! # The Patient Attack Problem
//!
//! With pure age-based decay, a patient attacker can:
//! 1. Wait for UTXO to become eligible (720 blocks)
//! 2. Send to self → get 5% decay credit
//! 3. Repeat → achieve 97% decay in one week with 84 self-transfers
//!
//! This allows attackers to clear cluster tags without genuine commerce.
//!
//! # The Solution: Entropy-Weighted Decay
//!
//! Decay credit is proportional to the entropy increase in the transaction:
//!
//! ```text
//! decay_credit = base_decay × entropy_delta_factor(before, after)
//!
//! Where entropy_delta_factor:
//! - Returns 0.0 if entropy unchanged (wash trade)
//! - Returns 1.0 if entropy increased significantly (genuine commerce)
//! - Scales between based on magnitude of entropy increase
//! ```
//!
//! # Why This Works
//!
//! | Attack | Age-Based | Entropy-Weighted |
//! |--------|-----------|------------------|
//! | Rapid wash (100 in 100 blocks) | Blocked | Blocked |
//! | Patient wash (1/720 blocks) | 1.35% tag after week | ~100% tag |
//! | Sybil wash (fake counterparties) | 1.35% tag | ~100% tag |
//! | Genuine commerce (50% trades) | 1.35% tag | ~50% decay |
//!
//! Sybil wash is blocked because closed-loop transactions don't increase
//! entropy.
//!
//! # Ring Signature Support
//!
//! For ring signatures, we use a conservative approach: require that the
//! entropy delta would be positive for ALL ring members. This prevents
//! attackers from using high-entropy decoys to mask low-entropy real inputs.

use crate::{
    age_decay::AgeDecayConfig,
    tag::{TagVector, TagWeight},
};

/// Configuration for entropy-weighted decay.
#[derive(Clone, Debug)]
pub struct EntropyDecayConfig {
    /// Base decay rate per eligible transaction (parts per million).
    /// E.g., 50_000 = 5% decay per eligible spend.
    pub base_decay_rate: TagWeight,

    /// Minimum age in blocks for decay eligibility.
    /// Same as age-based decay to prevent rapid wash trading.
    pub min_age_blocks: u64,

    /// Minimum entropy delta (in bits) to qualify for any decay.
    /// Below this threshold, no decay credit is given.
    /// Default: 0.1 bits (filters out noise from rounding)
    pub min_entropy_delta: f64,

    /// Entropy delta (in bits) at which full decay is granted.
    /// Above this threshold, full base_decay_rate is applied.
    /// Default: 0.5 bits (significant commerce detected)
    pub full_decay_entropy_delta: f64,

    /// Scaling function for partial decay between min and full thresholds.
    pub scaling: EntropyScaling,
}

/// Scaling function for partial decay credit.
#[derive(Clone, Copy, Debug, Default)]
pub enum EntropyScaling {
    /// Linear scaling: decay = base × (delta / full_decay_delta)
    Linear,

    /// Square root scaling: decay = base × sqrt(delta / full_decay_delta)
    /// More generous for small increases, discourages gaming at high end.
    #[default]
    Sqrt,

    /// Exponential scaling: decay = base × (1 - e^(-k × delta))
    /// Approaches maximum quickly, then plateaus.
    Exponential {
        /// Rate constant (higher = faster approach to max)
        k: f64,
    },
}

impl Default for EntropyDecayConfig {
    fn default() -> Self {
        Self {
            base_decay_rate: 50_000,       // 5%
            min_age_blocks: 720,           // ~2 hours
            min_entropy_delta: 0.1,        // bits
            full_decay_entropy_delta: 0.5, // bits
            scaling: EntropyScaling::Sqrt,
        }
    }
}

impl EntropyDecayConfig {
    /// Create config from age decay config with entropy enhancement.
    pub fn from_age_config(age_config: &AgeDecayConfig) -> Self {
        Self {
            base_decay_rate: age_config.decay_rate,
            min_age_blocks: age_config.min_age_blocks,
            ..Default::default()
        }
    }

    /// Calculate decay factor based on entropy change.
    ///
    /// Returns a factor in [0.0, 1.0]:
    /// - 0.0: No decay credit (wash trade or tiny increase)
    /// - 1.0: Full decay credit (significant commerce)
    /// - Between: Scaled based on entropy increase
    pub fn decay_factor(&self, entropy_before: f64, entropy_after: f64) -> f64 {
        let delta = entropy_after - entropy_before;

        // No decay credit for entropy decrease or tiny increase
        if delta < self.min_entropy_delta {
            return 0.0;
        }

        // Full decay credit for large entropy increase
        if delta >= self.full_decay_entropy_delta {
            return 1.0;
        }

        // Scaled decay for partial increase
        let normalized = (delta - self.min_entropy_delta)
            / (self.full_decay_entropy_delta - self.min_entropy_delta);

        match self.scaling {
            EntropyScaling::Linear => normalized,
            EntropyScaling::Sqrt => normalized.sqrt(),
            EntropyScaling::Exponential { k } => 1.0 - (-k * normalized).exp(),
        }
    }

    /// Calculate the actual decay amount in parts per million.
    pub fn calculate_decay(&self, entropy_before: f64, entropy_after: f64) -> TagWeight {
        let factor = self.decay_factor(entropy_before, entropy_after);
        (self.base_decay_rate as f64 * factor) as TagWeight
    }

    /// Check if a UTXO is eligible for decay based on its age.
    pub fn is_age_eligible(&self, utxo_creation_block: u64, current_block: u64) -> bool {
        current_block.saturating_sub(utxo_creation_block) >= self.min_age_blocks
    }
}

/// Unified decay mode supporting different strategies.
#[derive(Clone, Debug)]
pub enum DecayMode {
    /// Pure age-based decay: any eligible spend triggers full decay.
    /// Vulnerable to patient wash trading.
    AgeBased(AgeDecayConfig),

    /// Pure entropy-weighted decay: decay proportional to entropy increase.
    /// Resistant to patient wash trading.
    EntropyWeighted(EntropyDecayConfig),

    /// Hybrid mode: age-based baseline with entropy bonus.
    /// Provides backward compatibility with enhanced resistance.
    Hybrid {
        /// Base age decay (always applies if eligible)
        age: AgeDecayConfig,
        /// Additional decay bonus from entropy increase
        entropy: EntropyDecayConfig,
    },
}

impl Default for DecayMode {
    fn default() -> Self {
        // Default to entropy-weighted for best attack resistance
        DecayMode::EntropyWeighted(EntropyDecayConfig::default())
    }
}

impl DecayMode {
    /// Create age-based mode from config.
    pub fn age_based(config: AgeDecayConfig) -> Self {
        DecayMode::AgeBased(config)
    }

    /// Create entropy-weighted mode with default config.
    pub fn entropy_weighted() -> Self {
        DecayMode::EntropyWeighted(EntropyDecayConfig::default())
    }

    /// Create hybrid mode from configs.
    pub fn hybrid(age: AgeDecayConfig, entropy: EntropyDecayConfig) -> Self {
        DecayMode::Hybrid { age, entropy }
    }

    /// Calculate decay for a transaction.
    ///
    /// # Arguments
    /// * `input_tags` - Tag vector of the input UTXO
    /// * `output_tags` - Tag vector of the output (after commerce)
    /// * `utxo_creation_block` - Block when the input UTXO was created
    /// * `current_block` - Current block height
    ///
    /// # Returns
    /// Decay amount in parts per million (0 to base_decay_rate)
    pub fn calculate_decay(
        &self,
        input_tags: &TagVector,
        output_tags: &TagVector,
        utxo_creation_block: u64,
        current_block: u64,
    ) -> TagWeight {
        match self {
            DecayMode::AgeBased(config) => {
                if config.is_eligible(utxo_creation_block, current_block) {
                    config.decay_rate
                } else {
                    0
                }
            }
            DecayMode::EntropyWeighted(config) => {
                // Check age eligibility first
                if !config.is_age_eligible(utxo_creation_block, current_block) {
                    return 0;
                }

                // Use collision entropy for attack resistance
                let entropy_before = input_tags.collision_entropy();
                let entropy_after = output_tags.collision_entropy();

                config.calculate_decay(entropy_before, entropy_after)
            }
            DecayMode::Hybrid { age, entropy } => {
                // Base decay from age eligibility
                let age_decay = if age.is_eligible(utxo_creation_block, current_block) {
                    age.decay_rate
                } else {
                    0
                };

                // Bonus decay from entropy increase
                let entropy_bonus = {
                    if !entropy.is_age_eligible(utxo_creation_block, current_block) {
                        0
                    } else {
                        let before = input_tags.collision_entropy();
                        let after = output_tags.collision_entropy();
                        entropy.calculate_decay(before, after)
                    }
                };

                // Total capped at base rate (no double-dipping)
                age_decay.saturating_add(entropy_bonus).min(age.decay_rate)
            }
        }
    }
}

// ============================================================================
// Ring Signature Support (Phase 1: Conservative Approach)
// ============================================================================

/// Calculate conservative entropy delta for ring signatures.
///
/// For ring signatures, we don't know which input is real. The conservative
/// approach uses the MAXIMUM input entropy (worst case for sender):
///
/// - If the real input has high entropy, the delta is small
/// - If the real input has low entropy, the delta is large
/// - By using max, we assume the sender is trying to minimize decay
///
/// This prevents attackers from using low-entropy decoys to inflate their
/// apparent entropy increase.
pub fn conservative_entropy_delta(ring_member_tags: &[TagVector], output_tags: &TagVector) -> f64 {
    if ring_member_tags.is_empty() {
        return 0.0;
    }

    // Use MAX input entropy (most conservative for sender)
    let max_input_entropy = ring_member_tags
        .iter()
        .map(|tv| tv.collision_entropy())
        .fold(0.0, f64::max);

    let output_entropy = output_tags.collision_entropy();

    // Delta is output - max_input (conservative)
    (output_entropy - max_input_entropy).max(0.0)
}

/// Calculate decay for a ring signature transaction.
///
/// Uses the conservative entropy delta approach: assumes the real input
/// has the highest entropy among all ring members.
pub fn ring_entropy_decay(
    ring_member_tags: &[TagVector],
    output_tags: &TagVector,
    ring_creation_blocks: &[u64],
    current_block: u64,
    config: &EntropyDecayConfig,
) -> TagWeight {
    // All ring members must be age-eligible (conservative)
    let all_eligible = ring_creation_blocks
        .iter()
        .all(|&creation| config.is_age_eligible(creation, current_block));

    if !all_eligible {
        return 0;
    }

    // Calculate conservative entropy delta
    let max_input_entropy = ring_member_tags
        .iter()
        .map(|tv| tv.collision_entropy())
        .fold(0.0, f64::max);

    let output_entropy = output_tags.collision_entropy();

    config.calculate_decay(max_input_entropy, output_entropy)
}

/// Information about entropy-based decay for ring signatures.
#[derive(Clone, Debug)]
pub struct RingEntropyDecayInfo {
    /// Entropy of each ring member
    pub member_entropies: Vec<f64>,

    /// Maximum input entropy (conservative assumption)
    pub max_input_entropy: f64,

    /// Output entropy
    pub output_entropy: f64,

    /// Conservative entropy delta
    pub conservative_delta: f64,

    /// Whether all members are age-eligible
    pub all_age_eligible: bool,

    /// Calculated decay factor
    pub decay_factor: f64,
}

impl RingEntropyDecayInfo {
    /// Analyze a ring transaction for entropy-based decay.
    pub fn analyze(
        ring_member_tags: &[TagVector],
        output_tags: &TagVector,
        ring_creation_blocks: &[u64],
        current_block: u64,
        config: &EntropyDecayConfig,
    ) -> Self {
        let member_entropies: Vec<f64> = ring_member_tags
            .iter()
            .map(|tv| tv.collision_entropy())
            .collect();

        let max_input_entropy = member_entropies.iter().copied().fold(0.0, f64::max);

        let output_entropy = output_tags.collision_entropy();

        let conservative_delta = (output_entropy - max_input_entropy).max(0.0);

        let all_age_eligible = ring_creation_blocks
            .iter()
            .all(|&creation| config.is_age_eligible(creation, current_block));

        let decay_factor = if all_age_eligible {
            config.decay_factor(max_input_entropy, output_entropy)
        } else {
            0.0
        };

        Self {
            member_entropies,
            max_input_entropy,
            output_entropy,
            conservative_delta,
            all_age_eligible,
            decay_factor,
        }
    }

    /// Check if the transaction qualifies for any decay credit.
    pub fn qualifies_for_decay(&self) -> bool {
        self.all_age_eligible && self.decay_factor > 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ClusterId;

    // ========================================================================
    // EntropyDecayConfig Tests
    // ========================================================================

    #[test]
    fn test_decay_factor_wash_trade() {
        let config = EntropyDecayConfig::default();

        // Wash trade: same entropy in and out
        let factor = config.decay_factor(0.0, 0.0);
        assert_eq!(factor, 0.0, "Wash trade should get 0 decay");

        let factor = config.decay_factor(1.0, 1.0);
        assert_eq!(factor, 0.0, "Same entropy should get 0 decay");
    }

    #[test]
    fn test_decay_factor_commerce() {
        let config = EntropyDecayConfig::default();

        // Significant commerce: 0 -> 1 bit
        let factor = config.decay_factor(0.0, 1.0);
        assert!(
            (factor - 1.0).abs() < 0.01,
            "Large entropy increase should get full decay: {factor}"
        );
    }

    #[test]
    fn test_decay_factor_partial() {
        let config = EntropyDecayConfig::default();

        // Partial increase: 0 -> 0.3 bits
        let factor = config.decay_factor(0.0, 0.3);
        assert!(
            factor > 0.0 && factor < 1.0,
            "Partial increase should get partial decay: {factor}"
        );
    }

    #[test]
    fn test_decay_factor_below_min() {
        let config = EntropyDecayConfig::default();

        // Tiny increase below min_entropy_delta
        let factor = config.decay_factor(0.0, 0.05);
        assert_eq!(factor, 0.0, "Below min threshold should get 0 decay");
    }

    #[test]
    fn test_decay_factor_negative() {
        let config = EntropyDecayConfig::default();

        // Entropy decrease (shouldn't happen in practice, but should handle)
        let factor = config.decay_factor(1.0, 0.5);
        assert_eq!(factor, 0.0, "Entropy decrease should get 0 decay");
    }

    #[test]
    fn test_scaling_linear() {
        let config = EntropyDecayConfig {
            scaling: EntropyScaling::Linear,
            ..Default::default()
        };

        // Halfway point should give 0.5 factor
        let midpoint = (config.min_entropy_delta + config.full_decay_entropy_delta) / 2.0;
        let factor = config.decay_factor(0.0, midpoint);
        assert!(
            (factor - 0.5).abs() < 0.01,
            "Linear halfway should be 0.5: {factor}"
        );
    }

    #[test]
    fn test_scaling_sqrt() {
        let config = EntropyDecayConfig {
            scaling: EntropyScaling::Sqrt,
            ..Default::default()
        };

        // Sqrt is more generous: 0.25 normalized gives sqrt(0.25) = 0.5
        let quarter = config.min_entropy_delta
            + 0.25 * (config.full_decay_entropy_delta - config.min_entropy_delta);
        let factor = config.decay_factor(0.0, quarter);
        assert!(
            (factor - 0.5).abs() < 0.01,
            "Sqrt at quarter should be 0.5: {factor}"
        );
    }

    // ========================================================================
    // DecayMode Tests
    // ========================================================================

    #[test]
    fn test_decay_mode_age_based() {
        let config = AgeDecayConfig::default();
        let mode = DecayMode::age_based(config);

        let c1 = ClusterId::new(1);
        let input = TagVector::single(c1);
        let output = TagVector::single(c1); // Wash trade

        // Eligible age
        let decay = mode.calculate_decay(&input, &output, 0, 1000);
        assert_eq!(
            decay, 50_000,
            "Age-based should give full decay for eligible"
        );

        // Not eligible (too young)
        let decay = mode.calculate_decay(&input, &output, 900, 1000);
        assert_eq!(decay, 0, "Age-based should give no decay for young UTXO");
    }

    #[test]
    fn test_decay_mode_entropy_wash_trade() {
        let mode = DecayMode::entropy_weighted();

        let c1 = ClusterId::new(1);
        let input = TagVector::single(c1);
        let output = TagVector::single(c1); // Wash trade - same tags

        // Even with eligible age, wash trade gets no decay
        let decay = mode.calculate_decay(&input, &output, 0, 1000);
        assert_eq!(decay, 0, "Wash trade should get no decay");
    }

    #[test]
    fn test_decay_mode_entropy_commerce() {
        let mode = DecayMode::entropy_weighted();

        let c1 = ClusterId::new(1);
        let c2 = ClusterId::new(2);

        let input = TagVector::single(c1);
        let mut output = TagVector::new();
        output.set(c1, 500_000);
        output.set(c2, 500_000); // Mixed - commerce!

        // Commerce gets decay credit
        let decay = mode.calculate_decay(&input, &output, 0, 1000);
        assert!(decay > 0, "Commerce should get decay credit: {decay}");
    }

    #[test]
    fn test_decay_mode_entropy_age_check() {
        let mode = DecayMode::entropy_weighted();

        let c1 = ClusterId::new(1);
        let c2 = ClusterId::new(2);

        let input = TagVector::single(c1);
        let mut output = TagVector::new();
        output.set(c1, 500_000);
        output.set(c2, 500_000);

        // Young UTXO - no decay even with commerce
        let decay = mode.calculate_decay(&input, &output, 900, 1000);
        assert_eq!(decay, 0, "Young UTXO should get no decay");
    }

    // ========================================================================
    // Ring Signature Tests
    // ========================================================================

    #[test]
    fn test_conservative_entropy_delta() {
        let c1 = ClusterId::new(1);
        let c2 = ClusterId::new(2);

        // Ring with one low-entropy and one high-entropy member
        let low_entropy = TagVector::single(c1);
        let mut high_entropy = TagVector::new();
        high_entropy.set(c1, 500_000);
        high_entropy.set(c2, 500_000);

        // Output has same entropy as high member
        let output = high_entropy.clone();

        let ring_tags = vec![low_entropy, high_entropy];
        let delta = conservative_entropy_delta(&ring_tags, &output);

        // Conservative: use max input (1 bit) vs output (1 bit) = 0 delta
        assert!(delta < 0.01, "Conservative delta should be ~0: {delta}");
    }

    #[test]
    fn test_conservative_entropy_delta_genuine_commerce() {
        let c1 = ClusterId::new(1);
        let c2 = ClusterId::new(2);
        let c3 = ClusterId::new(3);

        // All ring members are low entropy
        let low1 = TagVector::single(c1);
        let low2 = TagVector::single(c2);

        // Output has high entropy (commerce happened)
        let mut output = TagVector::new();
        output.set(c1, 300_000);
        output.set(c2, 300_000);
        output.set(c3, 400_000);

        let ring_tags = vec![low1, low2];
        let delta = conservative_entropy_delta(&ring_tags, &output);

        // Output entropy > max input entropy, so positive delta
        assert!(
            delta > 1.0,
            "Genuine commerce should show positive delta: {delta}"
        );
    }

    #[test]
    fn test_ring_entropy_decay_all_eligible() {
        let config = EntropyDecayConfig::default();
        let c1 = ClusterId::new(1);
        let c2 = ClusterId::new(2);

        // All old ring members
        let ring_tags = vec![TagVector::single(c1), TagVector::single(c1)];
        let ring_blocks = vec![0, 100];
        let current_block = 1000;

        // Output with commerce
        let mut output = TagVector::new();
        output.set(c1, 500_000);
        output.set(c2, 500_000);

        let decay = ring_entropy_decay(&ring_tags, &output, &ring_blocks, current_block, &config);

        assert!(
            decay > 0,
            "Commerce with eligible ring should get decay: {decay}"
        );
    }

    #[test]
    fn test_ring_entropy_decay_one_young() {
        let config = EntropyDecayConfig::default();
        let c1 = ClusterId::new(1);
        let c2 = ClusterId::new(2);

        // One young ring member
        let ring_tags = vec![TagVector::single(c1), TagVector::single(c1)];
        let ring_blocks = vec![0, 900]; // Second is too young
        let current_block = 1000;

        // Output with commerce
        let mut output = TagVector::new();
        output.set(c1, 500_000);
        output.set(c2, 500_000);

        let decay = ring_entropy_decay(&ring_tags, &output, &ring_blocks, current_block, &config);

        assert_eq!(decay, 0, "One young member should block decay");
    }

    #[test]
    fn test_ring_entropy_decay_info() {
        let config = EntropyDecayConfig::default();
        let c1 = ClusterId::new(1);
        let c2 = ClusterId::new(2);

        let ring_tags = vec![TagVector::single(c1), TagVector::single(c2)];
        let ring_blocks = vec![0, 100];
        let current_block = 1000;

        let mut output = TagVector::new();
        output.set(c1, 500_000);
        output.set(c2, 500_000);

        let info = RingEntropyDecayInfo::analyze(
            &ring_tags,
            &output,
            &ring_blocks,
            current_block,
            &config,
        );

        assert!(info.all_age_eligible);
        assert_eq!(info.member_entropies.len(), 2);
        assert!(info.member_entropies[0] < 0.01); // Single cluster = 0 entropy
        assert!(info.member_entropies[1] < 0.01);
        assert!(info.output_entropy > 0.9); // Two equal clusters = 1 bit
        assert!(info.conservative_delta > 0.9);
        assert!(info.qualifies_for_decay());
    }

    // ========================================================================
    // Attack Scenario Tests
    // ========================================================================

    #[test]
    fn test_patient_wash_attack_blocked() {
        // Scenario: Attacker tries to decay tags through patient self-transfers
        let mode = DecayMode::entropy_weighted();
        let c1 = ClusterId::new(1);

        let tags = TagVector::single(c1);
        let mut last_block = 0u64;
        let mut current_block = 1000u64;

        // Attempt 84 wash trades over a week (one every 720 blocks)
        let mut total_decay = 0;
        for _ in 0..84 {
            let output = tags.clone(); // Self-transfer - same tags
            let decay = mode.calculate_decay(&tags, &output, last_block, current_block);
            total_decay += decay;

            last_block = current_block;
            current_block += 720;
        }

        // With entropy-weighted, wash trades get ZERO decay
        assert_eq!(total_decay, 0, "Patient wash attack should get no decay");
    }

    #[test]
    fn test_genuine_commerce_rewarded() {
        // Scenario: Merchant receives diverse payments, should get decay
        let mode = DecayMode::entropy_weighted();
        let c1 = ClusterId::new(1);
        let c2 = ClusterId::new(2);
        let c3 = ClusterId::new(3);

        // Start with single-source coins
        let input = TagVector::single(c1);

        // After commerce, coins are mixed
        let mut output = TagVector::new();
        output.set(c1, 400_000);
        output.set(c2, 300_000);
        output.set(c3, 300_000);

        let decay = mode.calculate_decay(&input, &output, 0, 1000);

        // Commerce should get significant decay credit
        assert!(
            decay > 40_000,
            "Genuine commerce should get substantial decay: {decay}"
        );
    }

    #[test]
    fn test_sybil_attack_blocked() {
        // Scenario: Attacker creates fake counterparties (Sybils)
        // But transactions between Sybils don't increase entropy
        let mode = DecayMode::entropy_weighted();
        let c1 = ClusterId::new(1);

        // Attacker controls all clusters in a closed loop
        // Transfer: c1 -> c1 (same cluster, just different addresses)
        let input = TagVector::single(c1);
        let output = TagVector::single(c1);

        let decay = mode.calculate_decay(&input, &output, 0, 1000);

        assert_eq!(decay, 0, "Sybil attack should get no decay (closed loop)");
    }

    #[test]
    fn test_partial_commerce() {
        // Scenario: Small amount of legitimate commerce
        // 90/10 split gives ~0.29 bits entropy, which is between
        // min_entropy_delta (0.1) and full_decay_entropy_delta (0.5)
        let mode = DecayMode::entropy_weighted();
        let c1 = ClusterId::new(1);
        let c2 = ClusterId::new(2);

        let input = TagVector::single(c1);

        // After small commerce (90% original, 10% new)
        let mut output = TagVector::new();
        output.set(c1, 900_000); // 90% original
        output.set(c2, 100_000); // 10% commerce

        let decay = mode.calculate_decay(&input, &output, 0, 1000);

        // Small commerce gets partial decay
        assert!(
            decay > 0 && decay < 50_000,
            "Small commerce (10%) should get partial decay: {decay}"
        );
    }

    #[test]
    fn test_substantial_commerce_full_decay() {
        // Scenario: Substantial commerce (70/30 split)
        // This gives ~0.79 bits entropy, which exceeds full_decay_entropy_delta (0.5)
        let mode = DecayMode::entropy_weighted();
        let c1 = ClusterId::new(1);
        let c2 = ClusterId::new(2);

        let input = TagVector::single(c1);

        // After substantial commerce
        let mut output = TagVector::new();
        output.set(c1, 700_000); // 70% original
        output.set(c2, 300_000); // 30% commerce

        let decay = mode.calculate_decay(&input, &output, 0, 1000);

        // Substantial commerce gets full decay
        assert_eq!(
            decay, 50_000,
            "Substantial commerce (30%) should get full decay: {decay}"
        );
    }
}
