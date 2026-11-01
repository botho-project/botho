//! Settlement-horizon calibration sweep — the empirical gate for issue #833.
//!
//! # What this sweep calibrates
//!
//! ADR 0003 makes **factor-1 (background) coins the only wrappable class**, and
//! adds a **demurrage-settlement operation** (#831) so a holder of a wealthy
//! coin can *pay* to reclassify it down to factor-1 and become wrap-eligible.
//! The settlement price is a **capitalized future demurrage** over a fixed
//! horizon:
//!
//! ```text
//! settlement_charge = demurrage_charge(value, factor, SETTLEMENT_HORIZON_BLOCKS,
//!                                      rate_bps, blocks_per_year)
//! ```
//!
//! `SETTLEMENT_HORIZON_BLOCKS` is a **monetary-policy magnitude**: how much a
//! wealthy coin pays to permanently exit demurrage by wrapping. The two failure
//! modes (from the #833 body) are symmetric:
//!
//! - **Too low** → wrapping is a cheap escape; wealthy holders wrap to dodge
//!   redistribution, eroding the cluster-tilted Gini gain the mechanism exists
//!   to produce.
//! - **Too high** → wrapping wealthy value is punitively expensive; the bridge
//!   on-ramp is unusable for exactly the holders who most want DeFi liquidity.
//!
//! # The key structural fact (why the horizon *is* the break-even period)
//!
//! [`crate::demurrage_charge`] is **linear in `elapsed`** (for fixed value,
//! factor and rate). Therefore:
//!
//! ```text
//! settlement_charge(H) = H years-worth of ordinary demurrage, paid up front.
//! ```
//!
//! So `SETTLEMENT_HORIZON_BLOCKS` measured in years is exactly the **holding
//! period at which settling-and-wrapping breaks even against holding-and-paying
//! ordinary demurrage**. This gives the #831 "no strictly-dominant escape"
//! soundness gate an analytic anchor: settlement is never cheaper than holding
//! for the horizon iff the horizon ≥ the holder's expected remaining holding
//! period. The sweep confirms this numerically (integer-arithmetic rounding can
//! only ever make settlement cost `>=` the linear expectation, never less) and
//! quantifies the two economic effects the analytic fact does not capture:
//! wrap-cost as a fraction of settled value, and the Gini erosion when the top
//! decile actually exits at each price.
//!
//! # Shared horizon with #925 (spend-to-background)
//!
//! Issue #834 (verdict `docs/research/demurrage-background-reset-leak.md`, PR
//! #931) confirmed a symmetric on-chain escape: a wealthy holder can deflate a
//! coin to background on an *ordinary spend* and pay **zero** future demurrage.
//! Its fix #925 prices that transition with **the same capitalized-future-
//! demurrage formula over a horizon of the same magnitude class**. #831
//! (wrap-to-factor-1) and #925 (spend-to-background) are the two doors out of
//! the demurrage regime, priced identically. This module runs the horizon sweep
//! for the wrapping escape and — because the charge formula is identical — the
//! resulting curve applies verbatim to #925. See
//! `docs/research/settlement-horizon-calibration.md` for the shared-vs-separate
//! recommendation.
//!
//! # Method (a focused, deterministic Monte-Carlo)
//!
//! Like [`super::decoy_quantile_sweep`], this is a focused Monte-Carlo rather
//! than the full agent framework: the lever here is a one-shot "settle+wrap vs.
//! hold" decision per wealthy holder, not multi-agent transaction flow. It
//! reuses the shipped consensus kernel [`crate::demurrage_charge`], the real
//! [`crate::ClusterFactorCurve`] to derive factor classes from cluster wealth,
//! and the shared [`super::metrics::calculate_gini`] — no second Gini or charge
//! implementation. All randomness is a deterministically seeded `ChaCha8Rng`,
//! so the doc numbers regenerate byte-for-byte.
//!
//! # Spend-time charging (#1698)
//!
//! The HOLD world (and the holders who stay behind in the ESCAPE world) is
//! charged the way the node charges ([`ChargeModel::SpendTime`]): demurrage is
//! collected only when a coin is spent, on its accrued age, by the shared
//! [`crate::consensus_fee_model`] (#1695). Epoch 0 is rate-0, the fee is split
//! 80% pool / 20% burned, and the ring-centroid floor is computed over a
//! 20-member ring, with an optional min-centroid dilution adversary
//! ([`DilutionAdversary`]). Spend velocities are per wealth decile. The
//! pre-#1698 yearly, 100%-to-pool model is kept as
//! [`ChargeModel::LegacyYearly`] for before/after comparison. Results: §8 of
//! `docs/research/settlement-horizon-calibration.md`.

use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;

#[cfg(test)]
use crate::SETTLEMENT_HORIZON_BLOCKS;
use crate::{
    capitalized_reset_charge,
    consensus_fee::{
        split_fee_pool_burn, CONSENSUS_RING_SIZE, WALLET_FACTOR_SIMILARITY_SPREAD_BPS,
    },
    consensus_fee_breakdown, consensus_fee_model, consensus_tx_size_bytes, demurrage_charge,
    fee_curve::{FeeConfig, PICO_PER_BTH},
    ring_centroid_implied_factor, ClaimedClass, ClusterFactorCurve, ConsensusFeeInputs,
    ConsensusFeeParams, FeeModelMode, FACTOR_SCALE,
};

use super::metrics::calculate_gini;

/// Blocks per year at the 5s reference block time (matches the node and the
/// decoy sweep default: `31_536_000 s / 5 s`).
pub const BLOCKS_PER_YEAR: u64 = 6_307_200;

/// Annual demurrage rate at maximum factor, basis points (2%/yr, #351).
pub const RATE_BPS: u32 = 200;

/// A candidate `SETTLEMENT_HORIZON_BLOCKS` value, labelled by its wall-clock
/// magnitude at the 5s reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Horizon {
    /// The candidate `SETTLEMENT_HORIZON_BLOCKS`.
    pub blocks: u64,
    /// Human label (e.g. "1yr").
    pub label: &'static str,
}

impl Horizon {
    /// Horizon expressed in fractional years (for reporting only).
    pub fn years(&self) -> f64 {
        self.blocks as f64 / BLOCKS_PER_YEAR as f64
    }
}

/// The candidate horizons swept, in report order: 1 month → 5 years of blocks
/// at the 5s reference. These bracket every plausible monetary-policy choice.
pub fn candidate_horizons() -> Vec<Horizon> {
    vec![
        Horizon {
            blocks: BLOCKS_PER_YEAR / 12,
            label: "1mo",
        },
        Horizon {
            blocks: BLOCKS_PER_YEAR / 2,
            label: "6mo",
        },
        Horizon {
            blocks: BLOCKS_PER_YEAR,
            label: "1yr",
        },
        Horizon {
            blocks: 2 * BLOCKS_PER_YEAR,
            label: "2yr",
        },
        Horizon {
            blocks: 5 * BLOCKS_PER_YEAR,
            label: "5yr",
        },
    ]
}

/// A wealth-factor class in the population: a cluster-wealth magnitude and the
/// factor the real curve assigns it.
#[derive(Clone, Copy, Debug)]
pub struct FactorClass {
    /// Human label of the cluster-wealth magnitude (e.g. "1M BTH cluster").
    pub label: &'static str,
    /// Cluster wealth in BTH used to derive the factor from the real curve.
    pub cluster_wealth_bth: u64,
    /// Per-holder coin value in BTH that would be settled/wrapped.
    pub coin_value_bth: u64,
    /// Factor in FACTOR_SCALE units (1000..=6000), from `ClusterFactorCurve`.
    pub factor_scaled: u64,
}

