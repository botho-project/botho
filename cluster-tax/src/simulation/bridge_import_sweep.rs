//! Bridge-import calibration sweep (#937), rebuilt on production semantics
//! (#1697).
//!
//! # What this sweep calibrates
//!
//! ADR 0007 makes an **unwrap** (wBTH → BTH) mint into a **block-epoch import
//! cluster** `c_import(m) = H("bridge-import" ‖ m)`, `m = ⌊height / K⌋`, with
//! an import-factor floor `F`. The two ratified constants (`K` = 17,280
//! blocks, `F` = 1.5×) live in [`crate::bridge_import`]; this sweep imports
//! them and never restates them.
//!
//! # Production semantics (what changed in #1697)
//!
//! The 2026-07 version of this sweep modelled the ADR's *intent*. The node
//! does something different in five ways, and this version models the node:
//!
//! 1. **Import wealth is cumulative downstream wealth.** The block writer adds
//!    `amount·weight` to `W[c]` for *every* output that carries a tag on `c`
//!    (`update_cluster_wealth_for_output`, `writer.rs`), and never subtracts.
//!    So `W[c_import(m)]` starts at the epoch's unwrap total and then *grows*
//!    every time an import-tagged coin is spent — by its owner, by a recipient,
//!    or by a self-churner. It is not "Σ unwrap amounts in the epoch".
//! 2. **The factor is a three-way `max`, evaluated per spend.** The fee floor
//!    (`consensus_fee_floor`, `validation.rs`) charges demurrage at
//!    `max(curve(output-tag wealth), ring-centroid factor, F-blend on output
//!    tags)`. The `F` term is a value-weighted blend of `F` on import-tagged
//!    output value and 1× on the rest — it is read from the **outputs** the
//!    spender declares, not from the inputs.
//! 3. **Tags are the production `ClusterTagVector`.** 16 tags, 0.1% pruning,
//!    and the wallet's 5% decay per transfer
//!    (`ClusterTagVector::merge_weighted(inputs, 5%)`). Consensus only rejects
//!    tag *inflation*; any decay, including dropping every tag, is valid.
//! 4. **Block time is a parameter.** `K` is fixed in blocks, so the wall-clock
//!    cost of an epoch scales with the block time. Split-game cost is reported
//!    at 5, 20 and 40 s blocks.
//! 5. **Fees come from [`crate::consensus_fee_model`]** (#1695), fed by the
//!    pure ledger mirrors in [`crate::fee_floor_inputs`], so the sweep prices
//!    every spend the way the node does: size/output base, plus `max(accrued
//!    demurrage, capitalized downgrade charge)`.
//!
//! It adds two adversaries the old sweep could not express: a **tag-drop**
//! spender (declares background outputs, which consensus accepts) and a
//! **self-churner** (spends to itself; the default wallet's 5% decay strips
//! the import tag with no domestic money involved).
//!
//! Everything is deterministic (no RNG); the doc numbers regenerate
//! byte-for-byte.

use std::collections::{BTreeMap, BTreeSet};

use bth_transaction_types::{ClusterId as TagClusterId, ClusterTagVector, RING_SIZE};

pub use crate::bridge_import::import_cluster_id;
use crate::{
    bridge_import::{
        import_cluster_factor, BRIDGE_IMPORT_EPOCH_BLOCKS, BRIDGE_IMPORT_FACTOR_FLOOR,
    },
    consensus_fee::{
        consensus_fee_breakdown, consensus_tx_size_bytes, ClaimedClass, ConsensusFeeInputs,
        ConsensusFeeParams, FeeModelMode, MAINNET_BLOCKS_PER_YEAR,
    },
    fee_curve::PICO_PER_BTH,
    fee_floor_inputs::{
        output_wealth_contributions, resolve_fee_floor_at, FloorOutput, ResolvedFeeFloor, TagPair,
        WALLET_CLUSTER_DECAY_RATE,
    },
    ClusterFactorCurve,
};

use super::metrics::calculate_gini;

/// Seconds per year (365 days), used only to convert blocks to wall-clock.
const SECONDS_PER_YEAR: u64 = 365 * 86_400;

/// The block time the node's monetary policy assumes (5 s), derived from the
/// shared [`MAINNET_BLOCKS_PER_YEAR`] rather than restated. `K` and every
/// demurrage horizon are counted in blocks against this reference.
pub fn reference_block_secs() -> u64 {
    SECONDS_PER_YEAR / MAINNET_BLOCKS_PER_YEAR
}

/// Block times the split-game cost is reported at, in seconds. 5 s is the
/// policy reference; 20 s and 40 s bracket the 3–40 s blocks observed on the
/// testnet (#1697).
pub const BLOCK_TIMES_SECS: [u64; 3] = [5, 20, 40];

/// Blocks a coin is held between spends in the fee experiments (one day at the
/// reference block time). Sets the accrued-demurrage clock.
pub fn hold_blocks_one_day() -> u64 {
    MAINNET_BLOCKS_PER_YEAR / 365
}

/// The fee-model parameters every spend is priced with: consensus floor only,
/// mainnet schedule, at the first epoch-1 height so demurrage is active.
pub fn fee_params() -> ConsensusFeeParams {
    ConsensusFeeParams::mainnet(FeeModelMode::ConsensusOnly)
}

fn bth(n: u64) -> u64 {
    (n as u128 * PICO_PER_BTH).min(u64::MAX as u128) as u64
}

// ============================================================================
// Candidate grids
// ============================================================================

/// A candidate epoch length `K`, in blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Epoch {
    /// The candidate `K` in blocks.
    pub blocks: u64,
    /// Human label at the 5 s reference (e.g. "1d").
    pub label: &'static str,
}

impl Epoch {
    /// Wall-clock hours one epoch spans at `block_secs` seconds per block.
    pub fn hours_at(&self, block_secs: u64) -> f64 {
        self.blocks as f64 * block_secs as f64 / 3600.0
    }
}

/// Candidate epoch lengths, ~14 h → ~1 week at the 5 s reference. The
/// ratified `K` is [`BRIDGE_IMPORT_EPOCH_BLOCKS`]; the others are multiples of
/// it so the grid tracks the production constant.
pub fn candidate_epochs() -> Vec<Epoch> {
    let k = BRIDGE_IMPORT_EPOCH_BLOCKS;
    vec![
        Epoch {
            blocks: k * 7 / 12,
            label: "14h",
        },
        Epoch {
            blocks: k,
            label: "1d",
        },
        Epoch {
            blocks: 2 * k,
            label: "2d",
        },
        Epoch {
            blocks: k * 7 / 2,
            label: "3.5d",
        },
        Epoch {
            blocks: 7 * k,
            label: "1wk",
        },
    ]
}

