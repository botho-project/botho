//! Production lottery draw implementation.
//!
//! This module provides the core lottery selection logic for fee
//! redistribution. Unlike the simulation module, this is designed for actual
//! use in block production and validation with verifiable randomness.
//!
//! ## Selection Mode
//!
//! The default selection mode is `ClusterWeighted`: winner weight is UTXO
//! value scaled by the inverse cluster factor. This is the only mode whose
//! progressive term is split-invariant — weights are value-based (splitting
//! a position never increases total weight) and the tilt depends on cluster
//! provenance, which inherits through splits.
//!
//! Per-UTXO weight terms (Uniform, the alpha component of Hybrid) are
//! subsidies to whoever splits hardest: adversarial simulation shows a
//! strategic whale splitting into 1,000 UTXOs captures the payout stream
//! (~300x weight gain under Hybrid alpha=0.3) and inequality rises.
//!
//! See `docs/design/cluster-tilted-redistribution.md` for the validated
//! design and `experiments/ANALYSIS.md` for the gamed-equilibrium results.

use sha2::{Digest, Sha256};

use crate::TagVector;

/// Configuration for lottery drawings.
#[derive(Clone, Debug)]
pub struct LotteryDrawConfig {
    /// Fraction of fees that go to lottery pool (remainder burned).
    /// Default: 0.8 (80% to lottery, 20% burned)
    pub pool_fraction: f64,

    /// Number of winners per drawing.
    /// Default: 4
    pub winners_per_draw: usize,

    /// Minimum UTXO age in blocks to participate.
    /// Default: 720 (~2 hours at 10s blocks)
    pub min_utxo_age: u64,

    /// Minimum UTXO value to participate (in base units).
    /// Default: 1_000_000 (1 microBTH)
    pub min_utxo_value: u64,

    /// Selection mode for choosing winners.
    pub selection_mode: SelectionMode,
}

impl Default for LotteryDrawConfig {
    fn default() -> Self {
        Self {
            pool_fraction: 0.8,
            winners_per_draw: 4,
            min_utxo_age: 720,
            min_utxo_value: 1_000_000,
            // Cluster-tilted: the only split-invariant progressive mode.
            // See docs/design/cluster-tilted-redistribution.md
            selection_mode: SelectionMode::ClusterWeighted,
        }
    }
}

/// Selection mode for lottery winners.
///
/// Different modes provide different trade-offs between progressivity
/// (favoring small holders) and Sybil resistance (preventing gaming).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum SelectionMode {
    /// Uniform: each UTXO has equal chance.
    /// - Progressive: Yes (many small UTXOs beat few large ones)
    /// - Sybil resistance: Poor (splitting = more tickets)
    /// - Gaming ratio: ~9.3x
    Uniform,

    /// Value-weighted: probability proportional to UTXO value.
    /// - Progressive: No (same as just holding)
    /// - Sybil resistance: Excellent (splitting doesn't help)
    /// - Gaming ratio: ~1.04x
    ValueWeighted,

    /// Hybrid: weight = α + (1-α) × normalized_value
    /// - α = 0.0: pure value-weighted
    /// - α = 1.0: pure uniform
    /// - α = 0.3: recommended balance (3.84x gaming, 69% Gini reduction)
    Hybrid { alpha: f64 },

    /// Cluster-factor weighted: lower factor = more weight.
    /// Progressive via fee system (commerce coins worth more).
    #[default]
    ClusterWeighted,

    /// Entropy-weighted: higher tag entropy = more weight.
    /// Sybil-resistant (splits preserve entropy).
    EntropyWeighted { entropy_bonus: f64 },
}

/// Fixed-point scale for cluster factors (1000 = 1.0x, 6000 = 6.0x).
pub const FACTOR_SCALE: u64 = 1000;

/// Maximum cluster factor in FACTOR_SCALE units (6.0x).
pub const MAX_FACTOR_SCALED: u64 = 6 * FACTOR_SCALE;