/// Build the wealth-factor classes spanning the top decile, deriving each
/// factor from the **real** [`ClusterFactorCurve`] at that cluster wealth.
///
/// The curve is a log-domain sigmoid centred at 100k BTH (`W_MID`), saturating
/// at 6x. The magnitudes below bracket the on-ramp population: a mid-tier
/// wealthy cluster (~2x), a large one near the midpoint knee (~3x), and
/// whale-scale clusters at/above saturation (5-6x).
pub fn factor_classes() -> Vec<FactorClass> {
    let curve = ClusterFactorCurve::default_params();
    let specs: &[(&str, u64, u64)] = &[
        // (label, cluster_wealth_bth, coin_value_bth)
        ("10k-BTH cluster", 10_000, 1_000),
        ("50k-BTH cluster", 50_000, 5_000),
        ("100k-BTH cluster", 100_000, 10_000),
        ("500k-BTH cluster", 500_000, 50_000),
        ("2M-BTH cluster", 2_000_000, 100_000),
        ("10M-BTH cluster", 10_000_000, 100_000),
    ];
    specs
        .iter()
        .map(|&(label, cluster_wealth_bth, coin_value_bth)| {
            let cluster_wealth_pico = cluster_wealth_bth as u128 * PICO_PER_BTH;
            FactorClass {
                label,
                cluster_wealth_bth,
                coin_value_bth,
                factor_scaled: curve.factor(cluster_wealth_pico),
            }
        })
        .collect()
}

/// One row of the horizon × factor-class cost table.
#[derive(Clone, Debug)]
pub struct CostRow {
    pub horizon: Horizon,
    pub class: FactorClass,
    /// Settlement charge in picocredits.
    pub settlement_charge_pico: u128,
    /// Wrap cost as a fraction of the settled coin value (charge / value).
    pub wrap_cost_frac: f64,
    /// Churn-invariance margin: settlement_charge / (H-years of ordinary
    /// demurrage on the same coin). By linearity this is `>= 1.0`; a value
    /// below 1.0 would mean settling is *cheaper* than holding-and-paying over
    /// the horizon — a strictly-dominant escape (the #831 soundness failure).
    pub churn_invariance_margin: f64,
}

/// Compute the full cost table (every horizon × every factor class).
pub fn cost_table(horizons: &[Horizon], classes: &[FactorClass]) -> Vec<CostRow> {
    let mut rows = Vec::with_capacity(horizons.len() * classes.len());
    for &horizon in horizons {
        for &class in classes {
            let value_pico = class.coin_value_bth as u128 * PICO_PER_BTH;
            // The charge kernel takes u64 value; coin values here (<= 100k BTH
            // = 1e17 pico) fit u64 comfortably.
            let value_u64 = u64::try_from(value_pico).expect("coin value fits u64 picocredits");

            let settlement_charge = demurrage_charge(
                value_u64,
                class.factor_scaled,
                horizon.blocks,
                RATE_BPS,
                BLOCKS_PER_YEAR,
            );
            // "Hold and pay ordinary demurrage over the SAME horizon" — the
            // same kernel with the same elapsed. Equal by construction; we
            // compute it independently so the invariance gate is measured, not
            // assumed.
            let hold_cost = demurrage_charge(
                value_u64,
                class.factor_scaled,
                horizon.blocks,
                RATE_BPS,
                BLOCKS_PER_YEAR,
            );

            let wrap_cost_frac = settlement_charge as f64 / value_u64 as f64;
            let churn_invariance_margin = if hold_cost > 0 {
                settlement_charge as f64 / hold_cost as f64
            } else {
                // Factor-1 (background) coins owe zero either way -> no escape
                // to price; treat as break-even.
                1.0
            };

            rows.push(CostRow {
                horizon,
                class,
                settlement_charge_pico: settlement_charge as u128,
                wrap_cost_frac,
                churn_invariance_margin,
            });
        }
    }
    rows
}

/// How the HOLD (and ESCAPE-remainder) world charges demurrage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChargeModel {
    /// #1698, node-faithful: demurrage is collected **only when a coin is
    /// spent**, on the coin's accrued age, by the shared
    /// [`crate::consensus_fee_model`] (the pure half of
    /// `Ledger::consensus_fee_floor`). Epoch-0 rate is 0, the fee is split
    /// 80% to the lottery pool / 20% burned exactly as
    /// `LotteryFeeConfig::split_fees`, and the ring-centroid floor is computed
    /// with [`crate::ring_centroid_implied_factor`] over a
    /// [`CONSENSUS_RING_SIZE`] ring. A coin that is never spent pays nothing.
    SpendTime,
    /// The pre-#1698 model, kept for before/after comparison: every wealthy
    /// holder pays one year of demurrage every simulated year whether or not
    /// it spends, 100% of it to the pool, no base fee, no burn, no epoch-0
    /// gate, no ring floor. The node has never charged this way.
    LegacyYearly,
}

impl ChargeModel {
    /// Short label for reports.
    pub fn label(self) -> &'static str {
        match self {
            ChargeModel::SpendTime => "spend-time (node)",
            ChargeModel::LegacyYearly => "legacy yearly",
        }
    }
}

/// The decoy-dilution adversary (#1698): how a spender builds its ring and
/// declares its output class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DilutionAdversary {
    /// Node default ring behaviour: the wallet's
    /// `select_decoys_cluster_aware` draws decoys uniformly from resident coins
    /// whose factor lies within ±[`WALLET_FACTOR_SIMILARITY_SPREAD_BPS`] of the
    /// real input's factor (`botho/src/decoy_selection.rs`), topping up
    /// uniformly from the rest only if the band holds fewer than
    /// `CONSENSUS_RING_SIZE − 1` coins. The spender declares its true class;
    /// the node's demurrage factor is `max(claimed, centroid)`. The band keeps
    /// whale decoys out of background rings (the §4.5 over-charge channel of
    /// `demurrage-downgrade-overcharge.md`), so honest rings neither dilute a
    /// whale nor inflate a background spender.
    Honest,
    /// Worst-case dilution: the spender picks the `CONSENSUS_RING_SIZE − 1`
    /// resident coins that minimise the value-weighted centroid (lowest cluster
    /// wealth, then highest value) and declares exactly the centroid class.
    /// `claimed == floor`, so the #925 capitalized term is 0 and the coin pays
    /// accrued demurrage at the diluted factor. The declared class becomes the
    /// new coin's tag, so the dilution ratchets down on every spend — the node
    /// cannot see which ring member is real.
    MinCentroid,
}

impl DilutionAdversary {
    /// Short label for reports.
    pub fn label(self) -> &'static str {
        match self {
            DilutionAdversary::Honest => "honest ring (node default)",
            DilutionAdversary::MinCentroid => "min-centroid decoys",
        }
    }
}

/// Default per-decile spend velocities, in milli-spends per year (1000 = one
/// spend a year), indexed by wealth decile (0 = poorest, 9 = richest).
///
/// Borrowed from the M2 population ladder (`simulation::m2::m2_population`):
/// ordinary holders spend about once a year (`velocity_per_year: 1.0`) and the
/// whale cohort once every five years (`velocity_per_year: 0.2`). The velocity
/// sensitivity table in the report sweeps the top decile from 0 (never spends)
/// to monthly.
pub const DEFAULT_DECILE_VELOCITY_MILLI: [u32; 10] = [
    1_000, 1_000, 1_000, 1_000, 1_000, 1_000, 1_000, 1_000, 1_000, 200,
];

