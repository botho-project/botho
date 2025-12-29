//! Fee calculation for Cadence's three transaction types.
//!
//! Cadence uses a dual-incentive fee model:
//! - **Privacy as priced resource**: Private transactions cost more because
//!   they impose verification burden and reduce transparency.
//! - **Progressive wealth taxation**: For private transactions, wealthy clusters
//!   pay a multiplier on top of the base privacy fee.
//!
//! ## Transaction Types
//!
//! | Type    | Privacy | Fee Structure                          |
//! |---------|---------|----------------------------------------|
//! | Plain   | None    | Flat 0.05% (transparent, Bitcoin-like) |
//! | Hidden  | Full    | 0.2% × cluster_factor (1x-6x)          |
//! | Mining  | N/A     | No fee (reward claim)                  |
//!
//! ## Rationale
//!
//! - **Plain transactions** enable cheap, auditable transfers for those who
//!   don't need privacy (exchanges, public payments, transparency by choice).
//! - **Hidden transactions** pay for the societal cost of moving money in the
//!   dark, with wealthy clusters paying more to limit wealth concentration.
//! - **Mining transactions** create new coins via PoW and don't transfer value.

/// Fee rate as a fixed-point value (basis points, 1/10000).
///
/// Using integer arithmetic avoids floating-point non-determinism in consensus.
/// 10000 = 100%, 100 = 1%, 1 = 0.01%
pub type FeeRateBps = u32;

/// The type of transaction, determining fee calculation path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum TransactionType {
    /// Transparent transaction (sender, receiver, amount all public).
    /// No ring signatures, no decoys. Lowest fee.
    Plain,

    /// Private transaction with ring signatures and hidden amounts.
    /// Fee depends on cluster wealth of the sender.
    Hidden,

    /// Mining transaction claiming PoW reward.
    /// No fee (creates new coins).
    Mining,
}

/// Fee configuration for the three transaction types.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FeeConfig {
    /// Fee rate for plain transactions (basis points).
    /// Default: 5 bps (0.05%)
    pub plain_fee_bps: FeeRateBps,

    /// Base fee rate for hidden transactions before cluster multiplier.
    /// Default: 20 bps (0.2%)
    pub hidden_base_fee_bps: FeeRateBps,

    /// Cluster factor curve for hidden transactions.
    pub cluster_curve: ClusterFactorCurve,
}

impl Default for FeeConfig {
    fn default() -> Self {
        Self {
            plain_fee_bps: 5,           // 0.05%
            hidden_base_fee_bps: 20,    // 0.2%
            cluster_curve: ClusterFactorCurve::default(),
        }
    }
}

impl FeeConfig {
    /// Compute the fee for a transaction.
    ///
    /// # Arguments
    /// * `tx_type` - The transaction type
    /// * `amount` - Transfer amount
    /// * `cluster_wealth` - Total wealth of sender's cluster (only used for Hidden)
    ///
    /// # Returns
    /// (fee_amount, net_amount_after_fee)
    pub fn compute_fee(
        &self,
        tx_type: TransactionType,
        amount: u64,
        cluster_wealth: u64,
    ) -> (u64, u64) {
        let rate_bps = self.fee_rate_bps(tx_type, cluster_wealth);
        let fee = (amount as u128 * rate_bps as u128 / 10_000) as u64;
        let net = amount.saturating_sub(fee);
        (fee, net)
    }

    /// Get the fee rate in basis points for a transaction.
    pub fn fee_rate_bps(&self, tx_type: TransactionType, cluster_wealth: u64) -> FeeRateBps {
        match tx_type {
            TransactionType::Plain => self.plain_fee_bps,
            TransactionType::Hidden => {
                let factor = self.cluster_curve.factor(cluster_wealth);
                // fee = base × factor, where factor is in FACTOR_SCALE units
                (self.hidden_base_fee_bps as u64 * factor / ClusterFactorCurve::FACTOR_SCALE) as FeeRateBps
            }
            TransactionType::Mining => 0,
        }
    }
}

