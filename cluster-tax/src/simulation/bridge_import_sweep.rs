//! Bridge-import calibration sweep — the empirical gate for issue #937.
//!
//! # What this sweep calibrates
//!
//! ADR 0007 makes an **unwrap** (wBTH → BTH) mint into a **block-epoch import
//! cluster**: every unwrap in height range `[mK, (m+1)K)` joins one shared
//! origin `c_import(m) = H("bridge-import" ‖ m)`, where `m = ⌊height / K⌋`. The
//! import cluster's wealth (the curve input) is the **sum of all unwrap amounts
//! in the epoch**, and the production [`crate::ClusterFactorCurve`] maps it to a
//! factor — the identical curve domestic clusters use — clamped to a floor `F`:
//!
//! ```text
//! import_factor(m) = max(F, ClusterFactorCurve(Σ unwrap amounts in epoch m))
//! ```
//!
//! Two constants require calibration before ADR 0007 moves Proposed → Accepted:
//!
//! 1. **Epoch length `K`** (blocks). Trades **split-game cost** (≈ `K` blocks ×
//!    5 s per dilution step — a whale must spread its unwraps across *epochs* to
//!    dilute the shared pool, and each epoch costs wall-clock time) against
//!    **co-location collateral** (a genuinely small entrant that lands in a
//!    co-occurring whale-flood epoch inherits the flood's high factor). Short
//!    `K` → little collateral but cheaper to split-game over time; long `K` →
//!    costlier to game but a small entrant is likelier caught in a flood.
//! 2. **Import-factor floor `F`**. Bounds the split-game payoff (the best factor
//!    a whale can reach by diluting is `F`, not `1`) and sets the minimum
//!    bridge-entry toll. `F = 1` = no floor (split-gameable toward 1×); higher
//!    `F` penalizes *all* imports more bluntly, including small honest entrants.
//!
//! # The load-bearing structural fact (why the epoch key defeats the split)
//!
//! A **size-based per-unwrap factor** is Sybil-able: a whale drip-splits into
//! `N` dust unwraps, each a separate low-wealth origin at factor ~1×, then
//! reassembles domestically. The epoch key defeats this because all unwraps in a
//! window **share one accumulating cluster** — intra-epoch splitting piles into
//! the same pool and still hits the high factor. Diluting requires spreading
//! across *epochs*, which costs `K` blocks each. **Time-as-cost replaces
//! provenance-as-cost.** This sweep quantifies exactly how much time, per `K`.
//!
//! # Decay by circulation (the third experiment)
//!
//! An imported coin's factor falls **only** by circulating: as it mixes with
//! background-tagged inputs through ordinary spends it shifts weight off
//! `c_import(m)` toward background and its factor drops. There is no time-based
//! decay. We model this with the **real** [`crate::TagVector::mix`] value-
//! weighted blend (no reimplementation) and count how many domestic-mixing
//! spends normalize an imported coin toward background. We also confirm the
//! intended invariant: a **pure-external holder who never receives domestic
//! money stays ≥ F** forever (it never mixes, so its tag never shifts).
//!
//! # Method (a focused, deterministic experiment)
//!
//! Like [`super::settlement_horizon_sweep`], this is a focused, deterministic
//! computation rather than the full agent framework: the levers here are the two
//! constants `K` and `F` against analytic failure modes, not multi-agent flow.
//! It reuses the shipped [`crate::ClusterFactorCurve`], the shipped
//! [`crate::TagVector`] blend, the shipped [`crate::demurrage_charge`] kernel,
//! and the shared [`super::metrics::calculate_gini`] — no second curve, blend,
//! charge, or Gini implementation. Everything is a pure function of the inputs
//! (no RNG is needed); the doc numbers regenerate byte-for-byte.

use crate::{
    demurrage_charge, fee_curve::PICO_PER_BTH, ClusterFactorCurve, ClusterId, TagVector,
    TAG_WEIGHT_SCALE,
};

use super::metrics::calculate_gini;

/// Blocks per year at the 5s reference block time (matches the node and the
/// settlement-horizon sweep: `31_536_000 s / 5 s`).
pub const BLOCKS_PER_YEAR: u64 = 6_307_200;

/// Reference block time in seconds (the 5s node reference; #833 shares it).
pub const SECONDS_PER_BLOCK: u64 = 5;

/// Annual demurrage rate at maximum factor, basis points (2%/yr, #351). Used to
/// price the residual anti-hoarding toll on imported wealth (the `F` sweep).
pub const RATE_BPS: u32 = 200;

/// Floor scale: `F` is expressed in `ClusterFactorCurve::FACTOR_SCALE` units
/// (1000 = 1.0×), matching every other factor in the codebase.
pub const FLOOR_SCALE: u64 = ClusterFactorCurve::FACTOR_SCALE;

// ============================================================================
// Candidate grids
// ============================================================================

/// A candidate epoch length `K`, labelled by its wall-clock magnitude at the 5s
/// reference. The candidate range is the ADR §Open-calibration bracket:
/// ~10k blocks (~14 h) to ~1 week (~120,960 blocks).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Epoch {
    /// The candidate `K` in blocks.
    pub blocks: u64,
    /// Human label (e.g. "14h").
    pub label: &'static str,
}

impl Epoch {
    /// Wall-clock hours one epoch spans at the 5s reference.
    pub fn hours(&self) -> f64 {
        self.blocks as f64 * SECONDS_PER_BLOCK as f64 / 3600.0
    }
}

