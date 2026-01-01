//! Fee calculation for Botho's transaction types.
//!
//! Botho uses a size-based fee model with progressive wealth taxation:
//!
//! ```text
//! fee = fee_per_byte × tx_size × cluster_factor
//! ```
//!
//! ## Transaction Types
//!
//! | Type            | Ring Signature | Typical Size | Fee Rate           |
//! |-----------------|----------------|--------------|---------------------|
//! | Standard-Private| CLSAG          | ~4 KB        | size × cluster_factor |
//! | PQ-Private      | LION           | ~65 KB       | size × cluster_factor |
//! | Minting         | N/A            | ~1.5 KB      | No fee              |
//!
//! ## Fee Components
//!
//! 1. **Size-based fee**: Larger transactions pay more (proportional to bytes)
//! 2. **Progressive multiplier**: Cluster factor ranges from 1x to 6x based on
//!    the sender's cluster wealth, ensuring wealthy clusters pay more
//!
//! ## Size Rationale
//!
//! | Type            | Input Size    | Output Size | Typical Total |
//! |-----------------|---------------|-------------|---------------|
//! | Standard-Private| ~700 B (CLSAG)| ~1.2 KB     | ~4 KB         |
//! | PQ-Private      | ~63 KB (LION) | ~1.2 KB     | ~65 KB        |
//!
//! PQ-Private transactions are ~16x larger due to lattice-based signatures,
//! so they naturally cost ~16x more in size fees.
//!
//! ## Progressive Taxation
//!
//! The cluster factor ensures wealthy clusters pay higher fees:
//! - Small clusters: 1x multiplier (just size fee)
//! - Large clusters: up to 6x multiplier
//! - Sigmoid curve provides smooth transition

/// Fee rate as a fixed-point value (basis points, 1/10000).
///
/// Using integer arithmetic avoids floating-point non-determinism in consensus.
/// 10000 = 100%, 100 = 1%, 1 = 0.01%
pub type FeeRateBps = u32;

/// Count the number of outputs with encrypted memos.
///
/// This counts outputs where `has_memo(output)` is true.
/// Wallets should set `e_memo = None` (rather than encrypting an `UnusedMemo`)
/// to avoid memo fees on outputs that don't need memos.
///
/// # Usage in transaction validation:
/// ```ignore
/// let num_memos = tx.prefix.outputs.iter()
///     .filter(|o| o.e_memo.is_some())
///     .count();
/// let required_fee = fee_config.minimum_fee(tx_type, amount, cluster_wealth, num_memos);
/// ```
///
/// # Memo Fee Economics
///
/// Each memo adds ~5% to the base fee (configurable via `memo_fee_rate_bps`).
/// This incentivizes:
/// - Skipping memos on change outputs
/// - Using `e_memo = None` instead of encrypting `UnusedMemo`
/// - Thoughtful use of memo storage (66 bytes per memo, stored forever)
pub fn count_outputs_with_memos<T, F>(outputs: &[T], has_memo: F) -> usize
where
    F: Fn(&T) -> bool,
{
    outputs.iter().filter(|o| has_memo(o)).count()
}

/// The type of transaction, determining fee calculation path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum TransactionType {
    /// Standard-private transaction with CLSAG ring signatures (~700B/input).
    /// Fee = size × cluster_factor. Recommended for daily transactions.
    Hidden,

    /// PQ-private transaction with LION ring signatures (~63KB/input).
    /// Fee = size × cluster_factor. ~16x larger than Hidden due to lattice
    /// sigs. Recommended for high-value or long-term security needs.
    PqHidden,

    /// Minting transaction claiming PoW reward.
    /// No fee (creates new coins).
    Minting,
}

/// Fee configuration for transaction types.
///
/// Fees are calculated as: `fee_per_byte × tx_size × cluster_factor`
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FeeConfig {
    /// Fee per byte in nanoBTH.
    /// Default: 1 nanoBTH per byte
    pub fee_per_byte: u64,

    /// Cluster factor curve for progressive fee calculation.
    /// Multiplier ranges from 1x (small clusters) to 6x (large clusters).
    pub cluster_curve: ClusterFactorCurve,

    /// Fee per memo in nanoBTH.
    /// Each output with `e_memo.is_some()` adds this flat fee.
    /// Default: 100 nanoBTH per memo (66 bytes stored forever)
    pub fee_per_memo: u64,
}

