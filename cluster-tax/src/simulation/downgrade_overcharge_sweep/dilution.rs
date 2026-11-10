//! Adversarial-dilution mode of the downgrade-overcharge sweep (issue #1686).
//!
//! # The question
//!
//! The #925 downgrade charge floors the spender's declared output factor at a
//! factor implied by the ring. Before protocol 10.0.0 the node computed that
//! floor with [`crate::ring_centroid_implied_factor`], the **value-weighted
//! mean** of the ring members' cluster wealth, pooled over every input. The
//! spender picks the decoys, so padding a ring with high-value background
//! decoys pulls the floor toward 1× and dilutes the charge. This module
//! measures four candidate floor rules side by side, plus the rule the
//! operator ruled for 10.0.0 as the node runs it:
//!
//! | id | rule |
//! |---|---|
//! | (a) `mean` | pre-10.0.0 node: value-weighted mean of member wealth (the kernel itself) |
//! | (b) `match-mean` | value-weighted mean over members whose amount equals `pseudo_output_amount` |
//! | (b) `match-max` | maximum over members whose amount equals `pseudo_output_amount` |
//! | (c) `max` | maximum member factor over the whole ring (`ring_cluster_factor`'s rule) |
//! | (d) `p50` / `p90` / `p95` | unweighted nearest-rank quantile of member factors |
//! | (d) `vw-median` | value-weighted median of member wealth |
//! | `node 10.0.0` | the production kernel [`crate::amount_matched_ring_floor_wealth`] (#1686 ruling: (b) `match-max`, per input, value-weighted by pseudo amount) |
//!
//! Every rule returns a **floor wealth**, which goes through the production
//! [`ClusterFactorCurve`] once. The curve is monotone, so the order statistics
//! of wealth and of factor coincide (`floor_rules_agree_with_kernels` pins
//! that, pins rule (a) to the old kernel bit for bit, and pins `node 10.0.0`
//! to (b) `match-max` composed per input, `node_rule_is_per_input_match_max`).
//! The charge is
//! the node's own [`crate::spend_demurrage_charge`] with the node's composition
//! `max(claimed, floor)`.
//!
//! # What is measured
//!
//! 1. **Dilution** (the acceptance grid from #1686): a 1,000 BTH holding in a
//!    10M-BTH cluster is split into `k ∈ {1, 10, 100, 1000}` coins, and each
//!    coin is spent young to a background output in a ring of 20 whose 19
//!    decoys are background coins worth `D ∈ {10, 2k, 100k}` BTH. An extra
//!    column sets `D` to the coin's own amount, the best response against the
//!    amount-matched rules. The metric is the charge collected as a share of
//!    the undiluted charge (the floor at the real coin's own factor). #1686's
//!    pass bar is ≥ 95% in every cell.
//! 2. **Honest false positives** on rings drawn by the production decoy pickers
//!    (the #1757 `node`, `cli` and `web` routes, via
//!    [`sample_route_ring_ages`]). Each decoy's amount and cluster wealth come
//!    from a deterministic per-output population model; the real input comes
//!    from the shipped agent archetypes ([`super::honest_archetypes`]). The
//!    declared output carries the production wallet's 5% per-hop tag decay
//!    (`DEFAULT_CLUSTER_DECAY_RATE = 50_000`). The reference is an **oracle**
//!    floor at the real input's own wealth: the charge a rule would levy if it
//!    knew the real input. A false positive is a charge above the oracle's;
//!    "collection" is the share of the oracle's total charge a rule levies.
//! 3. **Privacy and decoy diversity** on the same rings: how many members share
//!    the real input's amount (the public-amount leak #902 documents), and how
//!    the minimal decoy filter that avoids max-rule over-charges shrinks the
//!    pool and changes a max-factor adversary's hit rate.
//! 4. **Multi-input composition**: pooling every input's ring into one
//!    statistic (the pre-10.0.0 node) against a per-input floor composed by the
//!    public `pseudo_output_amount`s (the 10.0.0 node), for an honest two-input
//!    spend that blends a wealthy and a background coin. The `node 10.0.0` rule
//!    composes per input by construction, so it shows the same value in both
//!    columns.
//! 5. **Confidential-amounts compatibility** (#902, ADR 0009): which public
//!    data each rule reads.
//!
//! All randomness is seeded (`SEED = 0xB07A_1686`), so the report regenerates
//! byte for byte (`report_is_reproducible`).

use super::{honest_archetypes, HonestArchetype};
use crate::{
    fee_curve::PICO_PER_BTH,
    simulation::{
        privacy_routes::{sample_route_ring_ages, ChainModel, DecoyRoute, RealAgeSpec},
        settlement_horizon_sweep::{factor_classes, BLOCKS_PER_YEAR, RATE_BPS},
    },
    spend_demurrage_charge,
    wide::WeightedMean,
    ClusterFactorCurve,
};
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;

/// Deterministic seed for this mode (distinct from the parent's `0xB07A_0925`).
pub const SEED: u64 = 0xB07A_1686;

/// The attacker's cluster wealth (BTH): the 10M-BTH cluster of #1686's table.
pub const ATTACK_CLUSTER_WEALTH_BTH: u64 = 10_000_000;

/// The attacker's total holding (BTH) before the split.
pub const ATTACK_TOTAL_VALUE_BTH: u64 = 1_000;

/// Production wallet per-hop tag decay, parts per million
/// (`DEFAULT_CLUSTER_DECAY_RATE` in `botho/src/wallet.rs` and
/// `botho-wallet/src/transaction.rs`).
pub const PRODUCTION_DECAY_PPM: u32 = 50_000;

/// Honest rings drawn per (route, population) row.
pub const HONEST_RINGS: usize = 4_000;

/// Honest two-input spends drawn for the composition table.
pub const MULTI_INPUT_SPENDS: usize = 2_000;

/// Split counts of the #1686 grid.
pub fn split_counts() -> Vec<u64> {
    vec![1, 10, 100, 1_000]
}

/// Decoy values (BTH) of the #1686 grid.
pub fn decoy_values_bth() -> Vec<u64> {
    vec![10, 2_000, 100_000]
}

/// Round amounts (BTH) an output snaps to when the population model's
/// round-amount share fires. Exact equality is what the amount-matched rules
/// key on, so round amounts are the only honest channel for a decoy to share
/// the real input's amount.
pub const ROUND_AMOUNTS_BTH: [u64; 6] = [1, 10, 100, 1_000, 10_000, 100_000];

