// Copyright (c) 2024 Botho Foundation

//! Lottery fee redistribution for block production and validation.
//!
//! This module integrates the `cluster-tax` lottery system into consensus:
//!
//! - **Fee splitting**: 80% to lottery pool, 20% burned (deflationary)
//! - **Lottery draw**: Select 4 winners per block from eligible UTXOs
//! - **Block validation**: Verify lottery results are correct
//!
//! # Fee Flow
//!
//! ```text
//! Transaction Fees
//!        │
//!        ├──(80%)──> Lottery Pool ──> 4 Winners (random UTXOs)
//!        │
//!        └──(20%)──> Burned (not included in outputs)
//! ```
//!
//! # Usage
//!
//! ```ignore
//! // In block builder:
//! let fees_collected = block.total_fees();
//! let (pool_amount, burn_amount) = split_fees(fees_collected, &config);
//!
//! // Draw winners from eligible UTXOs
//! let result = draw_lottery_winners(
//!     &utxo_candidates,
//!     pool_amount,
//!     block_height,
//!     &prev_block_hash,
//!     &config,
//! );
//!
//! // Add lottery outputs to block
//! let lottery_outputs = result.to_outputs(&minter_private_key);
//! ```

use bth_cluster_tax::{
    draw_winners, verify_drawing, LotteryCandidate, LotteryDrawConfig, LotteryResult,
    LotteryWinner, SelectionMode, TagVector,
};
use tracing::{debug, info};

/// Configuration for lottery fee redistribution.
#[derive(Clone, Debug)]
pub struct LotteryFeeConfig {
    /// Fraction of fees that go to lottery pool (remainder burned).
    /// Default: 0.8 (80% to lottery, 20% burned)
    pub pool_fraction_permille: u32,

    /// Lottery draw configuration
    pub draw_config: LotteryDrawConfig,
}

impl Default for LotteryFeeConfig {
    fn default() -> Self {
        Self {
            pool_fraction_permille: 800, // 80%
            draw_config: draw_config_from_env(),
        }
    }
}

/// Build the lottery draw config, honoring optional test-only eligibility
/// overrides.
///
/// Production runs use [`LotteryDrawConfig::default`] (UTXOs must be 720 blocks
/// old and worth at least 1 microBTH to enter the draw). When
/// `BOTHO_LOTTERY_MIN_UTXO_AGE` and/or `BOTHO_LOTTERY_MIN_UTXO_VALUE` are set
/// to non-negative integers, the corresponding threshold is lowered to that
/// value.
///
/// This exists purely so automated end-to-end tests (e.g. the three-user
/// exchange-until-lottery-payout node-backed test, issue #394) can make freshly
/// created UTXOs eligible without pre-mining ~720 blocks per round.
///
/// CONSENSUS NOTE: both the block proposer (`run::apply_lottery_to_block`) and
/// the validator (`LedgerStore::add_block`) build their lottery config via
/// `LotteryFeeConfig::default()`, so both read the SAME environment in the same
/// process and agree on the candidate set exactly — the lottery draw stays
/// consensus-deterministic. The override is a no-op when the variables are
/// unset, so it never changes mainnet/testnet behavior.
fn draw_config_from_env() -> LotteryDrawConfig {
    let mut cfg = LotteryDrawConfig::default();
    if let Ok(raw) = std::env::var("BOTHO_LOTTERY_MIN_UTXO_AGE") {
        if let Ok(age) = raw.trim().parse::<u64>() {
            cfg.min_utxo_age = age;
        } else {
            tracing::warn!(
                "Ignoring invalid BOTHO_LOTTERY_MIN_UTXO_AGE={:?}: must be a non-negative integer",
                raw
            );
        }
    }
    if let Ok(raw) = std::env::var("BOTHO_LOTTERY_MIN_UTXO_VALUE") {
        if let Ok(value) = raw.trim().parse::<u64>() {
            cfg.min_utxo_value = value;
        } else {
            tracing::warn!(
                "Ignoring invalid BOTHO_LOTTERY_MIN_UTXO_VALUE={:?}: must be a non-negative integer",
                raw
            );
        }
    }
    cfg
}

impl LotteryFeeConfig {
    /// Split fees into lottery pool and burn amounts.
    ///
    /// Returns (pool_amount, burn_amount).
    pub fn split_fees(&self, total_fees: u64) -> (u64, u64) {
        let pool_amount = (total_fees as u128 * self.pool_fraction_permille as u128 / 1000) as u64;
        let burn_amount = total_fees.saturating_sub(pool_amount);
        (pool_amount, burn_amount)
    }
}

/// Split fees into lottery pool and burn amounts.
///
/// Default: 80% to lottery, 20% burned.
///
/// # Returns
/// (pool_amount, burn_amount)
pub fn split_fees(total_fees: u64, config: &LotteryFeeConfig) -> (u64, u64) {
    config.split_fees(total_fees)
}

/// Result of lottery drawing for a block, ready for inclusion.
#[derive(Clone, Debug)]
pub struct BlockLotteryResult {
    /// Block height of the drawing
    pub block_height: u64,

    /// Total fees collected in the block
    pub total_fees: u64,

    /// Amount going to lottery pool (80%)
    pub pool_amount: u64,

    /// Amount burned (20%)
    pub burn_amount: u64,

    /// Winning UTXOs and their payouts
    pub winners: Vec<LotteryWinner>,

    /// Seed used for verifiable randomness
    pub seed: [u8; 32],
}

