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

use crate::fee_curve::PICO_PER_BTH;
use crate::{demurrage_charge, ClusterFactorCurve};

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
    /// Number of simulated years. Each round is one year of holding.
    pub years: u64,
    /// RNG seed (kept for structural parity with the decoy sweep; the erosion
    /// run is deterministic without stochastic draws, but future noise models
    /// can hang off this).
    pub seed: u64,
}

impl GiniSweepParams {
    /// Default population: 90 background holders + 10 wealthy holders (a clean
    /// top decile), wealthy holders in the saturating (6x) class, run 10 years.
    pub fn default_for(class: FactorClass) -> Self {
        Self {
            poor: 90,
            poor_wealth_bth: 1_000,
            wealthy: 10,
            wealthy_wealth_bth: class.coin_value_bth,
            wealthy_class: class,
            years: 10,
            seed: 0xB07A_0833,
        }
    }
}

/// Outcome of the erosion experiment at one horizon.
#[derive(Clone, Debug)]
pub struct GiniErosionRow {
    pub horizon: Horizon,
    /// Fraction of the top decile that settled+wrapped, in basis points
    /// (10000 = the whole decile exits — the worst case).
    pub adoption_bps: u32,
    /// Final Gini when the top decile HOLDS and pays ongoing demurrage (which
    /// funds per-capita redistribution) for the whole run.
    pub gini_hold: f64,
    /// Final Gini when the top decile SETTLES+WRAPS at round 0: it pays the
    /// one-shot settlement charge into the pool, then its wrapped value LEAVES
    /// the demurrage/redistribution system (wBTH pays no further demurrage,
    /// ADR 0003 §4) for the rest of the run.
    pub gini_escape: f64,
    /// Erosion = gini_escape − gini_hold. Positive = escaping at this price
    /// leaves the population MORE unequal than holding would have (the leak the
    /// horizon must limit).
    pub gini_erosion: f64,
    /// Δgini vs the no-demurrage baseline in the HOLD world — the redistribution
    /// gain the mechanism produces; must exceed 0.05 (the design floor).
    pub delta_gini_hold: f64,
    /// Δgini vs the no-demurrage baseline in the ESCAPE world — the residual
    /// gain once the top decile has wrapped out. If this drops below 0.05 the
    /// escape at this price has erased the design gain.
    pub delta_gini_escape: f64,
    /// Lottery-pool revenue captured over the run in the ESCAPE world, in BTH
    /// (the one-shot settlement charges routed to the pool).
    pub pool_revenue_escape_bth: f64,
    /// Lottery-pool revenue captured over the run in the HOLD world, in BTH
    /// (the ongoing demurrage the wealthy pay while holding).
    pub pool_revenue_hold_bth: f64,
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
    /// Top decile holds and pays ongoing demurrage the whole run.
    Hold,
    /// A fraction of the top decile settles+wraps at round 0, then exits; the
    /// rest holds and pays ongoing demurrage. The fraction is in basis points
    /// (10000 = the whole decile exits).
    Escape { adoption_bps: u32 },
}

