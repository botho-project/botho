//! CT provenance-factor calibration sweep — the design-research gate for
//! **decision D2** of issue #902 (confidential amounts × the cluster-factor
//! core).
//!
//! # The problem this sweep exists to answer
//!
//! Under ADR 0006 confidential amounts, the cluster-factor curve's input —
//! **cluster wealth** `W(c) = Σ v·weight(c)` — is the one quantity that
//! genuinely goes hidden (`docs/research/ct-economics-gadgets.md` §4). Two
//! resolutions are on the table:
//!
//! - **Option A** — an octave-bucket range proof over a *committed* cluster-
//!   wealth aggregate (exact-ish, ~1–2 KB/spend, real crypto).
//! - **Option B** — a **value-free provenance factor** keyed on signals that
//!   stay PUBLIC under CT (mint-epoch origin size, coin age, hop count),
//!   generalizing ADR 0007's bridge-import tagging to *domestic* coins. Zero
//!   proof, coarser. If it holds, it **dissolves the last hard CT gadget** the
//!   way Path C (`ct-compatible-lottery-selection.md`) dissolved the lottery.
//!
//! # The empirical bar (the same one Path C and bridge-import cleared)
//!
//! Does a value-free provenance factor **preserve the redistribution (Δgini)
//! floor AND the Sybil-resistance** that the live value-weighted factor
//! delivers? The Sybil axis is the crux: unlike hidden *value* (which a whale
//! cannot cheaply shrink — the live aggregate is split-invariant and can only
//! fall by giving value away), provenance signals like **age and hop count are
//! things a whale manufactures for free** (churn to stay young, self-hop to
//! fake circulation). This module tests that manufacturability explicitly.
//!
//! # The rules scored
//!
//! | rule | factor input | public under CT? | notes |
//! |---|---|---|---|
//! | **ValueWeighted** | live `Σ v·weight` | **no** (hidden) | the baseline CT would hide; yardstick only |
//! | **Age** | blocks since mint (idle → higher) | yes | value-free, ad-hoc scale |
//! | **HopCount** | number of spends (fresh → higher) | yes | value-free, ad-hoc scale |
//! | **EpochOrigin** | public mint-epoch pool wealth, circulation-decayed, floored `F` | yes | the ADR 0007 generalization |
//!
//! `EpochOrigin` is the load-bearing candidate: mint (coinbase) reward amounts
//! are **public** (PoW-bound minting), so the mint-epoch pool wealth `Σ public
//! coinbase in [mK,(m+1)K)` is a value-free curve input; its factor **decays
//! only by circulation** (the real value-weighted [`crate::TagVector::mix`],
//! not free self-hops), and it is **floored at `F`** — exactly ADR 0007,
//! pointed at domestic mints instead of bridge unwraps.
//!
//! # Method (a focused, deterministic experiment)
//!
//! Same shape as [`super::settlement_horizon_sweep`],
//! [`super::lottery_selection_sweep`] and [`super::bridge_import_sweep`]: a
//! focused, deterministic computation, not the full agent framework, because
//! the lever is the *factor rule*, not multi-agent flow. It reuses the shipped
//! kernels — the real [`crate::ClusterFactorCurve`], the real
//! [`crate::demurrage_charge`], the real [`crate::TagVector::mix`] blend (for
//! circulation decay), and the shared [`super::metrics::calculate_gini`]. No
//! second curve, blend, charge, or Gini implementation. Everything is a pure
//! function of the inputs (no RNG needed); the doc numbers regenerate
//! byte-for-byte (`report_is_reproducible`).

use crate::{
    demurrage_charge, fee_curve::PICO_PER_BTH, ClusterFactorCurve, TagVector, TAG_WEIGHT_SCALE,
};

use super::{bridge_import_sweep::import_cluster_id, metrics::calculate_gini};

/// Blocks per year at the 5s reference block time (matches the node and every
/// other sweep: `31_536_000 s / 5 s`).
pub const BLOCKS_PER_YEAR: u64 = 6_307_200;

/// Annual demurrage rate at maximum factor, basis points (2%/yr, #351).
pub const RATE_BPS: u32 = 200;

/// Import-factor floor `F` for the `EpochOrigin` rule, FACTOR_SCALE units
/// (1500 = 1.5×). Ratified for the bridge boundary by ADR 0007 (#937/#940); the
/// domestic generalization inherits it.
pub const FLOOR_SCALED: u64 = 1_500;

/// Age (blocks) at which the `Age` rule saturates to 6× — an idle coin held
/// this long reads as maximally hoarded. 2 years at the 5s reference.
pub const AGE_REF_BLOCKS: u64 = 2 * BLOCKS_PER_YEAR;

/// Hop count at which the `HopCount` rule bottoms out at 1× — a coin spent this
/// many times reads as fully circulated commerce.
pub const HOP_REF: u32 = 20;

/// Public per-output base fee floor, picocredits (0.25 BTH). The only real cost
/// a churn/hop/split attacker pays per manufactured event.
pub const BASE_FEE_PICO: u128 = PICO_PER_BTH / 4;

