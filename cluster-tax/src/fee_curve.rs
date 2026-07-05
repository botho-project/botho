//! Fee calculation for Botho's transaction types.
//!
//! Botho uses a size-based fee model with progressive wealth taxation and
//! superlinear output fees to prevent UTXO farming attacks:
//!
//! ```text
//! fee = fee_per_byte × tx_size × cluster_factor × output_penalty + memo_fees
//! ```
//!
//! where `output_penalty = min(output_count, cap)^exponent`
//!
//! ## Transaction Types
//!
//! | Type     | Signature Type | Typical Size | Fee Rate                              |
//! |----------|----------------|--------------|---------------------------------------|
//! | Transfer | CLSAG          | ~4 KB        | size × cluster_factor × output²       |
//! | Minting  | N/A            | ~1.5 KB      | No fee                                |
//!
//! ## Fee Components
//!
//! 1. **Size-based fee**: Larger transactions pay more (proportional to bytes)
//! 2. **Progressive multiplier**: Cluster factor ranges from 1x to 6x based on
//!    the sender's cluster wealth, ensuring wealthy clusters pay more
//! 3. **Superlinear output fee**: Quadratic penalty for multiple outputs
//!    prevents UTXO farming (splitting coins to game lottery systems)
//! 4. **Memo fees**: Flat fee per encrypted memo
//!
//! ## Superlinear Output Fees
//!
//! The quadratic output fee makes mass splitting economically unfeasible:
//!
//! | Outputs | Penalty (default) | Example Fee (base=1000) |
//! |---------|-------------------|-------------------------|
//! | 1       | 1x                | 1,000                   |
//! | 2       | 4x                | 4,000 (normal tx)       |
//! | 5       | 25x               | 25,000                  |
//! | 10      | 100x (capped)     | 100,000                 |
//! | 20      | 100x (capped)     | 100,000                 |
//!
//! The cap at 10 outputs protects legitimate batch transactions while
//! making UTXO farming prohibitively expensive.
//!
//! ## Size Rationale
//!
//! | Type     | Input Size      | Output Size | Typical Total |
//! |----------|-----------------|-------------|---------------|
//! | Transfer | ~700 B (CLSAG)  | ~1.2 KB     | ~4 KB         |
//!
//! All private transfers use CLSAG ring signatures for sender anonymity.
//!
//! ## Progressive Taxation
//!
//! The cluster factor ensures wealthy clusters pay higher fees:
//! - Small clusters: 1x multiplier (just size fee)
//! - Large clusters: up to 6x multiplier
//! - Sigmoid curve provides smooth transition
//!
//! ## Dust Prevention
//!
//! Outputs below `min_output_value` (default: 1M picocredits = 1e-6 BTH) are
//! considered dust and should be rejected. This prevents attacks that create
//! many tiny UTXOs.

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
    /// Private transfer with CLSAG ring signatures (~700B/input).
    /// Fee = size × cluster_factor.
    Hidden,

    /// Minting transaction claiming PoW reward.
    /// No fee (creates new coins).
    Minting,
}

/// Fee configuration for transaction types.
///
/// Fees are calculated as:
/// ```text
/// fee = fee_per_byte × tx_size × cluster_factor × output_penalty + memo_fees
/// ```
///
/// where `output_penalty = min(output_count,
/// output_count_cap)^output_fee_exponent`
///
/// This superlinear output fee prevents UTXO farming attacks where attackers
/// split coins into many small UTXOs to game lottery systems.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FeeConfig {
    /// Fee per byte in picocredits.
    /// Default: 1 picocredits per byte
    pub fee_per_byte: u64,

    /// Cluster factor curve for progressive fee calculation.
    /// Multiplier ranges from 1x (small clusters) to 6x (large clusters).
    pub cluster_curve: ClusterFactorCurve,

    /// Fee per memo in picocredits.
    /// Each output with `e_memo.is_some()` adds this flat fee.
    /// Default: 100 picocredits per memo (66 bytes stored forever)
    pub fee_per_memo: u64,

    /// Exponent for superlinear output fee calculation.
    /// Fee multiplier = output_count^exponent
    ///
    /// - 1.0 = linear (no penalty for multiple outputs)
    /// - 2.0 = quadratic (default, 10 outputs costs 100x)
    ///
    /// Stored as fixed-point: actual_exponent = output_fee_exponent_scaled /
    /// 1000 Default: 2000 (= 2.0 quadratic) to prevent UTXO farming
    /// attacks.
    pub output_fee_exponent_scaled: u32,

    /// Maximum output count to apply the exponent to.
    /// Prevents excessive fees for legitimate batch transactions.
    ///
    /// For output counts above this cap, the penalty is capped:
    /// `penalty = cap^exponent` (not `output_count^exponent`)
    ///
    /// Default: 10 (caps penalty at 100x for quadratic exponent)
    pub output_count_cap: u32,

    /// Minimum value per output in picocredits.
    /// Outputs below this value are considered dust and rejected.
    /// This prevents dust attacks that create many tiny UTXOs.
    ///
    /// Default: 1_000_000 picocredits (1e-6 BTH; these size/dust fee constants
    /// are relative and were not recalibrated for the pico scale — see #626)
    pub min_output_value: u64,
}

/// Scale for output fee exponent fixed-point representation.
/// EXPONENT_SCALE = 1000, so exponent_scaled=2000 means 2.0.
pub const OUTPUT_FEE_EXPONENT_SCALE: u32 = 1000;

impl Default for FeeConfig {
    fn default() -> Self {
        Self {
            fee_per_byte: 1, // 1 picocredits per byte
            cluster_curve: ClusterFactorCurve::default(),
            fee_per_memo: 100,                // 100 picocredits per memo
            output_fee_exponent_scaled: 2000, // 2.0 (quadratic)
            output_count_cap: 10,             // Cap at 10 outputs
            min_output_value: 1_000_000,      // 0.001 BTH minimum
        }
    }
}

impl FeeConfig {
    /// Compute the output penalty multiplier for superlinear fees.
    ///
    /// Formula: `penalty = min(output_count, cap)^exponent`
    ///
    /// Returns the multiplier in OUTPUT_PENALTY_SCALE (1000 = 1x).
    ///
    /// # Determinism (consensus path)
    /// This is the per-tx minimum-fee path, so it must be **bit-reproducible**
    /// across platforms/compilers — floating point (`f64::powf`) is not, so it
    /// is forbidden here. The computation is integer-only.
    ///
    /// # Exponent policy
    /// The consensus fee exponent MUST be an integer (i.e.
    /// `output_fee_exponent_scaled` MUST be a multiple of
    /// `OUTPUT_FEE_EXPONENT_SCALE`). A fractional exponent has no current
    /// consumer and cannot be reproduced bit-identically across platforms, so
    /// any fractional part is **clamped down** to the nearest integer exponent
    /// here (integer division of `output_fee_exponent_scaled`). Callers should
    /// validate configs with [`FeeConfig::validate`] before use; the clamp is a
    /// defense-in-depth fallback, not a supported configuration mode.
    ///
    /// For integer exponents this is **bit-identical** to the previous
    /// `f64::powf` implementation across the entire in-domain
    /// `[0, output_count_cap]` (the product is exact, so the old truncating
    /// `(x * 1000.0) as u64` was already a no-op rounding step).
    ///
    /// # Examples
    /// With default config (exponent=2.0, cap=10):
    /// - 0 outputs: 0^2 = 0x (0)
    /// - 1 output: 1^2 = 1x (1000)
    /// - 2 outputs: 2^2 = 4x (4000)
    /// - 5 outputs: 5^2 = 25x (25000)
    /// - 10 outputs: 10^2 = 100x (100000)
    /// - 20 outputs: 10^2 = 100x (capped at 100000)
    pub fn output_penalty(&self, output_count: usize) -> u64 {
        // Apply cap. u128 staging keeps `count^exp` from overflowing before the
        // final saturating narrow to u64.
        let effective_count = std::cmp::min(output_count as u32, self.output_count_cap) as u128;

        // Integer exponent only (see "Exponent policy" above): floor the
        // fixed-point exponent to a whole number via integer division.
        let exponent = self.output_fee_exponent_scaled / OUTPUT_FEE_EXPONENT_SCALE;

        // penalty = count^exponent, returned in OUTPUT_FEE_EXPONENT_SCALE
        // fixed-point. `checked_pow` saturates an overflowing power to u128::MAX
        // (then narrowed below) instead of panicking on adversarial configs.
        let penalty = effective_count
            .checked_pow(exponent)
            .unwrap_or(u128::MAX)
            .saturating_mul(OUTPUT_FEE_EXPONENT_SCALE as u128);

        penalty.min(u64::MAX as u128) as u64
    }