impl BlockLotteryResult {
    /// Create a result when there are no winners (no eligible candidates, or
    /// nothing to pay out).
    ///
    /// Only the fee burn share is burned; the pool share (fees + emission)
    /// carries over to future blocks via the persistent lottery pool.
    pub fn no_winners(
        block_height: u64,
        total_fees: u64,
        accounting: &LotteryPoolAccounting,
    ) -> Self {
        Self {
            block_height,
            total_fees,
            pool_amount: 0,
            burn_amount: accounting.fee_burn,
            winners: Vec::new(),
            seed: [0u8; 32],
        }
    }

    /// Total amount distributed to winners.
    pub fn total_distributed(&self) -> u64 {
        self.winners.iter().map(|w| w.payout).sum()
    }

    /// Check if this result has winners.
    pub fn has_winners(&self) -> bool {
        !self.winners.is_empty()
    }
}

/// Pool accounting for one block's lottery.
///
/// CONSENSUS-CRITICAL: every field is a deterministic integer function of
/// (block fees, block reward, height schedule, stored pool balance), all of
/// which are consensus state — proposer and validators must agree exactly.
///
/// Width note: the per-block amounts (`fee_pool`, `fee_burn`, `emission_share`,
/// `payout`) stay `u64`. Each is bounded by a single block: fee shares are a
/// split of one block's `total_fees` (itself `u64`), `emission_share` is a
/// fraction of one block reward, and `payout` is capped at one block reward.
/// None can approach `u64::MAX` within a single block. Only the cumulative
/// `available` carryover can grow unbounded across blocks, so it alone is
/// widened to `u128`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LotteryPoolAccounting {
    /// Pool share of this block's transaction fees (80% by default).
    pub fee_pool: u64,
    /// Burn share of this block's transaction fees (20% by default).
    pub fee_burn: u64,
    /// Lottery share of this block's emission (height-scheduled fraction of
    /// the block reward; the miner receives the remainder).
    pub emission_share: u64,
    /// Total available to distribute: carryover + emission share + fee pool.
    ///
    /// This is the cumulative carryover balance and is the one field here that
    /// can grow without bound: the pool drains at most one block reward per
    /// block (the anti-grinding `payout` cap) but inflow per block is
    /// `emission_share + fee_pool`, so sustained high-fee blocks accumulate.
    /// It is therefore `u128` (the per-block amounts below stay `u64`): a `u64`
    /// would saturate at `u64::MAX` (~18.4M BTH in picocredits) under sustained
    /// inflow, silently losing value from supply conservation.
    pub available: u128,
    /// Amount actually paid out this block: min(available, payout cap).
    ///
    /// The cap (one block reward) makes seed-grinding unprofitable by
    /// construction: regrinding the previous block costs a full PoW solution
    /// but can shift at most a fraction of one reward's worth of payout.
    pub payout: u64,
}

impl LotteryPoolAccounting {
    /// Pool balance carried to the next block if `distributed` was paid out.
    ///
    /// Returns the cumulative carryover as `u128`; `distributed` (a single
    /// block's payout) is `u64` and widened for the subtraction.
    pub fn carryover_after(&self, distributed: u64) -> u128 {
        self.available.saturating_sub(distributed as u128)
    }
}

/// Compute the lottery pool accounting for a block.
///
/// # Arguments
/// * `total_fees` - Total transaction fees in the block
/// * `emission_share` - Lottery share of the block reward
///   (`MintingTx::lottery_emission_share`)
/// * `stored_pool` - Carryover pool balance before this block
/// * `payout_cap` - Maximum payout per block (the block reward; anti-grinding
///   bound)
pub fn compute_pool_accounting(
    total_fees: u64,
    emission_share: u64,
    stored_pool: u128,
    payout_cap: u64,
    config: &LotteryFeeConfig,
) -> LotteryPoolAccounting {
    let (fee_pool, fee_burn) = config.split_fees(total_fees);
    // Cumulative carryover: widened to u128 so sustained high-fee inflow can
    // never saturate (see LotteryPoolAccounting::available).
    let available = stored_pool
        .saturating_add(emission_share as u128)
        .saturating_add(fee_pool as u128);
    // payout is capped at one block reward (u64), so the min result always
    // fits in u64; the cast is lossless by construction.
    let payout = available.min(payout_cap as u128) as u64;

    LotteryPoolAccounting {
        fee_pool,
        fee_burn,
        emission_share,
        available,
        payout,
    }
}

/// Draw lottery winners for a block.
///
/// # Arguments
/// * `candidates` - Eligible UTXOs from the UTXO set
/// * `total_fees` - Total fees collected from block transactions
/// * `accounting` - Pool accounting (carryover + emission + fees, capped)
/// * `block_height` - Current block height
/// * `prev_block_hash` - Hash of previous block (for verifiable randomness)
/// * `config` - Lottery configuration
///
/// # Returns
/// `BlockLotteryResult` with winners and fee allocation
pub fn draw_lottery_winners(
    candidates: &[LotteryCandidate],
    total_fees: u64,
    accounting: &LotteryPoolAccounting,
    block_height: u64,
    prev_block_hash: &[u8; 32],
    config: &LotteryFeeConfig,
) -> BlockLotteryResult {
    if accounting.payout == 0 {
        debug!(
            block_height = block_height,
            "Nothing to distribute, skipping lottery"
        );
        return BlockLotteryResult::no_winners(block_height, total_fees, accounting);
    }

    // Draw winners using cluster-tax lottery implementation
    match draw_winners(
        candidates,
        accounting.payout,
        block_height,
        prev_block_hash,
        &config.draw_config,
    ) {
        Some(result) => {
            info!(
                block_height = block_height,
                winners = result.winners.len(),
                payout = accounting.payout,
                fee_burn = accounting.fee_burn,
                emission_share = accounting.emission_share,
                "Lottery draw complete"
            );

            BlockLotteryResult {
                block_height,
                total_fees,
                pool_amount: accounting.payout,
                burn_amount: accounting.fee_burn,
                winners: result.winners,
                seed: result.seed,
            }
        }
        None => {
            debug!(
                block_height = block_height,
                "No eligible lottery candidates; pool carries over"
            );
            BlockLotteryResult::no_winners(block_height, total_fees, accounting)
        }
    }
}

