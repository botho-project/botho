//! Demurrage time-quantization study (issue #1875): compares candidate fixes
//! for the six-block dead zone in the pre-9.0.0 `demurrage_charge`.
//!
//! Protocol 9.0.0 adopted option 1: production [`crate::demurrage_charge`] is
//! now the exact kernel (pinned to [`exact_charge`] by
//! `quantized_floor_matches_production`). This study describes the pre-fix
//! world, so its "defer" / "today" baseline is the legacy quantized formula,
//! re-implemented here as [`quantized_time_charge`] with `ceil_time = false`,
//! and its published tables are unchanged by the fix.
//!
//! # The finding under study
//!
//! The pre-9.0.0 `demurrage_charge` folded elapsed time into `TIME_SCALE = 1e6`
//! units of a `blocks_per_year` year **before** applying the value:
//!
//! ```text
//! time_fraction = elapsed × 1_000_000 / blocks_per_year      // floored
//! charge        = (value × rate × p / 10_000 / 5_000) × time_fraction / 1_000_000
//! ```
//!
//! At the mainnet 6,307,200 blocks/year every hold of 1..=6 blocks charges
//! zero for every value, and every hold is charged a whole number of
//! ~6.31-block units, so `frac(elapsed × 1e6 / bpy)` units leak on every
//! spend. Consensus feeds `elapsed` from `ring_elapsed_quantile@max` over the
//! ring members' public ages, and it enforces no minimum ring-member age
//! (`MIN_DECOY_AGE_BLOCKS = 10` is wallet policy). A custom wallet whose ring
//! is entirely young can churn every few blocks (PR #1899 reproduces this on
//! the production gate).
//!
//! The operator ruling on #1875 is "simulate before choosing". This module
//! scores the candidate fixes; it does **not** change consensus and it does
//! not change `demurrage_charge`. The operator picks the fix (9.0.0: option 1).
//!
//! # Candidates ([`Candidate`])
//!
//! | option | rule |
//! |--------|------|
//! | 4 (defer) | the pre-9.0.0 quantized kernel ([`quantized_time_charge`] with `ceil_time = false`) |
//! | 1 (exact) | `floor(value × rate × p × elapsed / (10_000 × 5_000 × bpy))`, no intermediate quantization ([`exact_charge`]) |
//! | 2a | the pre-9.0.0 charge, but at least 1 pico whenever the true charge is nonzero |
//! | 2b | the pre-9.0.0 formula with the time fraction rounded **up** |
//! | 2c | option 1 rounded up (`ceil` of the exact rational) — a variant, not in the ruling's list |
//! | 3a(M) | pre-9.0.0 charge; consensus rejects a ring with **any** member younger than `M` (the real input must be `M` blocks old) |
//! | 3b(M) | pre-9.0.0 charge; consensus rejects a ring whose **oldest** member is younger than `M`. Under the `@max` quantile this is the same as charging `max(elapsed, M)` |
//!
//! # What is measured
//!
//! - **(a) Attack.** An optimal churner holds its principal in hops of `h`
//!   blocks for one year (`floor(bpy/h)·h` blocks) and pays each hop's charge,
//!   against a buy-and-hold baseline under the **same** rule. Fees are the
//!   node's consensus floor for a 1-input, 1-output CLSAG transaction
//!   ([`crate::consensus_fee::consensus_fee_breakdown`]), optionally raised to
//!   the relay minimum. Two cost bounds bracket reality: *cheapest* = consensus
//!   floor only, one coin, decoys supplied by a busy chain; *dearest* = relay
//!   minimum, the principal split into 20 coins that churn round-robin so they
//!   serve as each other's young decoys (no chain throughput assumed). Lottery
//!   recapture of fees is ignored, which overstates the attacker's cost.
//! - **(b) Honest users.** Honest rings come from the production node-picker
//!   port ([`crate::simulation::privacy_routes`]) under the gamma spend model,
//!   plus short-hold commerce ages. The consensus elapsed statistic is the
//!   ring's oldest member, as in the node. Each kernel is compared with today's
//!   charge and with the exact rational charge (computed in 256-bit integer
//!   arithmetic, [`rational_charge`]). Split-versus-unsplit loss (the
//!   churn-invariance target) is measured over random time and value splits.
//! - **(c) Option 3.** Young-coin spendability and ring privacy for real ages
//!   near the minimum, using the node-picker port, and the share of honest real
//!   inputs and decoys a minimum would exclude.
//! - **(d) Extremes.** Every kernel at `u64::MAX` values, elapsed and rates,
//!   and at absurd `blocks_per_year`, compared with the 256-bit oracle.
//!
//! # Determinism
//!
//! The kernels are pure integer arithmetic with explicit saturation (release
//! builds keep `overflow-checks = true`). The Monte-Carlo parts use
//! `ChaCha8Rng` seeded from [`SEED`]; the report regenerates byte for byte.

use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;

use crate::{
    consensus_fee::{
        consensus_fee_breakdown, consensus_tx_size_bytes, ClaimedClass, ConsensusFeeInputs,
        ConsensusFeeParams, FeeModelMode, MAINNET_BLOCKS_PER_YEAR, RELAY_MIN_TX_FEE,
    },
    demurrage::{FACTOR_SCALE, MAX_FACTOR_SCALED},
    demurrage_charge,
    fee_curve::PICO_PER_BTH,
    simulation::privacy_routes::{
        node_select_decoys_for_input, run_route_row, sample_route_ring_ages, ChainModel,
        DecoyRoute, RealAgeSpec, RouteCandidate, RouteRingAges, RouteRowResult, RouteSimConfig,
        MIN_DECOY_AGE_BLOCKS,
    },
    ClusterFactorCurve,
};

/// Deterministic seed for this sweep.
pub const SEED: u64 = 0xB07A_1875;

/// Mainnet blocks per year at 5 s blocks.
pub const BLOCKS_PER_YEAR: u64 = MAINNET_BLOCKS_PER_YEAR;

/// The epoch-1 demurrage rate, bps (shared with the sibling sweeps).
pub const RATE_BPS: u32 = super::settlement_horizon_sweep::RATE_BPS;

/// The pre-9.0.0 kernel's time scale (a function-local constant inside the
/// legacy `demurrage_charge`; production is exact since 9.0.0, #1875).
pub const TIME_SCALE: u128 = 1_000_000;

/// Ring size the node requires (real input + 19 decoys).
pub const RING_SIZE: usize = crate::consensus_fee::CONSENSUS_RING_SIZE;

/// Coins the "dearest" attack splits its principal into so that the coins
/// are each other's young decoys (one ring's worth).
pub const SELF_DECOY_COINS: u64 = RING_SIZE as u64;

const BPS: u128 = 10_000;
const PROG_SPAN: u128 = (MAX_FACTOR_SCALED - FACTOR_SCALE) as u128;

/// Largest `blocks_per_year` for which [`exact_charge`] is exact over the
/// whole `u64` input domain: the remainder product `rem × elapsed` fits in
/// `u128` exactly when `10_000 × 5_000 × bpy ≤ 2^64 + 2`. Above it the
/// kernel saturates to `u64::MAX` (the fee-floor-raising direction).
pub const EXACT_BPY_LIMIT: u64 = (((1u128 << 64) + 2) / (BPS * PROG_SPAN)) as u64;

fn progressivity(factor: u64) -> u64 {
    factor.clamp(FACTOR_SCALE, MAX_FACTOR_SCALED) - FACTOR_SCALE
}

// ============================================================================
// 256-bit oracle for the exact rational charge
// ============================================================================

/// `a × b` as a 256-bit `(hi, lo)` pair.
fn mul_wide(a: u128, b: u128) -> (u128, u128) {
    const M: u128 = u64::MAX as u128;
    let (a1, a0) = (a >> 64, a & M);
    let (b1, b0) = (b >> 64, b & M);
    let p00 = a0 * b0;
    let p01 = a0 * b1;
    let p10 = a1 * b0;
    let p11 = a1 * b1;
    let mid = (p00 >> 64) + (p01 & M) + (p10 & M);
    let lo = (p00 & M) | (mid << 64);
    let hi = p11 + (p01 >> 64) + (p10 >> 64) + (mid >> 64);
    (hi, lo)
}

/// `(hi, lo) / d` by binary long division: `((q_hi, q_lo), remainder)`.
fn divrem_wide(hi: u128, lo: u128, d: u128) -> ((u128, u128), u128) {
    assert!(d != 0, "division by zero");
    let (mut q_hi, mut q_lo, mut rem) = (0u128, 0u128, 0u128);
    for i in (0..256u32).rev() {
        let bit = if i >= 128 {
            (hi >> (i - 128)) & 1
        } else {
            (lo >> i) & 1
        };
        let carry = rem >> 127;
        rem = (rem << 1) | bit;
        if carry == 1 || rem >= d {
            rem = rem.wrapping_sub(d);
            if i >= 128 {
                q_hi |= 1 << (i - 128);
            } else {
                q_lo |= 1 << i;
            }
        }
    }
    ((q_hi, q_lo), rem)
}

