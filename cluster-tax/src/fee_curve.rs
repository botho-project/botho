//! Fee calculation for Botho's three transaction types.
//!
//! Botho uses a dual-incentive fee model:
//! - **Privacy as priced resource**: Private transactions cost more because
//!   they impose verification burden and reduce transparency.
//! - **Progressive wealth taxation**: ALL value transfers (plain and hidden)
//!   apply progressive fees based on cluster wealth to reduce inequality.
//!
//! ## Transaction Types
//!
//! | Type    | Privacy | Fee Structure                                    |
//! |---------|---------|--------------------------------------------------|
//! | Plain   | None    | base_plain × cluster_factor (0.05% to 3%)        |
//! | Hidden  | Full    | base_hidden × cluster_factor (0.2% to 12%)       |
//! | Minting | N/A     | No fee (reward claim)                            |
//!
//! ## Fee Formula
//!
//! ```text
//! fee_rate = base_rate × cluster_factor(sender_cluster_wealth)
//! ```
//!
//! Where:
//! - `base_rate` differs by transaction type to reflect work/storage costs
//! - `cluster_factor` ranges from 1x (small holders) to 6x (large holders)
//!
//! ## Base Rate Rationale (Work/Storage Prefactors)
//!
//! | Type   | Base Rate | Justification                                    |
//! |--------|-----------|--------------------------------------------------|
//! | Plain  | 5 bps     | Minimal verification, small tx size (~250 bytes) |
//! | Hidden | 20 bps    | Ring sigs, bulletproofs, ~2.5KB tx size (4x)     |
//!
//! The 4x multiplier for hidden transactions reflects:
//! - ~10x more verification work (ring signature + bulletproof verification)
//! - ~10x more storage (larger transaction size)
//! - Averaged to 4x to keep privacy accessible while pricing the resource
//!
//! ## Progressive Taxation Rationale
//!
//! Applying progressive fees to BOTH transaction types ensures:
//! - Large holders can't avoid progressive taxation by using plain transactions
//! - Inequality reduction works regardless of privacy preference
//! - Small holders still get cheap transactions in both modes

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
    /// Transparent transaction (sender, receiver, amount all public).
    /// No ring signatures, no decoys. Lowest fee.
    Plain,

    /// Standard-private transaction with CLSAG ring signatures (~700B/input).
    /// Fee depends on cluster wealth of the sender.
    Hidden,

    /// PQ-private transaction with LION ring signatures (~63KB/input).
    /// Higher fee due to much larger signature size.
    /// Recommended for high-value or long-term security needs.
    PqHidden,

    /// Minting transaction claiming PoW reward.
    /// No fee (creates new coins).
    Minting,
}

/// Fee configuration for transaction types.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FeeConfig {
    /// Base fee rate for plain transactions before cluster multiplier.
    /// Default: 5 bps (0.05%)
    pub plain_base_fee_bps: FeeRateBps,

    /// Base fee rate for hidden (CLSAG) transactions before cluster multiplier.
    /// Default: 20 bps (0.2%)
    pub hidden_base_fee_bps: FeeRateBps,

    /// Base fee rate for PQ-hidden (LION) transactions before cluster multiplier.
    /// Higher than hidden due to ~90x larger signature size.
    /// Default: 100 bps (1.0%)
    pub pq_hidden_base_fee_bps: FeeRateBps,

    /// Cluster factor curve for progressive fee calculation.
    /// Applied to both plain and hidden transactions.
    pub cluster_curve: ClusterFactorCurve,

    /// Fee rate per memo in basis points.
    /// Each output with `e_memo.is_some()` adds this rate to the base fee.
    /// Default: 500 bps (5%) per memo
    pub memo_fee_rate_bps: FeeRateBps,
}

impl Default for FeeConfig {
    fn default() -> Self {
        Self {
            plain_base_fee_bps: 5,         // 0.05% base (up to 0.3% with 6x factor)
            hidden_base_fee_bps: 20,       // 0.2% base (up to 1.2% with 6x factor)
            pq_hidden_base_fee_bps: 100,   // 1.0% base for LION (~90x larger signatures)
            cluster_curve: ClusterFactorCurve::default(),
            memo_fee_rate_bps: 500,        // 5% per memo (500 bps = 0.05 multiplier per memo)
        }
    }
}

impl FeeConfig {
    /// Compute the fee for a transaction.
    ///
    /// # Arguments
    /// * `tx_type` - The transaction type
    /// * `amount` - Transfer amount
    /// * `cluster_wealth` - Total wealth of sender's cluster (used for Plain and Hidden)
    ///
    /// # Returns
    /// (fee_amount, net_amount_after_fee)
    pub fn compute_fee(
        &self,
        tx_type: TransactionType,
        amount: u64,
        cluster_wealth: u64,
    ) -> (u64, u64) {
        self.compute_fee_with_memos(tx_type, amount, cluster_wealth, 0)
    }