impl Default for FeeConfig {
    fn default() -> Self {
        Self {
            fee_per_byte: 1, // 1 nanoBTH per byte
            cluster_curve: ClusterFactorCurve::default(),
            fee_per_memo: 100, // 100 nanoBTH per memo
        }
    }
}

impl FeeConfig {
    /// Compute the fee for a transaction based on size and cluster wealth.
    ///
    /// Formula: `fee = (fee_per_byte × tx_size_bytes × cluster_factor) +
    /// memo_fees`
    ///
    /// # Arguments
    /// * `tx_type` - The transaction type (Minting pays no fee)
    /// * `tx_size_bytes` - Size of the transaction in bytes
    /// * `cluster_wealth` - Total wealth of sender's cluster
    /// * `num_memos` - Number of outputs with encrypted memos
    ///
    /// # Returns
    /// The fee amount in nanoBTH
    pub fn compute_fee(
        &self,
        tx_type: TransactionType,
        tx_size_bytes: usize,
        cluster_wealth: u64,
        num_memos: usize,
    ) -> u64 {
        if tx_type == TransactionType::Minting {
            return 0;
        }

        // Get cluster factor (1x to 6x in 1000-scale fixed point)
        let cluster_factor = self.cluster_curve.factor(cluster_wealth);

        // Size-based fee: fee_per_byte × size × cluster_factor
        let size_fee = self
            .fee_per_byte
            .saturating_mul(tx_size_bytes as u64)
            .saturating_mul(cluster_factor)
            / ClusterFactorCurve::FACTOR_SCALE;

        // Memo fees: flat fee per memo (already accounts for 66 bytes storage)
        let memo_fee = self.fee_per_memo.saturating_mul(num_memos as u64);

        size_fee.saturating_add(memo_fee)
    }

    /// Compute the fee without memos (convenience method).
    pub fn compute_fee_no_memos(
        &self,
        tx_type: TransactionType,
        tx_size_bytes: usize,
        cluster_wealth: u64,
    ) -> u64 {
        self.compute_fee(tx_type, tx_size_bytes, cluster_wealth, 0)
    }

    /// Get the cluster factor for a given wealth level.
    ///
    /// Returns the multiplier as a fixed-point value (1000 = 1x, 6000 = 6x).
    pub fn cluster_factor(&self, cluster_wealth: u64) -> u64 {
        self.cluster_curve.factor(cluster_wealth)
    }

    /// Estimate fee for a typical transaction.
    ///
    /// Uses approximate sizes:
    /// - Hidden (CLSAG): ~4 KB typical
    /// - PqHidden (LION): ~65 KB typical
    pub fn estimate_typical_fee(
        &self,
        tx_type: TransactionType,
        cluster_wealth: u64,
        num_memos: usize,
    ) -> u64 {
        let typical_size = match tx_type {
            TransactionType::Hidden => 4_000,    // ~4 KB for CLSAG
            TransactionType::PqHidden => 65_000, // ~65 KB for LION
            TransactionType::Minting => 1_500,   // ~1.5 KB for minting
        };
        self.compute_fee(tx_type, typical_size, cluster_wealth, num_memos)
    }

    /// Compute the minimum fee for a transaction (alias for validation).
    pub fn minimum_fee(
        &self,
        tx_type: TransactionType,
        tx_size_bytes: usize,
        cluster_wealth: u64,
        num_memos: usize,
    ) -> u64 {
        self.compute_fee(tx_type, tx_size_bytes, cluster_wealth, num_memos)
    }

