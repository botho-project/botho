//! Structural Sybil-brake sweep — the *realized*-capture follow-up to the
//! CT-compatible lottery-selection research (issue #902).
//!
//! # Why this exists
//!
//! The first pass ([`super::lottery_selection_sweep`], memo §1–§6) proved a
//! hard negative: **no value-free WEIGHT can be split-invariant** — the
//! progressive direction and the Sybil hole are the same per-coin term, so a
//! whale that fragments a position captures ~91% of the *ticket share* in a
//! fixed 100-holder population. That measurement, however, deliberately
//! overstates the attack: it holds the denominator fixed at 100 and reads the
//! *weight share*, not the money the whale actually takes home.
//!
//! This module tests the maintainer's follow-up question: do **structural
//! brakes that are NOT weight tilts** — a *last-N-blocks candidate window*, a
//! *fee-denominated eligibility floor*, and a *per-block payout cap* — keep the
//! **realized** Sybil subsidy bounded, so a value-free rule (memo Path A/C)
//! becomes viable after all? All three brakes are value-free / CT-clean: they
//! key only on public block height, public fee, and public age — never the
//! hidden amount.
//!
//! # The modeling fix: realized capture, not weight share
//!
//! Three corrections to the §4.3 measurement, all of which *shrink* the
//! apparent attack and so give the value-free path its best shot:
//!
//! 1. **A real organic denominator.** A live economy creates many fresh outputs
//!    every block (commerce change, received payments). The candidate pool is
//!    `organic_rate × N` outputs deep, not 100. The whale competes against that
//!    whole stream, not a fixed hundred.
//! 2. **The window rolls.** A coin the whale split is only eligible while it is
//!    *within the last N blocks*. To keep tickets the whale must **re-create
//!    its coins forever**, paying `base_fee` per coin per refresh. The attack
//!    has a recurring cost, not a one-time `(K−1)·base_fee`.
//! 3. **Realized money, not tickets.** We integrate the whale's *payout share*
//!    over a long run and compare it to the *fees it burns* to sustain the
//!    attack. If the recurring cost exceeds the captured pool, the attack does
//!    not pay for itself and the "91% ticket share" is economically empty.
//!
//! # The result in one line (derived below, confirmed by the K-scan)
//!
//! Under a uniform-over-eligible value-free rule with a last-N window, the
//! whale's profit-maximising realised capture is
//!
//! ```text
//! s* = max(0, 1 − sqrt(ρ · base_fee / R))
//! ```
//!
//! where `ρ` = organic eligible outputs per block, `base_fee` = per-output
//! floor, `R` = per-block lottery reward. **This is independent of the window
//! size N.** The window changes the *recency semantics* and the whale's
//! refresh *cost scale*, but not the capture ceiling. The brake that actually
//! bounds capture is the ratio `ρ·base_fee / R`: the honest economy's own
//! circulation cost. When `ρ·base_fee ≥ R` the attack is unprofitable at any N
//! and the value-free rule is Sybil-safe **by economics alone**. When the
//! chain is quiet or the reward is fat (`ρ·base_fee ≪ R`), no window rescues
//! it and only a payout cap bounds capture — and the cap is itself
//! cluster-Sybil-evadable under CT. The full verdict is in
//! `docs/research/ct-compatible-lottery-selection.md` §7.
//!
//! # Method
//!
//! Deterministic and RNG-free: the realised capture of a stationary splitting
//! attack is an *expected value* (the per-block lottery draw averages to the
//! weight share over 10k+ blocks by the law of large numbers), so we compute
//! the stationary per-block shares in closed form and confirm the whale's
//! optimal split by an explicit deterministic K-scan. Reuses the shipped
//! kernels: [`crate::demurrage_charge`] (the public-fee-vs-age curve that sets
//! the fee floor's aging cost), the real [`crate::ClusterFactorCurve`] (tier
//! factors), the shared [`super::metrics::calculate_gini`] (honest
//! redistribution), and the constants + [`super::lottery_selection_sweep::
//! public_fee_pico`] proxy from the sibling sweep (NO second implementation).
//! `SEED` is documented for parity; there are no draws.

use crate::{fee_curve::PICO_PER_BTH, ClusterFactorCurve};

use super::{
    lottery_selection_sweep::{public_fee_pico, BASE_FEE_PICO, BLOCKS_PER_YEAR, RATE_BPS},
    metrics::calculate_gini,
};

/// Deterministic seed (documented for parity with the sibling sweeps; the model
/// is expected-value and takes no draws).
pub const SEED: u64 = 0xB07A_0902_2;

/// Base per-output fee in BTH (0.25 BTH), from the sibling sweep's
/// [`BASE_FEE_PICO`]. This is the recurring per-coin cost of sustaining a
/// split.
pub fn base_fee_bth() -> f64 {
    BASE_FEE_PICO as f64 / PICO_PER_BTH as f64
}

// ===========================================================================
// Organic candidate stream
// ===========================================================================

/// One tier of the organic economy that continuously creates fresh outputs
/// (commerce change, received payments). Each block the tier mints
/// `rate_per_block` fresh outputs of `value_bth`, all at the tier's cluster
/// factor. These are the honest denominator the whale must out-ticket.
#[derive(Clone, Copy, Debug)]
pub struct OrganicTier {
    pub label: &'static str,
    /// Fresh outputs this tier creates per block (may be fractional — an
    /// expected rate).
    pub rate_per_block: f64,
    /// Value of each fresh output, in BTH.
    pub value_bth: u64,
}

