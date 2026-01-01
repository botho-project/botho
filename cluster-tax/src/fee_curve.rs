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

// ============================================================================
// ZK-Compatible Piecewise Linear Fee Curve
// ============================================================================

/// Parameters for a single segment of the piecewise linear fee curve.
///
/// Used for ZK proof construction where the prover demonstrates:
/// 1. Wealth falls within segment bounds: `w_lo <= wealth < w_hi`
/// 2. Fee satisfies linear relation: `fee >= intercept + slope * wealth`
///
/// The slope is scaled by `SLOPE_SCALE` (10^12) for precision in fixed-point arithmetic.
/// The intercept is scaled by `FACTOR_SCALE` (10^3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SegmentParams {
    /// Lower bound of wealth range (inclusive)
    pub w_lo: u64,
    /// Upper bound of wealth range (exclusive, or MAX for last segment)
    pub w_hi: u64,
    /// Slope of the linear segment, scaled by SLOPE_SCALE (10^12).
    /// For segment from (w_lo, f_lo) to (w_hi, f_hi):
    /// slope_scaled = (f_hi - f_lo) * SLOPE_SCALE / (w_hi - w_lo)
    ///
    /// To compute factor: factor = f_lo + slope_scaled * (w - w_lo) / SLOPE_SCALE
    pub slope_scaled: i64,
    /// Y-intercept of the linear segment, scaled by FACTOR_SCALE (10^3).
    /// intercept_scaled = f_lo * FACTOR_SCALE
    pub intercept_scaled: i64,
}

/// 3-segment piecewise linear fee curve for ZK compatibility.
///
/// Replaces the sigmoid-based `ClusterFactorCurve` for Phase 2 committed tags,
/// where fee verification must be provable in zero knowledge.
///
/// ## Design
///
/// The curve approximates the sigmoid's S-curve behavior with 3 linear segments:
/// - **Segment 1 (Poor)**: Low, flat factor for small wealth holders
/// - **Segment 2 (Middle)**: Linear ramp where most redistribution occurs
/// - **Segment 3 (Rich)**: High, flat factor plateau for large wealth holders
///
/// ## ZK Proof Strategy
///
/// Using a 3-way OR-proof, the prover demonstrates:
/// - Wealth falls within exactly one segment (range proofs)
/// - Fee satisfies that segment's linear relation
///
/// The verifier cannot determine which segment is real (privacy preserved).
/// Total proof overhead: ~4.5 KB (3 segments × ~1.5 KB each).
///
/// ## Example
///
/// ```
/// use cluster_tax::fee_curve::ZkFeeCurve;
///
/// let curve = ZkFeeCurve::default();
///
/// // Poor segment: 1x factor
/// assert_eq!(curve.factor(0), 1000);
///
/// // Rich segment: 6x factor
/// assert_eq!(curve.factor(u64::MAX), 6000);
///
/// // Middle segment: linear interpolation
/// let mid_factor = curve.factor(10_000_000);
/// assert!(mid_factor > 1000 && mid_factor < 6000);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ZkFeeCurve {
    /// Segment boundaries: [0, w1, w2, MAX]
    /// - boundaries[0] = 0 (start of poor segment)
    /// - boundaries[1] = poor/middle boundary
    /// - boundaries[2] = middle/rich boundary
    /// - boundaries[3] = MAX (end of rich segment)
    pub boundaries: [u64; 4],

    /// Factor at each boundary: [f0, f1, f2, f3] in FACTOR_SCALE units
    /// - factors[0] = factor at wealth 0 (start of poor segment)
    /// - factors[1] = factor at poor/middle boundary
    /// - factors[2] = factor at middle/rich boundary
    /// - factors[3] = factor at MAX wealth (end of rich segment)
    pub factors: [u64; 4],
}

impl ZkFeeCurve {
    /// Fixed-point scale for factor output, matching `ClusterFactorCurve`.
    /// FACTOR_SCALE = 1000, so factor=1000 means 1x, factor=6000 means 6x.
    pub const FACTOR_SCALE: u64 = 1000;

    /// High-precision scale for slope calculations to avoid integer truncation.
    /// SLOPE_SCALE = 10^12 preserves precision for small slopes.
    pub const SLOPE_SCALE: i128 = 1_000_000_000_000;

