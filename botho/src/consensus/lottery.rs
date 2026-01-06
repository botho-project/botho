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
/// * `candidates` - Eligible UTXOs from the UTXO set (must be at state before block)
/// * `prev_block_hash` - Hash of the previous block (for verifiable randomness)
/// * `config` - Lottery configuration
///
/// # Returns
/// `Ok(())` if validation passes, `Err(LotteryValidationError)` otherwise.
pub fn validate_block_lottery(
    block: &crate::block::Block,
    candidates: &[LotteryCandidate],
    prev_block_hash: &[u8; 32],
    config: &LotteryFeeConfig,
) -> Result<(), LotteryValidationError> {
    let total_fees = block.total_fees();

    // 1. Verify fee split is correct
    let (expected_pool, expected_burn) = config.split_fees(total_fees);

    // Handle no-winners case
    if block.lottery_outputs.is_empty() {
        // When there are no winners, all fees should be burned
        let expected_total_burn = total_fees;
        if block.lottery_summary.amount_burned != expected_total_burn {
            return Err(LotteryValidationError::InvalidFeeSplit {
                expected_pool: 0,
                expected_burn: expected_total_burn,
                actual_pool: block.lottery_summary.pool_distributed,
                actual_burn: block.lottery_summary.amount_burned,
            });
        }
        // No further validation needed for no-winners case
        return Ok(());
    }

    // Verify the fee split in the summary
    if block.lottery_summary.pool_distributed != expected_pool
        || block.lottery_summary.amount_burned != expected_burn
    {
        return Err(LotteryValidationError::InvalidFeeSplit {
            expected_pool,
            expected_burn,
            actual_pool: block.lottery_summary.pool_distributed,
            actual_burn: block.lottery_summary.amount_burned,
        });
    }

    // 2. Reconstruct BlockLotteryResult from block data for verification
    let block_result = BlockLotteryResult {
        block_height: block.height(),
        total_fees,
        pool_amount: expected_pool,
        burn_amount: expected_burn,
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
    if !verify_lottery_result(candidates, &block_result, prev_block_hash, config) {
        return Err(LotteryValidationError::InvalidDrawing);
    }

    // 4. Verify total payouts match pool amount
    let total_payouts: u64 = block.lottery_outputs.iter().map(|o| o.payout).sum();
    if total_payouts != expected_pool {
        return Err(LotteryValidationError::PayoutMismatch {
            expected: expected_pool,
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
        pool_amount = expected_pool,
        burn_amount = expected_burn,
        "Lottery validation passed"
    );

    Ok(())
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
            vec![crate::transaction::Transaction::new_stub_with_fee(total_fees)]
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

        let result = validate_block_lottery(&block, &candidates, &prev_hash, &config);
        assert!(result.is_ok(), "No fees/winners should pass validation");
    }

    #[test]
    fn test_validate_block_lottery_invalid_fee_split() {
        let config = LotteryFeeConfig::default();
        let prev_hash = [0u8; 32];

        // Create candidates that are eligible (old enough)
        let candidates = vec![
            make_candidate(1, 10_000, 0),
            make_candidate(2, 20_000, 0),
        ];

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

        let result = validate_block_lottery(&block, &candidates, &prev_hash, &config);
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

        let candidates = vec![
            make_candidate(1, 10_000, 0),
            make_candidate(2, 20_000, 0),
        ];

        // First draw to get a valid result
        let valid_result = draw_lottery_winners(&candidates, 1000, 100, &prev_hash, &config);

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

        let result = validate_block_lottery(&block, &candidates, &prev_hash, &config);
        // Should fail either with PayoutMismatch or InvalidDrawing depending on validation order
        assert!(
            result.is_err(),
            "Payout mismatch should fail validation"
        );
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

        let result = validate_block_lottery(&block, &candidates, &prev_hash, &config);
        // Should fail because the winner is not in the (empty) candidate list
        assert!(
            result.is_err(),
            "Winner not in candidates should fail validation"
        );
    }

    #[test]
    fn test_validate_block_lottery_no_winners_all_burned() {
        let config = LotteryFeeConfig::default();
        let prev_hash = [0u8; 32];

        // No eligible candidates
        let candidates: Vec<LotteryCandidate> = vec![];

        // When no winners, all fees should be burned
        let lottery_summary = BlockLotterySummary {
            total_fees: 1000,
            pool_distributed: 0,
            amount_burned: 1000, // All burned when no winners
            lottery_seed: [0u8; 32],
        };

        let block = create_test_block(100, prev_hash, 1000, lottery_summary, vec![]);

        let result = validate_block_lottery(&block, &candidates, &prev_hash, &config);
        assert!(result.is_ok(), "No winners, all burned should pass: {:?}", result);
    }
}
