//! Progressive fee curve: maps cluster wealth to fee rate.

/// Fee rate as a fixed-point value (basis points, 1/10000).
///
/// Using integer arithmetic avoids floating-point non-determinism in consensus.
/// 10000 = 100%, 100 = 1%, 1 = 0.01%
pub type FeeRateBps = u32;

/// Progressive fee curve configuration.
///
/// Maps cluster wealth to fee rate using a sigmoid function:
/// r(W) = r_min + (r_max - r_min) × sigmoid((W - w_mid) / steepness)
///
/// This ensures:
/// - Small clusters pay near r_min
/// - Large clusters pay near r_max
/// - Smooth transition around w_mid
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FeeCurve {
    /// Minimum fee rate in basis points (e.g., 5 = 0.05%)
    pub r_min_bps: FeeRateBps,

    /// Maximum fee rate in basis points (e.g., 3000 = 30%)
    pub r_max_bps: FeeRateBps,

    /// Wealth level at sigmoid midpoint (inflection point)
    pub w_mid: u64,

    /// Controls sigmoid steepness (larger = more gradual transition)
    pub steepness: u64,

    /// Fee rate for fully diffused "background" wealth (basis points)
    pub background_rate_bps: FeeRateBps,
}

impl FeeCurve {
    /// Default curve with reasonable starting parameters.
    ///
    /// These will need tuning based on expected total supply and distribution.
    pub fn default_params() -> Self {
        Self {
            r_min_bps: 5,           // 0.05%
            r_max_bps: 3000,        // 30%
            w_mid: 10_000_000,      // inflection at 10M
            steepness: 5_000_000,   // gradual transition
            background_rate_bps: 10, // 0.1% for diffused coins
        }
    }

    /// Compute fee rate for a given cluster wealth.
    ///
    /// Returns fee rate in basis points.
    pub fn rate_bps(&self, cluster_wealth: u64) -> FeeRateBps {
        // Compute sigmoid: 1 / (1 + e^(-x)) where x = (W - w_mid) / steepness
        // Using fixed-point approximation for determinism
        let sigmoid = self.sigmoid_approx(cluster_wealth);

        // r = r_min + (r_max - r_min) * sigmoid
        let range = self.r_max_bps.saturating_sub(self.r_min_bps);
        let adjustment = ((range as u64 * sigmoid) / Self::SIGMOID_SCALE) as u32;

        self.r_min_bps.saturating_add(adjustment)
    }

    /// Compute fee for a given transfer amount and cluster wealth.
    ///
    /// Returns (fee_amount, net_amount_after_fee).
    pub fn compute_fee(&self, amount: u64, cluster_wealth: u64) -> (u64, u64) {
        let rate = self.rate_bps(cluster_wealth);
        let fee = (amount as u128 * rate as u128 / 10_000) as u64;
        let net = amount.saturating_sub(fee);
        (fee, net)
    }

    // Fixed-point scale for sigmoid output (2^16)
    const SIGMOID_SCALE: u64 = 65536;

    /// Approximate sigmoid function using fixed-point arithmetic.
    ///
    /// Returns value in [0, SIGMOID_SCALE] representing [0, 1].
    ///
    /// Uses piecewise linear interpolation between known sigmoid values:
    /// - sigmoid(-6) ≈ 0.002
    /// - sigmoid(-4) ≈ 0.018
    /// - sigmoid(-2) ≈ 0.119
    /// - sigmoid(0)  = 0.500
    /// - sigmoid(2)  ≈ 0.881
    /// - sigmoid(4)  ≈ 0.982
    /// - sigmoid(6)  ≈ 0.998
    fn sigmoid_approx(&self, wealth: u64) -> u64 {
        if self.steepness == 0 {
            // Avoid division by zero: step function at w_mid
            return if wealth >= self.w_mid {
                Self::SIGMOID_SCALE
            } else {
                0
            };
        }

        // Compute x * 1000 to preserve precision (x_scaled = x * 1000)
        let x_scaled: i64 = if wealth >= self.w_mid {
            ((wealth - self.w_mid) as i128 * 1000 / self.steepness as i128) as i64
        } else {
            -(((self.w_mid - wealth) as i128 * 1000 / self.steepness as i128) as i64)
        };

        // Lookup table: (x * 1000, sigmoid(x) * SIGMOID_SCALE)
        // Using actual sigmoid values for accuracy
        const LUT: [(i64, u64); 7] = [
            (-6000, 131),     // 0.002 * 65536
            (-4000, 1180),    // 0.018 * 65536
            (-2000, 7798),    // 0.119 * 65536
            (0, 32768),       // 0.500 * 65536
            (2000, 57738),    // 0.881 * 65536
            (4000, 64356),    // 0.982 * 65536
            (6000, 65405),    // 0.998 * 65536
        ];

        // Clamp to table range
        if x_scaled <= LUT[0].0 {
            return LUT[0].1;
        }
        if x_scaled >= LUT[LUT.len() - 1].0 {
            return LUT[LUT.len() - 1].1;
        }

        // Find the segment and interpolate
        for i in 0..LUT.len() - 1 {
            let (x0, y0) = LUT[i];
            let (x1, y1) = LUT[i + 1];

            if x_scaled >= x0 && x_scaled < x1 {
                // Linear interpolation: y = y0 + (y1 - y0) * (x - x0) / (x1 - x0)
                let t = (x_scaled - x0) as u64;
                let dx = (x1 - x0) as u64;
                let dy = if y1 >= y0 {
                    y0 + (y1 - y0) * t / dx
                } else {
                    y0 - (y0 - y1) * t / dx
                };
                return dy;
            }
        }

        // Fallback (shouldn't reach here)
        Self::SIGMOID_SCALE / 2
    }
}