fn bth_to_pico(bth: u64) -> u64 {
    u64::try_from(bth as u128 * PICO_PER_BTH).unwrap_or(u64::MAX)
}

fn wealth_pico(bth: u64) -> u128 {
    bth as u128 * PICO_PER_BTH
}

// ===========================================================================
// Candidate floor rules
// ===========================================================================

/// A candidate ring floor rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloorRule {
    /// (a) Value-weighted mean of member wealth: the shipped kernel.
    Mean,
    /// (b) Value-weighted mean over members whose amount equals the input's
    /// `pseudo_output_amount`.
    MatchedMean,
    /// (b) Maximum over members whose amount equals `pseudo_output_amount`.
    MatchedMax,
    /// (c) Maximum over the whole ring.
    Max,
    /// (d) Unweighted nearest-rank quantile of member wealth, in basis points
    /// (same convention as [`crate::ring_elapsed_quantile`]).
    Quantile(u32),
    /// (d) Value-weighted median of member wealth.
    WeightedMedian,
    /// The protocol 10.0.0 node rule, computed by the production kernel
    /// [`crate::amount_matched_ring_floor_wealth`] (#1686). It composes inputs
    /// itself (per input, by pseudo amount).
    Node,
}

impl FloorRule {
    /// Short column label.
    pub fn label(&self) -> String {
        match self {
            FloorRule::Mean => "(a) mean".into(),
            FloorRule::MatchedMean => "(b) match-mean".into(),
            FloorRule::MatchedMax => "(b) match-max".into(),
            FloorRule::Max => "(c) max".into(),
            FloorRule::Quantile(q) => format!("(d) p{}", q / 100),
            FloorRule::WeightedMedian => "(d) vw-median".into(),
            FloorRule::Node => "node 10.0.0".into(),
        }
    }

    /// Whether the rule reads member amounts (hidden under CT).
    pub fn reads_member_amounts(&self) -> bool {
        matches!(
            self,
            FloorRule::Mean
                | FloorRule::MatchedMean
                | FloorRule::MatchedMax
                | FloorRule::WeightedMedian
                | FloorRule::Node
        )
    }

    /// Whether the rule reads the input's `pseudo_output_amount` (a commitment
    /// under CT).
    pub fn reads_pseudo_amount(&self) -> bool {
        matches!(
            self,
            FloorRule::MatchedMean | FloorRule::MatchedMax | FloorRule::Node
        )
    }
}

/// The rules compared, in column order.
pub fn candidate_rules() -> Vec<FloorRule> {
    vec![
        FloorRule::Mean,
        FloorRule::MatchedMean,
        FloorRule::MatchedMax,
        FloorRule::Max,
        FloorRule::Quantile(5_000),
        FloorRule::Quantile(9_000),
        FloorRule::Quantile(9_500),
        FloorRule::WeightedMedian,
        FloorRule::Node,
    ]
}

/// The floor wealth a rule implies for one ring.
///
/// `members` are `(amount_pico, effective_cluster_wealth_pico)`, as the node
/// resolves them; `pseudo` is the input's `pseudo_output_amount`. The matched
/// rules fall back to the whole ring when no member matches; consensus rejects
/// such a transaction (`verify_ring_members`), so the fallback is unreachable
/// on valid input.
pub fn floor_wealth(rule: FloorRule, members: &[(u64, u128)], pseudo: u64) -> u128 {
    let matched: Vec<(u64, u128)> = members
        .iter()
        .copied()
        .filter(|&(v, _)| v == pseudo)
        .collect();
    let matched: &[(u64, u128)] = if matched.is_empty() {
        members
    } else {
        &matched
    };
    match rule {
        FloorRule::Mean => weighted_mean(members),
        FloorRule::MatchedMean => weighted_mean(matched),
        FloorRule::MatchedMax => matched.iter().map(|&(_, w)| w).max().unwrap_or(0),
        FloorRule::Node => crate::amount_matched_ring_floor_wealth(&[crate::RingFloorInput {
            pseudo_amount: pseudo,
            members,
        }]),
        FloorRule::Max => members.iter().map(|&(_, w)| w).max().unwrap_or(0),
        FloorRule::Quantile(q) => {
            if members.is_empty() {
                return 0;
            }
            let mut ws: Vec<u128> = members.iter().map(|&(_, w)| w).collect();
            ws.sort_unstable();
            let n = ws.len() as u64;
            let rank = (q.min(10_000) as u64 * n).div_ceil(10_000);
            ws[rank.saturating_sub(1).min(n - 1) as usize]
        }
        FloorRule::WeightedMedian => {
            let mut ms: Vec<(u64, u128)> = members.to_vec();
            ms.sort_unstable_by_key(|&(v, w)| (w, v));
            let total: u128 = ms.iter().map(|&(v, _)| v as u128).sum();
            if total == 0 {
                return 0;
            }
            let mut cum: u128 = 0;
            for &(v, w) in &ms {
                cum += v as u128;
                if 2 * cum >= total {
                    return w;
                }
            }
            ms.last().map(|&(_, w)| w).unwrap_or(0)
        }
    }
}

fn weighted_mean(members: &[(u64, u128)]) -> u128 {
    let mut mean = WeightedMean::new();
    for &(value, wealth) in members {
        mean.add_product(value as u128, wealth);
        mean.add_weight(value as u128);
    }
    mean.floor()
}

/// The floor factor (FACTOR_SCALE units) a rule implies for one ring.
pub fn floor_factor(
    rule: FloorRule,
    members: &[(u64, u128)],
    pseudo: u64,
    curve: &ClusterFactorCurve,
) -> u64 {
    curve.factor(floor_wealth(rule, members, pseudo))
}

/// The node's demurrage charge for a spend whose ring floor is `floor` and
/// whose declared output factor is `claimed`: `spend_demurrage_charge` with
/// the node's `max(claimed, floor)` composition.
pub fn node_charge(value: u64, floor: u64, claimed: u64, elapsed: u64) -> u64 {
    spend_demurrage_charge(
        value,
        floor.max(claimed),
        claimed,
        elapsed,
        RATE_BPS,
        BLOCKS_PER_YEAR,
    )
}

// ===========================================================================
// 1. Adversarial-dilution grid
// ===========================================================================