/// The exact rational demurrage charge
/// `value × rate × p × elapsed / (10_000 × 5_000 × bpy)`, split into an
/// integer part (clamped to `u64`) and a fractional part.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RationalCharge {
    /// `floor(true charge)`, clamped to `u64::MAX`.
    pub floor: u64,
    /// The true charge exceeds `u64::MAX`.
    pub saturated: bool,
    /// The fractional part, `remainder / denominator`, in `[0, 1)`.
    pub frac: f64,
    /// The fractional part is exactly zero.
    pub frac_is_zero: bool,
    /// The true charge is exactly zero.
    pub is_zero: bool,
}

impl RationalCharge {
    const ZERO: Self = Self {
        floor: 0,
        saturated: false,
        frac: 0.0,
        frac_is_zero: true,
        is_zero: true,
    };

    /// `ceil(true charge)`, clamped to `u64::MAX`.
    pub fn ceil(&self) -> u64 {
        if self.frac_is_zero || self.saturated {
            self.floor
        } else {
            self.floor.saturating_add(1)
        }
    }

    /// `kernel − true charge` in picocredits (positive = over-charge).
    pub fn error_of(&self, kernel: u64) -> f64 {
        (kernel as i128 - self.floor as i128) as f64 - self.frac
    }

    /// The true charge as `f64` picocredits (for ratios only).
    pub fn as_f64(&self) -> f64 {
        self.floor as f64 + self.frac
    }
}

/// The exact rational charge, by 256-bit arithmetic. Same guards and factor
/// clamp as the production kernel.
pub fn rational_charge(
    value: u64,
    factor: u64,
    elapsed: u64,
    rate_bps: u32,
    bpy: u64,
) -> RationalCharge {
    let p = progressivity(factor);
    if rate_bps == 0 || elapsed == 0 || bpy == 0 || p == 0 || value == 0 {
        return RationalCharge::ZERO;
    }
    let num = value as u128 * rate_bps as u128 * p as u128; // < 2^109
    let den = BPS * PROG_SPAN * bpy as u128; // < 2^90
    let (hi, lo) = mul_wide(num, elapsed as u128);
    let ((q_hi, q_lo), rem) = divrem_wide(hi, lo, den);
    let saturated = q_hi != 0 || q_lo > u64::MAX as u128;
    RationalCharge {
        floor: if saturated { u64::MAX } else { q_lo as u64 },
        saturated,
        frac: rem as f64 / den as f64,
        frac_is_zero: rem == 0,
        is_zero: false,
    }
}

// ============================================================================
// Candidate kernels
// ============================================================================

/// The pre-9.0.0 production formula with the time fraction floored
/// (`ceil_time = false`, identical to the legacy `demurrage_charge` and the
/// study's "defer" / "today" baseline) or rounded up (option 2b).
pub fn quantized_time_charge(
    value: u64,
    factor: u64,
    elapsed: u64,
    rate_bps: u32,
    bpy: u64,
    ceil_time: bool,
) -> u64 {
    if rate_bps == 0 || elapsed == 0 || bpy == 0 {
        return 0;
    }
    let p = progressivity(factor);
    if p == 0 {
        return 0;
    }
    let scaled = elapsed as u128 * TIME_SCALE;
    let time_fraction = if ceil_time {
        scaled.div_ceil(bpy as u128)
    } else {
        scaled / bpy as u128
    };
    let base = value as u128 * rate_bps as u128 * p as u128 / BPS / PROG_SPAN;
    let charge = base.saturating_mul(time_fraction) / TIME_SCALE;
    charge.min(u64::MAX as u128) as u64
}

/// Option 1 (and 2c with `round_up`): the exact charge with no intermediate
/// quantization, in the production-shaped form a consensus fix would use —
/// `u128` only, no 256-bit type.
///
/// ```text
/// num = value × rate × p              (< 2^109)
/// den = 10_000 × 5_000 × bpy          (< 2^90)
/// charge = (num / den) × elapsed + floor((num % den) × elapsed / den)
/// ```
///
/// Exact (equal to the 256-bit oracle, then clamped) for every input with
/// `bpy ≤` [`EXACT_BPY_LIMIT`] (≈ 3.69e11, i.e. any block time above ~86 µs).
/// Beyond that the remainder product can overflow and the kernel saturates
/// to `u64::MAX`, which only ever raises the fee floor.
pub fn exact_charge(
    value: u64,
    factor: u64,
    elapsed: u64,
    rate_bps: u32,
    bpy: u64,
    round_up: bool,
) -> u64 {
    if rate_bps == 0 || elapsed == 0 || bpy == 0 {
        return 0;
    }
    let p = progressivity(factor);
    if p == 0 {
        return 0;
    }
    let num = value as u128 * rate_bps as u128 * p as u128;
    let den = BPS * PROG_SPAN * bpy as u128;
    let elapsed = elapsed as u128;
    let Some(whole) = (num / den).checked_mul(elapsed) else {
        return u64::MAX;
    };
    let Some(rem_scaled) = (num % den).checked_mul(elapsed) else {
        return u64::MAX;
    };
    let mut part = rem_scaled / den;
    if round_up && rem_scaled % den != 0 {
        part += 1;
    }
    whole.saturating_add(part).min(u64::MAX as u128) as u64
}

/// A candidate fix (or the status quo).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Candidate {
    /// Option 4: defer (the pre-9.0.0 quantized `demurrage_charge`).
    Defer,
    /// Option 1: exact, floored once at the end.
    Exact,
    /// Option 2a: pre-9.0.0 charge, at least 1 pico if the true charge > 0.
    AtLeastOne,
    /// Option 2b: pre-9.0.0 formula with the time fraction rounded up.
    CeilTime,
    /// Option 2c (variant): exact, rounded up once at the end.
    ExactCeil,
    /// Option 3a: every ring member must be at least `M` blocks old.
    MinMemberAge(u64),
    /// Option 3b: the oldest ring member must be at least `M` blocks old
    /// (equivalently, the elapsed statistic is clamped to `>= M`).
    MinRingMax(u64),
}

impl Candidate {
    /// The candidate columns of the report, in order.
    pub fn all() -> Vec<Candidate> {
        vec![
            Candidate::Defer,
            Candidate::Exact,
            Candidate::AtLeastOne,
            Candidate::CeilTime,
            Candidate::ExactCeil,
            Candidate::MinMemberAge(7),
            Candidate::MinMemberAge(10),
            Candidate::MinRingMax(7),
            Candidate::MinRingMax(10),
        ]
    }

    /// Short column label.
    pub fn label(&self) -> String {
        match self {
            Candidate::Defer => "4 defer".into(),
            Candidate::Exact => "1 exact".into(),
            Candidate::AtLeastOne => "2a >=1 pico".into(),
            Candidate::CeilTime => "2b ceil time".into(),
            Candidate::ExactCeil => "2c exact ceil".into(),
            Candidate::MinMemberAge(m) => format!("3a all>={m}"),
            Candidate::MinRingMax(m) => format!("3b max>={m}"),
        }
    }

    /// The charge for a spend whose ring members are all exactly `elapsed`
    /// blocks old (the attacker's ring), or `None` if consensus rejects it.
    pub fn charge(
        &self,
        value: u64,
        factor: u64,
        elapsed: u64,
        rate_bps: u32,
        bpy: u64,
    ) -> Option<u64> {
        self.charge_ring(value, factor, elapsed, elapsed, rate_bps, bpy)
    }

    /// The charge for a spend whose ring's youngest member is `min_age` and
    /// oldest is `elapsed` (the consensus `@max` statistic), or `None` if
    /// consensus rejects the spend (option 3a: `min_age < M`).
    pub fn charge_ring(
        &self,
        value: u64,
        factor: u64,
        min_age: u64,
        elapsed: u64,
        rate_bps: u32,
        bpy: u64,
    ) -> Option<u64> {
        Some(match *self {
            Candidate::Defer => quantized_time_charge(value, factor, elapsed, rate_bps, bpy, false),
            Candidate::Exact => exact_charge(value, factor, elapsed, rate_bps, bpy, false),
            Candidate::AtLeastOne => {
                let c = quantized_time_charge(value, factor, elapsed, rate_bps, bpy, false);
                if c == 0 && !rational_charge(value, factor, elapsed, rate_bps, bpy).is_zero {
                    1
                } else {
                    c
                }
            }
            Candidate::CeilTime => {
                quantized_time_charge(value, factor, elapsed, rate_bps, bpy, true)
            }
            Candidate::ExactCeil => exact_charge(value, factor, elapsed, rate_bps, bpy, true),
            Candidate::MinMemberAge(m) => {
                if min_age < m {
                    return None;
                }
                quantized_time_charge(value, factor, elapsed, rate_bps, bpy, false)
            }
            Candidate::MinRingMax(m) => {
                quantized_time_charge(value, factor, elapsed.max(m), rate_bps, bpy, false)
            }
        })
    }
}

// ============================================================================
// (a) The churn attack
// ============================================================================

/// The fee a churner pays on each hop, besides the demurrage itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CostModel {
    /// Consensus floor only, one coin, decoys from a busy chain.
    Cheapest,
    /// `max(floor, RELAY_MIN_TX_FEE)`, principal split into
    /// [`SELF_DECOY_COINS`] coins that are each other's decoys.
    Dearest,
}