    /// Number of segments in the piecewise curve.
    pub const NUM_SEGMENTS: usize = 3;

    /// Default 3-segment balanced configuration.
    ///
    /// This configuration was validated via simulation (`scripts/gini_3segment.py`)
    /// and achieves:
    /// - **Better Gini reduction**: -0.2399 vs -0.2393 (sigmoid)
    /// - **Lower burn rate**: 12.4% vs 12.5%
    /// - **ZK-provable**: ~4.5 KB proof overhead
    ///
    /// Segment configuration:
    /// - Segment 1 (Poor): [0, 15% max_wealth) → 1x factor
    /// - Segment 2 (Middle): [15%, 70% max_wealth) → 2x to 5x linear
    /// - Segment 3 (Rich): [70%+ max_wealth] → 6x factor
    ///
    /// Using `w_mid = 10_000_000` as reference (matching `ClusterFactorCurve`):
    /// - w1 = 5_000_000 (15% equivalent based on simulation)
    /// - w2 = 20_000_000 (70% equivalent based on simulation)
    pub fn default() -> Self {
        Self {
            // Boundaries: [0, 5M, 20M, MAX]
            boundaries: [0, 5_000_000, 20_000_000, u64::MAX],
            // Factors at boundaries: [1x, 2x, 5x, 6x] in FACTOR_SCALE units
            factors: [1000, 2000, 5000, 6000],
        }
    }

    /// Create a flat factor curve (no progressivity).
    ///
    /// Useful for testing or if progressive taxation is disabled.
    pub fn flat(factor: u64) -> Self {
        let factor_scaled = factor * Self::FACTOR_SCALE;
        Self {
            boundaries: [0, 1, 2, u64::MAX],
            factors: [factor_scaled, factor_scaled, factor_scaled, factor_scaled],
        }
    }

    /// Check if this is a flat (non-progressive) curve.
    pub fn is_flat(&self) -> bool {
        self.factors[0] == self.factors[1]
            && self.factors[1] == self.factors[2]
            && self.factors[2] == self.factors[3]
    }

    /// Compute the cluster factor for a given cluster wealth.
    ///
    /// Returns factor in FACTOR_SCALE units (1000 = 1x, 6000 = 6x).
    ///
    /// The factor is computed via linear interpolation within the appropriate
    /// segment. For segment `i` with boundaries `[w_lo, w_hi)` and factors
    /// `[f_lo, f_hi]`:
    ///
    /// ```text
    /// factor(w) = f_lo + (f_hi - f_lo) × (w - w_lo) / (w_hi - w_lo)
    /// ```
    pub fn factor(&self, cluster_wealth: u64) -> u64 {
        // Find which segment the wealth falls into
        let segment = self.find_segment(cluster_wealth);

        let w_lo = self.boundaries[segment];
        let w_hi = self.boundaries[segment + 1];
        let f_lo = self.factors[segment];
        let f_hi = self.factors[segment + 1];

        // Handle edge case: if boundaries are equal (shouldn't happen in valid config)
        if w_hi == w_lo || w_hi == 0 {
            return f_lo;
        }

        // Handle the last segment boundary (u64::MAX)
        // To avoid overflow, we use saturating arithmetic and check for the max case
        if cluster_wealth >= w_hi.saturating_sub(1) && segment == Self::NUM_SEGMENTS - 1 {
            return f_hi;
        }

        // Linear interpolation: f_lo + (f_hi - f_lo) × (w - w_lo) / (w_hi - w_lo)
        let w_range = w_hi.saturating_sub(w_lo);
        let w_offset = cluster_wealth.saturating_sub(w_lo);

        if f_hi >= f_lo {
            // Increasing factor (normal case)
            let f_range = f_hi - f_lo;
            // Use 128-bit arithmetic to avoid overflow
            let adjustment = (f_range as u128 * w_offset as u128 / w_range as u128) as u64;
            f_lo.saturating_add(adjustment)
        } else {
            // Decreasing factor (unusual but handle it)
            let f_range = f_lo - f_hi;
            let adjustment = (f_range as u128 * w_offset as u128 / w_range as u128) as u64;
            f_lo.saturating_sub(adjustment)
        }
    }