    /// Validate that this fee config is usable on the consensus fee path.
    ///
    /// The only consensus-relevant constraint today is the integer-exponent
    /// policy documented on [`FeeConfig::output_penalty`]: a fractional
    /// `output_fee_exponent_scaled` cannot be reproduced bit-identically across
    /// platforms and has no current consumer. Returns `Err` describing the
    /// offending field rather than silently clamping, so config loaders can
    /// reject it up front.
    pub fn validate(&self) -> Result<(), String> {
        if self.output_fee_exponent_scaled % OUTPUT_FEE_EXPONENT_SCALE != 0 {
            return Err(format!(
                "output_fee_exponent_scaled ({}) must be a multiple of \
                 OUTPUT_FEE_EXPONENT_SCALE ({}); fractional fee exponents are \
                 not bit-reproducible and not allowed on the consensus fee path",
                self.output_fee_exponent_scaled, OUTPUT_FEE_EXPONENT_SCALE
            ));
        }
        Ok(())
    }

    /// Compute the fee for a transaction based on size, cluster wealth, and
    /// output count.
    ///
    /// Formula:
    /// ```text
    /// fee = (fee_per_byte × tx_size_bytes × cluster_factor × output_penalty) + memo_fees
    /// ```
    ///
    /// where `output_penalty = min(output_count, cap)^exponent`
    ///
    /// # Arguments
    /// * `tx_type` - The transaction type (Minting pays no fee)
    /// * `tx_size_bytes` - Size of the transaction in bytes
    /// * `cluster_wealth` - Total wealth of sender's cluster
    /// * `num_outputs` - Number of transaction outputs (for superlinear
    ///   penalty)
    /// * `num_memos` - Number of outputs with encrypted memos
    ///
    /// # Returns
    /// The fee amount in picocredits
    pub fn compute_fee_with_outputs(
        &self,
        tx_type: TransactionType,
        tx_size_bytes: usize,
        cluster_wealth: u128,
        num_outputs: usize,
        num_memos: usize,
    ) -> u64 {
        if tx_type == TransactionType::Minting {
            return 0;
        }

        // Get cluster factor (1x to 6x in 1000-scale fixed point)
        let cluster_factor = self.cluster_curve.factor(cluster_wealth);

        // Get output penalty (capped quadratic by default)
        let output_penalty = self.output_penalty(num_outputs);

        // Size-based fee: fee_per_byte × size × cluster_factor × output_penalty
        // Both cluster_factor and output_penalty are scaled by 1000
        let size_fee = self
            .fee_per_byte
            .saturating_mul(tx_size_bytes as u64)
            .saturating_mul(cluster_factor)
            .saturating_mul(output_penalty)
            / (ClusterFactorCurve::FACTOR_SCALE * OUTPUT_FEE_EXPONENT_SCALE as u64);

        // Memo fees: flat fee per memo (already accounts for 66 bytes storage)
        let memo_fee = self.fee_per_memo.saturating_mul(num_memos as u64);

        size_fee.saturating_add(memo_fee)
    }

    /// Compute the fee for a transaction (legacy API without output count).
    ///
    /// This method assumes 2 outputs (standard transfer with change).
    /// For full control, use `compute_fee_with_outputs`.
    ///
    /// Formula: `fee = (fee_per_byte × tx_size_bytes × cluster_factor ×
    /// output_penalty) + memo_fees`
    ///
    /// # Arguments
    /// * `tx_type` - The transaction type (Minting pays no fee)
    /// * `tx_size_bytes` - Size of the transaction in bytes
    /// * `cluster_wealth` - Total wealth of sender's cluster
    /// * `num_memos` - Number of outputs with encrypted memos
    ///
    /// # Returns
    /// The fee amount in picocredits
    pub fn compute_fee(
        &self,
        tx_type: TransactionType,
        tx_size_bytes: usize,
        cluster_wealth: u128,
        num_memos: usize,
    ) -> u64 {
        // Default to 2 outputs (standard transfer: payment + change)
        self.compute_fee_with_outputs(tx_type, tx_size_bytes, cluster_wealth, 2, num_memos)
    }

    /// Compute the fee without memos (convenience method).
    pub fn compute_fee_no_memos(
        &self,
        tx_type: TransactionType,
        tx_size_bytes: usize,
        cluster_wealth: u128,
    ) -> u64 {
        self.compute_fee(tx_type, tx_size_bytes, cluster_wealth, 0)
    }

    /// Check if an output value is above the minimum threshold.
    ///
    /// Returns `true` if the value is acceptable, `false` if it's dust.
    pub fn is_output_above_dust(&self, value: u64) -> bool {
        value >= self.min_output_value
    }

    /// Get the minimum output value threshold.
    pub fn dust_threshold(&self) -> u64 {
        self.min_output_value
    }

    /// Get the cluster factor for a given wealth level.
    ///
    /// Returns the multiplier as a fixed-point value (1000 = 1x, 6000 = 6x).
    pub fn cluster_factor(&self, cluster_wealth: u128) -> u64 {
        self.cluster_curve.factor(cluster_wealth)
    }

    /// Estimate fee for a typical transaction.
    ///
    /// Uses approximate sizes:
    /// - Hidden (CLSAG): ~4 KB typical
    /// - Minting: ~1.5 KB typical
    ///
    /// Assumes 2 outputs (standard payment + change).
    /// For multi-output estimation, use `estimate_fee_with_outputs`.
    pub fn estimate_typical_fee(
        &self,
        tx_type: TransactionType,
        cluster_wealth: u128,
        num_memos: usize,
    ) -> u64 {
        self.estimate_fee_with_outputs(tx_type, cluster_wealth, 2, num_memos)
    }

    /// Estimate fee for a transaction with specified output count.
    ///
    /// Uses approximate sizes:
    /// - Hidden (CLSAG): ~4 KB + ~1.2 KB per additional output
    /// - Minting: ~1.5 KB typical
    pub fn estimate_fee_with_outputs(
        &self,
        tx_type: TransactionType,
        cluster_wealth: u128,
        num_outputs: usize,
        num_memos: usize,
    ) -> u64 {
        let typical_size = match tx_type {
            TransactionType::Hidden => {
                // Base size ~2.5 KB + ~1.2 KB per output
                2_500 + num_outputs * 1_200
            }
            TransactionType::Minting => 1_500, // ~1.5 KB for minting
        };
        self.compute_fee_with_outputs(
            tx_type,
            typical_size,
            cluster_wealth,
            num_outputs,
            num_memos,
        )
    }

    /// Compute the minimum fee for a transaction (alias for validation).
    ///
    /// Assumes 2 outputs. For multi-output validation, use
    /// `minimum_fee_with_outputs`.
    pub fn minimum_fee(
        &self,
        tx_type: TransactionType,
        tx_size_bytes: usize,
        cluster_wealth: u128,
        num_memos: usize,
    ) -> u64 {
        self.compute_fee(tx_type, tx_size_bytes, cluster_wealth, num_memos)
    }

    /// Compute the minimum fee for a transaction with specified output count.
    pub fn minimum_fee_with_outputs(
        &self,
        tx_type: TransactionType,
        tx_size_bytes: usize,
        cluster_wealth: u128,
        num_outputs: usize,
        num_memos: usize,
    ) -> u64 {
        self.compute_fee_with_outputs(
            tx_type,
            tx_size_bytes,
            cluster_wealth,
            num_outputs,
            num_memos,
        )
    }

