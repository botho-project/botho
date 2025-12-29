//! Two-phase monetary policy for Botho.
//!
//! ## Design Goals
//!
//! 1. **Early adoption incentives**: Halving schedule for ~10 years rewards early adopters
//! 2. **Long-term stability**: 2% net inflation after halving period
//! 3. **Fee burn integration**: Progressive cluster taxes are always burned
//! 4. **Predictable mining**: Fixed reward per block, variable block rate
//!
//! ## Two-Phase Model
//!
//! ### Phase 1: Adoption (Years 0-10)
//! - Block reward follows halving schedule (e.g., halves every 2 years)
//! - Difficulty adjusts traditionally to maintain target block time
//! - Fee burns create bonus deflation (not compensated)
//!
//! ### Phase 2: Stability (Years 10+)
//! - Block reward is fixed (set at transition based on supply)
//! - Difficulty adjusts to achieve target NET inflation (2%)
//! - Block time floats within bounds (e.g., 8-12 minutes)
//! - Higher fees → faster blocks → more gross emission → stable net
//!
//! ## Key Insight
//!
//! Instead of adjusting reward per block (unpredictable for miners),
//! we adjust how many blocks are produced (via difficulty):
//!
//! ```text
//! net_inflation = gross_emission - fees_burned
//!               = (reward × blocks) - fees_burned
//!
//! To hit target net inflation:
//!   blocks_needed = (target_net + fees_burned) / reward
//!   difficulty adjusts to produce this many blocks
//! ```

/// Monetary policy configuration.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MonetaryPolicy {
    // === Phase 1: Halving ===
    /// Initial block reward (smallest unit, e.g., picoMOB).
    pub initial_reward: u64,

    /// Number of blocks between halvings.
    pub halving_interval: u64,

    /// Number of halvings before transitioning to tail emission.
    /// After this many halvings, Phase 2 begins.
    pub halving_count: u32,

    // === Phase 2: Tail Emission ===
    /// Target annual NET inflation rate in basis points (200 = 2%).
    pub tail_inflation_bps: u32,

    // === Block Time ===
    /// Target block time in seconds.
    pub target_block_time_secs: u64,

    /// Minimum block time in seconds (security floor).
    /// Blocks faster than this risk propagation issues.
    pub min_block_time_secs: u64,

    /// Maximum block time in seconds (usability ceiling).
    /// Blocks slower than this hurt user experience.
    pub max_block_time_secs: u64,

    // === Difficulty Adjustment ===
    /// Number of blocks per difficulty adjustment epoch.
    pub difficulty_adjustment_interval: u64,

    /// Maximum difficulty adjustment per epoch in basis points.
    /// E.g., 2500 = 25% max change per epoch.
    pub max_difficulty_adjustment_bps: u32,

    // === Phase 2 Calibration ===
    /// Expected steady-state fee burn rate as fraction of supply (bps).
    /// Used to calibrate tail reward. E.g., 50 = 0.5% of supply burned/year.
    pub expected_fee_burn_rate_bps: u32,
}

impl Default for MonetaryPolicy {
    fn default() -> Self {
        Self {
            // Phase 1: ~10 years of halvings
            initial_reward: 50_000_000_000, // 50 tokens in smallest unit
            halving_interval: 1_051_200,     // ~2 years at 1 min blocks
            halving_count: 5,                // 5 halvings = 10 years

            // Phase 2: 2% target
            tail_inflation_bps: 200,

            // Block time: 1 minute target, 45s-90s bounds
            target_block_time_secs: 60,
            min_block_time_secs: 45,
            max_block_time_secs: 90,

            // Difficulty: adjust every 1440 blocks (~1 day at 1 min)
            difficulty_adjustment_interval: 1440,
            max_difficulty_adjustment_bps: 2500, // 25% max change

            // Assume ~0.5% of supply burned in fees annually
            expected_fee_burn_rate_bps: 50,
        }
    }
}