/// Candidate import-factor floors `F`, FACTOR_SCALE units (1.0× → 2.0×).
pub fn candidate_floors() -> Vec<u64> {
    vec![1000, 1250, 1500, 1750, 2000]
}

/// A co-occurring bridge-inflow regime: the flood unwrap volume in an epoch.
#[derive(Clone, Copy, Debug)]
pub struct VolumeRegime {
    /// Human label.
    pub label: &'static str,
    /// Flood unwrap volume in the epoch, BTH.
    pub epoch_volume_bth: u64,
}

/// Flood regimes: a trickle, the curve midpoint, and a capital flood.
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

/// Whale import sizes for the split-game table, BTH.
pub fn whale_import_sizes() -> Vec<u64> {
    vec![500_000, 2_000_000, 10_000_000]
}

/// A representative retail bridge user, BTH.
pub const SMALL_ENTRANT_BTH: u64 = 1_000;

/// The flood-epoch coin the decay / churn experiments follow: a 10,000-BTH
/// share of a 2,000,000-BTH flood epoch.
pub const FLOOD_COIN_BTH: u64 = 10_000;
/// The flood epoch's total unwrap volume for the decay / churn experiments.
pub const FLOOD_EPOCH_BTH: u64 = 2_000_000;

// ============================================================================
// Production ledger model
// ============================================================================

/// A coin: value in picocredits plus its production cluster tags.
#[derive(Clone, Debug)]
pub struct Coin {
    /// Value, picocredits.
    pub value: u64,
    /// Production cluster tags.
    pub tags: ClusterTagVector,
}

fn pairs(tags: &ClusterTagVector) -> Vec<TagPair> {
    tags.entries
        .iter()
        .map(|e| (e.cluster_id.0, e.weight))
        .collect()
}

/// The slice of node state the import factor depends on: the cumulative
/// per-cluster wealth `W[c]` (`cluster_wealth_db`) and the recorded
/// bridge-import set (`bridge_import_clusters_db`).
#[derive(Clone, Debug, Default)]
pub struct ProductionLedger {
    wealth: BTreeMap<u64, u128>,
    imports: BTreeSet<u64>,
}

impl ProductionLedger {
    /// A fresh ledger.
    pub fn new() -> Self {
        Self::default()
    }

    /// Committed cumulative wealth of cluster `c`.
    pub fn wealth(&self, cluster: u64) -> u128 {
        self.wealth.get(&cluster).copied().unwrap_or(0)
    }

    /// Whether `c` is a recorded bridge-import cluster.
    pub fn is_import(&self, cluster: u64) -> bool {
        self.imports.contains(&cluster)
    }

    /// Apply a block's outputs: `update_cluster_wealth_for_output` for each.
    pub fn add_output(&mut self, amount: u64, tags: &ClusterTagVector) {
        for (cluster, contribution) in output_wealth_contributions(amount, &pairs(tags)) {
            let slot = self.wealth.entry(cluster).or_insert(0);
            *slot = slot.saturating_add(contribution);
        }
    }

    /// A bridge release in import epoch `epoch`: one output 100%-tagged to
    /// `import_cluster_id(epoch)` (`bth_scan::build_release_tx`), recorded in
    /// the import set (`record_bridge_import_clusters_for_output`) and added
    /// to `W` like any other output.
    pub fn unwrap(&mut self, epoch: u64, amount: u64) -> Coin {
        let id = import_cluster_id(epoch).0;
        let tags = ClusterTagVector::single(TagClusterId(id));
        self.imports.insert(id);
        self.add_output(amount, &tags);
        Coin {
            value: amount,
            tags,
        }
    }

    /// Resolve the fee-floor inputs for a spend, exactly as the ledger does,
    /// with import floor `floor_f`.
    pub fn resolve(
        &self,
        ring: &[(u64, Vec<TagPair>)],
        outputs: &[(u64, Vec<TagPair>)],
        floor_f: u64,
    ) -> ResolvedFeeFloor {
        let ring_refs: Vec<(u64, &[TagPair])> =
            ring.iter().map(|(v, t)| (*v, t.as_slice())).collect();
        let outs: Vec<FloorOutput<'_>> = outputs
            .iter()
            .map(|(amount, tags)| FloorOutput {
                amount: *amount,
                tags: tags.as_slice(),
            })
            .collect();
        resolve_fee_floor_at(
            &outs,
            &ring_refs,
            &|c| self.wealth(c),
            &|c| self.is_import(c),
            floor_f,
        )
    }
}

/// How the spender declares its output tags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputTagPolicy {
    /// The default wallet: `merge_weighted(inputs, 5%)` on every output.
    Wallet,
    /// The tag-drop adversary: background (empty) outputs. Valid at consensus
    /// (`check_cluster_tag_inheritance` rejects only inflation).
    Drop,
    /// A spender-chosen decay rate (parts per TAG_WEIGHT_SCALE) instead of the
    /// wallet's 5%. Any rate is valid at consensus.
    Shed(u32),
}

/// The decay rate that sheds a coin's tag weight to `1/RING_SIZE` per hop:
/// the most a spender can shed while its claimed output wealth stays at the
/// ring-centroid floor (the real input diluted by `RING_SIZE − 1` background
/// decoys), so the #925 downgrade charge does not fire.
pub fn max_shed_decay_rate() -> u32 {
    crate::TAG_WEIGHT_SCALE - crate::TAG_WEIGHT_SCALE / RING_SIZE as u32
}

/// The result of one priced spend.
#[derive(Clone, Debug)]
pub struct SpendResult {
    /// The ledger-resolved factor inputs.
    pub resolved: ResolvedFeeFloor,
    /// The consensus fee floor, picocredits.
    pub fee: u64,
    /// The demurrage part of `fee`.
    pub demurrage: u64,
    /// The merged output coin.
    pub out: Coin,
}

impl SpendResult {
    /// Fee as a percentage of the value moved.
    pub fn fee_pct(&self) -> f64 {
        self.fee as f64 / self.out.value.max(1) as f64 * 100.0
    }
}