/// A "normal" live-economy organic stream: retail commerce dominates the output
/// count, with progressively rarer larger outputs. Total ≈ 20 fresh
/// outputs/block (≈ 10 tx/block × 2 outputs), the shape of an active chain.
pub fn organic_stream_normal() -> Vec<OrganicTier> {
    vec![
        OrganicTier {
            label: "retail commerce",
            rate_per_block: 18.0,
            value_bth: 100,
        },
        OrganicTier {
            label: "merchant / mid",
            rate_per_block: 1.8,
            value_bth: 2_000,
        },
        OrganicTier {
            label: "wealthy (occasional)",
            rate_per_block: 0.2,
            value_bth: 20_000,
        },
    ]
}

/// Total organic fresh-output rate ρ (outputs/block) for a stream.
pub fn organic_rate(stream: &[OrganicTier]) -> f64 {
    stream.iter().map(|t| t.rate_per_block).sum()
}

/// The whale's cluster factor (10M-BTH cluster, ≈ 5.745×), matching the sibling
/// sweep's attacker. Fresh split coins are given this factor — the whale's best
/// case for clearing a fee floor (higher factor ⇒ faster demurrage accrual ⇒
/// clears the floor sooner).
pub fn whale_factor_scaled() -> u64 {
    ClusterFactorCurve::default_params().factor(10_000_000u128 * PICO_PER_BTH)
}

// ===========================================================================
// Fee-floor aging: how long must a coin age before its public fee ≥ F?
// ===========================================================================

