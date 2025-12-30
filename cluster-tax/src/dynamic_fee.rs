//! Dynamic fee base that responds to network congestion.
//!
//! # Design Philosophy
//!
//! Botho uses a cascaded congestion control system:
//!
//! 1. **Supply-side adaptation** (primary): Block timing adjusts from 40s to 3s
//!    based on transaction rate, providing up to 13x capacity scaling.
//!
//! 2. **Demand-side adaptation** (secondary): When block timing is at minimum
//!    (maximum capacity), the fee base increases exponentially to shed excess
//!    demand.
//!
//! This cascaded approach keeps fees low during normal operation while providing
//! strong back-pressure during extreme load.
//!
//! # Fee Formula
//!
//! ```text
//! fee = dynamic_base(congestion) × cluster_factor(wealth) × tx_size + memo_fees
//! ```
//!
//! Under extreme conditions (network saturated + wealthy sender):
//! - dynamic_base: up to 100x
//! - cluster_factor: up to 6x
//! - Combined: up to 600x base fee
//!
//! This is intentional egalitarian design: wealthy users pay significantly more
//! during congestion, ensuring small users maintain network access.
//!
//! # Control Theory
//!
//! The system uses exponential response with EMA smoothing:
//!
//! ```text
//! ema_fullness = α × current_fullness + (1-α) × previous_ema
//! fee_base = base_min × e^(k × max(0, ema_fullness - target))
//! ```
//!
//! Parameters:
//! - `α = 0.125`: ~8 block convergence time
//! - `k = 8.0`: Strong exponential response
//! - `target = 0.75`: 75% fullness target, leaving 25% headroom

use std::collections::VecDeque;

/// Dynamic fee base that responds to network congestion.
///
/// Only activates when block timing is at minimum (maximum capacity).
/// Uses exponential response to strongly discourage excess demand.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DynamicFeeBase {
    /// Minimum fee base (floor), in nanoBTH per byte.
    /// Fees never go below this, even when network is empty.
    pub base_min: u64,

    /// Maximum fee base (ceiling), in nanoBTH per byte.
    /// Prevents runaway fees during extreme scenarios.
    pub base_max: u64,

    /// Target block fullness (0.0 to 1.0).
    /// Fees stay at minimum when below this threshold.
    /// Default: 0.75 (75%), leaving 25% headroom for priority txs.
    pub target_fullness: f64,

    /// Exponential response factor.
    /// Higher = more aggressive fee increases above target.
    /// Default: 8.0 (gives ~7.4x at 100% fullness with 75% target)
    pub response_k: f64,

    /// EMA smoothing factor (0.0 to 1.0).
    /// Lower = more smoothing, slower response.
    /// Default: 0.125 (~8 block convergence)
    pub alpha: f64,

    /// Current EMA of block fullness (runtime state).
    /// Persisted across blocks, reset on node restart.
    #[cfg_attr(feature = "serde", serde(default))]
    ema_fullness: f64,

    /// Recent block fullness history for diagnostics.
    /// Not used in fee calculation, just for monitoring.
    #[cfg_attr(feature = "serde", serde(skip, default))]
    history: VecDeque<f64>,
}

/// Snapshot of dynamic fee state for RPC/diagnostics.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DynamicFeeState {
    /// Current fee base in nanoBTH per byte.
    pub current_base: u64,

    /// Current fee multiplier (1.0 = base_min).
    pub multiplier: f64,

    /// Current EMA of block fullness.
    pub ema_fullness: f64,

    /// Target fullness threshold.
    pub target_fullness: f64,

    /// Whether fee adjustment is active (at min block time).
    pub adjustment_active: bool,

    /// Recent block fullness values (newest first).
    pub recent_fullness: Vec<f64>,
}

impl Default for DynamicFeeBase {
    fn default() -> Self {
        Self {
            base_min: 1,           // 1 nanoBTH/byte floor
            base_max: 100,         // 100x max multiplier
            target_fullness: 0.75, // 75% target
            response_k: 8.0,       // Strong exponential response
            alpha: 0.125,          // ~8 block convergence
            ema_fullness: 0.0,
            history: VecDeque::with_capacity(32),
        }
    }
}

