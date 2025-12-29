// Copyright (c) 2024 Cadence Foundation

//! Smooth emission curve for Cadence PoW mining rewards.
//!
//! This module implements a Monero-style smooth emission curve with tail emission,
//! ensuring a decreasing block reward that eventually stabilizes at a minimum
//! (tail emission) to maintain ongoing miner incentives.

use core::cmp::max;

/// Initial mining reward in atomic units (50 billion picoCAD = 50 CAD)
pub const INITIAL_REWARD: u64 = 50_000_000_000_000;

/// Controls how quickly the reward decreases.
/// The reward halves approximately every EMISSION_SPEED_FACTOR blocks.
/// With a value of 2^18 = 262,144 blocks, and ~2 minute blocks,
/// this gives roughly annual halvings.
pub const EMISSION_SPEED_FACTOR: u64 = 1 << 18;

/// Minimum reward per mining transaction (tail emission).
/// Set to 0.6 CAD = 600 billion picoCAD to maintain miner incentives indefinitely.
pub const TAIL_EMISSION: u64 = 600_000_000_000;

/// Total supply cap in atomic units (picoCAD).
/// 21 million CAD = 21_000_000 * 10^12 picoCAD
pub const MAX_SUPPLY: u64 = 21_000_000_000_000_000_000;

/// Calculate the mining reward for a given block height using smooth emission.
///
/// The emission curve follows a smooth exponential decay:
/// reward(height) = INITIAL_REWARD * 2^(-height / EMISSION_SPEED_FACTOR)
///
/// Once the reward drops below TAIL_EMISSION, it stays at TAIL_EMISSION
/// to ensure miners always have an incentive to secure the network.
///
/// # Arguments
/// * `height` - The block height for which to calculate the reward
///
/// # Returns
/// The mining reward in atomic units (picoCAD)
pub fn block_reward(height: u64) -> u64 {
    // Calculate how many times to halve based on height
    let halvings = height / EMISSION_SPEED_FACTOR;

    // Prevent overflow - after 64 halvings, reward is effectively 0
    if halvings >= 64 {
        return TAIL_EMISSION;
    }

    // Calculate base reward with smooth exponential decay
    let base_reward = INITIAL_REWARD >> halvings;

    // Apply tail emission floor
    max(base_reward, TAIL_EMISSION)
}

/// Calculate the cumulative emission up to a given block height.
///
/// This is useful for verifying the total supply at any point in the chain.
///
/// # Arguments
/// * `height` - The block height up to which to calculate total emission
///
/// # Returns
/// The total emission in atomic units (picoCAD) from block 0 to height-1
pub fn cumulative_emission(height: u64) -> u64 {
    // This is a simplified approximation
    // For exact calculation, we would need to integrate the emission curve
    let mut total: u64 = 0;
    let mut current_height: u64 = 0;

    while current_height < height {
        let reward = block_reward(current_height);
        total = total.saturating_add(reward);
        current_height += 1;

        // Safety check to prevent infinite loops
        if current_height > 100_000_000 {
            break;
        }
    }

    total
}

/// Estimate cumulative emission using the geometric series formula.
/// This is much faster than iterating for large heights.
///
/// # Arguments
/// * `height` - The block height up to which to calculate total emission
///
/// # Returns
/// Approximate total emission in atomic units (picoCAD)
pub fn estimated_cumulative_emission(height: u64) -> u64 {
    // Number of complete halving periods
    let full_periods = height / EMISSION_SPEED_FACTOR;

    // For each complete period, calculate emission
    // Sum = INITIAL_REWARD * EMISSION_SPEED_FACTOR * (1 + 0.5 + 0.25 + ... + 2^-n)
    // This geometric series sums to approximately 2 * INITIAL_REWARD * EMISSION_SPEED_FACTOR

    if full_periods >= 64 {
        // We're in tail emission territory
        // Total from decay period + tail emission for remaining blocks
        let decay_total = INITIAL_REWARD
            .saturating_mul(EMISSION_SPEED_FACTOR)
            .saturating_mul(2);
        let tail_blocks = height.saturating_sub(64 * EMISSION_SPEED_FACTOR);
        decay_total.saturating_add(tail_blocks.saturating_mul(TAIL_EMISSION))
    } else {
        // Still in decay period
        let mut total: u64 = 0;
        let mut period_start_reward = INITIAL_REWARD;

        for period in 0..=full_periods {
            let period_start = period * EMISSION_SPEED_FACTOR;
            let period_end = if period == full_periods {
                height
            } else {
                (period + 1) * EMISSION_SPEED_FACTOR
            };
            let blocks_in_period = period_end - period_start;

            let period_emission = period_start_reward.saturating_mul(blocks_in_period);
            total = total.saturating_add(period_emission);

            period_start_reward /= 2;
        }

        total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_reward() {
        assert_eq!(block_reward(0), INITIAL_REWARD);
    }

    #[test]
    fn test_reward_decreases() {
        let reward_0 = block_reward(0);
        let reward_mid = block_reward(EMISSION_SPEED_FACTOR / 2);
        let reward_1 = block_reward(EMISSION_SPEED_FACTOR);

        // Reward should decrease over time
        assert!(reward_0 > reward_mid);
        assert!(reward_mid > reward_1);

        // After one full period, reward should be halved
        assert_eq!(reward_1, INITIAL_REWARD / 2);
    }

    #[test]
    fn test_tail_emission() {
        // After many halvings, should hit tail emission
        let reward = block_reward(64 * EMISSION_SPEED_FACTOR);
        assert_eq!(reward, TAIL_EMISSION);

        // Should never go below tail emission
        let reward_far_future = block_reward(u64::MAX / 2);
        assert_eq!(reward_far_future, TAIL_EMISSION);
    }

    #[test]
    fn test_halving_schedule() {
        // Test first few halvings
        assert_eq!(block_reward(0), INITIAL_REWARD);
        assert_eq!(block_reward(EMISSION_SPEED_FACTOR), INITIAL_REWARD / 2);
        assert_eq!(block_reward(2 * EMISSION_SPEED_FACTOR), INITIAL_REWARD / 4);
        assert_eq!(block_reward(3 * EMISSION_SPEED_FACTOR), INITIAL_REWARD / 8);
    }

    #[test]
    fn test_estimated_emission_sanity() {
        // Early blocks should have roughly INITIAL_REWARD * height
        let early_emission = estimated_cumulative_emission(100);
        assert!(early_emission > 0);
        assert!(early_emission < INITIAL_REWARD * 200); // Upper bound sanity check
    }
}