/// The candidate epoch lengths swept, in report order: ~14 h → ~1 week of blocks
/// at the 5s reference. These bracket the ADR's stated range.
pub fn candidate_epochs() -> Vec<Epoch> {
    vec![
        Epoch {
            blocks: 10_080,
            label: "14h",
        },
        Epoch {
            blocks: 17_280,
            label: "1d",
        },
        Epoch {
            blocks: 34_560,
            label: "2d",
        },
        Epoch {
            blocks: 60_480,
            label: "3.5d",
        },
        Epoch {
            blocks: 120_960,
            label: "1wk",
        },
    ]
}

/// Candidate import-factor floors `F`, in FACTOR_SCALE units. Report order 1.0×
/// (no floor) → 2.0×, bracketing the ADR's 1.5×–2× candidate window with the
/// no-floor endpoint for contrast.
pub fn candidate_floors() -> Vec<u64> {
    vec![1000, 1250, 1500, 1750, 2000]
}

/// A bridge-inflow regime: the total unwrap volume that lands in a single epoch,
/// used to derive the epoch cluster's factor from the real curve.
#[derive(Clone, Copy, Debug)]
pub struct VolumeRegime {
    /// Human label ("low" / "medium" / "high").
    pub label: &'static str,
    /// Total unwrap volume in the epoch, in BTH (the curve input).
    pub epoch_volume_bth: u64,
}

/// Bridge-volume regimes, spanning the curve: an organic trickle (well below the
/// 100k-BTH midpoint), a medium inflow near the knee, and a capital flood at/
/// above saturation.
pub fn volume_regimes() -> Vec<VolumeRegime> {
    vec![
        VolumeRegime {
            label: "low",
            epoch_volume_bth: 5_000,
        },
        VolumeRegime {
            label: "medium",
            epoch_volume_bth: 100_000,
        },
        VolumeRegime {
            label: "high",
            epoch_volume_bth: 2_000_000,
        },
    ]
}

/// The canonical `c_import(m)` cluster id for epoch `m`.
///
/// The consensus rule is `c_import(m) = H("bridge-import" ‖ m)`; here we only
/// need a *distinct, deterministic* id per epoch for the tag-blend decay model
/// (the actual hash is a node concern), so we fold the domain tag and `m` into
/// the simulation's `ClusterId` space. Distinctness across epochs is all the
/// decay model relies on.
pub fn import_cluster_id(m: u64) -> ClusterId {
    // A fixed high offset keeps import-cluster ids clear of the small
    // domestic ids used elsewhere in the sim.
    ClusterId::new(0xB010_0000_0000_0000 ^ m)
}

/// Compute the import factor for an epoch given its total unwrap volume and the
/// floor `F`, using the **real** curve clamped to the floor.
pub fn import_factor(epoch_volume_bth: u64, floor_scaled: u64) -> u64 {
    let curve = ClusterFactorCurve::default_params();
    let wealth_pico = epoch_volume_bth as u128 * PICO_PER_BTH;
    curve.factor(wealth_pico).max(floor_scaled)
}

// ============================================================================
// Experiment 1: split-game cost vs innocent-small-entrant collateral (K sweep)
// ============================================================================

/// One row of the split-game table: for a fixed floor `F` and a whale import of
/// a given total size, how expensive is diluting it to the floor at each `K`.
#[derive(Clone, Debug)]
pub struct SplitGameRow {
    pub epoch: Epoch,
    /// The whale's total import size in BTH (the wealth it wants at factor `F`).
    pub whale_import_bth: u64,
    /// The floor `F` in effect, FACTOR_SCALE units.
    pub floor_scaled: u64,
    /// Factor a whale pays if it dumps the whole import in ONE epoch (the
    /// naive, un-gamed path) — the real curve at the full volume.
    pub undiluted_factor_scaled: u64,
    /// Number of epochs the whale must spread across so each epoch's share is
    /// small enough that the per-epoch factor bottoms out at the floor `F`.
    pub epochs_to_floor: u64,
    /// Wall-clock cost of that dilution, in days (`epochs_to_floor × K × 5s`).
    pub split_game_days: f64,
}

/// Smallest number of equal epochs `n` such that spreading `whale_import_bth`
/// across them puts each epoch's per-epoch volume at or below the floor knee —
/// i.e. `import_factor(whale/n) == F`. This is the whale's cheapest dilution:
/// fewer epochs leaves some epoch above the floor; more epochs is wasted time.
fn epochs_to_reach_floor(whale_import_bth: u64, floor_scaled: u64) -> u64 {
    // If even the whole import is already at/below the floor, one epoch suffices.
    if import_factor(whale_import_bth, floor_scaled) <= floor_scaled {
        return 1;
    }
    // Otherwise search upward for the smallest split count. The factor is
    // monotonic in per-epoch volume, so the first `n` that reaches the floor is
    // the minimum. Bounded generously; the curve floors well before this.
    let mut n = 2u64;
    loop {
        let per_epoch = whale_import_bth / n;
        if import_factor(per_epoch, floor_scaled) <= floor_scaled {
            return n;
        }
        n += 1;
        if n > 10_000_000 {
            // Defensive: the log-domain curve floors long before this for any
            // realistic import; a hit here would indicate a floor set at 1.0×
            // with an import so large its per-epoch share never reaches 1×.
            return n;
        }
    }
}

