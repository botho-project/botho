//! Reward-cap sweep — the **Path C ratification gate** for the CT-compatible
//! lottery-selection research (issue #902).
//!
//! # Why this exists
//!
//! [`super::lottery_selection_sweep`] (memo §1–§6) proved a hard negative — no
//! value-free *weight* is split-invariant — and [`super::
//! lottery_sybil_brake_sweep`] (§7) showed the last-N window is
//! *capture-neutral* and the only real brake is the economic invariant `R ≤
//! ρ·base_fee` (reward ≤ organic circulation cost), which the earlier sweep
//! treated as an *external* condition that fails during bootstrap/quiet
//! regimes.
//!
//! The maintainer's chosen **Path C** turns that invariant from an external
//! hope into a *construction*: the per-block lottery reward is **defined** as
//!
//! ```text
//! R = min(actual_fee_pool, ρ · base_fee)
//! ```
//!
//! where **ρ counts ALL outputs in the last-N window, including the whale's own
//! splits** (consensus cannot distinguish whale from organic under CT — that is
//! the threat model). Because `R` is now *pinned* to `ρ·base_fee`, the
//! `R ≤ ρ·base_fee` invariant holds by definition at every activity level. This
//! module is the final validation gate for that design, testing two claims:
//!
//! * **Claim 1 — the reward-cap makes splitting net-zero.** A whale that
//!   creates `k` outputs/block competing against `ρ_o` organic outputs/block
//!   under a uniform draw wins `k/(ρ_o+k)·R` and pays `k·base_fee`. With `R =
//!   (ρ_o+k)·base_fee` the two are **exactly equal** — net-zero for *every*
//!   split factor `k`, window `N`, and organic rate `ρ_o`. [`claim1_sweep`]
//!   confirms (or refutes) this and shows what the cap prevents.
//!
//! * **Claim 2 — the cap doesn't throttle redistribution into irrelevance.**
//!   The same cap that buys Sybil-safety also clamps the payout at `ρ·base_fee`
//!   (a few BTH/block), while the fee pool is 80% of ALL fees — dominated by
//!   large **demurrage** fees (`fee ∝ value×factor×time`) that arrive in rare,
//!   huge lumps. [`claim2_throughput`] measures what fraction of demurrage
//!   revenue actually flows through the capped lottery vs is capped out, under
//!   two excess-handling policies (burn vs carry-forward), and [`claim2_gini`]
//!   measures the resulting Δgini against the uncapped windowed lottery (§7's
//!   +0.1144) and the value-weighted incumbent.
//!
//! # Method
//!
//! Deterministic and RNG-free (the per-block lottery draw averages to the
//! weight share over a long run, so realised capture and throughput are
//! expected values computed in closed form). Reuses the shipped kernels: the
//! real [`crate::demurrage_charge`] (the value×factor×time fee curve that sets
//! the lump sizes), the real [`crate::ClusterFactorCurve`] (tier factors), the
//! shared [`super::metrics::calculate_gini`], and the constants +
//! [`public_fee_pico`](super::lottery_selection_sweep::public_fee_pico) proxy
//! from the sibling sweeps (NO second implementation). `SEED` is documented for
//! parity; there are no draws. Full write-up: `docs/research/
//! ct-compatible-lottery-selection.md` §9.

use crate::{fee_curve::PICO_PER_BTH, ClusterFactorCurve};

use super::{
    lottery_selection_sweep::{BASE_FEE_PICO, BLOCKS_PER_YEAR, RATE_BPS},
    lottery_sybil_brake_sweep::{honest_tiers, HonestTier},
    metrics::calculate_gini,
};

/// Deterministic seed (documented for parity with the sibling sweeps; the model
/// is expected-value and takes no draws).
pub const SEED: u64 = 0xB07A_0902_3;

/// Base per-output fee in BTH (0.25 BTH), from the sibling sweep's
/// [`BASE_FEE_PICO`].
pub fn base_fee_bth() -> f64 {
    BASE_FEE_PICO as f64 / PICO_PER_BTH as f64
}

/// One year of ordinary demurrage for a holding, in picocredits, via the real
/// kernel. `value` and the returned charge are in the same (picocredit) units —
/// `demurrage_charge` is linear in value and scale-agnostic.
fn annual_demurrage_pico(value_pico: u128, factor_scaled: u64) -> u128 {
    let v = u64::try_from(value_pico).unwrap_or(u64::MAX);
    crate::demurrage_charge(v, factor_scaled, BLOCKS_PER_YEAR, RATE_BPS, BLOCKS_PER_YEAR) as u128
}

// ===========================================================================
// Claim 1 — the reward-cap makes splitting net-zero
// ===========================================================================