/// Spend `inputs` into two outputs (payment + change) carrying the tags the
/// policy declares. Each input sits in a ring of `RING_SIZE` members: itself
/// plus `RING_SIZE − 1` background decoys of equal value. The fee is priced
/// before the outputs are applied (the ledger reads committed state). If
/// `commit` is false the ledger is left untouched (a probe).
pub fn spend(
    ledger: &mut ProductionLedger,
    inputs: &[Coin],
    policy: OutputTagPolicy,
    floor_f: u64,
    elapsed: u64,
    commit: bool,
) -> SpendResult {
    let value: u64 = inputs
        .iter()
        .fold(0u64, |acc, c| acc.saturating_add(c.value));
    let out_tags = match policy {
        OutputTagPolicy::Wallet | OutputTagPolicy::Shed(_) => {
            let rate = match policy {
                OutputTagPolicy::Shed(rate) => rate,
                _ => WALLET_CLUSTER_DECAY_RATE,
            };
            let merge: Vec<(ClusterTagVector, u64)> =
                inputs.iter().map(|c| (c.tags.clone(), c.value)).collect();
            ClusterTagVector::merge_weighted(&merge, rate)
        }
        OutputTagPolicy::Drop => ClusterTagVector::empty(),
    };

    let mut ring: Vec<(u64, Vec<TagPair>)> = Vec::with_capacity(inputs.len() * RING_SIZE);
    for c in inputs {
        ring.push((c.value, pairs(&c.tags)));
        for _ in 1..RING_SIZE {
            ring.push((c.value, Vec::new()));
        }
    }
    let half = value / 2;
    let out_pairs = pairs(&out_tags);
    let outputs = vec![(half, out_pairs.clone()), (value - half, out_pairs)];

    let resolved = ledger.resolve(&ring, &outputs, floor_f);
    let params = fee_params();
    let breakdown = consensus_fee_breakdown(
        &ConsensusFeeInputs {
            size: consensus_tx_size_bytes(inputs.len(), outputs.len(), 0),
            n_out: outputs.len(),
            n_memo: 0,
            claimed: ClaimedClass::Wealth(resolved.claimed_wealth),
            floor: resolved.floor,
            elapsed,
            height: params.first_demurrage_height(),
            output_sum: value,
        },
        &params,
    );
    if commit {
        for (amount, _) in &outputs {
            ledger.add_output(*amount, &out_tags);
        }
    }
    SpendResult {
        resolved,
        fee: breakdown.fee,
        demurrage: breakdown.demurrage,
        out: Coin {
            value,
            tags: out_tags,
        },
    }
}

/// The import-tag weight a coin carries on `cluster`.
pub fn weight_on(coin: &Coin, cluster: u64) -> u32 {
    coin.tags.get_weight(TagClusterId(cluster))
}

/// A ledger holding one import epoch (epoch 0) with `other_bth` of co-occurring
/// unwraps plus the followed coin of `coin_bth`. Returns the coin.
fn epoch_with_coin(ledger: &mut ProductionLedger, other_bth: u64, coin_bth: u64) -> Coin {
    if other_bth > 0 {
        ledger.unwrap(0, bth(other_bth));
    }
    ledger.unwrap(0, bth(coin_bth))
}

/// The demurrage factor a freshly unwrapped coin's first wallet spend is
/// charged at, alone in its epoch.
fn first_spend(coin_bth: u64, floor_f: u64) -> SpendResult {
    let mut l = ProductionLedger::new();
    let coin = epoch_with_coin(&mut l, 0, coin_bth);
    spend(
        &mut l,
        &[coin],
        OutputTagPolicy::Wallet,
        floor_f,
        hold_blocks_one_day(),
        false,
    )
}

// ============================================================================
// Experiment 1: split-game cost (K × block time)
// ============================================================================

/// One row of the split-game table.
#[derive(Clone, Debug)]
pub struct SplitGameRow {
    pub epoch: Epoch,
    pub whale_import_bth: u64,
    /// Demurrage factor at the first spend of the whole import in one epoch.
    pub undiluted_factor_scaled: u64,
    /// Fewest equal epochs such that a tranche's first spend is charged at the
    /// import floor (the curve no longer binds).
    pub epochs_to_floor: u64,
    /// Wall-clock cost in days at each of [`BLOCK_TIMES_SECS`].
    pub split_game_days: [f64; 3],
}

/// Whether a tranche of `tranche_bth` is already at the floor on its first
/// spend: the curve term no longer exceeds the import-floor term.
fn tranche_at_floor(tranche_bth: u64, floor_f: u64) -> bool {
    let r = first_spend(tranche_bth, floor_f).resolved;
    r.claimed_factor <= r.import_floor
}

/// Fewest equal epochs that bring every tranche to the floor (binary search —
/// the factor is monotone in tranche size).
pub fn epochs_to_reach_floor(whale_bth: u64, floor_f: u64) -> u64 {
    if tranche_at_floor(whale_bth, floor_f) {
        return 1;
    }
    let (mut lo, mut hi) = (1u64, 2u64);
    while !tranche_at_floor(whale_bth / hi, floor_f) {
        lo = hi;
        hi *= 2;
        if hi > (1 << 40) {
            return hi;
        }
    }
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if tranche_at_floor(whale_bth / mid, floor_f) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
}

/// The split-game table at the ratified floor.
pub fn split_game_table(epochs: &[Epoch], whale_imports_bth: &[u64]) -> Vec<SplitGameRow> {
    let mut rows = Vec::new();
    for &whale in whale_imports_bth {
        let undiluted = first_spend(whale, BRIDGE_IMPORT_FACTOR_FLOOR)
            .resolved
            .demurrage_factor;
        let n = epochs_to_reach_floor(whale, BRIDGE_IMPORT_FACTOR_FLOOR);
        for &epoch in epochs {
            let total_blocks = n.saturating_mul(epoch.blocks);
            let days = BLOCK_TIMES_SECS.map(|bt| total_blocks as f64 * bt as f64 / 86_400.0);
            rows.push(SplitGameRow {
                epoch,
                whale_import_bth: whale,
                undiluted_factor_scaled: undiluted,
                epochs_to_floor: n,
                split_game_days: days,
            });
        }
    }
    rows
}

// ============================================================================
// Experiment 2: co-location collateral, with the flood circulating
// ============================================================================

