// Copyright (c) 2024 Botho Foundation

//! Monetary policy for Botho using the Two-Phase model.
//!
//! ## Two-Phase Model
//!
//! - **Phase 1 (Halving)**: Bitcoin-like halving schedule for ~5 years (~1-year
//!   halvings x 5). Difficulty adjusts to maintain target block time.
//!
//! - **Phase 2 (Tail Emission)**: Fixed tail reward with inflation-targeting.
//!   Difficulty adjusts to hit 2% net inflation (gross emission - fee burns).
//!
//! ## Key Insight
//!
//! Rewards are predictable (fixed per phase), difficulty adapts to monetary
//! goals. This gives minters stable income while absorbing fee burn volatility.
//!
//! ## Difficulty controller
//!
//! The LIVE difficulty controller is `crate::block::EmissionController`
//! (integer, f64-free; see issue #552). The
//! `bth_cluster_tax::DifficultyController` and the former `MonetarySystem`
//! wrapper that exposed it had **no callers in the node** — they were the dead,
//! opposite-convention second controller flagged by audit cycle-6 (C5/L3).
//! `MonetarySystem` has been removed to leave exactly one difficulty controller
//! in the node; this module now provides only the `MonetaryPolicy` constructors
//! actually used by the live reward path (`mainnet_policy`).
//! `DifficultyController` itself is retained in the `cluster-tax` crate for its
//! simulation tooling. (Recoverable from git history if the wrapper is ever
//! needed again.)

use bth_cluster_tax::MonetaryPolicy;

/// Default monetary policy for Botho mainnet.
///
/// # Block Time Assumption
///
/// All monetary calculations assume **5 second blocks** (the minimum block time
/// under high load). When actual block times are slower (up to 40s when idle),
/// effective inflation and halving pace will be proportionally lower.
///
/// This creates a natural inflation dampener: busy network = full inflation,
/// idle network = reduced inflation.
///
/// | Actual Block Time | Effective Inflation | Halving Period |
/// |-------------------|--------------------:|---------------:|
/// | 5s (high load)    | 2.0%/year           | 1 year         |
/// | 20s (normal)      | 0.5%/year           | 4 years        |
/// | 40s (idle)        | 0.25%/year          | 8 years        |
///
/// # Canonical Emission Schedule (decided 2026-06-15, issue #351)
///
/// Chosen from the #350 emission sweep on monetary-policy grounds:
///
/// - `initial_reward` = 50 BTH
/// - `halving_interval` = `BLOCKS_PER_YEAR` (~1 year at 5s blocks)
/// - `halving_count` = 5  →  time-to-tail = `H * K / BLOCKS_PER_YEAR` = 5.00
///   years
/// - `tail_inflation_bps` = 200 (2% perpetual net inflation)
///
/// Derived Phase-1 supply = `R0 * H * (2 - 2^-(K-1))`
/// = 50 * 6,307,200 * 1.9375 = **611,010,000 BTH** (~611M).
pub fn mainnet_policy() -> MonetaryPolicy {
    // Constants based on 5-second blocks (minimum block time)
    const SECS_PER_YEAR: u64 = 365 * 24 * 60 * 60;
    const ASSUMED_BLOCK_TIME: u64 = 5;
    const BLOCKS_PER_YEAR: u64 = SECS_PER_YEAR / ASSUMED_BLOCK_TIME; // 6,307,200

    MonetaryPolicy {
        // Phase 1: ~5 years of halvings (at 5s blocks under full load)
        initial_reward: 50_000_000_000_000, // 50 BTH in picocredits
        halving_interval: BLOCKS_PER_YEAR,  // 6,307,200 blocks (~1 year at 5s)
        halving_count: 5,                   // 5 halvings over ~5 years

        // Phase 2: 2% target net inflation (at 5s blocks)
        tail_inflation_bps: 200,

        // Block time: 5 seconds assumed for monetary calculations
        // Actual block time varies 5-40s based on network load (see dynamic_timing)
        target_block_time_secs: ASSUMED_BLOCK_TIME,
        min_block_time_secs: 3,  // Absolute floor (consensus needs time)
        max_block_time_secs: 60, // Absolute ceiling

        // Difficulty: adjust every ~24 hours at 5s blocks
        difficulty_adjustment_interval: BLOCKS_PER_YEAR / 365, // 17,280 blocks
        max_difficulty_adjustment_bps: 2500,                   // 25% max change per epoch

        // Assume ~0.5% of supply burned in fees annually
        expected_fee_burn_rate_bps: 50,
    }
}