/// Parameters for the dGini-erosion experiment.
#[derive(Clone, Debug)]
pub struct GiniSweepParams {
    /// Number of factor-1 background holders (poor).
    pub poor: usize,
    /// Per-holder wealth of a background holder, in BTH.
    pub poor_wealth_bth: u64,
    /// Number of wealthy holders (the "top decile" that may exit). Sized so
    /// wealthy holders are ~10% of the population.
    pub wealthy: usize,
    /// Per-holder wealth of a wealthy holder, in BTH (also the coin they would
    /// settle+wrap).
    pub wealthy_wealth_bth: u64,
    /// The factor class the wealthy holders belong to (fixes their factor and
    /// cluster wealth for the erosion run).
    pub wealthy_class: FactorClass,
    /// Number of simulated years.
    pub years: u64,
    /// RNG seed: spend phases and honest decoy draws.
    pub seed: u64,
    /// Charge model (#1698). Default [`ChargeModel::SpendTime`].
    pub charge_model: ChargeModel,
    /// Spend velocity per wealth decile, milli-spends per year
    /// ([`DEFAULT_DECILE_VELOCITY_MILLI`]). Spend-time model only.
    pub decile_velocity_milli: [u32; 10],
    /// Decoy-dilution adversary. Default [`DilutionAdversary::Honest`].
    pub dilution: DilutionAdversary,
    /// Chain height of sim block 0. Default: the first epoch-1 block
    /// (`ConsensusFeeParams::first_demurrage_height`), so the sweep measures
    /// the steady-state charge; `0` starts at launch, where epoch 0 charges no
    /// demurrage for the first year.
    pub start_height: u64,
    /// Redistribution ticks per year (the pool is paid out per capita at the
    /// end of each tick). Spend-time model only.
    pub ticks_per_year: u64,
}

impl GiniSweepParams {
    /// Default population: 90 background holders + 10 wealthy holders (a clean
    /// top decile), wealthy holders in the given class, run 10 years, charged
    /// at spend time at [`DEFAULT_DECILE_VELOCITY_MILLI`] with honest rings,
    /// starting in epoch 1.
    pub fn default_for(class: FactorClass) -> Self {
        Self {
            poor: 90,
            poor_wealth_bth: 1_000,
            wealthy: 10,
            wealthy_wealth_bth: class.coin_value_bth,
            wealthy_class: class,
            years: 10,
            seed: 0xB07A_0833,
            charge_model: ChargeModel::SpendTime,
            decile_velocity_milli: DEFAULT_DECILE_VELOCITY_MILLI,
            dilution: DilutionAdversary::Honest,
            start_height: fee_params().first_demurrage_height(),
            ticks_per_year: 12,
        }
    }

    /// The same population under the pre-#1698 yearly model.
    pub fn legacy_for(class: FactorClass) -> Self {
        Self {
            charge_model: ChargeModel::LegacyYearly,
            ..Self::default_for(class)
        }
    }
}

/// The consensus fee parameters the spend-time model uses: mainnet,
/// consensus floor only (the block-validity floor; demurrage is identical
/// under either mode, only tiny base fees differ).
pub fn fee_params() -> ConsensusFeeParams {
    ConsensusFeeParams::mainnet(FeeModelMode::ConsensusOnly)
}

/// Outcome of the erosion experiment at one horizon.
#[derive(Clone, Debug)]
pub struct GiniErosionRow {
    pub horizon: Horizon,
    /// Fraction of the top decile that settled+wrapped, in basis points
    /// (10000 = the whole decile exits — the worst case).
    pub adoption_bps: u32,
    /// Final Gini when the top decile HOLDS (paying demurrage per the charge
    /// model) for the whole run.
    pub gini_hold: f64,
    /// Final Gini when the top decile SETTLES+WRAPS at round 0: it pays the
    /// one-shot settlement charge, then its wrapped value LEAVES the
    /// demurrage/redistribution system (wBTH pays no further demurrage,
    /// ADR 0003 §4) for the rest of the run.
    pub gini_escape: f64,
    /// Erosion = gini_escape − gini_hold. Positive = escaping at this price
    /// leaves the population MORE unequal than holding would have.
    pub gini_erosion: f64,
    /// Δgini vs the no-demurrage baseline in the HOLD world.
    pub delta_gini_hold: f64,
    /// Δgini vs the no-demurrage baseline in the ESCAPE world — the residual
    /// gain once the top decile has wrapped out. Must stay > 0.05.
    pub delta_gini_escape: f64,
    /// Lottery-pool revenue over the run in the ESCAPE world, in BTH.
    pub pool_revenue_escape_bth: f64,
    /// Lottery-pool revenue over the run in the HOLD world, in BTH.
    pub pool_revenue_hold_bth: f64,
    /// Fees burned over the run in the ESCAPE world, in BTH (0 under legacy).
    pub burned_escape_bth: f64,
    /// Fees burned over the run in the HOLD world, in BTH (0 under legacy).
    pub burned_hold_bth: f64,
    /// HOLD world: demurrage accrued on resident coins at run end but never
    /// collected because those coins were not spent again, in BTH (0 under
    /// legacy, which charges as it accrues).
    pub uncollected_hold_bth: f64,
}

impl GiniErosionRow {
    /// The erosion gate: escaping at this horizon must not push the residual
    /// redistribution gain below the 0.05 design floor.
    pub fn passes_floor(&self) -> bool {
        self.delta_gini_escape > 0.05
    }
}

/// Which world a run models.
#[derive(Clone, Copy, PartialEq, Eq)]
enum World {
    /// Demurrage disabled entirely (Δgini anchor).
    NoDemurrage,
    /// Top decile holds the whole run.
    Hold,
    /// A fraction of the top decile settles+wraps at round 0, then exits; the
    /// rest holds. The fraction is in basis points (10000 = whole decile).
    Escape { adoption_bps: u32 },
}

/// Everything one world run produces.
#[derive(Clone, Copy, Debug, Default)]
struct WorldOutcome {
    gini: f64,
    pool_pico: u128,
    burned_pico: u128,
    uncollected_pico: u128,
}

fn run_world(params: &GiniSweepParams, horizon: Horizon, world: World) -> WorldOutcome {
    match params.charge_model {
        ChargeModel::LegacyYearly => {
            let (gini, pool_pico) = run_world_legacy_yearly(params, horizon, world);
            WorldOutcome {
                gini,
                pool_pico,
                ..WorldOutcome::default()
            }
        }
        ChargeModel::SpendTime => run_world_spend_time(params, horizon, world),
    }
}