/// Smallest coin age (in blocks) at which a `value_bth` coin at `factor_scaled`
/// has public fee ≥ `floor_pico`, or `None` if it never clears within a
/// generous 200-year cap. The public fee is
/// `base_fee + demurrage(value, factor, age)` (the sibling sweep's
/// [`public_fee_pico`]), which is monotone non-decreasing in age, so a binary
/// search is exact.
///
/// This is the crux of the fee-floor brake under CT: a *fresh* coin (age ≈ 0)
/// has fee ≈ `base_fee` **regardless of its hidden value**, because demurrage
/// has not accrued. The fee only becomes a value proxy after long holding — so
/// a floor `F > base_fee` can only be cleared by *aged* coins, which is in
/// direct tension with a last-N-blocks *recency* window.
pub fn age_to_clear_floor(value_pico: u128, factor_scaled: u64, floor_pico: u128) -> Option<u64> {
    // Fresh coins already clear a floor at/below the base fee.
    if public_fee_pico(value_pico, factor_scaled, 0) >= floor_pico {
        return Some(0);
    }
    let cap = BLOCKS_PER_YEAR.saturating_mul(200);
    if public_fee_pico(value_pico, factor_scaled, cap) < floor_pico {
        return None; // value too small to ever clear the floor at a sane age
    }
    let (mut lo, mut hi) = (0u64, cap);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if public_fee_pico(value_pico, factor_scaled, mid) >= floor_pico {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    Some(lo)
}

// ===========================================================================
// Track A — realized capture vs the last-N window (the primary experiment)
// ===========================================================================

/// A regime of the economy: how fat the per-block reward is relative to organic
/// circulation. The capture ceiling depends only on `ρ·base_fee / R`, so these
/// three points span the outcome space.
#[derive(Clone, Copy, Debug)]
pub struct Regime {
    pub label: &'static str,
    /// Organic fresh-output rate ρ (outputs/block).
    pub organic_rate: f64,
    /// Per-block lottery reward R, in BTH (emission + demurrage pool routed to
    /// the lottery).
    pub reward_bth: f64,
}

/// The reward regimes swept. `reward ≈ 3.8 BTH/block` is the #351 steady state
/// (≈2% emission on ~600M supply ≈ 1.9 BTH/block, plus a similar demurrage
/// pool); the quiet-chain and fat-reward points bracket it.
pub fn regimes() -> Vec<Regime> {
    vec![
        Regime {
            label: "busy chain, steady reward",
            organic_rate: 20.0,
            reward_bth: 3.8,
        },
        Regime {
            label: "quiet chain, steady reward",
            organic_rate: 2.0,
            reward_bth: 3.8,
        },
        Regime {
            label: "busy chain, fat reward (bootstrap)",
            organic_rate: 20.0,
            reward_bth: 20.0,
        },
    ]
}

/// Window sizes swept, in blocks (5 s reference): ~8 min, ~1.4 h, ~14 h, ~5.8
/// d.
pub fn window_sizes() -> Vec<u64> {
    vec![100, 1_000, 10_000, 100_000]
}

/// Outcome of the whale's profit-maximising split against one (regime, N,
/// floor, cap) configuration.
#[derive(Clone, Debug)]
pub struct CaptureRow {
    pub regime: &'static str,
    pub window_blocks: u64,
    /// Fee floor F in BTH (0 = no floor).
    pub floor_bth: f64,
    /// Per-block per-owner payout cap as a fraction of the reward (1.0 = no
    /// cap).
    pub cap_frac: f64,
    /// The whale's profit-maximising eligible split count K* (continuously
    /// maintained; 0 = attack unprofitable, whale does not split).
    pub optimal_k: u64,
    /// Value of each split piece at K*, in BTH.
    pub piece_value_bth: f64,
    /// Realised whale capture: fraction of the total lottery payout the whale
    /// takes home over a long run, after the payout cap.
    pub realized_capture: f64,
    /// Recurring attacker cost per block, in BTH (fees burned to keep K* fresh
    /// eligible coins).
    pub cost_per_block_bth: f64,
    /// Attacker cost ÷ pool it captures. `< 1` ⇒ the attack pays for itself;
    /// `≥ 1` ⇒ the whale burns more in fees than it wins.
    pub cost_over_captured: f64,
    /// Whale net profit per block, in BTH (captured − cost). `≤ 0` ⇒ deterred.
    pub net_profit_bth: f64,
}

/// Organic eligible ticket count (Uniform weight = count of in-window,
/// floor-clearing organic outputs) at stationarity. A tier producing
/// `rate` outputs/block holds `rate·N` in-window; of those, the ones with age
/// ≥ `a_v` (cleared the floor) are eligible ⇒ `rate·max(0, N − a_v)`.
fn organic_eligible_count(stream: &[OrganicTier], n: u64, floor_pico: u128) -> f64 {
    let wf = whale_factor_scaled();
    stream
        .iter()
        .map(|t| {
            // Organic tiers clear the floor per their own value; use the whale
            // factor as a uniform reference so the floor's aging cost is
            // compared on equal footing (organic factors only lower the accrual,
            // i.e. make the floor MORE exclusionary of the poor — the whale-
            // factor choice is the floor's best case).
            let a_v = age_to_clear_floor(t.value_bth as u128 * PICO_PER_BTH, wf, floor_pico);
            match a_v {
                Some(a) if a < n => t.rate_per_block * (n - a) as f64,
                _ => 0.0,
            }
        })
        .sum()
}

/// Solve the whale's optimal split against a fixed organic denominator.
///
/// The whale continuously maintains `K` eligible coins (staggered births so a
/// coin is replaced as it ages out of the window at N, or as it must re-age to
/// clear the floor). With `org` organic eligible tickets and a per-block reward
/// `R`:
///
/// - capture(K)      = min(cap, K / (K + org))
/// - cost/block(K)   = base_fee · K / eligible_lifetime
/// - net(K)          = capture(K)·R − cost/block(K)
///
/// where `eligible_lifetime = N − a_piece` is how long each coin stays eligible
/// (window minus the aging needed to clear the floor). We scan a dense
/// geometric K-grid (deterministic) and return the argmax of net profit,
/// bounded by the whale's total value / a 1-BTH dust piece.
#[allow(clippy::too_many_arguments)]
fn optimize_split(
    whale_value_bth: u64,
    factor_scaled: u64,
    n: u64,
    floor_pico: u128,
    org_eligible: f64,
    reward_bth: f64,
    cap_frac: f64,
) -> (u64, f64, f64, f64) {
    // Returns (optimal_k, realized_capture, cost_per_block_bth, net_profit_bth).
    let base = base_fee_bth();
    let mut best = (0u64, 0.0f64, 0.0f64, 0.0f64); // k, capture, cost, net(=0 baseline: no attack)

    // Two independent caps on how far the whale splits:
    //  - dust cap: a piece must stay above the min UTXO value (1 microBTH), so K ≤
    //    value / dust. This is essentially non-binding — the real brake is:
    //  - cost cap: once `cost/block = base·K/N` exceeds the whole reward, net
    //    profit is negative for any capture ≤ 1, so K beyond `R·N/base` is
    //    pointless. This keeps the grid tight and is what actually bounds K.
    let dust_bth = 1e-6_f64;
    let value_cap = (whale_value_bth as f64 / dust_bth) as u64;
    let cost_cap = ((reward_bth / base) * n as f64 * 2.0).ceil() as u64;
    let k_max = value_cap.min(cost_cap).max(1);

    // Dense geometric grid (ratio ≈ 1.12) so the profit-max K is resolved to a
    // couple of percent — coarse decade grids overshoot the optimum.
    let mut k_f = 1.0_f64;
    let mut prev = 0u64;
    loop {
        let k = k_f.round() as u64;
        if k > k_max {
            break;
        }
        if k != prev {
            prev = k;
            let piece_bth = whale_value_bth as f64 / k as f64;
            if piece_bth >= dust_bth {
                let piece_pico = (piece_bth * PICO_PER_BTH as f64) as u128;
                let a_piece = age_to_clear_floor(piece_pico, factor_scaled, floor_pico);
                if let Some(a) = a_piece {
                    if a < n {
                        let eligible_lifetime = (n - a) as f64;
                        let capture = (k as f64 / (k as f64 + org_eligible)).min(cap_frac);
                        let cost = base * k as f64 / eligible_lifetime;
                        let net = capture * reward_bth - cost;
                        if net > best.3 {
                            best = (k, capture, cost, net);
                        }
                    }
                }
            }
        }
        k_f *= 1.12;
    }
    best
}

/// Run the capture experiment for one (regime, window, floor, cap) point.
#[allow(clippy::too_many_arguments)]
pub fn capture_point(
    regime: Regime,
    stream: &[OrganicTier],
    whale_value_bth: u64,
    n: u64,
    floor_bth: f64,
    cap_frac: f64,
) -> CaptureRow {
    let floor_pico = (floor_bth * PICO_PER_BTH as f64) as u128;
    // Rescale the organic stream so its total rate matches the regime's ρ while
    // keeping the tier value-mix.
    let base_rate = organic_rate(stream);
    let scale = if base_rate > 0.0 {
        regime.organic_rate / base_rate
    } else {
        0.0
    };
    let scaled: Vec<OrganicTier> = stream
        .iter()
        .map(|t| OrganicTier {
            rate_per_block: t.rate_per_block * scale,
            ..*t
        })
        .collect();
    let org_eligible = organic_eligible_count(&scaled, n, floor_pico);

    let (k, capture, cost, net) = optimize_split(
        whale_value_bth,
        whale_factor_scaled(),
        n,
        floor_pico,
        org_eligible,
        regime.reward_bth,
        cap_frac,
    );

    let captured_bth = capture * regime.reward_bth;
    let cost_over_captured = if captured_bth > 0.0 {
        cost / captured_bth
    } else {
        f64::INFINITY
    };
    CaptureRow {
        regime: regime.label,
        window_blocks: n,
        floor_bth,
        cap_frac,
        optimal_k: k,
        piece_value_bth: if k > 0 {
            whale_value_bth as f64 / k as f64
        } else {
            whale_value_bth as f64
        },
        realized_capture: capture,
        cost_per_block_bth: cost,
        cost_over_captured,
        net_profit_bth: net,
    }
}

// ===========================================================================
// Track B — honest redistribution under a windowed (circulated-only) lottery
// ===========================================================================

/// A holder tier for the honest-redistribution track. `circulated` = does this
/// tier transact often enough to keep a fresh output inside the last-N window
/// for free (commerce), vs an idle hoarder that would fall out of the window.
#[derive(Clone, Copy, Debug)]
pub struct HonestTier {
    pub label: &'static str,
    pub count: usize,
    pub wealth_bth: u64,
    pub cluster_wealth_bth: u64,
    pub circulated: bool,
}

/// The honest population (mirrors the sibling sweep's tiers): a large poor
/// commerce tier (circulated) plus idle wealthy hoarders (not circulated).
pub fn honest_tiers() -> Vec<HonestTier> {
    vec![
        HonestTier {
            label: "background (poor, commerce)",
            count: 90,
            wealth_bth: 1_000,
            cluster_wealth_bth: 1_000,
            circulated: true,
        },
        HonestTier {
            label: "50k-cluster wealthy (idle)",
            count: 4,
            wealth_bth: 5_000,
            cluster_wealth_bth: 50_000,
            circulated: false,
        },
        HonestTier {
            label: "500k-cluster wealthy (idle)",
            count: 3,
            wealth_bth: 50_000,
            cluster_wealth_bth: 500_000,
            circulated: false,
        },
        HonestTier {
            label: "10M-cluster whale (idle)",
            count: 3,
            wealth_bth: 100_000,
            cluster_wealth_bth: 10_000_000,
            circulated: false,
        },
    ]
}

/// Which eligibility policy the honest-redistribution loop applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HonestPolicy {
    /// Burn baseline: collect demurrage, redistribute nothing.
    Burn,
    /// Unrestricted uniform lottery: every holder gets one ticket.
    UniformAll,
    /// Last-N window: only circulated (recently-moved) holders are eligible.
    WindowedCirculated,
}