/// Flood self-spend hops after which the entrant's factor is re-measured.
pub const FLOOD_HOPS: [u64; 3] = [0, 5, 20];

/// One row of the collateral table.
#[derive(Clone, Debug)]
pub struct CollateralRow {
    pub regime: VolumeRegime,
    /// The entrant's first-spend factor alone in its epoch.
    pub alone_factor_scaled: u64,
    /// The entrant's first-spend factor in the flood epoch after the flood has
    /// been spent `FLOOD_HOPS[i]` times (each hop adds the flood's
    /// import-weighted value to the shared cumulative `W`).
    pub caught_factor_scaled: [u64; 3],
}

fn entrant_factor_after_flood(flood_bth: u64, small_bth: u64, hops: u64) -> u64 {
    let mut l = ProductionLedger::new();
    let mut flood = l.unwrap(0, bth(flood_bth));
    let small = l.unwrap(0, bth(small_bth));
    for _ in 0..hops {
        flood = spend(
            &mut l,
            &[flood],
            OutputTagPolicy::Wallet,
            BRIDGE_IMPORT_FACTOR_FLOOR,
            hold_blocks_one_day(),
            true,
        )
        .out;
    }
    spend(
        &mut l,
        &[small],
        OutputTagPolicy::Wallet,
        BRIDGE_IMPORT_FACTOR_FLOOR,
        hold_blocks_one_day(),
        false,
    )
    .resolved
    .demurrage_factor
}

/// The collateral table at the ratified floor.
pub fn collateral_table(regimes: &[VolumeRegime], small_bth: u64) -> Vec<CollateralRow> {
    let alone = first_spend(small_bth, BRIDGE_IMPORT_FACTOR_FLOOR)
        .resolved
        .demurrage_factor;
    regimes
        .iter()
        .map(|&regime| CollateralRow {
            regime,
            alone_factor_scaled: alone,
            caught_factor_scaled: FLOOD_HOPS
                .map(|h| entrant_factor_after_flood(regime.epoch_volume_bth, small_bth, h)),
        })
        .collect()
}

// ============================================================================
// Experiment 3: the floor F — onboarding toll vs the tag-drop escape
// ============================================================================

/// One row of the floor table.
#[derive(Clone, Debug)]
pub struct FloorRow {
    pub floor_scaled: u64,
    /// A small entrant's first-spend demurrage factor.
    pub small_entrant_factor_scaled: u64,
    /// Its first wallet spend after one day, fee % of value.
    pub first_spend_1d_pct: f64,
    /// Its first wallet spend after one year, fee % of value.
    pub first_spend_1y_pct: f64,
    /// The same coin spent once to background outputs (tag drop) after one
    /// day, fee % of value.
    pub tag_drop_1d_pct: f64,
    /// `(F − 1×) / (6× − 1×)`.
    pub residual_above_background: f64,
}

fn small_entrant_spend(floor_f: u64, policy: OutputTagPolicy, elapsed: u64) -> SpendResult {
    let mut l = ProductionLedger::new();
    let coin = epoch_with_coin(&mut l, 0, SMALL_ENTRANT_BTH);
    spend(&mut l, &[coin], policy, floor_f, elapsed, false)
}

/// The floor table across candidate `F`.
pub fn floor_table(floors: &[u64]) -> Vec<FloorRow> {
    floors
        .iter()
        .map(|&f| {
            let day = small_entrant_spend(f, OutputTagPolicy::Wallet, hold_blocks_one_day());
            let year = small_entrant_spend(f, OutputTagPolicy::Wallet, MAINNET_BLOCKS_PER_YEAR);
            let drop = small_entrant_spend(f, OutputTagPolicy::Drop, hold_blocks_one_day());
            let max_gap = ClusterFactorCurve::FACTOR_SCALE * 5;
            FloorRow {
                floor_scaled: f,
                small_entrant_factor_scaled: day.resolved.demurrage_factor,
                first_spend_1d_pct: day.fee_pct(),
                first_spend_1y_pct: year.fee_pct(),
                tag_drop_1d_pct: drop.fee_pct(),
                residual_above_background: (f - ClusterFactorCurve::FACTOR_SCALE) as f64
                    / max_gap as f64,
            }
        })
        .collect()
}

// ============================================================================
// Experiment 4: decay by domestic circulation
// ============================================================================

/// One row of a circulation trace (domestic mixing or self-churn).
#[derive(Clone, Debug)]
pub struct TraceRow {
    /// Spends applied so far.
    pub spends: u64,
    /// Import weight on the coin, TAG_WEIGHT_SCALE units.
    pub import_weight: u32,
    /// Cumulative wealth of the import cluster, BTH.
    pub import_wealth_bth: f64,
    /// Demurrage factor the coin's next wallet spend is charged at.
    pub next_spend_factor_scaled: u64,
    /// Fee of the spend that produced this row, % of value (0 for row 0).
    pub step_fee_pct: f64,
    /// Fees paid so far, % of the coin's starting value.
    pub cumulative_fee_pct: f64,
}

/// Spends traced in the domestic-mixing experiment.
pub const DECAY_MAX_SPENDS: u64 = 20;

fn probe(ledger: &mut ProductionLedger, coin: &Coin) -> u64 {
    spend(
        ledger,
        std::slice::from_ref(coin),
        OutputTagPolicy::Wallet,
        BRIDGE_IMPORT_FACTOR_FLOOR,
        hold_blocks_one_day(),
        false,
    )
    .resolved
    .demurrage_factor
}