/// The pre-#1698 world (kept verbatim for before/after comparison): returns
/// (final_gini, pool_revenue_pico).
fn run_world_legacy_yearly(
    params: &GiniSweepParams,
    horizon: Horizon,
    world: World,
) -> (f64, u128) {
    let poor_wealth = params.poor_wealth_bth as u128 * PICO_PER_BTH;
    let wealthy_wealth = params.wealthy_wealth_bth as u128 * PICO_PER_BTH;

    // Wealth tracked in picocredits (u128) to avoid narrowing at scale.
    let mut wealths: Vec<u128> = Vec::with_capacity(params.poor + params.wealthy);
    for _ in 0..params.poor {
        wealths.push(poor_wealth);
    }
    for _ in 0..params.wealthy {
        wealths.push(wealthy_wealth);
    }
    let n = wealths.len() as u128;
    let poor = params.poor;

    // `wrapped_out[i]` = value that holder i moved into wBTH and thus removed
    // from the demurrage/redistribution population.
    let mut wrapped_out: Vec<u128> = vec![0; wealths.len()];

    let mut pool_revenue: u128 = 0;
    let mut pool_carry: u128 = 0;

    if let World::Escape { adoption_bps } = world {
        let total_wealthy = wealths.len() - poor;
        let n_exit = (total_wealthy as u64 * adoption_bps as u64 / 10_000) as usize;
        let exit_start = wealths.len() - n_exit;
        for i in exit_start..wealths.len() {
            let value_u64 =
                u64::try_from(wealths[i]).expect("wealthy coin value fits u64 picocredits");
            let charge = demurrage_charge(
                value_u64,
                params.wealthy_class.factor_scaled,
                horizon.blocks,
                RATE_BPS,
                BLOCKS_PER_YEAR,
            ) as u128;
            let charge = charge.min(wealths[i]);
            wealths[i] -= charge;
            pool_revenue += charge; // one-shot settlement fee -> lottery pool
            pool_carry += charge;
            wrapped_out[i] = wealths[i];
            wealths[i] = 0;
        }
    }

    for _year in 0..params.years {
        let mut pool: u128 = pool_carry;

        if world != World::NoDemurrage {
            for w in wealths[poor..].iter_mut() {
                if *w == 0 {
                    continue;
                }
                let value_u64 = u64::try_from(*w).expect("wealthy holding fits u64 picocredits");
                let charge = demurrage_charge(
                    value_u64,
                    params.wealthy_class.factor_scaled,
                    BLOCKS_PER_YEAR, // one year of holding
                    RATE_BPS,
                    BLOCKS_PER_YEAR,
                ) as u128;
                let charge = charge.min(*w);
                *w -= charge;
                pool += charge;
                pool_revenue += charge;
            }
        }

        // Per-capita redistribution across the whole population (as shipped
        // before #1698, wrapped-out holders included).
        if n > 0 && pool > 0 {
            let share = pool / n;
            pool_carry = pool % n;
            if share > 0 {
                for w in wealths.iter_mut() {
                    *w += share;
                }
            }
        } else {
            pool_carry = pool;
        }
    }

    (final_gini(&wealths, &wrapped_out), pool_revenue)
}

/// Gini over on-chain wealth plus wrapped wBTH (value preserved 1:1, ADR 0003
/// factor-1 peg): the escape does not vaporize value, it exempts it.
fn final_gini(wealths: &[u128], wrapped_out: &[u128]) -> f64 {
    let final_wealths: Vec<u64> = wealths
        .iter()
        .zip(wrapped_out.iter())
        .map(|(&w, &wr)| u64::try_from((w + wr) / PICO_PER_BTH).unwrap_or(u64::MAX))
        .collect();
    calculate_gini(&final_wealths)
}

/// One holder in the spend-time world: a single coin.
#[derive(Clone, Debug)]
struct Holder {
    /// On-chain coin value, picocredits.
    value: u64,
    /// Value-normalized effective cluster wealth of the coin's tag
    /// (`Σ weight × W_global`), picocredits — what the ledger resolves for a
    /// ring member.
    eff_wealth: u128,
    /// Sim block at which the coin was last created (its demurrage clock).
    received_at: u64,
    /// Next scheduled spend (sim block), `None` = never spends / exited.
    next_spend: Option<u64>,
    /// Blocks between spends.
    interval: u64,
    /// Value wrapped out to wBTH, picocredits.
    wrapped: u64,
}

/// Value-weighted ring centroid wealth, with the same saturating arithmetic
/// as [`ring_centroid_implied_factor`] (which maps it through the curve).
fn centroid_wealth(members: &[(u64, u128)]) -> u128 {
    let mut weighted: u128 = 0;
    let mut total: u128 = 0;
    for &(v, w) in members {
        weighted = weighted.saturating_add((v as u128).saturating_mul(w));
        total = total.saturating_add(v as u128);
    }
    weighted.checked_div(total).unwrap_or(0)
}

/// Build the spender's ring: its own coin plus `CONSENSUS_RING_SIZE − 1`
/// decoys from the other resident coins, per the adversary policy.
fn build_ring(
    holders: &[Holder],
    spender: usize,
    policy: DilutionAdversary,
    rng: &mut ChaCha8Rng,
) -> Vec<(u64, u128)> {
    let mut candidates: Vec<usize> = (0..holders.len())
        .filter(|&j| j != spender && holders[j].value > 0)
        .collect();
    let k = (CONSENSUS_RING_SIZE - 1).min(candidates.len());
    match policy {
        DilutionAdversary::Honest => {
            // Wallet factor band first (`factor_similarity_band`), then the
            // rest; a uniform draw within each group (partial Fisher-Yates).
            let curve = FeeConfig::default().cluster_curve;
            let f = curve
                .factor(holders[spender].eff_wealth)
                .clamp(FACTOR_SCALE, 6 * FACTOR_SCALE);
            let delta = f * WALLET_FACTOR_SIMILARITY_SPREAD_BPS / 10_000;
            let (lo, hi) = (
                f.saturating_sub(delta).max(FACTOR_SCALE),
                (f + delta).min(6 * FACTOR_SCALE),
            );
            let (mut band, mut rest): (Vec<usize>, Vec<usize>) =
                candidates.iter().partition(|&&j| {
                    let fj = curve.factor(holders[j].eff_wealth);
                    fj >= lo && fj <= hi
                });
            for group in [&mut band, &mut rest] {
                for i in 0..group.len() {
                    let j = rng.random_range(i..group.len());
                    group.swap(i, j);
                }
            }
            band.extend(rest);
            candidates = band;
        }
        DilutionAdversary::MinCentroid => {
            candidates.sort_by(|&a, &b| {
                holders[a]
                    .eff_wealth
                    .cmp(&holders[b].eff_wealth)
                    .then(holders[b].value.cmp(&holders[a].value))
                    .then(a.cmp(&b))
            });
        }
    }
    let mut ring = Vec::with_capacity(k + 1);
    ring.push((holders[spender].value, holders[spender].eff_wealth));
    ring.extend(
        candidates[..k]
            .iter()
            .map(|&j| (holders[j].value, holders[j].eff_wealth)),
    );
    ring
}

/// The spender's `(claimed_factor, floor_factor, new_eff_wealth)` for a ring,
/// exactly as the node composes them: `floor = max(claimed, centroid)`.
fn ring_classes(
    holder: &Holder,
    ring: &[(u64, u128)],
    policy: DilutionAdversary,
    curve: &ClusterFactorCurve,
) -> (u64, u64, u128) {
    let implied = ring_centroid_implied_factor(ring, curve);
    match policy {
        DilutionAdversary::Honest => {
            let claimed = curve.factor(holder.eff_wealth);
            (claimed, claimed.max(implied), holder.eff_wealth)
        }
        DilutionAdversary::MinCentroid => {
            let c = centroid_wealth(ring);
            let claimed = curve.factor(c);
            debug_assert_eq!(claimed, implied);
            (claimed, claimed.max(implied), c)
        }
    }
}

/// Pay a fee out of a coin: `(paid, pool, burn)` with the node's 80/20 split.
fn pay_fee(value: &mut u64, fee: u64) -> (u64, u64, u64) {
    let paid = fee.min(*value);
    *value -= paid;
    let (pool, burn) = split_fee_pool_burn(paid);
    (paid, pool, burn)
}

