// Copyright (c) 2024 Botho Foundation

//! Smooth emission curve for Botho PoW mining rewards.
//!
//! This module implements a Monero-style smooth emission curve with tail emission,
//! ensuring a decreasing block reward that eventually stabilizes at a minimum
//! (tail emission) to maintain ongoing miner incentives.

use core::cmp::max;

/// Atomic units per CAD (10^12 picoCAD per CAD)
pub const PICO_CAD: u64 = 1_000_000_000_000;

/// Target block time in seconds
pub const TARGET_BLOCK_TIME_SECS: u64 = 20;

/// Initial mining reward in atomic units (50 CAD = 50 * 10^12 picoCAD)
pub const INITIAL_REWARD: u64 = 50 * PICO_CAD;

/// Controls how quickly the reward decreases.
/// The reward halves approximately every EMISSION_SPEED_FACTOR blocks.
/// With 20-second blocks, 6,307,200 blocks ≈ 4 years.
pub const EMISSION_SPEED_FACTOR: u64 = 6_307_200;

/// Minimum reward per mining transaction (tail emission).
/// Set to 0.6 CAD to maintain miner incentives indefinitely.
pub const TAIL_EMISSION: u64 = 600_000_000_000; // 0.6 CAD

/// Maximum theoretical supply in atomic units (picoCAD).
/// With 10^12 picoCAD per CAD, u64 can hold ~18.4 million CAD max.
/// Note: With tail emission, supply grows indefinitely but very slowly.
/// This represents the approximate cap from the initial emission curve.
pub const MAX_SUPPLY: u64 = 18_000_000 * PICO_CAD;

/// Calculate the mining reward for a given block height using smooth emission.
///
/// The emission curve follows a smooth piecewise-linear decay that interpolates
/// between halving points. Within each halving period, the reward decreases
/// linearly from the period start to the period end.
///
/// - At height 0: INITIAL_REWARD (50 CAD)
/// - At height EMISSION_SPEED_FACTOR: INITIAL_REWARD / 2 (25 CAD)
/// - At height 2 * EMISSION_SPEED_FACTOR: INITIAL_REWARD / 4 (12.5 CAD)
/// - And so on...
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
    // Determine which halving period we're in
    let period = height / EMISSION_SPEED_FACTOR;

    // Prevent overflow - after 64 halvings, reward is effectively 0
    if period >= 64 {
        return TAIL_EMISSION;
    }

    // Reward at the start of this period
    let period_start_reward = INITIAL_REWARD >> period;

    // Reward at the start of the next period (end of this period)
    let period_end_reward = INITIAL_REWARD >> (period + 1);

    // Position within the current period (0 to EMISSION_SPEED_FACTOR - 1)
    let position_in_period = height % EMISSION_SPEED_FACTOR;

    // Linear interpolation within the period
    // reward = start - (start - end) * position / period_length
    // Use u128 to avoid overflow with large EMISSION_SPEED_FACTOR
    let reward_decrease = ((period_start_reward - period_end_reward) as u128
        * position_in_period as u128
        / EMISSION_SPEED_FACTOR as u128) as u64;
    let reward = period_start_reward - reward_decrease;

    // Apply tail emission floor
    max(reward, TAIL_EMISSION)
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