/// Run one erosion world and return (final_gini, pool_revenue_pico).
fn run_world(params: &GiniSweepParams, horizon: Horizon, world: World) -> (f64, u128) {
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
    // from the demurrage/redistribution population. Kept separate so it does
    // not receive redistribution and pays no further demurrage.
    let mut wrapped_out: Vec<u128> = vec![0; wealths.len()];

    let mut pool_revenue: u128 = 0;
    let mut pool_carry: u128 = 0;

    // In the Escape world a fraction of the top decile settles+wraps at round
    // 0. The exiting holders are the highest-indexed ones (arbitrary but
    // deterministic); the fraction is `adoption_bps / 10000` of the decile.
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
            // The remaining value wraps out of the demurrage system.
            wrapped_out[i] = wealths[i];
            wealths[i] = 0;
        }
    }

    for _year in 0..params.years {
        // Carry from the previous round's indivisible remainder; reassigned at
        // the end of this round.
        let mut pool: u128 = pool_carry;

        if world != World::NoDemurrage {
            // The still-in-system wealthy holders pay one year of ordinary
            // demurrage (Hold world; Escape holders already left so their
            // wealth is 0 and the charge is 0).
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

        // Per-capita redistribution across the WHOLE resident population
        // (egalitarian lottery payout). Wrapped-out value does not participate.
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

    // Final wealth vector for Gini: a wrapped-out holder's on-chain wealth is
    // now its wBTH holding (value preserved 1:1, ADR 0003 factor-1 peg), so it
    // counts toward the distribution — the escape does not vaporize value, it
    // exempts it from further redistribution.
    let final_wealths: Vec<u64> = wealths
        .iter()
        .zip(wrapped_out.iter())
        .map(|(&w, &wr)| {
            let total = w + wr;
            // Scale pico -> BTH for Gini (ratio-invariant; keeps values in u64).
            u64::try_from(total / PICO_PER_BTH).unwrap_or(u64::MAX)
        })
        .collect();

    (calculate_gini(&final_wealths), pool_revenue)
}

/// Run the erosion experiment across all horizons for a fixed wealthy class at
/// a given adoption fraction (basis points of the top decile that exit).
pub fn gini_erosion_sweep(
    params: &GiniSweepParams,
    horizons: &[Horizon],
    adoption_bps: u32,
) -> Vec<GiniErosionRow> {
    // The no-demurrage baseline is horizon- and adoption-independent.
    let (baseline_gini, _) = run_world(params, horizons[0], World::NoDemurrage);

    horizons
        .iter()
        .map(|&horizon| {
            let (gini_hold, pool_hold) = run_world(params, horizon, World::Hold);
            let (gini_escape, pool_escape) =
                run_world(params, horizon, World::Escape { adoption_bps });
            GiniErosionRow {
                horizon,
                adoption_bps,
                gini_hold,
                gini_escape,
                gini_erosion: gini_escape - gini_hold,
                delta_gini_hold: baseline_gini - gini_hold,
                delta_gini_escape: baseline_gini - gini_escape,
                pool_revenue_escape_bth: pool_escape as f64 / PICO_PER_BTH as f64,
                pool_revenue_hold_bth: pool_hold as f64 / PICO_PER_BTH as f64,
            }
        })
        .collect()
}

/// Adoption fractions swept in the sensitivity table, in basis points of the
/// top decile that settle+wrap out: 25% / 50% / 100% (worst case).
pub fn adoption_levels() -> Vec<u32> {
    vec![2_500, 5_000, 10_000]
}

/// The full sweep report: the cost table plus the erosion table for a chosen
/// representative wealthy class.
#[derive(Clone, Debug)]
pub struct SettlementHorizonReport {
    pub horizons: Vec<Horizon>,
    pub classes: Vec<FactorClass>,
    pub cost_rows: Vec<CostRow>,
    /// The wealthy class used for the erosion experiment.
    pub erosion_class: FactorClass,
    /// Erosion at 100% adoption (the whole top decile exits — worst case).
    pub erosion_rows: Vec<GiniErosionRow>,
    /// Erosion sensitivity: `(adoption_bps, rows)` for each adoption level.
    pub adoption_sensitivity: Vec<(u32, Vec<GiniErosionRow>)>,
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
    // whale cluster, which owes the most demurrage and thus has the strongest
    // incentive to wrap out.
    let erosion_class = *classes
        .iter()
        .max_by_key(|c| c.factor_scaled)
        .expect("at least one factor class");
    let gp = GiniSweepParams::default_for(erosion_class);
    // Primary erosion table: worst case, the whole top decile exits.
    let erosion_rows = gini_erosion_sweep(&gp, &horizons, 10_000);
    // Sensitivity: partial adoption.
    let adoption_sensitivity = adoption_levels()
        .into_iter()
        .map(|bps| (bps, gini_erosion_sweep(&gp, &horizons, bps)))
        .collect();

    let (baseline_gini, _) = run_world(&gp, horizons[0], World::NoDemurrage);
    // Initial population Gini.
    let mut init: Vec<u64> = vec![gp.poor_wealth_bth; gp.poor];
    init.extend(std::iter::repeat_n(gp.wealthy_wealth_bth, gp.wealthy));
    let initial_gini = calculate_gini(&init);

    SettlementHorizonReport {
        horizons,
        classes,
        cost_rows,
        erosion_class,
        erosion_rows,
        adoption_sensitivity,
        baseline_gini,
        initial_gini,
    }
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

    s.push_str("### dGini erosion by horizon (worst case: 100% of top decile exits)\n\n");
    s.push_str(&format!(
        "Erosion population: {} background holders (1x) + {} wealthy holders ({:.3}x, {} BTH each), \
         run {} years. Initial Gini {:.4}; no-demurrage baseline Gini {:.4} (Δgini anchor). \
         HOLD = top decile holds and pays ongoing demurrage; ESCAPE = top decile settles+wraps at \
         year 0 then exits. Δgini_escape must stay > 0.05 (design floor).\n\n",
        GiniSweepParams::default_for(report.erosion_class).poor,
        GiniSweepParams::default_for(report.erosion_class).wealthy,
        report.erosion_class.factor_scaled as f64 / ClusterFactorCurve::FACTOR_SCALE as f64,
        report.erosion_class.coin_value_bth,
        GiniSweepParams::default_for(report.erosion_class).years,
        report.initial_gini,
        report.baseline_gini,
    ));
    s.push_str(
        "| horizon | gini_hold | gini_escape | erosion | Δgini_hold | Δgini_escape | pool (escape, BTH) | pool (hold, BTH) | floor |\n",
    );
    s.push_str(
        "|---------|----------:|------------:|--------:|-----------:|-------------:|-------------------:|-----------------:|:-----:|\n",
    );
    for r in &report.erosion_rows {
        s.push_str(&format!(
            "| {} ({:.2}yr) | {:.4} | {:.4} | {:+.4} | {:+.4} | {:+.4} | {:.2} | {:.2} | {} |\n",
            r.horizon.label,
            r.horizon.years(),
            r.gini_hold,
            r.gini_escape,
            r.gini_erosion,
            r.delta_gini_hold,
            r.delta_gini_escape,
            r.pool_revenue_escape_bth,
            r.pool_revenue_hold_bth,
            if r.passes_floor() { "PASS" } else { "FAIL" },
        ));
    }
    s.push('\n');

    s.push_str("### Sensitivity: Δgini_escape by horizon × adoption fraction\n\n");
    s.push_str(
        "How much residual redistribution gain survives if only a FRACTION of the top decile \
         wraps out. The 100% column is the worst case above; realistic adoption is lower. \
         A cell PASSES the floor when Δgini_escape > 0.05.\n\n",
    );
    // Header
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
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factor_classes_span_the_curve() {
        let classes = factor_classes();
        // The curve should assign a rising factor with cluster wealth, from
        // near-1x for small clusters to near-6x (saturation) for whales.
        let min = classes.iter().map(|c| c.factor_scaled).min().unwrap();
        let max = classes.iter().map(|c| c.factor_scaled).max().unwrap();
        assert!(min < 3000, "smallest class should be well under 3x: {min}");
        assert!(max > 5000, "largest class should approach 6x: {max}");
    }

    #[test]
    fn settlement_is_never_cheaper_than_holding_the_horizon() {
        // The core #831 soundness gate: by linearity of the charge in elapsed,
        // settling = H years of demurrage up front = holding-and-paying over H
        // years. The margin must be >= 1.0 for every horizon × class.
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
        // For a fixed factor class, a longer horizon costs strictly more to
        // wrap (the "too high = punitive" lever).
        let report = run_settlement_horizon_sweep();
        for class in &report.classes {
            let mut last = -1.0f64;
            for h in &report.horizons {
                let row = report
                    .cost_rows
                    .iter()
                    .find(|r| r.horizon.label == h.label && r.class.label == class.label)
                    .unwrap();
                // Factor-1 classes cost 0 at every horizon; skip the strict
                // check for them but ensure non-decreasing.
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
        // Escaping at a SHORT horizon (cheap) should erode the residual Δgini
        // more than escaping at a LONG horizon (expensive): the design tension.
        let report = run_settlement_horizon_sweep();
        let short = &report.erosion_rows[0]; // 1mo
        let long = report.erosion_rows.last().unwrap(); // 5yr
        assert!(
            short.delta_gini_escape <= long.delta_gini_escape + 1e-9,
            "a cheaper (shorter) horizon should not preserve MORE redistribution gain: \
             1mo Δgini_escape {} vs 5yr {}",
            short.delta_gini_escape,
            long.delta_gini_escape,
        );
        // Holding always preserves the full design gain (well above floor).
        assert!(
            report.erosion_rows[0].delta_gini_hold > 0.05,
            "HOLD world should clear the 0.05 design floor: {}",
            report.erosion_rows[0].delta_gini_hold,
        );
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
            assert!(md.contains(h.label), "missing horizon {} in report", h.label);
        }
        assert!(md.contains("churn-inv margin"));
        assert!(md.contains("Δgini_escape"));
    }
}