/// One (ρ_o, k, N) point of the net-zero experiment.
///
/// The whale creates `k` outputs/block and refreshes them to stay inside the
/// last-N window; the organic economy creates `ρ_o` outputs/block. Under a
/// uniform draw, the whale's stationary ticket share is `k/(ρ_o+k)` (both whale
/// and organic coins live N blocks, so the window fraction equals the
/// creation-rate fraction — **N cancels**). The reward is capped at
/// `R = min(pool, ρ_total·base_fee)` with `ρ_total = ρ_o + k` counting the
/// whale's own outputs.
#[derive(Clone, Debug)]
pub struct Claim1Row {
    pub rho_o: f64,
    pub k: f64,
    pub window_blocks: u64,
    /// The available fee pool (80% of all fees) fed to the `min()`, in BTH.
    pub pool_bth: f64,
    /// Whale ticket share `k/(ρ_o+k)`.
    pub share: f64,
    /// Capped reward `R = min(pool, (ρ_o+k)·base_fee)`, in BTH.
    pub reward_capped_bth: f64,
    /// Whale winnings/block `= share·R`, in BTH.
    pub winnings_bth: f64,
    /// Whale cost/block `= k·base_fee`, in BTH.
    pub cost_bth: f64,
    /// Realised whale net/block `= winnings − cost`, in BTH. Claim 1 predicts
    /// **exactly 0** when the cap binds.
    pub net_bth: f64,
    /// Counterfactual net/block if the reward were **uncapped** at a fat pool
    /// `R_fat` (what the whale would extract absent the cap) — shows the cap is
    /// load-bearing.
    pub net_uncapped_bth: f64,
    /// Counterfactual net/block if the cap counted **only organic** outputs
    /// (`R = ρ_o·base_fee`, ignoring the whale's splits) — still ≤ 0, showing
    /// counting-all is the tightest (break-even) choice, not a loophole.
    pub net_organic_only_cap_bth: f64,
}

/// A generously fat uncapped reward (BTH/block) used only for the "what the cap
/// prevents" counterfactual column — a bootstrap-scale demurrage pool.
pub const FAT_UNCAPPED_REWARD_BTH: f64 = 20.0;

/// Organic output rates ρ_o swept (outputs/block): quiet, busy, very busy.
pub fn organic_rates() -> Vec<f64> {
    vec![2.0, 20.0, 200.0]
}

/// Whale split factors k swept (outputs/block created and refreshed).
pub fn split_factors() -> Vec<f64> {
    vec![1.0, 5.0, 50.0, 500.0, 5_000.0]
}

/// Window sizes swept (blocks) — included to *demonstrate* N-invariance of the
/// net; the value is identical across N.
pub fn window_sizes() -> Vec<u64> {
    vec![1_000, 10_000, 100_000]
}

/// Compute one net-zero point. `pool_bth` is the available fee pool (80% of all
/// fees) for the `min()`; pass a large value to exercise the cap-binding case
/// (the whale's best case for extraction).
pub fn claim1_point(rho_o: f64, k: f64, window: u64, pool_bth: f64) -> Claim1Row {
    let base = base_fee_bth();
    let rho_total = rho_o + k;
    let share = k / rho_total;
    let cap_bth = rho_total * base;
    let reward = pool_bth.min(cap_bth);

    let winnings = share * reward;
    let cost = k * base;
    let net = winnings - cost;

    // Counterfactual: no cap, whale draws its share of a fat fixed reward.
    let net_uncapped = share * FAT_UNCAPPED_REWARD_BTH - cost;
    // Counterfactual: cap counts only organic outputs (ρ_o·base), ignoring the
    // whale — the whale then draws share of a smaller, fixed cap.
    let reward_org_only = pool_bth.min(rho_o * base);
    let net_org_only = share * reward_org_only - cost;

    Claim1Row {
        rho_o,
        k,
        window_blocks: window,
        pool_bth,
        share,
        reward_capped_bth: reward,
        winnings_bth: winnings,
        cost_bth: cost,
        net_bth: net,
        net_uncapped_bth: net_uncapped,
        net_organic_only_cap_bth: net_org_only,
    }
}

/// Full Claim-1 sweep over (ρ_o, k) at the mid window, plus a small
/// N-invariance probe. `pool_bth` is set large so the cap binds (whale's best
/// case).
pub fn claim1_sweep() -> Vec<Claim1Row> {
    // Large pool so R = cap (cap-binding, the adversary's best case).
    let big_pool = 1.0e9;
    let mut rows = Vec::new();
    for rho_o in organic_rates() {
        for k in split_factors() {
            rows.push(claim1_point(rho_o, k, 10_000, big_pool));
        }
    }
    rows
}

/// N-invariance probe: the same (ρ_o, k) across all window sizes — the net is
/// identical, proving the window does not enter the capture ledger.
pub fn claim1_window_probe() -> Vec<Claim1Row> {
    let big_pool = 1.0e9;
    window_sizes()
        .into_iter()
        .map(|n| claim1_point(20.0, 500.0, n, big_pool))
        .collect()
}