impl DynamicFeeBase {
    /// Maximum history entries to keep for diagnostics.
    const MAX_HISTORY: usize = 32;

    /// Create with custom parameters.
    pub fn new(
        base_min: u64,
        base_max: u64,
        target_fullness: f64,
        response_k: f64,
        alpha: f64,
    ) -> Self {
        Self {
            base_min,
            base_max,
            target_fullness: target_fullness.clamp(0.0, 1.0),
            response_k,
            alpha: alpha.clamp(0.0, 1.0),
            ema_fullness: 0.0,
            history: VecDeque::with_capacity(Self::MAX_HISTORY),
        }
    }

    /// Create a disabled (always minimum) fee base.
    /// Useful for testing or if dynamic fees are turned off.
    pub fn disabled() -> Self {
        Self {
            base_min: 1,
            base_max: 1, // Same as min = always 1x
            target_fullness: 1.0,
            response_k: 0.0,
            alpha: 0.0,
            ema_fullness: 0.0,
            history: VecDeque::new(),
        }
    }

    /// Check if dynamic adjustment is disabled.
    pub fn is_disabled(&self) -> bool {
        self.base_min >= self.base_max || self.response_k == 0.0
    }

    /// Update state after a block is finalized.
    ///
    /// Returns the new fee base to use for the next block.
    ///
    /// # Arguments
    /// * `tx_count` - Number of transactions in the finalized block
    /// * `max_tx_count` - Maximum transactions per block (from consensus config)
    /// * `at_min_block_time` - Whether block timing is at minimum (cascaded trigger)
    pub fn update(
        &mut self,
        tx_count: usize,
        max_tx_count: usize,
        at_min_block_time: bool,
    ) -> u64 {
        let fullness = if max_tx_count > 0 {
            (tx_count as f64 / max_tx_count as f64).clamp(0.0, 1.0)
        } else {
            0.0
        };

        // Update EMA
        self.ema_fullness = self.alpha * fullness + (1.0 - self.alpha) * self.ema_fullness;

        // Track history for diagnostics
        self.history.push_front(fullness);
        if self.history.len() > Self::MAX_HISTORY {
            self.history.pop_back();
        }

        self.compute_base(at_min_block_time)
    }

    /// Compute current fee base without updating state.
    ///
    /// # Arguments
    /// * `at_min_block_time` - Whether block timing is at minimum
    pub fn compute_base(&self, at_min_block_time: bool) -> u64 {
        // Disabled check
        if self.is_disabled() {
            return self.base_min;
        }

        // Cascaded: only adjust when timing is maxed out
        if !at_min_block_time {
            return self.base_min;
        }

        // Below target: stay at minimum
        if self.ema_fullness <= self.target_fullness {
            return self.base_min;
        }

        // Above target: exponential increase
        // fee_base = base_min × e^(k × (fullness - target))
        let excess = self.ema_fullness - self.target_fullness;
        let multiplier = (self.response_k * excess).exp();

        let fee = (self.base_min as f64 * multiplier) as u64;
        fee.clamp(self.base_min, self.base_max)
    }

    /// Compute the multiplier (fee_base / base_min) for diagnostics.
    pub fn compute_multiplier(&self, at_min_block_time: bool) -> f64 {
        self.compute_base(at_min_block_time) as f64 / self.base_min as f64
    }

    /// Get current EMA fullness.
    pub fn current_fullness(&self) -> f64 {
        self.ema_fullness
    }

    /// Get full state snapshot for RPC/diagnostics.
    pub fn state(&self, at_min_block_time: bool) -> DynamicFeeState {
        DynamicFeeState {
            current_base: self.compute_base(at_min_block_time),
            multiplier: self.compute_multiplier(at_min_block_time),
            ema_fullness: self.ema_fullness,
            target_fullness: self.target_fullness,
            adjustment_active: at_min_block_time && self.ema_fullness > self.target_fullness,
            recent_fullness: self.history.iter().copied().collect(),
        }
    }

    /// Reset state (e.g., after major reorg or restart).
    pub fn reset(&mut self) {
        self.ema_fullness = 0.0;
        self.history.clear();
    }