/// A flood-epoch coin that, on every spend, merges with an equal-value
/// background (domestic) input under the default wallet's 5% decay, pays half
/// the merged value away and keeps the other half as change (so the coin's
/// value stays constant while its import weight blends down).
pub fn decay_by_circulation() -> Vec<TraceRow> {
    let mut l = ProductionLedger::new();
    let mut coin = epoch_with_coin(&mut l, FLOOD_EPOCH_BTH - FLOOD_COIN_BTH, FLOOD_COIN_BTH);
    let id = import_cluster_id(0).0;
    let start = coin.value as f64;
    let mut fees = 0u128;
    let mut rows = vec![TraceRow {
        spends: 0,
        import_weight: weight_on(&coin, id),
        import_wealth_bth: l.wealth(id) as f64 / PICO_PER_BTH as f64,
        next_spend_factor_scaled: probe(&mut l, &coin),
        step_fee_pct: 0.0,
        cumulative_fee_pct: 0.0,
    }];
    for s in 1..=DECAY_MAX_SPENDS {
        let domestic = Coin {
            value: coin.value,
            tags: ClusterTagVector::empty(),
        };
        let r = spend(
            &mut l,
            &[coin, domestic],
            OutputTagPolicy::Wallet,
            BRIDGE_IMPORT_FACTOR_FLOOR,
            hold_blocks_one_day(),
            true,
        );
        fees += r.fee as u128;
        let step = r.fee_pct();
        // Two equal outputs: the payment leaves, the change (same merged tags,
        // the coin's original value) is kept. Both outputs were added to W.
        coin = Coin {
            value: r.out.value / 2,
            tags: r.out.tags,
        };
        rows.push(TraceRow {
            spends: s,
            import_weight: weight_on(&coin, id),
            import_wealth_bth: l.wealth(id) as f64 / PICO_PER_BTH as f64,
            next_spend_factor_scaled: probe(&mut l, &coin),
            step_fee_pct: step,
            cumulative_fee_pct: fees as f64 / start * 100.0,
        });
    }
    rows
}

// ============================================================================
// Experiment 5: the self-churn adversary
// ============================================================================

/// Rows printed from a self-churn trace.
pub const CHURN_PRINT: [u64; 9] = [0, 1, 2, 5, 10, 20, 50, 100, 150];
/// Self-churn spends simulated.
pub const CHURN_MAX_SPENDS: u64 = 200;
/// Blocks between self-churn spends (the churner waits ~a minute per hop).
pub const CHURN_SPACING_BLOCKS: u64 = 12;

/// A self-churn trace: the coin is spent to its owner every
/// `CHURN_SPACING_BLOCKS` with no domestic inflow, under the default wallet.
#[derive(Clone, Debug)]
pub struct ChurnTrace {
    pub label: &'static str,
    pub rows: Vec<TraceRow>,
    /// First spend count at which the next-spend factor is ≤ F.
    pub spends_to_floor: Option<u64>,
    /// First spend count at which the next-spend factor is ≤ 1.1×.
    pub spends_to_1_1x: Option<u64>,
    /// First spend count at which the import tag is pruned (weight 0).
    pub spends_to_pruned: Option<u64>,
    /// Cumulative fee % at `spends_to_pruned`.
    pub fee_pct_to_pruned: Option<f64>,
}

fn self_churn(
    label: &'static str,
    other_bth: u64,
    coin_bth: u64,
    policy: OutputTagPolicy,
) -> ChurnTrace {
    let mut l = ProductionLedger::new();
    let mut coin = epoch_with_coin(&mut l, other_bth, coin_bth);
    let id = import_cluster_id(0).0;
    let start = coin.value as f64;
    let mut fees = 0u128;
    let mut rows = vec![TraceRow {
        spends: 0,
        import_weight: weight_on(&coin, id),
        import_wealth_bth: l.wealth(id) as f64 / PICO_PER_BTH as f64,
        next_spend_factor_scaled: probe(&mut l, &coin),
        step_fee_pct: 0.0,
        cumulative_fee_pct: 0.0,
    }];
    for s in 1..=CHURN_MAX_SPENDS {
        let r = spend(
            &mut l,
            &[coin],
            policy,
            BRIDGE_IMPORT_FACTOR_FLOOR,
            CHURN_SPACING_BLOCKS,
            true,
        );
        fees += r.fee as u128;
        let step = r.fee_pct();
        coin = r.out;
        rows.push(TraceRow {
            spends: s,
            import_weight: weight_on(&coin, id),
            import_wealth_bth: l.wealth(id) as f64 / PICO_PER_BTH as f64,
            next_spend_factor_scaled: probe(&mut l, &coin),
            step_fee_pct: step,
            cumulative_fee_pct: fees as f64 / start * 100.0,
        });
    }
    let find = |pred: &dyn Fn(&TraceRow) -> bool| rows.iter().find(|r| pred(r)).map(|r| r.spends);
    let spends_to_pruned = find(&|r| r.import_weight == 0);
    ChurnTrace {
        label,
        spends_to_floor: find(&|r| r.next_spend_factor_scaled <= BRIDGE_IMPORT_FACTOR_FLOOR),
        spends_to_1_1x: find(&|r| r.next_spend_factor_scaled <= 1_100),
        spends_to_pruned,
        fee_pct_to_pruned: spends_to_pruned.map(|s| rows[s as usize].cumulative_fee_pct),
        rows,
    }
}

/// Self-churn traces: a small entrant and a flood-epoch coin under the
/// default wallet, and the flood coin shedding at [`max_shed_decay_rate`].
pub fn self_churn_traces() -> Vec<ChurnTrace> {
    vec![
        self_churn(
            "small entrant (1,000 BTH alone), default wallet",
            0,
            SMALL_ENTRANT_BTH,
            OutputTagPolicy::Wallet,
        ),
        self_churn(
            "flood coin (10,000 of a 2,000,000-BTH epoch), default wallet",
            FLOOD_EPOCH_BTH - FLOOD_COIN_BTH,
            FLOOD_COIN_BTH,
            OutputTagPolicy::Wallet,
        ),
        self_churn(
            "flood coin (10,000 of a 2,000,000-BTH epoch), max-shed decay",
            FLOOD_EPOCH_BTH - FLOOD_COIN_BTH,
            FLOOD_COIN_BTH,
            OutputTagPolicy::Shed(max_shed_decay_rate()),
        ),
    ]
}

// ============================================================================
// Experiment 6: the tag-drop adversary
// ============================================================================

/// One row of the tag-drop table.
#[derive(Clone, Debug)]
pub struct TagDropRow {
    pub label: &'static str,
    /// Honest (wallet) first spend.
    pub honest_factor_scaled: u64,
    pub honest_fee_pct: f64,
    /// Tag-drop first spend: background outputs.
    pub drop_factor_scaled: u64,
    pub drop_ring_floor_scaled: u64,
    pub drop_fee_pct: f64,
}