/// Pool-limited probe: when the actual fee pool is *below* the cap, `R = pool`
/// and the whale's net goes strictly **negative** (it loses). Shows net ≤ 0 is
/// the theorem; net = 0 only at the cap-binding ceiling.
pub fn claim1_pool_limited_probe() -> Vec<Claim1Row> {
    // Busy chain, aggressive split; sweep the pool from below to above the cap.
    let rho_o = 20.0;
    let k = 500.0;
    let cap = (rho_o + k) * base_fee_bth(); // = 130 BTH
    vec![
        claim1_point(rho_o, k, 10_000, 0.25 * cap),
        claim1_point(rho_o, k, 10_000, 0.5 * cap),
        claim1_point(rho_o, k, 10_000, cap),
        claim1_point(rho_o, k, 10_000, 4.0 * cap),
    ]
}

// ===========================================================================
// Claim 2 — throughput (does the cap throttle redistribution into a trickle?)
// ===========================================================================

/// Per-tier spend granularity for the throughput model: how many separate
/// transactions the tier's annual turnover is split into. Big holders make few
/// big transactions (a whale's year of accrued demurrage lands in ~1 huge lump
/// that dwarfs the per-block cap); retail makes many small ones (each fee well
/// under the cap). This is the lumpiness the per-block cap penalises.
#[derive(Clone, Copy, Debug)]
pub struct GranularTier {
    pub tier: HonestTier,
    /// Number of separate spends per year (spread across blocks). Each spend
    /// realises `annual_demurrage / granularity` of accrued fee.
    pub granularity: u64,
}

/// The default throughput population: the §4.1 honest tiers, annotated with
/// realistic spend granularity (retail transacts often; whales rarely).
pub fn granular_population() -> Vec<GranularTier> {
    honest_tiers()
        .into_iter()
        .map(|tier| {
            // Big, idle holders make few large transactions per year; the poor
            // commerce tier makes many small ones.
            let granularity = match tier.cluster_wealth_bth {
                w if w >= 5_000_000 => 1, // whale: ~one big annual spend
                w if w >= 100_000 => 2,   // wealthy: a couple big spends
                w if w >= 10_000 => 4,    // mid
                _ => 52,                  // retail commerce: ~weekly
            };
            GranularTier { tier, granularity }
        })
        .collect()
}

/// How the excess above the per-block cap is handled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExcessPolicy {
    /// Burn the per-block excess (deflationary). Each demurrage *lump* is
    /// clamped to the cap; everything above is destroyed and never
    /// redistributed.
    Burn,
    /// Carry the excess forward in an accumulating pool, drained at ≤ cap per
    /// block. Because the annual drain capacity `cap·BLOCKS_PER_YEAR` vastly
    /// exceeds annual inflow on a mature chain, essentially all revenue is
    /// eventually redistributed (just delayed) — *provided* mean inflow/block ≤
    /// cap (the stability condition, checked separately).
    CarryForward,
}

/// Result of the throughput experiment for one (cap, policy) point.
#[derive(Clone, Debug)]
pub struct ThroughputRow {
    /// Organic output rate ρ that sets the cap `= ρ·base_fee`.
    pub rho: f64,
    /// Per-block payout cap in BTH (`ρ·base_fee`).
    pub cap_bth: f64,
    pub policy: ExcessPolicy,
    /// Total annual demurrage revenue (BTH) generated by the population.
    pub total_revenue_bth: f64,
    /// Annual revenue that actually flows through the lottery (BTH).
    pub through_bth: f64,
    /// Fraction of revenue redistributed (through / total). The headline.
    pub throughput_frac: f64,
    /// Mean demurrage inflow per block (BTH) — compare to `cap_bth` for the
    /// carry-forward stability condition.
    pub mean_inflow_per_block_bth: f64,
}

/// Annual demurrage revenue and per-lump throughput for the population under a
/// per-block cap and excess policy.
pub fn throughput_point(pop: &[GranularTier], rho: f64, policy: ExcessPolicy) -> ThroughputRow {
    let curve = ClusterFactorCurve::default_params();
    let base = base_fee_bth();
    let cap_bth = rho * base;
    let cap_pico = (cap_bth * PICO_PER_BTH as f64) as u128;

    let mut total_pico: u128 = 0;
    let mut through_pico: u128 = 0;
    for gt in pop {
        let f = curve.factor(gt.tier.cluster_wealth_bth as u128 * PICO_PER_BTH);
        let holder_annual = annual_demurrage_pico(gt.tier.wealth_bth as u128 * PICO_PER_BTH, f);
        for _ in 0..gt.tier.count {
            total_pico += holder_annual;
            match policy {
                ExcessPolicy::Burn => {
                    // The annual accrual is realised in `granularity` separate
                    // lumps; each lump is clamped to the per-block cap.
                    let lump = holder_annual / gt.granularity as u128;
                    let through = lump.min(cap_pico) * gt.granularity as u128;
                    through_pico += through;
                }
                ExcessPolicy::CarryForward => {
                    // Excess waits in the pool and drains over the year's spare
                    // block capacity; the annual drain ceiling is
                    // cap·BLOCKS_PER_YEAR per holder-share, which dwarfs any
                    // single holder's annual accrual, so all of it clears.
                    through_pico += holder_annual;
                }
            }
        }
    }

    let to_bth = |p: u128| p as f64 / PICO_PER_BTH as f64;
    let total_bth = to_bth(total_pico);
    let through_bth = to_bth(through_pico);
    ThroughputRow {
        rho,
        cap_bth,
        policy,
        total_revenue_bth: total_bth,
        through_bth,
        throughput_frac: if total_bth > 0.0 {
            through_bth / total_bth
        } else {
            0.0
        },
        mean_inflow_per_block_bth: total_bth / BLOCKS_PER_YEAR as f64,
    }
}