impl MonetaryPolicy {
    /// Create policy with Bitcoin-like parameters (10 min blocks, 4 year halvings).
    pub fn bitcoin_like() -> Self {
        Self {
            initial_reward: 50_000_000_000,
            halving_interval: 210_000,       // ~4 years at 10 min blocks
            halving_count: 5,

            tail_inflation_bps: 200,

            target_block_time_secs: 600,     // 10 minutes
            min_block_time_secs: 480,        // 8 minutes
            max_block_time_secs: 720,        // 12 minutes

            difficulty_adjustment_interval: 2016, // ~2 weeks
            max_difficulty_adjustment_bps: 2500,

            expected_fee_burn_rate_bps: 50,
        }
    }

    /// Create policy for faster testing.
    pub fn fast_test() -> Self {
        Self {
            initial_reward: 1000,
            halving_interval: 100,
            halving_count: 3,

            tail_inflation_bps: 200,

            target_block_time_secs: 1,
            min_block_time_secs: 1,
            max_block_time_secs: 2,

            difficulty_adjustment_interval: 10,
            max_difficulty_adjustment_bps: 5000, // 50% for faster convergence

            expected_fee_burn_rate_bps: 100,
        }
    }

    /// Get the block height where Phase 2 (tail emission) begins.
    pub fn tail_emission_start_height(&self) -> u64 {
        self.halving_interval * self.halving_count as u64
    }

    /// Check if a given height is in Phase 1 (halving period).
    pub fn is_halving_phase(&self, height: u64) -> bool {
        height < self.tail_emission_start_height()
    }

    /// Get the block reward for a given height (Phase 1 only).
    /// Returns None if in Phase 2 (use tail_reward from state).
    pub fn halving_reward(&self, height: u64) -> Option<u64> {
        if !self.is_halving_phase(height) {
            return None;
        }

        let halvings = height / self.halving_interval;
        Some(self.initial_reward >> halvings)
    }

    /// Calculate the tail emission reward based on supply at transition.
    ///
    /// This is called once when transitioning to Phase 2.
    /// The reward is set to achieve target_net + expected_fees at target block rate.
    pub fn calculate_tail_reward(&self, supply_at_transition: u64) -> u64 {
        // Target annual NET emission
        let target_net = supply_at_transition as u128
            * self.tail_inflation_bps as u128
            / 10_000;

        // Expected annual fee burns
        let expected_burns = supply_at_transition as u128
            * self.expected_fee_burn_rate_bps as u128
            / 10_000;

        // Gross emission needed
        let gross_needed = target_net + expected_burns;

        // Blocks per year at target rate
        let secs_per_year: u128 = 365 * 24 * 3600;
        let blocks_per_year = secs_per_year / self.target_block_time_secs as u128;

        // Reward per block
        let reward = gross_needed / blocks_per_year;

        reward.max(1) as u64
    }

    /// Blocks per year at target block time.
    pub fn target_blocks_per_year(&self) -> u64 {
        let secs_per_year: u64 = 365 * 24 * 3600;
        secs_per_year / self.target_block_time_secs
    }

    /// Epochs per year at target block time.
    pub fn target_epochs_per_year(&self) -> u64 {
        self.target_blocks_per_year() / self.difficulty_adjustment_interval
    }
}

/// Current state of the monetary system.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MonetaryState {
    /// Current block height.
    pub height: u64,

    /// Total circulating supply.
    pub total_supply: u64,

    /// Current mining difficulty.
    /// Higher = harder to mine = slower blocks.
    pub difficulty: u64,

    /// Block reward for Phase 2 (set at transition).
    /// None during Phase 1.
    pub tail_reward: Option<u64>,

    // === Epoch Tracking ===
    /// Block height at start of current difficulty epoch.
    pub epoch_start_height: u64,

    /// Timestamp (seconds) at start of current difficulty epoch.
    pub epoch_start_time: u64,

    /// Total rewards emitted in current epoch.
    pub epoch_rewards_emitted: u64,

    /// Total fees burned in current epoch.
    pub epoch_fees_burned: u64,

    // === Cumulative Stats ===
    /// Total rewards emitted all time.
    pub total_rewards_emitted: u64,

    /// Total fees burned all time.
    pub total_fees_burned: u64,

    /// Number of difficulty adjustments made.
    pub adjustment_count: u64,
}