/// Verify a lottery drawing result.
///
/// Re-runs the drawing with the same parameters and verifies the result
/// matches.
///
/// # Arguments
/// * `candidates` - Eligible UTXOs (must match what was used in draw)
/// * `result` - The result to verify
/// * `prev_block_hash` - Hash of previous block
/// * `config` - Lottery configuration
///
/// # Returns
/// `true` if the drawing is valid, `false` otherwise
pub fn verify_lottery_result(
    candidates: &[LotteryCandidate],
    result: &BlockLotteryResult,
    accounting: &LotteryPoolAccounting,
    prev_block_hash: &[u8; 32],
    config: &LotteryFeeConfig,
) -> bool {
    if result.winners.is_empty() {
        // No winners: only the fee burn share is burned; the pool share
        // (fees + emission) carries over.
        return result.pool_amount == 0 && result.burn_amount == accounting.fee_burn;
    }

    if result.pool_amount != accounting.payout || result.burn_amount != accounting.fee_burn {
        return false;
    }

    // Verify the lottery draw itself
    let lottery_result = LotteryResult {
        block_height: result.block_height,
        pool_amount: result.pool_amount,
        winners: result.winners.clone(),
        seed: result.seed,
    };

    verify_drawing(
        candidates,
        &lottery_result,
        prev_block_hash,
        &config.draw_config,
    )
}

/// Convert a UTXO to a lottery candidate.
///
/// # Arguments
/// * `utxo_id` - 36-byte UTXO identifier (tx_hash || output_index)
/// * `value` - UTXO value
/// * `cluster_factor` - Cluster factor for this UTXO (1000-6000 scale)
/// * `tags` - Tag vector for entropy calculation
/// * `creation_block` - Block height when UTXO was created
pub fn utxo_to_candidate(
    utxo_id: [u8; 36],
    value: u64,
    cluster_factor: u64,
    tags: &TagVector,
    creation_block: u64,
) -> LotteryCandidate {
    LotteryCandidate::new(utxo_id, value, cluster_factor, tags, creation_block)
}

/// Lottery state tracking across blocks.
///
/// Tracks cumulative statistics for monitoring and analytics.
#[derive(Clone, Debug, Default)]
pub struct LotteryStats {
    /// Total fees processed
    pub total_fees_processed: u64,

    /// Total amount distributed to winners
    pub total_distributed: u64,

    /// Total amount burned
    pub total_burned: u64,

    /// Total number of drawings
    pub total_drawings: u64,

    /// Total number of winners paid
    pub total_winners: u64,
}

impl LotteryStats {
    /// Create new empty stats.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a lottery result.
    pub fn record_result(&mut self, result: &BlockLotteryResult) {
        self.total_fees_processed += result.total_fees;
        self.total_distributed += result.total_distributed();
        self.total_burned += result.burn_amount;
        if !result.winners.is_empty() {
            self.total_drawings += 1;
            self.total_winners += result.winners.len() as u64;
        }
    }

    /// Effective burn rate (burned / total fees) as permille.
    pub fn burn_rate_permille(&self) -> u32 {
        if self.total_fees_processed == 0 {
            return 0;
        }
        (self.total_burned as u128 * 1000 / self.total_fees_processed as u128) as u32
    }

    /// Average payout per winner.
    pub fn avg_payout(&self) -> u64 {
        if self.total_winners == 0 {
            return 0;
        }
        self.total_distributed / self.total_winners
    }
}

/// Errors that can occur during lottery validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LotteryValidationError {
    /// Fee split doesn't match expected 80/20 split
    InvalidFeeSplit {
        expected_pool: u64,
        expected_burn: u64,
        actual_pool: u64,
        actual_burn: u64,
    },
    /// Lottery drawing verification failed
    InvalidDrawing,
    /// Total payout doesn't match expected amount
    PayoutMismatch { expected: u64, actual: u64 },
    /// Number of outputs doesn't match number of winners
    OutputCountMismatch { expected: usize, actual: usize },
    /// A winner UTXO is not in the eligible candidates
    WinnerNotEligible { utxo_id: String },
    /// Lottery output doesn't match winner
    OutputMismatch { index: usize, reason: String },
}

impl std::fmt::Display for LotteryValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidFeeSplit {
                expected_pool,
                expected_burn,
                actual_pool,
                actual_burn,
            } => write!(
                f,
                "Invalid fee split: expected pool={}, burn={}, got pool={}, burn={}",
                expected_pool, expected_burn, actual_pool, actual_burn
            ),
            Self::InvalidDrawing => write!(f, "Lottery drawing verification failed"),
            Self::PayoutMismatch { expected, actual } => {
                write!(f, "Payout mismatch: expected {}, got {}", expected, actual)
            }
            Self::OutputCountMismatch { expected, actual } => {
                write!(
                    f,
                    "Output count mismatch: expected {}, got {}",
                    expected, actual
                )
            }
            Self::WinnerNotEligible { utxo_id } => {
                write!(f, "Winner UTXO not eligible: {}", utxo_id)
            }
            Self::OutputMismatch { index, reason } => {
                write!(f, "Output {} mismatch: {}", index, reason)
            }
        }
    }
}

impl std::error::Error for LotteryValidationError {}