/// The #1698 node-faithful world: demurrage only at spend time, via the
/// shared consensus fee model.
fn run_world_spend_time(params: &GiniSweepParams, horizon: Horizon, world: World) -> WorldOutcome {
    let fp = fee_params();
    let curve = FeeConfig::default().cluster_curve;
    let bpy = fp.blocks_per_year;
    let n = params.poor + params.wealthy;

    let poor_value = params.poor_wealth_bth * PICO_PER_BTH as u64;
    let wealthy_value = params.wealthy_wealth_bth * PICO_PER_BTH as u64;
    let wealthy_cluster = params.wealthy_class.cluster_wealth_bth as u128 * PICO_PER_BTH;

    // Spend schedules are drawn identically in every world (same seed, same
    // order) so HOLD and ESCAPE differ only in the escape itself.
    let mut phase_rng = ChaCha8Rng::seed_from_u64(params.seed);
    let mut holders: Vec<Holder> = (0..n)
        .map(|i| {
            let wealthy = i >= params.poor;
            let decile = (i * 10 / n).min(9);
            let v = params.decile_velocity_milli[decile];
            let (interval, next_spend) = if v == 0 {
                (0, None)
            } else {
                let interval = ((bpy as u128 * 1_000) / v as u128).max(1) as u64;
                (interval, Some(phase_rng.random_range(0..interval)))
            };
            Holder {
                value: if wealthy { wealthy_value } else { poor_value },
                // Background holders carry no cluster mass (factor 1×).
                eff_wealth: if wealthy { wealthy_cluster } else { 0 },
                received_at: 0,
                next_spend,
                interval,
                wrapped: 0,
            }
        })
        .collect();

    let initial: Vec<u128> = holders.iter().map(|h| h.value as u128).collect();
    if world == World::NoDemurrage {
        return WorldOutcome {
            gini: final_gini(&initial, &vec![0; n]),
            ..WorldOutcome::default()
        };
    }

    let mut decoy_rng = ChaCha8Rng::seed_from_u64(params.seed ^ 0xDEC0_1698);
    let mut out = WorldOutcome::default();
    let mut pool: u128 = 0;

    // ESCAPE: the highest-indexed fraction of the top decile settles to
    // background at block 0 (the #831 settlement priced at the swept horizon
    // from the ring-floored class) and wraps out.
    if let World::Escape { adoption_bps } = world {
        let n_exit = (params.wealthy as u64 * adoption_bps as u64 / 10_000) as usize;
        let height = params.start_height;
        for i in (n - n_exit)..n {
            let ring = build_ring(&holders, i, params.dilution, &mut decoy_rng);
            let (_, floor, _) = ring_classes(&holders[i], &ring, params.dilution, &curve);
            let fee = settlement_fee(holders[i].value, floor, horizon.blocks, height);
            let (_, p, b) = pay_fee(&mut holders[i].value, fee);
            pool += p as u128;
            out.pool_pico += p as u128;
            out.burned_pico += b as u128;
            holders[i].wrapped = holders[i].value;
            holders[i].value = 0;
            holders[i].next_spend = None;
        }
    }

    let ticks = params.years * params.ticks_per_year;
    for tick in 1..=ticks {
        let tick_end = tick * bpy / params.ticks_per_year;
        for i in 0..n {
            while let Some(s) = holders[i].next_spend {
                if s > tick_end || holders[i].value == 0 {
                    break;
                }
                let ring = build_ring(&holders, i, params.dilution, &mut decoy_rng);
                let (claimed, floor, new_wealth) =
                    ring_classes(&holders[i], &ring, params.dilution, &curve);
                let inputs = ConsensusFeeInputs {
                    size: consensus_tx_size_bytes(1, 2, 0),
                    n_out: 2,
                    n_memo: 0,
                    claimed: ClaimedClass::Factor(claimed),
                    floor,
                    elapsed: s - holders[i].received_at,
                    height: params.start_height + s,
                    // Charged on the pre-fee value: over-pays the exact
                    // fixed-point floor by O(rate²), never under-pays.
                    output_sum: holders[i].value,
                };
                let fee = consensus_fee_model(&inputs, &fp);
                let (_, p, b) = pay_fee(&mut holders[i].value, fee);
                pool += p as u128;
                out.pool_pico += p as u128;
                out.burned_pico += b as u128;
                // The spend recreates the coin (self-transfer with change):
                // fresh demurrage clock, declared class becomes its tag.
                holders[i].received_at = s;
                holders[i].eff_wealth = new_wealth;
                holders[i].next_spend = Some(s + holders[i].interval);
            }
        }

        // Per-capita lottery stand-in across RESIDENT holders (a wrapped-out
        // holder owns no BTH UTXO and cannot win).
        let residents = holders.iter().filter(|h| h.value > 0).count() as u128;
        if residents > 0 && pool > 0 {
            let share = pool / residents;
            pool %= residents;
            if share > 0 {
                for h in holders.iter_mut().filter(|h| h.value > 0) {
                    h.value = h.value.saturating_add(share as u64);
                }
            }
        }
    }

    // Demurrage still owed on unspent coins at run end: accrued, never paid.
    let end = params.years * bpy;
    let rate = fp.demurrage_rate_bps(params.start_height + end);
    for h in holders.iter().filter(|h| h.value > 0) {
        out.uncollected_pico += demurrage_charge(
            h.value,
            curve.factor(h.eff_wealth),
            end - h.received_at,
            rate,
            bpy,
        ) as u128;
    }

    let values: Vec<u128> = holders.iter().map(|h| h.value as u128).collect();
    let wrapped: Vec<u128> = holders.iter().map(|h| h.wrapped as u128).collect();
    out.gini = final_gini(&values, &wrapped);
    out
}

/// The #831 settlement fee at a swept horizon: the consensus base fee for a
/// 1-in/2-out background-claimed tx plus the capitalized charge from the
/// ring floor down to factor-1 over `horizon_blocks`. At
/// `horizon_blocks == SETTLEMENT_HORIZON_BLOCKS` this is exactly
/// [`crate::consensus_fee_model`] (unit-tested).
pub fn settlement_fee(value: u64, floor_factor: u64, horizon_blocks: u64, height: u64) -> u64 {
    let fp = fee_params();
    let base = consensus_fee_breakdown(
        &ConsensusFeeInputs {
            size: consensus_tx_size_bytes(1, 2, 0),
            n_out: 2,
            n_memo: 0,
            claimed: ClaimedClass::Factor(FACTOR_SCALE),
            floor: floor_factor,
            elapsed: 0,
            height,
            output_sum: value,
        },
        &fp,
    )
    .base;
    base.saturating_add(capitalized_reset_charge(
        value,
        floor_factor,
        FACTOR_SCALE,
        horizon_blocks,
        fp.demurrage_rate_bps(height),
        fp.blocks_per_year,
    ))
}

/// Run the erosion experiment across all horizons for a fixed wealthy class at
/// a given adoption fraction (basis points of the top decile that exit).
pub fn gini_erosion_sweep(
    params: &GiniSweepParams,
    horizons: &[Horizon],
    adoption_bps: u32,
) -> Vec<GiniErosionRow> {
    // The no-demurrage baseline is horizon- and adoption-independent.
    let baseline_gini = run_world(params, horizons[0], World::NoDemurrage).gini;
    let to_bth = |p: u128| p as f64 / PICO_PER_BTH as f64;

    horizons
        .iter()
        .map(|&horizon| {
            let hold = run_world(params, horizon, World::Hold);
            let esc = run_world(params, horizon, World::Escape { adoption_bps });
            GiniErosionRow {
                horizon,
                adoption_bps,
                gini_hold: hold.gini,
                gini_escape: esc.gini,
                gini_erosion: esc.gini - hold.gini,
                delta_gini_hold: baseline_gini - hold.gini,
                delta_gini_escape: baseline_gini - esc.gini,
                pool_revenue_escape_bth: to_bth(esc.pool_pico),
                pool_revenue_hold_bth: to_bth(hold.pool_pico),
                burned_escape_bth: to_bth(esc.burned_pico),
                burned_hold_bth: to_bth(hold.burned_pico),
                uncollected_hold_bth: to_bth(hold.uncollected_pico),
            }
        })
        .collect()
}