/// One cell of the dilution grid.
#[derive(Clone, Debug)]
pub struct DilutionCell {
    pub split_k: u64,
    /// Decoy value in BTH, or `None` for the amount-matched column (decoys
    /// worth exactly the coin's amount).
    pub decoy_value_bth: Option<u64>,
    /// The spent coin's value in picocredits.
    pub coin_value_pico: u64,
    /// Per rule: (floor factor, charge collected / undiluted charge).
    pub per_rule: Vec<(u64, f64)>,
}

/// The dilution grid plus its reference numbers.
#[derive(Clone, Debug)]
pub struct DilutionGrid {
    pub rules: Vec<FloorRule>,
    pub ring_size: usize,
    /// Undiluted floor: the real coin's own factor.
    pub undiluted_factor: u64,
    /// Undiluted charge as a share of the coin's value.
    pub undiluted_pct: f64,
    pub cells: Vec<DilutionCell>,
}

impl DilutionGrid {
    /// Worst (lowest) collected share per rule over every cell.
    pub fn worst_per_rule(&self) -> Vec<f64> {
        (0..self.rules.len())
            .map(|i| {
                self.cells
                    .iter()
                    .map(|c| c.per_rule[i].1)
                    .fold(f64::INFINITY, f64::min)
            })
            .collect()
    }
}

/// Run the adversarial-dilution grid.
pub fn dilution_grid(ring_size: usize, curve: &ClusterFactorCurve) -> DilutionGrid {
    let rules = candidate_rules();
    let w = wealth_pico(ATTACK_CLUSTER_WEALTH_BTH);
    let undiluted_factor = curve.factor(w);
    let background = curve.factor(0);
    let mut cells = Vec::new();
    for k in split_counts() {
        let coin = bth_to_pico(ATTACK_TOTAL_VALUE_BTH) / k;
        let columns: Vec<Option<u64>> = decoy_values_bth()
            .into_iter()
            .map(Some)
            .chain(std::iter::once(None))
            .collect();
        for d in columns {
            let decoy_value = d.map(bth_to_pico).unwrap_or(coin);
            let mut ring = vec![(coin, w)];
            ring.extend(std::iter::repeat_n((decoy_value, 0u128), ring_size - 1));
            // Young coin (elapsed 0): only the capitalized downgrade term binds.
            let undiluted = node_charge(coin, undiluted_factor, background, 0);
            let per_rule = rules
                .iter()
                .map(|&r| {
                    let f = floor_factor(r, &ring, coin, curve);
                    let c = node_charge(coin, f, background, 0);
                    let share = if undiluted == 0 {
                        1.0
                    } else {
                        c as f64 / undiluted as f64
                    };
                    (f, share)
                })
                .collect();
            cells.push(DilutionCell {
                split_k: k,
                decoy_value_bth: d,
                coin_value_pico: coin,
                per_rule,
            });
        }
    }
    let one = bth_to_pico(ATTACK_TOTAL_VALUE_BTH);
    let undiluted_pct =
        node_charge(one, undiluted_factor, background, 0) as f64 / one as f64 * 100.0;
    DilutionGrid {
        rules,
        ring_size,
        undiluted_factor,
        undiluted_pct,
        cells,
    }
}

// ===========================================================================
// 2. Honest population on the production decoy routes
// ===========================================================================

/// The UTXO population the production pickers draw decoys from. Pickers read
/// only ages, so a decoy's amount and wealth are a deterministic function of
/// its output id (independent of age).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PopulationModel {
    /// Share of outputs carrying a wealthy cluster tag, basis points.
    pub contamination_bps: u32,
    /// Declared-output tag decay of honest spends, parts per million.
    pub decay_ppm: u32,
    /// Share of amounts that snap to a round value, basis points.
    pub round_share_bps: u32,
}

impl PopulationModel {
    fn label(&self) -> String {
        format!(
            "c={}%, decay={}%, round={}%",
            fmt_bps(self.contamination_bps),
            fmt_bps(self.decay_ppm / 100),
            fmt_bps(self.round_share_bps)
        )
    }
}

fn fmt_bps(bps: u32) -> String {
    if bps % 100 == 0 {
        format!("{}", bps / 100)
    } else {
        format!("{:.1}", bps as f64 / 100.0)
    }
}

/// Wealthy cluster classes (BTH) a contaminating output is drawn from: the
/// settlement sweep's six classes plus the 50M-BTH cluster the structuring sim
/// uses, so whales (10M) can meet a wealthier decoy.
pub fn wealthy_classes_bth() -> Vec<u64> {
    let mut v: Vec<u64> = factor_classes()
        .iter()
        .map(|c| c.cluster_wealth_bth)
        .collect();
    v.push(50_000_000);
    v
}

fn splitmix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Snap `bth` to the nearest round amount (log distance).
fn snap_round(bth: u64) -> u64 {
    let x = (bth.max(1) as f64).ln();
    *ROUND_AMOUNTS_BTH
        .iter()
        .min_by(|a, b| {
            let da = ((**a as f64).ln() - x).abs();
            let db = ((**b as f64).ln() - x).abs();
            da.partial_cmp(&db).expect("finite")
        })
        .expect("non-empty")
}

/// Draw one output's `(amount_pico, wealth_pico)` from the population.
fn draw_output(model: &PopulationModel, classes: &[u64], rng: &mut ChaCha8Rng) -> (u64, u128) {
    let wealthy = rng.random_range(0..10_000u32) < model.contamination_bps;
    let (mut bth, wealth) = if wealthy {
        let class = classes[rng.random_range(0..classes.len())];
        (rng.random_range(5_000..=100_000u64), wealth_pico(class))
    } else {
        (rng.random_range(10..=2_000u64), 0)
    };
    if rng.random_range(0..10_000u32) < model.round_share_bps {
        bth = snap_round(bth);
    }
    (bth_to_pico(bth), wealth)
}

/// The attributes of output `id` (deterministic in `seed`, `id`, `model`).
fn output_attrs(seed: u64, id: u64, model: &PopulationModel, classes: &[u64]) -> (u64, u128) {
    let mut rng = ChaCha8Rng::seed_from_u64(splitmix64(seed ^ splitmix64(id)));
    draw_output(model, classes, &mut rng)
}

fn draw_archetype<'a>(
    archetypes: &'a [HonestArchetype],
    rng: &mut ChaCha8Rng,
) -> &'a HonestArchetype {
    let total: u32 = archetypes.iter().map(|a| a.weight).sum();
    let mut pick = rng.random_range(0..total);
    for a in archetypes {
        if pick < a.weight {
            return a;
        }
        pick -= a.weight;
    }
    archetypes.last().expect("non-empty")
}

