// Copyright (c) 2024 Cadence Foundation

//! Difficulty adjustment algorithm for Cadence PoW mining.
//!
//! This module implements a difficulty adjustment algorithm that targets
//! a specific emission rate of mining transactions per block. The difficulty
//! adjusts smoothly based on the observed rate of mining transactions
//! over a sliding window of recent blocks.

use mc_transaction_core::DifficultyTarget;

/// Target number of mining transactions per block.
///
/// The difficulty will adjust to try to achieve this rate.
/// With 5 mining txs/block and ~2 minute block times, this gives
/// roughly 10 minutes per mining reward (similar to Bitcoin's block time).
pub const TARGET_MINING_TXS_PER_BLOCK: u64 = 5;

/// Number of blocks to average over for difficulty calculation.
///
/// Using 720 blocks (~24 hours with 2-minute blocks) provides
/// enough data for stable difficulty adjustment while still
/// responding to significant hashrate changes.
pub const DIFFICULTY_WINDOW: u64 = 720;

/// Lag blocks before applying new difficulty.
///
/// This provides stability by ensuring recent blocks don't
/// immediately affect difficulty calculation.
pub const DIFFICULTY_LAG: u64 = 15;

/// Maximum difficulty adjustment factor per window (2x up or 0.5x down).
///
/// This prevents extreme difficulty swings from attacks or
/// sudden hashrate changes.
pub const MAX_ADJUSTMENT_FACTOR: u64 = 2;

/// Minimum difficulty value to prevent the network from becoming trivial.
pub const MIN_DIFFICULTY: u128 = 1_000_000;

/// Initial difficulty for the genesis block.
/// This should be tuned based on expected initial network hashrate.
pub const INITIAL_DIFFICULTY: u128 = 100_000_000;

/// Information about recent blocks needed for difficulty calculation.
#[derive(Debug, Clone)]
pub struct BlockDifficultyInfo {
    /// Number of mining transactions in this block.
    pub mining_tx_count: u64,
    /// The difficulty target for this block.
    pub difficulty: DifficultyTarget,
    /// Block timestamp (Unix seconds).
    pub timestamp: u64,
}

/// Calculate the next difficulty target based on recent block history.
///
/// The algorithm works as follows:
/// 1. Count total mining transactions in the window
/// 2. Calculate the expected number based on target rate
/// 3. Adjust difficulty proportionally to match target rate
/// 4. Apply dampening and limits to prevent extreme swings
///
/// # Arguments
/// * `recent_blocks` - Block info for the most recent DIFFICULTY_WINDOW blocks
/// * `current_difficulty` - The current difficulty target
///
/// # Returns
/// The new difficulty target for the next block
pub fn next_difficulty(
    recent_blocks: &[BlockDifficultyInfo],
    current_difficulty: DifficultyTarget,
) -> DifficultyTarget {
    // If we don't have enough blocks, use current difficulty
    if recent_blocks.len() < DIFFICULTY_LAG as usize {
        return current_difficulty;
    }

    // Use blocks excluding the most recent LAG blocks for stability
    let window_end = recent_blocks.len().saturating_sub(DIFFICULTY_LAG as usize);
    let window_start = window_end.saturating_sub(DIFFICULTY_WINDOW as usize);
    let window = &recent_blocks[window_start..window_end];

    if window.is_empty() {
        return current_difficulty;
    }

    // Count total mining transactions in the window
    let total_mining_txs: u64 = window.iter().map(|b| b.mining_tx_count).sum();
    let window_blocks = window.len() as u64;

    // Calculate expected mining txs based on target rate
    let expected_mining_txs = window_blocks * TARGET_MINING_TXS_PER_BLOCK;

    // Avoid division by zero
    if total_mining_txs == 0 {
        // No mining txs found, decrease difficulty significantly
        let new_value = current_difficulty.value() / MAX_ADJUSTMENT_FACTOR as u128;
        return DifficultyTarget::new(new_value.max(MIN_DIFFICULTY));
    }

    // Calculate adjustment ratio
    // If we have more mining txs than expected, increase difficulty
    // If we have fewer mining txs than expected, decrease difficulty
    let current_value = current_difficulty.value();

    // Use u128 to avoid overflow during calculation
    let new_value = if total_mining_txs > expected_mining_txs {
        // Too many mining txs - increase difficulty
        let ratio = total_mining_txs as u128 * 1000 / expected_mining_txs as u128;
        let adjustment = current_value.saturating_mul(ratio) / 1000;

        // Limit the increase to MAX_ADJUSTMENT_FACTOR
        adjustment.min(current_value.saturating_mul(MAX_ADJUSTMENT_FACTOR as u128))
    } else {
        // Too few mining txs - decrease difficulty
        let ratio = expected_mining_txs as u128 * 1000 / total_mining_txs as u128;
        let adjustment = current_value.saturating_mul(1000) / ratio;

        // Limit the decrease to 1/MAX_ADJUSTMENT_FACTOR
        adjustment.max(current_value / MAX_ADJUSTMENT_FACTOR as u128)
    };

    // Apply minimum difficulty floor
    DifficultyTarget::new(new_value.max(MIN_DIFFICULTY))
}