/// Result of the honest-redistribution track for one policy.
#[derive(Clone, Debug)]
pub struct HonestRow {
    pub policy: HonestPolicy,
    pub final_gini: f64,
    /// Δgini = burn_gini − final_gini (higher = more equalizing).
    pub delta_gini: f64,
}

/// Run a 10-year collect-and-redistribute loop under a policy and return the
/// final Gini. Every holder pays ordinary demurrage into a pool each year;
/// the pool is redistributed uniformly among the *eligible* set (all, or only
/// circulated), or burned. Reuses [`crate::demurrage_charge`] and
/// [`calculate_gini`]; expected-value, no RNG.
fn run_honest(tiers: &[HonestTier], years: u64, policy: HonestPolicy) -> f64 {
    let curve = ClusterFactorCurve::default_params();
    // Flatten to per-holder wealth/factor/circulated.
    let mut wealth: Vec<u128> = Vec::new();
    let mut factor: Vec<u64> = Vec::new();
    let mut circ: Vec<bool> = Vec::new();
    for t in tiers {
        let f = curve.factor(t.cluster_wealth_bth as u128 * PICO_PER_BTH);
        for _ in 0..t.count {
            wealth.push(t.wealth_bth as u128 * PICO_PER_BTH);
            factor.push(f);
            circ.push(t.circulated);
        }
    }

    let mut carry: u128 = 0;
    for _ in 0..years {
        let mut pool = carry;
        for i in 0..wealth.len() {
            let v = u64::try_from(wealth[i]).unwrap_or(u64::MAX);
            let charge = (demurrage_charge_pico(v, factor[i]) as u128).min(wealth[i]);
            wealth[i] -= charge;
            pool += charge;
        }
        match policy {
            HonestPolicy::Burn => {
                carry = 0;
            }
            HonestPolicy::UniformAll | HonestPolicy::WindowedCirculated => {
                let eligible: Vec<usize> = (0..wealth.len())
                    .filter(|&i| policy == HonestPolicy::UniformAll || circ[i])
                    .collect();
                if eligible.is_empty() {
                    carry = pool;
                    continue;
                }
                let share = pool / eligible.len() as u128;
                for &i in &eligible {
                    wealth[i] += share;
                }
                carry = pool - share * eligible.len() as u128;
            }
        }
    }

    let final_wealths: Vec<u64> = wealth
        .iter()
        .map(|&w| u64::try_from(w / PICO_PER_BTH).unwrap_or(u64::MAX))
        .collect();
    calculate_gini(&final_wealths)
}