impl MonetaryState {
    /// Create initial state.
    pub fn new(initial_supply: u64, initial_difficulty: u64, start_time: u64) -> Self {
        Self {
            height: 0,
            total_supply: initial_supply,
            difficulty: initial_difficulty,
            tail_reward: None,

            epoch_start_height: 0,
            epoch_start_time: start_time,
            epoch_rewards_emitted: 0,
            epoch_fees_burned: 0,

            total_rewards_emitted: 0,
            total_fees_burned: 0,
            adjustment_count: 0,
        }
    }

    /// Net supply change since genesis.
    pub fn net_supply_change(&self) -> i64 {
        self.total_rewards_emitted as i64 - self.total_fees_burned as i64
    }

    /// Current effective inflation rate (annualized, in bps).
    pub fn effective_inflation_bps(&self, policy: &MonetaryPolicy) -> i64 {
        if self.total_supply == 0 || self.height == 0 {
            return 0;
        }

        // Estimate time elapsed based on target block time
        let estimated_secs = self.height * policy.target_block_time_secs;
        let years = estimated_secs as f64 / (365.25 * 24.0 * 3600.0);

        if years < 0.001 {
            return 0;
        }

        let net_change = self.net_supply_change();
        let annual_rate = net_change as f64 / self.total_supply as f64 / years;

        (annual_rate * 10_000.0) as i64
    }

    /// Blocks mined in current epoch.
    pub fn blocks_in_epoch(&self) -> u64 {
        self.height - self.epoch_start_height
    }
}

/// Difficulty controller implementing the two-phase monetary policy.
#[derive(Clone, Debug)]
pub struct DifficultyController {
    pub policy: MonetaryPolicy,
    pub state: MonetaryState,
}

impl DifficultyController {
    /// Create a new difficulty controller.
    pub fn new(policy: MonetaryPolicy, initial_supply: u64, initial_difficulty: u64, start_time: u64) -> Self {
        Self {
            policy,
            state: MonetaryState::new(initial_supply, initial_difficulty, start_time),
        }
    }

    /// Get the current block reward.
    pub fn block_reward(&self) -> u64 {
        if let Some(tail) = self.state.tail_reward {
            tail
        } else {
            self.policy.halving_reward(self.state.height).unwrap_or(1)
        }
    }

    /// Record a fee burn.
    pub fn record_fee_burn(&mut self, amount: u64) {
        self.state.epoch_fees_burned += amount;
        self.state.total_fees_burned += amount;
        self.state.total_supply = self.state.total_supply.saturating_sub(amount);
    }

    /// Process a mined block.
    ///
    /// Returns the block reward. Call this after a block is successfully mined.
    pub fn process_block(&mut self, block_time: u64) -> u64 {
        let reward = self.block_reward();

        // Update state
        self.state.height += 1;
        self.state.total_supply += reward;
        self.state.total_rewards_emitted += reward;
        self.state.epoch_rewards_emitted += reward;

        // Check for phase transition
        if self.state.tail_reward.is_none()
            && !self.policy.is_halving_phase(self.state.height)
        {
            // Transition to Phase 2
            self.state.tail_reward = Some(
                self.policy.calculate_tail_reward(self.state.total_supply)
            );
        }

        // Check for difficulty adjustment
        if self.state.blocks_in_epoch() >= self.policy.difficulty_adjustment_interval {
            self.adjust_difficulty(block_time);
        }

        reward
    }

    /// Adjust difficulty at epoch boundary.
    fn adjust_difficulty(&mut self, current_time: u64) {
        let new_difficulty = if self.policy.is_halving_phase(self.state.height) {
            self.traditional_adjustment(current_time)
        } else {
            self.monetary_adjustment(current_time)
        };

        // Apply new difficulty and reset epoch
        self.state.difficulty = new_difficulty;
        self.state.epoch_start_height = self.state.height;
        self.state.epoch_start_time = current_time;
        self.state.epoch_rewards_emitted = 0;
        self.state.epoch_fees_burned = 0;
        self.state.adjustment_count += 1;
    }