/// Deterministic identity for this run (kept for parity with the sibling
/// sweeps; the report is expected-value and needs no draws).
pub const SEED: u64 = 0xB07A_0902_D2;

/// A factor rule under test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule {
    /// Baseline: `ClusterFactorCurve(live Σ v·weight)`. Reads the hidden value
    /// → NOT CT-compatible. The yardstick CT would have to hide (Option A
    /// proves it in a bucket; Option B replaces it).
    ValueWeighted,
    /// Value-free: factor rises with coin **age** (blocks since mint). Idle
    /// hoarding reads as concentrated. Public, but an ad-hoc scale.
    Age,
    /// Value-free: factor falls with **hop count** (spends). A fresh,
    /// uncirculated coin reads as concentrated. Public, but an ad-hoc
    /// scale.
    HopCount,
    /// Value-free: `max(F, ClusterFactorCurve(public mint-epoch pool wealth))`,
    /// decayed toward background **only by circulation** (real
    /// `TagVector::mix`). The ADR 0007 generalization to domestic mints.
    EpochOrigin,
}

impl Rule {
    /// All rules in report order.
    pub fn all() -> Vec<Rule> {
        vec![
            Rule::ValueWeighted,
            Rule::Age,
            Rule::HopCount,
            Rule::EpochOrigin,
        ]
    }

    /// Human label.
    pub fn label(self) -> &'static str {
        match self {
            Rule::ValueWeighted => "ValueWeighted",
            Rule::Age => "Age",
            Rule::HopCount => "HopCount",
            Rule::EpochOrigin => "EpochOrigin",
        }
    }

    /// Does the rule read the hidden UTXO value? If so it is NOT CT-compatible
    /// (it is the thing Option A must prove in ZK, or Option B must replace).
    pub fn reads_hidden_value(self) -> bool {
        matches!(self, Rule::ValueWeighted)
    }

    /// CT-compatible iff it never reads the hidden value.
    pub fn is_ct_clean(self) -> bool {
        !self.reads_hidden_value()
    }

    /// The public signal the rule keys on (CT-leakage note).
    pub fn signal(self) -> &'static str {
        match self {
            Rule::ValueWeighted => "hidden live cluster wealth Σ v·weight",
            Rule::Age => "public blocks-since-mint",
            Rule::HopCount => "public spend/hop count",
            Rule::EpochOrigin => "public mint-epoch pool wealth + circulation blend",
        }
    }
}

/// A holder / UTXO in the population, carrying every signal each rule reads.
#[derive(Clone, Copy, Debug)]
pub struct Holder {
    /// On-chain value in picocredits (the hidden amount under CT).
    pub value_pico: u128,
    /// Live cluster wealth in BTH (`Σ v·weight`) — the ValueWeighted input.
    pub cluster_wealth_bth: u64,
    /// Blocks since mint — the Age input.
    pub age_blocks: u64,
    /// Number of spends the coin has been through — the HopCount input.
    pub hops: u32,
    /// Public mint-epoch pool wealth in BTH (`Σ public coinbase in the epoch`)
    /// — the EpochOrigin base input.
    pub mint_epoch_wealth_bth: u64,
    /// Remaining weight on the mint-epoch origin cluster, TAG_WEIGHT_SCALE
    /// units (1_000_000 = fully origin-tagged; 0 = fully circulated to
    /// background) — the EpochOrigin decay state.
    pub origin_weight_ppm: u32,
}

// ===========================================================================
// The factor rules (each returns FACTOR_SCALE units, 1000 = 1×)
// ===========================================================================

/// Baseline value-weighted factor: the real curve at the live cluster wealth.
pub fn value_weighted_factor(h: &Holder) -> u64 {
    ClusterFactorCurve::default_params().factor(h.cluster_wealth_bth as u128 * PICO_PER_BTH)
}

/// Age factor: `1× + 5× · min(age, AGE_REF)/AGE_REF`. Idle → higher (anti-
/// hoarding). Monotone increasing in age. Value-free but ad-hoc: it has no
/// BTH scale, so it cannot tell a 50k cluster from a 10M one — both just read
/// "old".
pub fn age_factor(age_blocks: u64) -> u64 {
    let f = ClusterFactorCurve::FACTOR_SCALE; // 1000
    let span = 5 * f; // 1×..6×
    f + span * age_blocks.min(AGE_REF_BLOCKS) / AGE_REF_BLOCKS
}

/// Hop factor: `1× + 5× · (HOP_REF − min(hops, HOP_REF))/HOP_REF`. Fresh (few
/// hops) → higher; well-circulated → 1×. Monotone decreasing in hops.
/// Value-free but ad-hoc (no BTH scale).
pub fn hop_factor(hops: u32) -> u64 {
    let f = ClusterFactorCurve::FACTOR_SCALE;
    let span = 5 * f;
    let remaining = HOP_REF - hops.min(HOP_REF);
    f + span * remaining as u64 / HOP_REF as u64
}