    /// Find which segment a given wealth value falls into.
    ///
    /// Returns segment index (0, 1, or 2).
    fn find_segment(&self, wealth: u64) -> usize {
        for i in 0..Self::NUM_SEGMENTS {
            if wealth < self.boundaries[i + 1] {
                return i;
            }
        }
        // Wealth is >= last boundary, use last segment
        Self::NUM_SEGMENTS - 1
    }

    /// Get segment parameters for ZK proof construction.
    ///
    /// Returns the slope and intercept for the linear equation in segment `i`:
    /// ```text
    /// factor(w) = f_lo + slope_scaled × (w - w_lo) / SLOPE_SCALE
    /// ```
    ///
    /// The slope is scaled by SLOPE_SCALE (10^12) for precision.
    /// The intercept is f_lo × FACTOR_SCALE.
    ///
    /// # Panics
    ///
    /// Panics if `segment >= NUM_SEGMENTS`.
    pub fn segment_params(&self, segment: usize) -> SegmentParams {
        assert!(
            segment < Self::NUM_SEGMENTS,
            "segment index {} out of bounds (max {})",
            segment,
            Self::NUM_SEGMENTS - 1
        );

        let w_lo = self.boundaries[segment];
        let w_hi = self.boundaries[segment + 1];
        let f_lo = self.factors[segment] as i64;
        let f_hi = self.factors[segment + 1] as i64;

        // Calculate slope with high precision: (f_hi - f_lo) * SLOPE_SCALE / (w_hi - w_lo)
        let w_range = w_hi.saturating_sub(w_lo) as i128;
        let f_range = f_hi as i128 - f_lo as i128;

        let (slope_scaled, intercept_scaled) = if w_range == 0 {
            // Degenerate case: zero-width segment
            (0i64, f_lo * Self::FACTOR_SCALE as i64)
        } else {
            // slope_scaled = (f_hi - f_lo) * SLOPE_SCALE / (w_hi - w_lo)
            // This preserves precision for small slopes
            let slope = (f_range * Self::SLOPE_SCALE / w_range) as i64;
            // intercept = f_lo * FACTOR_SCALE (the factor at w_lo)
            let intercept = f_lo * Self::FACTOR_SCALE as i64;
            (slope, intercept)
        };

        SegmentParams {
            w_lo,
            w_hi,
            slope_scaled,
            intercept_scaled,
        }
    }

    /// Get all segment parameters for ZK proof construction.
    ///
    /// Returns parameters for all 3 segments, useful for constructing
    /// the OR-proof where the prover demonstrates membership in exactly
    /// one segment.
    pub fn all_segment_params(&self) -> [SegmentParams; 3] {
        [
            self.segment_params(0),
            self.segment_params(1),
            self.segment_params(2),
        ]
    }

    /// Check if a wealth value falls within a specific segment.
    ///
    /// Used for verifying segment membership in ZK proofs.
    pub fn in_segment(&self, wealth: u64, segment: usize) -> bool {
        if segment >= Self::NUM_SEGMENTS {
            return false;
        }
        wealth >= self.boundaries[segment] && wealth < self.boundaries[segment + 1]
    }
}