/// Compute the split-game table across every epoch for a fixed floor and a set
/// of whale import sizes.
pub fn split_game_table(
    epochs: &[Epoch],
    floor_scaled: u64,
    whale_imports_bth: &[u64],
) -> Vec<SplitGameRow> {
    let mut rows = Vec::with_capacity(epochs.len() * whale_imports_bth.len());
    for &whale in whale_imports_bth {
        let undiluted = import_factor(whale, floor_scaled);
        let n = epochs_to_reach_floor(whale, floor_scaled);
        for &epoch in epochs {
            // Each dilution step is one fresh epoch: (n-1) waits of K blocks
            // (the first epoch is "free" — the whale is unwrapping anyway), or
            // n epochs total elapsed. We report total elapsed time to place all
            // n tranches, which is what the whale actually pays.
            let total_blocks = n.saturating_mul(epoch.blocks);
            let split_game_days =
                total_blocks as f64 * SECONDS_PER_BLOCK as f64 / 86_400.0;
            rows.push(SplitGameRow {
                epoch,
                whale_import_bth: whale,
                floor_scaled,
                undiluted_factor_scaled: undiluted,
                epochs_to_floor: n,
                split_game_days,
            });
        }
    }
    rows
}

/// One row of the collateral table: a genuinely small entrant unwraps `small`
/// BTH, but co-occurring whale-flood volume also lands in the same epoch. The
/// entrant inherits the SHARED pool's factor (epoch wealth = small + flood).
#[derive(Clone, Debug)]
pub struct CollateralRow {
    /// The bridge-volume regime of the co-occurring flood.
    pub regime: VolumeRegime,
    /// The innocent small entrant's own unwrap size in BTH.
    pub small_entrant_bth: u64,
    /// The floor `F` in effect, FACTOR_SCALE units.
    pub floor_scaled: u64,
    /// Factor the small entrant would get ALONE (no co-occurring flood):
    /// `max(F, curve(small))`. A retail-scale import is low on the curve, so
    /// this is the floor F whenever F exceeds the coin's natural curve factor.
    pub alone_factor_scaled: u64,
    /// Factor the small entrant actually gets, caught in the shared flood epoch:
    /// `import_factor(small + flood)`.
    pub caught_factor_scaled: u64,
    /// The collateral premium: caught factor ÷ alone factor. 1.0 = no
    /// collateral; higher = the innocent pays for the whale's flood.
    pub collateral_ratio: f64,
}

/// Compute the collateral table across every volume regime for a fixed floor
/// and a small-entrant size. `K` does not enter the factor arithmetic (the
/// shared pool is per-epoch regardless of `K`); `K` governs the *probability* a
/// small entrant is caught, which we discuss qualitatively in the doc. This
/// table quantifies the *severity* when caught.
pub fn collateral_table(
    regimes: &[VolumeRegime],
    floor_scaled: u64,
    small_entrant_bth: u64,
) -> Vec<CollateralRow> {
    let alone = import_factor(small_entrant_bth, floor_scaled);
    regimes
        .iter()
        .map(|&regime| {
            let shared_volume = regime.epoch_volume_bth + small_entrant_bth;
            let caught = import_factor(shared_volume, floor_scaled);
            CollateralRow {
                regime,
                small_entrant_bth,
                floor_scaled,
                alone_factor_scaled: alone,
                caught_factor_scaled: caught,
                collateral_ratio: caught as f64 / alone as f64,
            }
        })
        .collect()
}

// ============================================================================
// Experiment 2: residual anti-hoarding vs onboarding friction (F sweep)
// ============================================================================

/// One row of the floor table: for each candidate `F`, the residual anti-
/// hoarding on imported wealth before it circulates, and the onboarding friction
/// a genuine small entrant pays.
#[derive(Clone, Debug)]
pub struct FloorRow {
    /// The floor `F`, FACTOR_SCALE units.
    pub floor_scaled: u64,
    /// Effective factor a genuine SMALL entrant pays: `max(F, curve(small))`.
    /// A retail-scale import sits low on the curve, so the floor F is the toll
    /// whenever F exceeds the coin's natural curve factor (the onboarding toll).
    pub small_entrant_factor_scaled: u64,
    /// One year of demurrage a small entrant's imported coin owes at factor `F`
    /// before it circulates, as a % of the coin value — the onboarding friction
    /// in money terms.
    pub small_entrant_annual_toll_pct: f64,
    /// Effective factor a whale who fully split-gamed down to the floor pays —
    /// also `F` (the floor is the residual anti-hoarding that survives gaming).
    pub gamed_whale_floor_factor_scaled: u64,
    /// dGini-style residual: the effective-factor GAP between a background
    /// (factor-1) domestic coin and a floored imported coin, normalized to the
    /// max possible gap (6× − 1×). 0 = imported wealth is indistinguishable
    /// from background (no residual anti-hoarding); 1 = maximally taxed. This is
    /// the "does imported wealth stay meaningfully above background" metric.
    pub residual_above_background: f64,
}

/// A representative small-entrant coin value for the onboarding-toll figure, in
/// BTH. A genuine retail-scale bridge user.
pub const SMALL_ENTRANT_BTH: u64 = 1_000;

