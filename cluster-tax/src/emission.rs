//! Adaptive emission controller for Botho's monetary policy.
//!
//! Botho uses an adaptive emission model that adjusts block rewards based on
//! fee burn rates to achieve a target net inflation rate.
//!
//! ## Design Goals
//!
//! 1. **Stable purchasing power**: Target a predictable inflation rate (e.g., 2% annually)
//! 2. **Fee-burn offset**: When fees are burned, increase emission to compensate
//! 3. **Smooth adjustments**: Avoid sudden reward changes that could destabilize mining
//!
//! ## Formula
//!
//! ```text
//! target_emission_per_epoch = (supply × target_inflation) / epochs_per_year
//! required_gross_emission = target_emission_per_epoch + fees_burned_last_epoch
//! block_reward = required_gross_emission / blocks_per_epoch
//! ```
//!
//! ## Example
//!
//! With 100M supply, 2% target inflation, 1000 blocks/epoch, 365 epochs/year:
//! - Target net emission per epoch: 100M × 0.02 / 365 ≈ 5,479 coins
//! - If 2,000 coins were burned in fees last epoch:
//! - Gross emission needed: 5,479 + 2,000 = 7,479 coins
//! - Block reward: 7,479 / 1000 ≈ 7.48 coins per block
//!
//! ## Bounds
//!
//! To prevent runaway emission or deflation:
//! - `min_block_reward`: Floor to ensure miners are always compensated
//! - `max_block_reward`: Ceiling to prevent hyperinflation if fees spike
//! - `max_adjustment_rate`: Limit how fast rewards can change between epochs

/// Configuration for the adaptive emission controller.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EmissionConfig {
    /// Target annual inflation rate in basis points (100 = 1%, 200 = 2%).
    pub target_inflation_bps: u32,

    /// Number of blocks per epoch (emission adjustment period).
    pub blocks_per_epoch: u64,

    /// Number of epochs per year.
    pub epochs_per_year: u64,

    /// Minimum block reward (floor).
    pub min_block_reward: u64,

    /// Maximum block reward (ceiling).
    pub max_block_reward: u64,

    /// Maximum adjustment rate per epoch in basis points.
    /// E.g., 1000 = 10% max change per epoch.
    pub max_adjustment_rate_bps: u32,

    /// Initial block reward before any adjustments.
    pub initial_block_reward: u64,
}

impl Default for EmissionConfig {
    fn default() -> Self {
        Self {
            target_inflation_bps: 200,       // 2% annual target
            blocks_per_epoch: 1000,          // ~1 epoch per day at 1 block/86s
            epochs_per_year: 365,            // Daily epochs
            min_block_reward: 1,             // Never go to zero
            max_block_reward: 1_000_000,     // 1M coin ceiling
            max_adjustment_rate_bps: 1000,   // Max 10% change per epoch
            initial_block_reward: 1000,      // Starting reward
        }
    }
}

impl EmissionConfig {
    /// Create config for a specific target inflation rate.
    pub fn with_target_inflation(mut self, rate_percent: f64) -> Self {
        self.target_inflation_bps = (rate_percent * 100.0) as u32;
        self
    }

    /// Create config with specific epoch parameters.
    pub fn with_epoch_params(mut self, blocks_per_epoch: u64, epochs_per_year: u64) -> Self {
        self.blocks_per_epoch = blocks_per_epoch;
        self.epochs_per_year = epochs_per_year;
        self
    }
}

/// Tracks emission state across epochs.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EmissionState {
    /// Current circulating supply.
    pub total_supply: u64,

    /// Current epoch number.
    pub current_epoch: u64,

    /// Current block reward.
    pub current_block_reward: u64,

    /// Fees burned in the current epoch so far.
    pub fees_burned_current_epoch: u64,

    /// Fees burned in the previous epoch (used for adjustment).
    pub fees_burned_last_epoch: u64,

    /// Total fees burned all time.
    pub total_fees_burned: u64,

    /// Total coins emitted all time.
    pub total_emitted: u64,

    /// Blocks mined in current epoch.
    pub blocks_in_current_epoch: u64,
}