/// One honest single-input spend on a production-route ring.
#[derive(Clone, Debug)]
struct HonestSpend {
    is_background: bool,
    value: u64,
    real_wealth: u128,
    claimed_factor: u64,
    elapsed: u64,
    /// Ring members, real input first.
    ring: Vec<(u64, u128)>,
}

fn route_tag(route: DecoyRoute) -> u64 {
    match route {
        DecoyRoute::Node => 1,
        DecoyRoute::Cli => 2,
        DecoyRoute::Web => 3,
        DecoyRoute::WebPre1735 => 4,
    }
}

fn honest_spends(
    chain: &ChainModel,
    route: DecoyRoute,
    model: &PopulationModel,
    n: usize,
    curve: &ClusterFactorCurve,
) -> Vec<HonestSpend> {
    let archetypes = honest_archetypes();
    let classes = wealthy_classes_bth();
    let rings = sample_route_ring_ages(chain, route, RealAgeSpec::GammaSpend, n, SEED);
    let mut rng = ChaCha8Rng::seed_from_u64(splitmix64(SEED ^ route_tag(route)));
    rings
        .iter()
        .map(|r| {
            let arch = draw_archetype(&archetypes, &mut rng);
            let mut bth = arch.coin_value_bth;
            if rng.random_range(0..10_000u32) < model.round_share_bps {
                bth = snap_round(bth);
            }
            let value = bth_to_pico(bth);
            let real_wealth = wealth_pico(arch.spender_cluster_wealth_bth);
            let declared = wealth_pico(arch.output_cluster_wealth_bth)
                * (1_000_000 - model.decay_ppm as u128)
                / 1_000_000;
            let decoy_ids: Vec<u64> = r.decoy_ages.iter().map(|&a| chain.id_at(a, 0)).collect();
            let mut ring = vec![(value, real_wealth)];
            ring.extend(
                decoy_ids
                    .iter()
                    .map(|&id| output_attrs(SEED, id, model, &classes)),
            );
            // The node's clock: the oldest ring member's age
            // (`ring_elapsed_quantile` at 10000 bps).
            let elapsed = r.decoy_ages.iter().copied().fold(r.real_age, u64::max);
            HonestSpend {
                is_background: arch.is_background,
                value,
                real_wealth,
                claimed_factor: curve.factor(declared),
                elapsed,
                ring,
            }
        })
        .collect()
}

/// Per-rule honest statistics for one row.
#[derive(Clone, Debug, Default)]
pub struct HonestRuleStat {
    /// Share of spends charged above the oracle.
    pub fp_rate: f64,
    /// Mean over-charge on those spends, % of the coin's value.
    pub mean_excess_pct: f64,
    /// Total over-charge as % of all honest value moved.
    pub aggregate_excess_pct: f64,
    /// Total charge levied / total oracle charge (1.0 = exact; < 1 means
    /// honest decay downgrades go under-priced).
    pub collection: f64,
}

/// One honest row: a route under a population model.
#[derive(Clone, Debug)]
pub struct HonestRow {
    pub route: DecoyRoute,
    pub model: PopulationModel,
    pub spends: usize,
    pub per_rule: Vec<HonestRuleStat>,
}

fn honest_stats(
    spends: &[HonestSpend],
    rules: &[FloorRule],
    curve: &ClusterFactorCurve,
) -> Vec<HonestRuleStat> {
    let total_value: f64 = spends.iter().map(|s| s.value as f64).sum();
    let oracle: Vec<u64> = spends
        .iter()
        .map(|s| {
            node_charge(
                s.value,
                curve.factor(s.real_wealth),
                s.claimed_factor,
                s.elapsed,
            )
        })
        .collect();
    let oracle_total: f64 = oracle.iter().map(|&c| c as f64).sum();
    rules
        .iter()
        .map(|&rule| {
            let mut fp = 0usize;
            let mut excess_pct_sum = 0.0f64;
            let mut excess_total = 0.0f64;
            let mut levied_total = 0.0f64;
            for (s, &o) in spends.iter().zip(&oracle) {
                let f = floor_factor(rule, &s.ring, s.value, curve);
                let c = node_charge(s.value, f, s.claimed_factor, s.elapsed);
                levied_total += c as f64;
                if c > o {
                    fp += 1;
                    let ex = (c - o) as f64;
                    excess_pct_sum += ex / s.value as f64 * 100.0;
                    excess_total += ex;
                }
            }
            HonestRuleStat {
                fp_rate: fp as f64 / spends.len().max(1) as f64,
                mean_excess_pct: if fp > 0 {
                    excess_pct_sum / fp as f64
                } else {
                    0.0
                },
                aggregate_excess_pct: excess_total / total_value * 100.0,
                collection: if oracle_total > 0.0 {
                    levied_total / oracle_total
                } else {
                    1.0
                },
            }
        })
        .collect()
}

// ===========================================================================
// 3. Privacy and decoy diversity
// ===========================================================================

/// Privacy measurements for one (route, population, spender kind) row.
#[derive(Clone, Debug)]
pub struct PrivacyRow {
    pub route: DecoyRoute,
    pub model: PopulationModel,
    /// "background" or "wealthy".
    pub spender: &'static str,
    pub rings: usize,
    /// Mean number of ring members whose amount equals the real input's
    /// (`pseudo_output_amount`), real input included.
    pub mean_amount_matches: f64,
    /// Share of rings where the real input is the ONLY amount match (public
    /// amounts identify it outright).
    pub unique_amount_match: f64,
    /// Share of rings with at least one decoy wealthier than the real input
    /// (the max rule over-charges every one of them).
    pub wealthier_decoy_rate: f64,
    /// Max-factor adversary (guess the wealthiest member, ties split) on the
    /// production picker's rings.
    pub max_adversary_id: f64,
    /// Share of decoy draws a wallet must reject to never meet a wealthier
    /// decoy (the minimal max-safe filter).
    pub filter_rejected: f64,
    /// Max-factor adversary on rings built with that filter.
    pub max_adversary_id_filtered: f64,
}

fn argmax_hit(ring: &[(u64, u128)]) -> f64 {
    let top = ring.iter().map(|&(_, w)| w).max().unwrap_or(0);
    let ties = ring.iter().filter(|&&(_, w)| w == top).count();
    if ring[0].1 == top {
        1.0 / ties as f64
    } else {
        0.0
    }
}