fn tag_drop_row(label: &'static str, other_bth: u64, coin_bth: u64) -> TagDropRow {
    let mut l = ProductionLedger::new();
    let coin = epoch_with_coin(&mut l, other_bth, coin_bth);
    let run = |l: &mut ProductionLedger, p| {
        spend(
            l,
            std::slice::from_ref(&coin),
            p,
            BRIDGE_IMPORT_FACTOR_FLOOR,
            hold_blocks_one_day(),
            false,
        )
    };
    let honest = run(&mut l, OutputTagPolicy::Wallet);
    let drop = run(&mut l, OutputTagPolicy::Drop);
    TagDropRow {
        label,
        honest_factor_scaled: honest.resolved.demurrage_factor,
        honest_fee_pct: honest.fee_pct(),
        drop_factor_scaled: drop.resolved.demurrage_factor,
        drop_ring_floor_scaled: drop.resolved.ring_floor,
        drop_fee_pct: drop.fee_pct(),
    }
}

/// Tag-drop vs honest first spend for three import shapes.
pub fn tag_drop_table() -> Vec<TagDropRow> {
    vec![
        tag_drop_row("small entrant (1,000 BTH alone)", 0, SMALL_ENTRANT_BTH),
        tag_drop_row(
            "flood coin (10,000 of 2,000,000)",
            FLOOD_EPOCH_BTH - FLOOD_COIN_BTH,
            FLOOD_COIN_BTH,
        ),
        tag_drop_row("undiluted whale (2,000,000 alone)", 0, FLOOD_EPOCH_BTH),
    ]
}

// ============================================================================
// Gini illustration
// ============================================================================

/// Effective-spendable Gini with a 2M-BTH flood entering at 1× vs at its
/// production import factor.
#[derive(Clone, Debug)]
pub struct GiniIllustration {
    pub flood_factor_scaled: u64,
    pub gini_flood_at_background: f64,
    pub gini_flood_at_import_factor: f64,
}

fn gini_illustration() -> GiniIllustration {
    let curve = ClusterFactorCurve::default_params();
    let flood_bth = FLOOD_EPOCH_BTH;
    let flood_factor = import_cluster_factor(flood_bth as u128 * PICO_PER_BTH, &curve);
    let eff = |value_bth: u64, factor: u64| -> u64 {
        (value_bth as u128 * ClusterFactorCurve::FACTOR_SCALE as u128 / factor as u128) as u64
    };
    let mut at_bg = vec![eff(1_000, 1_000); 100];
    at_bg.push(eff(flood_bth, 1_000));
    let mut at_import = vec![eff(1_000, 1_000); 100];
    at_import.push(eff(flood_bth, flood_factor));
    GiniIllustration {
        flood_factor_scaled: flood_factor,
        gini_flood_at_background: calculate_gini(&at_bg),
        gini_flood_at_import_factor: calculate_gini(&at_import),
    }
}

// ============================================================================
// Full report
// ============================================================================

/// The complete bridge-import sweep report.
#[derive(Clone, Debug)]
pub struct BridgeImportReport {
    pub epochs: Vec<Epoch>,
    pub split_game_rows: Vec<SplitGameRow>,
    pub collateral_rows: Vec<CollateralRow>,
    pub floor_rows: Vec<FloorRow>,
    pub decay_rows: Vec<TraceRow>,
    pub churn: Vec<ChurnTrace>,
    pub tag_drop_rows: Vec<TagDropRow>,
    pub gini: GiniIllustration,
}

/// Run the complete sweep.
pub fn run_bridge_import_sweep() -> BridgeImportReport {
    let epochs = candidate_epochs();
    BridgeImportReport {
        split_game_rows: split_game_table(&epochs, &whale_import_sizes()),
        collateral_rows: collateral_table(&volume_regimes(), SMALL_ENTRANT_BTH),
        floor_rows: floor_table(&candidate_floors()),
        decay_rows: decay_by_circulation(),
        churn: self_churn_traces(),
        tag_drop_rows: tag_drop_table(),
        gini: gini_illustration(),
        epochs,
    }
}

fn fmt_factor(scaled: u64) -> String {
    format!(
        "{:.3}x",
        scaled as f64 / ClusterFactorCurve::FACTOR_SCALE as f64
    )
}

fn fmt_opt(v: Option<u64>) -> String {
    v.map(|s| s.to_string())
        .unwrap_or_else(|| format!("> {CHURN_MAX_SPENDS}"))
}