/// EpochOrigin factor: the real curve at the *public mint-epoch pool wealth*,
/// clamped to the floor `F`, then blended toward background 1× by the coin's
/// remaining origin weight (its circulation state). Fully origin-tagged →
/// (floored) curve factor; fully circulated → 1×. This mirrors ADR 0007's
/// `import_factor` and its circulation decay, keyed on domestic mints.
pub fn epoch_origin_factor(mint_epoch_wealth_bth: u64, origin_weight_ppm: u32) -> u64 {
    let base = ClusterFactorCurve::default_params()
        .factor(mint_epoch_wealth_bth as u128 * PICO_PER_BTH)
        .max(FLOOR_SCALED);
    // Value-weighted blend of the origin factor (on the origin-tagged share) and
    // 1× background (on the rest) — the same blend the bridge sweep uses.
    let w = origin_weight_ppm as u128;
    let scale = TAG_WEIGHT_SCALE as u128;
    let bg = ClusterFactorCurve::FACTOR_SCALE as u128;
    ((base as u128 * w + bg * (scale - w)) / scale) as u64
}

/// The factor a holder is charged under a given rule (FACTOR_SCALE units).
pub fn factor_for(rule: Rule, h: &Holder) -> u64 {
    match rule {
        Rule::ValueWeighted => value_weighted_factor(h),
        Rule::Age => age_factor(h.age_blocks),
        Rule::HopCount => hop_factor(h.hops),
        Rule::EpochOrigin => epoch_origin_factor(h.mint_epoch_wealth_bth, h.origin_weight_ppm),
    }
}

// ===========================================================================
// Population
// ===========================================================================

/// One population tier. Its signals are set so that, in an HONEST population,
/// concentration correlates across all four signals (a wealthy hoarder is old,
/// unspent, big-mint-origin, and uncirculated; a poor commerce coin is young,
/// churned, small-origin, and fully circulated) — the regime where every rule
/// should recover Δgini. The Sybil track then breaks that correlation by
/// letting the whale manufacture the cheap signals.
#[derive(Clone, Copy, Debug)]
pub struct Tier {
    pub label: &'static str,
    pub count: usize,
    pub value_bth: u64,
    pub cluster_wealth_bth: u64,
    pub age_years_x100: u64,
    pub hops: u32,
    pub mint_epoch_wealth_bth: u64,
    /// Origin weight as a fraction in basis points (10000 = fully
    /// origin-tagged).
    pub origin_weight_bps: u32,
}

/// The redistribution population — the same tier shape as
/// [`super::lottery_selection_sweep::population_tiers`] (a large
/// poor/background commerce tier + a graded wealthy idle tier spanning the real
/// curve), extended with the age / hop / mint-origin signals each provenance
/// rule reads.
pub fn population_tiers() -> Vec<Tier> {
    vec![
        Tier {
            label: "background (poor)",
            count: 90,
            value_bth: 1_000,
            cluster_wealth_bth: 1_000,
            age_years_x100: 2, // ~1 week: young, actively circulating
            hops: 30,          // fully churned commerce
            mint_epoch_wealth_bth: 1_000,
            origin_weight_bps: 0, // fully circulated to background
        },
        Tier {
            label: "50k-cluster wealthy",
            count: 4,
            value_bth: 5_000,
            cluster_wealth_bth: 50_000,
            age_years_x100: 150,
            hops: 0,
            mint_epoch_wealth_bth: 50_000,
            origin_weight_bps: 10_000,
        },
        Tier {
            label: "500k-cluster wealthy",
            count: 3,
            value_bth: 50_000,
            cluster_wealth_bth: 500_000,
            age_years_x100: 200,
            hops: 0,
            mint_epoch_wealth_bth: 500_000,
            origin_weight_bps: 10_000,
        },
        Tier {
            label: "10M-cluster whale",
            count: 3,
            value_bth: 100_000,
            cluster_wealth_bth: 10_000_000,
            age_years_x100: 200,
            hops: 0,
            mint_epoch_wealth_bth: 10_000_000,
            origin_weight_bps: 10_000,
        },
    ]
}

/// Build the flat holder vector from the tiers.
pub fn build_population(tiers: &[Tier]) -> Vec<Holder> {
    let mut holders = Vec::new();
    for t in tiers {
        let age_blocks = t.age_years_x100 * BLOCKS_PER_YEAR / 100;
        let origin_weight_ppm =
            (t.origin_weight_bps as u64 * TAG_WEIGHT_SCALE as u64 / 10_000) as u32;
        for _ in 0..t.count {
            holders.push(Holder {
                value_pico: t.value_bth as u128 * PICO_PER_BTH,
                cluster_wealth_bth: t.cluster_wealth_bth,
                age_blocks,
                hops: t.hops,
                mint_epoch_wealth_bth: t.mint_epoch_wealth_bth,
                origin_weight_ppm,
            });
        }
    }
    holders
}

// ===========================================================================
// Track 1 — faithfulness + honest redistribution
// ===========================================================================

/// Per-tier factor under every rule (the coarseness / faithfulness view).
#[derive(Clone, Debug)]
pub struct FaithfulnessRow {
    pub tier: &'static str,
    pub cluster_wealth_bth: u64,
    /// Factor per rule, in FACTOR_SCALE units, report order of `Rule::all()`.
    pub factors: Vec<u64>,
}