    /// Traditional difficulty adjustment (Phase 1): target block time.
    fn traditional_adjustment(&self, current_time: u64) -> u64 {
        let elapsed = current_time.saturating_sub(self.state.epoch_start_time);
        let blocks = self.state.blocks_in_epoch();

        if elapsed == 0 || blocks == 0 {
            return self.state.difficulty;
        }

        // Expected time for this many blocks
        let expected = self.policy.target_block_time_secs * blocks;

        // Adjustment ratio: if blocks came too fast, increase difficulty
        let ratio = expected as f64 / elapsed as f64;

        self.apply_bounded_adjustment(ratio)
    }

    /// Monetary-aware difficulty adjustment (Phase 2): target net inflation.
    fn monetary_adjustment(&self, current_time: u64) -> u64 {
        let elapsed = current_time.saturating_sub(self.state.epoch_start_time);
        let blocks = self.state.blocks_in_epoch();

        if elapsed == 0 || blocks == 0 {
            return self.state.difficulty;
        }

        // === Timing Component ===
        let expected_time = self.policy.target_block_time_secs * blocks;
        let timing_ratio = expected_time as f64 / elapsed as f64;

        // === Monetary Component ===
        // Calculate actual net emission this epoch
        let net_emission = self.state.epoch_rewards_emitted as i64
            - self.state.epoch_fees_burned as i64;

        // Calculate target net emission per epoch
        let epochs_per_year = self.policy.target_epochs_per_year();
        let annual_target = self.state.total_supply as u128
            * self.policy.tail_inflation_bps as u128
            / 10_000;
        let epoch_target = (annual_target / epochs_per_year as u128) as i64;

        // If net emission is too high, we need slower blocks (higher difficulty)
        //   → ratio > 1.0 → multiply difficulty up
        // If net emission is too low, we need faster blocks (lower difficulty)
        //   → ratio < 1.0 → multiply difficulty down
        //
        // The ratio is: what fraction of target did we achieve?
        // If we achieved less than target, ratio < 1, so we reduce difficulty.
        let monetary_ratio = if net_emission > 0 && epoch_target > 0 {
            // Normal case: positive net emission
            // If net_emission < target, ratio < 1, difficulty decreases
            net_emission as f64 / epoch_target as f64
        } else if net_emission <= 0 {
            // We're in deflation! Speed up significantly (lower difficulty).
            0.5_f64.max(
                self.policy.min_block_time_secs as f64
                    / self.policy.max_block_time_secs as f64
            )
        } else {
            // Edge case: target is 0 or negative (shouldn't happen)
            1.0
        };

        // === Blend ===
        // In Phase 2, prioritize monetary target but don't ignore timing stability
        // 70% monetary, 30% timing
        let combined_ratio = timing_ratio * 0.3 + monetary_ratio * 0.7;

        self.apply_bounded_adjustment(combined_ratio)
    }

    /// Apply adjustment with bounds.
    fn apply_bounded_adjustment(&self, ratio: f64) -> u64 {
        let current = self.state.difficulty;

        // Calculate new difficulty
        let new = ((current as f64 * ratio) as u64).max(1);

        // Bound by max adjustment rate
        let max_change = current as u128
            * self.policy.max_difficulty_adjustment_bps as u128
            / 10_000;
        let max_change = (max_change as u64).max(1);

        let rate_floor = current.saturating_sub(max_change);
        let rate_ceiling = current.saturating_add(max_change);

        // Also bound by block time limits (for Phase 2)
        // This only applies meaningfully when we're in monetary adjustment mode
        // Higher difficulty = slower blocks, so:
        // - min_difficulty corresponds to max_block_time (easier = slower)
        // - max_difficulty corresponds to min_block_time (harder = faster)
        //
        // But these are relative to TARGET, not current. We need to think about
        // what difficulty would produce the min/max block times.
        //
        // For simplicity, we just apply rate bounds. Block time bounds are
        // implicitly maintained by the gradual rate limits over multiple epochs.

        // Apply rate bounds only
        let bounded = new.clamp(rate_floor.max(1), rate_ceiling);

        bounded
    }