impl CostModel {
    fn coins(self) -> u64 {
        match self {
            CostModel::Cheapest => 1,
            CostModel::Dearest => SELF_DECOY_COINS,
        }
    }
}

/// Size/output part of the consensus floor for a 1-in-1-out CLSAG spend.
pub fn hop_base_fee(factor: u64) -> u64 {
    let params = ConsensusFeeParams::mainnet(FeeModelMode::ConsensusOnly);
    consensus_fee_breakdown(
        &ConsensusFeeInputs {
            size: consensus_tx_size_bytes(1, 1, 0),
            n_out: 1,
            n_memo: 0,
            claimed: ClaimedClass::Factor(factor),
            floor: factor,
            elapsed: 0,
            height: params.first_demurrage_height(),
            output_sum: 0,
        },
        &params,
    )
    .base
}

/// One year of churning at hold `h`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChurnYear {
    pub hold: u64,
    /// Transactions sent (all coins).
    pub hops: u64,
    /// Demurrage the churner pays, pico.
    pub paid: u128,
    /// Demurrage a buy-and-hold holder pays over the same window, same rule.
    pub baseline: u128,
    /// Non-demurrage fees on the churner's extra transactions, pico.
    pub fees: u128,
}

impl ChurnYear {
    /// Demurrage escaped (negative = churning costs extra demurrage).
    pub fn escaped(&self) -> i128 {
        self.baseline as i128 - self.paid as i128
    }

    /// Escaped demurrage net of fees.
    pub fn net(&self) -> i128 {
        self.escaped() - self.fees as i128
    }

    /// Escaped demurrage as a fraction of the baseline.
    pub fn escaped_frac(&self) -> f64 {
        if self.baseline == 0 {
            0.0
        } else {
            self.escaped() as f64 / self.baseline as f64
        }
    }
}

/// Simulate one year (`floor(bpy/h)·h` blocks) of churning `principal` pico
/// at factor `factor` in hops of `hold` blocks under `candidate`, or `None` if
/// consensus rejects every hop.
pub fn churn_year(
    candidate: Candidate,
    principal: u64,
    factor: u64,
    hold: u64,
    cost: CostModel,
) -> Option<ChurnYear> {
    churn_year_with_base(
        candidate,
        principal,
        factor,
        hold,
        cost,
        hop_base_fee(factor),
    )
}

/// [`churn_year`] with the hop base fee ([`hop_base_fee`]) precomputed.
pub fn churn_year_with_base(
    candidate: Candidate,
    principal: u64,
    factor: u64,
    hold: u64,
    cost: CostModel,
    base_fee: u64,
) -> Option<ChurnYear> {
    let k = cost.coins();
    let coin = principal / k;
    let n = BLOCKS_PER_YEAR / hold;
    let window = n * hold;
    let per_hop = candidate.charge(coin, factor, hold, RATE_BPS, BLOCKS_PER_YEAR)? as u128;
    let paid = per_hop * (n * k) as u128;
    let baseline = candidate.charge(principal, factor, window, RATE_BPS, BLOCKS_PER_YEAR)? as u128;
    let base = base_fee as u128;
    let hop_fee = match cost {
        CostModel::Cheapest => base,
        CostModel::Dearest => (base + per_hop).max(RELAY_MIN_TX_FEE as u128) - per_hop,
    };
    let hops = n * k;
    // The holder spends once too; the churner's cost is its extra hops.
    let fees = hop_fee * (hops - 1) as u128;
    Some(ChurnYear {
        hold,
        hops,
        paid,
        baseline,
        fees,
    })
}

/// The churner's best hold in `1..=max_hold` (largest net gain), or `None`
/// if no hold is profitable or allowed.
pub fn best_churn(
    candidate: Candidate,
    principal: u64,
    factor: u64,
    max_hold: u64,
    cost: CostModel,
) -> Option<ChurnYear> {
    let base = hop_base_fee(factor);
    (1..=max_hold)
        .filter_map(|h| churn_year_with_base(candidate, principal, factor, h, cost, base))
        .filter(|c| c.net() > 0)
        .max_by_key(|c| (c.net(), std::cmp::Reverse(c.hold)))
}

/// Largest hold in `1..=max_hold` whose net escape is at least
/// `threshold_bps` of the buy-and-hold charge (`threshold_bps = 0`: any
/// profit), or `None` if no hold reaches it.
pub fn break_even_hold(
    candidate: Candidate,
    principal: u64,
    factor: u64,
    max_hold: u64,
    cost: CostModel,
    threshold_bps: u32,
) -> Option<u64> {
    let base = hop_base_fee(factor);
    (1..=max_hold).rev().find(|&h| {
        churn_year_with_base(candidate, principal, factor, h, cost, base).is_some_and(|c| {
            c.net() > 0 && c.net() * 10_000 >= c.baseline as i128 * threshold_bps as i128
        })
    })
}

/// A principal × cluster cell of the attack grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttackCell {
    pub principal_bth: u64,
    pub cluster_bth: u64,
    pub factor: u64,
}

impl AttackCell {
    pub fn principal(&self) -> u64 {
        bth(self.principal_bth)
    }
}

/// Principals of the attack grid, BTH.
pub const PRINCIPALS_BTH: [u64; 4] = [1_000, 100_000, 1_000_000, 10_000_000];

/// Cluster wealths of the attack grid, BTH (1B BTH is past the curve's 6.0×
/// saturation at ~409.6M BTH).
pub const CLUSTERS_BTH: [u64; 4] = [100_000, 1_000_000, 10_000_000, 1_000_000_000];

/// BTH → pico as `u64` (every amount here is below 18.4M BTH).
pub fn bth(amount: u64) -> u64 {
    u64::try_from(amount as u128 * PICO_PER_BTH).expect("fits u64")
}

/// Factor of a cluster of `cluster_bth` on the production curve.
pub fn cluster_factor(cluster_bth: u64) -> u64 {
    ClusterFactorCurve::default().factor(cluster_bth as u128 * PICO_PER_BTH)
}

/// The grid cells: every principal in every cluster at least as large.
pub fn attack_cells() -> Vec<AttackCell> {
    let mut out = Vec::new();
    for &p in &PRINCIPALS_BTH {
        for &c in &CLUSTERS_BTH {
            if c >= p {
                out.push(AttackCell {
                    principal_bth: p,
                    cluster_bth: c,
                    factor: cluster_factor(c),
                });
            }
        }
    }
    out
}

/// The headline cell: the #1899 repro's 10M BTH cluster with a 1M BTH coin.
pub fn headline_cell() -> AttackCell {
    AttackCell {
        principal_bth: 1_000_000,
        cluster_bth: 10_000_000,
        factor: cluster_factor(10_000_000),
    }
}

// ============================================================================
// (b) Honest users
// ============================================================================

/// One honest spend: the ring's elapsed statistic, value and factor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HonestSpend {
    /// Oldest ring member's age (the consensus elapsed statistic).
    pub elapsed: u64,
    /// Youngest ring member's age (what option 3a checks).
    pub min_age: u64,
    pub real_age: u64,
    pub value: u64,
    pub factor: u64,
}

/// A deterministic log-uniform value in `[1 BTH, 1M BTH]` and a factor from
/// the grid's clusters.
fn draw_value_factor(rng: &mut ChaCha8Rng) -> (u64, u64) {
    let exp: f64 = rng.random::<f64>() * 6.0;
    let value_bth = 10f64.powf(exp);
    let value = (value_bth * PICO_PER_BTH as f64) as u64;
    let factor = cluster_factor(CLUSTERS_BTH[rng.random_range(0..CLUSTERS_BTH.len())]);
    (value.max(1), factor)
}

/// Rings from the node-picker port under the gamma spend model on the
/// default 1.2M-block chain.
pub fn gamma_rings(n: usize, seed: u64) -> Vec<RouteRingAges> {
    sample_route_ring_ages(
        &ChainModel::default(),
        DecoyRoute::Node,
        RealAgeSpec::GammaSpend,
        n,
        seed,
    )
}

/// Honest spends built from [`gamma_rings`].
pub fn gamma_population(n: usize, seed: u64) -> Vec<HonestSpend> {
    let rings = gamma_rings(n, seed);
    let mut rng = ChaCha8Rng::seed_from_u64(seed ^ 0x6A11);
    rings
        .iter()
        .map(|r| {
            let (value, factor) = draw_value_factor(&mut rng);
            HonestSpend {
                elapsed: r.decoy_ages.iter().copied().fold(r.real_age, u64::max),
                min_age: r.decoy_ages.iter().copied().fold(r.real_age, u64::min),
                real_age: r.real_age,
                value,
                factor,
            }
        })
        .collect()
}

/// Real ages of the short-hold commerce population (10 blocks = 50 s to
/// 1 day).
pub const SHORT_HOLD_AGES: [u64; 8] = [10, 12, 20, 60, 120, 720, 4_320, 17_280];