fn privacy_rows(
    route: DecoyRoute,
    model: &PopulationModel,
    spends: &[HonestSpend],
) -> Vec<PrivacyRow> {
    let classes = wealthy_classes_bth();
    ["background", "wealthy"]
        .iter()
        .map(|&kind| {
            let want_bg = kind == "background";
            let sel: Vec<&HonestSpend> = spends
                .iter()
                .filter(|s| s.is_background == want_bg)
                .collect();
            let n = sel.len().max(1) as f64;
            let mut matches = 0.0;
            let mut unique = 0.0;
            let mut wealthier = 0.0;
            let mut id = 0.0;
            let mut id_filtered = 0.0;
            let mut draws = 0u64;
            let mut rejected = 0u64;
            for (i, s) in sel.iter().enumerate() {
                let m = s.ring.iter().filter(|&&(v, _)| v == s.value).count();
                matches += m as f64;
                if m == 1 {
                    unique += 1.0;
                }
                if s.ring[1..].iter().any(|&(_, w)| w > s.real_wealth) {
                    wealthier += 1.0;
                }
                id += argmax_hit(&s.ring);
                // Minimal max-safe filter: redraw any decoy wealthier than the
                // real input from the same population until it is not.
                let mut rng = ChaCha8Rng::seed_from_u64(splitmix64(
                    SEED ^ 0xF117_0000 ^ route_tag(route) ^ splitmix64(i as u64),
                ));
                let mut filtered = vec![s.ring[0]];
                for &decoy in &s.ring[1..] {
                    let mut d = decoy;
                    draws += 1;
                    let mut guard = 0;
                    while d.1 > s.real_wealth && guard < 10_000 {
                        rejected += 1;
                        draws += 1;
                        guard += 1;
                        d = draw_output(model, &classes, &mut rng);
                    }
                    filtered.push(d);
                }
                id_filtered += argmax_hit(&filtered);
            }
            PrivacyRow {
                route,
                model: *model,
                spender: kind,
                rings: sel.len(),
                mean_amount_matches: matches / n,
                unique_amount_match: unique / n,
                wealthier_decoy_rate: wealthier / n,
                max_adversary_id: id / n,
                filter_rejected: if draws > 0 {
                    rejected as f64 / draws as f64
                } else {
                    0.0
                },
                max_adversary_id_filtered: id_filtered / n,
            }
        })
        .collect()
}

// ===========================================================================
// 4. Multi-input composition
// ===========================================================================

/// How a rule's per-ring statistic is combined across a transaction's inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Composition {
    /// What the node did before 10.0.0: every input's ring members in one
    /// statistic.
    Pooled,
    /// One floor wealth per input, combined by the public
    /// `pseudo_output_amount`s (value-weighted mean of the per-input floors).
    PerInputByPseudo,
}

/// One rule's multi-input result under one composition.
#[derive(Clone, Debug, Default)]
pub struct MultiInputStat {
    pub fp_rate: f64,
    pub aggregate_excess_pct: f64,
    pub collection: f64,
}

/// Multi-input composition result.
#[derive(Clone, Debug)]
pub struct MultiInputResult {
    pub model: PopulationModel,
    pub spends: usize,
    /// Per rule: (pooled, per-input).
    pub per_rule: Vec<(MultiInputStat, MultiInputStat)>,
}

fn multi_input_floor(
    rule: FloorRule,
    comp: Composition,
    rings: &[(&[(u64, u128)], u64)],
    curve: &ClusterFactorCurve,
) -> u64 {
    if rule == FloorRule::Node {
        // The node composes per input itself, whatever the column.
        let inputs: Vec<crate::RingFloorInput<'_>> = rings
            .iter()
            .map(|(r, p)| crate::RingFloorInput {
                pseudo_amount: *p,
                members: r,
            })
            .collect();
        return curve.factor(crate::amount_matched_ring_floor_wealth(&inputs));
    }
    match comp {
        Composition::Pooled => {
            // Matched rules keep each ring's own matches; the rest pool all.
            let pooled: Vec<(u64, u128)> = match rule {
                FloorRule::MatchedMean | FloorRule::MatchedMax => rings
                    .iter()
                    .flat_map(|(r, p)| {
                        let m: Vec<(u64, u128)> =
                            r.iter().copied().filter(|&(v, _)| v == *p).collect();
                        if m.is_empty() {
                            r.to_vec()
                        } else {
                            m
                        }
                    })
                    .collect(),
                _ => rings.iter().flat_map(|(r, _)| r.iter().copied()).collect(),
            };
            let as_rule = match rule {
                FloorRule::MatchedMean => FloorRule::Mean,
                FloorRule::MatchedMax => FloorRule::Max,
                r => r,
            };
            curve.factor(floor_wealth(as_rule, &pooled, 0))
        }
        Composition::PerInputByPseudo => {
            let mut mean = WeightedMean::new();
            for (r, p) in rings {
                mean.add_product(*p as u128, floor_wealth(rule, r, *p));
                mean.add_weight(*p as u128);
            }
            curve.factor(mean.floor())
        }
    }
}