/// ρ values (output rates) swept for the cap level.
pub fn throughput_rhos() -> Vec<f64> {
    vec![2.0, 20.0, 200.0]
}

/// Run the throughput sweep across ρ (cap level) × {burn, carry-forward}.
pub fn claim2_throughput() -> Vec<ThroughputRow> {
    let pop = granular_population();
    let mut rows = Vec::new();
    for rho in throughput_rhos() {
        for policy in [ExcessPolicy::Burn, ExcessPolicy::CarryForward] {
            rows.push(throughput_point(&pop, rho, policy));
        }
    }
    rows
}

/// One row of the granularity-sensitivity probe: burn throughput as a function
/// of a global granularity multiplier applied to every tier (smoother spending
/// ⇒ more revenue clears the cap).
#[derive(Clone, Debug)]
pub struct GranularityRow {
    pub granularity_mult: u64,
    pub throughput_frac: f64,
}

/// Granularity sensitivity: multiply every tier's spend granularity and watch
/// burn throughput climb toward 1.0 as spending smooths out. At the realistic
/// low-granularity end (big holders make few big spends), throughput is a
/// trickle.
pub fn claim2_granularity_probe(rho: f64) -> Vec<GranularityRow> {
    let base_pop = granular_population();
    [1u64, 4, 16, 64, 256]
        .into_iter()
        .map(|mult| {
            let pop: Vec<GranularTier> = base_pop
                .iter()
                .map(|gt| GranularTier {
                    granularity: gt.granularity * mult,
                    ..*gt
                })
                .collect();
            let row = throughput_point(&pop, rho, ExcessPolicy::Burn);
            GranularityRow {
                granularity_mult: mult,
                throughput_frac: row.throughput_frac,
            }
        })
        .collect()
}

// ===========================================================================
// Claim 2 — Gini under the cap
// ===========================================================================

/// The lottery selection + cap policy scored on Δgini.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GiniPolicy {
    /// Burn baseline: collect demurrage, redistribute nothing.
    Burn,
    /// Uncapped uniform windowed lottery (all demurrage redistributed to
    /// circulated holders) — the §7.4 yardstick (+0.1144).
    UncappedWindowed,
    /// Capped uniform windowed lottery, excess **burned** (per-block cap clamps
    /// each demurrage lump; the excess is destroyed).
    CappedBurn,
    /// Capped uniform windowed lottery, excess **carried forward** (all revenue
    /// eventually redistributed; equals uncapped over a 10-yr horizon on a
    /// mature chain).
    CappedCarryForward,
    /// Value-weighted incumbent (ClusterWeighted, `value·(maxf−f+1)`) — the
    /// NOT-CT-compatible progressive yardstick, redistributes to *all* holders
    /// by value·tilt.
    ValueWeightedAll,
}

/// Result of the Gini track for one policy.
#[derive(Clone, Debug)]
pub struct GiniRow {
    pub policy: GiniPolicy,
    pub final_gini: f64,
    /// Δgini = burn_gini − final_gini (higher = more equalizing).
    pub delta_gini: f64,
}