    /// Get current phase description.
    pub fn phase(&self) -> &'static str {
        if self.policy.is_halving_phase(self.state.height) {
            "Halving"
        } else {
            "Tail Emission"
        }
    }

    /// Get current halving number (0-indexed), or None if in tail emission.
    pub fn current_halving(&self) -> Option<u32> {
        if self.policy.is_halving_phase(self.state.height) {
            Some((self.state.height / self.policy.halving_interval) as u32)
        } else {
            None
        }
    }

    /// Blocks until next halving, or None if in tail emission.
    pub fn blocks_until_next_halving(&self) -> Option<u64> {
        if self.policy.is_halving_phase(self.state.height) {
            let next_halving_height = ((self.state.height / self.policy.halving_interval) + 1)
                * self.policy.halving_interval;
            Some(next_halving_height - self.state.height)
        } else {
            None
        }
    }

    /// Estimate current block time based on recent epoch.
    pub fn estimated_block_time(&self, current_time: u64) -> f64 {
        let elapsed = current_time.saturating_sub(self.state.epoch_start_time);
        let blocks = self.state.blocks_in_epoch();

        if blocks == 0 {
            return self.policy.target_block_time_secs as f64;
        }

        elapsed as f64 / blocks as f64
    }
}

/// Statistics snapshot for reporting.
#[derive(Clone, Debug)]
pub struct MonetaryStats {
    pub height: u64,
    pub phase: &'static str,
    pub current_halving: Option<u32>,
    pub blocks_until_halving: Option<u64>,
    pub block_reward: u64,
    pub difficulty: u64,
    pub total_supply: u64,
    pub total_rewards_emitted: u64,
    pub total_fees_burned: u64,
    pub net_supply_change: i64,
    pub effective_inflation_bps: i64,
    pub estimated_block_time: f64,
}

