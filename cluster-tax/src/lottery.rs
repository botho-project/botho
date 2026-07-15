//! Production lottery draw implementation.
//!
//! This module provides the core lottery selection logic for fee
//! redistribution. Unlike the simulation module, this is designed for actual
//! use in block production and validation with verifiable randomness.
//!
//! ## Selection Mode — Path C (value-free)
//!
//! The default selection mode is `Uniform`: every eligible UTXO has an equal
//! chance. This is the ratified **Path C** value-free selection
//! (`docs/research/ct-compatible-lottery-selection.md`), which is compatible
//! with confidential amounts because the draw reads no UTXO value.
//!
//! A uniform draw is not split-invariant on its own — a whale that fragments a
//! position into `k` fresh coins buys `k` tickets. Sybil resistance instead
//! comes from two value-free, structural brakes applied *outside* the weight
//! formula:
//!
//! 1. **Circulation window** ([`CIRCULATION_WINDOW_BLOCKS`]): only outputs
//!    created within the last `N` blocks are eligible, so the whale must
//!    continuously re-split (paying base fees each cycle) to stay in the draw.
//! 2. **Endogenous reward cap** `R = min(fee_pool, ρ · base_fee)`, where `ρ` is
//!    the count of in-window eligible outputs (whale splits included) and
//!    `base_fee` is [`LOTTERY_BASE_FEE_PICO`]. Because the cap rises by exactly
//!    `base_fee` per extra eligible output, a splitter wins back exactly the
//!    base fee it pays: splitting is **net-zero** (net-negative when the pool
//!    is thin). See [`sybil_reward_cap`] and §9 of the research memo.
//!
//! The historical value-weighted modes (`ClusterWeighted`, `ValueWeighted`,
//! `Hybrid`, `EntropyWeighted`) are retained for simulation/back-compat but are
//! no longer the default: they read the (now-confidential) UTXO value and would
//! require a ZK weighted-sampling sort under CT.
//!
//! See `docs/design/cluster-tilted-redistribution.md` for the prior
//! value-weighted design and `docs/research/ct-compatible-lottery-selection.md`
//! for the Path C ratification (redistribution, Sybil, and CT analysis).

/// Circulation window `N`, in blocks: an output is lottery-eligible only if it
/// was created within the last `N` blocks (a recency / "recently circulated"
/// filter). ~14 h at the 5 s reference block time.
///
/// CONSENSUS-CRITICAL: proposer and validators must agree exactly.
/// Ratified value (research §7.5 / §9): 10,000 blocks.
pub const CIRCULATION_WINDOW_BLOCKS: u64 = 10_000;

/// Per-output base fee floor, in picocredits (0.25 BTH; 1 BTH = 1e12 pico).
///
/// Sets the Path C endogenous reward cap `R = min(fee_pool, ρ · base_fee)`
/// (see [`sybil_reward_cap`]). This is the only value gate the lottery keeps
/// beyond dust — matching the public base-fee floor charged on every output.
pub const LOTTERY_BASE_FEE_PICO: u64 = 250_000_000_000;

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

    /// Circulation window `N`, in blocks: an output is eligible only if it was
    /// created within the last `N` blocks (recency filter for Path C).
    /// Default: [`CIRCULATION_WINDOW_BLOCKS`] (10,000).
    pub circulation_window: u64,

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
            circulation_window: CIRCULATION_WINDOW_BLOCKS,
            // Path C: value-free uniform draw over the circulation window.
            // Sybil resistance comes from the window + endogenous reward cap
            // (sybil_reward_cap), not the weight formula. CT-compatible.
            // See docs/research/ct-compatible-lottery-selection.md
            selection_mode: SelectionMode::Uniform,
        }
    }
}