/// Run a 10-year collect-and-redistribute loop under a Gini policy. Every
/// holder pays ordinary demurrage each year; the pool (subject to the per-block
/// cap and excess policy) is redistributed among the eligible set. Reuses the
/// real [`crate::demurrage_charge`] and [`calculate_gini`]; expected-value, no
/// RNG.
fn run_gini(pop: &[GranularTier], years: u64, rho: f64, policy: GiniPolicy) -> f64 {
    let curve = ClusterFactorCurve::default_params();
    let base = base_fee_bth();
    let cap_pico = ((rho * base) * PICO_PER_BTH as f64) as u128;

    // Flatten to per-holder wealth / factor / circulated / granularity / value.
    let mut wealth: Vec<u128> = Vec::new();
    let mut factor: Vec<u64> = Vec::new();
    let mut circ: Vec<bool> = Vec::new();
    let mut gran: Vec<u64> = Vec::new();
    for gt in pop {
        let f = curve.factor(gt.tier.cluster_wealth_bth as u128 * PICO_PER_BTH);
        for _ in 0..gt.tier.count {
            wealth.push(gt.tier.wealth_bth as u128 * PICO_PER_BTH);
            factor.push(f);
            circ.push(gt.tier.circulated);
            gran.push(gt.granularity);
        }
    }
    let max_factor = ClusterFactorCurve::default_params().factor(u128::MAX);

    let mut carry: u128 = 0;
    for _ in 0..years {
        // --- Collect one year of demurrage; compute the redistributable pool
        //     after the cap/excess policy, and the value-weight per holder.
        let mut pool: u128 = carry;
        let mut charge_v: Vec<u128> = vec![0; wealth.len()];
        for i in 0..wealth.len() {
            let d = annual_demurrage_pico(wealth[i], factor[i]).min(wealth[i]);
            wealth[i] -= d;
            charge_v[i] = d;
            let through = match policy {
                GiniPolicy::Burn => 0,
                GiniPolicy::CappedBurn => {
                    let lump = d / gran[i] as u128;
                    lump.min(cap_pico) * gran[i] as u128
                }
                // Uncapped / carry-forward / value-weighted all put the full
                // demurrage into the redistributable pool.
                _ => d,
            };
            pool += through;
        }

        if policy == GiniPolicy::Burn {
            carry = 0;
            continue;
        }

        // --- Redistribute the pool.
        let weights: Vec<u128> = (0..wealth.len())
            .map(|i| match policy {
                GiniPolicy::ValueWeightedAll => {
                    // Incumbent: value·(maxf − f + 1), to ALL holders.
                    let tilt = (max_factor - factor[i] + ClusterFactorCurve::FACTOR_SCALE) as u128;
                    (wealth[i] / PICO_PER_BTH).saturating_mul(tilt)
                }
                // Uniform over *circulated* (windowed) holders.
                _ => {
                    if circ[i] {
                        1
                    } else {
                        0
                    }
                }
            })
            .collect();
        let total_w: u128 = weights.iter().sum();
        if total_w == 0 {
            carry = pool;
            continue;
        }
        let mut distributed: u128 = 0;
        for i in 0..wealth.len() {
            let share = pool.saturating_mul(weights[i]) / total_w;
            wealth[i] += share;
            distributed += share;
        }
        carry = pool - distributed;
    }

    let final_wealths: Vec<u64> = wealth
        .iter()
        .map(|&w| u64::try_from(w / PICO_PER_BTH).unwrap_or(u64::MAX))
        .collect();
    calculate_gini(&final_wealths)
}

/// Run the Gini track for all policies at a given ρ (cap level), scoring Δgini
/// vs the burn baseline.
pub fn claim2_gini(rho: f64) -> Vec<GiniRow> {
    let pop = granular_population();
    let years = 10;
    let burn = run_gini(&pop, years, rho, GiniPolicy::Burn);
    [
        GiniPolicy::UncappedWindowed,
        GiniPolicy::CappedCarryForward,
        GiniPolicy::CappedBurn,
        GiniPolicy::ValueWeightedAll,
    ]
    .into_iter()
    .map(|policy| {
        let g = run_gini(&pop, years, rho, policy);
        GiniRow {
            policy,
            final_gini: g,
            delta_gini: burn - g,
        }
    })
    .collect()
}

// ===========================================================================
// Report
// ===========================================================================

/// The full reward-cap report.
#[derive(Clone, Debug)]
pub struct RewardCapReport {
    pub claim1_rows: Vec<Claim1Row>,
    pub claim1_window_probe: Vec<Claim1Row>,
    pub claim1_pool_limited: Vec<Claim1Row>,
    pub throughput_rows: Vec<ThroughputRow>,
    pub granularity_probe: Vec<GranularityRow>,
    /// Gini rows at the busy-chain cap (ρ=20 ⇒ cap=5 BTH), the #351 reference.
    pub gini_rows: Vec<GiniRow>,
    pub gini_burn_baseline: f64,
    pub gini_rho: f64,
}

/// Run the complete reward-cap sweep.
pub fn run_reward_cap_sweep() -> RewardCapReport {
    let gini_rho = 20.0;
    let pop = granular_population();
    let gini_burn_baseline = run_gini(&pop, 10, gini_rho, GiniPolicy::Burn);
    RewardCapReport {
        claim1_rows: claim1_sweep(),
        claim1_window_probe: claim1_window_probe(),
        claim1_pool_limited: claim1_pool_limited_probe(),
        throughput_rows: claim2_throughput(),
        granularity_probe: claim2_granularity_probe(20.0),
        gini_rows: claim2_gini(gini_rho),
        gini_burn_baseline,
        gini_rho,
    }
}

fn policy_label(p: ExcessPolicy) -> &'static str {
    match p {
        ExcessPolicy::Burn => "burn excess",
        ExcessPolicy::CarryForward => "carry-forward",
    }
}