fn multi_input(
    chain: &ChainModel,
    model: &PopulationModel,
    n: usize,
    curve: &ClusterFactorCurve,
    rules: &[FloorRule],
) -> MultiInputResult {
    let archetypes: Vec<HonestArchetype> = honest_archetypes()
        .into_iter()
        .filter(|a| !a.is_background)
        .collect();
    let classes = wealthy_classes_bth();
    let rings = sample_route_ring_ages(
        chain,
        DecoyRoute::Node,
        RealAgeSpec::GammaSpend,
        2 * n,
        SEED ^ 0x2,
    );
    let mut rng = ChaCha8Rng::seed_from_u64(splitmix64(SEED ^ 0x4D17));
    let mut cases = Vec::new();
    for pair in rings.chunks_exact(2) {
        let arch = draw_archetype(&archetypes, &mut rng);
        let v1 = bth_to_pico(arch.coin_value_bth);
        let w1 = wealth_pico(arch.spender_cluster_wealth_bth);
        // An honest background coin of the same size: a 50/50 class blend.
        let v2 = v1;
        let mut r1 = vec![(v1, w1)];
        let mut r2 = vec![(v2, 0u128)];
        for &a in &pair[0].decoy_ages {
            r1.push(output_attrs(SEED, chain.id_at(a, 0), model, &classes));
        }
        for &a in &pair[1].decoy_ages {
            r2.push(output_attrs(SEED, chain.id_at(a, 0), model, &classes));
        }
        let blend = (w1 * v1 as u128) / (v1 as u128 + v2 as u128);
        let declared = blend * (1_000_000 - model.decay_ppm as u128) / 1_000_000;
        let elapsed = pair
            .iter()
            .flat_map(|r| {
                r.decoy_ages
                    .iter()
                    .copied()
                    .chain(std::iter::once(r.real_age))
            })
            .max()
            .unwrap_or(0);
        cases.push((
            r1,
            r2,
            v1,
            v2,
            curve.factor(blend),
            curve.factor(declared),
            elapsed,
        ));
    }
    let per_rule = rules
        .iter()
        .map(|&rule| {
            let mut out = [MultiInputStat::default(), MultiInputStat::default()];
            for (ci, comp) in [Composition::Pooled, Composition::PerInputByPseudo]
                .into_iter()
                .enumerate()
            {
                let mut fp = 0usize;
                let mut excess = 0.0f64;
                let mut levied = 0.0f64;
                let mut oracle_total = 0.0f64;
                let mut value_total = 0.0f64;
                for (r1, r2, v1, v2, oracle_f, claimed, elapsed) in &cases {
                    let value = v1 + v2;
                    let f = multi_input_floor(rule, comp, &[(r1, *v1), (r2, *v2)], curve);
                    let c = node_charge(value, f, *claimed, *elapsed);
                    let o = node_charge(value, *oracle_f, *claimed, *elapsed);
                    levied += c as f64;
                    oracle_total += o as f64;
                    value_total += value as f64;
                    if c > o {
                        fp += 1;
                        excess += (c - o) as f64;
                    }
                }
                out[ci] = MultiInputStat {
                    fp_rate: fp as f64 / cases.len().max(1) as f64,
                    aggregate_excess_pct: excess / value_total * 100.0,
                    collection: if oracle_total > 0.0 {
                        levied / oracle_total
                    } else {
                        1.0
                    },
                };
            }
            let [pooled, per_input] = out;
            (pooled, per_input)
        })
        .collect();
    MultiInputResult {
        model: *model,
        spends: cases.len(),
        per_rule,
    }
}

// ===========================================================================
// Report
// ===========================================================================

/// The full adversarial-dilution report.
#[derive(Clone, Debug)]
pub struct DilutionReport {
    pub rules: Vec<FloorRule>,
    pub chain: ChainModel,
    pub grid: DilutionGrid,
    /// Node route, contamination sweep (production decay, no round amounts).
    pub honest_node: Vec<HonestRow>,
    /// All production routes at the reference population.
    pub honest_routes: Vec<HonestRow>,
    /// Sensitivity rows on the node route.
    pub honest_sensitivity: Vec<HonestRow>,
    pub privacy: Vec<PrivacyRow>,
    pub multi_input: MultiInputResult,
}

/// The reference population: 5% wealthy outputs, production decay, no round
/// amounts.
pub fn reference_model() -> PopulationModel {
    PopulationModel {
        contamination_bps: 500,
        decay_ppm: PRODUCTION_DECAY_PPM,
        round_share_bps: 0,
    }
}

/// Run the adversarial-dilution mode at `ring_size` (20 = the node's ring).
pub fn run_dilution_sweep(ring_size: usize) -> DilutionReport {
    run_dilution_sweep_with(ring_size, HONEST_RINGS, MULTI_INPUT_SPENDS)
}

/// [`run_dilution_sweep`] with explicit sample sizes (tests use small ones).
pub fn run_dilution_sweep_with(
    ring_size: usize,
    honest_rings: usize,
    multi_spends: usize,
) -> DilutionReport {
    let curve = ClusterFactorCurve::default_params();
    let rules = candidate_rules();
    let chain = ChainModel::default();
    let grid = dilution_grid(ring_size, &curve);
    let reference = reference_model();

    let row = |route: DecoyRoute, model: PopulationModel| {
        let spends = honest_spends(&chain, route, &model, honest_rings, &curve);
        (
            HonestRow {
                route,
                model,
                spends: spends.len(),
                per_rule: honest_stats(&spends, &rules, &curve),
            },
            spends,
        )
    };

    let honest_node = [0u32, 100, 500, 1_000]
        .into_iter()
        .map(|c| {
            row(
                DecoyRoute::Node,
                PopulationModel {
                    contamination_bps: c,
                    ..reference
                },
            )
            .0
        })
        .collect();

    let mut honest_routes = Vec::new();
    let mut privacy = Vec::new();
    for route in DecoyRoute::PRODUCTION {
        let (r, spends) = row(route, reference);
        honest_routes.push(r);
        privacy.extend(privacy_rows(route, &reference, &spends));
    }

    let mut honest_sensitivity = Vec::new();
    for model in [
        PopulationModel {
            decay_ppm: 0,
            ..reference
        },
        PopulationModel {
            round_share_bps: 2_500,
            ..reference
        },
    ] {
        let (r, spends) = row(DecoyRoute::Node, model);
        honest_sensitivity.push(r);
        if model.round_share_bps > 0 {
            privacy.extend(privacy_rows(DecoyRoute::Node, &model, &spends));
        }
    }

    let multi_input = multi_input(&chain, &reference, multi_spends, &curve, &rules);

    DilutionReport {
        rules,
        chain,
        grid,
        honest_node,
        honest_routes,
        honest_sensitivity,
        privacy,
        multi_input,
    }
}

fn fx(scaled: u64) -> String {
    format!(
        "{:.3}×",
        scaled as f64 / ClusterFactorCurve::FACTOR_SCALE as f64
    )
}

fn pct(x: f64) -> String {
    format!("{:.1}%", x * 100.0)
}

fn pct2(x: f64) -> String {
    format!("{:.2}%", x * 100.0)
}

fn rule_header(rules: &[FloorRule], lead: &str) -> String {
    let mut s = format!("| {lead} |");
    for r in rules {
        s.push_str(&format!(" {} |", r.label()));
    }
    s.push('\n');
    s.push('|');
    for _ in 0..=rules.len() {
        s.push_str("---|");
    }
    s.push('\n');
    s
}

fn honest_table(
    s: &mut String,
    rows: &[HonestRow],
    rules: &[FloorRule],
    lead: &str,
    cell: impl Fn(&HonestRuleStat) -> String,
) {
    s.push_str(&rule_header(rules, lead));
    for r in rows {
        s.push_str(&format!("| {} / {} |", r.route.name(), r.model.label()));
        for st in &r.per_rule {
            s.push_str(&format!(" {} |", cell(st)));
        }
        s.push('\n');
    }
    s.push('\n');
}