/// Selection mode for lottery winners.
///
/// Different modes provide different trade-offs between progressivity
/// (favoring small holders) and Sybil resistance (preventing gaming).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum SelectionMode {
    /// Uniform: each UTXO has equal chance. **Path C default** — value-free
    /// and CT-compatible; Sybil resistance comes from the circulation window +
    /// endogenous reward cap ([`sybil_reward_cap`]), not the weight formula.
    #[default]
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
    /// Retired as the default under CT (reads the confidential UTXO value).
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
    ///
    /// Path C eligibility is a *circulation window*: the output must be mature
    /// enough (`age >= min_utxo_age`, anti-manipulation) yet recently created
    /// (`age <= circulation_window`, the recency filter that forces a splitter
    /// to keep re-paying base fees), and clear the dust floor
    /// (`value >= min_utxo_value`). All bounds are integer and deterministic.
    pub fn is_eligible(&self, current_block: u64, config: &LotteryDrawConfig) -> bool {
        let age = current_block.saturating_sub(self.creation_block);
        age >= config.min_utxo_age
            && age <= config.circulation_window
            && self.value >= config.min_utxo_value
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

/// Count the in-window eligible outputs `ρ` for a candidate set.
///
/// This is the Path C reward-cap denominator: `ρ` counts every eligible output
/// in the circulation window, **including a splitter's own outputs** —
/// consensus cannot distinguish organic from Sybil outputs, so it must count
/// them all. That is precisely what makes the cap Sybil-neutral (see
/// [`sybil_reward_cap`]).
///
/// CONSENSUS-CRITICAL: a pure count over [`LotteryCandidate::is_eligible`], so
/// proposer and validators agree exactly.
pub fn count_eligible(
    candidates: &[LotteryCandidate],
    current_block: u64,
    config: &LotteryDrawConfig,
) -> usize {
    candidates
        .iter()
        .filter(|c| c.is_eligible(current_block, config))
        .count()
}

/// Path C endogenous reward cap `ρ · base_fee`, in base units (picocredits).
///
/// The per-block lottery payout is capped at `R = min(fee_pool, ρ · base_fee)`,
/// where `ρ` = [`count_eligible`] and `base_fee` = [`LOTTERY_BASE_FEE_PICO`].
/// Under a uniform draw a splitter creating `k` of the `ρ` eligible outputs
/// wins expected `k/ρ · R = k · base_fee`, exactly the `k · base_fee` it pays
/// in base fees — splitting is net-zero (net-negative when `fee_pool < cap`).
/// This is the regime-independent Sybil bound (research §9.1).
///
/// Returns `u128`: `ρ` can be up to the candidate cap and `base_fee` is
/// ~2.5e11, so the product can exceed `u64` in principle; the consensus caller
/// clamps it against the block reward (anti-grinding) before it is used as a
/// `u64` payout.
pub fn sybil_reward_cap(eligible_count: usize) -> u128 {
    (eligible_count as u128).saturating_mul(LOTTERY_BASE_FEE_PICO as u128)
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
        // Weighted sampling WITHOUT replacement: the roll must be taken against
        // the weight of the *remaining* (not-yet-selected) candidates, not the
        // original `total_weight`. Rolling against the full total lets a roll
        // land in the range covered by already-selected candidates, so the
        // cumulative walk finds no winner and the draw silently under-fills —
        // which is common under the uniform (small-integer) Path C weights.
        // Recomputing the remaining total guarantees every slot is filled while
        // staying a pure, deterministic function of the seed and weights.
        let remaining_total: u128 = weights
            .iter()
            .enumerate()
            .filter(|(idx, _)| !used_indices.contains(idx))
            .map(|(_, (_, w))| *w)
            .sum();
        if remaining_total == 0 {
            break;
        }

        // Deterministic 128-bit roll in [0, remaining_total). The modulo bias
        // is at most remaining_total / 2^128 — negligible and, crucially,
        // identical on every platform.
        let roll = verifiable_random_u128(&seed, i as u64) % remaining_total;

        // Select winner based on cumulative weights over remaining candidates.
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
    fn test_default_is_uniform_path_c() {
        // Path C: the default is the value-free Uniform draw over the
        // circulation window. ClusterWeighted (value-reading) is retired as the
        // default because it is not CT-compatible. Sybil resistance now comes
        // from the window + endogenous reward cap, not the weight formula.
        // See docs/research/ct-compatible-lottery-selection.md
        let config = LotteryDrawConfig::default();
        assert!(matches!(config.selection_mode, SelectionMode::Uniform));
        assert_eq!(config.circulation_window, CIRCULATION_WINDOW_BLOCKS);
    }

    #[test]
    fn test_circulation_window_eligibility() {
        // Eligible only inside [min_utxo_age, circulation_window] and above the
        // dust floor. The recency upper bound is the Path C addition.
        let config = LotteryDrawConfig::default(); // age 720..=10_000
        let current = 20_000u64;

        // In window: created 5_000 blocks ago (age 15_000)? No — too old.
        let too_old = make_candidate(1, 1_000_000, 1000, 0.0, current - 15_000);
        assert!(
            !too_old.is_eligible(current, &config),
            "age > window excluded"
        );

        // Recently circulated: age 5_000 (within 10_000) and mature (>=720).
        let fresh = make_candidate(2, 1_000_000, 1000, 0.0, current - 5_000);
        assert!(fresh.is_eligible(current, &config), "in-window is eligible");

        // Boundary: exactly N blocks old is still eligible (inclusive).
        let boundary = make_candidate(3, 1_000_000, 1000, 0.0, current - 10_000);
        assert!(boundary.is_eligible(current, &config), "age == N eligible");

        // Too young (age < min_utxo_age).
        let young = make_candidate(4, 1_000_000, 1000, 0.0, current - 100);
        assert!(!young.is_eligible(current, &config), "age < min excluded");
    }

    #[test]
    fn test_splitting_is_net_zero_under_reward_cap() {
        // KEY Sybil property (research §9.1): under the endogenous cap
        // R = ρ·base_fee with a uniform draw, a whale that creates k of the ρ
        // eligible outputs wins back EXACTLY the base fees it pays — net-zero,
        // independent of the split factor k and the organic rate ρ_o.
        //
        // winnings = R · k / ρ = (ρ·base_fee)·k/ρ = k·base_fee = cost.
        // This is integer-exact because R is a multiple of ρ.
        let base_fee = LOTTERY_BASE_FEE_PICO as u128;
        for rho_o in [2usize, 20, 200, 5000] {
            for k in [1usize, 5, 50, 500, 5000] {
                let rho = rho_o + k;
                let cap = sybil_reward_cap(rho); // R with the pool ≫ cap
                assert_eq!(cap, rho as u128 * base_fee);

                // Whale's expected winnings = R · (k/ρ), integer-exact.
                let winnings = cap * k as u128 / rho as u128;
                let cost = k as u128 * base_fee;
                assert_eq!(
                    winnings, cost,
                    "splitting must be net-zero: ρ_o={rho_o} k={k} \
                     winnings={winnings} cost={cost}"
                );
            }
        }
    }

    #[test]
    fn test_splitting_is_net_negative_when_pool_thin() {
        // net = 0 is only the ceiling: when the actual fee pool is below the
        // cap, R = pool < ρ·base_fee, so the whale wins strictly less than it
        // pays (research §9.1, thin-pool table).
        let base_fee = LOTTERY_BASE_FEE_PICO as u128;
        let (rho_o, k) = (20usize, 500usize);
        let rho = rho_o + k;
        let cap = sybil_reward_cap(rho);
        // Pool at half the cap.
        let pool = cap / 2;
        let r = pool.min(cap);
        let winnings = r * k as u128 / rho as u128;
        let cost = k as u128 * base_fee;
        assert!(winnings < cost, "thin pool → net-negative for the splitter");
    }

    #[test]
    fn test_sybil_reward_cap_grows_one_base_fee_per_output() {
        // The cap must rise by exactly one base fee for each additional
        // eligible output — the algebraic root of net-zero splitting.
        let base = LOTTERY_BASE_FEE_PICO as u128;
        assert_eq!(sybil_reward_cap(0), 0);
        assert_eq!(sybil_reward_cap(1), base);
        assert_eq!(sybil_reward_cap(101) - sybil_reward_cap(100), base);
    }

    #[test]
    fn test_count_eligible_counts_splits() {
        // ρ counts every in-window eligible output, splits included.
        let config = LotteryDrawConfig::default();
        let current = 30_000u64;
        // Created 25_000 → age 5_000, inside the 10_000 window.
        let cands: Vec<LotteryCandidate> = (0..10)
            .map(|i| make_candidate(i, 1_000_000, 1000, 0.0, 25_000))
            .collect();
        assert_eq!(count_eligible(&cands, current, &config), 10);

        // Add an out-of-window (too-old, age 30_000) output: not counted.
        let mut with_old = cands.clone();
        with_old.push(make_candidate(99, 1_000_000, 1000, 0.0, 0));
        assert_eq!(count_eligible(&with_old, current, &config), 10);
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