/// Test/development monetary policy with faster parameters.
///
/// Uses same 5s block assumption but with accelerated halving for testing.
pub fn testnet_policy() -> MonetaryPolicy {
    MonetaryPolicy {
        initial_reward: 50_000_000_000_000, // 50 BTH
        halving_interval: 120_000,          // ~1 week at 5s blocks
        halving_count: 5,

        tail_inflation_bps: 200,

        target_block_time_secs: 5,
        min_block_time_secs: 3,
        max_block_time_secs: 60,

        difficulty_adjustment_interval: 1000, // Every ~1.4 hours at 5s
        max_difficulty_adjustment_bps: 5000,  // 50% max change (faster convergence)

        expected_fee_burn_rate_bps: 100,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mainnet_policy() {
        let policy = mainnet_policy();

        assert_eq!(policy.halving_count, 5);
        assert_eq!(policy.tail_inflation_bps, 200);
        assert_eq!(policy.initial_reward, 50_000_000_000_000); // 50 BTH
                                                               // Block time is now 5s (assumed minimum under high load)
        assert_eq!(policy.target_block_time_secs, 5);
        // Canonical schedule (#351): 1 year worth of 5s blocks (BLOCKS_PER_YEAR).
        assert_eq!(policy.halving_interval, 6_307_200);
    }

    /// Locks the canonical emission schedule chosen in #351 so the decision is
    /// regression-proof: derived Phase-1 supply ~611M BTH and time-to-tail = 5
    /// years, computed directly from the policy parameters.
    #[test]
    fn test_mainnet_emission_schedule_locked() {
        const BLOCKS_PER_YEAR: u64 = 6_307_200;
        const PICO_PER_BTH: u128 = 1_000_000_000_000;

        let policy = mainnet_policy();

        // Time-to-tail = H * K / BLOCKS_PER_YEAR.
        let h = policy.halving_interval;
        let k = policy.halving_count as u64;
        let years_to_tail = (h * k) as f64 / BLOCKS_PER_YEAR as f64;
        assert!(
            (years_to_tail - 5.0).abs() < 1e-9,
            "time-to-tail expected 5.00 years, got {years_to_tail}",
        );

        // Derived Phase-1 supply by summing the halving schedule:
        //   sum_{i=0}^{K-1} (R0 >> i) * H  (in picocredits).
        let r0 = policy.initial_reward as u128;
        let h128 = h as u128;
        let mut supply_pico: u128 = 0;
        for i in 0..policy.halving_count {
            supply_pico += (r0 >> i) * h128;
        }

        // Closed form: R0 * H * (2 - 2^-(K-1)).
        // = 50 * 6,307,200 * 1.9375 BTH = 611,010,000 BTH.
        let supply_bth = supply_pico / PICO_PER_BTH;
        assert_eq!(
            supply_bth, 611_010_000,
            "Phase-1 supply expected 611,010,000 BTH, got {supply_bth}",
        );

        // u128 supply accounting (#333): ~611M BTH = 6.11e20 picocredits, which
        // exceeds u64::MAX (~1.84e19). Confirm the chosen scale still requires
        // u128 and is represented without truncation.
        assert!(
            supply_pico > u64::MAX as u128,
            "611M BTH in picocredits ({supply_pico}) must exceed u64::MAX",
        );
        assert_eq!(supply_pico, 611_010_000 * PICO_PER_BTH);
    }
}