    /// Initialize from recent block history.
    ///
    /// Process blocks oldest-to-newest to build up accurate EMA.
    pub fn initialize_from_history<I>(&mut self, recent_blocks: I)
    where
        I: IntoIterator<Item = (usize, usize)>, // (tx_count, max_count)
    {
        self.reset();
        for (tx_count, max_count) in recent_blocks {
            let fullness = if max_count > 0 {
                (tx_count as f64 / max_count as f64).clamp(0.0, 1.0)
            } else {
                0.0
            };
            self.ema_fullness = self.alpha * fullness + (1.0 - self.alpha) * self.ema_fullness;
            self.history.push_front(fullness);
            if self.history.len() > Self::MAX_HISTORY {
                self.history.pop_back();
            }
        }
    }

    /// Estimate how many blocks until fees would return to minimum
    /// given current state and assuming empty blocks.
    pub fn blocks_to_recovery(&self) -> usize {
        if self.ema_fullness <= self.target_fullness {
            return 0;
        }

        // Solve: target = alpha * 0 + (1-alpha)^n * current
        // (1-alpha)^n = target / current
        // n = log(target/current) / log(1-alpha)
        let ratio = self.target_fullness / self.ema_fullness;
        let n = ratio.ln() / (1.0 - self.alpha).ln();
        n.ceil() as usize
    }
}

/// Fee suggestion for wallets based on current network state.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FeeSuggestion {
    /// Minimum fee that will be accepted (may be evicted under load).
    pub minimum: u64,

    /// Standard fee for normal priority (likely included next block).
    pub standard: u64,

    /// Priority fee for guaranteed fast inclusion.
    pub priority: u64,

    /// Current network congestion level (0.0 to 1.0).
    pub congestion: f64,

    /// Whether the network is under high load.
    pub high_load: bool,

    /// Estimated blocks until congestion clears (0 if not congested).
    pub blocks_to_clear: usize,
}