/// Honest short-hold spends: rings from the node-picker port on a
/// one-output-per-block chain for each [`SHORT_HOLD_AGES`] real age.
pub fn short_hold_population(per_age: usize, seed: u64) -> Vec<HonestSpend> {
    let chain = ChainModel {
        tip_height: 20_000,
        outputs_per_block: 1,
    };
    let mut rng = ChaCha8Rng::seed_from_u64(seed ^ 0x5407);
    let mut out = Vec::new();
    for &age in &SHORT_HOLD_AGES {
        let rings = sample_route_ring_ages(
            &chain,
            DecoyRoute::Node,
            RealAgeSpec::Fixed(age),
            per_age,
            seed ^ age,
        );
        for r in rings {
            let (value, factor) = draw_value_factor(&mut rng);
            out.push(HonestSpend {
                elapsed: r.decoy_ages.iter().copied().fold(r.real_age, u64::max),
                min_age: r.decoy_ages.iter().copied().fold(r.real_age, u64::min),
                real_age: r.real_age,
                value,
                factor,
            });
        }
    }
    out
}

/// Per-kernel honest-impact statistics over a population.
#[derive(Clone, Debug, PartialEq)]
pub struct HonestStats {
    pub candidate: Candidate,
    /// Σ kernel / Σ today − 1 (extra charge versus today).
    pub extra_vs_today: f64,
    /// Σ (kernel − today), BTH, per 1,000 spends.
    pub extra_bth_per_1k: f64,
    /// Σ kernel / Σ true rational − 1.
    pub bias_vs_true: f64,
    /// Per-spend `kernel − true` in pico: min, median, p99, max.
    pub err_min: f64,
    pub err_p50: f64,
    pub err_p99: f64,
    pub err_max: f64,
    /// Largest per-spend |kernel − true| / true.
    pub max_rel_err: f64,
    /// Share of spends whose charge differs from today's.
    pub changed_share: f64,
    /// Share of spends consensus rejects (option 3a).
    pub rejected_share: f64,
}

fn percentile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((q * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len()) - 1;
    sorted[idx]
}

/// Score `candidate` on an honest population.
pub fn honest_stats(candidate: Candidate, pop: &[HonestSpend]) -> HonestStats {
    let (mut sum_k, mut sum_today, mut sum_true) = (0f64, 0f64, 0f64);
    let mut extra_pico = 0i128;
    let mut errs = Vec::with_capacity(pop.len());
    let (mut max_rel, mut changed, mut rejected) = (0f64, 0usize, 0usize);
    for s in pop {
        let today = quantized_time_charge(
            s.value,
            s.factor,
            s.elapsed,
            RATE_BPS,
            BLOCKS_PER_YEAR,
            false,
        );
        let truth = rational_charge(s.value, s.factor, s.elapsed, RATE_BPS, BLOCKS_PER_YEAR);
        let Some(k) = candidate.charge_ring(
            s.value,
            s.factor,
            s.min_age,
            s.elapsed,
            RATE_BPS,
            BLOCKS_PER_YEAR,
        ) else {
            rejected += 1;
            continue;
        };
        sum_k += k as f64;
        sum_today += today as f64;
        sum_true += truth.as_f64();
        extra_pico += k as i128 - today as i128;
        changed += usize::from(k != today);
        let e = truth.error_of(k);
        errs.push(e);
        if !truth.is_zero {
            max_rel = max_rel.max(e.abs() / truth.as_f64());
        }
    }
    errs.sort_by(|a, b| a.total_cmp(b));
    let n = pop.len().max(1) as f64;
    HonestStats {
        candidate,
        extra_vs_today: if sum_today > 0.0 {
            sum_k / sum_today - 1.0
        } else {
            0.0
        },
        extra_bth_per_1k: extra_pico as f64 / PICO_PER_BTH as f64 * 1000.0 / n,
        bias_vs_true: if sum_true > 0.0 {
            sum_k / sum_true - 1.0
        } else {
            0.0
        },
        err_min: errs.first().copied().unwrap_or(0.0),
        err_p50: percentile(&errs, 0.5),
        err_p99: percentile(&errs, 0.99),
        err_max: errs.last().copied().unwrap_or(0.0),
        max_rel_err: max_rel,
        changed_share: changed as f64 / n,
        rejected_share: rejected as f64 / n,
    }
}

/// Split-versus-unsplit loss: `charge(whole) − Σ charge(parts)` (positive =
/// splitting pays less), worst cases over random splits.
#[derive(Clone, Debug, PartialEq)]
pub struct SplitStats {
    pub candidate: Candidate,
    pub parts: u64,
    /// Time split: one hold of `h` vs `parts` consecutive holds summing to
    /// `h`. Max and min loss, pico.
    pub time_max: i128,
    pub time_min: i128,
    /// Value split: one coin vs `parts` coins summing to it, same hold.
    pub value_max: i128,
    pub value_min: i128,
}

/// Random composition of `total` into `parts` positive pieces.
fn compose(total: u64, parts: u64, rng: &mut ChaCha8Rng) -> Vec<u64> {
    let mut cuts: Vec<u64> = (0..parts - 1).map(|_| rng.random_range(1..total)).collect();
    cuts.sort_unstable();
    cuts.dedup();
    let mut out = Vec::with_capacity(parts as usize);
    let mut prev = 0;
    for c in cuts {
        out.push(c - prev);
        prev = c;
    }
    out.push(total - prev);
    out
}

/// Worst split-versus-unsplit loss of `candidate` over `trials` random
/// splits into `parts` pieces. Option 3 candidates evaluate parts below the
/// minimum at the minimum (3b) or skip the trial (3a).
pub fn split_stats(candidate: Candidate, parts: u64, trials: usize, seed: u64) -> SplitStats {
    let mut rng = ChaCha8Rng::seed_from_u64(seed ^ parts);
    let (mut tmax, mut tmin, mut vmax, mut vmin) = (i128::MIN, i128::MAX, i128::MIN, i128::MAX);
    let ch = |v: u64, f: u64, h: u64| candidate.charge(v, f, h, RATE_BPS, BLOCKS_PER_YEAR);
    for _ in 0..trials {
        let (value, factor) = draw_value_factor(&mut rng);
        let value = value.max(parts);
        // Holds from 1 block to ~1 week, log-uniform.
        let hold = (10f64.powf(rng.random::<f64>() * 5.0) as u64).max(parts + 1);
        if let Some(whole) = ch(value, factor, hold) {
            let pieces = compose(hold, parts, &mut rng);
            let split: Option<u128> = pieces
                .iter()
                .map(|&h| ch(value, factor, h).map(u128::from))
                .sum();
            if let Some(split) = split {
                let loss = whole as i128 - split as i128;
                tmax = tmax.max(loss);
                tmin = tmin.min(loss);
            }
            let vparts = compose(value, parts, &mut rng);
            let vsplit: Option<u128> = vparts
                .iter()
                .map(|&v| ch(v, factor, hold).map(u128::from))
                .sum();
            if let Some(vsplit) = vsplit {
                let loss = whole as i128 - vsplit as i128;
                vmax = vmax.max(loss);
                vmin = vmin.min(loss);
            }
        }
    }
    let fix = |x: i128, empty: i128| if x == empty { 0 } else { x };
    SplitStats {
        candidate,
        parts,
        time_max: fix(tmax, i128::MIN),
        time_min: fix(tmin, i128::MAX),
        value_max: fix(vmax, i128::MIN),
        value_min: fix(vmin, i128::MAX),
    }
}

// ============================================================================
// (c) Option 3: young-coin spendability and ring privacy
// ============================================================================

/// Rings built for one real age under a consensus minimum member age.
#[derive(Clone, Debug, PartialEq)]
pub struct YoungRingRow {
    pub real_age: u64,
    pub outputs_per_block: u64,
    /// Consensus minimum member age (3a); decoys younger are unusable.
    /// `MIN_DECOY_AGE_BLOCKS` (10) = today's wallet floor.
    pub min_age: u64,
    pub rings: usize,
    /// Share of rings in which the real input is the youngest member, ties
    /// split evenly (ideal picker: 1/20 = 5%).
    pub real_youngest: f64,
    /// Largest share of any single age rank (ties split): the success rate
    /// of a picker-aware adversary that always guesses the most likely rank
    /// (in-sample; ideal 5%).
    pub best_rank_rate: f64,
    /// Median ring max age (the consensus elapsed statistic).
    pub median_elapsed: u64,
    /// Median `elapsed / real_age` (the `@max` over-charge).
    pub median_overcharge: f64,
    /// Blocks the coin must wait under 3a before it may be spent.
    pub wait_blocks: u64,
}