/// Render the report as Markdown (the doc numbers are generated from this).
pub fn to_markdown(report: &BridgeImportReport) -> String {
    let mut s = String::new();
    let f = fmt_factor(BRIDGE_IMPORT_FACTOR_FLOOR);

    s.push_str(&format!(
        "Ratified constants (imported from `cluster_tax::bridge_import`): K = {} blocks, F = {}. \
         Reference block time {} s. Wallet tag decay {}% per transfer; ring size {}. Fees are \
         `consensus_fee_model` (consensus floor only) at the first demurrage-active height.\n\n",
        BRIDGE_IMPORT_EPOCH_BLOCKS,
        f,
        reference_block_secs(),
        WALLET_CLUSTER_DECAY_RATE as f64 / 10_000.0,
        RING_SIZE,
    ));

    // --- Split game -------------------------------------------------------
    s.push_str("### Split-game cost by epoch length K and block time (at F)\n\n");
    s.push_str(
        "`undiluted factor` = the demurrage factor at the first wallet spend of the whole import \
         unwrapped in one epoch. `epochs to floor` = the fewest equal epochs after which a \
         tranche's first spend is charged at the import floor (the curve no longer binds). \
         K is fixed in blocks, so the wall-clock cost scales with the block time.\n\n",
    );
    s.push_str(
        "| K | whale import (BTH) | undiluted factor | epochs to floor | days @ 5 s | days @ 20 s | days @ 40 s |\n",
    );
    s.push_str("|---|---:|---:|---:|---:|---:|---:|\n");
    for r in &report.split_game_rows {
        s.push_str(&format!(
            "| {} ({} blocks) | {} | {} | {} | {:.1} | {:.1} | {:.1} |\n",
            r.epoch.label,
            r.epoch.blocks,
            r.whale_import_bth,
            fmt_factor(r.undiluted_factor_scaled),
            r.epochs_to_floor,
            r.split_game_days[0],
            r.split_game_days[1],
            r.split_game_days[2],
        ));
    }
    s.push('\n');

    // --- Collateral -------------------------------------------------------
    s.push_str("### Small-entrant collateral in a flood epoch, as the flood circulates\n\n");
    s.push_str(&format!(
        "A {SMALL_ENTRANT_BTH}-BTH entrant shares an epoch with a flood. Its factor is measured at \
         its first wallet spend, after the flood has been spent 0, 5 and 20 times. Each flood \
         spend adds the flood's import-weighted value to the shared cumulative wealth, so the \
         entrant's factor keeps rising after the epoch closes.\n\n"
    ));
    s.push_str("| flood regime | flood (BTH) | entrant alone | caught, 0 hops | caught, 5 hops | caught, 20 hops |\n");
    s.push_str("|---|---:|---:|---:|---:|---:|\n");
    for r in &report.collateral_rows {
        s.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            r.regime.label,
            r.regime.epoch_volume_bth,
            fmt_factor(r.alone_factor_scaled),
            fmt_factor(r.caught_factor_scaled[0]),
            fmt_factor(r.caught_factor_scaled[1]),
            fmt_factor(r.caught_factor_scaled[2]),
        ));
    }
    s.push('\n');

    // --- Floor ------------------------------------------------------------
    s.push_str("### Floor F sweep: onboarding toll vs the tag-drop escape\n\n");
    s.push_str(&format!(
        "A {SMALL_ENTRANT_BTH}-BTH entrant alone in its epoch. Fees are the consensus floor as a \
         % of value: its first wallet spend after one day and after one year, and the same coin \
         spent once to background outputs (tag drop) after one day.\n\n"
    ));
    s.push_str("| F | entrant factor | wallet spend @ 1 day | wallet spend @ 1 year | tag-drop spend @ 1 day | residual above background |\n");
    s.push_str("|---|---:|---:|---:|---:|---:|\n");
    for r in &report.floor_rows {
        s.push_str(&format!(
            "| {} | {} | {:.4}% | {:.4}% | {:.4}% | {:.3} |\n",
            fmt_factor(r.floor_scaled),
            fmt_factor(r.small_entrant_factor_scaled),
            r.first_spend_1d_pct,
            r.first_spend_1y_pct,
            r.tag_drop_1d_pct,
            r.residual_above_background,
        ));
    }
    s.push('\n');

    // --- Decay ------------------------------------------------------------
    s.push_str("### Decay by domestic circulation (production tags and wealth)\n\n");
    s.push_str(&format!(
        "A {FLOOD_COIN_BTH}-BTH coin from a {FLOOD_EPOCH_BTH}-BTH epoch. Each spend merges it with \
         an equal-value background input under the wallet's 5% decay, pays half away and keeps \
         half as change. `import W` is the import cluster's cumulative wealth; `next-spend factor` is what the coin's next wallet spend is \
         charged at.\n\n"
    ));
    s.push_str("| spends | import weight | import W (BTH) | next-spend factor | step fee | cumulative fee |\n");
    s.push_str("|---:|---:|---:|---:|---:|---:|\n");
    for r in &report.decay_rows {
        s.push_str(&format!(
            "| {} | {} | {:.0} | {} | {:.4}% | {:.4}% |\n",
            r.spends,
            r.import_weight,
            r.import_wealth_bth,
            fmt_factor(r.next_spend_factor_scaled),
            r.step_fee_pct,
            r.cumulative_fee_pct,
        ));
    }
    s.push('\n');

    // --- Self-churn -------------------------------------------------------
    s.push_str("### Self-churn adversary (no domestic money)\n\n");
    s.push_str(&format!(
        "The coin is spent to its owner every {CHURN_SPACING_BLOCKS} blocks. Nothing domestic \
         enters. Under the default wallet the 5% decay alone strips the import tag, and \
         `ClusterTagVector` prunes it to background below 0.1%. The max-shed spender declares \
         decay {:.1}% per hop (weight ÷ {}), the most it can shed while its claimed wealth stays \
         at the ring-centroid floor, so the downgrade charge does not fire. Both are valid at \
         consensus, which rejects only tag inflation.\n\n",
        max_shed_decay_rate() as f64 / 10_000.0,
        RING_SIZE,
    ));
    for t in &report.churn {
        s.push_str(&format!("**{}**\n\n", t.label));
        s.push_str(
            "| spends | import weight | import W (BTH) | next-spend factor | cumulative fee |\n",
        );
        s.push_str("|---:|---:|---:|---:|---:|\n");
        for &n in &CHURN_PRINT {
            if let Some(r) = t.rows.get(n as usize) {
                s.push_str(&format!(
                    "| {} | {} | {:.0} | {} | {:.4}% |\n",
                    r.spends,
                    r.import_weight,
                    r.import_wealth_bth,
                    fmt_factor(r.next_spend_factor_scaled),
                    r.cumulative_fee_pct,
                ));
            }
        }
        s.push_str(&format!(
            "\nSpends to factor ≤ {f}: {}; to ≤ 1.100x: {}; to tag pruned: {} (cumulative fee {}).\n\n",
            fmt_opt(t.spends_to_floor),
            fmt_opt(t.spends_to_1_1x),
            fmt_opt(t.spends_to_pruned),
            t.fee_pct_to_pruned
                .map(|p| format!("{p:.4}%"))
                .unwrap_or_else(|| "n/a".to_string()),
        ));
    }

    // --- Tag drop ---------------------------------------------------------
    s.push_str("### Tag-drop adversary (background outputs)\n\n");
    s.push_str(
        "First spend after one day, honest wallet vs background outputs. With background \
         outputs the import floor reads 1× (it is computed from output tags); only the \
         ring-centroid floor remains, and 19 background decoys dilute it.\n\n",
    );
    s.push_str(
        "| coin | honest factor | honest fee | drop factor | drop ring floor | drop fee |\n",
    );
    s.push_str("|---|---:|---:|---:|---:|---:|\n");
    for r in &report.tag_drop_rows {
        s.push_str(&format!(
            "| {} | {} | {:.4}% | {} | {} | {:.4}% |\n",
            r.label,
            fmt_factor(r.honest_factor_scaled),
            r.honest_fee_pct,
            fmt_factor(r.drop_factor_scaled),
            fmt_factor(r.drop_ring_floor_scaled),
            r.drop_fee_pct,
        ));
    }
    s.push('\n');

    // --- Gini -------------------------------------------------------------
    s.push_str("### Concentration signal: flood's cheap-money footprint\n\n");
    s.push_str(&format!(
        "100 domestic 1,000-BTH holders plus one {FLOOD_EPOCH_BTH}-BTH flood; Gini of value ÷ \
         factor. Production `import_cluster_factor` prices the flood at {}.\n\n",
        fmt_factor(report.gini.flood_factor_scaled),
    ));
    s.push_str("| flood enters at | Gini (effective-spendable) |\n|---|---:|\n");
    s.push_str(&format!(
        "| factor-1 (ADR 0003 status quo) | {:.4} |\n| import factor (ADR 0007) | {:.4} |\n\n",
        report.gini.gini_flood_at_background, report.gini.gini_flood_at_import_factor,
    ));
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fee_floor_inputs::import_floor_factor_from_outputs;

    #[test]
    fn sim_uses_production_import_cluster_id() {
        // The sweep's unwrap tags must carry the consensus id, not a sim copy.
        let mut l = ProductionLedger::new();
        let coin = l.unwrap(3, 1_000);
        assert_eq!(
            coin.tags.entries[0].cluster_id.0,
            crate::bridge_import::import_cluster_id(3).0
        );
        assert_eq!(
            crate::bridge_import::import_cluster_id_for_height(3 * BRIDGE_IMPORT_EPOCH_BLOCKS).0,
            coin.tags.entries[0].cluster_id.0
        );
    }

    #[test]
    fn sim_factor_matches_production_import_factor_for_sampled_wealth() {
        // A freshly unwrapped coin (100% import-tagged output, no ring) is
        // charged at exactly the production `import_cluster_factor(W)`.
        let curve = ClusterFactorCurve::default_params();
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        for _ in 0..200 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let w_bth = x % 20_000_000 + 1;
            let mut l = ProductionLedger::new();
            let coin = l.unwrap(0, bth(w_bth));
            let outputs = vec![(coin.value, pairs(&coin.tags))];
            let r = l.resolve(&[], &outputs, BRIDGE_IMPORT_FACTOR_FLOOR);
            assert_eq!(
                r.demurrage_factor,
                import_cluster_factor(l.wealth(coin.tags.entries[0].cluster_id.0), &curve),
                "W = {w_bth} BTH"
            );
        }
    }

    #[test]
    fn import_wealth_accumulates_downstream_outputs() {
        // writer.rs: every tagged output adds amount × weight to W.
        let mut l = ProductionLedger::new();
        let coin = l.unwrap(0, bth(1_000));
        let id = import_cluster_id(0).0;
        assert_eq!(l.wealth(id), bth(1_000) as u128);
        let r = spend(
            &mut l,
            &[coin],
            OutputTagPolicy::Wallet,
            BRIDGE_IMPORT_FACTOR_FLOOR,
            1,
            true,
        );
        let w = weight_on(&r.out, id) as u128;
        assert_eq!(w, 950_000, "one 5% wallet decay");
        let added = l.wealth(id) - bth(1_000) as u128;
        assert!(
            added.abs_diff(bth(1_000) as u128 * w / 1_000_000) <= 2,
            "the spend's outputs add their import-weighted value to W"
        );
    }

    #[test]
    fn tag_drop_escapes_the_import_floor() {
        // The import floor reads output tags, so background outputs read 1x.
        let mut l = ProductionLedger::new();
        let coin = l.unwrap(0, bth(1_000));
        let r = spend(
            &mut l,
            &[coin],
            OutputTagPolicy::Drop,
            BRIDGE_IMPORT_FACTOR_FLOOR,
            1,
            false,
        );
        assert_eq!(r.resolved.import_floor, ClusterFactorCurve::FACTOR_SCALE);
        assert!(r.resolved.demurrage_factor < BRIDGE_IMPORT_FACTOR_FLOOR);
        assert_eq!(
            import_floor_factor_from_outputs(&[], &|_| true),
            ClusterFactorCurve::FACTOR_SCALE
        );
    }

    #[test]
    fn self_churn_strips_the_tag_without_domestic_money() {
        let traces = self_churn_traces();
        for t in &traces {
            assert!(
                t.spends_to_pruned.is_some(),
                "{}: the 5% wallet decay must prune the tag",
                t.label
            );
            let first = t.rows.first().unwrap().next_spend_factor_scaled;
            let last = t.rows.last().unwrap().next_spend_factor_scaled;
            assert!(last < first, "{}: churn lowers the factor", t.label);
        }
    }

    #[test]
    fn split_game_scales_with_epoch_and_block_time() {
        let rows = split_game_table(&candidate_epochs(), &[2_000_000]);
        let n = rows[0].epochs_to_floor;
        assert!(n > 1, "the epoch key must resist intra-epoch splitting");
        let mut last = -1.0;
        for r in &rows {
            assert_eq!(r.epochs_to_floor, n, "epochs-to-floor is K-independent");
            assert!(r.split_game_days[0] > last);
            assert!(r.split_game_days[1] > r.split_game_days[0]);
            assert!(r.split_game_days[2] > r.split_game_days[1]);
            last = r.split_game_days[0];
        }
    }

    #[test]
    fn collateral_grows_as_the_flood_circulates() {
        let rows = collateral_table(&volume_regimes(), SMALL_ENTRANT_BTH);
        for r in &rows {
            assert!(r.caught_factor_scaled[0] >= r.alone_factor_scaled);
            assert!(r.caught_factor_scaled[2] >= r.caught_factor_scaled[0]);
        }
    }

    #[test]
    fn decay_by_circulation_is_monotone() {
        let rows = decay_by_circulation();
        for w in rows.windows(2) {
            assert!(w[1].import_weight <= w[0].import_weight);
            assert!(w[1].import_wealth_bth >= w[0].import_wealth_bth);
        }
    }

    #[test]
    fn epoch_grid_tracks_the_ratified_k() {
        assert!(candidate_epochs()
            .iter()
            .any(|e| e.blocks == BRIDGE_IMPORT_EPOCH_BLOCKS));
        assert_eq!(reference_block_secs(), 5);
    }

    #[test]
    fn report_is_reproducible() {
        let a = to_markdown(&run_bridge_import_sweep());
        let b = to_markdown(&run_bridge_import_sweep());
        assert_eq!(a, b, "sweep output must be byte-for-byte deterministic");
        for e in candidate_epochs() {
            assert!(a.contains(e.label));
        }
        for section in [
            "Split-game cost",
            "collateral",
            "Decay by domestic circulation",
            "Self-churn adversary",
            "Tag-drop adversary",
        ] {
            assert!(a.contains(section), "missing section {section}");
        }
    }
}