/// Compute the floor table across every candidate `F`.
pub fn floor_table(floors: &[u64]) -> Vec<FloorRow> {
    floors
        .iter()
        .map(|&floor_scaled| {
            // A small entrant's own volume is below the curve knee, so its
            // curve factor is ~1× and the effective factor is exactly the floor.
            let small_factor = import_factor(SMALL_ENTRANT_BTH, floor_scaled);
            let value_pico = SMALL_ENTRANT_BTH as u128 * PICO_PER_BTH;
            let value_u64 =
                u64::try_from(value_pico).expect("small-entrant value fits u64 picocredits");
            let annual_toll = demurrage_charge(
                value_u64,
                small_factor,
                BLOCKS_PER_YEAR,
                RATE_BPS,
                BLOCKS_PER_YEAR,
            );
            let annual_toll_pct = annual_toll as f64 / value_u64 as f64 * 100.0;

            // The residual anti-hoarding that survives full split-gaming is the
            // floor itself.
            let max_gap = ClusterFactorCurve::FACTOR_SCALE * 6 - ClusterFactorCurve::FACTOR_SCALE;
            let residual =
                (floor_scaled - ClusterFactorCurve::FACTOR_SCALE) as f64 / max_gap as f64;

            FloorRow {
                floor_scaled,
                small_entrant_factor_scaled: small_factor,
                small_entrant_annual_toll_pct: annual_toll_pct,
                gamed_whale_floor_factor_scaled: floor_scaled,
                residual_above_background: residual,
            }
        })
        .collect()
}

// ============================================================================
// Experiment 3: decay-by-circulation (real TagVector::mix)
// ============================================================================

/// One row of the decay table: after `spends` domestic-mixing transactions, the
/// imported coin's remaining weight on its import cluster and its effective
/// factor (derived by treating the import-tag weight as the fraction of the
/// coin still priced at the import factor).
#[derive(Clone, Debug)]
pub struct DecayRow {
    /// Number of domestic-mixing spends applied.
    pub spends: u64,
    /// Remaining weight on the import cluster, in TAG_WEIGHT_SCALE units.
    pub import_weight: u32,
    /// Remaining weight as a fraction (0..=1).
    pub import_weight_frac: f64,
    /// Effective factor: a value-weighted blend of the import factor (on the
    /// remaining import-tagged fraction) and background 1× (on the rest),
    /// FACTOR_SCALE units.
    pub effective_factor_scaled: u64,
}

/// Parameters for the decay experiment.
#[derive(Clone, Debug)]
pub struct DecayParams {
    /// The imported coin's starting value, BTH.
    pub coin_value_bth: u64,
    /// The size of each incoming domestic (background-tagged) payment the coin
    /// receives per spend, as a fraction of the coin value, in basis points.
    /// 10000 = the coin receives its own value in domestic money each spend.
    pub incoming_frac_bps: u32,
    /// The import factor the coin starts at, FACTOR_SCALE units (its epoch's
    /// factor, ≥ F).
    pub import_factor_scaled: u64,
    /// Max spends to simulate.
    pub max_spends: u64,
    /// The import cluster's epoch index (for the tag id).
    pub epoch_m: u64,
}

impl Default for DecayParams {
    fn default() -> Self {
        Self {
            coin_value_bth: 10_000,
            incoming_frac_bps: 10_000, // receives 1× its value in domestic money per spend
            import_factor_scaled: 6 * ClusterFactorCurve::FACTOR_SCALE, // worst case: a flood import
            max_spends: 20,
            epoch_m: 7,
        }
    }
}

/// Effective factor from an import-cluster weight fraction: value-weighted blend
/// of `import_factor` (on the import-tagged share) and 1× (background) on the
/// rest — mirrors how the demurrage/fee kernels weight a coin's factor by its
/// tag composition.
fn effective_factor(import_weight: u32, import_factor_scaled: u64) -> u64 {
    let w = import_weight as u128;
    let scale = TAG_WEIGHT_SCALE as u128;
    let background = ClusterFactorCurve::FACTOR_SCALE as u128;
    let blended = (import_factor_scaled as u128 * w + background * (scale - w)) / scale;
    blended as u64
}

/// Run the decay-by-circulation experiment using the **real**
/// [`crate::TagVector::mix`] blend. The coin starts 100%-tagged to its import
/// cluster; each spend mixes in an incoming background-tagged payment, shifting
/// weight off the import tag exactly as the consensus blend does.
pub fn decay_by_circulation(params: &DecayParams) -> Vec<DecayRow> {
    let import_id = import_cluster_id(params.epoch_m);

    // The imported coin: 100% weight on its epoch import cluster.
    let mut tag = TagVector::single(import_id);
    // Track value in picocredits so `mix`'s value-weighting is faithful to
    // real magnitudes (mix widens to u128 internally).
    let coin_value_pico = params.coin_value_bth as u128 * PICO_PER_BTH;
    let mut held_value: u64 = u64::try_from(coin_value_pico).unwrap_or(u64::MAX);
    let incoming_value: u64 =
        (held_value as u128 * params.incoming_frac_bps as u128 / 10_000) as u64;

    // A purely-domestic incoming payment: empty tag vector = 100% background.
    let background_tag = TagVector::new();

    let mut rows = Vec::with_capacity(params.max_spends as usize + 1);
    let record = |spends: u64, tag: &TagVector| {
        let import_weight = tag.get(import_id);
        DecayRow {
            spends,
            import_weight,
            import_weight_frac: import_weight as f64 / TAG_WEIGHT_SCALE as f64,
            effective_factor_scaled: effective_factor(
                import_weight,
                params.import_factor_scaled,
            ),
        }
    };
    rows.push(record(0, &tag));

    for s in 1..=params.max_spends {
        // Receive a domestic (background) payment: the real value-weighted mix.
        tag.mix(held_value, &background_tag, incoming_value);
        held_value = held_value.saturating_add(incoming_value);
        rows.push(record(s, &tag));
    }
    rows
}