/// Build `rings` rings with the node-picker port for a real input of
/// `real_age` on a chain of `outputs_per_block`, with decoys restricted to
/// ages `>= min_age` (a 3a minimum above the wallet's 10-block floor).
pub fn young_ring_row(
    real_age: u64,
    outputs_per_block: u64,
    min_age: u64,
    rings: usize,
    seed: u64,
) -> YoungRingRow {
    let floor = min_age.max(MIN_DECOY_AGE_BLOCKS);
    let tip = (4 * real_age.max(floor)).max(400) + 200;
    let chain = ChainModel {
        tip_height: tip,
        outputs_per_block,
    };
    let real_id = chain.id_at(real_age, 0);
    let real_key = ChainModel::key(real_id);
    let candidates: Vec<RouteCandidate> = chain
        .candidates_by_age(floor, chain.max_age())
        .into_iter()
        .filter(|c| c.key != real_key)
        .collect();
    let mut rng = ChaCha8Rng::seed_from_u64(
        seed ^ real_age.wrapping_mul(0x9E37) ^ (outputs_per_block << 40) ^ (min_age << 20),
    );
    let mut hist = [0f64; RING_SIZE];
    let mut elapsed = Vec::with_capacity(rings);
    let mut built = 0usize;
    for _ in 0..rings {
        let Ok(keys) = node_select_decoys_for_input(
            &candidates,
            RING_SIZE - 1,
            &[real_key],
            real_age,
            &mut rng,
        ) else {
            continue;
        };
        built += 1;
        let ages: Vec<u64> = keys
            .iter()
            .map(|k| chain.age_of(ChainModel::id_of(k)))
            .collect();
        let younger = ages.iter().filter(|&&a| a < real_age).count();
        let tied = ages.iter().filter(|&&a| a == real_age).count();
        for slot in &mut hist[younger..=younger + tied] {
            *slot += 1.0 / (tied + 1) as f64;
        }
        elapsed.push(ages.iter().copied().fold(real_age, u64::max));
    }
    elapsed.sort_unstable();
    let median_elapsed = elapsed.get(elapsed.len() / 2).copied().unwrap_or(0);
    let denom = built.max(1) as f64;
    YoungRingRow {
        real_age,
        outputs_per_block,
        min_age,
        rings: built,
        real_youngest: hist[0] / denom,
        best_rank_rate: hist.iter().copied().fold(0.0, f64::max) / denom,
        median_elapsed,
        median_overcharge: median_elapsed as f64 / real_age.max(1) as f64,
        wait_blocks: min_age.saturating_sub(real_age),
    }
}

/// Share of honest real inputs and decoys younger than `m` in a gamma-model
/// ring sample.
pub fn gamma_share_below(m: u64, rings: &[RouteRingAges]) -> (f64, f64) {
    let total = rings.len().max(1) as f64;
    let real = rings.iter().filter(|r| r.real_age < m).count() as f64 / total;
    let decoys: usize = rings.iter().map(|r| r.decoy_ages.len()).sum();
    let young: usize = rings
        .iter()
        .map(|r| r.decoy_ages.iter().filter(|&&a| a < m).count())
        .sum();
    (real, young as f64 / decoys.max(1) as f64)
}

/// Option 3 with today's floored kernel: the worst gross escape fraction of
/// any hold `h >= m` (value-independent up to the base floor), searched over
/// `m..m+span`.
pub fn worst_escape_at_or_above(m: u64, span: u64) -> (u64, f64) {
    let cell = headline_cell();
    let base = hop_base_fee(cell.factor);
    (m..m + span)
        .filter_map(|h| {
            churn_year_with_base(
                Candidate::Defer,
                cell.principal(),
                cell.factor,
                h,
                CostModel::Cheapest,
                base,
            )
            .map(|c| (h, c.escaped_frac()))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)))
        .unwrap_or((m, 0.0))
}

// ============================================================================
// (d) Extreme inputs
// ============================================================================

/// An extreme-input probe: `(label, value, factor, elapsed, rate, bpy)`.
pub type Extreme = (&'static str, u64, u64, u64, u32, u64);

/// Extreme and boundary inputs for the determinism / overflow table.
pub fn extremes() -> Vec<Extreme> {
    vec![
        (
            "u64::MAX value, 6 blocks",
            u64::MAX,
            6000,
            6,
            200,
            BLOCKS_PER_YEAR,
        ),
        (
            "u64::MAX value, 7 blocks",
            u64::MAX,
            6000,
            7,
            200,
            BLOCKS_PER_YEAR,
        ),
        (
            "u64::MAX value, 1 year",
            u64::MAX,
            6000,
            BLOCKS_PER_YEAR,
            200,
            BLOCKS_PER_YEAR,
        ),
        (
            "u64::MAX value, u64::MAX elapsed",
            u64::MAX,
            6000,
            u64::MAX,
            200,
            BLOCKS_PER_YEAR,
        ),
        ("1 pico, 1 block, 1.001x", 1, 1001, 1, 200, BLOCKS_PER_YEAR),
        (
            "1M BTH, u32::MAX bps, 1 block",
            bth(1_000_000),
            6000,
            1,
            u32::MAX,
            BLOCKS_PER_YEAR,
        ),
        (
            "u64::MAX all, bpy = 1",
            u64::MAX,
            u64::MAX,
            u64::MAX,
            u32::MAX,
            1,
        ),
        (
            "u64::MAX all, bpy = limit",
            u64::MAX,
            6000,
            u64::MAX,
            u32::MAX,
            EXACT_BPY_LIMIT,
        ),
        (
            "1M BTH, 1 block, bpy = limit",
            bth(1_000_000),
            6000,
            1,
            200,
            EXACT_BPY_LIMIT,
        ),
        (
            "u64::MAX value, 1 block, bpy = u64::MAX",
            u64::MAX,
            6000,
            1,
            u32::MAX,
            u64::MAX,
        ),
        (
            "u64::MAX value+elapsed, bpy = u64::MAX",
            u64::MAX,
            6000,
            u64::MAX,
            u32::MAX,
            u64::MAX,
        ),
    ]
}

/// The kernels compared in the extreme table (option 3 uses today's kernel).
pub fn kernel_candidates() -> Vec<Candidate> {
    vec![
        Candidate::Defer,
        Candidate::Exact,
        Candidate::AtLeastOne,
        Candidate::CeilTime,
        Candidate::ExactCeil,
    ]
}

// ============================================================================
// Report
// ============================================================================

/// Sweep sizes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SweepParams {
    /// Holds of the profile table, `1..=profile_holds`.
    pub profile_holds: u64,
    /// Holds searched for the attack optimum and break-even.
    pub search_holds: u64,
    /// Gamma-model honest spends.
    pub gamma_spends: usize,
    /// Short-hold honest spends per real age.
    pub short_per_age: usize,
    /// Random splits per (candidate, parts).
    pub split_trials: usize,
    /// Rings per young-ring row.
    pub young_rings: usize,
    pub seed: u64,
}

impl Default for SweepParams {
    fn default() -> Self {
        Self {
            profile_holds: 50,
            search_holds: 17_280,
            gamma_spends: 1_000,
            short_per_age: 150,
            split_trials: 2_000,
            young_rings: 300,
            seed: SEED,
        }
    }
}

impl SweepParams {
    /// A small configuration for unit tests.
    pub fn smoke() -> Self {
        Self {
            profile_holds: 14,
            search_holds: 60,
            gamma_spends: 40,
            short_per_age: 6,
            split_trials: 60,
            young_rings: 20,
            seed: SEED,
        }
    }
}

fn fmt_bth(pico: i128) -> String {
    format!("{:.4}", pico as f64 / PICO_PER_BTH as f64)
}

fn fmt_factor(f: u64) -> String {
    format!("{:.3}x", f as f64 / FACTOR_SCALE as f64)
}

fn fmt_cluster(c: u64) -> String {
    match c {
        c if c >= 1_000_000_000 => format!("{}B", c / 1_000_000_000),
        c if c >= 1_000_000 => format!("{}M", c / 1_000_000),
        c => format!("{}k", c / 1_000),
    }
}

fn table_header(out: &mut String, cols: &[String]) {
    out.push_str(&format!("| {} |\n", cols.join(" | ")));
    let seps: Vec<&str> = cols.iter().map(|_| "---").collect();
    out.push_str(&format!("|{}|\n", seps.join("|")));
}