    /// Compute fee with dynamic base adjustment for congestion control.
    ///
    /// This is the full fee formula:
    /// ```text
    /// fee = dynamic_base × tx_size × cluster_factor + memo_fees
    /// ```
    ///
    /// # Arguments
    /// * `tx_type` - Transaction type (Minting pays no fee)
    /// * `tx_size_bytes` - Size of transaction in bytes
    /// * `cluster_wealth` - Total wealth of sender's cluster
    /// * `num_memos` - Number of outputs with encrypted memos
    /// * `dynamic_base` - Current dynamic fee base (1 to 100 nanoBTH/byte)
    ///
    /// # Returns
    /// Fee in nanoBTH
    pub fn compute_fee_with_dynamic_base(
        &self,
        tx_type: TransactionType,
        tx_size_bytes: usize,
        cluster_wealth: u64,
        num_memos: usize,
        dynamic_base: u64,
    ) -> u64 {
        if tx_type == TransactionType::Minting {
            return 0;
        }

        // Get cluster factor (1x to 6x in 1000-scale fixed point)
        let cluster_factor = self.cluster_curve.factor(cluster_wealth);

        // Size-based fee: dynamic_base × size × cluster_factor
        let size_fee = dynamic_base
            .saturating_mul(tx_size_bytes as u64)
            .saturating_mul(cluster_factor)
            / ClusterFactorCurve::FACTOR_SCALE;

        // Memo fees scale with dynamic base too
        let memo_base = std::cmp::max(self.fee_per_memo, dynamic_base * 100);
        let memo_fee = memo_base.saturating_mul(num_memos as u64);

        size_fee.saturating_add(memo_fee)
    }

    /// Compute minimum fee with dynamic base (alias for validation).
    pub fn minimum_fee_dynamic(
        &self,
        tx_type: TransactionType,
        tx_size_bytes: usize,
        cluster_wealth: u64,
        num_memos: usize,
        dynamic_base: u64,
    ) -> u64 {
        self.compute_fee_with_dynamic_base(
            tx_type,
            tx_size_bytes,
            cluster_wealth,
            num_memos,
            dynamic_base,
        )
    }
}

/// Cluster factor curve: maps cluster wealth to a multiplier (1x to 6x).
///
/// The fee formula is: `fee_per_byte × tx_size × cluster_factor`
/// This creates progressive taxation where wealthy clusters pay more.
///
/// Uses a sigmoid function:
/// factor(W) = 1 + 5 × sigmoid((W - w_mid) / steepness)
///
/// This ensures:
/// - Small clusters pay ~1x (just the base fee)
/// - Large clusters pay up to 6x (heavily taxed)
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
            factor_min: 1,        // 1x multiplier
            factor_max: 6,        // 6x multiplier
            w_mid: 10_000_000,    // inflection at 10M
            steepness: 5_000_000, // gradual transition
            background_factor: 1, // 1x for diffused coins
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
            (-6000, 131),  // sigmoid(-6) ≈ 0.002
            (-4000, 1180), // sigmoid(-4) ≈ 0.018
            (-2000, 7798), // sigmoid(-2) ≈ 0.119
            (0, 32768),    // sigmoid(0)  = 0.500
            (2000, 57738), // sigmoid(2)  ≈ 0.881
            (4000, 64356), // sigmoid(4)  ≈ 0.982
            (6000, 65405), // sigmoid(6)  ≈ 0.998
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
    fn test_size_based_fee() {
        let config = FeeConfig::default();

        // 4 KB transaction (typical CLSAG) with small cluster
        let fee_small = config.compute_fee(TransactionType::Hidden, 4_000, 0, 0);
        // fee = 1 nanoBTH/byte × 4000 bytes × ~1.6x factor ≈ 6400 nanoBTH
        assert!(
            fee_small >= 4_000 && fee_small <= 10_000,
            "4KB tx with small cluster: {fee_small}"
        );

        // Same transaction with large cluster (6x factor)
        let fee_large = config.compute_fee(TransactionType::Hidden, 4_000, 100_000_000, 0);
        assert!(
            fee_large > fee_small * 2,
            "Large cluster should pay more: {fee_large} > {fee_small}"
        );

        // LION transaction (65 KB) should cost ~16x more
        let fee_lion = config.compute_fee(TransactionType::PqHidden, 65_000, 0, 0);
        assert!(
            fee_lion > fee_small * 10,
            "LION should be much larger: {fee_lion} vs {fee_small}"
        );
    }

    #[test]
    fn test_minting_no_fee() {
        let config = FeeConfig::default();

        // Minting transactions always have 0 fee
        let fee = config.compute_fee(TransactionType::Minting, 1_500, 0, 0);
        assert_eq!(fee, 0);

        let fee_wealthy = config.compute_fee(TransactionType::Minting, 1_500, 100_000_000, 0);
        assert_eq!(fee_wealthy, 0);
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
    fn test_memo_fees() {
        let config = FeeConfig::default();

        // No memos
        let fee_no_memo = config.compute_fee(TransactionType::Hidden, 4_000, 0, 0);

        // 1 memo adds flat fee
        let fee_1_memo = config.compute_fee(TransactionType::Hidden, 4_000, 0, 1);
        assert_eq!(fee_1_memo, fee_no_memo + config.fee_per_memo);

        // 3 memos add 3x flat fee
        let fee_3_memo = config.compute_fee(TransactionType::Hidden, 4_000, 0, 3);
        assert_eq!(fee_3_memo, fee_no_memo + 3 * config.fee_per_memo);
    }

    #[test]
    fn test_typical_fee_estimates() {
        let config = FeeConfig::default();

        // Typical Hidden (CLSAG) transaction
        let hidden_fee = config.estimate_typical_fee(TransactionType::Hidden, 0, 0);
        assert!(
            hidden_fee > 0,
            "Hidden fee should be non-zero: {hidden_fee}"
        );

        // Typical PqHidden (LION) transaction should be much larger
        let pq_fee = config.estimate_typical_fee(TransactionType::PqHidden, 0, 0);
        assert!(
            pq_fee > hidden_fee * 10,
            "LION should be ~16x larger: {pq_fee} vs {hidden_fee}"
        );
    }

    #[test]
    fn test_progressive_fees() {
        let config = FeeConfig::default();
        let tx_size = 4_000; // 4 KB

        // Test that fees increase with cluster wealth
        let fee_small = config.compute_fee(TransactionType::Hidden, tx_size, 0, 0);
        let fee_mid = config.compute_fee(TransactionType::Hidden, tx_size, 10_000_000, 0);
        let fee_large = config.compute_fee(TransactionType::Hidden, tx_size, 100_000_000, 0);

        // Fees should increase monotonically
        assert!(
            fee_small < fee_mid && fee_mid < fee_large,
            "Fees should be progressive: {} < {} < {}",
            fee_small,
            fee_mid,
            fee_large
        );
    }

    #[test]
    fn test_size_proportional() {
        let config = FeeConfig {
            fee_per_byte: 1,
            cluster_curve: ClusterFactorCurve::flat(1), // 1x for predictable results
            fee_per_memo: 0,
        };

        // Double the size should double the fee
        let fee_1k = config.compute_fee(TransactionType::Hidden, 1_000, 0, 0);
        let fee_2k = config.compute_fee(TransactionType::Hidden, 2_000, 0, 0);
        assert_eq!(fee_2k, fee_1k * 2, "Fee should scale linearly with size");
    }
}