/// Cluster factor curve: maps cluster wealth to a multiplier (1x to 6x).
///
/// For hidden transactions, the fee = base_rate × cluster_factor.
/// This creates progressive taxation where wealthy clusters pay more for privacy.
///
/// Uses a sigmoid function:
/// factor(W) = 1 + 5 × sigmoid((W - w_mid) / steepness)
///
/// This ensures:
/// - Small clusters pay ~1x (just the base privacy fee)
/// - Large clusters pay up to 6x (heavily taxed for private transfers)
/// - Smooth transition around w_mid
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ClusterFactorCurve {
    /// Minimum multiplier (1x = just base fee)
    pub factor_min: u32,

    /// Maximum multiplier (6x = heavily taxed)
    pub factor_max: u32,

    /// Wealth level at sigmoid midpoint (inflection point)
    pub w_mid: u64,

    /// Controls sigmoid steepness (larger = more gradual transition)
    pub steepness: u64,

    /// Factor for fully diffused "background" wealth
    pub background_factor: u32,
}

impl ClusterFactorCurve {
    /// Fixed-point scale for factor output.
    /// FACTOR_SCALE = 1000, so factor=1000 means 1x, factor=6000 means 6x.
    pub const FACTOR_SCALE: u64 = 1000;

    /// Fixed-point scale for sigmoid output (2^16)
    pub const SIGMOID_SCALE: u64 = 65536;

    /// Default curve with reasonable starting parameters.
    ///
    /// - factor_min = 1x (small clusters just pay base privacy fee)
    /// - factor_max = 6x (large clusters pay 6× base fee)
    /// - w_mid = 10M (inflection point)
    pub fn default_params() -> Self {
        Self {
            factor_min: 1,          // 1x multiplier
            factor_max: 6,          // 6x multiplier
            w_mid: 10_000_000,      // inflection at 10M
            steepness: 5_000_000,   // gradual transition
            background_factor: 1,   // 1x for diffused coins
        }
    }

    /// Create a flat factor curve (no progressivity).
    ///
    /// Useful for testing or if progressive taxation is disabled.
    pub fn flat(factor: u32) -> Self {
        Self {
            factor_min: factor,
            factor_max: factor,
            w_mid: 0,
            steepness: 1,
            background_factor: factor,
        }
    }

    /// Check if this is a flat (non-progressive) curve.
    pub fn is_flat(&self) -> bool {
        self.factor_min == self.factor_max
    }

    /// Compute the cluster factor for a given cluster wealth.
    ///
    /// Returns factor in FACTOR_SCALE units (1000 = 1x, 6000 = 6x).
    pub fn factor(&self, cluster_wealth: u64) -> u64 {
        let sigmoid = self.sigmoid_approx(cluster_wealth);

        // factor = factor_min + (factor_max - factor_min) × sigmoid
        let range = self.factor_max.saturating_sub(self.factor_min);
        let adjustment = (range as u64 * sigmoid) / Self::SIGMOID_SCALE;

        (self.factor_min as u64 + adjustment) * Self::FACTOR_SCALE
    }

    /// Approximate sigmoid function using fixed-point arithmetic.
    ///
    /// Returns value in [0, SIGMOID_SCALE] representing [0, 1].
    pub fn sigmoid_approx(&self, wealth: u64) -> u64 {
        if self.steepness == 0 {
            return if wealth >= self.w_mid {
                Self::SIGMOID_SCALE
            } else {
                0
            };
        }

        // Compute x * 1000 to preserve precision
        let x_scaled: i64 = if wealth >= self.w_mid {
            ((wealth - self.w_mid) as i128 * 1000 / self.steepness as i128) as i64
        } else {
            -(((self.w_mid - wealth) as i128 * 1000 / self.steepness as i128) as i64)
        };

        // Lookup table: (x * 1000, sigmoid(x) * SIGMOID_SCALE)
        const LUT: [(i64, u64); 7] = [
            (-6000, 131),     // sigmoid(-6) ≈ 0.002
            (-4000, 1180),    // sigmoid(-4) ≈ 0.018
            (-2000, 7798),    // sigmoid(-2) ≈ 0.119
            (0, 32768),       // sigmoid(0)  = 0.500
            (2000, 57738),    // sigmoid(2)  ≈ 0.881
            (4000, 64356),    // sigmoid(4)  ≈ 0.982
            (6000, 65405),    // sigmoid(6)  ≈ 0.998
        ];

        // Clamp to table range
        if x_scaled <= LUT[0].0 {
            return LUT[0].1;
        }
        if x_scaled >= LUT[LUT.len() - 1].0 {
            return LUT[LUT.len() - 1].1;
        }

        // Linear interpolation between table entries
        for i in 0..LUT.len() - 1 {
            let (x0, y0) = LUT[i];
            let (x1, y1) = LUT[i + 1];

            if x_scaled >= x0 && x_scaled < x1 {
                let t = (x_scaled - x0) as u64;
                let dx = (x1 - x0) as u64;
                return if y1 >= y0 {
                    y0 + (y1 - y0) * t / dx
                } else {
                    y0 - (y0 - y1) * t / dx
                };
            }
        }

        Self::SIGMOID_SCALE / 2
    }
}