/// A UTXO eligible for lottery participation.
#[derive(Clone, Debug)]
pub struct LotteryCandidate {
    /// Unique identifier (tx_hash, output_index)
    pub id: [u8; 36],
    /// UTXO value in base units
    pub value: u64,
    /// Cluster factor in FACTOR_SCALE units (1000 = 1.0x to 6000 = 6.0x)
    pub cluster_factor: u64,
    /// Tag entropy in bits (from TagVector::cluster_entropy()).
    /// NOT consensus-safe (computed via floating-point log2); only used by
    /// the research-only EntropyWeighted mode.
    pub tag_entropy: f64,
    /// Block height when UTXO was created
    pub creation_block: u64,
}

impl LotteryCandidate {
    /// Create a new lottery candidate.
    ///
    /// `cluster_factor` is in FACTOR_SCALE units (1000..=6000), as produced
    /// by `ClusterFactorCurve::factor()`. Out-of-range values are clamped.
    pub fn new(
        id: [u8; 36],
        value: u64,
        cluster_factor: u64,
        tags: &TagVector,
        creation_block: u64,
    ) -> Self {
        Self {
            id,
            value,
            cluster_factor: cluster_factor.clamp(FACTOR_SCALE, MAX_FACTOR_SCALED),
            tag_entropy: tags.cluster_entropy(),
            creation_block,
        }
    }

    /// Check if this UTXO is eligible for the lottery.
    pub fn is_eligible(&self, current_block: u64, config: &LotteryDrawConfig) -> bool {
        let age = current_block.saturating_sub(self.creation_block);
        age >= config.min_utxo_age && self.value >= config.min_utxo_value
    }

    /// Calculate the lottery weight based on selection mode.
    ///
    /// CONSENSUS-CRITICAL: weights are pure integer arithmetic so that every
    /// validator computes bit-identical draws on any platform. Weights are
    /// only meaningful relative to other candidates under the same mode, so
    /// common scale constants are not divided out.
    ///
    /// `EntropyWeighted` is research-only: its entropy input comes from
    /// floating-point log2 and must not be used in consensus.
    pub fn weight(&self, mode: SelectionMode, max_value: u64) -> u128 {
        match mode {
            SelectionMode::Uniform => 1,

            SelectionMode::ValueWeighted => self.value as u128,

            SelectionMode::Hybrid { alpha } => {
                // weight = alpha + (1 - alpha) × value/max_value, scaled by
                // max_value × 10_000: alpha quantized to basis points (the
                // config constant must be identical across nodes, so the
                // f64→bps conversion is deterministic).
                let alpha_bps = (alpha.clamp(0.0, 1.0) * 10_000.0).round() as u128;
                alpha_bps * max_value.max(1) as u128 + (10_000 - alpha_bps) * self.value as u128
            }

            SelectionMode::ClusterWeighted => {
                // weight = value × (max_factor − factor + 1), in FACTOR_SCALE
                // units: value × (6000 − factor + 1000). A factor-1.0 coin
                // has 6x the per-value weight of a factor-6.0 coin.
                let factor = self.cluster_factor.clamp(FACTOR_SCALE, MAX_FACTOR_SCALED);
                self.value as u128 * (MAX_FACTOR_SCALED - factor + FACTOR_SCALE) as u128
            }

            SelectionMode::EntropyWeighted { entropy_bonus } => {
                // RESEARCH ONLY (not consensus-safe): entropy comes from
                // floating-point log2. weight = value × (1 + bonus × entropy)
                // in micro-units.
                let entropy_millibits = (self.tag_entropy.max(0.0) * 1000.0).round() as u128;
                let bonus_bps = (entropy_bonus.max(0.0) * 10_000.0).round() as u128;
                self.value as u128 * (1_000_000 + bonus_bps * entropy_millibits / 10)
            }
        }
    }
}

/// Result of a lottery drawing.
#[derive(Clone, Debug)]
pub struct LotteryResult {
    /// Block height of the drawing
    pub block_height: u64,
    /// Total pool amount being distributed
    pub pool_amount: u64,
    /// Winning UTXOs and their payouts
    pub winners: Vec<LotteryWinner>,
    /// Seed used for verifiable randomness
    pub seed: [u8; 32],
}

/// A lottery winner.
#[derive(Clone, Debug)]
pub struct LotteryWinner {
    /// UTXO identifier
    pub utxo_id: [u8; 36],
    /// Amount won
    pub payout: u64,
}