fn gini_policy_label(p: GiniPolicy) -> &'static str {
    match p {
        GiniPolicy::Burn => "Burn (baseline)",
        GiniPolicy::UncappedWindowed => "Uncapped windowed (uniform)",
        GiniPolicy::CappedBurn => "Capped + burn excess",
        GiniPolicy::CappedCarryForward => "Capped + carry-forward",
        GiniPolicy::ValueWeightedAll => "ValueWeighted (incumbent, NOT CT)",
    }
}

/// Render the report as Markdown tables (the §9 doc numbers are generated here,
/// never hand-computed).
pub fn to_markdown(report: &RewardCapReport) -> String {
    let mut s = String::new();

    // --- Setup.
    s.push_str(&format!(
        "### Setup\n\nPath C under test: a **uniform** draw over the last-N-blocks \
         circulation window with the reward-cap invariant `R = min(actual_fee_pool, \
         ρ·base_fee)`, where **ρ counts ALL outputs in the window including the whale's \
         own splits** (consensus cannot tell them apart under CT). base_fee = {:.2} BTH. \
         All numbers are stationary expected values (the per-block draw averages to the \
         weight share over a long run); reused kernels: real `demurrage_charge`, \
         `ClusterFactorCurve`, `calculate_gini`.\n\n",
        base_fee_bth(),
    ));

    // --- Claim 1.
    s.push_str("### Claim 1 — the reward-cap makes splitting net-zero\n\n");
    s.push_str(
        "A whale creates `k` outputs/block (refreshed to stay in-window) against `ρ_o` \
         organic outputs/block. Uniform draw ⇒ whale share `k/(ρ_o+k)`; reward capped at \
         `R = (ρ_o+k)·base_fee` (cap binds; pool ≫ cap). **net/blk = winnings − cost** \
         should be exactly 0 for every `k`. The last two columns are counterfactuals: \
         net if the reward were **uncapped** at a fat pool (`R=20 BTH`), and net if the \
         cap counted **only organic** outputs (`ρ_o·base`, ignoring the whale).\n\n",
    );
    s.push_str(
        "| ρ_o | k | share | R cap (BTH) | winnings/blk | cost/blk | **net/blk** | net if uncapped | net if organic-only cap |\n",
    );
    s.push_str("|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n");
    for r in &report.claim1_rows {
        s.push_str(&format!(
            "| {:.0} | {:.0} | {:.3}% | {:.3} | {:.4} | {:.4} | {:+.2e} | {:+.4} | {:+.4} |\n",
            r.rho_o,
            r.k,
            r.share * 100.0,
            r.reward_capped_bth,
            r.winnings_bth,
            r.cost_bth,
            r.net_bth,
            r.net_uncapped_bth,
            r.net_organic_only_cap_bth,
        ));
    }
    s.push('\n');
    s.push_str(&format!(
        "**Reading it.** The **net/blk** column is 0 (to floating-point epsilon) for every \
         `(ρ_o, k)` — the whale breaks exactly even at any split factor. The mechanism is \
         algebraic: winnings `= k/(ρ_o+k)·(ρ_o+k)·base = k·base = cost`. Inflating ρ by \
         splitting raises the cap `R` by `k·base`, but the whale only collects its share \
         `k/(ρ_o+k)` of that increase — exactly its own fee outlay. The *uncapped* column \
         is positive at the whale's optimal (modest) split — absent the cap the whale \
         profits from a fat {:.0}-BTH pool (it turns negative only when the whale \
         *over*-splits past that optimum, since uncapped there IS a finite best `k`; the \
         capped case, by contrast, is net-zero at EVERY `k`). This confirms the cap is the \
         load-bearing brake. The \
         *organic-only-cap* column is ≤ 0 too — counting the whale's outputs is not a \
         loophole; it is the tightest (break-even) choice consensus can make without \
         distinguishing whale from organic.\n\n",
        FAT_UNCAPPED_REWARD_BTH,
    ));

    // --- N-invariance probe.
    s.push_str(
        "**N-invariance (ρ_o=20, k=500):** the net does not depend on the window \
                size N — the window sets recency and refresh cost, not the capture ledger.\n\n",
    );
    s.push_str("| N (blocks) | share | R cap (BTH) | net/blk (BTH) |\n");
    s.push_str("|---:|---:|---:|---:|\n");
    for r in &report.claim1_window_probe {
        s.push_str(&format!(
            "| {} | {:.3}% | {:.3} | {:+.2e} |\n",
            r.window_blocks,
            r.share * 100.0,
            r.reward_capped_bth,
            r.net_bth,
        ));
    }
    s.push('\n');

    // --- Pool-limited probe.
    s.push_str(
        "**net ≤ 0 is the theorem, net = 0 the ceiling (ρ_o=20, k=500, cap=130 BTH):** \
                when the actual fee pool is *below* the cap, `R = pool` and the whale goes \
                strictly negative — it loses money splitting.\n\n",
    );
    s.push_str(
        "| fee pool (BTH) | R = min(pool,cap) | winnings/blk | cost/blk | net/blk (BTH) |\n",
    );
    s.push_str("|---:|---:|---:|---:|---:|\n");
    for r in &report.claim1_pool_limited {
        s.push_str(&format!(
            "| {:.1} | {:.2} | {:.3} | {:.3} | {:+.4} |\n",
            r.pool_bth, r.reward_capped_bth, r.winnings_bth, r.cost_bth, r.net_bth,
        ));
    }
    s.push('\n');

    // --- Claim 2 throughput.
    s.push_str("### Claim 2 — redistribution throughput under the cap\n\n");
    s.push_str(&format!(
        "10-holder-tier population (the §4.1 tiers), each tier's annual demurrage realised \
         in realistic lumps (whale ≈ 1 big spend/yr, retail ≈ weekly). The per-block cap \
         `ρ·base_fee` is a few BTH, while a single wealthy spend's demurrage fee is \
         hundreds–thousands of BTH. **Throughput** = fraction of total demurrage revenue \
         that actually reaches the lottery. Total revenue = {:.0} BTH/yr; mean inflow = \
         {:.4} BTH/block.\n\n",
        report
            .throughput_rows
            .first()
            .map(|r| r.total_revenue_bth)
            .unwrap_or(0.0),
        report
            .throughput_rows
            .first()
            .map(|r| r.mean_inflow_per_block_bth)
            .unwrap_or(0.0),
    ));
    s.push_str("| ρ (outputs/blk) | cap (BTH/blk) | excess policy | through (BTH/yr) | **throughput** | mean inflow/blk (BTH) |\n");
    s.push_str("|---:|---:|---|---:|---:|---:|\n");
    for r in &report.throughput_rows {
        s.push_str(&format!(
            "| {:.0} | {:.2} | {} | {:.1} | {:.2}% | {:.4} |\n",
            r.rho,
            r.cap_bth,
            policy_label(r.policy),
            r.through_bth,
            r.throughput_frac * 100.0,
            r.mean_inflow_per_block_bth,
        ));
    }
    s.push('\n');
    s.push_str(
        "**Reading it.** Under **burn-the-excess** the capped lottery is a *trickle*: the \
         large demurrage fees that dominate revenue arrive in lumps hundreds of times the \
         per-block cap, so all but `cap/lump` of each is destroyed — single-digit-percent \
         throughput. Under **carry-forward** the excess waits in an accumulating pool and \
         drains over the year's spare block capacity (`cap·BLOCKS_PER_YEAR` ≫ annual \
         inflow), so ~100% of revenue is eventually redistributed — *provided* the mean \
         inflow/block stays below the cap (the stability condition; compare the last two \
         columns). Carry-forward keeps the same per-block payout `R = cap`, so Claim 1's \
         net-zero Sybil-safety is preserved.\n\n",
    );

    // --- Granularity sensitivity.
    s.push_str(
        "**Burn throughput vs spend granularity (ρ=20, cap=5 BTH):** as spending \
                smooths (each fee shrinks below the cap), burn throughput climbs toward \
                100%. The realistic low-granularity end — big holders make few big spends — \
                is exactly where burn throttles hardest.\n\n",
    );
    s.push_str("| granularity ×mult | burn throughput |\n");
    s.push_str("|---:|---:|\n");
    for r in &report.granularity_probe {
        s.push_str(&format!(
            "| {} | {:.2}% |\n",
            r.granularity_mult,
            r.throughput_frac * 100.0,
        ));
    }
    s.push('\n');

    // --- Claim 2 Gini.
    s.push_str(&format!(
        "### Claim 2 — Δgini under the cap (ρ={:.0} ⇒ cap={:.0} BTH/block)\n\n",
        report.gini_rho,
        report.gini_rho * base_fee_bth(),
    ));
    s.push_str(&format!(
        "10-year collect-and-redistribute; burn baseline Gini {:.4}. Δgini = burn − policy \
         (higher = more equalizing). Compare the capped policies to the uncapped windowed \
         lottery (§7.4's +0.1144 yardstick) and the value-weighted incumbent.\n\n",
        report.gini_burn_baseline,
    ));
    s.push_str("| policy | final Gini | Δgini |\n");
    s.push_str("|---|---:|---:|\n");
    for r in &report.gini_rows {
        s.push_str(&format!(
            "| {} | {:.4} | {:+.4} |\n",
            gini_policy_label(r.policy),
            r.final_gini,
            r.delta_gini,
        ));
    }
    s.push('\n');
    s.push_str(
        "**Reading it.** The uncapped windowed uniform lottery and the capped \
         **carry-forward** lottery deliver the same Δgini (carry-forward redistributes the \
         same revenue, only delayed) — and both beat the value-weighted incumbent, whose \
         `value·tilt` weight routes much of the pool back to the wealthy (§4.2). The capped \
         **burn** lottery collapses toward the burn baseline: because most demurrage \
         revenue is destroyed rather than handed to the poor, the equalizing benefit is \
         largely lost (the whale still sheds the demurrage — deflationary — but the poor \
         never receive it). **Burn-the-excess is not an acceptable excess policy; \
         carry-forward is.**\n\n",
    );

    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_is_reproducible() {
        let a = to_markdown(&run_reward_cap_sweep());
        let b = to_markdown(&run_reward_cap_sweep());
        assert_eq!(a, b, "sweep output must be byte-for-byte deterministic");
    }

    #[test]
    fn claim1_net_is_zero_across_the_sweep() {
        // The headline: the reward-cap makes splitting net-zero for every
        // (ρ_o, k) — |net| is floating-point epsilon.
        for r in claim1_sweep() {
            assert!(
                r.net_bth.abs() < 1e-9,
                "whale net must be ~0 under the cap: ρ_o={}, k={}, net={:e}",
                r.rho_o,
                r.k,
                r.net_bth,
            );
        }
    }

    #[test]
    fn claim1_net_is_window_invariant() {
        let probe = claim1_window_probe();
        let first = probe[0].net_bth;
        for r in &probe {
            assert!(
                (r.net_bth - first).abs() < 1e-12,
                "net must not depend on N"
            );
            assert!(r.net_bth.abs() < 1e-9);
        }
    }

    #[test]
    fn claim1_uncapped_whale_profits() {
        // Without the cap the whale profits at its optimal (modest) split —
        // proving the cap is load-bearing. (Uncapped has a finite optimal k:
        // over-splitting past it is unprofitable because cost = k·base grows;
        // the capped case is net-zero at EVERY k, which is the stronger result.)
        let r = claim1_point(20.0, 1.0, 10_000, 1.0e9);
        assert!(
            r.net_uncapped_bth > 0.0,
            "uncapped whale should profit at small k, got {}",
            r.net_uncapped_bth
        );
    }

    #[test]
    fn claim1_pool_limited_is_negative() {
        // When the pool is below the cap, R=pool and the whale strictly loses.
        for r in claim1_pool_limited_probe() {
            // At exactly cap-binding (pool >= cap) net == 0; below, net < 0.
            assert!(
                r.net_bth <= 1e-9,
                "whale net must be <= 0 always: net={}",
                r.net_bth
            );
        }
        // The sub-cap points are strictly negative.
        let below = claim1_point(20.0, 500.0, 10_000, 0.5 * (520.0 * base_fee_bth()));
        assert!(below.net_bth < 0.0);
    }

    #[test]
    fn claim2_burn_is_a_trickle_carryforward_is_full() {
        let pop = granular_population();
        let burn = throughput_point(&pop, 20.0, ExcessPolicy::Burn);
        let cf = throughput_point(&pop, 20.0, ExcessPolicy::CarryForward);
        assert!(
            burn.throughput_frac < 0.15,
            "burn throughput should be a trickle, got {:.3}",
            burn.throughput_frac
        );
        assert!(
            cf.throughput_frac > 0.999,
            "carry-forward should be ~full, got {:.3}",
            cf.throughput_frac
        );
    }

    #[test]
    fn claim2_carryforward_stability_holds_on_busy_chain() {
        // On a busy chain (ρ=20 ⇒ cap=5 BTH) the mean inflow/block is below the
        // cap, so carry-forward is stable (no unbounded backlog).
        let pop = granular_population();
        let row = throughput_point(&pop, 20.0, ExcessPolicy::CarryForward);
        assert!(
            row.mean_inflow_per_block_bth < row.cap_bth,
            "mean inflow {} should be below cap {} for stability",
            row.mean_inflow_per_block_bth,
            row.cap_bth
        );
    }

    #[test]
    fn claim2_burn_gini_collapses_carryforward_survives() {
        let rho = 20.0;
        let rows = claim2_gini(rho);
        let get = |p: GiniPolicy| rows.iter().find(|r| r.policy == p).unwrap().delta_gini;
        let uncapped = get(GiniPolicy::UncappedWindowed);
        let cf = get(GiniPolicy::CappedCarryForward);
        let burn = get(GiniPolicy::CappedBurn);
        assert!(uncapped > 0.0, "uncapped should equalize");
        assert!(
            (cf - uncapped).abs() < 1e-6,
            "carry-forward {cf} should match uncapped {uncapped}"
        );
        assert!(
            burn < cf,
            "burn Δgini {burn} should be worse than carry-forward {cf}"
        );
    }

    #[test]
    fn markdown_has_all_sections() {
        let md = to_markdown(&run_reward_cap_sweep());
        assert!(md.contains("Claim 1 — the reward-cap makes splitting net-zero"));
        assert!(md.contains("Claim 2 — redistribution throughput"));
        assert!(md.contains("Claim 2 — Δgini under the cap"));
    }
}