// ============================================================================
// Backwards-compatible FeeCurve for simulation code
// ============================================================================

/// Backwards-compatible fee curve that maps cluster wealth directly to fee
/// rate. Used by simulation code for comparing progressive vs flat fee
/// scenarios.
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
        Self {
            r_min_bps: 5,
            r_max_bps: 3000,
            w_mid: 10_000_000,
            steepness: 5_000_000,
            background_rate_bps: 10,
        }
    }

    pub fn flat(rate_bps: FeeRateBps) -> Self {
        Self {
            r_min_bps: rate_bps,
            r_max_bps: rate_bps,
            w_mid: 0,
            steepness: 1,
            background_rate_bps: rate_bps,
        }
    }

    pub fn is_flat(&self) -> bool {
        self.r_min_bps == self.r_max_bps
    }

    pub fn rate_bps(&self, cluster_wealth: u64) -> FeeRateBps {
        if self.is_flat() {
            return self.r_min_bps;
        }
        let curve = ClusterFactorCurve {
            factor_min: self.r_min_bps,
            factor_max: self.r_max_bps,
            w_mid: self.w_mid,
            steepness: self.steepness,
            background_factor: self.background_rate_bps,
        };
        let sigmoid = curve.sigmoid_approx(cluster_wealth);
        let range = self.r_max_bps.saturating_sub(self.r_min_bps);
        self.r_min_bps
            .saturating_add(((range as u64 * sigmoid) / ClusterFactorCurve::SIGMOID_SCALE) as u32)
    }

    pub fn compute_fee(&self, amount: u64, cluster_wealth: u64) -> (u64, u64) {
        let rate = self.rate_bps(cluster_wealth);
        let fee = (amount as u128 * rate as u128 / 10_000) as u64;
        (fee, amount.saturating_sub(fee))
    }
}

impl Default for FeeCurve {
    fn default() -> Self {
        Self::default_params()
    }
}