/// Build the faithfulness table: one representative holder per tier, factor
/// under every rule. Reveals how coarsely each value-free signal tracks the
/// value-weighted baseline.
pub fn faithfulness_table(tiers: &[Tier]) -> Vec<FaithfulnessRow> {
    let holders = build_population(tiers);
    // One representative holder index per tier (the first of each).
    let mut idx = 0usize;
    let mut rows = Vec::new();
    for t in tiers {
        let h = &holders[idx];
        let factors = Rule::all().into_iter().map(|r| factor_for(r, h)).collect();
        rows.push(FaithfulnessRow {
            tier: t.label,
            cluster_wealth_bth: t.cluster_wealth_bth,
            factors,
        });
        idx += t.count;
    }
    rows
}

/// Mean absolute factor error of a rule vs the ValueWeighted baseline across
/// the whole population, in FACTOR_SCALE units (lower = more faithful).
pub fn mean_abs_factor_error(holders: &[Holder], rule: Rule) -> f64 {
    if holders.is_empty() {
        return 0.0;
    }
    let mut sum = 0f64;
    for h in holders {
        let base = value_weighted_factor(h) as f64;
        let f = factor_for(rule, h) as f64;
        sum += (f - base).abs();
    }
    sum / holders.len() as f64
}

/// Result of the redistribution experiment for one rule.
#[derive(Clone, Debug)]
pub struct RedistRow {
    pub rule: Rule,
    pub final_gini: f64,
    /// Δgini = burn_baseline_gini − final_gini (positive = more equalizing).
    pub delta_gini: f64,
    /// Recovery vs the ValueWeighted baseline: delta_gini /
    /// baseline_delta_gini.
    pub recovery_pct: f64,
    pub ct_clean: bool,
}

/// Run the collect-and-redistribute loop for one rule and return the final
/// Gini.
///
/// Each year every holder pays one year of ordinary [`crate::demurrage_charge`]
/// at the rule's factor into a pool; the pool is redistributed **per-capita**
/// (the Path C uniform lottery, #980). `None` = burn baseline (collect, no
/// payout) — the Δgini anchor. `force_gamed` (Track 3) overrides the wealthy
/// tiers' factor to 1× for the cheaply-gameable rules.
fn run_redistribution(
    holders: &[Holder],
    years: u64,
    rule: Option<Rule>,
    force_gamed: bool,
) -> f64 {
    let mut wealth: Vec<u128> = holders.iter().map(|h| h.value_pico).collect();
    let n = wealth.len() as u128;
    let mut carry: u128 = 0;

    for _ in 0..years {
        let mut pool: u128 = carry;
        // Collect one year of demurrage at each holder's rule-factor.
        for (i, h) in holders.iter().enumerate() {
            let factor = match rule {
                None => value_weighted_factor(h), // burn world uses the true factor
                Some(r) => effective_factor_for(r, h, force_gamed),
            };
            let value_u64 = u64::try_from(wealth[i]).unwrap_or(u64::MAX);
            let charge = demurrage_charge(
                value_u64,
                factor,
                BLOCKS_PER_YEAR,
                RATE_BPS,
                BLOCKS_PER_YEAR,
            ) as u128;
            let charge = charge.min(wealth[i]);
            wealth[i] -= charge;
            pool += charge;
        }

        let Some(_) = rule else {
            carry = 0; // burn
            continue;
        };

        // Per-capita (uniform) redistribution — Path C.
        if n > 0 && pool > 0 {
            let share = pool / n;
            carry = pool % n;
            if share > 0 {
                for w in wealth.iter_mut() {
                    *w += share;
                }
            }
        } else {
            carry = pool;
        }
    }

    let final_wealths: Vec<u64> = wealth
        .iter()
        .map(|&w| u64::try_from(w / PICO_PER_BTH).unwrap_or(u64::MAX))
        .collect();
    calculate_gini(&final_wealths)
}

/// A holder's factor under a rule, optionally in the "gamed" world where a
/// wealthy holder has manufactured its cheapest free low factor. Only the
/// cheaply-gameable rules (Age, HopCount) collapse to 1×; ValueWeighted and
/// EpochOrigin cannot be gamed for free, so they keep the honest factor.
fn effective_factor_for(rule: Rule, h: &Holder, gamed: bool) -> u64 {
    if gamed && is_wealthy(h) && rule_is_free_gameable(rule) {
        // The whale manufactures the favorable signal (churn young / self-hop)
        // for the price of base fees → the rule bottoms out at 1×.
        ClusterFactorCurve::FACTOR_SCALE
    } else {
        factor_for(rule, h)
    }
}

/// A holder counts as "wealthy" (a demurrage target) if its live cluster wealth
/// puts it above background.
fn is_wealthy(h: &Holder) -> bool {
    h.cluster_wealth_bth > 10_000
}

/// Age and HopCount are manufacturable for free (churn to reset age; self-hop
/// to fake circulation). ValueWeighted and EpochOrigin are not.
pub fn rule_is_free_gameable(rule: Rule) -> bool {
    matches!(rule, Rule::Age | Rule::HopCount)
}

