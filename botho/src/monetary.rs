// Copyright (c) 2024 Botho Foundation

//! Monetary system for Botho using the Two-Phase model.
//!
//! This module wraps `bth_cluster_tax::DifficultyController` to provide
//! thread-safe monetary policy management for the node.
//!
//! ## Two-Phase Model
//!
//! - **Phase 1 (Halving)**: Bitcoin-like halving schedule for ~10 years.
//!   Difficulty adjusts to maintain target block time.
//!
//! - **Phase 2 (Tail Emission)**: Fixed tail reward with inflation-targeting.
//!   Difficulty adjusts to hit 2% net inflation (gross emission - fee burns).
//!
//! ## Key Insight
//!
//! Rewards are predictable (fixed per phase), difficulty adapts to monetary
//! goals. This gives minters stable income while absorbing fee burn volatility.

use std::{
    sync::{Arc, RwLock},
    time::{SystemTime, UNIX_EPOCH},
};

use bth_cluster_tax::{DifficultyController, MonetaryPolicy, MonetaryStats};

/// Thread-safe monetary system for the Botho node.
///
/// Wraps `DifficultyController` with a `RwLock` for concurrent access.
#[derive(Clone)]
pub struct MonetarySystem {
    inner: Arc<RwLock<DifficultyController>>,
}

impl MonetarySystem {
    /// Create a new monetary system with default policy.
    pub fn new(initial_supply: u64, initial_difficulty: u64) -> Self {
        let start_time = current_unix_time();
        let controller = DifficultyController::new(
            MonetaryPolicy::default(),
            initial_supply,
            initial_difficulty,
            start_time,
        );
        Self {
            inner: Arc::new(RwLock::new(controller)),
        }
    }

    /// Create a new monetary system with custom policy.
    pub fn with_policy(
        policy: MonetaryPolicy,
        initial_supply: u64,
        initial_difficulty: u64,
    ) -> Self {
        let start_time = current_unix_time();
        let controller =
            DifficultyController::new(policy, initial_supply, initial_difficulty, start_time);
        Self {
            inner: Arc::new(RwLock::new(controller)),
        }
    }

    /// Create from an existing controller (for loading from persistence).
    pub fn from_controller(controller: DifficultyController) -> Self {
        Self {
            inner: Arc::new(RwLock::new(controller)),
        }
    }

    /// Get the current block reward.
    pub fn block_reward(&self) -> u64 {
        self.inner.read().map(|c| c.block_reward()).unwrap_or(0)
    }

    /// Get the current difficulty.
    pub fn difficulty(&self) -> u64 {
        self.inner.read().map(|c| c.state.difficulty).unwrap_or(1)
    }

    /// Get the current block height according to monetary system.
    pub fn height(&self) -> u64 {
        self.inner.read().map(|c| c.state.height).unwrap_or(0)
    }

    /// Get the total circulating supply.
    pub fn total_supply(&self) -> u64 {
        self.inner.read().map(|c| c.state.total_supply).unwrap_or(0)
    }

