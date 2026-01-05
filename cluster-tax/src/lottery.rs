//! Production lottery draw implementation.
//!
//! This module provides the core lottery selection logic for fee redistribution.
//! Unlike the simulation module, this is designed for actual use in block production
//! and validation with verifiable randomness.
//!
//! ## Selection Mode
//!
//! The default selection mode is `Hybrid { alpha: 0.3 }`, which provides:
//! - 3.84x Sybil resistance (acceptable gaming ratio)
//! - 69% Gini coefficient reduction (progressive redistribution)
//! - 0 bits privacy cost (no information leaked)
//!
//! See `docs/design/lottery-redistribution.md` for analysis.

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
            // Hybrid α=0.3: Best trade-off per docs/design/lottery-redistribution.md
            selection_mode: SelectionMode::Hybrid { alpha: 0.3 },
        }
    }
}

/// Selection mode for lottery winners.
///
/// Different modes provide different trade-offs between progressivity
/// (favoring small holders) and Sybil resistance (preventing gaming).
#[derive(Clone, Copy, Debug, PartialEq)]
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
    ClusterWeighted,

    /// Entropy-weighted: higher tag entropy = more weight.
    /// Sybil-resistant (splits preserve entropy).
    EntropyWeighted { entropy_bonus: f64 },
}

impl Default for SelectionMode {
    fn default() -> Self {
        Self::Hybrid { alpha: 0.3 }
    }
}

/// A UTXO eligible for lottery participation.
#[derive(Clone, Debug)]
pub struct LotteryCandidate {
    /// Unique identifier (tx_hash, output_index)
    pub id: [u8; 36],
    /// UTXO value in base units
    pub value: u64,
    /// Cluster factor (1.0 to 6.0)
    pub cluster_factor: f64,
    /// Tag entropy in bits (from TagVector::cluster_entropy())
    pub tag_entropy: f64,
    /// Block height when UTXO was created
    pub creation_block: u64,
}

impl LotteryCandidate {
    /// Create a new lottery candidate.
    pub fn new(
        id: [u8; 36],
        value: u64,
        cluster_factor: f64,
        tags: &TagVector,
        creation_block: u64,
    ) -> Self {
        Self {
            id,
            value,
            cluster_factor,
            tag_entropy: tags.cluster_entropy(),
            creation_block,
        }
    }

    /// Check if this UTXO is eligible for the lottery.
    pub fn is_eligible(&self, current_block: u64, config: &LotteryDrawConfig) -> bool {
        let age = current_block.saturating_sub(self.creation_block);
        age >= config.min_utxo_age && self.value >= config.min_utxo_value
    }