/// Confirm the intended invariant: a **pure-external holder who never receives
/// domestic money** never mixes, so its import tag stays at 100% and its
/// effective factor stays at the import factor (≥ F) forever. Returns the
/// effective factor after `spends` self-spends with NO domestic inflow (which
/// must equal the starting import factor).
pub fn pure_external_holder_factor(params: &DecayParams, spends: u64) -> u64 {
    let import_id = import_cluster_id(params.epoch_m);
    let mut tag = TagVector::single(import_id);
    let held = params.coin_value_bth;
    // Self-spends receive no domestic money: incoming background value is 0, so
    // `mix` is a no-op on the tag (total_value unchanged direction). We still
    // call it to prove the real blend leaves a never-mixing coin untouched.
    let background = TagVector::new();
    for _ in 0..spends {
        tag.mix(held, &background, 0);
    }
    effective_factor(tag.get(import_id), params.import_factor_scaled)
}

// ============================================================================
// Gini illustration: an import flood's effect on the domestic distribution
// ============================================================================

/// Show that pricing a bridge flood above background shrinks its **cheap-money
/// footprint** — the concentration signal the ADR wants. We compute the Gini of
/// the population's *effective-spendable* wealth (value ÷ factor, a proxy for
/// how much low-cost purchasing power each holder commands), with the flood
/// entering at 1× (ADR 0003 status quo) vs at its epoch factor ≥ `F` (ADR 0007).
/// Entering above background DIVIDES the flood's spendable footprint by its
/// factor, so the effective-spendable Gini *falls*: the flood no longer
/// dominates the cheap-money distribution the way a factor-1 entry would. The
/// gap between the two Ginis is the leak ADR 0007 narrows.
#[derive(Clone, Debug)]
pub struct GiniIllustration {
    pub floor_scaled: u64,
    /// Gini of effective-spendable wealth when the flood enters at factor-1
    /// (the ADR 0003 status quo — the entry leak).
    pub gini_flood_at_background: f64,
    /// Gini of effective-spendable wealth when the flood enters at the floor `F`
    /// (ADR 0007).
    pub gini_flood_at_floor: f64,
}

/// Population for the Gini illustration: many small domestic (factor-1) holders
/// plus one whale flood entrant.
fn gini_illustration(floor_scaled: u64) -> GiniIllustration {
    let n_domestic = 100usize;
    let domestic_bth = 1_000u64;
    let flood_bth = 2_000_000u64;
    let flood_factor = import_factor(flood_bth, floor_scaled);

    // Effective spendable wealth = value scaled down by factor (higher factor =
    // costlier to spend = less "cheap money"). We scale by FACTOR_SCALE to keep
    // integers for the shared Gini kernel.
    let eff = |value_bth: u64, factor_scaled: u64| -> u64 {
        (value_bth as u128 * ClusterFactorCurve::FACTOR_SCALE as u128 / factor_scaled as u128)
            as u64
    };

    let mut at_bg: Vec<u64> = vec![eff(domestic_bth, ClusterFactorCurve::FACTOR_SCALE); n_domestic];
    at_bg.push(eff(flood_bth, ClusterFactorCurve::FACTOR_SCALE)); // status quo: flood at 1×

    let mut at_floor: Vec<u64> =
        vec![eff(domestic_bth, ClusterFactorCurve::FACTOR_SCALE); n_domestic];
    at_floor.push(eff(flood_bth, flood_factor)); // ADR 0007: flood at F (or curve)

    GiniIllustration {
        floor_scaled,
        gini_flood_at_background: calculate_gini(&at_bg),
        gini_flood_at_floor: calculate_gini(&at_floor),
    }
}

// ============================================================================
// Full report
// ============================================================================

/// The whale import sizes swept in the split-game table, in BTH: a mid whale, a
/// large whale, and a mega-whale (the flood that would most dilute the domestic
/// distribution).
pub fn whale_import_sizes() -> Vec<u64> {
    vec![500_000, 2_000_000, 10_000_000]
}

/// The complete bridge-import sweep report.
#[derive(Clone, Debug)]
pub struct BridgeImportReport {
    pub epochs: Vec<Epoch>,
    pub floors: Vec<u64>,
    pub regimes: Vec<VolumeRegime>,
    pub whale_imports: Vec<u64>,
    /// Split-game table at the RECOMMENDED floor (see [`RECOMMENDED_FLOOR`]).
    pub split_game_rows: Vec<SplitGameRow>,
    /// Collateral table at the recommended floor for the small entrant.
    pub collateral_rows: Vec<CollateralRow>,
    /// Floor table (residual anti-hoarding vs onboarding friction).
    pub floor_rows: Vec<FloorRow>,
    /// Decay-by-circulation table (worst-case flood import, 6× start).
    pub decay_rows: Vec<DecayRow>,
    /// Pure-external-holder effective factor after many self-spends (must equal
    /// the import factor — the intended ≥ F invariant).
    pub pure_external_factor_scaled: u64,
    /// The import factor the decay experiment starts from (worst-case flood).
    pub decay_start_factor_scaled: u64,
    /// Gini illustration at the recommended floor.
    pub gini_illustration: GiniIllustration,
    /// Spends needed to blend the worst-case import coin below the halfway
    /// effective-factor point (import → background).
    pub decay_half_spends: Option<u64>,
    /// Spends needed to blend the worst-case import coin down to the recommended
    /// floor (`RECOMMENDED_FLOOR`) — the "how long until an imported coin is as
    /// cheap as a fresh honest import" milestone.
    pub decay_to_floor_spends: Option<u64>,
}