fn section_attack(p: &SweepParams, out: &mut String) {
    let cands = Candidate::all();
    let cell = headline_cell();
    let truth = rational_charge(
        cell.principal(),
        cell.factor,
        BLOCKS_PER_YEAR,
        RATE_BPS,
        BLOCKS_PER_YEAR,
    );
    out.push_str("## (a) Churn attack\n\n");
    out.push_str(&format!(
        "Headline cell: {} BTH coin in a {} BTH cluster (factor {}), rate {} bps. \
         A one-year hold owes {} BTH (exact). Hop fee (1-in-1-out, consensus floor): {} pico; \
         relay minimum: {} pico.\n\n",
        cell.principal_bth,
        fmt_cluster(cell.cluster_bth),
        fmt_factor(cell.factor),
        RATE_BPS,
        fmt_bth(truth.floor as i128),
        hop_base_fee(cell.factor),
        RELAY_MIN_TX_FEE,
    ));

    out.push_str(
        "### A1. Gross escape by hold length (headline cell, % of the buy-and-hold charge under the same rule; `rej` = consensus rejects the spend)\n\n",
    );
    let mut cols = vec!["hold".to_string()];
    cols.extend(cands.iter().map(|c| c.label()));
    table_header(out, &cols);
    for h in 1..=p.profile_holds {
        let mut row = vec![h.to_string()];
        for c in &cands {
            row.push(
                match churn_year(*c, cell.principal(), cell.factor, h, CostModel::Cheapest) {
                    Some(y) => format!("{:.2}%", 100.0 * y.escaped_frac()),
                    None => "rej".into(),
                },
            );
        }
        out.push_str(&format!("| {} |\n", row.join(" | ")));
    }
    out.push('\n');

    for (cost, title) in [
        (
            CostModel::Cheapest,
            "A2. Net escape at the attack optimum, cheapest cost (consensus floor, 1 coin): BTH/yr (best hold)",
        ),
        (
            CostModel::Dearest,
            "A3. Net escape at the attack optimum, dearest cost (relay minimum, 20 self-decoy coins): BTH/yr (best hold)",
        ),
    ] {
        out.push_str(&format!(
            "### {title}; holds searched 1..={}\n\n",
            p.search_holds
        ));
        let mut cols = vec!["principal".to_string(), "cluster".into(), "factor".into()];
        cols.extend(cands.iter().map(|c| c.label()));
        table_header(out, &cols);
        for cell in attack_cells() {
            let mut row = vec![
                cell.principal_bth.to_string(),
                fmt_cluster(cell.cluster_bth),
                fmt_factor(cell.factor),
            ];
            for c in &cands {
                row.push(
                    match best_churn(*c, cell.principal(), cell.factor, p.search_holds, cost) {
                        Some(y) => format!("{} (h={})", fmt_bth(y.net()), y.hold),
                        None => "none".into(),
                    },
                );
            }
            out.push_str(&format!("| {} |\n", row.join(" | ")));
        }
        out.push('\n');
    }

    out.push_str(&format!(
        "### A4. Break-even and materiality holds, dearest cost: largest hold in 1..={cap} with net escape > 0 / >= 1% / >= 0.1% of the buy-and-hold charge (`none` = never; `{cap}` = still true at the search cap)\n\n",
        cap = p.search_holds
    ));
    let mut cols = vec!["principal".to_string(), "cluster".into()];
    cols.extend(cands.iter().map(|c| c.label()));
    table_header(out, &cols);
    for cell in attack_cells() {
        let mut row = vec![
            cell.principal_bth.to_string(),
            fmt_cluster(cell.cluster_bth),
        ];
        for c in &cands {
            let at = |bps: u32| {
                break_even_hold(
                    *c,
                    cell.principal(),
                    cell.factor,
                    p.search_holds,
                    CostModel::Dearest,
                    bps,
                )
                .map(|h| h.to_string())
                .unwrap_or_else(|| "none".into())
            };
            row.push(format!("{} / {} / {}", at(0), at(100), at(10)));
        }
        out.push_str(&format!("| {} |\n", row.join(" | ")));
    }
    out.push('\n');
}

fn section_honest(p: &SweepParams, out: &mut String) {
    out.push_str("## (b) Honest users\n\n");
    let honest_cands: Vec<Candidate> = Candidate::all();
    let pops = [
        (
            "gamma spend model (node picker, default chain)",
            gamma_population(p.gamma_spends, p.seed),
        ),
        (
            "short holds 10 blocks..1 day (node picker, 1 output/block)",
            short_hold_population(p.short_per_age, p.seed),
        ),
    ];
    for (i, (name, pop)) in pops.iter().enumerate() {
        let mut elapsed: Vec<u64> = pop.iter().map(|s| s.elapsed).collect();
        elapsed.sort_unstable();
        out.push_str(&format!(
            "### B{}. {} ({} spends; elapsed statistic min {}, median {} blocks; values log-uniform 1..1M BTH)\n\n",
            i + 1,
            name,
            pop.len(),
            elapsed.first().copied().unwrap_or(0),
            elapsed.get(elapsed.len() / 2).copied().unwrap_or(0),
        ));
        table_header(
            out,
            &[
                "kernel".into(),
                "extra vs today".into(),
                "extra BTH / 1k spends".into(),
                "bias vs exact rational".into(),
                "err min (pico)".into(),
                "err p50".into(),
                "err p99".into(),
                "err max".into(),
                "max rel err".into(),
                "spends changed".into(),
                "rejected".into(),
            ],
        );
        for c in &honest_cands {
            let s = honest_stats(*c, pop);
            out.push_str(&format!(
                "| {} | {:+.6}% | {:.6} | {:+.6}% | {:.3} | {:.3} | {:.3} | {:.3} | {:.3e} | {:.1}% | {:.1}% |\n",
                c.label(),
                100.0 * s.extra_vs_today,
                s.extra_bth_per_1k,
                100.0 * s.bias_vs_true,
                s.err_min,
                s.err_p50,
                s.err_p99,
                s.err_max,
                s.max_rel_err,
                100.0 * s.changed_share,
                100.0 * s.rejected_share,
            ));
        }
        out.push('\n');
    }

    out.push_str(&format!(
        "### B3. Split-versus-unsplit loss, pico: `charge(whole) − Σ charge(parts)` over {} random splits (positive = splitting pays less; holds log-uniform 1 block..~1 week)\n\n",
        p.split_trials
    ));
    table_header(
        out,
        &[
            "kernel".into(),
            "parts".into(),
            "time split max".into(),
            "time split min".into(),
            "value split max".into(),
            "value split min".into(),
        ],
    );
    for c in Candidate::all() {
        for parts in [2u64, 10] {
            let s = split_stats(c, parts, p.split_trials, p.seed);
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} |\n",
                c.label(),
                parts,
                s.time_max,
                s.time_min,
                s.value_max,
                s.value_min
            ));
        }
    }
    out.push('\n');
}

fn section_option3(p: &SweepParams, out: &mut String) {
    out.push_str("## (c) Option 3: minimum ring-member age\n\n");
    out.push_str("### C1. Tuning M with today's floored kernel (headline cell)\n\n");
    table_header(
        out,
        &[
            "M (blocks)".into(),
            "M (time)".into(),
            "worst gross escape, h>=M".into(),
            "at h".into(),
            "3a net BTH/yr cheapest".into(),
            "3a net BTH/yr dearest".into(),
            "3b net BTH/yr dearest".into(),
            "3a max wait".into(),
        ],
    );
    let cell = headline_cell();
    for m in [7u64, 10, 13, 20, 64, 128, 631, 1_262, 6_308] {
        let (h, esc) = worst_escape_at_or_above(m, 64);
        let span = p.search_holds.max(m + 64);
        let net = |c: Candidate, cost: CostModel| {
            best_churn(c, cell.principal(), cell.factor, span, cost)
                .map(|y| fmt_bth(y.net()))
                .unwrap_or_else(|| "none".into())
        };
        let secs = (m - 1) * 5;
        let time = if secs < 120 {
            format!("{secs} s")
        } else {
            format!("{:.1} min", secs as f64 / 60.0)
        };
        out.push_str(&format!(
            "| {} | {} | {:.2}% | {} | {} | {} | {} | {} |\n",
            m,
            format_args!("{:.1} min", m as f64 * 5.0 / 60.0),
            100.0 * esc,
            h,
            net(Candidate::MinMemberAge(m), CostModel::Cheapest),
            net(Candidate::MinMemberAge(m), CostModel::Dearest),
            net(Candidate::MinRingMax(m), CostModel::Dearest),
            format_args!("{} blocks ({time})", m - 1),
        ));
    }
    out.push('\n');

    out.push_str(&format!(
        "### C2. Young real inputs under today's wallet floor (production picker ports, {} rings per row; youngest = real input has the youngest age rank, ties split; Age-Rank / Band-Center = `privacy_routes` adversaries' identification rates; ideal 5%)\n\n",
        p.young_rings
    ));
    table_header(
        out,
        &[
            "real age".into(),
            "outputs/block".into(),
            "node youngest".into(),
            "node Age-Rank".into(),
            "node Band-Center".into(),
            "web Age-Rank".into(),
            "cli refuses".into(),
            "node median @max overcharge".into(),
            "3a M=7 wait".into(),
            "3a M=10 wait".into(),
        ],
    );
    for &ops in &[1u64, 4] {
        let config = RouteSimConfig {
            rings: p.young_rings,
            chain: ChainModel {
                tip_height: 2_000,
                outputs_per_block: ops,
            },
            seed: p.seed,
            ..RouteSimConfig::default()
        };
        for &a in &[1u64, 3, 6, 7, 9, 10, 12, 15, 19, 20, 30, 60, 120] {
            let spec = RealAgeSpec::Fixed(a);
            let node = run_route_row(&config, DecoyRoute::Node, spec);
            let web = run_route_row(&config, DecoyRoute::Web, spec);
            let cli = run_route_row(&config, DecoyRoute::Cli, spec);
            let r = young_ring_row(a, ops, MIN_DECOY_AGE_BLOCKS, p.young_rings, p.seed);
            let rate_of = |row: &RouteRowResult, adversary: &str| {
                row.id_rate(adversary)
                    .map(|x| format!("{:.1}%", 100.0 * x))
                    .unwrap_or_default()
            };
            out.push_str(&format!(
                "| {} | {} | {:.1}% | {} | {} | {} | {:.0}% | {:.2} | {} | {} |\n",
                a,
                ops,
                100.0 * node.youngest_rate(),
                rate_of(&node, "Age-Rank"),
                rate_of(&node, "Band-Center"),
                rate_of(&web, "Age-Rank"),
                100.0 * cli.failures as f64 / (cli.failures + cli.rings).max(1) as f64,
                r.median_overcharge,
                7u64.saturating_sub(a),
                10u64.saturating_sub(a),
            ));
        }
    }
    out.push('\n');

    out.push_str(
        "### C3. A minimum above the wallet floor (3a, M > 10): real inputs just above M, node picker with decoys restricted to ages >= M (1 output/block; Age-Rank here is in-sample)\n\n",
    );
    table_header(
        out,
        &[
            "M".into(),
            "real age".into(),
            "youngest, floor M".into(),
            "Age-Rank, floor M".into(),
            "youngest, floor 10".into(),
            "Age-Rank, floor 10".into(),
            "median @max overcharge, floor M".into(),
        ],
    );
    for m in [20u64, 64, 631] {
        for a in [m, m + m / 20, m + m / 4, 2 * m] {
            let r = young_ring_row(a, 1, m, p.young_rings, p.seed);
            let base = young_ring_row(a, 1, MIN_DECOY_AGE_BLOCKS, p.young_rings, p.seed);
            out.push_str(&format!(
                "| {} | {} | {:.1}% | {:.1}% | {:.1}% | {:.1}% | {:.3} |\n",
                m,
                a,
                100.0 * r.real_youngest,
                100.0 * r.best_rank_rate,
                100.0 * base.real_youngest,
                100.0 * base.best_rank_rate,
                r.median_overcharge,
            ));
        }
    }
    out.push('\n');

    out.push_str(&format!(
        "### C4. Share of honest gamma-model rings a minimum excludes ({} rings)\n\n",
        p.gamma_spends
    ));
    table_header(
        out,
        &[
            "M".into(),
            "real inputs younger than M".into(),
            "decoys younger than M".into(),
        ],
    );
    let rings = gamma_rings(p.gamma_spends, p.seed);
    for m in [7u64, 10, 64, 631, 6_308, 17_280] {
        let (real, decoys) = gamma_share_below(m, &rings);
        out.push_str(&format!(
            "| {} | {:.2}% | {:.2}% |\n",
            m,
            100.0 * real,
            100.0 * decoys
        ));
    }
    out.push('\n');
}