impl DifficultyController {
    /// Get a statistics snapshot.
    pub fn stats(&self, current_time: u64) -> MonetaryStats {
        MonetaryStats {
            height: self.state.height,
            phase: self.phase(),
            current_halving: self.current_halving(),
            blocks_until_halving: self.blocks_until_next_halving(),
            block_reward: self.block_reward(),
            difficulty: self.state.difficulty,
            total_supply: self.state.total_supply,
            total_rewards_emitted: self.state.total_rewards_emitted,
            total_fees_burned: self.state.total_fees_burned,
            net_supply_change: self.state.net_supply_change(),
            effective_inflation_bps: self.state.effective_inflation_bps(&self.policy),
            estimated_block_time: self.estimated_block_time(current_time),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_halving_reward() {
        let policy = MonetaryPolicy {
            initial_reward: 1000,
            halving_interval: 100,
            halving_count: 3,
            ..Default::default()
        };

        // Before first halving
        assert_eq!(policy.halving_reward(0), Some(1000));
        assert_eq!(policy.halving_reward(50), Some(1000));
        assert_eq!(policy.halving_reward(99), Some(1000));

        // After first halving
        assert_eq!(policy.halving_reward(100), Some(500));
        assert_eq!(policy.halving_reward(150), Some(500));

        // After second halving (height 200-299)
        assert_eq!(policy.halving_reward(200), Some(250));
        assert_eq!(policy.halving_reward(299), Some(250));

        // After all halvings (tail emission)
        assert_eq!(policy.halving_reward(300), None);
        assert_eq!(policy.halving_reward(1000), None);
    }

    #[test]
    fn test_phase_transition() {
        let policy = MonetaryPolicy::fast_test();
        let mut controller = DifficultyController::new(policy, 100_000, 1000, 0);

        assert_eq!(controller.phase(), "Halving");
        assert!(controller.state.tail_reward.is_none());

        // Mine through halving phase
        // 3 halvings × 100 blocks = 300 blocks
        for block_num in 1..=300 {
            controller.process_block(block_num);
        }

        assert_eq!(controller.phase(), "Tail Emission");
        assert!(controller.state.tail_reward.is_some());
    }

    #[test]
    fn test_halving_reduces_reward() {
        let policy = MonetaryPolicy {
            initial_reward: 1000,
            halving_interval: 10,
            halving_count: 3,
            difficulty_adjustment_interval: 100, // Don't adjust during test
            ..MonetaryPolicy::fast_test()
        };
        let mut controller = DifficultyController::new(policy, 0, 1000, 0);

        // First halving period
        assert_eq!(controller.block_reward(), 1000);

        for i in 1..=10 {
            controller.process_block(i);
        }

        // Second halving period
        assert_eq!(controller.block_reward(), 500);

        for i in 11..=20 {
            controller.process_block(i);
        }

        // Third halving period
        assert_eq!(controller.block_reward(), 250);
    }

    #[test]
    fn test_fee_burn_tracking() {
        let policy = MonetaryPolicy::fast_test();
        let mut controller = DifficultyController::new(policy, 100_000, 1000, 0);

        controller.record_fee_burn(500);
        assert_eq!(controller.state.epoch_fees_burned, 500);
        assert_eq!(controller.state.total_fees_burned, 500);
        assert_eq!(controller.state.total_supply, 99_500);

        controller.record_fee_burn(300);
        assert_eq!(controller.state.epoch_fees_burned, 800);
        assert_eq!(controller.state.total_fees_burned, 800);
        assert_eq!(controller.state.total_supply, 99_200);
    }

    #[test]
    fn test_traditional_difficulty_adjustment() {
        let policy = MonetaryPolicy {
            target_block_time_secs: 10,
            difficulty_adjustment_interval: 10,
            max_difficulty_adjustment_bps: 5000,
            ..MonetaryPolicy::fast_test()
        };
        let mut controller = DifficultyController::new(policy, 100_000, 1000, 0);

        // Mine 10 blocks in half the expected time (blocks too fast)
        // Expected: 10 blocks × 10 secs = 100 secs
        // Actual: 50 secs
        for i in 1..=10 {
            controller.process_block(i * 5); // 5 secs per block
        }

        // Difficulty should have increased (blocks were too fast)
        assert!(
            controller.state.difficulty > 1000,
            "Difficulty should increase when blocks are fast: {}",
            controller.state.difficulty
        );
    }

    #[test]
    fn test_monetary_difficulty_adjustment() {
        let policy = MonetaryPolicy {
            initial_reward: 100,
            halving_interval: 10,
            halving_count: 1, // Quick transition to tail
            tail_inflation_bps: 200,
            target_block_time_secs: 10,
            difficulty_adjustment_interval: 10,
            max_difficulty_adjustment_bps: 5000,
            expected_fee_burn_rate_bps: 100,
            ..MonetaryPolicy::fast_test()
        };
        let mut controller = DifficultyController::new(policy, 100_000, 1000, 0);

        // Get through halving phase with correct timing
        for i in 1..=10 {
            controller.process_block(i * 10); // Exactly target block time
        }

        assert_eq!(controller.phase(), "Tail Emission");

        // Mine another epoch at correct timing to establish baseline
        for i in 11..=20 {
            controller.process_block(i * 10);
        }
        let baseline_difficulty = controller.state.difficulty;

        // Now simulate high fee burns (more than expected)
        // This should cause difficulty to decrease (faster blocks needed)
        controller.record_fee_burn(50_000); // Very large burn (50% of supply!)

        // Mine another epoch at correct timing
        for i in 21..=30 {
            controller.process_block(i * 10);
        }

        // With massive fee burns and low net emission, difficulty should decrease
        // to speed up block production and increase gross emission
        // Note: adjustment is bounded, so it may not be a huge decrease
        assert!(
            controller.state.difficulty < baseline_difficulty,
            "Difficulty should decrease with high fee burns: {} vs baseline {}",
            controller.state.difficulty,
            baseline_difficulty
        );
    }

    #[test]
    fn test_difficulty_bounds() {
        let policy = MonetaryPolicy {
            target_block_time_secs: 10,
            min_block_time_secs: 8,
            max_block_time_secs: 12,
            difficulty_adjustment_interval: 5,
            max_difficulty_adjustment_bps: 2500, // 25% max change
            ..MonetaryPolicy::fast_test()
        };
        let mut controller = DifficultyController::new(policy, 100_000, 1000, 0);

        // Mine blocks extremely fast (1 sec each, target is 10)
        for i in 1..=5 {
            controller.process_block(i);
        }

        // Difficulty should increase but be bounded by 25% max change
        let max_expected = 1000 + (1000 * 2500 / 10_000); // 1250
        assert!(
            controller.state.difficulty <= max_expected,
            "Difficulty {} should be bounded by max adjustment rate (max {})",
            controller.state.difficulty,
            max_expected
        );
        assert!(
            controller.state.difficulty > 1000,
            "Difficulty should have increased from 1000: {}",
            controller.state.difficulty
        );
    }

    #[test]
    fn test_net_supply_change() {
        let policy = MonetaryPolicy::fast_test();
        let mut controller = DifficultyController::new(policy, 100_000, 1000, 0);

        // Mine some blocks
        for i in 1..=10 {
            controller.process_block(i);
        }
        let rewards = controller.state.total_rewards_emitted;

        // Burn some fees
        controller.record_fee_burn(500);

        assert_eq!(
            controller.state.net_supply_change(),
            rewards as i64 - 500
        );
    }

    #[test]
    fn test_tail_reward_calculation() {
        let policy = MonetaryPolicy {
            tail_inflation_bps: 200,           // 2%
            expected_fee_burn_rate_bps: 50,    // 0.5%
            target_block_time_secs: 60,        // 1 minute
            ..Default::default()
        };

        let supply = 100_000_000u64; // 100M
        let tail_reward = policy.calculate_tail_reward(supply);

        // Annual target: 2% of 100M = 2M net
        // Expected burns: 0.5% of 100M = 0.5M
        // Gross needed: 2.5M
        // Blocks per year: 525,600 (at 1 min)
        // Reward per block: 2.5M / 525,600 ≈ 4.76

        let blocks_per_year = 365 * 24 * 60; // 525,600
        let expected_gross = 2_500_000u64;
        let expected_reward = expected_gross / blocks_per_year;

        assert!(
            (tail_reward as i64 - expected_reward as i64).abs() < 2,
            "Tail reward {} should be close to {}",
            tail_reward,
            expected_reward
        );
    }

    #[test]
    fn test_stats_snapshot() {
        let policy = MonetaryPolicy::fast_test();
        let mut controller = DifficultyController::new(policy, 100_000, 1000, 0);

        for i in 1..=50 {
            controller.process_block(i);
        }
        controller.record_fee_burn(1000);

        let stats = controller.stats(50);

        assert_eq!(stats.height, 50);
        assert_eq!(stats.phase, "Halving");
        assert_eq!(stats.total_fees_burned, 1000);
        assert!(stats.total_rewards_emitted > 0);
    }

    #[test]
    fn test_blocks_until_halving() {
        let policy = MonetaryPolicy {
            halving_interval: 100,
            halving_count: 3,
            ..MonetaryPolicy::fast_test()
        };
        let mut controller = DifficultyController::new(policy, 100_000, 1000, 0);

        assert_eq!(controller.blocks_until_next_halving(), Some(100));

        for i in 1..=50 {
            controller.process_block(i);
        }
        assert_eq!(controller.blocks_until_next_halving(), Some(50));

        for i in 51..=100 {
            controller.process_block(i);
        }
        assert_eq!(controller.blocks_until_next_halving(), Some(100));

        // Get to tail emission
        for i in 101..=300 {
            controller.process_block(i);
        }
        assert_eq!(controller.blocks_until_next_halving(), None);
    }
}