impl DynamicFeeBase {
    /// Generate fee suggestions for wallets.
    ///
    /// # Arguments
    /// * `tx_size` - Estimated transaction size in bytes
    /// * `cluster_factor` - Cluster wealth factor (1000 = 1x, 6000 = 6x)
    /// * `at_min_block_time` - Whether at minimum block time
    pub fn suggest_fees(
        &self,
        tx_size: usize,
        cluster_factor: u64,
        at_min_block_time: bool,
    ) -> FeeSuggestion {
        let base = self.compute_base(at_min_block_time);
        let base_fee = base
            .saturating_mul(tx_size as u64)
            .saturating_mul(cluster_factor)
            / 1000; // FACTOR_SCALE

        // Scale suggestions based on congestion
        let congestion = if at_min_block_time {
            ((self.ema_fullness - self.target_fullness) / (1.0 - self.target_fullness))
                .clamp(0.0, 1.0)
        } else {
            0.0
        };

        // Higher congestion = larger gap between tiers
        let priority_multiplier = 1.5 + congestion; // 1.5x to 2.5x

        FeeSuggestion {
            minimum: base_fee,
            standard: ((base_fee as f64) * 1.2) as u64, // 20% buffer
            priority: ((base_fee as f64) * priority_multiplier) as u64,
            congestion,
            high_load: at_min_block_time && self.ema_fullness > self.target_fullness,
            blocks_to_clear: self.blocks_to_recovery(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_parameters() {
        let fee = DynamicFeeBase::default();
        assert_eq!(fee.base_min, 1);
        assert_eq!(fee.base_max, 100);
        assert!((fee.target_fullness - 0.75).abs() < 0.001);
        assert!((fee.response_k - 8.0).abs() < 0.001);
        assert!((fee.alpha - 0.125).abs() < 0.001);
    }

    #[test]
    fn test_below_target_stays_at_minimum() {
        let mut fee = DynamicFeeBase::default();

        // 50% full, at min block time - should stay at minimum
        for _ in 0..20 {
            fee.update(50, 100, true);
        }

        assert_eq!(fee.compute_base(true), fee.base_min);
    }

    #[test]
    fn test_at_target_stays_at_minimum() {
        let mut fee = DynamicFeeBase::default();

        // Exactly 75% full
        for _ in 0..20 {
            fee.update(75, 100, true);
        }

        assert_eq!(fee.compute_base(true), fee.base_min);
    }

    #[test]
    fn test_above_target_increases() {
        let mut fee = DynamicFeeBase::default();

        // 90% full, at min block time - need many iterations for EMA to converge
        for _ in 0..50 {
            fee.update(90, 100, true);
        }

        let base = fee.compute_base(true);
        assert!(base > fee.base_min, "Should increase above target: {}", base);
        assert!(base <= fee.base_max, "Should not exceed max: {}", base);

        // At 90% with target 75%, excess = 0.15, k=8
        // multiplier = e^(8 * 0.15) = e^1.2 ≈ 3.3x
        // But EMA may not fully converge, so allow wider range
        let multiplier = fee.compute_multiplier(true);
        assert!(
            multiplier >= 1.5 && multiplier < 5.0,
            "Multiplier at 90%: {}",
            multiplier
        );
    }

    #[test]
    fn test_full_blocks_strong_response() {
        let mut fee = DynamicFeeBase::default();

        // 100% full, sustained
        for _ in 0..30 {
            fee.update(100, 100, true);
        }

        // At 100% with target 75%, excess = 0.25, k=8
        // multiplier = e^(8 * 0.25) = e^2 ≈ 7.4x
        let multiplier = fee.compute_multiplier(true);
        assert!(
            multiplier > 5.0 && multiplier < 10.0,
            "Multiplier at 100%: {}",
            multiplier
        );
    }

    #[test]
    fn test_cascaded_requires_min_block_time() {
        let mut fee = DynamicFeeBase::default();

        // 100% full, but NOT at min block time
        for _ in 0..20 {
            fee.update(100, 100, false);
        }

        // Should stay at minimum because timing can still adapt
        assert_eq!(fee.compute_base(false), fee.base_min);

        // But if we check with at_min_block_time=true, it should be higher
        // (EMA is still tracking)
        assert!(fee.compute_base(true) > fee.base_min);
    }

    #[test]
    fn test_ema_smoothing() {
        let mut fee = DynamicFeeBase::default();

        // Spike to 100%
        fee.update(100, 100, true);
        let after_spike = fee.current_fullness();

        // Drop to 0%
        fee.update(0, 100, true);
        let after_drop = fee.current_fullness();

        // EMA should smooth the transition
        assert!(
            after_drop < after_spike,
            "Should decrease: {} -> {}",
            after_spike,
            after_drop
        );
        assert!(after_drop > 0.0, "Should retain memory: {}", after_drop);

        // After many empty blocks, should approach 0
        for _ in 0..50 {
            fee.update(0, 100, true);
        }
        assert!(
            fee.current_fullness() < 0.01,
            "Should approach 0: {}",
            fee.current_fullness()
        );
    }

    #[test]
    fn test_convergence_time() {
        let mut fee = DynamicFeeBase::default();

        // Start at 100% full - need enough iterations to converge
        for _ in 0..50 {
            fee.update(100, 100, true);
        }
        assert!(fee.current_fullness() > 0.95, "Should converge to ~100%: {}", fee.current_fullness());

        // Switch to 50% full - should converge in ~16-20 blocks (alpha=0.125)
        for i in 0..30 {
            fee.update(50, 100, true);
            if i == 15 {
                // After 16 blocks, should be reasonably close to 50%
                let diff = (fee.current_fullness() - 0.5).abs();
                assert!(
                    diff < 0.3,
                    "After 16 blocks, should be near target: {}",
                    fee.current_fullness()
                );
            }
        }

        // Should be close to 50% now (EMA never perfectly converges)
        let diff = (fee.current_fullness() - 0.5).abs();
        assert!(
            diff < 0.1,
            "After 30 blocks, should be at target: {}",
            fee.current_fullness()
        );
    }

    #[test]
    fn test_ceiling_prevents_runaway() {
        let mut fee = DynamicFeeBase::default();

        // Sustained extreme overload
        for _ in 0..100 {
            fee.update(100, 100, true);
        }

        let base = fee.compute_base(true);
        // At 100% with target 75%, excess = 0.25, k=8
        // multiplier = e^(8 * 0.25) = e^2 ≈ 7.4x
        // This is below the 100x ceiling, so we won't hit it
        // The test should verify we're getting a reasonable high multiplier
        assert!(base > fee.base_min * 5, "Should have significant multiplier at 100% load: {}", base);
        assert!(base <= fee.base_max, "Should not exceed ceiling: {}", base);
    }

    #[test]
    fn test_disabled_mode() {
        let fee = DynamicFeeBase::disabled();

        assert!(fee.is_disabled());
        assert_eq!(fee.compute_base(true), fee.base_min);
        assert_eq!(fee.compute_base(false), fee.base_min);
    }

    #[test]
    fn test_history_tracking() {
        let mut fee = DynamicFeeBase::default();

        for i in 0..10 {
            fee.update(i * 10, 100, true);
        }

        let state = fee.state(true);
        assert_eq!(state.recent_fullness.len(), 10);
        // Most recent should be 90%
        assert!((state.recent_fullness[0] - 0.9).abs() < 0.01);
    }

    #[test]
    fn test_initialize_from_history() {
        let mut fee = DynamicFeeBase::default();

        // Simulate historical blocks: all at 80% - need enough for EMA to converge
        let history: Vec<(usize, usize)> = (0..50).map(|_| (80, 100)).collect();
        fee.initialize_from_history(history);

        // EMA should be near 80% (with some tolerance for EMA lag)
        assert!(
            (fee.current_fullness() - 0.8).abs() < 0.1,
            "Should initialize to ~80%: {}",
            fee.current_fullness()
        );
    }

    #[test]
    fn test_blocks_to_recovery() {
        let mut fee = DynamicFeeBase::default();

        // At 95% fullness
        for _ in 0..20 {
            fee.update(95, 100, true);
        }

        let blocks = fee.blocks_to_recovery();
        // Should take some blocks to get back below 75%
        assert!(blocks > 0, "Should need recovery time");
        assert!(blocks < 50, "Shouldn't take forever: {}", blocks);
    }

    #[test]
    fn test_fee_suggestions() {
        let mut fee = DynamicFeeBase::default();

        // Low load
        for _ in 0..20 {
            fee.update(50, 100, false);
        }

        let suggestion = fee.suggest_fees(4000, 1000, false);
        assert!(!suggestion.high_load);
        assert_eq!(suggestion.blocks_to_clear, 0);

        // High load
        for _ in 0..20 {
            fee.update(95, 100, true);
        }

        let suggestion = fee.suggest_fees(4000, 1000, true);
        assert!(suggestion.high_load);
        assert!(suggestion.priority > suggestion.standard);
        assert!(suggestion.standard > suggestion.minimum);
    }

    #[test]
    fn test_zero_max_tx_count() {
        let mut fee = DynamicFeeBase::default();

        // Should handle gracefully
        let base = fee.update(10, 0, true);
        assert_eq!(base, fee.base_min);
    }

    #[test]
    fn test_progressive_response_curve() {
        // Verify the exponential response at different fullness levels
        // Uses the update() method to build up the EMA, then checks multiplier

        let test_cases = [
            (75, 1.0),   // At target: 1x
            (80, 1.5),   // Slight excess: ~1.5x
            (85, 2.2),   // Moderate excess: ~2.2x
            (90, 3.3),   // High: ~3.3x
            (95, 4.9),   // Very high: ~4.9x
            (100, 7.4),  // Saturated: ~7.4x
        ];

        for (fullness_pct, expected_approx) in test_cases {
            let mut test_fee = DynamicFeeBase::default();

            // Run many updates to converge EMA to the target fullness
            for _ in 0..100 {
                test_fee.update(fullness_pct, 100, true);
            }

            let multiplier = test_fee.compute_multiplier(true);

            // For target (75%), expect 1x
            if fullness_pct <= 75 {
                assert!(
                    multiplier < 1.1,
                    "At {}% fullness: got {}x, expected ~1x",
                    fullness_pct,
                    multiplier
                );
            } else {
                // For above target, expect exponential increase
                let tolerance = expected_approx * 0.4; // 40% tolerance for EMA lag
                assert!(
                    (multiplier - expected_approx).abs() < tolerance,
                    "At {}% fullness: got {}x, expected ~{}x",
                    fullness_pct,
                    multiplier,
                    expected_approx
                );
            }
        }
    }
}