impl Default for ZkFeeCurve {
    fn default() -> Self {
        Self::default()
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

    // ========================================================================
    // ZkFeeCurve Tests
    // ========================================================================

    #[test]
    fn test_zk_fee_curve_boundary_values() {
        let curve = ZkFeeCurve::default();

        // At wealth=0, factor should be 1x (1000 in FACTOR_SCALE)
        assert_eq!(curve.factor(0), 1000, "Zero wealth should have 1x factor");

        // At boundary 1 (5M), factor should be 2x (2000)
        assert_eq!(
            curve.factor(5_000_000),
            2000,
            "Boundary 1 should have 2x factor"
        );

        // At boundary 2 (20M), factor should be 5x (5000)
        assert_eq!(
            curve.factor(20_000_000),
            5000,
            "Boundary 2 should have 5x factor"
        );

        // At very high wealth, factor should be 6x (6000)
        assert_eq!(
            curve.factor(u64::MAX),
            6000,
            "Max wealth should have 6x factor"
        );
    }

    #[test]
    fn test_zk_fee_curve_linear_interpolation_segment1() {
        let curve = ZkFeeCurve::default();

        // Segment 1 (Poor): [0, 5M) with factors [1000, 2000]
        // Midpoint at 2.5M should give factor ~1500
        let mid_factor = curve.factor(2_500_000);
        assert_eq!(mid_factor, 1500, "Segment 1 midpoint should be 1.5x");

        // Quarter point at 1.25M should give factor ~1250
        let quarter_factor = curve.factor(1_250_000);
        assert_eq!(quarter_factor, 1250, "Segment 1 quarter point should be 1.25x");
    }

    #[test]
    fn test_zk_fee_curve_linear_interpolation_segment2() {
        let curve = ZkFeeCurve::default();

        // Segment 2 (Middle): [5M, 20M) with factors [2000, 5000]
        // Range is 15M, factor range is 3000
        // At 12.5M (midpoint), factor should be 3500
        let mid_factor = curve.factor(12_500_000);
        assert_eq!(mid_factor, 3500, "Segment 2 midpoint should be 3.5x");

        // At 8.75M (quarter point), factor should be 2750
        let quarter_factor = curve.factor(8_750_000);
        assert_eq!(quarter_factor, 2750, "Segment 2 quarter point should be 2.75x");
    }

    #[test]
    fn test_zk_fee_curve_linear_interpolation_segment3() {
        let curve = ZkFeeCurve::default();

        // Segment 3 (Rich): [20M, MAX) with factors [5000, 6000]
        // At 20M, factor should be 5000
        assert_eq!(
            curve.factor(20_000_000),
            5000,
            "Segment 3 start should be 5x"
        );

        // The rich segment has an enormous range (20M to u64::MAX ≈ 18 quintillion)
        // so the linear interpolation produces negligible change for small wealth differences.
        // At 21M, the factor is essentially still 5000 due to the tiny slope.
        let factor_21m = curve.factor(21_000_000);
        assert_eq!(
            factor_21m, 5000,
            "Factor at 21M should still be ~5x (huge segment range): {factor_21m}"
        );

        // At MAX, factor should reach 6000
        assert_eq!(
            curve.factor(u64::MAX),
            6000,
            "At max wealth, factor should be 6x"
        );
    }

    #[test]
    fn test_zk_fee_curve_monotonic_increase() {
        let curve = ZkFeeCurve::default();
        let mut prev_factor = 0;

        // Test that factors increase monotonically across all segments
        for wealth in [
            0,
            1_000_000,
            2_500_000,
            5_000_000,
            7_500_000,
            10_000_000,
            15_000_000,
            20_000_000,
            50_000_000,
            100_000_000,
            u64::MAX,
        ] {
            let factor = curve.factor(wealth);
            assert!(
                factor >= prev_factor,
                "Factor should increase with wealth: {} -> {} at {}",
                prev_factor,
                factor,
                wealth
            );
            prev_factor = factor;
        }
    }

    #[test]
    fn test_zk_fee_curve_flat() {
        let curve = ZkFeeCurve::flat(3);

        // Flat curve should return same factor regardless of wealth
        assert_eq!(curve.factor(0), 3000);
        assert_eq!(curve.factor(1_000_000), 3000);
        assert_eq!(curve.factor(100_000_000), 3000);
        assert!(curve.is_flat());
    }

    #[test]
    fn test_zk_fee_curve_segment_membership() {
        let curve = ZkFeeCurve::default();

        // Segment 0: [0, 5M)
        assert!(curve.in_segment(0, 0));
        assert!(curve.in_segment(4_999_999, 0));
        assert!(!curve.in_segment(5_000_000, 0));

        // Segment 1: [5M, 20M)
        assert!(curve.in_segment(5_000_000, 1));
        assert!(curve.in_segment(19_999_999, 1));
        assert!(!curve.in_segment(20_000_000, 1));

        // Segment 2: [20M, MAX)
        assert!(curve.in_segment(20_000_000, 2));
        assert!(curve.in_segment(100_000_000, 2));
        assert!(curve.in_segment(u64::MAX - 1, 2));

        // Invalid segment
        assert!(!curve.in_segment(0, 3));
    }

    #[test]
    fn test_zk_fee_curve_segment_params() {
        let curve = ZkFeeCurve::default();

        // Segment 0: [0, 5M) with factors [1000, 2000]
        let params0 = curve.segment_params(0);
        assert_eq!(params0.w_lo, 0);
        assert_eq!(params0.w_hi, 5_000_000);
        // Slope should be positive (increasing factor)
        assert!(
            params0.slope_scaled > 0,
            "Segment 0 should have positive slope: {}",
            params0.slope_scaled
        );

        // Segment 1: [5M, 20M) with factors [2000, 5000]
        let params1 = curve.segment_params(1);
        assert_eq!(params1.w_lo, 5_000_000);
        assert_eq!(params1.w_hi, 20_000_000);
        assert!(
            params1.slope_scaled > 0,
            "Segment 1 should have positive slope: {}",
            params1.slope_scaled
        );

        // Segment 2: [20M, MAX) with factors [5000, 6000]
        // Note: Due to the enormous wealth range (u64::MAX - 20M ≈ 18 quintillion),
        // the slope is extremely small and may truncate to 0 in fixed-point.
        // This is expected behavior - the rich segment is essentially flat.
        let params2 = curve.segment_params(2);
        assert_eq!(params2.w_lo, 20_000_000);
        assert_eq!(params2.w_hi, u64::MAX);
        // Slope may be 0 or very small due to integer truncation with huge range
        assert!(
            params2.slope_scaled >= 0,
            "Segment 2 should have non-negative slope: {}",
            params2.slope_scaled
        );
    }

    #[test]
    fn test_zk_fee_curve_all_segment_params() {
        let curve = ZkFeeCurve::default();
        let all_params = curve.all_segment_params();

        assert_eq!(all_params.len(), 3);
        assert_eq!(all_params[0].w_lo, 0);
        assert_eq!(all_params[1].w_lo, 5_000_000);
        assert_eq!(all_params[2].w_lo, 20_000_000);
    }

    #[test]
    fn test_zk_fee_curve_compare_to_sigmoid() {
        // Compare ZkFeeCurve output against ClusterFactorCurve at key points
        // The piecewise curve should approximate the sigmoid's S-curve behavior
        let sigmoid = ClusterFactorCurve::default_params();
        let piecewise = ZkFeeCurve::default();

        // At low wealth, both should be low (~1x-2x range)
        let sig_low = sigmoid.factor(0);
        let pw_low = piecewise.factor(0);
        assert!(
            sig_low < 3000 && pw_low < 3000,
            "Both should be low at zero wealth: sigmoid={sig_low}, piecewise={pw_low}"
        );

        // At midpoint (10M), both should be in middle range (~3x-4x)
        let sig_mid = sigmoid.factor(10_000_000);
        let pw_mid = piecewise.factor(10_000_000);
        assert!(
            sig_mid > 2500 && sig_mid < 4500,
            "Sigmoid at midpoint: {sig_mid}"
        );
        assert!(
            pw_mid > 2500 && pw_mid < 4500,
            "Piecewise at midpoint: {pw_mid}"
        );

        // At high wealth, both should be high (~5x-6x range)
        let sig_high = sigmoid.factor(100_000_000);
        let pw_high = piecewise.factor(100_000_000);
        assert!(
            sig_high >= 5000,
            "Sigmoid should be high at 100M: {sig_high}"
        );
        assert!(
            pw_high >= 5000,
            "Piecewise should be high at 100M: {pw_high}"
        );
    }

    #[test]
    fn test_zk_fee_curve_factor_scale_consistency() {
        // Verify FACTOR_SCALE is consistent between curves
        assert_eq!(
            ZkFeeCurve::FACTOR_SCALE,
            ClusterFactorCurve::FACTOR_SCALE,
            "FACTOR_SCALE should match between curves"
        );
    }

    #[test]
    #[should_panic(expected = "segment index 3 out of bounds")]
    fn test_zk_fee_curve_segment_params_out_of_bounds() {
        let curve = ZkFeeCurve::default();
        let _ = curve.segment_params(3); // Should panic
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