/// Perform a verifiable lottery drawing.
///
/// # Arguments
/// * `candidates` - Eligible UTXOs to select from
/// * `pool_amount` - Total amount to distribute
/// * `block_height` - Current block height
/// * `block_hash` - Hash of the previous block (for randomness)
/// * `config` - Lottery configuration
///
/// # Returns
/// `LotteryResult` with winners and payouts, or `None` if no eligible
/// candidates.
pub fn draw_winners(
    candidates: &[LotteryCandidate],
    pool_amount: u64,
    block_height: u64,
    block_hash: &[u8; 32],
    config: &LotteryDrawConfig,
) -> Option<LotteryResult> {
    // Filter to eligible candidates
    let eligible: Vec<&LotteryCandidate> = candidates
        .iter()
        .filter(|c| c.is_eligible(block_height, config))
        .collect();

    if eligible.is_empty() || pool_amount == 0 {
        return None;
    }

    // Generate deterministic seed from block hash and height
    let seed = generate_seed(block_hash, block_height);

    // Calculate max value for normalization (used by Hybrid mode)
    let max_value = eligible.iter().map(|c| c.value).max().unwrap_or(1);

    // Calculate weights for all candidates.
    // CONSENSUS-CRITICAL: pure integer arithmetic — every validator must
    // compute bit-identical draws on any platform.
    let weights: Vec<(&LotteryCandidate, u128)> = eligible
        .iter()
        .map(|c| (*c, c.weight(config.selection_mode, max_value)))
        .collect();

    let total_weight: u128 = weights.iter().map(|(_, w)| *w).sum();

    if total_weight == 0 {
        return None;
    }

    // Select winners using verifiable random selection
    let num_winners = config.winners_per_draw.min(eligible.len());
    let payout_per_winner = pool_amount / num_winners as u64;

    let mut winners = Vec::with_capacity(num_winners);
    let mut used_indices = std::collections::HashSet::new();

    for i in 0..num_winners {
        // Deterministic 128-bit roll in [0, total_weight). The modulo bias
        // is at most total_weight / 2^128 — negligible and, crucially,
        // identical on every platform.
        let roll = verifiable_random_u128(&seed, i as u64) % total_weight;

        // Select winner based on cumulative weights
        let mut cumulative: u128 = 0;
        for (idx, (candidate, weight)) in weights.iter().enumerate() {
            if used_indices.contains(&idx) {
                continue; // Skip already-selected winners
            }
            cumulative += weight;
            if cumulative > roll {
                winners.push(LotteryWinner {
                    utxo_id: candidate.id,
                    payout: payout_per_winner,
                });
                used_indices.insert(idx);
                break;
            }
        }
    }

    // Handle remainder (dust goes to last winner)
    let total_paid: u64 = winners.iter().map(|w| w.payout).sum();
    if let Some(last) = winners.last_mut() {
        last.payout += pool_amount - total_paid;
    }

    Some(LotteryResult {
        block_height,
        pool_amount,
        winners,
        seed,
    })
}

/// Verify a lottery drawing result.
///
/// Returns `true` if the drawing is valid and matches the expected result.
pub fn verify_drawing(
    candidates: &[LotteryCandidate],
    result: &LotteryResult,
    block_hash: &[u8; 32],
    config: &LotteryDrawConfig,
) -> bool {
    // Re-run the drawing and compare
    match draw_winners(
        candidates,
        result.pool_amount,
        result.block_height,
        block_hash,
        config,
    ) {
        Some(expected) => {
            // Verify seed matches
            if expected.seed != result.seed {
                return false;
            }

            // Verify winners match
            if expected.winners.len() != result.winners.len() {
                return false;
            }

            for (e, r) in expected.winners.iter().zip(result.winners.iter()) {
                if e.utxo_id != r.utxo_id || e.payout != r.payout {
                    return false;
                }
            }

            true
        }
        None => result.winners.is_empty(),
    }
}

/// Generate deterministic seed from block hash and height.
fn generate_seed(block_hash: &[u8; 32], block_height: u64) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"LOTTERY_SEED_V1");
    hasher.update(block_hash);
    hasher.update(block_height.to_le_bytes());
    hasher.finalize().into()
}