/// Run the redistribution track for all rules in the honest world.
pub fn redistribution_sweep(holders: &[Holder], years: u64, gamed: bool) -> Vec<RedistRow> {
    let burn_gini = run_redistribution(holders, years, None, false);
    let base_gini = run_redistribution(holders, years, Some(Rule::ValueWeighted), gamed);
    let base_delta = (burn_gini - base_gini).max(1e-12);

    Rule::all()
        .into_iter()
        .map(|rule| {
            let final_gini = run_redistribution(holders, years, Some(rule), gamed);
            let delta_gini = burn_gini - final_gini;
            RedistRow {
                rule,
                final_gini,
                delta_gini,
                recovery_pct: delta_gini / base_delta * 100.0,
                ct_clean: rule.is_ct_clean(),
            }
        })
        .collect()
}

// ===========================================================================
// Track 2 — Sybil manufacturability (the crux)
// ===========================================================================

/// A cheap manipulation a whale can attempt to manufacture a lower factor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attack {
    /// No manipulation — the honest factor.
    Honest,
    /// Fragment the position into K fresh coins (a single spend).
    Split,
    /// Self-spend to reset the coin's age to 0 (stay "young").
    ChurnYoung,
    /// Self-hop `HOP_REF` times (wash) to fake a high spend count.
    SelfHopWash,
    /// Acquire real background-tagged value and mix it in (genuine circulation)
    /// — the ONLY attack that is not free.
    RealCirculate,
}

impl Attack {
    pub fn all() -> Vec<Attack> {
        vec![
            Attack::Honest,
            Attack::Split,
            Attack::ChurnYoung,
            Attack::SelfHopWash,
            Attack::RealCirculate,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Attack::Honest => "honest",
            Attack::Split => "split→K coins",
            Attack::ChurnYoung => "churn young (age→0)",
            Attack::SelfHopWash => "self-hop wash (hops→20)",
            Attack::RealCirculate => "real-circulate (costs value)",
        }
    }

    /// Is the attack free (only base fees) or does it cost real value?
    pub fn is_free(self) -> bool {
        !matches!(self, Attack::RealCirculate)
    }
}

/// The attacker: a concentrated whale coin (10M-BTH cluster → factor ~5.745×),
/// value 100k BTH, honestly aged 1 year, unspent, big mint-epoch origin, fully
/// origin-tagged.
pub fn whale() -> Holder {
    Holder {
        value_pico: 100_000u128 * PICO_PER_BTH,
        cluster_wealth_bth: 10_000_000,
        age_blocks: BLOCKS_PER_YEAR,
        hops: 0,
        mint_epoch_wealth_bth: 10_000_000,
        origin_weight_ppm: TAG_WEIGHT_SCALE,
    }
}

/// Apply a cheap attack to the whale, returning the manipulated holder.
/// `RealCirculate` mixes in `circulate_mix_frac`·value of background money per
/// step for `circulate_steps` steps via the **real** [`crate::TagVector::mix`].
fn apply_attack(base: &Holder, attack: Attack) -> Holder {
    let mut h = *base;
    match attack {
        Attack::Honest => {}
        Attack::Split => {
            // A split is a single spend: fresh outputs (age 0, hops+1), but the
            // children inherit the parent's cluster wealth, mint-epoch origin and
            // origin tag (provenance carries through splits). Value per piece
            // shrinks but the FACTOR inputs that matter are unchanged.
            h.age_blocks = 0;
            h.hops = base.hops + 1;
        }
        Attack::ChurnYoung => {
            // Self-spend resets age; no background value enters, so the origin
            // tag is untouched (mix with 0 incoming is a no-op).
            h.age_blocks = 0;
            h.hops = base.hops + 1;
        }
        Attack::SelfHopWash => {
            // Many free self-spends: fake a high hop count. Age stays ~0 (fresh
            // outputs each hop). Origin tag untouched (no background value).
            h.age_blocks = 0;
            h.hops = HOP_REF;
        }
        Attack::RealCirculate => {
            // Genuine circulation: receive real background-tagged value and mix.
            // This is the only attack that shifts the origin tag — and it costs
            // real value (measured separately). We drive the REAL blend.
            let import_id = import_cluster_id(0xD2);
            let mut tag = TagVector::single(import_id);
            let background = TagVector::new();
            let mut held: u64 = u64::try_from(base.value_pico).unwrap_or(u64::MAX);
            // Receive 1× its value in background money over 9 spends — the
            // ADR 0007 worst-case decay depth (#940: ≈9 spends to the floor).
            let incoming = (held as u128 / 1) as u64; // 1× value per spend
            for _ in 0..9 {
                tag.mix(held, &background, incoming);
                held = held.saturating_add(incoming);
            }
            h.origin_weight_ppm = tag.get(import_id);
            // Age/hops also move under real circulation, but the origin tag is
            // what governs EpochOrigin.
            h.age_blocks = 0;
            h.hops = base.hops + 9;
        }
    }
    h
}

/// One cell of the Sybil table: the whale's factor under `rule` after `attack`.
#[derive(Clone, Debug)]
pub struct SybilCell {
    pub rule: Rule,
    pub attack: Attack,
    /// Factor after the attack, FACTOR_SCALE units.
    pub factor_scaled: u64,
    /// Drop from the honest factor, FACTOR_SCALE units (positive = the attack
    /// lowered the factor).
    pub drop_scaled: i64,
    /// Was the attack free (only base fees)?
    pub free: bool,
}