    /// Compute fee with dynamic base adjustment for congestion control.
    ///
    /// This is the full fee formula:
    /// ```text
    /// fee = dynamic_base × tx_size × cluster_factor × output_penalty + memo_fees
    /// ```
    ///
    /// Assumes 2 outputs. For multi-output, use
    /// `compute_fee_with_dynamic_base_and_outputs`.
    ///
    /// # Arguments
    /// * `tx_type` - Transaction type (Minting pays no fee)
    /// * `tx_size_bytes` - Size of transaction in bytes
    /// * `cluster_wealth` - Total wealth of sender's cluster
    /// * `num_memos` - Number of outputs with encrypted memos
    /// * `dynamic_base` - Current dynamic fee base (1 to 100 picocredits/byte)
    ///
    /// # Returns
    /// Fee in picocredits
    pub fn compute_fee_with_dynamic_base(
        &self,
        tx_type: TransactionType,
        tx_size_bytes: usize,
        cluster_wealth: u128,
        num_memos: usize,
        dynamic_base: u64,
    ) -> u64 {
        // Default to 2 outputs
        self.compute_fee_with_dynamic_base_and_outputs(
            tx_type,
            tx_size_bytes,
            cluster_wealth,
            2,
            num_memos,
            dynamic_base,
        )
    }

    /// Compute fee with dynamic base and specified output count.
    ///
    /// Full fee formula:
    /// ```text
    /// fee = dynamic_base × tx_size × cluster_factor × output_penalty + memo_fees
    /// ```
    ///
    /// # Arguments
    /// * `tx_type` - Transaction type (Minting pays no fee)
    /// * `tx_size_bytes` - Size of transaction in bytes
    /// * `cluster_wealth` - Total wealth of sender's cluster
    /// * `num_outputs` - Number of transaction outputs
    /// * `num_memos` - Number of outputs with encrypted memos
    /// * `dynamic_base` - Current dynamic fee base (1 to 100 picocredits/byte)
    ///
    /// # Returns
    /// Fee in picocredits
    pub fn compute_fee_with_dynamic_base_and_outputs(
        &self,
        tx_type: TransactionType,
        tx_size_bytes: usize,
        cluster_wealth: u128,
        num_outputs: usize,
        num_memos: usize,
        dynamic_base: u64,
    ) -> u64 {
        if tx_type == TransactionType::Minting {
            return 0;
        }

        // Get cluster factor (1x to 6x in 1000-scale fixed point)
        let cluster_factor = self.cluster_curve.factor(cluster_wealth);

        // Get output penalty (capped quadratic by default)
        let output_penalty = self.output_penalty(num_outputs);

        // Size-based fee: dynamic_base × size × cluster_factor × output_penalty
        let size_fee = dynamic_base
            .saturating_mul(tx_size_bytes as u64)
            .saturating_mul(cluster_factor)
            .saturating_mul(output_penalty)
            / (ClusterFactorCurve::FACTOR_SCALE * OUTPUT_FEE_EXPONENT_SCALE as u64);

        // Memo fees scale with dynamic base too
        let memo_base = std::cmp::max(self.fee_per_memo, dynamic_base * 100);
        let memo_fee = memo_base.saturating_mul(num_memos as u64);

        size_fee.saturating_add(memo_fee)
    }

    /// Compute minimum fee with dynamic base (alias for validation).
    ///
    /// Assumes 2 outputs. For multi-output, use
    /// `minimum_fee_dynamic_with_outputs`.
    pub fn minimum_fee_dynamic(
        &self,
        tx_type: TransactionType,
        tx_size_bytes: usize,
        cluster_wealth: u128,
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

    /// Compute minimum fee with dynamic base and specified output count.
    pub fn minimum_fee_dynamic_with_outputs(
        &self,
        tx_type: TransactionType,
        tx_size_bytes: usize,
        cluster_wealth: u128,
        num_outputs: usize,
        num_memos: usize,
        dynamic_base: u64,
    ) -> u64 {
        self.compute_fee_with_dynamic_base_and_outputs(
            tx_type,
            tx_size_bytes,
            cluster_wealth,
            num_outputs,
            num_memos,
            dynamic_base,
        )
    }

    /// Create a fee config with no output penalty (linear fees).
    ///
    /// Useful for testing or when output penalties should be disabled.
    pub fn with_linear_output_fees() -> Self {
        Self {
            output_fee_exponent_scaled: 1000, // 1.0 = linear
            ..Self::default()
        }
    }

    /// Create a fee config with custom output fee parameters.
    pub fn with_output_fee_params(exponent: f64, cap: u32, min_output: u64) -> Self {
        Self {
            output_fee_exponent_scaled: (exponent * OUTPUT_FEE_EXPONENT_SCALE as f64) as u32,
            output_count_cap: cap,
            min_output_value: min_output,
            ..Self::default()
        }
    }
}

/// Picocredits per BTH. `1 BTH = 10^12 pico`.
///
/// # Unit-consistency contract (#626)
///
/// This MUST equal the ledger's picocredit scale (`botho::monetary`). On-chain
/// `cluster_wealth_db` stores picocredits, and the cluster-factor curve
/// consumes those same units. When these diverged (curve calibrated in
/// simulator units, ledger in pico) the on-chain sigmoid collapsed to a step
/// function — the exact bug #626 fixes. `W_MID_PICO` may ONLY be written as
/// `100_000 * PICO_PER_BTH` (never a bare `1e17`-style literal), and the
/// compile-time assert on `ClusterFactorCurve::LOG2_WMID_FP` locks the product
/// against future drift.
pub const PICO_PER_BTH: u128 = 1_000_000_000_000;

/// Piecewise-linear fixed-point log2, Q16. Pure `const fn`, monotone.
///
/// Returns `floor(log2(w)) << 16 | frac`, where `frac` is the top 16 mantissa
/// bits below the most-significant bit. The integer part comes from
/// `leading_zeros` (exactly defined by Rust on every target); the fractional
/// part is a piecewise-linear approximation of the true mantissa. This
/// approximation **is** the normative curve — mathematical `log2` is only a
/// design guide (#626 log-domain spec §1).
///
/// # Determinism
/// CONSENSUS-CRITICAL: no floats, only `leading_zeros`, shifts and masks — all
/// bit-identical across platforms. Caller MUST guarantee `w > 0`.
const fn log2_fp(w: u128) -> u64 {
    // caller guarantees w > 0
    let msb = 127 - w.leading_zeros(); // exact integer part of log2
    let frac = if msb >= 16 {
        ((w >> (msb - 16)) as u64) & 0xFFFF // top 16 mantissa bits
    } else {
        ((w << (16 - msb)) as u64) & 0xFFFF
    };
    ((msb as u64) << 16) | frac
}

/// Piecewise-linear sigmoid lookup, returning `[0, SIGMOID_SCALE]`.
///
/// `x_scaled` is the sigmoid argument in milli-units (`x * 1000`). The 7
/// interior knots are the logistic curve sampled at x ∈ {−6,−4,−2,0,2,4,6}; the
/// tails are clamped to **exact saturation** (`x ≤ −6000 → 0`, `x ≥ 6000 →
/// SIGMOID_SCALE`) so the factor curve reaches an exact 1x below `W_MID >> 12`
/// and an exact 6x at or above `W_MID << 12`. Monotone: the tail values bound
/// the adjacent knots (0 < 131, 65536 > 65405).
///
/// # Determinism
/// CONSENSUS-CRITICAL: pure integer arithmetic, no floats.
fn lut_sigmoid(x_scaled: i64) -> u64 {
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

    // Exact-saturation tails (#626 spec §1): clamp beyond ±6000.
    if x_scaled <= LUT[0].0 {
        return 0;
    }
    if x_scaled >= LUT[LUT.len() - 1].0 {
        return ClusterFactorCurve::SIGMOID_SCALE;
    }

    // Linear interpolation between table entries.
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

    ClusterFactorCurve::SIGMOID_SCALE / 2 // unreachable: x_scaled ∈ (−6000,
                                          // 6000)
}

/// Overflow-safe `floor(f_range * w_offset / w_range)` for u128 ranges.
///
/// Used only by the (provisional, Phase-2) [`ZkFeeCurve`] linear interpolation,
/// whose segments can be astronomically wide (up to the `u128::MAX` tail). When
/// `f_range * w_offset` would overflow `u128`, both operands are shifted down
/// by a common amount (the ratio is preserved), so the product always fits.
fn lerp_factor(f_range: u64, w_offset: u128, w_range: u128) -> u64 {
    if w_range == 0 {
        return 0;
    }
    match (f_range as u128).checked_mul(w_offset) {
        Some(p) => (p / w_range) as u64,
        None => {
            let s = 64;
            ((f_range as u128 * (w_offset >> s)) / (w_range >> s).max(1)) as u64
        }
    }
}

// Compile-time locks (#626 spec §3): any drift in the constants or the
// `log2_fp` definition fails the build.
const _: () = assert!(ClusterFactorCurve::W_MID_PICO == 100_000 * PICO_PER_BTH);
const _: () = assert!(ClusterFactorCurve::LOG2_WMID_FP == 3_695_429);

/// Cluster factor curve: maps cluster wealth (picocredits) to a multiplier
/// (1x to 6x) via a **log-domain** sigmoid.
///
/// The fee formula is: `fee_per_byte × tx_size × cluster_factor`.
/// This creates progressive taxation where wealthy clusters pay more.
///
/// The factor rises smoothly across *orders of magnitude* of wealth (log
/// domain), rather than within a single linear band. Concretely: `z` is the
/// number of octaves the wealth sits above/below the midpoint `W_MID_PICO`
/// (= 100,000 BTH), scaled by `log_width_fp` (2.0 octaves per sigmoid unit),
/// then fed through the shared sigmoid LUT (#626 log-domain spec).
///
/// This ensures:
/// - Coinbase-scale clusters (tens of BTH) pay ~1x (just the base fee)
/// - A 100,000-BTH cluster sits at exactly 3.5x (the sigmoid midpoint)
/// - Multi-million-BTH clusters pay up to 6x (heavily taxed)
/// - Smooth, order-of-magnitude progressivity in between
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ClusterFactorCurve {
    /// Minimum multiplier (1x = just base fee)
    pub factor_min: u32,

    /// Maximum multiplier (6x = heavily taxed)
    pub factor_max: u32,

    /// Wealth level at the sigmoid midpoint, in picocredits.
    /// The curve pins its midpoint at the module constant
    /// [`ClusterFactorCurve::W_MID_PICO`]; this field records it for
    /// documentation and the unit-consistency asserts.
    pub w_mid_pico: u128,

    /// Log-domain width: octaves per sigmoid unit, Q16 fixed-point.
    /// Larger = more gradual transition across octaves.
    pub log_width_fp: u64,

    /// Factor for fully diffused "background" wealth
    pub background_factor: u32,
}

impl ClusterFactorCurve {
    /// Fixed-point scale for factor output.
    /// FACTOR_SCALE = 1000, so factor=1000 means 1x, factor=6000 means 6x.
    pub const FACTOR_SCALE: u64 = 1000;