/// Validate the lottery results in a block.
///
/// This function verifies that:
/// 1. Fee splitting is correct (80% pool, 20% burned)
/// 2. Winner selection matches the deterministic algorithm
/// 3. Payouts sum to the pool amount
/// 4. All winners are from eligible UTXOs
/// 5. Lottery outputs match the claimed winners
///
/// # Arguments
/// * `block` - The block to validate
/// * `candidates` - Eligible UTXOs from the UTXO set (must be at state before
///   block)
/// * `prev_block_hash` - Hash of the previous block (for verifiable randomness)
/// * `config` - Lottery configuration
///
/// # Returns
/// `Ok(())` if validation passes, `Err(LotteryValidationError)` otherwise.
pub fn validate_block_lottery(
    block: &crate::block::Block,
    candidates: &[LotteryCandidate],
    stored_pool: u128,
    prev_block_hash: &[u8; 32],
    config: &LotteryFeeConfig,
) -> Result<u128, LotteryValidationError> {
    let total_fees = block.total_fees();

    // 1. Compute the expected pool accounting from consensus state: fees,
    // the height-scheduled emission share, the stored carryover pool, and
    // the per-block payout cap (one block reward; anti-grinding bound).
    let emission_share = block.minting_tx.lottery_emission_share();
    let accounting = compute_pool_accounting(
        total_fees,
        emission_share,
        stored_pool,
        block.minting_tx.reward,
        config,
    );

    // Handle no-winners case: only the fee burn share is burned; the pool
    // share (fees + emission) carries over to future blocks.
    if block.lottery_outputs.is_empty() {
        if block.lottery_summary.pool_distributed != 0
            || block.lottery_summary.amount_burned != accounting.fee_burn
        {
            return Err(LotteryValidationError::InvalidFeeSplit {
                expected_pool: 0,
                expected_burn: accounting.fee_burn,
                actual_pool: block.lottery_summary.pool_distributed,
                actual_burn: block.lottery_summary.amount_burned,
            });
        }
        // A no-winner block is only valid if the deterministic draw actually
        // produces no winners. Without this check a producer could omit the
        // (predictable) draw whenever it doesn't favor them, carry the pool
        // over, and include it only when they win.
        if accounting.payout > 0
            && draw_winners(
                candidates,
                accounting.payout,
                block.height(),
                prev_block_hash,
                &config.draw_config,
            )
            .is_some()
        {
            return Err(LotteryValidationError::InvalidDrawing);
        }
        return Ok(accounting.carryover_after(0));
    }

    // Verify the split in the summary
    if block.lottery_summary.pool_distributed != accounting.payout
        || block.lottery_summary.amount_burned != accounting.fee_burn
    {
        return Err(LotteryValidationError::InvalidFeeSplit {
            expected_pool: accounting.payout,
            expected_burn: accounting.fee_burn,
            actual_pool: block.lottery_summary.pool_distributed,
            actual_burn: block.lottery_summary.amount_burned,
        });
    }

    // 2. Reconstruct BlockLotteryResult from block data for verification
    let block_result = BlockLotteryResult {
        block_height: block.height(),
        total_fees,
        pool_amount: accounting.payout,
        burn_amount: accounting.fee_burn,
        winners: block
            .lottery_outputs
            .iter()
            .map(|output| LotteryWinner {
                utxo_id: output.winner_utxo_id(),
                payout: output.payout,
            })
            .collect(),
        seed: block.lottery_summary.lottery_seed,
    };

    // 3. Verify the lottery drawing
    if !verify_lottery_result(
        candidates,
        &block_result,
        &accounting,
        prev_block_hash,
        config,
    ) {
        return Err(LotteryValidationError::InvalidDrawing);
    }

    // 4. Verify total payouts match the capped payout amount
    let total_payouts: u64 = block.lottery_outputs.iter().map(|o| o.payout).sum();
    if total_payouts != accounting.payout {
        return Err(LotteryValidationError::PayoutMismatch {
            expected: accounting.payout,
            actual: total_payouts,
        });
    }

    // 5. Verify all winners are in the eligible candidates
    for output in &block.lottery_outputs {
        let winner_id = output.winner_utxo_id();
        let is_eligible = candidates.iter().any(|c| c.id == winner_id);
        if !is_eligible {
            return Err(LotteryValidationError::WinnerNotEligible {
                utxo_id: hex::encode(&winner_id[..8]),
            });
        }
    }

    info!(
        block_height = block.height(),
        winners = block.lottery_outputs.len(),
        payout = accounting.payout,
        fee_burn = accounting.fee_burn,
        emission_share = emission_share,
        pool_after = accounting.carryover_after(accounting.payout),
        "Lottery validation passed"
    );

    Ok(accounting.carryover_after(accounting.payout))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bth_cluster_tax::TagVector;

    fn make_candidate(id: u8, value: u64, creation_block: u64) -> LotteryCandidate {
        let mut utxo_id = [0u8; 36];
        utxo_id[0] = id;
        let empty_tags = TagVector::new();
        LotteryCandidate::new(utxo_id, value, 1000, &empty_tags, creation_block)
    }

    #[test]
    fn test_fee_splitting_default() {
        let config = LotteryFeeConfig::default();
        let (pool, burn) = config.split_fees(1000);

        assert_eq!(pool, 800); // 80%
        assert_eq!(burn, 200); // 20%
    }

    #[test]
    fn test_fee_splitting_custom() {
        let config = LotteryFeeConfig {
            pool_fraction_permille: 750, // 75%
            ..Default::default()
        };
        let (pool, burn) = config.split_fees(1000);

        assert_eq!(pool, 750); // 75%
        assert_eq!(burn, 250); // 25%
    }

    #[test]
    fn test_fee_splitting_zero() {
        let config = LotteryFeeConfig::default();
        let (pool, burn) = config.split_fees(0);

        assert_eq!(pool, 0);
        assert_eq!(burn, 0);
    }

    #[test]
    fn test_no_winners_result() {
        let config = LotteryFeeConfig::default();
        // 1000 fees, no emission, no carryover, cap = one block reward
        let accounting = compute_pool_accounting(1000, 0, 0, 1000, &config);
        let result = BlockLotteryResult::no_winners(100, 1000, &accounting);

        assert_eq!(result.block_height, 100);
        assert_eq!(result.total_fees, 1000);
        assert_eq!(result.winners.len(), 0);
        // Only the fee burn share is burned; the pool share carries over
        assert_eq!(result.burn_amount, 200);
        assert_eq!(result.pool_amount, 0);
        assert_eq!(result.total_distributed(), 0);
        assert_eq!(accounting.carryover_after(0), 800);
    }

    #[test]
    fn test_pool_accounting_carryover_and_cap() {
        let config = LotteryFeeConfig::default();

        // Inflow: 800 (fees) + 500 (emission) + 1000 (carryover) = 2300
        // Cap: 1500 (block reward) -> payout 1500, carryover 800
        let accounting = compute_pool_accounting(1000, 500, 1000, 1500, &config);
        assert_eq!(accounting.fee_pool, 800);
        assert_eq!(accounting.fee_burn, 200);
        assert_eq!(accounting.available, 2300);
        assert_eq!(accounting.payout, 1500);
        assert_eq!(accounting.carryover_after(accounting.payout), 800);

        // Uncapped case: everything available pays out
        let accounting = compute_pool_accounting(1000, 0, 0, 10_000, &config);
        assert_eq!(accounting.payout, 800);
        assert_eq!(accounting.carryover_after(accounting.payout), 0);
    }

    // Drive the cumulative carryover past u64::MAX by feeding sustained
    // per-block fee inflow that exceeds the anti-grinding payout cap. The
    // carryover must accumulate EXACTLY in u128 with no saturation or wrap,
    // and the payout cap must still hold at that scale.
    //
    // Per block the maximum fee inflow is one block's `total_fees` (a u64);
    // its 80% pool share is `0.8 * u64::MAX`, so a single block cannot push
    // the carryover past u64::MAX, but a handful of sustained max-fee blocks
    // do — exactly the regime where a u64 carryover would have saturated.
    #[test]
    fn test_pool_carryover_accumulates_past_u64_max_without_saturation() {
        let config = LotteryFeeConfig::default();

        // Largest possible single-block fee inflow.
        let total_fees: u64 = u64::MAX;
        let (fee_pool, _) = config.split_fees(total_fees);
        let emission_share: u64 = 0;
        let reward: u64 = 5_000_000; // payout cap = one block reward

        // Net per-block growth of the carryover: inflow - payout cap. With a
        // max-fee block this is ~0.8 * u64::MAX, so just a few blocks cross
        // u64::MAX (and overflow a u64, which u128 must absorb exactly).
        let inflow_per_block = fee_pool as u128 + emission_share as u128;
        assert!(
            inflow_per_block > reward as u128,
            "test requires inflow above the payout cap so the pool grows"
        );

        // Reference carryover computed independently in u128.
        let mut expected: u128 = 0;
        let mut pool: u128 = 0;
        let blocks = 4; // 4 * (0.8 * u64::MAX) far exceeds u64::MAX
        for _ in 0..blocks {
            let accounting =
                compute_pool_accounting(total_fees, emission_share, pool, reward, &config);

            // Anti-grinding cap preserved: payout == min(available, reward).
            assert_eq!(
                accounting.payout as u128,
                accounting.available.min(reward as u128),
                "payout must equal min(available, reward) at all scales"
            );
            // The pool always dwarfs the reward here, so the cap binds exactly.
            assert_eq!(
                accounting.payout, reward,
                "payout must be capped at one block reward"
            );

            // Independent u128 reference: available = pool + emission + fee_pool,
            // carryover = available - capped payout. No saturation expected.
            let available_ref = pool + emission_share as u128 + fee_pool as u128;
            assert_eq!(
                accounting.available, available_ref,
                "available must accumulate exactly in u128 (no saturation)"
            );
            expected = available_ref - reward as u128;

            pool = accounting.carryover_after(accounting.payout);
            assert_eq!(
                pool, expected,
                "carryover must accumulate exactly with no saturation/wrap"
            );
        }

        // The pool has grown past what u64 could represent — proving the
        // widening prevents the u64::MAX saturation this fix targets.
        assert!(
            pool > u64::MAX as u128,
            "carryover ({pool}) should exceed u64::MAX ({})",
            u64::MAX
        );
    }

    #[test]
    fn test_draw_lottery_no_candidates() {
        let config = LotteryFeeConfig::default();
        let prev_hash = [0u8; 32];

        let accounting = compute_pool_accounting(1000, 0, 0, 1000, &config);
        let result = draw_lottery_winners(&[], 1000, &accounting, 100, &prev_hash, &config);

        assert!(!result.has_winners());
        // Fee burn share only; the pool share carries over
        assert_eq!(result.burn_amount, 200);
        assert_eq!(result.pool_amount, 0);
    }

    #[test]
    fn test_draw_lottery_with_candidates() {
        let config = LotteryFeeConfig {
            draw_config: LotteryDrawConfig {
                min_utxo_age: 10,
                min_utxo_value: 100,
                winners_per_draw: 2,
                ..Default::default()
            },
            ..Default::default()
        };
        let prev_hash = [42u8; 32];

        // Create eligible candidates (old enough)
        let candidates = vec![
            make_candidate(1, 10_000, 0),
            make_candidate(2, 20_000, 0),
            make_candidate(3, 30_000, 0),
        ];

        let accounting = compute_pool_accounting(1000, 0, 0, 1000, &config);
        let result = draw_lottery_winners(&candidates, 1000, &accounting, 100, &prev_hash, &config);

        assert!(result.has_winners());
        assert_eq!(result.winners.len(), 2);
        assert_eq!(result.pool_amount, 800);
        assert_eq!(result.burn_amount, 200);
        assert_eq!(result.total_distributed(), 800);
    }

    #[test]
    fn test_verify_lottery_result() {
        let config = LotteryFeeConfig {
            draw_config: LotteryDrawConfig {
                min_utxo_age: 10,
                min_utxo_value: 100,
                winners_per_draw: 2,
                ..Default::default()
            },
            ..Default::default()
        };
        let prev_hash = [42u8; 32];

        let candidates = vec![
            make_candidate(1, 10_000, 0),
            make_candidate(2, 20_000, 0),
            make_candidate(3, 30_000, 0),
        ];

        let accounting = compute_pool_accounting(1000, 0, 0, 1000, &config);
        let result = draw_lottery_winners(&candidates, 1000, &accounting, 100, &prev_hash, &config);

        // Verification should pass with same parameters
        assert!(verify_lottery_result(
            &candidates,
            &result,
            &accounting,
            &prev_hash,
            &config
        ));

        // Verification should fail with different hash
        let wrong_hash = [0u8; 32];
        assert!(!verify_lottery_result(
            &candidates,
            &result,
            &accounting,
            &wrong_hash,
            &config
        ));
    }

    #[test]
    fn test_lottery_stats() {
        let mut stats = LotteryStats::new();

        // Record a result with winners
        let result1 = BlockLotteryResult {
            block_height: 100,
            total_fees: 1000,
            pool_amount: 800,
            burn_amount: 200,
            winners: vec![
                LotteryWinner {
                    utxo_id: [1u8; 36],
                    payout: 400,
                },
                LotteryWinner {
                    utxo_id: [2u8; 36],
                    payout: 400,
                },
            ],
            seed: [0u8; 32],
        };
        stats.record_result(&result1);

        assert_eq!(stats.total_fees_processed, 1000);
        assert_eq!(stats.total_distributed, 800);
        assert_eq!(stats.total_burned, 200);
        assert_eq!(stats.total_drawings, 1);
        assert_eq!(stats.total_winners, 2);
        assert_eq!(stats.burn_rate_permille(), 200); // 20%
        assert_eq!(stats.avg_payout(), 400);
    }

    // ========================================================================
    // Tests for validate_block_lottery
    // ========================================================================

    use crate::block::{Block, BlockHeader, BlockLotterySummary, LotteryOutput, MintingTx};

    fn create_test_block(
        height: u64,
        prev_hash: [u8; 32],
        total_fees: u64,
        lottery_summary: BlockLotterySummary,
        lottery_outputs: Vec<LotteryOutput>,
    ) -> Block {
        let header = BlockHeader {
            version: 1,
            prev_block_hash: prev_hash,
            tx_root: [0u8; 32],
            timestamp: 0,
            height,
            difficulty: u64::MAX,
            nonce: 0,
            minter_view_key: [0u8; 32],
            minter_spend_key: [0u8; 32],
        };

        let minting_tx = MintingTx {
            block_height: height,
            reward: 1000,
            minter_view_key: [0u8; 32],
            minter_spend_key: [0u8; 32],
            target_key: [0u8; 32],
            public_key: [0u8; 32],
            prev_block_hash: prev_hash,
            difficulty: u64::MAX,
            nonce: 0,
            timestamp: 0,
        };

        // Create transactions that sum to total_fees
        let transactions = if total_fees > 0 {
            vec![crate::transaction::Transaction::new_stub_with_fee(
                total_fees,
            )]
        } else {
            vec![]
        };

        Block {
            header,
            minting_tx,
            transactions,
            lottery_outputs,
            lottery_summary,
        }
    }

    #[test]
    fn test_validate_block_lottery_no_fees_no_winners() {
        let config = LotteryFeeConfig::default();
        let prev_hash = [0u8; 32];
        let candidates: Vec<LotteryCandidate> = vec![];

        let lottery_summary = BlockLotterySummary {
            total_fees: 0,
            pool_distributed: 0,
            amount_burned: 0,
            lottery_seed: [0u8; 32],
        };

        let block = create_test_block(100, prev_hash, 0, lottery_summary, vec![]);

        let result = validate_block_lottery(&block, &candidates, 0, &prev_hash, &config);
        assert!(result.is_ok(), "No fees/winners should pass validation");
        assert_eq!(result.unwrap(), 0, "Pool should stay empty");
    }

    #[test]
    fn test_validate_block_lottery_invalid_fee_split() {
        let config = LotteryFeeConfig::default();
        let prev_hash = [0u8; 32];

        // Create candidates that are eligible (old enough)
        let candidates = vec![make_candidate(1, 10_000, 0), make_candidate(2, 20_000, 0)];

        // Create a lottery summary with incorrect split
        // Total fees = 1000, so expected: pool=800, burn=200
        // But we'll set wrong values
        let lottery_summary = BlockLotterySummary {
            total_fees: 1000,
            pool_distributed: 900, // Wrong! Should be 800
            amount_burned: 100,    // Wrong! Should be 200
            lottery_seed: [0u8; 32],
        };

        // Need at least one lottery output for the fee split to be checked
        let lottery_outputs = vec![LotteryOutput {
            winner_tx_hash: [0u8; 32],
            winner_output_index: 0,
            payout: 900,
            target_key: [0u8; 32],
            public_key: [0u8; 32],
        }];

        let block = create_test_block(100, prev_hash, 1000, lottery_summary, lottery_outputs);

        let result = validate_block_lottery(&block, &candidates, 0, &prev_hash, &config);
        assert!(
            matches!(result, Err(LotteryValidationError::InvalidFeeSplit { .. })),
            "Invalid fee split should fail: {:?}",
            result
        );
    }

    #[test]
    fn test_validate_block_lottery_payout_mismatch() {
        let config = LotteryFeeConfig {
            draw_config: LotteryDrawConfig {
                min_utxo_age: 10,
                min_utxo_value: 100,
                winners_per_draw: 1,
                ..Default::default()
            },
            ..Default::default()
        };
        let prev_hash = [42u8; 32];

        let candidates = vec![make_candidate(1, 10_000, 0), make_candidate(2, 20_000, 0)];

        // First draw to get a valid result
        let accounting = compute_pool_accounting(1000, 0, 0, 1000, &config);
        let valid_result =
            draw_lottery_winners(&candidates, 1000, &accounting, 100, &prev_hash, &config);

        // Create a lottery summary with correct split but wrong payout in output
        let lottery_summary = BlockLotterySummary {
            total_fees: 1000,
            pool_distributed: 800,
            amount_burned: 200,
            lottery_seed: valid_result.seed,
        };

        // Create output with wrong payout (not equal to pool_distributed)
        let winner_utxo_id = if !valid_result.winners.is_empty() {
            valid_result.winners[0].utxo_id
        } else {
            [1u8; 36]
        };

        let lottery_outputs = vec![LotteryOutput {
            winner_tx_hash: {
                let mut h = [0u8; 32];
                h.copy_from_slice(&winner_utxo_id[..32]);
                h
            },
            winner_output_index: u32::from_le_bytes(winner_utxo_id[32..36].try_into().unwrap()),
            payout: 700, // Wrong! Should be 800
            target_key: [0u8; 32],
            public_key: [0u8; 32],
        }];

        let block = create_test_block(100, prev_hash, 1000, lottery_summary, lottery_outputs);

        let result = validate_block_lottery(&block, &candidates, 0, &prev_hash, &config);
        // Should fail either with PayoutMismatch or InvalidDrawing depending on
        // validation order
        assert!(result.is_err(), "Payout mismatch should fail validation");
    }

    #[test]
    fn test_validate_block_lottery_winner_not_eligible() {
        let config = LotteryFeeConfig::default();
        let prev_hash = [0u8; 32];

        // Empty candidates - no UTXOs are eligible
        let candidates: Vec<LotteryCandidate> = vec![];

        // But claim a winner that doesn't exist
        let lottery_summary = BlockLotterySummary {
            total_fees: 1000,
            pool_distributed: 800,
            amount_burned: 200,
            lottery_seed: [0u8; 32],
        };

        let lottery_outputs = vec![LotteryOutput {
            winner_tx_hash: [0xFFu8; 32], // Non-existent UTXO
            winner_output_index: 0,
            payout: 800,
            target_key: [0u8; 32],
            public_key: [0u8; 32],
        }];

        let block = create_test_block(100, prev_hash, 1000, lottery_summary, lottery_outputs);

        let result = validate_block_lottery(&block, &candidates, 0, &prev_hash, &config);
        // Should fail because the winner is not in the (empty) candidate list
        assert!(
            result.is_err(),
            "Winner not in candidates should fail validation"
        );
    }

    #[test]
    fn test_validate_block_lottery_no_winners_pool_carries() {
        let config = LotteryFeeConfig::default();
        let prev_hash = [0u8; 32];

        // No eligible candidates
        let candidates: Vec<LotteryCandidate> = vec![];

        // When no winners, only the fee burn share is burned; the pool
        // share (800) carries over to the next block.
        let lottery_summary = BlockLotterySummary {
            total_fees: 1000,
            pool_distributed: 0,
            amount_burned: 200,
            lottery_seed: [0u8; 32],
        };

        let block = create_test_block(100, prev_hash, 1000, lottery_summary, vec![]);

        let result = validate_block_lottery(&block, &candidates, 0, &prev_hash, &config);
        assert!(result.is_ok(), "No winners should pass: {:?}", result);
        assert_eq!(result.unwrap(), 800, "Fee pool share should carry over");
    }

    #[test]
    fn test_validate_block_lottery_rejects_suppressed_draw() {
        // A producer must not be able to claim "no winners" when the
        // deterministic draw would have produced winners (payout
        // suppression: carry the pool over and harvest it later when the
        // predictable draw favors the producer).
        let config = LotteryFeeConfig::default();
        let prev_hash = [42u8; 32];

        // Eligible candidates exist (default min age 720, min value 1 microBTH)
        let candidates = vec![
            make_candidate(1, 10_000_000, 0),
            make_candidate(2, 20_000_000, 0),
        ];

        // Correct summary numbers for a no-winner block...
        let lottery_summary = BlockLotterySummary {
            total_fees: 1000,
            pool_distributed: 0,
            amount_burned: 200,
            lottery_seed: [0u8; 32],
        };

        // ...but the draw would have produced winners, so this block is invalid.
        let block = create_test_block(10_000, prev_hash, 1000, lottery_summary, vec![]);

        let result = validate_block_lottery(&block, &candidates, 0, &prev_hash, &config);
        assert!(
            matches!(result, Err(LotteryValidationError::InvalidDrawing)),
            "Suppressed draw must be rejected: {:?}",
            result
        );
    }

    #[test]
    fn test_validate_block_lottery_rejects_burn_all_when_no_winners() {
        // The pre-carryover behavior (burning the pool share when there are
        // no winners) must now be rejected.
        let config = LotteryFeeConfig::default();
        let prev_hash = [0u8; 32];
        let candidates: Vec<LotteryCandidate> = vec![];

        let lottery_summary = BlockLotterySummary {
            total_fees: 1000,
            pool_distributed: 0,
            amount_burned: 1000, // Old semantics: burn everything
            lottery_seed: [0u8; 32],
        };

        let block = create_test_block(100, prev_hash, 1000, lottery_summary, vec![]);

        let result = validate_block_lottery(&block, &candidates, 0, &prev_hash, &config);
        assert!(
            matches!(result, Err(LotteryValidationError::InvalidFeeSplit { .. })),
            "Burning the pool share must be rejected: {:?}",
            result
        );
    }

    // ------------------------------------------------------------------
    // Fuzz-harness wiring sanity checks (issue #337).
    //
    // These are NOT a substitute for the libfuzzer runs (CI-deferred:
    // cargo-fuzz cannot run on the macOS dev host). They only confirm that
    // the core logic the fuzz targets drive — `validate_block_lottery` and
    // the cluster-tax monetary primitives — behaves as the harnesses assert
    // on a valid input and a malformed input, so an obvious harness/API
    // wiring bug is caught at `cargo test` time.
    // ------------------------------------------------------------------

    #[test]
    fn fuzz_wiring_lottery_validation_deterministic_and_split_checked() {
        // Mirrors fuzz_lottery_validation: same input → same result, and an
        // Ok result implies the 80/20 split matches compute_pool_accounting.
        let config = LotteryFeeConfig::default();
        let prev_hash = [9u8; 32];

        // --- Valid (no-winners) block: burn share only, pool carries over.
        let total_fees = 1000u64;
        let (_pool, expected_burn) = config.split_fees(total_fees);
        let valid_summary = BlockLotterySummary {
            total_fees,
            pool_distributed: 0,
            amount_burned: expected_burn,
            lottery_seed: [0u8; 32],
        };
        let valid_block = create_test_block(100, prev_hash, total_fees, valid_summary, vec![]);
        let candidates: Vec<LotteryCandidate> = vec![];

        let r1 = validate_block_lottery(&valid_block, &candidates, 0, &prev_hash, &config);
        let r2 = validate_block_lottery(&valid_block, &candidates, 0, &prev_hash, &config);
        assert_eq!(r1, r2, "validation must be deterministic");

        if r1.is_ok() {
            let emission_share = valid_block.minting_tx.lottery_emission_share();
            let accounting = compute_pool_accounting(
                valid_block.total_fees(),
                emission_share,
                0,
                valid_block.minting_tx.reward,
                &config,
            );
            assert_eq!(
                valid_block.lottery_summary.amount_burned,
                accounting.fee_burn
            );
            // No-winner block: the pool share carries over (returned new pool),
            // so the summary's pool_distributed is 0 even though payout > 0.
            if valid_block.lottery_outputs.is_empty() {
                assert_eq!(valid_block.lottery_summary.pool_distributed, 0);
            } else {
                assert_eq!(
                    valid_block.lottery_summary.pool_distributed,
                    accounting.payout
                );
            }
        }

        // --- Malformed block: claims a wrong burn share → must be rejected,
        // and rejection must also be deterministic.
        let bad_summary = BlockLotterySummary {
            total_fees,
            pool_distributed: 0,
            amount_burned: expected_burn + 1, // off by one: wrong split
            lottery_seed: [0u8; 32],
        };
        let bad_block = create_test_block(100, prev_hash, total_fees, bad_summary, vec![]);
        let b1 = validate_block_lottery(&bad_block, &candidates, 0, &prev_hash, &config);
        let b2 = validate_block_lottery(&bad_block, &candidates, 0, &prev_hash, &config);
        assert!(b1.is_err(), "wrong split must be rejected");
        assert_eq!(b1, b2, "rejection must be deterministic");
    }

    #[test]
    fn fuzz_wiring_cluster_tax_math_bounds() {
        // Mirrors fuzz_cluster_tax_math: cluster factor in documented bounds,
        // demurrage exempt for factor-1, emission share <= reward, and the
        // monetary primitives never panic on extreme inputs.
        use bth_cluster_tax::{
            demurrage::{FACTOR_SCALE, MAX_FACTOR_SCALED},
            demurrage_charge, ClusterFactorCurve, MonetaryPolicy,
        };

        let curve = ClusterFactorCurve::default_params();
        // Valid mid-range wealth and the u64 extremes all stay in [1000, 6000].
        for w in [0u64, 10_000_000, u64::MAX] {
            let f = curve.factor(w as u128);
            assert!(
                (FACTOR_SCALE..=MAX_FACTOR_SCALED).contains(&f),
                "factor {} out of [{}, {}] for wealth {}",
                f,
                FACTOR_SCALE,
                MAX_FACTOR_SCALED,
                w
            );
        }

        // Factor-1 coins are exempt (valid "no charge" case).
        assert_eq!(
            demurrage_charge(u64::MAX, FACTOR_SCALE, 1_000_000, 200, 6_307_200),
            0
        );
        // Max factor over one year ≈ rate_bps of value (malformed-extreme value
        // must not panic / overflow): bounded by the transfer value.
        let bpy = 6_307_200u64;
        let charge = demurrage_charge(1_000_000, MAX_FACTOR_SCALED, bpy, 200, bpy);
        assert!(charge <= 1_000_000, "in-horizon charge exceeds value");

        // Emission share can never exceed the reward, even at extreme height.
        let policy = MonetaryPolicy::default();
        for (h, r) in [(0u64, 0u64), (1, u64::MAX), (u64::MAX, 50_000_000_000)] {
            assert!(policy.lottery_emission_share(h, r) <= r);
            // Tail reward + block reward must not panic for extreme supply.
            let _ = policy.calculate_tail_reward(r);
            let _ = crate::block::calculate_block_reward(h, r as u128);
        }
    }
}