/// Run the Sybil track: every rule × every attack.
pub fn sybil_table() -> Vec<SybilCell> {
    let base = whale();
    let mut cells = Vec::new();
    for rule in Rule::all() {
        let honest = factor_for(rule, &base);
        for attack in Attack::all() {
            let h = apply_attack(&base, attack);
            let f = factor_for(rule, &h);
            cells.push(SybilCell {
                rule,
                attack,
                factor_scaled: f,
                drop_scaled: honest as i64 - f as i64,
                free: attack.is_free(),
            });
        }
    }
    cells
}

/// Is a rule cheaply gameable? True iff SOME **free** attack drops its factor
/// by more than a 0.25× threshold (250 FACTOR_SCALE units). This is the Sybil
/// verdict per rule.
pub fn is_cheaply_gameable(cells: &[SybilCell], rule: Rule) -> bool {
    cells
        .iter()
        .filter(|c| c.rule == rule && c.free)
        .any(|c| c.drop_scaled > 250)
}

// ===========================================================================
// Report
// ===========================================================================

/// The full sweep report.
#[derive(Clone, Debug)]
pub struct ProvenanceFactorReport {
    pub tiers: Vec<Tier>,
    pub years: u64,
    pub initial_gini: f64,
    pub burn_gini: f64,
    pub faithfulness: Vec<FaithfulnessRow>,
    pub mean_abs_errors: Vec<(Rule, f64)>,
    pub redist_honest: Vec<RedistRow>,
    pub redist_gamed: Vec<RedistRow>,
    pub sybil: Vec<SybilCell>,
}

/// Run the complete provenance-factor sweep with the default configuration.
pub fn run_provenance_factor_sweep() -> ProvenanceFactorReport {
    let tiers = population_tiers();
    let holders = build_population(&tiers);
    let years = 10;

    let initial: Vec<u64> = holders
        .iter()
        .map(|h| (h.value_pico / PICO_PER_BTH) as u64)
        .collect();
    let initial_gini = calculate_gini(&initial);
    let burn_gini = run_redistribution(&holders, years, None, false);

    let faithfulness = faithfulness_table(&tiers);
    let mean_abs_errors = Rule::all()
        .into_iter()
        .map(|r| (r, mean_abs_factor_error(&holders, r)))
        .collect();
    let redist_honest = redistribution_sweep(&holders, years, false);
    let redist_gamed = redistribution_sweep(&holders, years, true);
    let sybil = sybil_table();

    ProvenanceFactorReport {
        tiers,
        years,
        initial_gini,
        burn_gini,
        faithfulness,
        mean_abs_errors,
        redist_honest,
        redist_gamed,
        sybil,
    }
}