/// Generate a verifiable random u128 from the seed and selection index.
fn verifiable_random_u128(seed: &[u8; 32], index: u64) -> u128 {
    let mut hasher = Sha256::new();
    hasher.update(seed);
    hasher.update(index.to_le_bytes());
    let hash: [u8; 32] = hasher.finalize().into();

    u128::from_le_bytes(hash[0..16].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `factor` is in FACTOR_SCALE units (1000 = 1.0x .. 6000 = 6.0x).
    fn make_candidate(
        id: u8,
        value: u64,
        factor: u64,
        entropy: f64,
        block: u64,
    ) -> LotteryCandidate {
        let mut utxo_id = [0u8; 36];
        utxo_id[0] = id;
        LotteryCandidate {
            id: utxo_id,
            value,
            cluster_factor: factor,
            tag_entropy: entropy,
            creation_block: block,
        }
    }

    #[test]
    fn test_default_is_cluster_weighted() {
        // The default MUST stay split-invariant: per-UTXO weight terms
        // (Uniform, Hybrid's alpha component) are subsidies to splitters.
        // See docs/design/cluster-tilted-redistribution.md
        let config = LotteryDrawConfig::default();
        assert!(matches!(
            config.selection_mode,
            SelectionMode::ClusterWeighted
        ));
    }

    #[test]
    fn test_eligibility_check() {
        let config = LotteryDrawConfig::default();
        let current_block = 1000;

        // Too young
        let young = make_candidate(1, 1_000_000, 1000, 0.0, 500);
        assert!(!young.is_eligible(current_block, &config));

        // Old enough
        let old = make_candidate(2, 1_000_000, 1000, 0.0, 100);
        assert!(old.is_eligible(current_block, &config));

        // Too small
        let small = make_candidate(3, 100, 1000, 0.0, 100);
        assert!(!small.is_eligible(current_block, &config));
    }

    #[test]
    fn test_weight_calculation_hybrid() {
        let mode = SelectionMode::Hybrid { alpha: 0.3 };
        let max_value = 100_000;

        // Small UTXO: gets proportionally more weight per value
        let small = make_candidate(1, 10_000, 1000, 0.0, 0);
        let small_weight = small.weight(mode, max_value);

        // Large UTXO: gets less weight per value
        let large = make_candidate(2, 100_000, 1000, 0.0, 0);
        let large_weight = large.weight(mode, max_value);

        // Weight per value ratio should favor small holder
        let small_per_value = small_weight as f64 / small.value as f64;
        let large_per_value = large_weight as f64 / large.value as f64;

        assert!(
            small_per_value > large_per_value,
            "Hybrid should favor small holders: small={small_per_value}, large={large_per_value}"
        );
    }

    #[test]
    fn test_weight_calculation_value_weighted() {
        let mode = SelectionMode::ValueWeighted;
        let max_value = 100_000;

        let small = make_candidate(1, 10_000, 1000, 0.0, 0);
        let large = make_candidate(2, 100_000, 1000, 0.0, 0);

        // Weight per value should be equal (1:1) — exact in integer math
        assert_eq!(
            small.weight(mode, max_value) * large.value as u128,
            large.weight(mode, max_value) * small.value as u128,
            "ValueWeighted should have equal weight per value"
        );
    }

    #[test]
    fn test_weight_calculation_cluster() {
        let mode = SelectionMode::ClusterWeighted;
        let max_value = 100_000;

        // Same value, different cluster factors
        let low_factor = make_candidate(1, 50_000, 1000, 0.0, 0);
        let high_factor = make_candidate(2, 50_000, 5000, 0.0, 0);

        assert!(
            low_factor.weight(mode, max_value) > high_factor.weight(mode, max_value),
            "Lower cluster factor should have higher weight"
        );
    }

    #[test]
    fn test_weight_calculation_entropy() {
        let mode = SelectionMode::EntropyWeighted { entropy_bonus: 0.5 };
        let max_value = 100_000;

        // Same value, different entropy
        let low_entropy = make_candidate(1, 50_000, 1000, 0.0, 0);
        let high_entropy = make_candidate(2, 50_000, 1000, 2.0, 0);

        assert!(
            high_entropy.weight(mode, max_value) > low_entropy.weight(mode, max_value),
            "Higher entropy should have higher weight"
        );
    }

    #[test]
    fn test_verifiable_randomness_deterministic() {
        let block_hash = [42u8; 32];
        let height = 1000;

        let seed1 = generate_seed(&block_hash, height);
        let seed2 = generate_seed(&block_hash, height);

        assert_eq!(seed1, seed2, "Same inputs should produce same seed");

        let roll1 = verifiable_random_u128(&seed1, 0);
        let roll2 = verifiable_random_u128(&seed2, 0);

        assert_eq!(roll1, roll2, "Same seed should produce same roll");
    }

    #[test]
    fn test_verifiable_randomness_different_index() {
        let seed = [1u8; 32];

        let roll0 = verifiable_random_u128(&seed, 0);
        let roll1 = verifiable_random_u128(&seed, 1);

        assert_ne!(
            roll0, roll1,
            "Different indices should produce different rolls"
        );
    }

    #[test]
    fn test_draw_winners_basic() {
        let config = LotteryDrawConfig {
            min_utxo_age: 100,
            min_utxo_value: 1000,
            winners_per_draw: 2,
            ..Default::default()
        };

        let candidates = vec![
            make_candidate(1, 10_000, 1000, 0.0, 0),
            make_candidate(2, 20_000, 1000, 0.0, 0),
            make_candidate(3, 30_000, 1000, 0.0, 0),
        ];

        let block_hash = [0u8; 32];
        let result = draw_winners(&candidates, 1000, 500, &block_hash, &config);

        assert!(result.is_some());
        let result = result.unwrap();

        assert_eq!(result.winners.len(), 2);
        assert_eq!(
            result.winners.iter().map(|w| w.payout).sum::<u64>(),
            1000,
            "Total payout should equal pool"
        );
    }

    #[test]
    fn test_draw_no_eligible() {
        let config = LotteryDrawConfig::default();
        let candidates = vec![
            make_candidate(1, 10_000, 1000, 0.0, 999), // Too young
        ];

        let block_hash = [0u8; 32];
        let result = draw_winners(&candidates, 1000, 1000, &block_hash, &config);

        assert!(result.is_none());
    }

    #[test]
    fn test_verify_drawing() {
        let config = LotteryDrawConfig {
            min_utxo_age: 100,
            min_utxo_value: 1000,
            winners_per_draw: 2,
            ..Default::default()
        };

        let candidates = vec![
            make_candidate(1, 10_000, 1000, 0.0, 0),
            make_candidate(2, 20_000, 1000, 0.0, 0),
            make_candidate(3, 30_000, 1000, 0.0, 0),
        ];

        let block_hash = [42u8; 32];
        let result = draw_winners(&candidates, 1000, 500, &block_hash, &config).unwrap();

        // Should verify successfully
        assert!(verify_drawing(&candidates, &result, &block_hash, &config));

        // Should fail with different block hash
        let wrong_hash = [0u8; 32];
        assert!(!verify_drawing(&candidates, &result, &wrong_hash, &config));
    }

    #[test]
    fn test_hybrid_sybil_resistance() {
        // Test that splitting doesn't proportionally increase chances
        let config = LotteryDrawConfig {
            selection_mode: SelectionMode::Hybrid { alpha: 0.3 },
            ..Default::default()
        };

        // One large UTXO
        let large = make_candidate(1, 100_000, 1000, 0.0, 0);
        let large_weight = large.weight(config.selection_mode, 100_000);

        // Same value split into 10 UTXOs
        let small_weight: u128 = (0..10)
            .map(|i| {
                let c = make_candidate(i, 10_000, 1000, 0.0, 0);
                c.weight(config.selection_mode, 100_000)
            })
            .sum();

        // Splitting should give some advantage (α=0.3 → 3.84x expected).
        // This is exactly why Hybrid is NOT the consensus default.
        let gaming_ratio = small_weight as f64 / large_weight as f64;

        assert!(
            gaming_ratio > 1.0 && gaming_ratio < 5.0,
            "Hybrid α=0.3 gaming ratio should be ~3.84x, got {gaming_ratio}"
        );
    }
}