/// One year of ordinary demurrage for a holding, in picocredits.
fn demurrage_charge_pico(value_bth_units: u64, factor_scaled: u64) -> u64 {
    crate::demurrage_charge(
        value_bth_units,
        factor_scaled,
        BLOCKS_PER_YEAR,
        RATE_BPS,
        BLOCKS_PER_YEAR,
    )
}

/// Run all three honest policies and score Δgini vs the burn baseline.
pub fn honest_sweep(tiers: &[HonestTier], years: u64) -> Vec<HonestRow> {
    let burn = run_honest(tiers, years, HonestPolicy::Burn);
    [HonestPolicy::UniformAll, HonestPolicy::WindowedCirculated]
        .into_iter()
        .map(|policy| {
            let g = run_honest(tiers, years, policy);
            HonestRow {
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

/// The full structural-brake report.
#[derive(Clone, Debug)]
pub struct SybilBrakeReport {
    pub whale_value_bth: u64,
    /// Track A window sweep (no floor, no cap): the N-invariance demonstration.
    pub window_rows: Vec<CaptureRow>,
    /// Track A payout-cap sweep (busy/quiet regimes, N=10k, no floor).
    pub cap_rows: Vec<CaptureRow>,
    /// Track A window+floor combo (busy regime, N=10k).
    pub combo_rows: Vec<CaptureRow>,
    /// Fee-floor aging table: age-to-clear F for representative coin values.
    pub floor_aging: Vec<FloorAgingRow>,
    /// Honest redistribution Δgini.
    pub honest_rows: Vec<HonestRow>,
    pub honest_burn_gini: f64,
    pub honest_initial_gini: f64,
}

/// One cell of the fee-floor aging table.
#[derive(Clone, Debug)]
pub struct FloorAgingRow {
    pub floor_bth: f64,
    pub value_bth: u64,
    /// Age (blocks) to clear the floor, or None (never within 200 yr).
    pub age_blocks: Option<u64>,
}

/// Representative coin values (BTH) for the fee-floor aging table.
pub fn floor_probe_values() -> Vec<u64> {
    vec![100, 1_000, 10_000, 100_000]
}

/// Fee floors (BTH) probed. `base_fee = 0.25`, so `0.25` = "no real floor".
pub fn floor_levels() -> Vec<f64> {
    vec![0.25, 0.5, 1.0]
}

/// Run the complete structural-brake sweep with the default configuration.
pub fn run_sybil_brake_sweep() -> SybilBrakeReport {
    let stream = organic_stream_normal();
    let whale_value_bth = 100_000u64; // matches the sibling sweep's attacker

    // Track A.1 — window sweep, no floor, no cap: capture ceiling vs N.
    let mut window_rows = Vec::new();
    for regime in regimes() {
        for n in window_sizes() {
            window_rows.push(capture_point(regime, &stream, whale_value_bth, n, 0.0, 1.0));
        }
    }

    // Track A.2 — payout cap sweep at N=10k, no floor (busy + quiet regimes).
    let mut cap_rows = Vec::new();
    let cap_regimes = [regimes()[0], regimes()[1]];
    for regime in cap_regimes {
        for cap in [1.0, 0.25, 0.10, 0.05] {
            cap_rows.push(capture_point(
                regime,
                &stream,
                whale_value_bth,
                10_000,
                0.0,
                cap,
            ));
        }
    }

    // Track A.3 — window+floor combo at N=10k, quiet regime (where the attack
    // pays, so the floor has something to bite).
    let mut combo_rows = Vec::new();
    let quiet = regimes()[1];
    for floor in [0.0, 0.5, 1.0] {
        combo_rows.push(capture_point(
            quiet,
            &stream,
            whale_value_bth,
            10_000,
            floor,
            1.0,
        ));
    }

    // Fee-floor aging table.
    let wf = whale_factor_scaled();
    let mut floor_aging = Vec::new();
    for &floor in &floor_levels() {
        let floor_pico = (floor * PICO_PER_BTH as f64) as u128;
        for &v in &floor_probe_values() {
            floor_aging.push(FloorAgingRow {
                floor_bth: floor,
                value_bth: v,
                age_blocks: age_to_clear_floor(v as u128 * PICO_PER_BTH, wf, floor_pico),
            });
        }
    }

    // Honest redistribution.
    let tiers = honest_tiers();
    let years = 10;
    let honest_rows = honest_sweep(&tiers, years);
    let honest_burn_gini = run_honest(&tiers, years, HonestPolicy::Burn);
    let curve = ClusterFactorCurve::default_params();
    let initial: Vec<u64> = tiers
        .iter()
        .flat_map(|t| std::iter::repeat_n(t.wealth_bth, t.count))
        .collect();
    let _ = &curve;
    let honest_initial_gini = calculate_gini(&initial);

    SybilBrakeReport {
        whale_value_bth,
        window_rows,
        cap_rows,
        combo_rows,
        floor_aging,
        honest_rows,
        honest_burn_gini,
        honest_initial_gini,
    }
}

fn policy_label(p: HonestPolicy) -> &'static str {
    match p {
        HonestPolicy::Burn => "Burn (baseline)",
        HonestPolicy::UniformAll => "Uniform (all holders)",
        HonestPolicy::WindowedCirculated => "Windowed (circulated only)",
    }
}

/// Render the report as Markdown tables (the §7 doc numbers are generated
/// here).
pub fn to_markdown(report: &SybilBrakeReport) -> String {
    let mut s = String::new();

    // --- Setup.
    s.push_str(&format!(
        "### Setup\n\nAttacker: a {}-BTH whale (factor {:.3}×) that fragments its \
         position and continuously refreshes the pieces to keep them inside the \
         last-N window. Per-output base fee {:.2} BTH (the recurring cost of one \
         fresh ticket). Organic stream (a live economy's fresh outputs): retail \
         18/blk @100 BTH, mid 1.8/blk @2k, wealthy 0.2/blk @20k (≈20 outputs/blk, \
         rescaled to each regime's ρ). Rule under test: **uniform weight over \
         eligible candidates** — the representative value-free rule (all §4 \
         value-free rules share the same per-ticket Sybil hole). All numbers are \
         stationary expected values (the per-block draw averages to the weight \
         share over 10k+ blocks); confirmed by a deterministic K-scan.\n\n",
        report.whale_value_bth,
        whale_factor_scaled() as f64 / ClusterFactorCurve::FACTOR_SCALE as f64,
        base_fee_bth(),
    ));

    // --- Track A.1 window sweep.
    s.push_str("### Track A.1 — realized capture vs the last-N window (no floor, no cap)\n\n");
    s.push_str(
        "The whale picks its profit-maximising split K\\*. **Realized capture** = share of \
         total payout it takes home over a long run. **cost/captured** = recurring fees \
         burned ÷ pool captured (`<1` ⇒ the attack pays for itself). Note the capture is \
         **flat across N** within a regime — the window does not change the ceiling.\n\n",
    );
    s.push_str(
        "| regime (ρ, R) | N (blocks) | K\\* | piece (BTH) | realized capture | cost/blk (BTH) | cost/captured | net/blk (BTH) |\n",
    );
    s.push_str("|---|---:|---:|---:|---:|---:|---:|---:|\n");
    for r in &report.window_rows {
        s.push_str(&format!(
            "| {} | {} | {} | {} | {:.2}% | {:.4} | {} | {:+.4} |\n",
            r.regime,
            r.window_blocks,
            r.optimal_k,
            fmt_piece(r),
            r.realized_capture * 100.0,
            r.cost_per_block_bth,
            fmt_ratio(r.cost_over_captured),
            r.net_profit_bth,
        ));
    }
    s.push('\n');
    s.push_str(
        "**Closed form (derived in the module header, matches the K-scan):** the \
         profit-maximising realized capture is `s* = max(0, 1 − √(ρ·base_fee / R))`, \
         **independent of N**. The brake is the ratio `ρ·base_fee / R` — the honest \
         economy's own per-block circulation cost vs the reward. When `ρ·base_fee ≥ R` \
         (busy chain, steady reward) the attack is unprofitable at every window size and \
         K\\* = 0. When the chain is quiet or the reward is fat, the whale captures a \
         positive share that **no window shrinks**.\n\n",
    );

    // --- Track A.2 payout cap.
    s.push_str("### Track A.2 — payout cap (N=10k, no floor)\n\n");
    s.push_str(
        "A per-block per-owner payout cap (as a fraction of the reward) directly bounds \
         realized capture to the cap — but **only if the cap can be keyed to an \
         unforgeable identity**. Under CT the whale's fresh coins share no tags, so they \
         form distinct stealth clusters: a per-cluster cap is evaded by splitting across \
         unlinkable clusters (the same wall the weight hits). The cap column below is the \
         *best case* (cap enforced per whole attacker); the CT-realistic case is the \
         uncapped row.\n\n",
    );
    s.push_str(
        "| regime (ρ, R) | cap | K\\* | realized capture | cost/captured | net/blk (BTH) |\n",
    );
    s.push_str("|---|---:|---:|---:|---:|---:|\n");
    for r in &report.cap_rows {
        s.push_str(&format!(
            "| {} | {} | {} | {:.2}% | {} | {:+.4} |\n",
            r.regime,
            if r.cap_frac >= 1.0 {
                "none".to_string()
            } else {
                format!("{:.0}%", r.cap_frac * 100.0)
            },
            r.optimal_k,
            r.realized_capture * 100.0,
            fmt_ratio(r.cost_over_captured),
            r.net_profit_bth,
        ));
    }
    s.push('\n');

    // --- Fee-floor aging.
    s.push_str("### Track A.3a — the fee floor's aging tax (why F fights the window)\n\n");
    s.push_str(&format!(
        "Blocks a coin must age before its public fee clears floor F (at the whale \
         factor {:.3}×, the floor's best case). A *fresh* coin has fee ≈ base ({:.2} BTH) \
         **regardless of its hidden value**, so any F above base can only be cleared by an \
         *aged* coin — but the last-N window requires *recency*. For reference, N=10k \
         blocks ≈ 14 h; N=100k ≈ 5.8 d; 1 year = {} blocks.\n\n",
        whale_factor_scaled() as f64 / ClusterFactorCurve::FACTOR_SCALE as f64,
        base_fee_bth(),
        BLOCKS_PER_YEAR,
    ));
    s.push_str("| floor F (BTH) | 100 BTH coin | 1k | 10k | 100k |\n");
    s.push_str("|---:|---:|---:|---:|---:|\n");
    for &floor in &floor_levels() {
        let cell = |v: u64| {
            report
                .floor_aging
                .iter()
                .find(|r| (r.floor_bth - floor).abs() < 1e-9 && r.value_bth == v)
                .map(|r| match r.age_blocks {
                    Some(a) => format!("{a}"),
                    None => "never".to_string(),
                })
                .unwrap_or_else(|| "-".to_string())
        };
        s.push_str(&format!(
            "| {:.2} | {} | {} | {} | {} |\n",
            floor,
            cell(100),
            cell(1_000),
            cell(10_000),
            cell(100_000),
        ));
    }
    s.push('\n');
    s.push_str(
        "**Reading it:** a floor set high enough to force the whale into large pieces \
         (fewer eligible tickets) *also* pushes the aging-to-clear beyond a short window — \
         excluding the very poor (100-BTH) coins the lottery is meant to reach, since they \
         can never clear the floor inside the window. The floor is regressive under a \
         recency window: it disenfranchises the poor faster than it caps the whale.\n\n",
    );

    // --- Combo.
    s.push_str("### Track A.3b — window + fee-floor combo (quiet regime, N=10k)\n\n");
    s.push_str(
        "The one regime where the attack pays (quiet chain), now with a floor added. The \
         floor forces the whale into larger pieces and shortens each coin's eligible life \
         (it must age to clear F, then falls out at N), raising cost — but it cannot drive \
         capture to zero without also excluding the organic poor (see A.3a).\n\n",
    );
    s.push_str("| floor F (BTH) | K\\* | piece (BTH) | realized capture | cost/captured | net/blk (BTH) |\n");
    s.push_str("|---:|---:|---:|---:|---:|---:|\n");
    for r in &report.combo_rows {
        s.push_str(&format!(
            "| {:.2} | {} | {} | {:.2}% | {} | {:+.4} |\n",
            r.floor_bth,
            r.optimal_k,
            fmt_piece(r),
            r.realized_capture * 100.0,
            fmt_ratio(r.cost_over_captured),
            r.net_profit_bth,
        ));
    }
    s.push('\n');
    s.push_str(
        "**Reading it:** the floor is not merely weak, it is *non-monotone and \
         counterproductive*. At F=0.50 it caps the whale (large pieces ⇒ few tickets) but \
         only by excluding every organic tier that cannot clear the floor inside the \
         window. Pushed to F=1.00 it excludes the organic poor *and* the honest wealthy — \
         no in-window coin clears it except the whale's own large, aged, consolidated \
         position, which then wins **100%** of a pool nobody else can enter. A fee floor \
         under a recency window inverts into a whale subsidy.\n\n",
    );

    // --- Honest redistribution.
    s.push_str("### Track B — honest redistribution under a windowed lottery\n\n");
    s.push_str(&format!(
        "10-year collect-and-redistribute over the honest population (initial Gini \
         {:.4}; burn baseline Gini {:.4}). Does restricting the lottery to *circulated* \
         (recently-moved) holders starve honest redistribution? Δgini = burn − policy \
         (higher = more equalizing).\n\n",
        report.honest_initial_gini, report.honest_burn_gini,
    ));
    s.push_str("| policy | final Gini | Δgini |\n");
    s.push_str("|---|---:|---:|\n");
    for r in &report.honest_rows {
        s.push_str(&format!(
            "| {} | {:.4} | {:+.4} |\n",
            policy_label(r.policy),
            r.final_gini,
            r.delta_gini,
        ));
    }
    s.push('\n');
    s.push_str(
        "**Reading it:** the last-N window does **not** starve redistribution — it \
         concentrates the pool on actively-circulating holders (here the poor commerce \
         tier), which are exactly the intended beneficiaries; idle whales self-exclude by \
         not transacting. Windowed redistribution is at least as equalizing as the \
         unrestricted uniform lottery. The caveat is idle *poor* holders, who are also \
         excluded — acceptable only because demurrage already redistributes the stock \
         (per `cluster-tilted-redistribution.md`).\n\n",
    );

    s
}

fn fmt_piece(r: &CaptureRow) -> String {
    if r.optimal_k == 0 {
        "—".to_string()
    } else {
        format!("{:.0}", r.piece_value_bth)
    }
}

fn fmt_ratio(x: f64) -> String {
    if x.is_infinite() {
        "n/a".to_string()
    } else {
        format!("{x:.2}×")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_is_reproducible() {
        let a = to_markdown(&run_sybil_brake_sweep());
        let b = to_markdown(&run_sybil_brake_sweep());
        assert_eq!(a, b, "sweep output must be byte-for-byte deterministic");
    }

    #[test]
    fn capture_is_window_invariant() {
        // The headline: within a regime, realized capture does not depend on N.
        let stream = organic_stream_normal();
        for regime in regimes() {
            let caps: Vec<f64> = window_sizes()
                .into_iter()
                .map(|n| capture_point(regime, &stream, 100_000, n, 0.0, 1.0).realized_capture)
                .collect();
            let first = caps[0];
            for c in &caps {
                assert!(
                    (c - first).abs() < 0.02,
                    "capture must be ~N-invariant for {}: {caps:?}",
                    regime.label
                );
            }
        }
    }

    #[test]
    fn busy_steady_chain_deters_the_attack() {
        // When ρ·base_fee ≥ R the attack is unprofitable at every N (K*=0).
        let stream = organic_stream_normal();
        let busy = regimes()[0]; // ρ=20, R=3.8 ⇒ ρ·base=5 > 3.8
        assert!(busy.organic_rate * base_fee_bth() >= busy.reward_bth);
        for n in window_sizes() {
            let row = capture_point(busy, &stream, 100_000, n, 0.0, 1.0);
            assert_eq!(row.optimal_k, 0, "attack should be deterred at N={n}");
            assert!(row.realized_capture < 1e-9);
        }
    }

    #[test]
    fn quiet_or_fat_chain_lets_the_whale_capture() {
        // The negative half: a quiet chain (thin organic denominator) lets the
        // whale profitably capture a positive, N-invariant share.
        let stream = organic_stream_normal();
        let quiet = regimes()[1]; // ρ=2, R=3.8 ⇒ ρ·base=0.5 < 3.8
        let row = capture_point(quiet, &stream, 100_000, 10_000, 0.0, 1.0);
        assert!(row.optimal_k > 0);
        assert!(
            row.realized_capture > 0.2,
            "quiet chain should let the whale capture materially, got {:.3}",
            row.realized_capture
        );
        assert!(row.net_profit_bth > 0.0);
    }

    #[test]
    fn closed_form_matches_kscan() {
        // s* = 1 − sqrt(ρ·base/R), checked against the optimizer for a case
        // where the attack pays.
        let stream = organic_stream_normal();
        let quiet = regimes()[1];
        let row = capture_point(quiet, &stream, 100_000, 10_000, 0.0, 1.0);
        let predicted =
            (1.0 - (quiet.organic_rate * base_fee_bth() / quiet.reward_bth).sqrt()).max(0.0);
        assert!(
            (row.realized_capture - predicted).abs() < 0.05,
            "K-scan {:.3} should match closed form {:.3}",
            row.realized_capture,
            predicted
        );
    }

    #[test]
    fn payout_cap_bounds_capture() {
        // A per-attacker payout cap bounds realized capture to the cap (best
        // case; CT-realistically evadable via unlinkable clusters).
        let stream = organic_stream_normal();
        let quiet = regimes()[1];
        let uncapped = capture_point(quiet, &stream, 100_000, 10_000, 0.0, 1.0);
        let capped = capture_point(quiet, &stream, 100_000, 10_000, 0.0, 0.10);
        assert!(uncapped.realized_capture > 0.10);
        assert!(capped.realized_capture <= 0.10 + 1e-9);
    }

    #[test]
    fn fee_floor_ages_out_the_poor() {
        // A fresh coin clears only the base fee; a floor above base needs aging,
        // and a 100-BTH coin needs far longer than a 10k-block window.
        let wf = whale_factor_scaled();
        let floor_pico = (0.5 * PICO_PER_BTH as f64) as u128;
        let a_poor = age_to_clear_floor(100 * PICO_PER_BTH, wf, floor_pico).unwrap();
        assert!(
            a_poor > 10_000,
            "a 100-BTH coin should not clear a 0.5-BTH floor inside a 10k window, got {a_poor}"
        );
        // A fresh coin at/below the base fee is eligible immediately.
        let base_floor = BASE_FEE_PICO;
        assert_eq!(
            age_to_clear_floor(100 * PICO_PER_BTH, wf, base_floor),
            Some(0)
        );
    }

    #[test]
    fn windowed_redistribution_does_not_starve() {
        // Restricting to circulated holders is at least as equalizing as the
        // unrestricted uniform lottery (idle whales self-exclude).
        let tiers = honest_tiers();
        let rows = honest_sweep(&tiers, 10);
        let uni = rows
            .iter()
            .find(|r| r.policy == HonestPolicy::UniformAll)
            .unwrap();
        let win = rows
            .iter()
            .find(|r| r.policy == HonestPolicy::WindowedCirculated)
            .unwrap();
        assert!(uni.delta_gini > 0.0);
        assert!(
            win.delta_gini >= uni.delta_gini - 1e-9,
            "windowed {:.4} should be >= uniform {:.4}",
            win.delta_gini,
            uni.delta_gini
        );
    }
}