/// Adoption fractions swept in the sensitivity table, in basis points of the
/// top decile that settle+wrap out: 25% / 50% / 100% (worst case).
pub fn adoption_levels() -> Vec<u32> {
    vec![2_500, 5_000, 10_000]
}

/// Top-decile velocities swept in the velocity-sensitivity table,
/// milli-spends per year: never, every 5 years (default), yearly, quarterly,
/// monthly.
pub fn top_decile_velocities() -> Vec<u32> {
    vec![0, 200, 1_000, 4_000, 12_000]
}

/// The horizons the #1698 decision compares (1yr vs the shipped 5yr).
fn decision_horizons() -> Vec<Horizon> {
    candidate_horizons()
        .into_iter()
        .filter(|h| h.label == "1yr" || h.label == "5yr")
        .collect()
}

/// One labelled variant run (velocity, dilution or start-height sensitivity)
/// at the decision horizons, 100% adoption.
#[derive(Clone, Debug)]
pub struct VariantRows {
    pub label: String,
    pub rows: Vec<GiniErosionRow>,
}

/// The full sweep report: the cost table plus the erosion tables.
#[derive(Clone, Debug)]
pub struct SettlementHorizonReport {
    pub horizons: Vec<Horizon>,
    pub classes: Vec<FactorClass>,
    pub cost_rows: Vec<CostRow>,
    /// The wealthy class used for the erosion experiment.
    pub erosion_class: FactorClass,
    /// Erosion at 100% adoption under the node-faithful spend-time model.
    pub erosion_rows: Vec<GiniErosionRow>,
    /// The same table under the pre-#1698 legacy yearly model (before/after).
    pub legacy_erosion_rows: Vec<GiniErosionRow>,
    /// Erosion sensitivity (spend-time): `(adoption_bps, rows)`.
    pub adoption_sensitivity: Vec<(u32, Vec<GiniErosionRow>)>,
    /// Top-decile velocity sensitivity at 1yr/5yr, 100% adoption.
    pub velocity_sensitivity: Vec<VariantRows>,
    /// Dilution adversary at 1yr/5yr, 100% adoption.
    pub dilution_sensitivity: Vec<VariantRows>,
    /// Launch (epoch-0) start at 1yr/5yr, 100% adoption.
    pub epoch0_rows: Vec<GiniErosionRow>,
    /// No-demurrage baseline Gini for the erosion population (Δgini anchor).
    pub baseline_gini: f64,
    /// Initial Gini of the erosion population (sanity reference).
    pub initial_gini: f64,
}

/// Run the complete settlement-horizon sweep with the default configuration.
pub fn run_settlement_horizon_sweep() -> SettlementHorizonReport {
    let horizons = candidate_horizons();
    let classes = factor_classes();
    let cost_rows = cost_table(&horizons, &classes);

    // Erosion experiment uses the most escape-prone class: the saturating (6x)
    // whale cluster.
    let erosion_class = *classes
        .iter()
        .max_by_key(|c| c.factor_scaled)
        .expect("at least one factor class");
    let gp = GiniSweepParams::default_for(erosion_class);
    let erosion_rows = gini_erosion_sweep(&gp, &horizons, 10_000);
    let legacy_erosion_rows = gini_erosion_sweep(
        &GiniSweepParams::legacy_for(erosion_class),
        &horizons,
        10_000,
    );
    let adoption_sensitivity = adoption_levels()
        .into_iter()
        .map(|bps| (bps, gini_erosion_sweep(&gp, &horizons, bps)))
        .collect();

    let dh = decision_horizons();
    let velocity_sensitivity = top_decile_velocities()
        .into_iter()
        .map(|v| {
            let mut p = gp.clone();
            p.decile_velocity_milli[9] = v;
            VariantRows {
                label: format!("{:.1}/yr", v as f64 / 1_000.0),
                rows: gini_erosion_sweep(&p, &dh, 10_000),
            }
        })
        .collect();
    let dilution_sensitivity = [DilutionAdversary::Honest, DilutionAdversary::MinCentroid]
        .into_iter()
        .map(|d| {
            let mut p = gp.clone();
            p.dilution = d;
            VariantRows {
                label: d.label().to_string(),
                rows: gini_erosion_sweep(&p, &dh, 10_000),
            }
        })
        .collect();
    let epoch0_rows = {
        let mut p = gp.clone();
        p.start_height = 0;
        gini_erosion_sweep(&p, &dh, 10_000)
    };

    let baseline_gini = run_world(&gp, horizons[0], World::NoDemurrage).gini;
    let mut init: Vec<u64> = vec![gp.poor_wealth_bth; gp.poor];
    init.extend(std::iter::repeat_n(gp.wealthy_wealth_bth, gp.wealthy));
    let initial_gini = calculate_gini(&init);

    SettlementHorizonReport {
        horizons,
        classes,
        cost_rows,
        erosion_class,
        erosion_rows,
        legacy_erosion_rows,
        adoption_sensitivity,
        velocity_sensitivity,
        dilution_sensitivity,
        epoch0_rows,
        baseline_gini,
        initial_gini,
    }
}

fn erosion_table(s: &mut String, rows: &[GiniErosionRow], with_burn: bool) {
    if with_burn {
        s.push_str(
            "| horizon | gini_hold | gini_escape | erosion | Δgini_hold | Δgini_escape | pool (escape, BTH) | pool (hold, BTH) | burned (escape, BTH) | burned (hold, BTH) | uncollected (hold, BTH) | floor |\n",
        );
        s.push_str(
            "|---------|----------:|------------:|--------:|-----------:|-------------:|-------------------:|-----------------:|---------------------:|-------------------:|------------------------:|:-----:|\n",
        );
    } else {
        s.push_str(
            "| horizon | gini_hold | gini_escape | erosion | Δgini_hold | Δgini_escape | pool (escape, BTH) | pool (hold, BTH) | floor |\n",
        );
        s.push_str(
            "|---------|----------:|------------:|--------:|-----------:|-------------:|-------------------:|-----------------:|:-----:|\n",
        );
    }
    for r in rows {
        s.push_str(&format!(
            "| {} ({:.2}yr) | {:.4} | {:.4} | {:+.4} | {:+.4} | {:+.4} | {:.2} | {:.2} |",
            r.horizon.label,
            r.horizon.years(),
            r.gini_hold,
            r.gini_escape,
            r.gini_erosion,
            r.delta_gini_hold,
            r.delta_gini_escape,
            r.pool_revenue_escape_bth,
            r.pool_revenue_hold_bth,
        ));
        if with_burn {
            s.push_str(&format!(
                " {:.2} | {:.2} | {:.2} |",
                r.burned_escape_bth, r.burned_hold_bth, r.uncollected_hold_bth
            ));
        }
        s.push_str(&format!(
            " {} |\n",
            if r.passes_floor() { "PASS" } else { "FAIL" }
        ));
    }
    s.push('\n');
}