    /// Fixed-point scale for sigmoid output (2^16)
    pub const SIGMOID_SCALE: u64 = 65536;

    /// Q16 fixed-point shift for the log2 domain.
    pub const LOG2_FP_SHIFT: u32 = 16;

    /// Midpoint: 100,000 BTH in picocredits (= 1e17). MUST be written via
    /// `PICO_PER_BTH` (see the unit-consistency contract on that constant).
    pub const W_MID_PICO: u128 = 100_000 * PICO_PER_BTH;

    /// Log-width: octaves per sigmoid unit, Q16. 2.0 octaves/unit — a
    /// power-of-two divisor, so `z / log_width` is a shift.
    pub const LOG_WIDTH_FP: u64 = 2 << Self::LOG2_FP_SHIFT; // = 131_072

    /// `log2_fp(W_MID_PICO)`, const-evaluated with the normative `log2_fp`.
    /// Compile-time locked to `3_695_429` (msb=56, frac=25413).
    pub const LOG2_WMID_FP: u64 = log2_fp(Self::W_MID_PICO);

    /// Default curve with the ratified log-domain parameters (#626).
    ///
    /// - factor_min = 1x (small clusters just pay base privacy fee)
    /// - factor_max = 6x (large clusters pay 6× base fee)
    /// - midpoint at `W_MID_PICO` = 100,000 BTH
    /// - `log_width_fp` = 2.0 octaves per sigmoid unit
    pub fn default_params() -> Self {
        Self {
            factor_min: 1, // 1x multiplier
            factor_max: 6, // 6x multiplier
            w_mid_pico: Self::W_MID_PICO,
            log_width_fp: Self::LOG_WIDTH_FP,
            background_factor: 1, // 1x for diffused coins
        }
    }

    /// Create a flat factor curve (no progressivity).
    ///
    /// Useful for testing or if progressive taxation is disabled. A flat curve
    /// has `factor_min == factor_max`, so `factor()` returns that value for all
    /// wealths regardless of the midpoint/width.
    pub fn flat(factor: u32) -> Self {
        Self {
            factor_min: factor,
            factor_max: factor,
            w_mid_pico: Self::W_MID_PICO,
            log_width_fp: Self::LOG_WIDTH_FP,
            background_factor: factor,
        }
    }

    /// Check if this is a flat (non-progressive) curve.
    pub fn is_flat(&self) -> bool {
        self.factor_min == self.factor_max
    }

    /// Compute the cluster factor for a given cluster wealth in picocredits.
    ///
    /// Returns factor in FACTOR_SCALE units (1000 = 1x, 6000 = 6x). Output is
    /// the **smooth** (de-quantized) factor: `W = 0` is exactly the 1x floor,
    /// wealth ≥ `W_MID << 12` (≈409.6M BTH, incl. `u128::MAX`) is exactly 6x,
    /// and everything between interpolates through the sigmoid LUT.
    ///
    /// # Determinism
    /// CONSENSUS-CRITICAL: pure integer arithmetic. `i64` division truncates
    /// toward zero (Rust-guaranteed, normative here). No overflow is possible
    /// at any `u128` input: `log2_fp(u128::MAX) < 2^23`, so `z_fp * 1000 <
    /// 2^33` fits `i64` with wide margin. No floats, no saturating
    /// arithmetic needed.
    pub fn factor(&self, cluster_wealth_pico: u128) -> u64 {
        if cluster_wealth_pico == 0 {
            return self.factor_min as u64 * Self::FACTOR_SCALE;
        }

        // z = octaves above/below the midpoint, Q16.
        let z_fp: i64 = log2_fp(cluster_wealth_pico) as i64 - Self::LOG2_WMID_FP as i64;

        // Sigmoid argument in LUT milli-units. `/` truncates toward zero.
        let x_scaled: i64 = (z_fp * 1000) / (self.log_width_fp as i64);
        let sig = lut_sigmoid(x_scaled);

        // Smooth factor: factor_min + (factor_max − factor_min) × sigmoid.
        let range = self.factor_max.saturating_sub(self.factor_min) as u64 * Self::FACTOR_SCALE;
        self.factor_min as u64 * Self::FACTOR_SCALE + (range * sig) / Self::SIGMOID_SCALE
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
/// The slope is scaled by `SLOPE_SCALE` (10^12) for precision in fixed-point
/// arithmetic. The intercept is scaled by `FACTOR_SCALE` (10^3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SegmentParams {
    /// Lower bound of wealth range (inclusive), in picocredits.
    pub w_lo: u128,
    /// Upper bound of wealth range (exclusive, or MAX for last segment), in
    /// picocredits.
    pub w_hi: u128,
    /// Slope of the linear segment, scaled by SLOPE_SCALE (10^12).
    /// For segment from (w_lo, f_lo) to (w_hi, f_hi):
    /// slope_scaled = (f_hi - f_lo) * SLOPE_SCALE / (w_hi - w_lo)
    ///
    /// To compute factor: factor = f_lo + slope_scaled * (w - w_lo) /
    /// SLOPE_SCALE
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
/// The curve approximates the log-domain sigmoid with 5 linear segments whose
/// boundaries are power-of-two multiples of `W_MID_PICO` (#626 spec §4).
/// Factors are sampled from the normative sigmoid at each boundary; linear-in-W
/// interpolation inside a multi-octave segment tracks the log curve only
/// approximately (see the agreement test). Provisional until Phase 2 wiring.
///
/// ## ZK Proof Strategy
///
/// Using a 5-way OR-proof, the prover demonstrates:
/// - Wealth falls within exactly one segment (range proofs)
/// - Fee satisfies that segment's linear relation
///
/// The verifier cannot determine which segment is real (privacy preserved).
/// Total proof overhead: ~7.5 KB (5 segments × ~1.5 KB each).
///
/// ## Example
///
/// ```
/// use bth_cluster_tax::ZkFeeCurve;
///
/// let curve = ZkFeeCurve::default();
///
/// // Poor segment: 1x factor at zero wealth.
/// assert_eq!(curve.factor(0), 1000);
///
/// // Rich tail: 6x factor at the maximum.
/// assert_eq!(curve.factor(u128::MAX), 6000);
///
/// // Middle: strictly between 1x and 6x (100,000 BTH in picocredits).
/// let mid_factor = curve.factor(100_000 * 1_000_000_000_000u128);
/// assert!(mid_factor > 1000 && mid_factor < 6000);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ZkFeeCurve {
    /// Segment boundaries (picocredits), power-of-two multiples of
    /// `W_MID_PICO`. `[0, W_MID>>8, W_MID>>3, W_MID<<3, W_MID<<8,
    /// u128::MAX]` (#626 spec §4).
    pub boundaries: [u128; 6],

    /// Factor at each boundary in FACTOR_SCALE units, taken from the normative
    /// log-domain sigmoid at each boundary: `[1000, 1090, 2071, 4928, 5909,
    /// 6000]`.
    pub factors: [u64; 6],
}

impl ZkFeeCurve {
    /// Fixed-point scale for factor output, matching `ClusterFactorCurve`.
    /// FACTOR_SCALE = 1000, so factor=1000 means 1x, factor=6000 means 6x.
    pub const FACTOR_SCALE: u64 = 1000;