impl Default for FeeCurve {
    fn default() -> Self {
        Self::default_params()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fee_curve_extremes() {
        let curve = FeeCurve::default_params();

        // At wealth=0, x = -w_mid/steepness = -2 for default params
        // sigmoid(-2) ≈ 0.119, so rate ≈ 5 + 0.119 * 2995 ≈ 361 bps
        // This is correct behavior - the curve is centered at w_mid
        let rate_zero = curve.rate_bps(0);
        let rate_mid = curve.rate_bps(curve.w_mid);
        assert!(
            rate_zero < rate_mid,
            "Zero wealth should have rate below midpoint: {rate_zero} vs {rate_mid}"
        );

        // Rate at zero should be in lower portion of range (sigmoid(-2) ≈ 0.119)
        let expected_at_zero = curve.r_min_bps +
            ((curve.r_max_bps - curve.r_min_bps) as f64 * 0.119) as u32;
        let tolerance = 50;
        assert!(
            (rate_zero as i32 - expected_at_zero as i32).unsigned_abs() < tolerance,
            "Rate at zero wealth: got {rate_zero}, expected ~{expected_at_zero}"
        );

        // Very large cluster → near maximum rate
        let rate_large = curve.rate_bps(100_000_000);
        assert!(rate_large > 2000, "Large cluster should have high rate: {rate_large}");

        // At midpoint → exactly halfway (sigmoid(0) = 0.5)
        let expected_mid = (curve.r_min_bps + curve.r_max_bps) / 2;
        let tolerance = 50;
        assert!(
            (rate_mid as i32 - expected_mid as i32).unsigned_abs() < tolerance,
            "Mid cluster should have mid rate: {rate_mid} vs expected ~{expected_mid}"
        );
    }

    #[test]
    fn test_fee_computation() {
        let curve = FeeCurve {
            r_min_bps: 100,  // 1%
            r_max_bps: 100,  // 1% (flat for testing)
            w_mid: 1000,
            steepness: 1000,
            background_rate_bps: 100,
        };

        let (fee, net) = curve.compute_fee(10000, 0);
        assert_eq!(fee, 100);  // 1% of 10000
        assert_eq!(net, 9900);
    }

    #[test]
    fn test_monotonic_increase() {
        let curve = FeeCurve::default_params();
        let mut prev_rate = 0;

        for wealth in [0, 1000, 10_000, 100_000, 1_000_000, 10_000_000, 100_000_000] {
            let rate = curve.rate_bps(wealth);
            assert!(
                rate >= prev_rate,
                "Rate should increase with wealth: {prev_rate} -> {rate} at {wealth}"
            );
            prev_rate = rate;
        }
    }

    #[test]
    fn test_monotonic_fine_grained() {
        // Test monotonicity with fine-grained steps across the entire range
        let curve = FeeCurve::default_params();
        let mut prev_rate = 0;
        let mut prev_wealth = 0u64;

        // Test from 0 to 20x the midpoint in 1000 steps
        let max_wealth = curve.w_mid * 20;
        let step = max_wealth / 1000;

        for i in 0..=1000 {
            let wealth = step * i;
            let rate = curve.rate_bps(wealth);
            assert!(
                rate >= prev_rate,
                "Rate should be monotonic: at wealth {} got rate {}, but at {} got {}",
                prev_wealth, prev_rate, wealth, rate
            );
            prev_rate = rate;
            prev_wealth = wealth;
        }
    }

    #[test]
    fn test_sigmoid_continuity() {
        // Test that the sigmoid approximation is continuous (no big jumps)
        let curve = FeeCurve::default_params();

        let max_wealth = curve.w_mid * 10;
        let step = max_wealth / 10000;
        let mut prev_rate = curve.rate_bps(0);

        for i in 1..=10000 {
            let wealth = step * i;
            let rate = curve.rate_bps(wealth);

            // Rate change per step should be small (< 1% of total range per step)
            let max_delta = (curve.r_max_bps - curve.r_min_bps) / 100;
            let delta = if rate >= prev_rate { rate - prev_rate } else { prev_rate - rate };

            assert!(
                delta <= max_delta,
                "Rate change too large at wealth {}: {} -> {} (delta {}, max {})",
                wealth, prev_rate, rate, delta, max_delta
            );
            prev_rate = rate;
        }
    }
}