    /// Calculate lottery weight based on selection mode.
    pub fn weight(&self, mode: SelectionMode, max_value: u64) -> f64 {
        match mode {
            SelectionMode::Uniform => 1.0,

            SelectionMode::ValueWeighted => self.value as f64,

            SelectionMode::Hybrid { alpha } => {
                let norm_value = if max_value > 0 {
                    self.value as f64 / max_value as f64
                } else {
                    0.0
                };
                alpha + (1.0 - alpha) * norm_value
            }

            SelectionMode::ClusterWeighted => {
                // weight = value × (max_factor - factor + 1) / max_factor
                const MAX_FACTOR: f64 = 6.0;
                let factor_bonus = (MAX_FACTOR - self.cluster_factor + 1.0) / MAX_FACTOR;
                self.value as f64 * factor_bonus
            }

            SelectionMode::EntropyWeighted { entropy_bonus } => {
                // weight = value × (1 + entropy_bonus × tag_entropy)
                let entropy_factor = 1.0 + entropy_bonus * self.tag_entropy;
                self.value as f64 * entropy_factor
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
/// `LotteryResult` with winners and payouts, or `None` if no eligible candidates.
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

    // Calculate weights for all candidates
    let weights: Vec<(&LotteryCandidate, f64)> = eligible
        .iter()
        .map(|c| (*c, c.weight(config.selection_mode, max_value)))
        .collect();

    let total_weight: f64 = weights.iter().map(|(_, w)| *w).sum();

    if total_weight <= 0.0 {
        return None;
    }

    // Select winners using verifiable random selection
    let num_winners = config.winners_per_draw.min(eligible.len());
    let payout_per_winner = pool_amount / num_winners as u64;

    let mut winners = Vec::with_capacity(num_winners);
    let mut used_indices = std::collections::HashSet::new();

    for i in 0..num_winners {
        // Generate deterministic random value for this selection
        let roll = verifiable_random(&seed, i as u64, total_weight);

        // Select winner based on cumulative weights
        let mut cumulative = 0.0;
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

/// Generate a verifiable random f64 in range [0, max).
fn verifiable_random(seed: &[u8; 32], index: u64, max: f64) -> f64 {
    let mut hasher = Sha256::new();
    hasher.update(seed);
    hasher.update(index.to_le_bytes());
    let hash: [u8; 32] = hasher.finalize().into();

    // Convert first 8 bytes to u64, then to f64 in [0, 1)
    let rand_u64 = u64::from_le_bytes(hash[0..8].try_into().unwrap());
    let rand_f64 = (rand_u64 as f64) / (u64::MAX as f64);

    rand_f64 * max
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_candidate(id: u8, value: u64, factor: f64, entropy: f64, block: u64) -> LotteryCandidate {
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
    fn test_default_is_hybrid() {
        let config = LotteryDrawConfig::default();
        assert!(matches!(config.selection_mode, SelectionMode::Hybrid { alpha } if (alpha - 0.3).abs() < 0.001));
    }

    #[test]
    fn test_eligibility_check() {
        let config = LotteryDrawConfig::default();
        let current_block = 1000;

        // Too young
        let young = make_candidate(1, 1_000_000, 1.0, 0.0, 500);
        assert!(!young.is_eligible(current_block, &config));

        // Old enough
        let old = make_candidate(2, 1_000_000, 1.0, 0.0, 100);
        assert!(old.is_eligible(current_block, &config));

        // Too small
        let small = make_candidate(3, 100, 1.0, 0.0, 100);
        assert!(!small.is_eligible(current_block, &config));
    }

    #[test]
    fn test_weight_calculation_hybrid() {
        let mode = SelectionMode::Hybrid { alpha: 0.3 };
        let max_value = 100_000;

        // Small UTXO: gets proportionally more weight per value
        let small = make_candidate(1, 10_000, 1.0, 0.0, 0);
        let small_weight = small.weight(mode, max_value);

        // Large UTXO: gets less weight per value
        let large = make_candidate(2, 100_000, 1.0, 0.0, 0);
        let large_weight = large.weight(mode, max_value);

        // Weight per value ratio should favor small holder
        let small_per_value = small_weight / small.value as f64;
        let large_per_value = large_weight / large.value as f64;

        assert!(
            small_per_value > large_per_value,
            "Hybrid should favor small holders: small={small_per_value}, large={large_per_value}"
        );
    }

    #[test]
    fn test_weight_calculation_value_weighted() {
        let mode = SelectionMode::ValueWeighted;
        let max_value = 100_000;

        let small = make_candidate(1, 10_000, 1.0, 0.0, 0);
        let large = make_candidate(2, 100_000, 1.0, 0.0, 0);

        // Weight per value should be equal (1:1)
        let small_per_value = small.weight(mode, max_value) / small.value as f64;
        let large_per_value = large.weight(mode, max_value) / large.value as f64;

        assert!(
            (small_per_value - large_per_value).abs() < 0.001,
            "ValueWeighted should have equal weight per value"
        );
    }

    #[test]
    fn test_weight_calculation_cluster() {
        let mode = SelectionMode::ClusterWeighted;
        let max_value = 100_000;

        // Same value, different cluster factors
        let low_factor = make_candidate(1, 50_000, 1.0, 0.0, 0);
        let high_factor = make_candidate(2, 50_000, 5.0, 0.0, 0);

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
        let low_entropy = make_candidate(1, 50_000, 1.0, 0.0, 0);
        let high_entropy = make_candidate(2, 50_000, 1.0, 2.0, 0);

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

        let roll1 = verifiable_random(&seed1, 0, 100.0);
        let roll2 = verifiable_random(&seed2, 0, 100.0);

        assert_eq!(roll1, roll2, "Same seed should produce same roll");
    }

    #[test]
    fn test_verifiable_randomness_different_index() {
        let seed = [1u8; 32];

        let roll0 = verifiable_random(&seed, 0, 100.0);
        let roll1 = verifiable_random(&seed, 1, 100.0);

        assert_ne!(roll0, roll1, "Different indices should produce different rolls");
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
            make_candidate(1, 10_000, 1.0, 0.0, 0),
            make_candidate(2, 20_000, 1.0, 0.0, 0),
            make_candidate(3, 30_000, 1.0, 0.0, 0),
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
            make_candidate(1, 10_000, 1.0, 0.0, 999), // Too young
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
            make_candidate(1, 10_000, 1.0, 0.0, 0),
            make_candidate(2, 20_000, 1.0, 0.0, 0),
            make_candidate(3, 30_000, 1.0, 0.0, 0),
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
        let large = make_candidate(1, 100_000, 1.0, 0.0, 0);
        let large_weight = large.weight(config.selection_mode, 100_000);

        // Same value split into 10 UTXOs
        let small_weight: f64 = (0..10)
            .map(|i| {
                let c = make_candidate(i, 10_000, 1.0, 0.0, 0);
                c.weight(config.selection_mode, 100_000)
            })
            .sum();

        // Splitting should give some advantage (α=0.3 → 3.84x expected)
        let gaming_ratio = small_weight / large_weight;

        assert!(
            gaming_ratio > 1.0 && gaming_ratio < 5.0,
            "Hybrid α=0.3 gaming ratio should be ~3.84x, got {gaming_ratio}"
        );
    }
}