    /// High-precision scale for slope calculations to avoid integer truncation.
    /// SLOPE_SCALE = 10^12 preserves precision for small slopes.
    pub const SLOPE_SCALE: i128 = 1_000_000_000_000;

    /// Number of segments in the piecewise curve.
    pub const NUM_SEGMENTS: usize = 5;

    /// Default 5-segment configuration re-anchored to the log-domain curve
    /// (#626 spec §4).
    ///
    /// Boundaries are power-of-two multiples of `W_MID_PICO` (bit-friendly for
    /// ZK range proofs); factors are sampled from the normative sigmoid at each
    /// boundary. Linear-in-W interpolation inside a multi-octave segment cannot
    /// track the log curve exactly (±0.45x max divergence — see the agreement
    /// test); the ZK path is provisional until Phase 2 (committed-tag fees) is
    /// wired, and is recalibrated now only so the two curves cannot diverge by
    /// 100,000x in units again.
    ///
    /// | boundary | value | BTH | factor |
    /// |---|---|---|---|
    /// | b0 | 0 | 0 | 1000 |
    /// | b1 = W_MID>>8 | 3.906e14 | 390.625 | 1090 |
    /// | b2 = W_MID>>3 | 1.25e16 | 12,500 | 2071 |
    /// | b3 = W_MID<<3 | 8e17 | 800,000 | 4928 |
    /// | b4 = W_MID<<8 | 2.56e19 | 25,600,000 | 5909 |
    /// | b5 | u128::MAX | — | 6000 |
    pub fn default() -> Self {
        let w = ClusterFactorCurve::W_MID_PICO;
        Self {
            boundaries: [0, w >> 8, w >> 3, w << 3, w << 8, u128::MAX],
            factors: [1000, 1090, 2071, 4928, 5909, 6000],
        }
    }

    /// Create a flat factor curve (no progressivity).
    ///
    /// Useful for testing or if progressive taxation is disabled.
    pub fn flat(factor: u64) -> Self {
        let factor_scaled = factor * Self::FACTOR_SCALE;
        Self {
            boundaries: [0, 1, 2, 3, 4, u128::MAX],
            factors: [factor_scaled; 6],
        }
    }

    /// Check if this is a flat (non-progressive) curve.
    pub fn is_flat(&self) -> bool {
        self.factors.iter().all(|&f| f == self.factors[0])
    }

    /// Compute the cluster factor for a given cluster wealth (picocredits).
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
    pub fn factor(&self, cluster_wealth: u128) -> u64 {
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

        // Handle the last segment boundary (u128::MAX)
        // To avoid overflow, we use saturating arithmetic and check for the max case
        if cluster_wealth >= w_hi.saturating_sub(1) && segment == Self::NUM_SEGMENTS - 1 {
            return f_hi;
        }

        // Linear interpolation: f_lo + (f_hi - f_lo) × (w - w_lo) / (w_hi - w_lo)
        let w_range = w_hi.saturating_sub(w_lo);
        let w_offset = cluster_wealth.saturating_sub(w_lo);

        if f_hi >= f_lo {
            // Increasing factor (normal case)
            f_lo.saturating_add(lerp_factor(f_hi - f_lo, w_offset, w_range))
        } else {
            // Decreasing factor (unusual but handle it)
            f_lo.saturating_sub(lerp_factor(f_lo - f_hi, w_offset, w_range))
        }
    }