    /// Get the current phase ("Halving" or "Tail Emission").
    pub fn phase(&self) -> &'static str {
        self.inner.read().map(|c| c.phase()).unwrap_or("Unknown")
    }

    /// Get current halving number (0-indexed), or None if in tail emission.
    pub fn current_halving(&self) -> Option<u32> {
        self.inner.read().ok().and_then(|c| c.current_halving())
    }

    /// Blocks until next halving, or None if in tail emission.
    pub fn blocks_until_halving(&self) -> Option<u64> {
        self.inner
            .read()
            .ok()
            .and_then(|c| c.blocks_until_next_halving())
    }

    /// Record a fee burn.
    ///
    /// Call this when transaction fees are burned (subtracted from supply).
    pub fn record_fee_burn(&self, amount: u64) {
        if let Ok(mut controller) = self.inner.write() {
            controller.record_fee_burn(amount);
        }
    }

    /// Process a mined block.
    ///
    /// Returns the block reward. Call this after a block is successfully mined.
    /// The `block_time` should be the timestamp of the new block.
    pub fn process_block(&self, block_time: u64) -> u64 {
        self.inner
            .write()
            .map(|mut c| c.process_block(block_time))
            .unwrap_or(0)
    }

    /// Get a statistics snapshot.
    pub fn stats(&self) -> MonetaryStats {
        let current_time = current_unix_time();
        self.inner
            .read()
            .map(|c| c.stats(current_time))
            .unwrap_or_else(|_| MonetaryStats {
                height: 0,
                phase: "Unknown",
                current_halving: None,
                blocks_until_halving: None,
                block_reward: 0,
                difficulty: 1,
                total_supply: 0,
                total_rewards_emitted: 0,
                total_fees_burned: 0,
                net_supply_change: 0,
                effective_inflation_bps: 0,
                estimated_block_time: 0.0,
            })
    }

    /// Get a clone of the underlying controller (for persistence).
    pub fn controller(&self) -> DifficultyController {
        self.inner.read().map(|c| c.clone()).unwrap_or_else(|_| {
            DifficultyController::new(MonetaryPolicy::default(), 0, 1, current_unix_time())
        })
    }

    /// Replace the controller (for loading from persistence).
    pub fn set_controller(&self, controller: DifficultyController) {
        if let Ok(mut inner) = self.inner.write() {
            *inner = controller;
        }
    }

    /// Check if we're in the halving phase (Phase 1).
    pub fn is_halving_phase(&self) -> bool {
        self.inner
            .read()
            .map(|c| c.policy.is_halving_phase(c.state.height))
            .unwrap_or(false)
    }

    /// Get the policy configuration.
    pub fn policy(&self) -> MonetaryPolicy {
        self.inner
            .read()
            .map(|c| c.policy.clone())
            .unwrap_or_default()
    }
}

impl std::fmt::Debug for MonetarySystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let stats = self.stats();
        f.debug_struct("MonetarySystem")
            .field("height", &stats.height)
            .field("phase", &stats.phase)
            .field("block_reward", &stats.block_reward)
            .field("difficulty", &stats.difficulty)
            .field("total_supply", &stats.total_supply)
            .finish()
    }
}

/// Get current Unix timestamp in seconds.
fn current_unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_secs()
}

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
/// | 5s (high load)    | 2.0%/year           | 2 years        |
/// | 20s (normal)      | 0.5%/year           | 8 years        |
/// | 40s (idle)        | 0.25%/year          | 16 years       |
pub fn mainnet_policy() -> MonetaryPolicy {
    // Constants based on 5-second blocks (minimum block time)
    const SECS_PER_YEAR: u64 = 365 * 24 * 60 * 60;
    const ASSUMED_BLOCK_TIME: u64 = 5;
    const BLOCKS_PER_YEAR: u64 = SECS_PER_YEAR / ASSUMED_BLOCK_TIME; // 6,307,200

    MonetaryPolicy {
        // Phase 1: ~10 years of halvings (at 5s blocks under full load)
        initial_reward: 50_000_000_000_000, // 50 BTH in picocredits
        halving_interval: BLOCKS_PER_YEAR * 2, // 12,614,400 blocks (~2 years at 5s)
        halving_count: 5,                   // 5 halvings over ~10 years

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
    fn test_monetary_system_basic() {
        let system = MonetarySystem::new(0, 1000);

        assert_eq!(system.height(), 0);
        assert_eq!(system.phase(), "Halving");
        assert!(system.block_reward() > 0);
    }

    #[test]
    fn test_process_block() {
        let system = MonetarySystem::new(0, 1000);
        let initial_supply = system.total_supply();

        let reward = system.process_block(current_unix_time());

        assert_eq!(system.height(), 1);
        assert_eq!(system.total_supply(), initial_supply + reward);
    }

    #[test]
    fn test_fee_burn() {
        let system = MonetarySystem::new(1_000_000, 1000);

        system.record_fee_burn(100);

        assert_eq!(system.total_supply(), 999_900);
    }

    #[test]
    fn test_thread_safety() {
        use std::thread;

        let system = MonetarySystem::new(1_000_000, 1000);
        let system2 = system.clone();

        let handle = thread::spawn(move || {
            for _ in 0..100 {
                system2.block_reward();
            }
        });

        for _ in 0..100 {
            system.stats();
        }

        handle.join().unwrap();
    }

    #[test]
    fn test_mainnet_policy() {
        let policy = mainnet_policy();

        assert_eq!(policy.halving_count, 5);
        assert_eq!(policy.tail_inflation_bps, 200);
        // Block time is now 5s (assumed minimum under high load)
        assert_eq!(policy.target_block_time_secs, 5);
        // 2 years worth of 5s blocks
        assert_eq!(policy.halving_interval, 12_614_400);
    }
}