/// Render the report as Markdown tables (the doc's numbers come from here).
pub fn to_markdown(r: &DilutionReport) -> String {
    let mut s = String::new();
    let rules = &r.rules;

    // 1. Dilution grid.
    s.push_str("### 1. Dilution: charge collected / undiluted charge\n\n");
    s.push_str(&format!(
        "Attacker: {} BTH in a {}-BTH cluster (undiluted floor {}, charge {:.2}% of value), \
         split into k coins, each spent young to a background output in a ring of {} \
         whose {} decoys are background coins worth D BTH. `D = coin` sets every decoy \
         to the coin's own amount. Pass bar (#1686): ≥ 95% in every cell.\n\n",
        ATTACK_TOTAL_VALUE_BTH,
        ATTACK_CLUSTER_WEALTH_BTH,
        fx(r.grid.undiluted_factor),
        r.grid.undiluted_pct,
        r.grid.ring_size,
        r.grid.ring_size - 1
    ));
    s.push_str(&rule_header(rules, "k / coin / D"));
    for c in &r.grid.cells {
        let d = c
            .decoy_value_bth
            .map(|d| format!("{d}"))
            .unwrap_or_else(|| "= coin".into());
        s.push_str(&format!(
            "| {} / {} BTH / {} |",
            c.split_k,
            c.coin_value_pico as u128 / PICO_PER_BTH,
            d
        ));
        for &(_, share) in &c.per_rule {
            s.push_str(&format!(" {} |", pct(share)));
        }
        s.push('\n');
    }
    s.push_str("| **worst cell** |");
    for w in r.grid.worst_per_rule() {
        s.push_str(&format!(" **{}** |", pct(w)));
    }
    s.push_str("\n| **passes ≥ 95%** |");
    for w in r.grid.worst_per_rule() {
        s.push_str(if w >= 0.95 { " yes |" } else { " no |" });
    }
    s.push_str("\n\n");

    s.push_str("Floor factor under (a) `mean` per cell (reproduces #1686's table):\n\n");
    s.push_str("| k / coin | D = 10 | D = 2000 | D = 100000 | D = coin |\n|---|---|---|---|---|\n");
    for chunk in r.grid.cells.chunks(4) {
        s.push_str(&format!(
            "| {} / {} BTH |",
            chunk[0].split_k,
            chunk[0].coin_value_pico as u128 / PICO_PER_BTH
        ));
        for c in chunk {
            s.push_str(&format!(" {} |", c.per_rule[0].0));
        }
        s.push('\n');
    }
    s.push('\n');

    // 2. Honest false positives.
    s.push_str("### 2. Honest spends on production-route rings\n\n");
    s.push_str(&format!(
        "Rings from the #1757 production pickers (`sample_route_ring_ages`, gamma-spend real \
         ages, {}-block chain, one output per block), {} rings per row. Real inputs from the \
         agent archetypes (85% background, 15% 10M-class whales); declared outputs carry the \
         wallet's per-hop decay. The oracle floors at the real input's own wealth. \
         c = share of wealthy-tagged outputs.\n\n",
        r.chain.tip_height,
        r.honest_node.first().map(|h| h.spends).unwrap_or(0)
    ));
    s.push_str("#### 2a. False-positive rate (charged above the oracle)\n\n");
    honest_table(&mut s, &r.honest_node, rules, "route / population", |st| {
        pct(st.fp_rate)
    });
    s.push_str("#### 2b. Mean over-charge on false-positive spends (% of coin value)\n\n");
    honest_table(&mut s, &r.honest_node, rules, "route / population", |st| {
        format!("{:.2}%", st.mean_excess_pct)
    });
    s.push_str("#### 2c. Aggregate over-charge (% of all honest value moved)\n\n");
    honest_table(&mut s, &r.honest_node, rules, "route / population", |st| {
        format!("{:.3}%", st.aggregate_excess_pct)
    });
    s.push_str(
        "#### 2d. Honest collection (total charge / oracle charge; < 100% = honest decay \
         downgrades under-priced)\n\n",
    );
    honest_table(&mut s, &r.honest_node, rules, "route / population", |st| {
        pct(st.collection)
    });
    s.push_str("#### 2e. Route check: false-positive rate on every production route\n\n");
    honest_table(
        &mut s,
        &r.honest_routes,
        rules,
        "route / population",
        |st| pct(st.fp_rate),
    );
    s.push_str("#### 2f. Sensitivity (node route): false-positive rate / collection\n\n");
    honest_table(
        &mut s,
        &r.honest_sensitivity,
        rules,
        "route / population",
        |st| format!("{} / {}", pct(st.fp_rate), pct(st.collection)),
    );

    // 3. Privacy.
    s.push_str("### 3. Privacy and decoy diversity on the same rings\n\n");
    s.push_str(
        "| route / population | spender | rings | mean amount matches | real is the only match \
         | ring has a wealthier decoy | max-factor adversary ID | max-safe filter: draws rejected \
         | max-factor adversary ID (filtered) |\n|---|---|---|---|---|---|---|---|---|\n",
    );
    for p in &r.privacy {
        s.push_str(&format!(
            "| {} / {} | {} | {} | {:.2} | {} | {} | {} | {} | {} |\n",
            p.route.name(),
            p.model.label(),
            p.spender,
            p.rings,
            p.mean_amount_matches,
            pct(p.unique_amount_match),
            pct(p.wealthier_decoy_rate),
            pct(p.max_adversary_id),
            pct2(p.filter_rejected),
            pct(p.max_adversary_id_filtered),
        ));
    }
    s.push('\n');

    // 4. Multi-input.
    s.push_str("### 4. Multi-input composition (honest two-input blend)\n\n");
    s.push_str(&format!(
        "{} honest spends on node-route rings ({}): one 10M-class whale coin plus one \
         background coin of the same amount, declared at the decayed 50/50 blend. Pooled = \
         all 40 members in one statistic (the node before 10.0.0); per-input = one floor per \
         ring, combined by the public `pseudo_output_amount`s. `node 10.0.0` runs the \
         production kernel, which composes per input itself, so both of its columns are the \
         same.\n\n",
        r.multi_input.spends,
        r.multi_input.model.label()
    ));
    s.push_str(
        "| rule | pooled FP rate | pooled over-charge (% of value) | pooled collection \
         | per-input FP rate | per-input over-charge (% of value) | per-input collection |\n\
         |---|---|---|---|---|---|---|\n",
    );
    for (rule, (p, q)) in rules.iter().zip(&r.multi_input.per_rule) {
        s.push_str(&format!(
            "| {} | {} | {:.3}% | {} | {} | {:.3}% | {} |\n",
            rule.label(),
            pct(p.fp_rate),
            p.aggregate_excess_pct,
            pct(p.collection),
            pct(q.fp_rate),
            q.aggregate_excess_pct,
            pct(q.collection),
        ));
    }
    s.push('\n');

    // 5. CT compatibility.
    s.push_str("### 5. Confidential-amounts compatibility (#902)\n\n");
    s.push_str(
        "| rule | reads member amounts | reads `pseudo_output_amount` | computable from public \
         data under CT |\n|---|---|---|---|\n",
    );
    for rule in rules {
        let ct = !rule.reads_member_amounts() && !rule.reads_pseudo_amount();
        s.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            rule.label(),
            if rule.reads_member_amounts() {
                "yes"
            } else {
                "no"
            },
            if rule.reads_pseudo_amount() {
                "yes"
            } else {
                "no"
            },
            if ct { "yes" } else { "no" },
        ));
    }
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ring_centroid_implied_factor;

    fn curve() -> ClusterFactorCurve {
        ClusterFactorCurve::default_params()
    }

    #[test]
    fn floor_rules_agree_with_kernels() {
        let c = curve();
        let mut rng = ChaCha8Rng::seed_from_u64(7);
        for _ in 0..500 {
            let ring: Vec<(u64, u128)> = (0..20)
                .map(|_| {
                    let v = bth_to_pico(rng.random_range(1..=100_000u64));
                    let w = if rng.random_range(0..4u32) == 0 {
                        wealth_pico(rng.random_range(1..=50_000_000u64))
                    } else {
                        0
                    };
                    (v, w)
                })
                .collect();
            // (a) is the consensus kernel, bit for bit.
            assert_eq!(
                floor_factor(FloorRule::Mean, &ring, ring[0].0, &c),
                ring_centroid_implied_factor(&ring, &c)
            );
            // (c): max of wealth through a monotone curve == max of factors.
            let max_f = ring.iter().map(|&(_, w)| c.factor(w)).max().unwrap();
            assert_eq!(floor_factor(FloorRule::Max, &ring, 0, &c), max_f);
            // p100 is the max.
            assert_eq!(
                floor_factor(FloorRule::Quantile(10_000), &ring, 0, &c),
                max_f
            );
        }
    }

    #[test]
    fn grid_reproduces_issue_table() {
        // #1686's table: 1000 BTH coin with 2000-BTH decoys floors at 4162;
        // a 10 BTH coin with 2000-BTH decoys at 1441; a 1 BTH coin with
        // 100k-BTH decoys at 1000; undiluted charge 9.49%.
        let g = dilution_grid(20, &curve());
        let mean_at = |k: u64, d: u64| {
            g.cells
                .iter()
                .find(|c| c.split_k == k && c.decoy_value_bth == Some(d))
                .unwrap()
                .per_rule[0]
                .0
        };
        assert_eq!(mean_at(1, 2_000), 4_162);
        assert_eq!(mean_at(100, 2_000), 1_441);
        assert_eq!(mean_at(1_000, 100_000), 1_000);
        assert!((g.undiluted_pct - 9.49).abs() < 0.01, "{}", g.undiluted_pct);
    }

    #[test]
    fn max_rules_are_dilution_proof_and_mean_is_not() {
        let g = dilution_grid(20, &curve());
        let worst = g.worst_per_rule();
        let idx = |r: FloorRule| g.rules.iter().position(|x| *x == r).unwrap();
        assert!(worst[idx(FloorRule::Max)] >= 0.999);
        assert!(worst[idx(FloorRule::MatchedMax)] >= 0.999);
        assert!(worst[idx(FloorRule::Mean)] < 0.95);
        // A lone wealthy member never moves an unweighted median of 20.
        assert_eq!(worst[idx(FloorRule::Quantile(5_000))], 0.0);
    }

    /// The 10.0.0 node rule (the production kernel) is (b) `match-max`
    /// composed per input by pseudo amount, bit for bit: on single rings
    /// (including unmatched pseudo amounts, the whole-ring fallback) and on
    /// two-input spends. So every `node 10.0.0` cell in the report equals the
    /// ranked recommendation's.
    #[test]
    fn node_rule_is_per_input_match_max() {
        let c = curve();
        let mut rng = ChaCha8Rng::seed_from_u64(1686);
        let mut ring = || -> Vec<(u64, u128)> {
            (0..20)
                .map(|_| {
                    let v = bth_to_pico(rng.random_range(1..=20u64));
                    let w = if rng.random_range(0..3u32) == 0 {
                        wealth_pico(rng.random_range(1..=50_000_000u64))
                    } else {
                        0
                    };
                    (v, w)
                })
                .collect()
        };
        for i in 0..500 {
            let (r1, r2) = (ring(), ring());
            let p1 = if i % 5 == 0 { 1 } else { r1[i % 20].0 };
            let p2 = r2[(i + 7) % 20].0;
            assert_eq!(
                floor_factor(FloorRule::Node, &r1, p1, &c),
                floor_factor(FloorRule::MatchedMax, &r1, p1, &c)
            );
            for comp in [Composition::Pooled, Composition::PerInputByPseudo] {
                assert_eq!(
                    multi_input_floor(FloorRule::Node, comp, &[(&r1, p1), (&r2, p2)], &c),
                    multi_input_floor(
                        FloorRule::MatchedMax,
                        Composition::PerInputByPseudo,
                        &[(&r1, p1), (&r2, p2)],
                        &c
                    )
                );
            }
        }
        // And it passes the dilution bar like match-max.
        let g = dilution_grid(20, &c);
        let worst = g.worst_per_rule();
        let idx = |r: FloorRule| g.rules.iter().position(|x| *x == r).unwrap();
        assert_eq!(
            worst[idx(FloorRule::Node)],
            worst[idx(FloorRule::MatchedMax)]
        );
        assert!(worst[idx(FloorRule::Node)] >= 0.999);
    }

    #[test]
    fn report_is_reproducible() {
        let a = to_markdown(&run_dilution_sweep_with(20, 200, 100));
        let b = to_markdown(&run_dilution_sweep_with(20, 200, 100));
        assert_eq!(a, b);
        assert!(a.contains("worst cell"));
        assert!(a.contains("Confidential-amounts compatibility"));
    }
}