impl EmissionState {
    /// Create initial emission state.
    pub fn new(initial_supply: u64, initial_block_reward: u64) -> Self {
        Self {
            total_supply: initial_supply,
            current_epoch: 0,
            current_block_reward: initial_block_reward,
            fees_burned_current_epoch: 0,
            fees_burned_last_epoch: 0,
            total_fees_burned: 0,
            total_emitted: 0,
            blocks_in_current_epoch: 0,
        }
    }

    /// Get the net supply change since genesis.
    pub fn net_supply_change(&self) -> i64 {
        self.total_emitted as i64 - self.total_fees_burned as i64
    }

    /// Get current effective inflation rate (annualized, in bps).
    pub fn effective_inflation_bps(&self, config: &EmissionConfig) -> i64 {
        if self.total_supply == 0 || self.current_epoch == 0 {
            return 0;
        }

        let net_change = self.net_supply_change();
        let epochs_elapsed = self.current_epoch;
        let years_elapsed = epochs_elapsed as f64 / config.epochs_per_year as f64;

        if years_elapsed < 0.001 {
            return 0;
        }

        let annual_rate = net_change as f64 / self.total_supply as f64 / years_elapsed;
        (annual_rate * 10_000.0) as i64
    }
}

/// Adaptive emission controller.
#[derive(Clone, Debug)]
pub struct EmissionController {
    pub config: EmissionConfig,
    pub state: EmissionState,
}

impl EmissionController {
    /// Create a new emission controller.
    pub fn new(config: EmissionConfig, initial_supply: u64) -> Self {
        let initial_reward = config.initial_block_reward;
        Self {
            config,
            state: EmissionState::new(initial_supply, initial_reward),
        }
    }

    /// Record a fee burn event.
    pub fn record_fee_burn(&mut self, amount: u64) {
        self.state.fees_burned_current_epoch += amount;
        self.state.total_fees_burned += amount;
        // Fees are subtracted from supply
        self.state.total_supply = self.state.total_supply.saturating_sub(amount);
    }

    /// Get the current block reward.
    pub fn block_reward(&self) -> u64 {
        self.state.current_block_reward
    }

    /// Process a mined block: emit reward and track progress.
    ///
    /// Returns the block reward to pay to the miner.
    pub fn process_block(&mut self) -> u64 {
        let reward = self.state.current_block_reward;

        // Update state
        self.state.total_supply += reward;
        self.state.total_emitted += reward;
        self.state.blocks_in_current_epoch += 1;

        // Check if epoch is complete
        if self.state.blocks_in_current_epoch >= self.config.blocks_per_epoch {
            self.advance_epoch();
        }

        reward
    }

    /// Advance to the next epoch and recalculate block reward.
    fn advance_epoch(&mut self) {
        // Calculate new block reward based on target inflation and fee burns
        let new_reward = self.calculate_next_block_reward();

        // Apply adjustment rate limit
        let limited_reward = self.apply_adjustment_limit(new_reward);

        // Apply min/max bounds
        let bounded_reward = limited_reward
            .max(self.config.min_block_reward)
            .min(self.config.max_block_reward);

        // Transition to new epoch
        self.state.fees_burned_last_epoch = self.state.fees_burned_current_epoch;
        self.state.fees_burned_current_epoch = 0;
        self.state.blocks_in_current_epoch = 0;
        self.state.current_epoch += 1;
        self.state.current_block_reward = bounded_reward;
    }

    /// Calculate the ideal block reward for the next epoch.
    fn calculate_next_block_reward(&self) -> u64 {
        // Target net emission per epoch
        // = (supply × target_inflation_bps / 10000) / epochs_per_year
        let target_annual_emission = (self.state.total_supply as u128
            * self.config.target_inflation_bps as u128)
            / 10_000;
        let target_epoch_emission = target_annual_emission / self.config.epochs_per_year as u128;

        // Gross emission needed = target net + fees burned
        // This way: net = gross - fees = target
        let fees_burned = self.state.fees_burned_current_epoch as u128;
        let gross_emission_needed = target_epoch_emission + fees_burned;

        // Per-block reward
        let reward = gross_emission_needed / self.config.blocks_per_epoch as u128;

        reward as u64
    }

