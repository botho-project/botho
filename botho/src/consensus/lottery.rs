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
            draw_config: LotteryDrawConfig::default(),
        }
    }
}

impl LotteryFeeConfig {
    /// Split fees into lottery pool and burn amounts.
    ///
    /// Returns (pool_amount, burn_amount).
    pub fn split_fees(&self, total_fees: u64) -> (u64, u64) {
        let pool_amount =
            (total_fees as u128 * self.pool_fraction_permille as u128 / 1000) as u64;
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
    /// Create a result when there are no eligible candidates.
    ///
    /// In this case, all fees go to the burn (deflationary).
    pub fn no_winners(block_height: u64, total_fees: u64, config: &LotteryFeeConfig) -> Self {
        let (pool_amount, burn_amount) = config.split_fees(total_fees);

        Self {
            block_height,
            total_fees,
            pool_amount,
            // If no winners, pool amount is also burned
            burn_amount: burn_amount + pool_amount,
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

/// Draw lottery winners for a block.
///
/// # Arguments
/// * `candidates` - Eligible UTXOs from the UTXO set
/// * `total_fees` - Total fees collected from block transactions
/// * `block_height` - Current block height
/// * `prev_block_hash` - Hash of previous block (for verifiable randomness)
/// * `config` - Lottery configuration
///
/// # Returns
/// `BlockLotteryResult` with winners and fee allocation
pub fn draw_lottery_winners(
    candidates: &[LotteryCandidate],
    total_fees: u64,
    block_height: u64,
    prev_block_hash: &[u8; 32],
    config: &LotteryFeeConfig,
) -> BlockLotteryResult {
    let (pool_amount, burn_amount) = config.split_fees(total_fees);

    if pool_amount == 0 {
        debug!(
            block_height = block_height,
            "No fees to distribute, skipping lottery"
        );
        return BlockLotteryResult::no_winners(block_height, total_fees, config);
    }

    // Draw winners using cluster-tax lottery implementation
    match draw_winners(
        candidates,
        pool_amount,
        block_height,
        prev_block_hash,
        &config.draw_config,
    ) {
        Some(result) => {
            info!(
                block_height = block_height,
                winners = result.winners.len(),
                pool_amount = pool_amount,
                burn_amount = burn_amount,
                "Lottery draw complete"
            );

            BlockLotteryResult {
                block_height,
                total_fees,
                pool_amount,
                burn_amount,
                winners: result.winners,
                seed: result.seed,
            }
        }
        None => {
            debug!(
                block_height = block_height,
                "No eligible lottery candidates"
            );
            BlockLotteryResult::no_winners(block_height, total_fees, config)
        }
    }
}

/// Verify a lottery drawing result.
///
/// Re-runs the drawing with the same parameters and verifies the result matches.
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
    prev_block_hash: &[u8; 32],
    config: &LotteryFeeConfig,
) -> bool {
    // Verify fee splitting is correct
    let (expected_pool, expected_burn) = config.split_fees(result.total_fees);

    // If no winners, all pool goes to burn
    let actual_burn = if result.winners.is_empty() {
        result.burn_amount
    } else {
        result.burn_amount
    };

    if result.winners.is_empty() {
        // No winners: pool should be added to burn
        if result.burn_amount != expected_pool + expected_burn {
            return false;
        }
        return true;
    }

    if result.pool_amount != expected_pool || actual_burn != expected_burn {
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
    LotteryCandidate::new(
        utxo_id,
        value,
        cluster_factor as f64 / 1000.0,
        tags,
        creation_block,
    )
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

#[cfg(test)]
mod tests {
    use super::*;
    use bth_cluster_tax::TagVector;

    fn make_candidate(id: u8, value: u64, creation_block: u64) -> LotteryCandidate {
        let mut utxo_id = [0u8; 36];
        utxo_id[0] = id;
        let empty_tags = TagVector::new();
        LotteryCandidate::new(utxo_id, value, 1.0, &empty_tags, creation_block)
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
        let result = BlockLotteryResult::no_winners(100, 1000, &config);

        assert_eq!(result.block_height, 100);
        assert_eq!(result.total_fees, 1000);
        assert_eq!(result.winners.len(), 0);
        // All fees should be burned when no winners
        assert_eq!(result.burn_amount, 1000);
        assert_eq!(result.total_distributed(), 0);
    }

    #[test]
    fn test_draw_lottery_no_candidates() {
        let config = LotteryFeeConfig::default();
        let prev_hash = [0u8; 32];

        let result = draw_lottery_winners(&[], 1000, 100, &prev_hash, &config);

        assert!(!result.has_winners());
        assert_eq!(result.burn_amount, 1000);
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

        let result = draw_lottery_winners(&candidates, 1000, 100, &prev_hash, &config);

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

        let result = draw_lottery_winners(&candidates, 1000, 100, &prev_hash, &config);

        // Verification should pass with same parameters
        assert!(verify_lottery_result(
            &candidates,
            &result,
            &prev_hash,
            &config
        ));

        // Verification should fail with different hash
        let wrong_hash = [0u8; 32];
        assert!(!verify_lottery_result(
            &candidates,
            &result,
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
}