/// Calculate difficulty for the genesis block.
pub fn genesis_difficulty() -> DifficultyTarget {
    DifficultyTarget::new(INITIAL_DIFFICULTY)
}

/// Calculate time-weighted difficulty adjustment.
///
/// This variant also considers block timestamps to account for
/// blocks that were mined faster or slower than expected.
///
/// # Arguments
/// * `recent_blocks` - Block info for recent blocks
/// * `current_difficulty` - The current difficulty target
/// * `target_block_time_secs` - Target time between blocks in seconds
///
/// # Returns
/// The new difficulty target for the next block
pub fn next_difficulty_with_timestamps(
    recent_blocks: &[BlockDifficultyInfo],
    current_difficulty: DifficultyTarget,
    target_block_time_secs: u64,
) -> DifficultyTarget {
    if recent_blocks.len() < 2 {
        return current_difficulty;
    }

    // Get the basic difficulty adjustment
    let base_difficulty = next_difficulty(recent_blocks, current_difficulty);

    // Calculate actual vs expected time for the window
    let window_end = recent_blocks.len().saturating_sub(DIFFICULTY_LAG as usize);
    let window_start = window_end.saturating_sub(DIFFICULTY_WINDOW as usize);

    if window_start >= window_end || window_end > recent_blocks.len() {
        return base_difficulty;
    }

    let first_block = &recent_blocks[window_start];
    let last_block = &recent_blocks[window_end.saturating_sub(1)];

    let actual_time = last_block.timestamp.saturating_sub(first_block.timestamp);
    let expected_time = (window_end - window_start) as u64 * target_block_time_secs;

    if expected_time == 0 || actual_time == 0 {
        return base_difficulty;
    }

    // Adjust based on time ratio
    let base_value = base_difficulty.value();
    let time_adjusted = if actual_time < expected_time {
        // Blocks were mined faster than expected, increase difficulty
        let ratio = expected_time as u128 * 1000 / actual_time as u128;
        base_value.saturating_mul(ratio) / 1000
    } else {
        // Blocks were mined slower than expected, decrease difficulty
        let ratio = actual_time as u128 * 1000 / expected_time as u128;
        base_value.saturating_mul(1000) / ratio
    };

    // Apply limits
    let limited = time_adjusted
        .min(current_difficulty.value().saturating_mul(MAX_ADJUSTMENT_FACTOR as u128))
        .max(current_difficulty.value() / MAX_ADJUSTMENT_FACTOR as u128)
        .max(MIN_DIFFICULTY);

    DifficultyTarget::new(limited)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_block_info(mining_tx_count: u64, difficulty: u128, timestamp: u64) -> BlockDifficultyInfo {
        BlockDifficultyInfo {
            mining_tx_count,
            difficulty: DifficultyTarget::new(difficulty),
            timestamp,
        }
    }

    #[test]
    fn test_genesis_difficulty() {
        let diff = genesis_difficulty();
        assert_eq!(diff.value(), INITIAL_DIFFICULTY);
    }

    #[test]
    fn test_insufficient_blocks_returns_current() {
        let current = DifficultyTarget::new(1_000_000_000);
        let blocks = vec![make_block_info(5, 1_000_000_000, 0)];
        let next = next_difficulty(&blocks, current);
        assert_eq!(next.value(), current.value());
    }

    #[test]
    fn test_exact_target_maintains_difficulty() {
        let current = DifficultyTarget::new(1_000_000_000);

        // Create window with exactly TARGET_MINING_TXS_PER_BLOCK per block
        let blocks: Vec<_> = (0..(DIFFICULTY_WINDOW + DIFFICULTY_LAG + 10) as u64)
            .map(|i| make_block_info(TARGET_MINING_TXS_PER_BLOCK, 1_000_000_000, i * 120))
            .collect();

        let next = next_difficulty(&blocks, current);

        // Should be close to current difficulty (within rounding)
        let ratio = next.value() as f64 / current.value() as f64;
        assert!((ratio - 1.0).abs() < 0.01, "ratio: {}", ratio);
    }

    #[test]
    fn test_high_mining_rate_increases_difficulty() {
        let current = DifficultyTarget::new(1_000_000_000);

        // Create window with 2x the target mining rate
        let high_rate = TARGET_MINING_TXS_PER_BLOCK * 2;
        let blocks: Vec<_> = (0..(DIFFICULTY_WINDOW + DIFFICULTY_LAG + 10) as u64)
            .map(|i| make_block_info(high_rate, 1_000_000_000, i * 120))
            .collect();

        let next = next_difficulty(&blocks, current);

        // Difficulty should increase (but capped at 2x)
        assert!(next.value() > current.value());
        assert!(next.value() <= current.value() * 2);
    }

    #[test]
    fn test_low_mining_rate_decreases_difficulty() {
        let current = DifficultyTarget::new(1_000_000_000);

        // Create window with 0.5x the target mining rate
        let low_rate = TARGET_MINING_TXS_PER_BLOCK / 2;
        let blocks: Vec<_> = (0..(DIFFICULTY_WINDOW + DIFFICULTY_LAG + 10) as u64)
            .map(|i| make_block_info(low_rate.max(1), 1_000_000_000, i * 120))
            .collect();

        let next = next_difficulty(&blocks, current);

        // Difficulty should decrease (but floored at 0.5x)
        assert!(next.value() < current.value());
        assert!(next.value() >= current.value() / 2);
    }

    #[test]
    fn test_zero_mining_txs_decreases_difficulty() {
        let current = DifficultyTarget::new(1_000_000_000);

        // Create window with no mining transactions
        let blocks: Vec<_> = (0..(DIFFICULTY_WINDOW + DIFFICULTY_LAG + 10) as u64)
            .map(|i| make_block_info(0, 1_000_000_000, i * 120))
            .collect();

        let next = next_difficulty(&blocks, current);

        // Difficulty should decrease to minimum allowed
        assert_eq!(next.value(), current.value() / MAX_ADJUSTMENT_FACTOR as u128);
    }

    #[test]
    fn test_difficulty_never_below_minimum() {
        let current = DifficultyTarget::new(MIN_DIFFICULTY);

        // Create window with no mining transactions
        let blocks: Vec<_> = (0..(DIFFICULTY_WINDOW + DIFFICULTY_LAG + 10) as u64)
            .map(|i| make_block_info(0, MIN_DIFFICULTY, i * 120))
            .collect();

        let next = next_difficulty(&blocks, current);

        // Should not go below minimum
        assert!(next.value() >= MIN_DIFFICULTY);
    }
}