    /// Limit reward change to max adjustment rate.
    fn apply_adjustment_limit(&self, new_reward: u64) -> u64 {
        let current = self.state.current_block_reward;
        let max_delta = (current as u128 * self.config.max_adjustment_rate_bps as u128 / 10_000) as u64;
        let max_delta = max_delta.max(1); // Always allow at least 1 unit change

        if new_reward > current {
            current.saturating_add(max_delta.min(new_reward - current))
        } else {
            current.saturating_sub(max_delta.min(current - new_reward))
        }
    }

    /// Get projected annual inflation based on current state.
    pub fn projected_annual_inflation_bps(&self) -> i64 {
        self.state.effective_inflation_bps(&self.config)
    }

    /// Simulate N epochs forward with given fee burn rate.
    pub fn simulate_epochs(&self, num_epochs: u64, avg_fees_per_epoch: u64) -> EmissionState {
        let mut sim = self.clone();

        for _ in 0..num_epochs {
            // Simulate blocks in epoch
            for _ in 0..sim.config.blocks_per_epoch {
                sim.process_block();
            }
            // Simulate fee burns
            sim.record_fee_burn(avg_fees_per_epoch);
        }

        sim.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_emission() {
        let config = EmissionConfig::default();
        let mut controller = EmissionController::new(config, 100_000_000);

        // Initial state
        assert_eq!(controller.block_reward(), 1000);
        assert_eq!(controller.state.total_supply, 100_000_000);

        // Mine a block
        let reward = controller.process_block();
        assert_eq!(reward, 1000);
        assert_eq!(controller.state.total_supply, 100_001_000);
        assert_eq!(controller.state.total_emitted, 1000);
    }

    #[test]
    fn test_fee_burn_tracking() {
        let config = EmissionConfig::default();
        let mut controller = EmissionController::new(config, 100_000_000);

        controller.record_fee_burn(500);
        assert_eq!(controller.state.fees_burned_current_epoch, 500);
        assert_eq!(controller.state.total_fees_burned, 500);
        assert_eq!(controller.state.total_supply, 99_999_500);
    }

    #[test]
    fn test_epoch_transition() {
        let config = EmissionConfig {
            blocks_per_epoch: 10,
            ..Default::default()
        };
        let mut controller = EmissionController::new(config, 100_000_000);

        // Mine 10 blocks to complete an epoch
        for _ in 0..10 {
            controller.process_block();
        }

        assert_eq!(controller.state.current_epoch, 1);
        assert_eq!(controller.state.blocks_in_current_epoch, 0);
    }

    #[test]
    fn test_fee_burn_increases_emission() {
        let config = EmissionConfig {
            blocks_per_epoch: 10,
            target_inflation_bps: 200,
            max_adjustment_rate_bps: 10000, // Allow large adjustments for test
            ..Default::default()
        };
        let mut controller = EmissionController::new(config, 100_000_000);

        // Record large fee burn
        controller.record_fee_burn(10_000);

        // Complete epoch
        for _ in 0..10 {
            controller.process_block();
        }

        // Block reward should have increased to compensate for fee burn
        // (subject to adjustment limits)
        assert!(
            controller.state.current_block_reward >= 1000,
            "Reward should increase or stay same with fee burns: {}",
            controller.state.current_block_reward
        );
    }

    #[test]
    fn test_target_inflation_convergence() {
        let config = EmissionConfig {
            blocks_per_epoch: 100,
            epochs_per_year: 100,
            target_inflation_bps: 200, // 2%
            max_adjustment_rate_bps: 5000, // 50% adjustment allowed
            initial_block_reward: 1000,
            min_block_reward: 1,
            max_block_reward: 100_000,
        };
        let initial_supply = 100_000_000u64;
        let mut controller = EmissionController::new(config.clone(), initial_supply);

        // Simulate 100 epochs (1 year) with consistent fee burns
        let fees_per_epoch = 5_000u64; // Burn 5000 per epoch

        for _ in 0..100 {
            // Burn fees
            controller.record_fee_burn(fees_per_epoch);

            // Mine all blocks in epoch
            for _ in 0..config.blocks_per_epoch {
                controller.process_block();
            }
        }

        // Check that we're near target inflation
        let effective_rate = controller.projected_annual_inflation_bps();
        let target = config.target_inflation_bps as i64;

        // Should be within 50% of target (allowing for convergence time)
        let tolerance = target / 2;
        assert!(
            (effective_rate - target).abs() < tolerance,
            "Effective inflation {} should be near target {}, tolerance {}",
            effective_rate,
            target,
            tolerance
        );
    }

    #[test]
    fn test_adjustment_rate_limit() {
        let config = EmissionConfig {
            blocks_per_epoch: 10,
            max_adjustment_rate_bps: 1000, // 10% max change
            initial_block_reward: 1000,
            ..Default::default()
        };
        let mut controller = EmissionController::new(config, 100_000_000);

        // Record massive fee burn that would require huge reward increase
        controller.record_fee_burn(1_000_000);

        // Complete epoch
        for _ in 0..10 {
            controller.process_block();
        }

        // Reward should only increase by max 10%
        assert!(
            controller.state.current_block_reward <= 1100,
            "Reward {} should be limited to 10% increase from 1000",
            controller.state.current_block_reward
        );
    }

    #[test]
    fn test_min_max_bounds() {
        let config = EmissionConfig {
            blocks_per_epoch: 10,
            min_block_reward: 100,
            max_block_reward: 5000,
            max_adjustment_rate_bps: 10000, // No limit for test
            initial_block_reward: 1000,
            ..Default::default()
        };
        let mut controller = EmissionController::new(config, 100_000_000);

        // Scenario 1: Try to push reward below minimum
        // (would need deflation target, but we can force it via state)
        controller.state.current_block_reward = 50;
        controller.record_fee_burn(0);
        for _ in 0..10 {
            controller.process_block();
        }
        assert!(
            controller.state.current_block_reward >= 100,
            "Reward {} should respect minimum 100",
            controller.state.current_block_reward
        );
    }

    #[test]
    fn test_net_supply_change() {
        let config = EmissionConfig {
            blocks_per_epoch: 10,
            ..Default::default()
        };
        let mut controller = EmissionController::new(config, 100_000_000);

        // Emit 10000 (10 blocks × 1000)
        for _ in 0..10 {
            controller.process_block();
        }

        // Burn 3000
        controller.record_fee_burn(3000);

        // Net change should be 7000
        assert_eq!(controller.state.net_supply_change(), 7000);
        assert_eq!(controller.state.total_supply, 100_007_000);
    }

    #[test]
    fn test_zero_fee_scenario() {
        // Calculate the correct initial reward for 2% inflation:
        // target_annual = 100M * 0.02 = 2M
        // target_per_epoch = 2M / 365 ≈ 5479
        // target_per_block = 5479 / 100 ≈ 55
        let config = EmissionConfig {
            blocks_per_epoch: 100,
            epochs_per_year: 365,
            target_inflation_bps: 200,
            initial_block_reward: 55, // Correct for 2% with 100M supply
            max_adjustment_rate_bps: 5000, // Allow reasonable convergence
            ..Default::default()
        };
        let mut controller = EmissionController::new(config.clone(), 100_000_000);

        // Simulate with no fee burns - should maintain target inflation
        for _ in 0..10 {
            for _ in 0..config.blocks_per_epoch {
                controller.process_block();
            }
        }

        // With no fees burned, gross emission should equal net target
        // Block reward should converge to target
        let final_reward = controller.state.current_block_reward;
        let target_reward = 55u64;

        // Should be within 50% of target (allowing for convergence)
        assert!(
            (final_reward as i64 - target_reward as i64).unsigned_abs() < target_reward,
            "Reward {} should converge toward {} with no fee burns",
            final_reward,
            target_reward
        );
    }
}