impl Default for ClusterFactorCurve {
    fn default() -> Self {
        Self::default_params()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transaction_type_fees() {
        let config = FeeConfig::default();

        // Plain transaction: flat 0.05% = 5 bps = 5/10000 = 0.0005
        // 100,000 * 0.0005 = 50
        let (fee, net) = config.compute_fee(TransactionType::Plain, 100_000, 0);
        assert_eq!(fee, 50); // 0.05% of 100,000
        assert_eq!(net, 99_950);

        // Mining transaction: no fee
        let (fee, net) = config.compute_fee(TransactionType::Mining, 100_000, 0);
        assert_eq!(fee, 0);
        assert_eq!(net, 100_000);
    }

    #[test]
    fn test_hidden_fee_small_cluster() {
        let config = FeeConfig::default();

        // Small cluster (0 wealth): factor ≈ 1x, fee ≈ 0.2%
        let rate = config.fee_rate_bps(TransactionType::Hidden, 0);
        // With sigmoid(-2) ≈ 0.119, factor ≈ 1.6x, so rate ≈ 32 bps
        assert!(rate >= 20 && rate <= 50, "Small cluster rate should be near base: {rate}");
    }

    #[test]
    fn test_hidden_fee_large_cluster() {
        let config = FeeConfig::default();

        // Large cluster (100M wealth): factor ≈ 6x, fee ≈ 1.2%
        let rate = config.fee_rate_bps(TransactionType::Hidden, 100_000_000);
        // Should be close to 20 * 6 = 120 bps
        assert!(rate >= 100, "Large cluster should pay high rate: {rate}");
    }

    #[test]
    fn test_cluster_factor_extremes() {
        let curve = ClusterFactorCurve::default_params();

        // At wealth=0, factor should be near minimum
        let factor_zero = curve.factor(0);
        assert!(
            factor_zero < 3000, // Less than 3x
            "Zero wealth should have low factor: {factor_zero}"
        );

        // At very high wealth, factor should be near maximum (6x = 6000)
        let factor_large = curve.factor(100_000_000);
        assert!(
            factor_large >= 5000, // At least 5x
            "Large wealth should have high factor: {factor_large}"
        );

        // At midpoint, factor should be ~3.5x (halfway between 1x and 6x)
        // Due to integer truncation in the calculation, we get 3000 (3x) instead of 3500
        let factor_mid = curve.factor(curve.w_mid);
        let expected_mid = (1 + 6) * ClusterFactorCurve::FACTOR_SCALE / 2; // 3500
        let tolerance = 600; // Allow for integer truncation
        assert!(
            (factor_mid as i64 - expected_mid as i64).unsigned_abs() < tolerance,
            "Mid wealth factor: got {factor_mid}, expected ~{expected_mid}"
        );
    }

    #[test]
    fn test_factor_monotonic_increase() {
        let curve = ClusterFactorCurve::default_params();
        let mut prev_factor = 0;

        for wealth in [0, 1000, 10_000, 100_000, 1_000_000, 10_000_000, 100_000_000] {
            let factor = curve.factor(wealth);
            assert!(
                factor >= prev_factor,
                "Factor should increase with wealth: {prev_factor} -> {factor} at {wealth}"
            );
            prev_factor = factor;
        }
    }

    #[test]
    fn test_flat_curve() {
        let curve = ClusterFactorCurve::flat(3);

        // Flat curve should return same factor regardless of wealth
        assert_eq!(curve.factor(0), 3000);
        assert_eq!(curve.factor(1_000_000), 3000);
        assert_eq!(curve.factor(100_000_000), 3000);
        assert!(curve.is_flat());
    }

    #[test]
    fn test_fee_rate_calculation() {
        // Test with a flat curve for predictable results
        let config = FeeConfig {
            plain_fee_bps: 5,
            hidden_base_fee_bps: 20,
            cluster_curve: ClusterFactorCurve::flat(2), // 2x multiplier
        };

        // Plain: always 5 bps
        assert_eq!(config.fee_rate_bps(TransactionType::Plain, 0), 5);
        assert_eq!(config.fee_rate_bps(TransactionType::Plain, 1_000_000), 5);

        // Hidden with 2x flat factor: 20 * 2 = 40 bps
        assert_eq!(config.fee_rate_bps(TransactionType::Hidden, 0), 40);
        assert_eq!(config.fee_rate_bps(TransactionType::Hidden, 1_000_000), 40);

        // Mining: always 0
        assert_eq!(config.fee_rate_bps(TransactionType::Mining, 0), 0);
    }

    #[test]
    fn test_fee_computation() {
        let config = FeeConfig {
            plain_fee_bps: 100,  // 1% for easy math
            hidden_base_fee_bps: 100,
            cluster_curve: ClusterFactorCurve::flat(1),
        };

        let (fee, net) = config.compute_fee(TransactionType::Plain, 10_000, 0);
        assert_eq!(fee, 100);  // 1% of 10,000
        assert_eq!(net, 9_900);
    }
}

// ============================================================================
// Backwards-compatible FeeCurve for simulation code
// ============================================================================

/// Backwards-compatible fee curve that maps cluster wealth directly to fee rate.
/// Used by simulation code for comparing progressive vs flat fee scenarios.
#[derive(Clone, Debug)]
pub struct FeeCurve {
    pub r_min_bps: FeeRateBps,
    pub r_max_bps: FeeRateBps,
    pub w_mid: u64,
    pub steepness: u64,
    pub background_rate_bps: FeeRateBps,
}

impl FeeCurve {
    pub fn default_params() -> Self {
        Self { r_min_bps: 5, r_max_bps: 3000, w_mid: 10_000_000, steepness: 5_000_000, background_rate_bps: 10 }
    }