/// Recommended floor `F` = 1.5× (FACTOR_SCALE units). Rationale is derived from
/// the swept floor table in the doc; stated here so the report can key its
/// split-game / collateral tables to the recommendation.
pub const RECOMMENDED_FLOOR: u64 = 1500;

/// Recommended epoch length `K` = 1 day of blocks at the 5s reference.
pub const RECOMMENDED_EPOCH_BLOCKS: u64 = 17_280;

/// Run the complete bridge-import sweep with the default configuration.
pub fn run_bridge_import_sweep() -> BridgeImportReport {
    let epochs = candidate_epochs();
    let floors = candidate_floors();
    let regimes = volume_regimes();
    let whale_imports = whale_import_sizes();

    let split_game_rows = split_game_table(&epochs, RECOMMENDED_FLOOR, &whale_imports);
    let collateral_rows = collateral_table(&regimes, RECOMMENDED_FLOOR, SMALL_ENTRANT_BTH);
    let floor_rows = floor_table(&floors);

    // Decay experiment: worst case — a flood import that entered at 6×.
    let decay_params = DecayParams::default();
    let decay_rows = decay_by_circulation(&decay_params);
    let pure_external_factor_scaled = pure_external_holder_factor(&decay_params, 50);
    let decay_start_factor_scaled = decay_params.import_factor_scaled;

    // Spends to reach the halfway effective-factor point.
    let start = decay_rows.first().map(|r| r.effective_factor_scaled).unwrap_or(0);
    let halfway =
        (start + ClusterFactorCurve::FACTOR_SCALE) / 2;
    let decay_half_spends = decay_rows
        .iter()
        .find(|r| r.effective_factor_scaled <= halfway)
        .map(|r| r.spends);
    let decay_to_floor_spends = decay_rows
        .iter()
        .find(|r| r.effective_factor_scaled <= RECOMMENDED_FLOOR)
        .map(|r| r.spends);

    BridgeImportReport {
        epochs,
        floors,
        regimes,
        whale_imports,
        split_game_rows,
        collateral_rows,
        floor_rows,
        decay_rows,
        pure_external_factor_scaled,
        decay_start_factor_scaled,
        gini_illustration: gini_illustration(RECOMMENDED_FLOOR),
        decay_half_spends,
        decay_to_floor_spends,
    }
}

/// Format a FACTOR_SCALE factor as e.g. "1.500x".
fn fmt_factor(scaled: u64) -> String {
    format!("{:.3}x", scaled as f64 / ClusterFactorCurve::FACTOR_SCALE as f64)
}