/// Render the report as Markdown (the doc numbers are generated here, never
/// hand-computed).
pub fn to_markdown(report: &ProvenanceFactorReport) -> String {
    let mut s = String::new();
    let f_scale = ClusterFactorCurve::FACTOR_SCALE as f64;
    let rules = Rule::all();

    // --- Population.
    s.push_str("### Population (factors from the real `ClusterFactorCurve`)\n\n");
    s.push_str(
        "| tier | count | value (BTH) | cluster wealth (BTH) | age (yr) | hops | mint-epoch (BTH) | origin-tagged |\n",
    );
    s.push_str(
        "|------|------:|------------:|---------------------:|---------:|-----:|-----------------:|:-------------:|\n",
    );
    for t in &report.tiers {
        s.push_str(&format!(
            "| {} | {} | {} | {} | {:.2} | {} | {} | {:.0}% |\n",
            t.label,
            t.count,
            t.value_bth,
            t.cluster_wealth_bth,
            t.age_years_x100 as f64 / 100.0,
            t.hops,
            t.mint_epoch_wealth_bth,
            t.origin_weight_bps as f64 / 100.0,
        ));
    }
    s.push('\n');

    // --- Faithfulness.
    s.push_str("### Track 1a — faithfulness: factor per rule, by tier\n\n");
    s.push_str(
        "How coarsely each value-free signal tracks the value-weighted baseline. \
         A faithful rule reproduces the baseline column; a coarse one flattens the \
         wealthy tiers together.\n\n",
    );
    s.push_str("| tier | cluster wealth |");
    for r in &rules {
        s.push_str(&format!(" {} |", r.label()));
    }
    s.push('\n');
    s.push_str("|------|---------------:|");
    for _ in &rules {
        s.push_str("------:|");
    }
    s.push('\n');
    for row in &report.faithfulness {
        s.push_str(&format!("| {} | {} |", row.tier, row.cluster_wealth_bth));
        for f in &row.factors {
            s.push_str(&format!(" {:.3}x |", *f as f64 / f_scale));
        }
        s.push('\n');
    }
    s.push('\n');
    s.push_str(
        "Mean absolute factor error vs the ValueWeighted baseline (lower = more faithful):\n\n",
    );
    s.push_str("| rule | mean |factor − baseline| |\n");
    s.push_str("|------|--------------------:|\n");
    for (r, e) in &report.mean_abs_errors {
        s.push_str(&format!("| {} | {:.3}x |\n", r.label(), e / f_scale));
    }
    s.push('\n');

    // --- Honest redistribution.
    s.push_str("### Track 1b — redistribution in an HONEST population (Δgini vs burn)\n\n");
    s.push_str(&format!(
        "Collect-and-redistribute over {} years, per-capita (Path C uniform) payout. \
         Initial Gini {:.4}; burn baseline Gini {:.4}. Δgini = burn − rule (higher = \
         more equalizing). Recovery = Δgini ÷ ValueWeighted's Δgini.\n\n",
        report.years, report.initial_gini, report.burn_gini,
    ));
    s.push_str("| rule | final Gini | Δgini | recovery vs baseline | CT-clean |\n");
    s.push_str("|------|-----------:|------:|---------------------:|:--------:|\n");
    for r in &report.redist_honest {
        s.push_str(&format!(
            "| {} | {:.4} | {:+.4} | {:.1}% | {} |\n",
            r.rule.label(),
            r.final_gini,
            r.delta_gini,
            r.recovery_pct,
            if r.ct_clean { "yes" } else { "**no**" },
        ));
    }
    s.push('\n');

    // --- Sybil manufacturability.
    s.push_str("### Track 2 — Sybil manufacturability (the crux)\n\n");
    s.push_str(&format!(
        "A concentrated whale (value 100k BTH, live cluster wealth 10M → baseline {:.3}x) \
         tries to manufacture a LOWER factor with cheap manipulations. Each cell is the \
         whale's factor AFTER the attack; **bold** = the attack is FREE (base fees only). \
         A rule is Sybil-broken iff some free attack drops its factor materially.\n\n",
        value_weighted_factor(&whale()) as f64 / f_scale,
    ));
    s.push_str("| rule |");
    for a in Attack::all() {
        s.push_str(&format!(" {} |", a.label()));
    }
    s.push_str(" cheaply gameable? |\n");
    s.push_str("|------|");
    for _ in Attack::all() {
        s.push_str("------:|");
    }
    s.push_str(":-----------------:|\n");
    for rule in &rules {
        s.push_str(&format!("| {} |", rule.label()));
        for a in Attack::all() {
            let cell = report
                .sybil
                .iter()
                .find(|c| c.rule == *rule && c.attack == a)
                .unwrap();
            let f = cell.factor_scaled as f64 / f_scale;
            if cell.free {
                s.push_str(&format!(" **{:.3}x** |", f));
            } else {
                s.push_str(&format!(" {:.3}x |", f));
            }
        }
        let gameable = is_cheaply_gameable(&report.sybil, *rule);
        s.push_str(&format!(
            " {} |\n",
            if gameable { "**YES — broken**" } else { "no" }
        ));
    }
    s.push('\n');
    s.push_str(
        "Reading it: `split`, `churn young` and `self-hop wash` are all FREE (only base \
         fees). ValueWeighted and EpochOrigin are unmoved by every free attack — the live \
         aggregate is split-invariant and can only fall by giving value away, and the \
         epoch-origin tag decays **only** by real circulation (mixing 0 incoming value is a \
         no-op). Age and HopCount collapse to 1× under a free churn/wash: they key on \
         'structure the holder controls for free', the exact Path-C failure mode. \
         `real-circulate` (NOT free) is the only thing that lowers EpochOrigin — and it \
         costs the whale a full position's worth of genuine background value.\n\n",
    );

    // --- Gamed redistribution.
    s.push_str("### Track 3 — redistribution when the whale GAMES the factor\n\n");
    s.push_str(
        "The same collect-and-redistribute, but the wealthy tiers manufacture their \
         cheapest FREE low factor (Age → churn young; HopCount → self-hop wash → 1×). \
         ValueWeighted and EpochOrigin cannot be gamed for free, so the wealthy keep their \
         honest factor. This is the redistribution consequence of the Track-2 result.\n\n",
    );
    s.push_str("| rule | Δgini honest | Δgini gamed | survives gaming? | CT-clean |\n");
    s.push_str("|------|-------------:|------------:|:----------------:|:--------:|\n");
    for rule in &rules {
        let honest = report
            .redist_honest
            .iter()
            .find(|r| r.rule == *rule)
            .unwrap();
        let gamed = report
            .redist_gamed
            .iter()
            .find(|r| r.rule == *rule)
            .unwrap();
        // "Survives" = the gamed Δgini keeps most of the honest redistribution
        // (>= 60% retained) AND stays above the 0.05 design floor.
        let survives = gamed.delta_gini > 0.05 && gamed.delta_gini >= 0.6 * honest.delta_gini;
        s.push_str(&format!(
            "| {} | {:+.4} | {:+.4} | {} | {} |\n",
            rule.label(),
            honest.delta_gini,
            gamed.delta_gini,
            if survives { "yes" } else { "**NO**" },
            if rule.is_ct_clean() { "yes" } else { "**no**" },
        ));
    }
    s.push('\n');

    // --- CT / leakage summary.
    s.push_str("### CT-compatibility & signal summary\n\n");
    s.push_str("| rule | CT-clean | keys on | Sybil verdict |\n");
    s.push_str("|------|:--------:|---------|---------------|\n");
    for rule in &rules {
        let gameable = is_cheaply_gameable(&report.sybil, *rule);
        let verdict = match rule {
            Rule::ValueWeighted => "resistant (but reads hidden value → Option A)",
            _ if gameable => "**BROKEN — free manufacture**",
            _ => "resistant (decay is circulation-gated)",
        };
        s.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            rule.label(),
            if rule.is_ct_clean() { "yes" } else { "**no**" },
            rule.signal(),
            verdict,
        ));
    }
    s.push('\n');

    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_weighted_is_the_hidden_baseline() {
        assert!(!Rule::ValueWeighted.is_ct_clean());
        for r in [Rule::Age, Rule::HopCount, Rule::EpochOrigin] {
            assert!(r.is_ct_clean(), "{} should be CT-clean", r.label());
        }
    }

    #[test]
    fn age_and_hop_are_free_gameable_but_valueweighted_and_epochorigin_are_not() {
        // The crux negative/positive result: a free churn/wash collapses Age and
        // HopCount to ~1×, but leaves ValueWeighted and EpochOrigin untouched.
        let cells = sybil_table();
        assert!(
            is_cheaply_gameable(&cells, Rule::Age),
            "Age must be gameable"
        );
        assert!(
            is_cheaply_gameable(&cells, Rule::HopCount),
            "HopCount must be gameable"
        );
        assert!(
            !is_cheaply_gameable(&cells, Rule::EpochOrigin),
            "EpochOrigin must NOT be free-gameable"
        );
        assert!(
            !is_cheaply_gameable(&cells, Rule::ValueWeighted),
            "ValueWeighted must NOT be free-gameable"
        );
    }

    #[test]
    fn epoch_origin_only_falls_under_real_circulation() {
        // Every FREE attack leaves EpochOrigin ~unchanged; only RealCirculate
        // (which costs value) lowers it.
        let cells: Vec<_> = sybil_table()
            .into_iter()
            .filter(|c| c.rule == Rule::EpochOrigin)
            .collect();
        for c in &cells {
            if c.free {
                assert!(
                    c.drop_scaled <= 250,
                    "free attack {:?} should not move EpochOrigin (drop {})",
                    c.attack,
                    c.drop_scaled
                );
            }
        }
        let real = cells
            .iter()
            .find(|c| c.attack == Attack::RealCirculate)
            .unwrap();
        assert!(
            real.drop_scaled > 250,
            "real circulation should lower EpochOrigin, drop {}",
            real.drop_scaled
        );
    }

    #[test]
    fn honest_redistribution_recovers_for_all_rules() {
        // In an honest population every rule (age/hops/origin) recovers a
        // meaningful share of the baseline Δgini — the signals are correlated.
        let report = run_provenance_factor_sweep();
        let base = report
            .redist_honest
            .iter()
            .find(|r| r.rule == Rule::ValueWeighted)
            .unwrap();
        assert!(base.delta_gini > 0.05, "baseline must clear the floor");
        for r in &report.redist_honest {
            assert!(
                r.delta_gini > 0.05,
                "{} should clear the 0.05 floor honestly: {}",
                r.rule.label(),
                r.delta_gini
            );
        }
    }

    #[test]
    fn gamed_redistribution_collapses_only_for_gameable_rules() {
        // The money shot: under gaming, Age/HopCount lose their redistribution
        // while ValueWeighted/EpochOrigin keep it.
        let report = run_provenance_factor_sweep();
        let get = |rows: &[RedistRow], rule: Rule| {
            rows.iter().find(|r| r.rule == rule).unwrap().delta_gini
        };
        for rule in [Rule::Age, Rule::HopCount] {
            let honest = get(&report.redist_honest, rule);
            let gamed = get(&report.redist_gamed, rule);
            assert!(
                gamed < 0.5 * honest,
                "{} redistribution should collapse under gaming: honest {} gamed {}",
                rule.label(),
                honest,
                gamed
            );
        }
        for rule in [Rule::ValueWeighted, Rule::EpochOrigin] {
            let honest = get(&report.redist_honest, rule);
            let gamed = get(&report.redist_gamed, rule);
            assert!(
                (gamed - honest).abs() < 1e-9,
                "{} redistribution should survive gaming",
                rule.label()
            );
        }
    }

    #[test]
    fn report_is_reproducible() {
        let a = to_markdown(&run_provenance_factor_sweep());
        let b = to_markdown(&run_provenance_factor_sweep());
        assert_eq!(a, b, "sweep output must be byte-for-byte deterministic");
    }

    #[test]
    fn markdown_has_all_sections() {
        let md = to_markdown(&run_provenance_factor_sweep());
        assert!(md.contains("Track 1a — faithfulness"));
        assert!(md.contains("Track 1b — redistribution"));
        assert!(md.contains("Track 2 — Sybil manufacturability"));
        assert!(md.contains("Track 3 — redistribution when the whale GAMES"));
        for rule in Rule::all() {
            assert!(md.contains(rule.label()), "missing rule {}", rule.label());
        }
    }
}