fn section_extremes(out: &mut String) {
    out.push_str("## (d) Determinism and overflow at extreme inputs\n\n");
    out.push_str(&format!(
        "`exact` is exact up to `blocks_per_year` = {EXACT_BPY_LIMIT}; `oracle` is the 256-bit exact floor (clamped to u64::MAX). `=` marks a kernel equal to the oracle floor, `^` equal to its ceiling.\n\n"
    ));
    let mut cols = vec!["input".to_string(), "oracle floor".into()];
    cols.extend(kernel_candidates().iter().map(|c| c.label()));
    table_header(out, &cols);
    for (label, v, f, h, r, bpy) in extremes() {
        let truth = rational_charge(v, f, h, r, bpy);
        let mut row = vec![label.to_string(), truth.floor.to_string()];
        for c in kernel_candidates() {
            let k = c.charge(v, f, h, r, bpy).expect("kernels never reject");
            let mark = if k == truth.floor {
                " ="
            } else if k == truth.ceil() {
                " ^"
            } else {
                ""
            };
            row.push(format!("{k}{mark}"));
        }
        out.push_str(&format!("| {} |\n", row.join(" | ")));
    }
    out.push('\n');
}

/// Render the full report.
pub fn to_markdown(p: &SweepParams) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Seed {:#x}. {} blocks/year, {} bps, ring size {}. Kernels: see `simulation::demurrage_quantization_sweep`.\n\n",
        p.seed, BLOCKS_PER_YEAR, RATE_BPS, RING_SIZE
    ));
    section_attack(p, &mut out);
    section_honest(p, &mut out);
    section_option3(p, &mut out);
    section_extremes(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u256_oracle_small(v: u64, f: u64, h: u64, r: u32, bpy: u64) -> u128 {
        let p = progressivity(f) as u128;
        (v as u128 * r as u128 * p * h as u128) / (BPS * PROG_SPAN * bpy as u128)
    }

    #[test]
    fn wide_arithmetic_matches_u128_when_it_fits() {
        let mut rng = ChaCha8Rng::seed_from_u64(1);
        for _ in 0..2_000 {
            let a: u128 = rng.random::<u64>() as u128;
            let b: u128 = rng.random::<u64>() as u128;
            let d: u128 = (rng.random::<u64>() as u128).max(1);
            let (hi, lo) = mul_wide(a, b);
            assert_eq!(hi, 0);
            assert_eq!(lo, a * b);
            let ((qh, ql), rem) = divrem_wide(hi, lo, d);
            assert_eq!(qh, 0);
            assert_eq!(ql, a * b / d);
            assert_eq!(rem, a * b % d);
        }
        // 2^128 = (2^64)^2 and (2^128 − 1)^2 identities.
        assert_eq!(mul_wide(1 << 64, 1 << 64), (1, 0));
        assert_eq!(mul_wide(u128::MAX, u128::MAX), (u128::MAX - 1, 1));
        assert_eq!(divrem_wide(1, 0, 2), ((0, 1 << 127), 0));
        assert_eq!(
            divrem_wide(u128::MAX - 1, 1, u128::MAX),
            ((0, u128::MAX), 0)
        );
    }

    /// Since protocol 9.0.0 (#1875) production `demurrage_charge` IS the
    /// study's option 1 (`Candidate::Exact`): equal to [`exact_charge`]
    /// wherever that kernel is exact (`bpy <= EXACT_BPY_LIMIT`), and to the
    /// 256-bit oracle's clamped floor everywhere. This guards the study: if
    /// production drifts from the candidate it adopted, this fails.
    #[test]
    fn quantized_floor_matches_production() {
        let check = |v: u64, f: u64, h: u64, r: u32, bpy: u64| {
            let prod = demurrage_charge(v, f, h, r, bpy);
            assert_eq!(
                prod,
                rational_charge(v, f, h, r, bpy).floor,
                "{v} {f} {h} {r} {bpy}"
            );
            if bpy <= EXACT_BPY_LIMIT {
                assert_eq!(
                    Candidate::Exact.charge(v, f, h, r, bpy),
                    Some(prod),
                    "{v} {f} {h} {r} {bpy}"
                );
            }
        };
        let mut rng = ChaCha8Rng::seed_from_u64(2);
        for _ in 0..5_000 {
            let v = rng.random::<u64>() >> rng.random_range(0..64);
            let f = rng.random_range(0..8_000);
            let h = rng.random::<u64>() >> rng.random_range(0..64);
            let r = rng.random_range(0..20_000u32);
            let bpy = (rng.random::<u64>() >> rng.random_range(0..64)).max(1);
            check(v, f, h, r, bpy);
        }
        for e in extremes() {
            check(e.1, e.2, e.3, e.4, e.5);
        }
    }

    /// Option 1/2c equal the 256-bit oracle's floor/ceiling (clamped) over the
    /// whole domain with `bpy <= EXACT_BPY_LIMIT`, including the extremes.
    #[test]
    fn exact_kernel_matches_oracle() {
        let mut rng = ChaCha8Rng::seed_from_u64(3);
        let check = |v: u64, f: u64, h: u64, r: u32, bpy: u64| {
            let t = rational_charge(v, f, h, r, bpy);
            assert_eq!(
                exact_charge(v, f, h, r, bpy, false),
                t.floor,
                "{v} {f} {h} {r} {bpy}"
            );
            assert_eq!(
                exact_charge(v, f, h, r, bpy, true),
                t.ceil(),
                "{v} {f} {h} {r} {bpy}"
            );
        };
        for _ in 0..5_000 {
            let v = rng.random::<u64>() >> rng.random_range(0..64);
            let f = rng.random_range(0..8_000);
            let h = rng.random::<u64>() >> rng.random_range(0..64);
            let r = rng.random::<u32>() >> rng.random_range(0..32);
            let bpy = (rng.random_range(0..=EXACT_BPY_LIMIT) >> rng.random_range(0..38)).max(1);
            check(v, f, h, r, bpy);
        }
        for (_, v, f, h, r, bpy) in extremes() {
            if bpy <= EXACT_BPY_LIMIT {
                check(v, f, h, r, bpy);
            }
        }
        check(u64::MAX, 6000, u64::MAX, u32::MAX, EXACT_BPY_LIMIT);
        check(u64::MAX, 6000, u64::MAX - 1, u32::MAX, EXACT_BPY_LIMIT - 1);
        // Small domain cross-check against plain u128.
        for h in 1..200u64 {
            let v = bth(1_000_000);
            assert_eq!(
                exact_charge(v, 6000, h, 200, BLOCKS_PER_YEAR, false) as u128,
                u256_oracle_small(v, 6000, h, 200, BLOCKS_PER_YEAR)
            );
        }
    }

    /// Above the limit the exact kernel never under-charges the oracle: it is
    /// either exact or saturated to u64::MAX.
    #[test]
    fn exact_kernel_saturates_upward_beyond_limit() {
        let mut rng = ChaCha8Rng::seed_from_u64(4);
        for _ in 0..2_000 {
            let v = rng.random::<u64>();
            let h = rng.random::<u64>();
            let bpy = rng.random_range(EXACT_BPY_LIMIT..=u64::MAX);
            let t = rational_charge(v, 6000, h, u32::MAX, bpy);
            let k = exact_charge(v, 6000, h, u32::MAX, bpy, false);
            assert!(k == t.floor || k == u64::MAX, "{k} vs {}", t.floor);
        }
    }

    /// No kernel panics (overflow-checks are on in release) on extremes, and
    /// every kernel is a pure function.
    #[test]
    fn kernels_are_total_and_deterministic() {
        for (_, v, f, h, r, bpy) in extremes() {
            for c in Candidate::all() {
                let a = c.charge(v, f, h, r, bpy);
                assert_eq!(a, c.charge(v, f, h, r, bpy));
            }
        }
        for c in Candidate::all() {
            for (v, f, h, r, bpy) in [
                (0, 0, 0, 0, 0),
                (u64::MAX, u64::MAX, u64::MAX, u32::MAX, u64::MAX),
            ] {
                let _ = c.charge(v, f, h, r, bpy);
            }
        }
    }

    /// Defer reproduces the #1875 dead zone; exact and the round-up variants
    /// charge every nonzero hold.
    #[test]
    fn dead_zone_by_candidate() {
        let v = bth(1_000_000);
        for h in 1..=6 {
            let t = rational_charge(v, 6000, h, RATE_BPS, BLOCKS_PER_YEAR);
            assert!(!t.is_zero);
            assert_eq!(
                Candidate::Defer.charge(v, 6000, h, RATE_BPS, BLOCKS_PER_YEAR),
                Some(0)
            );
            assert_eq!(
                Candidate::AtLeastOne.charge(v, 6000, h, RATE_BPS, BLOCKS_PER_YEAR),
                Some(1)
            );
            assert_eq!(
                Candidate::Exact.charge(v, 6000, h, RATE_BPS, BLOCKS_PER_YEAR),
                Some(t.floor)
            );
            assert_eq!(
                Candidate::ExactCeil.charge(v, 6000, h, RATE_BPS, BLOCKS_PER_YEAR),
                Some(t.ceil())
            );
            assert!(
                Candidate::CeilTime
                    .charge(v, 6000, h, RATE_BPS, BLOCKS_PER_YEAR)
                    .unwrap()
                    >= t.ceil()
            );
            assert_eq!(
                Candidate::MinMemberAge(7).charge(v, 6000, h, RATE_BPS, BLOCKS_PER_YEAR),
                None
            );
            assert!(
                Candidate::MinRingMax(7)
                    .charge(v, 6000, h, RATE_BPS, BLOCKS_PER_YEAR)
                    .unwrap()
                    > 0
            );
        }
        // The issue's numbers: 0.02 BTH for 12 blocks today, ~0.038 exact.
        assert_eq!(
            Candidate::Defer.charge(v, 6000, 12, RATE_BPS, BLOCKS_PER_YEAR),
            Some(20_000_000_000)
        );
        assert_eq!(
            Candidate::Exact.charge(v, 6000, 12, RATE_BPS, BLOCKS_PER_YEAR),
            Some(38_051_750_380)
        );
    }

    /// Ordering: today <= exact <= exact-ceil <= ceil-time, for every input.
    #[test]
    fn kernel_ordering() {
        let mut rng = ChaCha8Rng::seed_from_u64(5);
        for _ in 0..5_000 {
            let v = rng.random::<u64>() >> rng.random_range(0..64);
            let f = rng.random_range(0..8_000);
            let h = rng.random_range(0..20_000_000u64);
            let today = quantized_time_charge(v, f, h, RATE_BPS, BLOCKS_PER_YEAR, false);
            let exact = exact_charge(v, f, h, RATE_BPS, BLOCKS_PER_YEAR, false);
            let ceil = exact_charge(v, f, h, RATE_BPS, BLOCKS_PER_YEAR, true);
            let ceil_time = quantized_time_charge(v, f, h, RATE_BPS, BLOCKS_PER_YEAR, true);
            assert!(today <= exact && exact <= ceil, "{v} {f} {h}");
            assert!(ceil <= ceil_time.max(ceil) && ceil_time >= today);
        }
    }

    /// The attack: defer leaks the whole charge at h <= 6 and is profitable
    /// even at the dearest cost for the 1M BTH headline cell; exact and both
    /// ceilings are never profitable.
    #[test]
    fn churn_attack_by_candidate() {
        let cell = headline_cell();
        assert_eq!(cell.factor, 5745);
        let y = churn_year(
            Candidate::Defer,
            cell.principal(),
            cell.factor,
            6,
            CostModel::Cheapest,
        )
        .unwrap();
        assert_eq!(y.paid, 0);
        assert!(y.escaped_frac() > 0.999);
        assert!(best_churn(
            Candidate::Defer,
            cell.principal(),
            cell.factor,
            60,
            CostModel::Dearest
        )
        .is_some());
        for c in [Candidate::Exact, Candidate::ExactCeil, Candidate::CeilTime] {
            for cost in [CostModel::Cheapest, CostModel::Dearest] {
                assert_eq!(
                    best_churn(c, cell.principal(), cell.factor, 60, cost),
                    None,
                    "{c:?}"
                );
            }
        }
        // 2a still leaks almost everything: one pico per hop.
        let y = churn_year(
            Candidate::AtLeastOne,
            cell.principal(),
            cell.factor,
            6,
            CostModel::Cheapest,
        )
        .unwrap();
        assert!(y.escaped_frac() > 0.999);
        // Exact: the churner can save at most 1 pico per hop.
        for h in 1..60 {
            let y = churn_year(
                Candidate::Exact,
                cell.principal(),
                cell.factor,
                h,
                CostModel::Cheapest,
            )
            .unwrap();
            assert!(y.escaped() < y.hops as i128 + 1, "h={h}");
        }
        // 3a rejects holds below M; 3b charges at least M blocks.
        assert!(churn_year(
            Candidate::MinMemberAge(7),
            cell.principal(),
            cell.factor,
            6,
            CostModel::Cheapest
        )
        .is_none());
        // 3a at M=7 still leaks ~47% at h=12 (the truncation beyond the dead zone).
        let y = churn_year(
            Candidate::MinMemberAge(7),
            cell.principal(),
            cell.factor,
            12,
            CostModel::Cheapest,
        )
        .unwrap();
        assert!(y.escaped_frac() > 0.4, "{}", y.escaped_frac());
    }

    /// Split invariance bounds: exact loses < parts pico per split, exact-ceil
    /// overcharges < parts pico, today's kernel loses far more.
    #[test]
    fn split_invariance_bounds() {
        for parts in [2u64, 10] {
            let e = split_stats(Candidate::Exact, parts, 300, SEED);
            assert!(e.time_min >= 0 && e.time_max < parts as i128, "{e:?}");
            assert!(e.value_min >= 0 && e.value_max < parts as i128, "{e:?}");
            let c = split_stats(Candidate::ExactCeil, parts, 300, SEED);
            assert!(c.time_max <= 0 && c.time_min > -(parts as i128), "{c:?}");
            assert!(c.value_max <= 0 && c.value_min > -(parts as i128), "{c:?}");
            let d = split_stats(Candidate::Defer, parts, 300, SEED);
            assert!(d.time_max > 1_000_000, "{d:?}");
        }
    }

    /// Honest rings always have an elapsed statistic >= 10 (wallet decoy
    /// floor), so no honest spend is in the dead zone, and 3a/3b at M <= 10
    /// never change an honest spend's charge.
    #[test]
    fn honest_population_never_in_dead_zone() {
        let p = SweepParams::smoke();
        let mut pop = gamma_population(p.gamma_spends, p.seed);
        pop.extend(short_hold_population(p.short_per_age, p.seed));
        assert!(!pop.is_empty());
        assert!(pop.iter().all(|s| s.elapsed >= MIN_DECOY_AGE_BLOCKS));
        for c in [
            Candidate::MinMemberAge(7),
            Candidate::MinMemberAge(10),
            Candidate::MinRingMax(7),
            Candidate::MinRingMax(10),
        ] {
            let s = honest_stats(c, &pop);
            assert_eq!(s.changed_share, 0.0, "{c:?}");
            assert_eq!(s.rejected_share, 0.0, "{c:?}");
        }
        let exact = honest_stats(Candidate::Exact, &pop);
        assert!(exact.extra_vs_today >= 0.0);
        assert!(exact.err_min > -1.0 && exact.err_max <= 0.0);
        let today = honest_stats(Candidate::Defer, &pop);
        assert!(today.bias_vs_true < 0.0, "today under-charges");
    }

    /// Below the wallet floor the node picker's fallback makes the real input
    /// the strictly youngest member of every ring.
    #[test]
    fn young_inputs_are_youngest_today() {
        for a in [1u64, 6, 9] {
            let r = young_ring_row(a, 1, MIN_DECOY_AGE_BLOCKS, 20, SEED);
            assert_eq!(r.rings, 20);
            assert!((r.real_youngest - 1.0).abs() < 1e-12, "age {a}");
            assert!((r.best_rank_rate - 1.0).abs() < 1e-12, "age {a}");
            assert!(r.median_elapsed >= MIN_DECOY_AGE_BLOCKS);
        }
    }

    #[test]
    fn report_renders_and_is_deterministic() {
        let p = SweepParams::smoke();
        let a = to_markdown(&p);
        assert_eq!(a, to_markdown(&p));
        for needle in [
            "(a) Churn attack",
            "(b) Honest users",
            "(c) Option 3",
            "(d) Determinism",
        ] {
            assert!(a.contains(needle), "{needle}");
        }
    }
}