/// Render the report as Markdown tables (the doc numbers are generated from
/// this, never hand-computed).
pub fn to_markdown(report: &BridgeImportReport) -> String {
    let mut s = String::new();

    // --- Split-game table -------------------------------------------------
    s.push_str("### Split-game cost by epoch length K (at the recommended floor F)\n\n");
    s.push_str(&format!(
        "Floor F = {} in effect. A whale that dumps its whole import in one epoch pays the \
         *undiluted* factor (the real curve at the full volume). To reach the floor it must \
         spread the import across enough epochs that each epoch's share prices out at the floor \
         — `epochs to floor` — and each epoch costs K blocks × 5 s of wall-clock time.\n\n",
        fmt_factor(RECOMMENDED_FLOOR),
    ));
    s.push_str("| K | whale import (BTH) | undiluted factor | epochs to floor | split-game cost |\n");
    s.push_str("|---|-------------------:|-----------------:|----------------:|----------------:|\n");
    for r in &report.split_game_rows {
        s.push_str(&format!(
            "| {} ({:.1}h) | {} | {} | {} | {:.1} days |\n",
            r.epoch.label,
            r.epoch.hours(),
            r.whale_import_bth,
            fmt_factor(r.undiluted_factor_scaled),
            r.epochs_to_floor,
            r.split_game_days,
        ));
    }
    s.push('\n');

    // --- Collateral table -------------------------------------------------
    s.push_str("### Innocent small-entrant collateral in a co-occurring flood epoch\n\n");
    s.push_str(&format!(
        "A genuine small entrant unwraps {} BTH. Alone, it sits low on the curve and pays \
         the floor F = {} (the floor binds because it exceeds the coin's natural curve factor). \
         But if a whale flood lands in the SAME epoch, the entrant \
         inherits the shared pool's factor `curve(small + flood)`. This is the collateral the \
         shared-fate coupling imposes (K governs how *likely* a co-occurrence is; this table \
         quantifies the *severity* when it happens).\n\n",
        SMALL_ENTRANT_BTH,
        fmt_factor(RECOMMENDED_FLOOR),
    ));
    s.push_str("| flood regime | flood volume (BTH) | entrant alone | entrant caught | collateral ratio |\n");
    s.push_str("|--------------|-------------------:|--------------:|---------------:|-----------------:|\n");
    for r in &report.collateral_rows {
        s.push_str(&format!(
            "| {} | {} | {} | {} | {:.3}x |\n",
            r.regime.label,
            r.regime.epoch_volume_bth,
            fmt_factor(r.alone_factor_scaled),
            fmt_factor(r.caught_factor_scaled),
            r.collateral_ratio,
        ));
    }
    s.push('\n');

    // --- Floor table ------------------------------------------------------
    s.push_str("### Import-factor floor F sweep: residual anti-hoarding vs onboarding friction\n\n");
    s.push_str(&format!(
        "For each candidate F: the effective factor a genuine small entrant ({} BTH) pays (the \
         onboarding toll — `max(F, curve(small))`; a 1,000-BTH import is ~1.265× on the raw \
         curve, so the floor binds once F exceeds that); the \
         money-terms toll (one year of demurrage at that factor, % of coin value); the floor a \
         fully split-gamed whale bottoms out at (also F — this is the residual anti-hoarding \
         that survives gaming); and `residual above background` = how far F sits above factor-1 \
         normalized to the 6×−1× span (0 = indistinguishable from background, the entry leak; \
         1 = maximally taxed).\n\n",
        SMALL_ENTRANT_BTH,
    ));
    s.push_str("| F | small-entrant factor | annual toll (% of value) | gamed-whale floor | residual above background |\n");
    s.push_str("|---|---------------------:|-------------------------:|------------------:|--------------------------:|\n");
    for r in &report.floor_rows {
        s.push_str(&format!(
            "| {} | {} | {:.4}% | {} | {:.3} |\n",
            fmt_factor(r.floor_scaled),
            fmt_factor(r.small_entrant_factor_scaled),
            r.small_entrant_annual_toll_pct,
            fmt_factor(r.gamed_whale_floor_factor_scaled),
            r.residual_above_background,
        ));
    }
    s.push('\n');

    // --- Decay table ------------------------------------------------------
    s.push_str("### Decay by circulation (real `TagVector::mix` value-weighted blend)\n\n");
    s.push_str(&format!(
        "A worst-case flood import that entered at {} (6×), receiving one payment of its own \
         value in domestic (background) money per spend. The real consensus blend shifts weight \
         off the import cluster toward background; the effective factor is the value-weighted \
         blend of the import factor (on the remaining import-tagged fraction) and 1× (background) \
         on the rest. No time-based decay — only *circulation* normalizes.\n\n",
        fmt_factor(report.decay_start_factor_scaled),
    ));
    s.push_str("| domestic-mixing spends | import weight | import weight (frac) | effective factor |\n");
    s.push_str("|-----------------------:|-------------:|---------------------:|-----------------:|\n");
    for r in &report.decay_rows {
        s.push_str(&format!(
            "| {} | {} | {:.4} | {} |\n",
            r.spends,
            r.import_weight,
            r.import_weight_frac,
            fmt_factor(r.effective_factor_scaled),
        ));
    }
    s.push('\n');
    if let Some(h) = report.decay_half_spends {
        s.push_str(&format!(
            "Spends to reach the halfway effective-factor point (import → background): **{h}** \
             (the first spend already halves the import weight when the coin receives its own \
             value in domestic money).\n"
        ));
    }
    if let Some(f) = report.decay_to_floor_spends {
        s.push_str(&format!(
            "Spends to blend down to the recommended floor {} (as cheap as a fresh honest \
             import): **{f}**.\n\n",
            fmt_factor(RECOMMENDED_FLOOR),
        ));
    } else {
        s.push('\n');
    }
    s.push_str(&format!(
        "**Pure-external-holder invariant:** a holder that NEVER receives domestic money \
         (50 self-spends, zero domestic inflow) keeps effective factor {} — unchanged from its \
         import factor. Imported wealth normalizes ONLY by circulating; sitting idle or churning \
         among external-only holders never reaches background (intended, ADR 0007 §4).\n\n",
        fmt_factor(report.pure_external_factor_scaled),
    ));

    // --- Gini illustration ------------------------------------------------
    s.push_str("### Concentration signal: flood's cheap-money footprint\n\n");
    s.push_str(&format!(
        "Gini of the population's effective-spendable wealth (value ÷ factor — how much \
         low-cost purchasing power each holder commands) for 100 domestic factor-1 holders \
         (1,000 BTH each) + one 2,000,000-BTH bridge flood. Under ADR 0003 the flood enters at \
         1× (background) and dominates the cheap-money distribution; under ADR 0007 it enters at \
         its epoch factor ≥ F = {}, which DIVIDES its spendable footprint by that factor. The \
         effective-spendable Gini therefore FALLS — the flood's cheap-money dominance is priced \
         down. The gap between the two rows is the entry leak ADR 0007 narrows.\n\n",
        fmt_factor(report.gini_illustration.floor_scaled),
    ));
    s.push_str("| flood enters at | Gini (effective-spendable) |\n");
    s.push_str("|-----------------|---------------------------:|\n");
    s.push_str(&format!(
        "| factor-1 (ADR 0003 status quo) | {:.4} |\n",
        report.gini_illustration.gini_flood_at_background,
    ));
    s.push_str(&format!(
        "| floor F (ADR 0007) | {:.4} |\n\n",
        report.gini_illustration.gini_flood_at_floor,
    ));

    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_factor_uses_real_curve_and_floor() {
        // A tiny import is below the knee: factor pinned to the floor.
        assert_eq!(import_factor(1_000, 1500), 1500);
        // A huge import saturates the curve well above any candidate floor.
        assert!(import_factor(10_000_000, 1000) > 5000);
        // The floor never LOWERS a curve factor that already exceeds it.
        assert!(import_factor(10_000_000, 1500) > 5000);
    }

    #[test]
    fn split_game_costs_scale_with_epoch_length() {
        // For a fixed whale + floor, a longer K costs strictly more wall-clock
        // time to dilute (same epochs-to-floor, more time per epoch).
        let epochs = candidate_epochs();
        let rows = split_game_table(&epochs, RECOMMENDED_FLOOR, &[2_000_000]);
        let mut last_days = -1.0;
        let mut last_n = 0u64;
        for r in &rows {
            if last_n != 0 {
                assert_eq!(r.epochs_to_floor, last_n, "epochs-to-floor is K-independent");
            }
            last_n = r.epochs_to_floor;
            assert!(
                r.split_game_days > last_days,
                "longer K must cost more wall-clock time to split-game"
            );
            last_days = r.split_game_days;
        }
        // Diluting a 2M-BTH whale to the floor must require MORE than one epoch
        // (otherwise the epoch key provides no split resistance).
        assert!(rows[0].epochs_to_floor > 1);
    }

    #[test]
    fn collateral_rises_with_flood_volume() {
        let regimes = volume_regimes();
        let rows = collateral_table(&regimes, RECOMMENDED_FLOOR, SMALL_ENTRANT_BTH);
        // Alone, the small entrant pays exactly the floor.
        assert_eq!(rows[0].alone_factor_scaled, RECOMMENDED_FLOOR);
        // A bigger co-occurring flood means bigger collateral.
        let low = rows.iter().find(|r| r.regime.label == "low").unwrap();
        let high = rows.iter().find(|r| r.regime.label == "high").unwrap();
        assert!(high.caught_factor_scaled >= low.caught_factor_scaled);
        assert!(high.collateral_ratio >= 1.0);
    }

    #[test]
    fn floor_trades_residual_against_onboarding() {
        let rows = floor_table(&candidate_floors());
        // At F = 1.0× the floor never binds, so the small entrant pays its
        // NATURAL curve factor (a 1,000-BTH import is ~1.265× on the real
        // curve, NOT 1.0× — a finding worth surfacing). `residual above
        // background` measures the floor's distance from 1×, so it is 0 here.
        let no_floor = rows.iter().find(|r| r.floor_scaled == 1000).unwrap();
        assert!(
            no_floor.small_entrant_factor_scaled > 1000,
            "a 1,000-BTH import is above 1x on the real curve even with no floor"
        );
        assert!(no_floor.residual_above_background.abs() < 1e-9);
        // Higher floor: non-decreasing residual AND non-decreasing toll (the
        // trade-off the sweep exists to expose). The toll is flat until the
        // floor overtakes the coin's natural curve factor, then rises.
        let mut last_res = -1.0;
        let mut last_toll = -1.0;
        for r in &rows {
            assert!(r.residual_above_background > last_res - 1e-12);
            assert!(r.small_entrant_annual_toll_pct > last_toll - 1e-12);
            last_res = r.residual_above_background;
            last_toll = r.small_entrant_annual_toll_pct;
        }
        // The recommended 1.5× floor binds a small entrant to exactly 1.5×.
        let rec = rows.iter().find(|r| r.floor_scaled == RECOMMENDED_FLOOR).unwrap();
        assert_eq!(rec.small_entrant_factor_scaled, RECOMMENDED_FLOOR);
        // A 2× floor sits above zero residual.
        let two_x = rows.iter().find(|r| r.floor_scaled == 2000).unwrap();
        assert!(two_x.residual_above_background > 0.0);
    }

    #[test]
    fn decay_normalizes_toward_background_via_real_blend() {
        let params = DecayParams::default();
        let rows = decay_by_circulation(&params);
        // Monotone non-increasing effective factor as the coin circulates.
        for w in rows.windows(2) {
            assert!(
                w[1].effective_factor_scaled <= w[0].effective_factor_scaled,
                "circulation must not RAISE the effective factor"
            );
        }
        // Starts near the import factor, ends much closer to background.
        assert!(rows.first().unwrap().effective_factor_scaled >= 5000);
        assert!(
            rows.last().unwrap().effective_factor_scaled
                < rows.first().unwrap().effective_factor_scaled,
            "circulation must reduce the effective factor"
        );
    }

    #[test]
    fn pure_external_holder_stays_at_import_factor() {
        // The intended ADR 0007 §4 invariant: a holder who never receives
        // domestic money never normalizes — its effective factor equals its
        // import factor even after many self-spends.
        let params = DecayParams::default();
        let f = pure_external_holder_factor(&params, 100);
        assert_eq!(
            f, params.import_factor_scaled,
            "a never-mixing coin must stay at its import factor (>= F)"
        );
    }

    #[test]
    fn gini_illustration_shows_flood_footprint_shrinks() {
        // Pricing the flood above background DIVIDES its cheap-money footprint
        // by its factor, so the population's effective-spendable Gini FALLS: the
        // flood no longer dominates the cheap-money distribution. The gap is the
        // entry leak ADR 0007 narrows.
        let g = gini_illustration(RECOMMENDED_FLOOR);
        assert!(
            g.gini_flood_at_floor < g.gini_flood_at_background,
            "pricing the flood above background must shrink its cheap-money footprint \
             (effective-spendable Gini falls): status quo {} vs ADR 0007 {}",
            g.gini_flood_at_background,
            g.gini_flood_at_floor,
        );
    }

    #[test]
    fn report_is_reproducible() {
        let a = to_markdown(&run_bridge_import_sweep());
        let b = to_markdown(&run_bridge_import_sweep());
        assert_eq!(a, b, "sweep output must be byte-for-byte deterministic");
    }

    #[test]
    fn markdown_contains_all_epochs_floors_and_sections() {
        let md = to_markdown(&run_bridge_import_sweep());
        for e in candidate_epochs() {
            assert!(md.contains(e.label), "missing epoch {} in report", e.label);
        }
        assert!(md.contains("Split-game cost"));
        assert!(md.contains("collateral"));
        assert!(md.contains("Decay by circulation"));
        assert!(md.contains("Pure-external-holder invariant"));
    }
}