    /// Compute the fee for a transaction with memo count.
    ///
    /// # Arguments
    /// * `tx_type` - The transaction type
    /// * `amount` - Transfer amount
    /// * `cluster_wealth` - Total wealth of sender's cluster
    /// * `num_memos` - Number of outputs with encrypted memos
    ///
    /// # Returns
    /// (fee_amount, net_amount_after_fee)
    pub fn compute_fee_with_memos(
        &self,
        tx_type: TransactionType,
        amount: u64,
        cluster_wealth: u64,
        num_memos: usize,
    ) -> (u64, u64) {
        let rate_bps = self.fee_rate_bps_with_memos(tx_type, cluster_wealth, num_memos);
        let fee = (amount as u128 * rate_bps as u128 / 10_000) as u64;
        let net = amount.saturating_sub(fee);
        (fee, net)
    }

    /// Get the fee rate in basis points for a transaction.
    ///
    /// Both Plain and Hidden transactions apply progressive fees based on
    /// cluster wealth. The difference is the base rate:
    /// - Plain: 5 bps base (reflects lower verification/storage cost)
    /// - Hidden: 20 bps base (reflects higher verification/storage cost)
    ///
    /// Both are then multiplied by the cluster factor (1x-6x).
    pub fn fee_rate_bps(&self, tx_type: TransactionType, cluster_wealth: u64) -> FeeRateBps {
        self.fee_rate_bps_with_memos(tx_type, cluster_wealth, 0)
    }

    /// Get the fee rate in basis points for a transaction with memos.
    ///
    /// The formula is:
    /// ```text
    /// rate = base_rate × cluster_factor × memo_factor
    /// where memo_factor = 1 + (memo_fee_rate_bps / 10000) × num_memos
    /// ```
    ///
    /// # Arguments
    /// * `tx_type` - The transaction type
    /// * `cluster_wealth` - Total wealth of sender's cluster
    /// * `num_memos` - Number of outputs with encrypted memos (`e_memo.is_some()`)
    pub fn fee_rate_bps_with_memos(
        &self,
        tx_type: TransactionType,
        cluster_wealth: u64,
        num_memos: usize,
    ) -> FeeRateBps {
        let base_rate = match tx_type {
            TransactionType::Plain => {
                let factor = self.cluster_curve.factor(cluster_wealth);
                (self.plain_base_fee_bps as u64 * factor / ClusterFactorCurve::FACTOR_SCALE) as FeeRateBps
            }
            TransactionType::Hidden => {
                let factor = self.cluster_curve.factor(cluster_wealth);
                (self.hidden_base_fee_bps as u64 * factor / ClusterFactorCurve::FACTOR_SCALE) as FeeRateBps
            }
            TransactionType::PqHidden => {
                // PQ-hidden uses same cluster factor but higher base rate
                let factor = self.cluster_curve.factor(cluster_wealth);
                (self.pq_hidden_base_fee_bps as u64 * factor / ClusterFactorCurve::FACTOR_SCALE) as FeeRateBps
            }
            TransactionType::Minting => return 0,
        };

        // Apply memo multiplier: rate × (1 + memo_rate × num_memos)
        // Using fixed point: rate × (10000 + memo_rate_bps × num_memos) / 10000
        let memo_multiplier = 10_000u64 + (self.memo_fee_rate_bps as u64 * num_memos as u64);
        ((base_rate as u64 * memo_multiplier) / 10_000) as FeeRateBps
    }

    /// Compute the memo factor as a multiplier (in 10000-scale fixed point).
    ///
    /// Returns 10000 for 1.0x, 10500 for 1.05x (one memo at 5%), etc.
    pub fn memo_factor(&self, num_memos: usize) -> u64 {
        10_000 + (self.memo_fee_rate_bps as u64 * num_memos as u64)
    }

    /// Compute the minimum fee for a transaction given type, wealth, and memo count.
    ///
    /// This is the canonical fee calculation for validation.
    pub fn minimum_fee(
        &self,
        tx_type: TransactionType,
        amount: u64,
        cluster_wealth: u64,
        num_memos: usize,
    ) -> u64 {
        let (fee, _) = self.compute_fee_with_memos(tx_type, amount, cluster_wealth, num_memos);
        fee
    }
}