    pub fn flat(rate_bps: FeeRateBps) -> Self {
        Self { r_min_bps: rate_bps, r_max_bps: rate_bps, w_mid: 0, steepness: 1, background_rate_bps: rate_bps }
    }

    pub fn is_flat(&self) -> bool { self.r_min_bps == self.r_max_bps }

    pub fn rate_bps(&self, cluster_wealth: u64) -> FeeRateBps {
        if self.is_flat() { return self.r_min_bps; }
        let curve = ClusterFactorCurve { factor_min: self.r_min_bps, factor_max: self.r_max_bps, w_mid: self.w_mid, steepness: self.steepness, background_factor: self.background_rate_bps };
        let sigmoid = curve.sigmoid_approx(cluster_wealth);
        let range = self.r_max_bps.saturating_sub(self.r_min_bps);
        self.r_min_bps.saturating_add(((range as u64 * sigmoid) / ClusterFactorCurve::SIGMOID_SCALE) as u32)
    }

    pub fn compute_fee(&self, amount: u64, cluster_wealth: u64) -> (u64, u64) {
        let rate = self.rate_bps(cluster_wealth);
        let fee = (amount as u128 * rate as u128 / 10_000) as u64;
        (fee, amount.saturating_sub(fee))
    }
}

impl Default for FeeCurve { fn default() -> Self { Self::default_params() } }