fn variant_table(s: &mut String, header: &str, variants: &[VariantRows]) {
    s.push_str(&format!(
        "| {header} | horizon | Δgini_hold | Δgini_escape | erosion | pool (hold, BTH) | uncollected (hold, BTH) | floor |\n"
    ));
    s.push_str("|---|---------|-----------:|-------------:|--------:|-----------------:|------------------------:|:-----:|\n");
    for v in variants {
        for r in &v.rows {
            s.push_str(&format!(
                "| {} | {} | {:+.4} | {:+.4} | {:+.4} | {:.2} | {:.2} | {} |\n",
                v.label,
                r.horizon.label,
                r.delta_gini_hold,
                r.delta_gini_escape,
                r.gini_erosion,
                r.pool_revenue_hold_bth,
                r.uncollected_hold_bth,
                if r.passes_floor() { "PASS" } else { "FAIL" },
            ));
        }
    }
    s.push('\n');
}

/// Render the report as Markdown tables (the doc numbers are generated from
/// this, never hand-computed).
pub fn to_markdown(report: &SettlementHorizonReport) -> String {
    let mut s = String::new();

    s.push_str("### Factor classes (from the real `ClusterFactorCurve`)\n\n");
    s.push_str("| class | cluster wealth (BTH) | coin value (BTH) | factor |\n");
    s.push_str("|-------|---------------------:|-----------------:|-------:|\n");
    for c in &report.classes {
        s.push_str(&format!(
            "| {} | {} | {} | {:.3}x |\n",
            c.label,
            c.cluster_wealth_bth,
            c.coin_value_bth,
            c.factor_scaled as f64 / ClusterFactorCurve::FACTOR_SCALE as f64,
        ));
    }
    s.push('\n');

    s.push_str("### Wrap cost by horizon × factor class\n\n");
    s.push_str(
        "Wrap cost = settlement charge as a percentage of the settled coin value. \
         Churn-invariance margin = settlement charge ÷ (H-years of ordinary demurrage); \
         must be ≥ 1.00 (settling is never cheaper than holding-and-paying over the horizon).\n\n",
    );
    s.push_str("| horizon | class | factor | wrap cost (% of value) | churn-inv margin |\n");
    s.push_str("|---------|-------|-------:|-----------------------:|-----------------:|\n");
    for r in &report.cost_rows {
        s.push_str(&format!(
            "| {} ({:.2}yr) | {} | {:.3}x | {:.4}% | {:.4} |\n",
            r.horizon.label,
            r.horizon.years(),
            r.class.label,
            r.class.factor_scaled as f64 / ClusterFactorCurve::FACTOR_SCALE as f64,
            r.wrap_cost_frac * 100.0,
            r.churn_invariance_margin,
        ));
    }
    s.push('\n');

    let gp = GiniSweepParams::default_for(report.erosion_class);
    s.push_str(
        "### dGini erosion by horizon — spend-time model (worst case: 100% of top decile exits)\n\n",
    );
    s.push_str(&format!(
        "Erosion population: {} background holders (1x) + {} wealthy holders ({:.3}x, {} BTH each), \
         run {} years from height {} (epoch 1), seed {:#x}. Initial Gini {:.4}; no-demurrage baseline \
         Gini {:.4} (Δgini anchor). Demurrage is charged ONLY when a coin is spent, on its accrued age, \
         by `cluster_tax::consensus_fee_model` (the pure half of `Ledger::consensus_fee_floor`); the fee \
         is split 80% pool / 20% burned; per-decile velocities (milli-spends/yr) {:?}; honest rings of \
         {}. HOLD = top decile holds; ESCAPE = top decile settles+wraps at year 0 then exits. \
         Δgini_escape must stay > 0.05 (design floor).\n\n",
        gp.poor,
        gp.wealthy,
        report.erosion_class.factor_scaled as f64 / ClusterFactorCurve::FACTOR_SCALE as f64,
        report.erosion_class.coin_value_bth,
        gp.years,
        gp.start_height,
        gp.seed,
        report.initial_gini,
        report.baseline_gini,
        gp.decile_velocity_milli,
        CONSENSUS_RING_SIZE,
    ));
    erosion_table(&mut s, &report.erosion_rows, true);

    s.push_str("### Before/after: the same table under the legacy yearly model\n\n");
    s.push_str(
        "The pre-#1698 model: every wealthy holder pays one year of demurrage every year whether \
         or not it spends, 100% to the pool. The node has never charged this way.\n\n",
    );
    erosion_table(&mut s, &report.legacy_erosion_rows, false);

    s.push_str("### Sensitivity: Δgini_escape by horizon × adoption fraction (spend-time)\n\n");
    s.push_str(
        "How much residual redistribution gain survives if only a FRACTION of the top decile \
         wraps out. A cell PASSES the floor when Δgini_escape > 0.05.\n\n",
    );
    s.push_str("| horizon |");
    for (bps, _) in &report.adoption_sensitivity {
        s.push_str(&format!(" {}% adoption |", bps / 100));
    }
    s.push('\n');
    s.push_str("|---------|");
    for _ in &report.adoption_sensitivity {
        s.push_str("------------:|");
    }
    s.push('\n');
    for (hi, h) in report.horizons.iter().enumerate() {
        s.push_str(&format!("| {} ({:.2}yr) |", h.label, h.years()));
        for (_, rows) in &report.adoption_sensitivity {
            let r = &rows[hi];
            s.push_str(&format!(
                " {:+.4} {} |",
                r.delta_gini_escape,
                if r.passes_floor() { "P" } else { "F" },
            ));
        }
        s.push('\n');
    }
    s.push('\n');

    s.push_str("### Sensitivity: top-decile spend velocity (spend-time, 100% adoption)\n\n");
    variant_table(&mut s, "top-decile velocity", &report.velocity_sensitivity);

    s.push_str("### Sensitivity: decoy-dilution adversary (spend-time, 100% adoption)\n\n");
    variant_table(&mut s, "ring policy", &report.dilution_sensitivity);

    s.push_str(
        "### Launch start: epoch 0 (height 0, first year demurrage-free; 100% adoption)\n\n",
    );
    erosion_table(&mut s, &report.epoch0_rows, true);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn whale() -> FactorClass {
        *factor_classes()
            .iter()
            .max_by_key(|c| c.factor_scaled)
            .unwrap()
    }

    #[test]
    fn factor_classes_span_the_curve() {
        let classes = factor_classes();
        let min = classes.iter().map(|c| c.factor_scaled).min().unwrap();
        let max = classes.iter().map(|c| c.factor_scaled).max().unwrap();
        assert!(min < 3000, "smallest class should be well under 3x: {min}");
        assert!(max > 5000, "largest class should approach 6x: {max}");
    }

    #[test]
    fn settlement_is_never_cheaper_than_holding_the_horizon() {
        let report = run_settlement_horizon_sweep();
        for r in &report.cost_rows {
            assert!(
                r.churn_invariance_margin >= 1.0 - 1e-9,
                "settling cheaper than holding at horizon {} class {} (margin {})",
                r.horizon.label,
                r.class.label,
                r.churn_invariance_margin,
            );
        }
    }

    #[test]
    fn wrap_cost_rises_monotonically_with_horizon() {
        let report = run_settlement_horizon_sweep();
        for class in &report.classes {
            let mut last = -1.0f64;
            for h in &report.horizons {
                let row = report
                    .cost_rows
                    .iter()
                    .find(|r| r.horizon.label == h.label && r.class.label == class.label)
                    .unwrap();
                assert!(
                    row.wrap_cost_frac >= last - 1e-12,
                    "wrap cost dropped with horizon for {}",
                    class.label
                );
                last = row.wrap_cost_frac;
            }
        }
    }

    #[test]
    fn escape_erodes_gini_gain_and_short_horizons_erode_more() {
        // Under both charge models a cheaper (shorter) horizon must not
        // preserve MORE redistribution gain than a longer one.
        let report = run_settlement_horizon_sweep();
        for rows in [&report.erosion_rows, &report.legacy_erosion_rows] {
            let short = &rows[0];
            let long = rows.last().unwrap();
            assert!(
                short.delta_gini_escape <= long.delta_gini_escape + 1e-9,
                "1mo Δgini_escape {} vs 5yr {}",
                short.delta_gini_escape,
                long.delta_gini_escape,
            );
        }
        // The legacy yearly HOLD world clears the floor (pre-#1698 result).
        assert!(report.legacy_erosion_rows[0].delta_gini_hold > 0.05);
    }

    #[test]
    fn legacy_model_reproduces_pre_1698_numbers() {
        // The legacy path is the pre-#1698 code verbatim; pin two headline
        // numbers from docs/research/settlement-horizon-calibration.md §4.3
        // (pre-#1698 edition) so before/after stays honest.
        let report = run_settlement_horizon_sweep();
        let five = report.legacy_erosion_rows.last().unwrap();
        assert_eq!(format!("{:+.4}", five.delta_gini_escape), "+0.0796");
        assert_eq!(format!("{:+.4}", five.delta_gini_hold), "+0.1452");
        assert_eq!(format!("{:.2}", five.pool_revenue_hold_bth), "175855.39");
    }

    #[test]
    fn never_spending_holder_pays_zero() {
        // #1698 acceptance: the node only charges demurrage at spend time, so
        // a holder that never spends pays nothing, however long it holds.
        let mut p = GiniSweepParams::default_for(whale());
        p.decile_velocity_milli = [0; 10];
        let hold = run_world(
            &p,
            Horizon {
                blocks: 1,
                label: "x",
            },
            World::Hold,
        );
        assert_eq!(hold.pool_pico, 0);
        assert_eq!(hold.burned_pico, 0);
        let base = run_world(
            &p,
            Horizon {
                blocks: 1,
                label: "x",
            },
            World::NoDemurrage,
        );
        assert_eq!(hold.gini, base.gini, "nothing moved for a never-spender");
        // ...while the demurrage it dodged is visible as uncollected.
        assert!(hold.uncollected_pico > 0);
    }

    #[test]
    fn only_the_top_decile_spender_pays_demurrage() {
        // Wealthy spenders pay; background spenders pay only the tiny base
        // fee. The pool is 80% of the total fee, the burn 20%.
        let p = GiniSweepParams::default_for(whale());
        let hold = run_world(&p, candidate_horizons()[4], World::Hold);
        assert!(hold.pool_pico > 0);
        let total = hold.pool_pico + hold.burned_pico;
        let pool_frac = hold.pool_pico as f64 / total as f64;
        assert!((pool_frac - 0.8).abs() < 1e-6, "pool fraction {pool_frac}");
    }

    #[test]
    fn epoch_zero_charges_no_demurrage() {
        // Settlement at launch (height 0, epoch 0): the capitalized charge is
        // zero because the consensus rate is zero; only the base fee remains.
        let value = 100_000 * PICO_PER_BTH as u64;
        let launch = settlement_fee(value, 5_745, 5 * BLOCKS_PER_YEAR, 0);
        let base_only = settlement_fee(value, FACTOR_SCALE, 5 * BLOCKS_PER_YEAR, 0);
        assert_eq!(launch, base_only);
        assert!(launch < 1_000_000, "base fee only: {launch}");
        // A one-year run starting at launch collects strictly less demurrage
        // than the same run in epoch 1.
        let mut p = GiniSweepParams::default_for(whale());
        p.years = 1;
        p.decile_velocity_milli = [12_000; 10];
        let h = candidate_horizons()[4];
        let epoch1 = run_world(&p, h, World::Hold);
        p.start_height = 0;
        let epoch0 = run_world(&p, h, World::Hold);
        assert!(epoch0.pool_pico < epoch1.pool_pico / 100);
    }

    #[test]
    fn settlement_fee_at_shipped_horizon_is_the_consensus_model() {
        // At the shipped SETTLEMENT_HORIZON_BLOCKS the sweep's settlement fee
        // is exactly the shared consensus fee model's floor for a
        // background-claimed spend of a ring-floored coin (#1695/#1698).
        let fp = fee_params();
        let h = fp.first_demurrage_height();
        for (value_bth, floor) in [(1_000u64, 1_939u64), (100_000, 5_745), (50, 1_000)] {
            let value = value_bth * PICO_PER_BTH as u64;
            let model = consensus_fee_model(
                &ConsensusFeeInputs {
                    size: consensus_tx_size_bytes(1, 2, 0),
                    n_out: 2,
                    n_memo: 0,
                    claimed: ClaimedClass::Factor(FACTOR_SCALE),
                    floor,
                    elapsed: 0,
                    height: h,
                    output_sum: value,
                },
                &fp,
            );
            assert_eq!(
                settlement_fee(value, floor, SETTLEMENT_HORIZON_BLOCKS, h),
                model
            );
        }
    }

    #[test]
    fn honest_ring_does_not_dilute_but_min_centroid_does() {
        let curve = FeeConfig::default().cluster_curve;
        let p = GiniSweepParams::default_for(whale());
        let wealthy = Holder {
            value: 100_000 * PICO_PER_BTH as u64,
            eff_wealth: whale().cluster_wealth_bth as u128 * PICO_PER_BTH,
            received_at: 0,
            next_spend: None,
            interval: 0,
            wrapped: 0,
        };
        let poor = Holder {
            value: 1_000 * PICO_PER_BTH as u64,
            eff_wealth: 0,
            ..wealthy.clone()
        };
        let mut holders = vec![poor; p.poor];
        holders.extend(std::iter::repeat_n(wealthy.clone(), p.wealthy));
        let spender = holders.len() - 1;
        let mut rng = ChaCha8Rng::seed_from_u64(1);
        for policy in [DilutionAdversary::Honest, DilutionAdversary::MinCentroid] {
            let ring = build_ring(&holders, spender, policy, &mut rng);
            assert_eq!(ring.len(), CONSENSUS_RING_SIZE);
            let (claimed, floor, new_w) = ring_classes(&holders[spender], &ring, policy, &curve);
            assert_eq!(
                floor,
                claimed.max(ring_centroid_implied_factor(&ring, &curve))
            );
            match policy {
                DilutionAdversary::Honest => {
                    assert_eq!(floor, whale().factor_scaled);
                    assert_eq!(new_w, wealthy.eff_wealth);
                }
                DilutionAdversary::MinCentroid => {
                    assert!(floor < whale().factor_scaled, "diluted: {floor}");
                    assert!(new_w < wealthy.eff_wealth, "ratchets down");
                }
            }
        }
    }

    #[test]
    fn report_is_reproducible() {
        let a = to_markdown(&run_settlement_horizon_sweep());
        let b = to_markdown(&run_settlement_horizon_sweep());
        assert_eq!(a, b, "sweep output must be byte-for-byte deterministic");
    }

    #[test]
    fn markdown_contains_all_horizons_and_gates() {
        let md = to_markdown(&run_settlement_horizon_sweep());
        for h in candidate_horizons() {
            assert!(
                md.contains(h.label),
                "missing horizon {} in report",
                h.label
            );
        }
        assert!(md.contains("churn-inv margin"));
        assert!(md.contains("Δgini_escape"));
        assert!(md.contains("legacy yearly"));
        assert!(md.contains("min-centroid"));
    }
}