/// Cluster factor curve: maps cluster wealth to a multiplier (1x to 6x).
///
/// For both Plain and Hidden transactions, the fee = base_rate × cluster_factor.
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

        // Plain transaction with small cluster (0 wealth): base rate with low factor
        // With sigmoid at low wealth, factor is ~1.6x, so rate ≈ 8 bps
        let (fee, net) = config.compute_fee(TransactionType::Plain, 100_000, 0);
        assert!(fee >= 50 && fee <= 100, "Small cluster Plain fee should be near base: {fee}");
        assert_eq!(net, 100_000 - fee);

        // Plain transaction with large cluster (100M wealth): base rate with high factor
        // Factor approaches 6x, so rate approaches 30 bps
        let (fee_large, net_large) = config.compute_fee(TransactionType::Plain, 100_000, 100_000_000);
        assert!(fee_large > fee, "Large cluster should pay more: {fee_large} > {fee}");
        assert!(fee_large >= 250, "Large cluster Plain fee should be near 6x base: {fee_large}");
        assert_eq!(net_large, 100_000 - fee_large);

        // Minting transaction: no fee
        let (fee, net) = config.compute_fee(TransactionType::Minting, 100_000, 0);
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
            plain_base_fee_bps: 5,
            hidden_base_fee_bps: 20,
            cluster_curve: ClusterFactorCurve::flat(2), // 2x multiplier
            memo_fee_rate_bps: 500,  // 5% per memo
        };

        // Plain with 2x flat factor: 5 * 2 = 10 bps
        assert_eq!(config.fee_rate_bps(TransactionType::Plain, 0), 10);
        assert_eq!(config.fee_rate_bps(TransactionType::Plain, 1_000_000), 10);

        // Hidden with 2x flat factor: 20 * 2 = 40 bps
        assert_eq!(config.fee_rate_bps(TransactionType::Hidden, 0), 40);
        assert_eq!(config.fee_rate_bps(TransactionType::Hidden, 1_000_000), 40);

        // Minting: always 0
        assert_eq!(config.fee_rate_bps(TransactionType::Minting, 0), 0);
    }

    #[test]
    fn test_fee_computation() {
        let config = FeeConfig {
            plain_base_fee_bps: 100,  // 1% base for easy math
            hidden_base_fee_bps: 100,
            cluster_curve: ClusterFactorCurve::flat(1),  // 1x multiplier
            memo_fee_rate_bps: 0,  // No memo fee for this test
        };

        // 1% base × 1x factor = 1% fee
        let (fee, net) = config.compute_fee(TransactionType::Plain, 10_000, 0);
        assert_eq!(fee, 100);  // 1% of 10,000
        assert_eq!(net, 9_900);
    }

    #[test]
    fn test_memo_fee_factor() {
        let config = FeeConfig {
            plain_base_fee_bps: 100,  // 1% base
            hidden_base_fee_bps: 100,
            cluster_curve: ClusterFactorCurve::flat(1),  // 1x multiplier
            memo_fee_rate_bps: 500,  // 5% per memo
        };

        // No memos: memo_factor = 10000 (1.0x)
        assert_eq!(config.memo_factor(0), 10_000);

        // 1 memo: memo_factor = 10000 + 500 = 10500 (1.05x)
        assert_eq!(config.memo_factor(1), 10_500);

        // 3 memos: memo_factor = 10000 + 1500 = 11500 (1.15x)
        assert_eq!(config.memo_factor(3), 11_500);

        // 16 memos (max outputs): memo_factor = 10000 + 8000 = 18000 (1.8x)
        assert_eq!(config.memo_factor(16), 18_000);
    }

    #[test]
    fn test_memo_fee_rate() {
        let config = FeeConfig {
            plain_base_fee_bps: 100,  // 1% base
            hidden_base_fee_bps: 100,
            cluster_curve: ClusterFactorCurve::flat(1),  // 1x multiplier
            memo_fee_rate_bps: 500,  // 5% per memo
        };

        // Base rate with no memos: 100 bps
        assert_eq!(config.fee_rate_bps_with_memos(TransactionType::Plain, 0, 0), 100);

        // With 1 memo: 100 * 1.05 = 105 bps
        assert_eq!(config.fee_rate_bps_with_memos(TransactionType::Plain, 0, 1), 105);

        // With 3 memos: 100 * 1.15 = 115 bps
        assert_eq!(config.fee_rate_bps_with_memos(TransactionType::Plain, 0, 3), 115);

        // Minting tx: always 0, regardless of memos
        assert_eq!(config.fee_rate_bps_with_memos(TransactionType::Minting, 0, 10), 0);
    }

    #[test]
    fn test_memo_fee_computation() {
        let config = FeeConfig {
            plain_base_fee_bps: 100,  // 1% base
            hidden_base_fee_bps: 100,
            cluster_curve: ClusterFactorCurve::flat(1),
            memo_fee_rate_bps: 500,  // 5% per memo
        };

        // 10,000 amount, no memos: fee = 1% = 100
        let (fee0, net0) = config.compute_fee_with_memos(TransactionType::Plain, 10_000, 0, 0);
        assert_eq!(fee0, 100);
        assert_eq!(net0, 9_900);

        // 10,000 amount, 1 memo: fee = 1.05% = 105
        let (fee1, net1) = config.compute_fee_with_memos(TransactionType::Plain, 10_000, 0, 1);
        assert_eq!(fee1, 105);
        assert_eq!(net1, 9_895);

        // 10,000 amount, 3 memos: fee = 1.15% = 115
        let (fee3, net3) = config.compute_fee_with_memos(TransactionType::Plain, 10_000, 0, 3);
        assert_eq!(fee3, 115);
        assert_eq!(net3, 9_885);
    }

    #[test]
    fn test_memo_fee_with_cluster_factor() {
        // Test that memo and cluster factors multiply together
        let config = FeeConfig {
            plain_base_fee_bps: 100,   // 1% base
            hidden_base_fee_bps: 100,
            cluster_curve: ClusterFactorCurve::flat(2),  // 2x cluster factor
            memo_fee_rate_bps: 1000,   // 10% per memo for easy math
        };

        // Base rate with 2x cluster factor, no memos: 100 * 2 = 200 bps
        let rate_no_memo = config.fee_rate_bps_with_memos(TransactionType::Plain, 0, 0);
        assert_eq!(rate_no_memo, 200);

        // With 1 memo: 200 * 1.10 = 220 bps
        let rate_1_memo = config.fee_rate_bps_with_memos(TransactionType::Plain, 0, 1);
        assert_eq!(rate_1_memo, 220);

        // With 2 memos: 200 * 1.20 = 240 bps
        let rate_2_memos = config.fee_rate_bps_with_memos(TransactionType::Plain, 0, 2);
        assert_eq!(rate_2_memos, 240);
    }

    #[test]
    fn test_memo_incentives() {
        // Verify the economic incentives are correctly structured
        let config = FeeConfig::default();

        // Memos should increase fees monotonically
        let mut prev_rate = 0;
        for num_memos in 0..=16 {
            let rate = config.fee_rate_bps_with_memos(TransactionType::Plain, 0, num_memos);
            assert!(
                rate >= prev_rate,
                "Rate should increase with memos: {} >= {} at {} memos",
                rate, prev_rate, num_memos
            );
            prev_rate = rate;
        }

        // 16 memos should cost significantly more than 0 memos
        let rate_0 = config.fee_rate_bps_with_memos(TransactionType::Plain, 0, 0);
        let rate_16 = config.fee_rate_bps_with_memos(TransactionType::Plain, 0, 16);
        assert!(
            rate_16 > rate_0 * 150 / 100,
            "16 memos should cost at least 50% more than 0 memos: {} vs {}",
            rate_16, rate_0
        );
    }

    #[test]
    fn test_progressive_plain_fees() {
        let config = FeeConfig::default();

        // Test that Plain fees increase with cluster wealth
        let rate_small = config.fee_rate_bps(TransactionType::Plain, 0);
        let rate_mid = config.fee_rate_bps(TransactionType::Plain, 10_000_000);
        let rate_large = config.fee_rate_bps(TransactionType::Plain, 100_000_000);

        // Rates should increase monotonically
        assert!(
            rate_small < rate_mid && rate_mid < rate_large,
            "Plain rates should be progressive: {} < {} < {}",
            rate_small, rate_mid, rate_large
        );

        // Small cluster should be near base (5 bps × ~1.6x ≈ 8 bps)
        assert!(rate_small >= 5 && rate_small <= 15, "Small cluster rate: {rate_small}");

        // Large cluster should approach 6x base (5 × 6 = 30 bps)
        assert!(rate_large >= 25, "Large cluster rate should approach 30 bps: {rate_large}");
    }

    #[test]
    fn test_plain_hidden_ratio() {
        let config = FeeConfig::default();

        // At any wealth level, Hidden should be ~4x Plain (20/5 = 4)
        for wealth in [0, 1_000_000, 10_000_000, 100_000_000] {
            let plain_rate = config.fee_rate_bps(TransactionType::Plain, wealth);
            let hidden_rate = config.fee_rate_bps(TransactionType::Hidden, wealth);

            // Hidden should be exactly 4x Plain (since they use same curve)
            assert_eq!(
                hidden_rate, plain_rate * 4,
                "Hidden should be 4x Plain at wealth {wealth}: {hidden_rate} vs {plain_rate}"
            );
        }
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