    /// Find which segment a given wealth value falls into.
    ///
    /// Returns segment index in `0..NUM_SEGMENTS`.
    fn find_segment(&self, wealth: u128) -> usize {
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

        // Calculate slope with high precision: (f_hi - f_lo) * SLOPE_SCALE / (w_hi -
        // w_lo)
        let w_range = w_hi.saturating_sub(w_lo); // u128
        let f_range = f_hi as i128 - f_lo as i128;

        let (slope_scaled, intercept_scaled) = if w_range == 0 {
            // Degenerate case: zero-width segment
            (0i64, f_lo * Self::FACTOR_SCALE as i64)
        } else if w_range > i128::MAX as u128 {
            // Astronomically wide segment (e.g. the u128::MAX tail): the slope
            // rounds to 0 at SLOPE_SCALE precision.
            (0i64, f_lo * Self::FACTOR_SCALE as i64)
        } else {
            // slope_scaled = (f_hi - f_lo) * SLOPE_SCALE / (w_hi - w_lo)
            // This preserves precision for small slopes. `f_range * SLOPE_SCALE`
            // is bounded (|f_range| < 6000, SLOPE_SCALE = 1e12) → fits i128.
            let slope = (f_range * Self::SLOPE_SCALE / w_range as i128) as i64;
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
    /// Returns parameters for all `NUM_SEGMENTS` segments, useful for
    /// constructing the OR-proof where the prover demonstrates membership in
    /// exactly one segment.
    pub fn all_segment_params(&self) -> [SegmentParams; Self::NUM_SEGMENTS] {
        [
            self.segment_params(0),
            self.segment_params(1),
            self.segment_params(2),
            self.segment_params(3),
            self.segment_params(4),
        ]
    }

    /// Check if a wealth value falls within a specific segment.
    ///
    /// Used for verifying segment membership in ZK proofs.
    pub fn in_segment(&self, wealth: u128, segment: usize) -> bool {
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

    /// Picocredits per BTH, for readable wealth literals in tests.
    const PICO: u128 = PICO_PER_BTH;

    // ========================================================================
    // Output Penalty Tests
    // ========================================================================

    #[test]
    fn test_output_penalty_quadratic() {
        let config = FeeConfig::default();

        // 1 output: 1^2 = 1x (1000)
        assert_eq!(config.output_penalty(1), 1000);

        // 2 outputs: 2^2 = 4x (4000)
        assert_eq!(config.output_penalty(2), 4000);

        // 5 outputs: 5^2 = 25x (25000)
        assert_eq!(config.output_penalty(5), 25000);

        // 10 outputs: 10^2 = 100x (100000)
        assert_eq!(config.output_penalty(10), 100000);
    }

    #[test]
    fn test_output_penalty_cap() {
        let config = FeeConfig::default();

        // Above cap (10), penalty should be capped at 100x
        assert_eq!(config.output_penalty(11), 100000);
        assert_eq!(config.output_penalty(20), 100000);
        assert_eq!(config.output_penalty(100), 100000);
    }

    #[test]
    fn test_output_penalty_linear() {
        let config = FeeConfig::with_linear_output_fees();

        // With exponent=1.0, penalty should be linear
        assert_eq!(config.output_penalty(1), 1000);
        assert_eq!(config.output_penalty(2), 2000);
        assert_eq!(config.output_penalty(5), 5000);
        assert_eq!(config.output_penalty(10), 10000);
    }

    /// Value-preservation guard for the integer rewrite of `output_penalty`
    /// (issue #570). Asserts the EXACT table the previous `f64::powf`
    /// implementation produced for the default config across the full
    /// `[0, cap]` domain (and the cap clamp above it). If any of these change,
    /// the rewrite is no longer bit-identical and must NOT land independently
    /// of a consensus reset.
    #[test]
    fn test_output_penalty_default_table_value_preserving() {
        let config = FeeConfig::default(); // exponent 2.0, cap 10

        // count^2 * 1000, exactly as the old `(count.powf(2.0) * 1000.0) as u64`.
        let expected: &[(usize, u64)] = &[
            (0, 0),        // 0^2 = 0
            (1, 1_000),    // 1^2
            (2, 4_000),    // 2^2
            (3, 9_000),    // 3^2
            (4, 16_000),   // 4^2
            (5, 25_000),   // 5^2
            (6, 36_000),   // 6^2
            (7, 49_000),   // 7^2
            (8, 64_000),   // 8^2
            (9, 81_000),   // 9^2
            (10, 100_000), // 10^2 (cap)
            (11, 100_000), // clamped to cap^2
            (20, 100_000), // clamped to cap^2
            (1_000, 100_000),
        ];

        for &(count, want) in expected {
            assert_eq!(
                config.output_penalty(count),
                want,
                "output_penalty({count}) regressed from value-preserving table"
            );
        }
    }

    /// `output_penalty` must be integer-only and must never panic, even for
    /// adversarial configs (huge exponent / huge cap) — it saturates instead.
    #[test]
    fn test_output_penalty_saturates_without_panic() {
        let config = FeeConfig {
            output_fee_exponent_scaled: 64_000, // exponent 64
            output_count_cap: 1_000_000,
            ..FeeConfig::default()
        };
        // 1_000_000^64 vastly exceeds u64; must saturate, not panic/overflow.
        assert_eq!(config.output_penalty(1_000_000), u64::MAX);
        // Small inputs still compute exactly: 1^64 * 1000 = 1000.
        assert_eq!(config.output_penalty(1), 1_000);
    }

    /// Exponent policy: integer exponents validate; fractional ones are
    /// rejected by `validate()` and clamped down (floored) by `output_penalty`.
    #[test]
    fn test_exponent_policy_validate_and_clamp() {
        // Integer exponents pass validation.
        assert!(FeeConfig::default().validate().is_ok());
        assert!(FeeConfig::with_linear_output_fees().validate().is_ok());

        // A fractional exponent (2.5 -> 2500) is not a multiple of 1000.
        let fractional = FeeConfig {
            output_fee_exponent_scaled: 2_500,
            ..FeeConfig::default()
        };
        assert!(fractional.validate().is_err());

        // output_penalty clamps the fractional exponent DOWN to 2.0, so it
        // behaves as the floored integer exponent (defense-in-depth).
        assert_eq!(fractional.output_penalty(3), 9_000); // 3^2 * 1000, not
                                                         // 3^2.5
    }

    #[test]
    fn test_output_fee_scaling() {
        let config = FeeConfig {
            fee_per_byte: 1,
            cluster_curve: ClusterFactorCurve::flat(1), // 1x cluster factor
            fee_per_memo: 0,
            output_fee_exponent_scaled: 2000, // quadratic
            output_count_cap: 10,
            min_output_value: 1_000_000,
        };

        // Fee should scale quadratically with outputs
        let fee_1 = config.compute_fee_with_outputs(TransactionType::Hidden, 1_000, 0, 1, 0);
        let fee_2 = config.compute_fee_with_outputs(TransactionType::Hidden, 1_000, 0, 2, 0);
        let fee_5 = config.compute_fee_with_outputs(TransactionType::Hidden, 1_000, 0, 5, 0);

        // 2 outputs should be 4x the fee of 1 output
        assert_eq!(fee_2, fee_1 * 4, "2 outputs = 4x fee");

        // 5 outputs should be 25x the fee of 1 output
        assert_eq!(fee_5, fee_1 * 25, "5 outputs = 25x fee");
    }

    #[test]
    fn test_superlinear_fee_prevents_splitting() {
        let config = FeeConfig::default();

        // Single 2-output transaction (normal)
        let fee_normal = config.compute_fee_with_outputs(TransactionType::Hidden, 4_000, 0, 2, 0);

        // Splitting into 10 outputs costs 25x more
        let fee_split = config.compute_fee_with_outputs(TransactionType::Hidden, 4_000, 0, 10, 0);

        // 10 outputs = 100x penalty, 2 outputs = 4x penalty
        // So splitting should cost 100/4 = 25x more
        assert_eq!(
            fee_split,
            fee_normal * 25,
            "10-output tx should cost 25x more"
        );
    }

    // ========================================================================
    // Dust Prevention Tests
    // ========================================================================

    #[test]
    fn test_dust_threshold() {
        let config = FeeConfig::default();

        // Default threshold is 1M picocredits
        assert_eq!(config.dust_threshold(), 1_000_000);

        // Values at or above threshold are OK
        assert!(config.is_output_above_dust(1_000_000));
        assert!(config.is_output_above_dust(2_000_000));

        // Values below threshold are dust
        assert!(!config.is_output_above_dust(999_999));
        assert!(!config.is_output_above_dust(0));
    }

    // ========================================================================
    // Legacy API Compatibility Tests
    // ========================================================================

    #[test]
    fn test_size_based_fee() {
        let config = FeeConfig::default();

        // 4 KB transaction (typical CLSAG) with small cluster (wealth 0 → 1x).
        // Now includes 4x output penalty for 2 outputs (default)
        let fee_small = config.compute_fee(TransactionType::Hidden, 4_000, 0, 0);
        // fee = 1 pico/byte × 4000 bytes × 1x factor × 4x output = 16,000
        assert!(
            fee_small >= 16_000 && fee_small <= 40_000,
            "4KB tx with small cluster (2 outputs): {fee_small}"
        );

        // Same transaction with a large (1M BTH) cluster → ~5x factor.
        let fee_large = config.compute_fee(
            TransactionType::Hidden,
            4_000,
            (1_000_000 * PICO) as u128,
            0,
        );
        assert!(
            fee_large > fee_small * 2,
            "Large cluster should pay more: {fee_large} > {fee_small}"
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

        // Even with many outputs
        let fee_many = config.compute_fee_with_outputs(TransactionType::Minting, 1_500, 0, 10, 0);
        assert_eq!(fee_many, 0);
    }

    #[test]
    fn test_cluster_factor_extremes() {
        let curve = ClusterFactorCurve::default_params();

        // At wealth=0, factor is exactly the 1x floor.
        assert_eq!(curve.factor(0), 1000, "Zero wealth should be exactly 1x");

        // At very high wealth (10M BTH), factor should be near maximum.
        let factor_large = curve.factor(10_000_000 * PICO);
        assert!(
            factor_large >= 5000, // At least 5x
            "Large wealth should have high factor: {factor_large}"
        );

        // At the midpoint (W_MID_PICO), factor is exactly 3.5x by construction.
        let factor_mid = curve.factor(ClusterFactorCurve::W_MID_PICO);
        assert_eq!(factor_mid, 3500, "Midpoint factor must be exactly 3.5x");
    }

    #[test]
    fn test_factor_monotonic_increase() {
        let curve = ClusterFactorCurve::default_params();
        let mut prev_factor = 0;

        for bth in [
            0u128, 10, 100, 1_000, 10_000, 100_000, 1_000_000, 10_000_000,
        ] {
            let factor = curve.factor(bth * PICO);
            assert!(
                factor >= prev_factor,
                "Factor should increase with wealth: {prev_factor} -> {factor} at {bth} BTH"
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
    }

    #[test]
    fn test_progressive_fees() {
        let config = FeeConfig::default();
        let tx_size = 4_000; // 4 KB

        // Test that fees increase with cluster wealth (picocredit inputs).
        // 1k BTH → 1.265x, 100k BTH (midpoint) → 3.5x, 1M BTH → 5.093x.
        let fee_small =
            config.compute_fee(TransactionType::Hidden, tx_size, (1_000 * PICO) as u128, 0);
        let fee_mid = config.compute_fee(
            TransactionType::Hidden,
            tx_size,
            (100_000 * PICO) as u128,
            0,
        );
        let fee_large = config.compute_fee(
            TransactionType::Hidden,
            tx_size,
            (1_000_000 * PICO) as u128,
            0,
        );

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
            output_fee_exponent_scaled: 2000,
            output_count_cap: 10,
            min_output_value: 1_000_000,
        };

        // Double the size should double the fee (same output count)
        let fee_1k = config.compute_fee_with_outputs(TransactionType::Hidden, 1_000, 0, 2, 0);
        let fee_2k = config.compute_fee_with_outputs(TransactionType::Hidden, 2_000, 0, 2, 0);
        assert_eq!(fee_2k, fee_1k * 2, "Fee should scale linearly with size");
    }

    // ========================================================================
    // Fee Estimation with Outputs Tests
    // ========================================================================

    #[test]
    fn test_estimate_fee_with_outputs() {
        let config = FeeConfig::default();

        // More outputs should cost more
        let fee_2 = config.estimate_fee_with_outputs(TransactionType::Hidden, 0, 2, 0);
        let fee_5 = config.estimate_fee_with_outputs(TransactionType::Hidden, 0, 5, 0);
        let fee_10 = config.estimate_fee_with_outputs(TransactionType::Hidden, 0, 10, 0);

        assert!(fee_5 > fee_2, "5 outputs should cost more than 2");
        assert!(fee_10 > fee_5, "10 outputs should cost more than 5");
    }

    #[test]
    fn test_custom_output_params() {
        // Custom config with cubic exponent and higher cap
        let config = FeeConfig::with_output_fee_params(3.0, 20, 500_000);

        // Check params are set correctly
        assert_eq!(config.output_fee_exponent_scaled, 3000);
        assert_eq!(config.output_count_cap, 20);
        assert_eq!(config.min_output_value, 500_000);

        // 2 outputs with cubic: 2^3 = 8x (8000)
        assert_eq!(config.output_penalty(2), 8000);
    }

    // ========================================================================
    // ZkFeeCurve Tests
    // ========================================================================

    #[test]
    fn test_zk_fee_curve_boundary_values() {
        // Factors at the re-anchored power-of-two boundaries (#626 spec §4),
        // sampled from the normative log-domain sigmoid.
        let curve = ZkFeeCurve::default();
        let w = ClusterFactorCurve::W_MID_PICO;
        assert_eq!(curve.factor(0), 1000);
        assert_eq!(curve.factor(w >> 8), 1090);
        assert_eq!(curve.factor(w >> 3), 2071);
        assert_eq!(curve.factor(w << 3), 4928);
        assert_eq!(curve.factor(w << 8), 5909);
        assert_eq!(curve.factor(u128::MAX), 6000);
    }

    #[test]
    fn test_zk_fee_curve_monotonic_increase() {
        let curve = ZkFeeCurve::default();
        let mut prev = 0;
        let mut w: u128 = 1;
        // Sweep every octave from 2^0 to 2^126.
        for _ in 0..127 {
            let f = curve.factor(w);
            assert!(f >= prev, "ZkFeeCurve non-monotone: {prev} -> {f} at {w}");
            prev = f;
            w <<= 1;
        }
        assert_eq!(curve.factor(u128::MAX), 6000);
    }

    #[test]
    fn test_zk_fee_curve_flat() {
        let curve = ZkFeeCurve::flat(3);
        assert_eq!(curve.factor(0), 3000);
        assert_eq!(curve.factor(1_000_000 * PICO), 3000);
        assert_eq!(curve.factor(u128::MAX), 3000);
        assert!(curve.is_flat());
    }

    #[test]
    fn test_zk_fee_curve_segment_membership() {
        let curve = ZkFeeCurve::default();
        let w = ClusterFactorCurve::W_MID_PICO;
        // Segment 0: [0, W>>8)
        assert!(curve.in_segment(0, 0));
        assert!(curve.in_segment((w >> 8) - 1, 0));
        assert!(!curve.in_segment(w >> 8, 0));
        // Segment 1: [W>>8, W>>3)
        assert!(curve.in_segment(w >> 8, 1));
        assert!(!curve.in_segment(w >> 3, 1));
        // Segment 4 (last): [W<<8, MAX)
        assert!(curve.in_segment(w << 8, 4));
        assert!(curve.in_segment(u128::MAX - 1, 4));
        // Invalid segment index (valid range is 0..=4).
        assert!(!curve.in_segment(0, 5));
    }

    #[test]
    fn test_zk_fee_curve_segment_params() {
        let curve = ZkFeeCurve::default();
        let w = ClusterFactorCurve::W_MID_PICO;

        // NOTE (#626): at picocredit scale the segment ranges (1e14..1e19) are
        // enormous relative to SLOPE_SCALE (1e12), so `slope_scaled` truncates to
        // 0 for every segment — the ZK linear-relation proof degenerates toward
        // flat-per-segment. `ZkFeeCurve::factor()` does NOT use these slopes (it
        // interpolates directly via `lerp_factor`), so the factor table is
        // unaffected; but the Phase-2 ZK prover precision needs a larger
        // SLOPE_SCALE. Flagged for the architect / Phase-2 work.
        let p0 = curve.segment_params(0);
        assert_eq!(p0.w_lo, 0);
        assert_eq!(p0.w_hi, w >> 8);
        assert!(p0.slope_scaled >= 0, "seg 0 slope: {}", p0.slope_scaled);

        let p1 = curve.segment_params(1);
        assert_eq!(p1.w_lo, w >> 8);
        assert_eq!(p1.w_hi, w >> 3);
        assert!(p1.slope_scaled >= 0, "seg 1 slope: {}", p1.slope_scaled);

        // Last segment spans up to u128::MAX: slope rounds to 0 in fixed point.
        let p4 = curve.segment_params(4);
        assert_eq!(p4.w_lo, w << 8);
        assert_eq!(p4.w_hi, u128::MAX);
        assert!(p4.slope_scaled >= 0, "seg 4 slope: {}", p4.slope_scaled);
    }

    #[test]
    fn test_zk_fee_curve_all_segment_params() {
        let curve = ZkFeeCurve::default();
        let all = curve.all_segment_params();
        let w = ClusterFactorCurve::W_MID_PICO;
        assert_eq!(all.len(), 5);
        assert_eq!(all[0].w_lo, 0);
        assert_eq!(all[1].w_lo, w >> 8);
        assert_eq!(all[4].w_lo, w << 8);
    }

    #[test]
    fn test_zk_fee_curve_agrees_with_sigmoid() {
        // The ZkFeeCurve (linear-in-W between 5 power-of-two boundaries) tracks
        // the log-domain sigmoid only approximately. NOTE (#626): the spec §4
        // prose estimated ±450 (0.45x), but the measured max divergence is 1243
        // (~1.24x) at 2^57 (~144k BTH), inside the wide central segment
        // [W>>3, W<<3] where linear-in-W lags the log curve hardest. The
        // boundaries/factors are the normative spec; this tolerance reflects the
        // construction as-specified (flagged for the architect / Phase-2 work).
        let sigmoid = ClusterFactorCurve::default_params();
        let zk = ZkFeeCurve::default();
        for k in 40..70u32 {
            let w = 1u128 << k;
            let s = sigmoid.factor(w) as i64;
            let z = zk.factor(w) as i64;
            assert!(
                (s - z).abs() <= 1300,
                "Zk vs sigmoid diverge at 2^{k}: sigmoid={s}, zk={z}, diff={}",
                (s - z).abs()
            );
        }
    }

    #[test]
    fn test_zk_fee_curve_factor_scale_consistency() {
        assert_eq!(
            ZkFeeCurve::FACTOR_SCALE,
            ClusterFactorCurve::FACTOR_SCALE,
            "FACTOR_SCALE should match between curves"
        );
    }

    #[test]
    #[should_panic(expected = "segment index 5 out of bounds")]
    fn test_zk_fee_curve_segment_params_out_of_bounds() {
        let curve = ZkFeeCurve::default();
        let _ = curve.segment_params(5); // Should panic (valid range 0..=4)
    }

    // ========================================================================
    // Log-domain curve guards (#626)
    // ========================================================================

    /// Golden-vector test: the exact factor table from the spec §2. Pinned
    /// verbatim so the curve can never silently change shape.
    #[test]
    fn test_golden_vector_factor_table() {
        let c = ClusterFactorCurve::default_params();
        assert_eq!(c.factor(0), 1000, "W=0");
        assert_eq!(c.factor(10 * PICO), 1000, "10 BTH");
        assert_eq!(c.factor(100 * PICO), 1050, "100 BTH");
        assert_eq!(c.factor(1_000 * PICO), 1265, "1k BTH");
        assert_eq!(c.factor(10_000 * PICO), 1939, "10k BTH");
        assert_eq!(c.factor(100_000 * PICO), 3500, "100k BTH (midpoint)");
        assert_eq!(c.factor(1_000_000 * PICO), 5093, "1M BTH");
        assert_eq!(c.factor(10_000_000 * PICO), 5745, "10M BTH");
        assert_eq!(
            c.factor(409_600_000 * PICO),
            6000,
            "409.6M BTH (saturation)"
        );
        assert_eq!(c.factor(u128::MAX), 6000, "u128::MAX");
    }

    /// Testnet-baseline goldens (spec §5 table): the live coinbase clusters.
    #[test]
    fn test_testnet_baseline_factors() {
        let c = ClusterFactorCurve::default_params();
        assert_eq!(c.factor(50 * PICO), 1030, "50 BTH (median live cluster)");
        assert_eq!(
            c.factor(975 * PICO / 10),
            1049,
            "97.5 BTH (max live cluster)"
        );
    }

    /// Unit-consistency guard (spec §6.2): coinbase-scale clusters are ~1x, so
    /// a power-of-ten unit drift between curve constants and ledger units
    /// fails.
    #[test]
    fn test_unit_consistency_guard() {
        let c = ClusterFactorCurve::default_params();
        assert!(
            c.factor(50 * PICO) < 1100,
            "a coinbase-scale cluster is <1.1x"
        );
        assert_eq!(c.factor(10 * PICO), 1000);
        assert_eq!(c.factor(100_000 * PICO), 3500);
        assert!(c.factor(10_000_000 * PICO) > 5700);
    }

    /// The picocredit constants match the ledger scale and each other.
    #[test]
    fn test_pico_and_midpoint_constants() {
        assert_eq!(PICO_PER_BTH, 1_000_000_000_000);
        assert_eq!(ClusterFactorCurve::W_MID_PICO, 100_000 * PICO_PER_BTH);
        assert_eq!(ClusterFactorCurve::W_MID_PICO, 100_000_000_000_000_000);
        assert_eq!(ClusterFactorCurve::LOG_WIDTH_FP, 131_072);
        assert_eq!(ClusterFactorCurve::LOG2_WMID_FP, 3_695_429);
    }

    /// `log2_fp` integer-part / mantissa correctness at reference points.
    #[test]
    fn test_log2_fp_reference_values() {
        assert_eq!(log2_fp(1), 0); // log2(1) = 0
        assert_eq!(log2_fp(2), 1 << 16); // log2(2) = 1.0
        assert_eq!(log2_fp(4), 2 << 16); // log2(4) = 2.0
        assert_eq!(log2_fp(1 << 56), 56 << 16); // exact power of two
                                                // Midpoint: msb=56, frac=25413.
        assert_eq!(log2_fp(ClusterFactorCurve::W_MID_PICO), (56 << 16) | 25413);
        // Bounded: log2_fp(u128::MAX) = (127<<16)|0xFFFF < 2^23.
        assert_eq!(log2_fp(u128::MAX), (127 << 16) | 0xFFFF);
        assert!(log2_fp(u128::MAX) < (1 << 23));
    }

    /// Exact-saturation tails of the shared sigmoid LUT (spec §1, §3c).
    #[test]
    fn test_lut_sigmoid_saturation_tails() {
        assert_eq!(lut_sigmoid(-6000), 0);
        assert_eq!(lut_sigmoid(-6001), 0);
        assert_eq!(lut_sigmoid(i64::MIN), 0);
        assert_eq!(lut_sigmoid(6000), 65536);
        assert_eq!(lut_sigmoid(6001), 65536);
        assert_eq!(lut_sigmoid(i64::MAX), 65536);
        assert_eq!(lut_sigmoid(0), 32768); // sigmoid(0) = 0.5
                                           // Monotone across the tail joins.
        assert!(lut_sigmoid(-6000) <= lut_sigmoid(-5999));
        assert!(lut_sigmoid(5999) <= lut_sigmoid(6000));
    }

    /// Monotonicity (spec §3a): random u128 pairs, deterministic PRNG.
    #[test]
    fn test_factor_monotonic_random_pairs() {
        use rand::{rngs::StdRng, Rng, SeedableRng};
        let c = ClusterFactorCurve::default_params();
        let mut rng = StdRng::seed_from_u64(0x626_C0FFEE);
        for _ in 0..200_000 {
            let a = ((rng.gen::<u64>() as u128) << 64) | rng.gen::<u64>() as u128;
            let b = ((rng.gen::<u64>() as u128) << 64) | rng.gen::<u64>() as u128;
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            assert!(
                c.factor(lo) <= c.factor(hi),
                "non-monotone: factor({lo}) > factor({hi})"
            );
        }
    }

    /// Monotonicity (spec §3b): exhaustive octave-boundary sweep. This is where
    /// a wrong mantissa extraction breaks monotonicity — at 2^k−1 the fraction
    /// is 0xFFFF with msb k−1; at 2^k it resets to 0 with msb k.
    #[test]
    fn test_factor_monotonic_octave_boundaries() {
        use std::collections::BTreeSet;
        let c = ClusterFactorCurve::default_params();
        // Every 2^k−1, 2^k, 2^k+1 for all representable k, swept in sorted order.
        let mut points: BTreeSet<u128> = BTreeSet::new();
        for k in 0..128u32 {
            let base = 1u128 << k;
            if let Some(v) = base.checked_sub(1) {
                points.insert(v);
            }
            points.insert(base);
            if let Some(v) = base.checked_add(1) {
                points.insert(v);
            }
        }
        points.remove(&0);
        let mut prev = 0u64;
        for &w in &points {
            let f = c.factor(w);
            assert!(
                f >= prev,
                "non-monotone at octave boundary w={w}: {prev} -> {f}"
            );
            prev = f;
        }
        assert_eq!(prev, 6000, "sweep must saturate at 6x");
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
        // Backwards-compatible LINEAR-domain sigmoid for simulation/analysis
        // only (NOT the consensus curve, which is log-domain — see
        // `ClusterFactorCurve::factor`). Uses the shared LUT.
        let x_scaled: i64 = if self.steepness == 0 {
            if cluster_wealth >= self.w_mid {
                6000
            } else {
                -6000
            }
        } else if cluster_wealth >= self.w_mid {
            ((cluster_wealth - self.w_mid) as i128 * 1000 / self.steepness as i128) as i64
        } else {
            -(((self.w_mid - cluster_wealth) as i128 * 1000 / self.steepness as i128) as i64)
        };
        let sigmoid = lut_sigmoid(x_scaled);
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
