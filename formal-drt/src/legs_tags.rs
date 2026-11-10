//! DRT legs of the cluster-tag model (`formal/Botho/Tags/`, #1840).
//!
//! - `merge_weighted`: the wallet's (and the mempool's) tag merge,
//!   `ClusterTagVector::merge_weighted`, called directly, against the
//!   post-#1863 model `Botho.Tags.mergeWeightedSafe` (overflow-safe,
//!   saturating). The fixed function never panics; a panic is reported as
//!   `{"panic": true}`, which no model output matches.
//! - `cluster_tag_inheritance`: the protocol 8.0.0 tag rules (#1870) as block
//!   validation runs them at a height: Rule 1 (`first_malformed_output_tags`),
//!   then `ValidationReads::verify_cluster_tag_inheritance(tx, height)` (Rule 2
//!   with the epoch import cluster, then C6) over the in-memory double
//!   `botho::ledger::formal_drt_tags`, against `Botho.Tags.inheritOk₈`. The
//!   Lean side derives the import cluster from the height on its own (SHA-256).
//! - `effective_cluster_wealth`, `import_floor_factor`: the fee floor's two
//!   reads of output tags, through the same double.
//! - `cluster_tag_retention`, `import_floor_from_inputs` (protocol 10.0.0,
//!   #1687): `ValidationReads::verify_cluster_tag_retention` and
//!   `ValidationReads::import_floor_factor_from_inputs` through the same
//!   double, against `Botho.Tags.retainOk` and `Botho.Tags.importFloorIn`.
//!   Inputs carry a pseudo-output amount, usually a member's value and
//!   sometimes shared by a second (same-amount) member; outputs keep 0%, 94.9%,
//!   95%, 95.1% or 100% of a spent member's weights, or the wallet's 5%-decayed
//!   merge.
//!
//! Generators are biased to empty vectors, 16 and 17 entries, weights 999,
//! 1000, `TAG_WEIGHT_SCALE` and `u32::MAX`, duplicate cluster ids (a small id
//! pool), values 0, 1 and `u64::MAX`, rings of 1 and 20 members, outputs at
//! a ring member's mass and just above it, (rarely) the 4,295-entry vector
//! that overflowed the pre-#1863 `merge_weighted`, heights at the
//! bridge-import epoch edges, and output entries naming the epoch's (or an
//! adjacent epoch's) import cluster.

use crate::{
    fixtures::Fixtures,
    gen,
    legs::Leg,
    wire::{self, n},
};
use botho::ledger::{formal_drt_tags, LedgerError};
use bth_cluster_tax::{import_cluster_id_for_height, BRIDGE_IMPORT_EPOCH_BLOCKS};
use bth_transaction_types::{ClusterId, ClusterTagEntry, ClusterTagVector, TAG_WEIGHT_SCALE};
use proptest::{collection::vec, prelude::*};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::panic::{catch_unwind, AssertUnwindSafe};

/// A tag vector as raw `(cluster, weight)` entries, kept malformed if
/// generated so (unsorted, duplicates, any length, any total).
pub type Tags = Vec<(u64, u32)>;

fn rust_tags(t: &Tags) -> ClusterTagVector {
    ClusterTagVector {
        entries: t
            .iter()
            .map(|&(c, w)| ClusterTagEntry {
                cluster_id: ClusterId(c),
                weight: w,
            })
            .collect(),
        decay_state: None,
    }
}

fn tags_wire(t: &Tags) -> Value {
    Value::Array(t.iter().map(|&(c, w)| json!([n(c), n(w)])).collect())
}

fn entries_wire(v: &ClusterTagVector) -> Value {
    Value::Array(
        v.entries
            .iter()
            .map(|e| json!([n(e.cluster_id.0), n(e.weight)]))
            .collect(),
    )
}

/// A value (`u64`) with edges.
fn value() -> BoxedStrategy<u64> {
    prop_oneof![
        3 => 1u64..2_000_000,
        2 => 1_000_000u64..1_000_000_000_000_000,
        1 => Just(0u64),
        1 => Just(1u64),
        1 => Just(u64::MAX),
        2 => gen::u64_edge(),
    ]
    .boxed()
}

/// A value for the merge: mostly sums that fit a `u64` (as every balancing
/// transaction's inputs do), sometimes ones that overflow.
fn merge_value() -> BoxedStrategy<u64> {
    prop_oneof![
        8 => 1u64..1_000_000_000_000_000,
        1 => Just(0u64),
        1 => Just(1u64),
        1 => value(),
    ]
    .boxed()
}

/// A weight (`u32`): the pruning threshold, 100%, the maximum, or anything.
fn weight() -> BoxedStrategy<u32> {
    prop_oneof![
        1 => Just(0u32),
        2 => 998u32..=1_001,
        1 => Just(62_500u32),
        2 => Just(TAG_WEIGHT_SCALE),
        1 => Just(TAG_WEIGHT_SCALE + 1),
        1 => Just(u32::MAX),
        4 => 1_000u32..=TAG_WEIGHT_SCALE,
        1 => any::<u32>(),
    ]
    .boxed()
}

/// A cluster id: mostly from a small pool, so ids collide across vectors.
fn cluster() -> BoxedStrategy<u64> {
    prop_oneof![
        6 => 0u64..8,
        2 => any::<u64>(),
        1 => Just(u64::MAX),
    ]
    .boxed()
}

fn entry() -> BoxedStrategy<(u64, u32)> {
    (cluster(), weight()).boxed()
}

/// A tag vector: valid ones (via `from_pairs`) and raw ones of the edge
/// lengths.
fn tags() -> BoxedStrategy<Tags> {
    let valid = vec(entry(), 0..=20).prop_map(|pairs| {
        let v = ClusterTagVector::from_pairs(
            &pairs
                .iter()
                .map(|&(c, w)| (ClusterId(c), w))
                .collect::<Vec<_>>(),
        );
        v.entries
            .iter()
            .map(|e| (e.cluster_id.0, e.weight))
            .collect::<Tags>()
    });
    prop_oneof![
        3 => Just(Vec::new()),
        6 => vec(entry(), 1..=3),
        4 => valid,
        2 => vec(entry(), 16..=17),
        1 => vec(entry(), 0..=40),
    ]
    .boxed()
}

/// `tags()`, plus (rarely) one cluster repeated 4,293 to 4,297 times, around
/// the 4,295 entries of weight `u32::MAX` at which the pre-#1863
/// `merge_weighted` overflowed (it now saturates). Only the merge leg uses it:
/// C6's cost grows with the square of the entry count, in both implementations.
fn merge_tags() -> BoxedStrategy<Tags> {
    prop_oneof![
        60 => tags(),
        1 => (cluster(), prop_oneof![Just(u32::MAX), any::<u32>()], 0usize..=4)
            .prop_map(|(c, w, d)| vec![(c, w); 4_293 + d]),
    ]
    .boxed()
}

// ---- merge_weighted ---------------------------------------------------------

pub struct MergeWeighted;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MergeIn {
    pub inputs: Vec<(Tags, u64)>,
    pub rate: u32,
}

impl Leg for MergeWeighted {
    const NAME: &'static str = "merge_weighted";
    const RUST: &'static str = "bth_transaction_types::ClusterTagVector::merge_weighted";
    type Input = MergeIn;
    fn strategy() -> BoxedStrategy<MergeIn> {
        let inputs = prop_oneof![
            1 => Just(Vec::new()),
            6 => vec((merge_tags(), merge_value()), 1..=4),
            2 => vec((tags(), merge_value()), 5..=20),
        ];
        let rate = prop_oneof![
            3 => Just(0u32),
            3 => Just(50_000u32),
            1 => Just(TAG_WEIGHT_SCALE),
            1 => Just(TAG_WEIGHT_SCALE + 1),
            1 => Just(u32::MAX),
            1 => any::<u32>(),
        ];
        (inputs, rate)
            .prop_map(|(inputs, rate)| MergeIn { inputs, rate })
            .boxed()
    }
    fn args(_: &Fixtures, i: &MergeIn) -> Value {
        json!({
            "inputs": i.inputs.iter().map(|(t, v)| json!({ "tags": tags_wire(t), "value": n(*v) })).collect::<Vec<_>>(),
            "rate": n(i.rate),
        })
    }
    fn rust(_: &Fixtures, i: &MergeIn) -> Value {
        let inputs: Vec<(ClusterTagVector, u64)> =
            i.inputs.iter().map(|(t, v)| (rust_tags(t), *v)).collect();
        match catch_unwind(AssertUnwindSafe(|| {
            ClusterTagVector::merge_weighted(&inputs, i.rate)
        })) {
            Ok(v) => json!({ "entries": entries_wire(&v) }),
            Err(_) => json!({ "panic": true }),
        }
    }
    fn classify(i: &MergeIn, r: &Value) -> String {
        if r.get("panic").is_some() {
            return "panic".into();
        }
        // The inputs the pre-#1863 function panicked on: now saturating.
        let total: u128 = i.inputs.iter().map(|(_, v)| *v as u128).sum();
        if total > u64::MAX as u128 {
            return "value sum above u64 (pre-#1863 panic)".into();
        }
        let entries = r["entries"].as_array();
        if entries.is_some_and(|es| es.iter().any(|e| e[1] == json!(n(u32::MAX)))) {
            return "saturated weight".into();
        }
        match entries.map_or(0, Vec::len) {
            0 => "empty".into(),
            16 => "16 entries".into(),
            _ => "1-15 entries".into(),
        }
    }
}

// ---- cluster_tag_inheritance ------------------------------------------------

pub struct Inheritance;

/// A ring member or an output.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CoinIn {
    pub value: u64,
    pub tags: Tags,
}

impl CoinIn {
    fn rust(&self) -> formal_drt_tags::Coin {
        (self.value, rust_tags(&self.tags))
    }
    fn wire(&self) -> Value {
        json!({ "value": n(self.value), "tags": tags_wire(&self.tags) })
    }
}

/// A tag vector that passes `is_valid` (Rule 1, #1870): distinct clusters,
/// scaled to a total of at most 100%, normalized by `from_pairs` (which
/// prunes below `MIN_STORED_WEIGHT`, sorts and truncates to 16).
fn valid_tags() -> BoxedStrategy<Tags> {
    vec((cluster(), 1_000u32..=TAG_WEIGHT_SCALE), 0..=18)
        .prop_map(|pairs| {
            let mut seen = std::collections::BTreeSet::new();
            let pairs: Vec<(u64, u32)> =
                pairs.into_iter().filter(|(c, _)| seen.insert(*c)).collect();
            let total: u64 = pairs.iter().map(|&(_, w)| w as u64).sum();
            let scaled: Vec<(ClusterId, u32)> = pairs
                .iter()
                .map(|&(c, w)| {
                    let w = if total > TAG_WEIGHT_SCALE as u64 {
                        (w as u64 * TAG_WEIGHT_SCALE as u64 / total) as u32
                    } else {
                        w
                    };
                    (ClusterId(c), w)
                })
                .collect();
            let v = ClusterTagVector::from_pairs(&scaled);
            debug_assert!(v.is_valid());
            v.entries
                .iter()
                .map(|e| (e.cluster_id.0, e.weight))
                .collect()
        })
        .boxed()
}

/// A coin for the tag-rule leg: half the time with `is_valid` tags, so that
/// Rule 2 and C6 are reached past Rule 1.
fn rule_coin() -> BoxedStrategy<CoinIn> {
    prop_oneof![
        1 => coin(),
        1 => (value(), valid_tags()).prop_map(|(value, tags)| CoinIn { value, tags }),
    ]
    .boxed()
}

fn coin() -> BoxedStrategy<CoinIn> {
    (value(), tags())
        .prop_map(|(value, tags)| CoinIn { value, tags })
        .boxed()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InheritIn {
    pub rings: Vec<Vec<CoinIn>>,
    pub outputs: Vec<CoinIn>,
    /// The including block's height: selects the epoch import cluster that
    /// Rule 2 admits.
    pub height: u64,
}

/// A block height, biased to the bridge-import epoch edges.
fn height() -> BoxedStrategy<u64> {
    let k = BRIDGE_IMPORT_EPOCH_BLOCKS;
    prop_oneof![
        2 => 0u64..(3 * k),
        1 => Just(0u64),
        1 => Just(k - 1),
        1 => Just(k),
        1 => Just(2 * k - 1),
        1 => Just(u64::MAX),
        1 => any::<u64>(),
    ]
    .boxed()
}

/// An entry naming the import cluster of `height`'s epoch (mostly) or of an
/// adjacent epoch.
fn import_entry(height: u64) -> BoxedStrategy<(u64, u32)> {
    let k = BRIDGE_IMPORT_EPOCH_BLOCKS;
    let at = |h: u64| import_cluster_id_for_height(h).0;
    let cluster = prop_oneof![
        4 => Just(at(height)),
        1 => Just(at(height.saturating_add(k))),
        1 => Just(at(height.saturating_sub(k))),
    ];
    (cluster, weight()).boxed()
}

/// An output that copies a ring member, with its weights scaled by
/// `num / 1000` (999: just under, 1000: at, 1001 and 2000: above).
fn scaled(m: &CoinIn, num: u64) -> CoinIn {
    CoinIn {
        value: m.value,
        tags: m
            .tags
            .iter()
            .map(|&(c, w)| (c, (w as u64 * num / 1000).min(u32::MAX as u64) as u32))
            .collect(),
    }
}

/// An output that copies a ring member's tags unchanged (so they pass Rule 1
/// whenever the member's do) with its value scaled by `num / 1000`: the C6
/// mass bound, reached past Rules 1 and 2 (#1870).
fn scaled_value(m: &CoinIn, num: u64) -> CoinIn {
    CoinIn {
        value: (m.value as u128 * num as u128 / 1000).min(u64::MAX as u128) as u64,
        tags: m.tags.clone(),
    }
}

fn ring() -> BoxedStrategy<Vec<CoinIn>> {
    prop_oneof![
        3 => vec(rule_coin(), 1..=1),
        1 => vec(rule_coin(), 20..=20),
        2 => vec(rule_coin(), 0..=5),
    ]
    .boxed()
}

impl Leg for Inheritance {
    const NAME: &'static str = "cluster_tag_inheritance";
    const RUST: &'static str =
        "botho::ledger::store::first_malformed_output_tags, then botho::ledger::store::validation::ValidationReads::verify_cluster_tag_inheritance(tx, height) (check_cluster_tag_inheritance)";
    type Input = InheritIn;
    fn strategy() -> BoxedStrategy<InheritIn> {
        let rings = prop_oneof![
            1 => Just(Vec::new()),
            6 => vec(ring(), 1..=1),
            3 => vec(ring(), 2..=3),
        ];
        rings
            .prop_flat_map(|rings| {
                let members: Vec<CoinIn> = rings.iter().flatten().cloned().collect();
                let free = vec(rule_coin(), 0..=4).boxed();
                let outs = if members.is_empty() {
                    free
                } else {
                    let copied = (
                        0..members.len(),
                        prop_oneof![Just(999u64), Just(1000u64), Just(1001u64), Just(2000u64)],
                        any::<bool>(),
                        vec(rule_coin(), 0..=1),
                    )
                        .prop_map(move |(k, num, by_value, mut rest)| {
                            let m = &members[k];
                            let o = if by_value {
                                scaled_value(m, num)
                            } else {
                                scaled(m, num)
                            };
                            rest.insert(0, o);
                            rest
                        });
                    prop_oneof![1 => free, 2 => copied].boxed()
                };
                (Just(rings), outs, height())
            })
            .prop_flat_map(|(rings, outputs, height)| {
                // Sometimes one output gains an entry naming an import
                // cluster (an unwrap's minted output, or a release included
                // in the wrong epoch), either replacing its tags or inserted
                // at its weight's sorted position, so the vector can stay
                // `is_valid`.
                let n = outputs.len();
                let inject = if n == 0 {
                    Just(None).boxed()
                } else {
                    prop_oneof![
                        2 => Just(None),
                        1 => (0..n, import_entry(height), any::<bool>()).prop_map(Some),
                    ]
                    .boxed()
                };
                (Just(rings), Just(outputs), Just(height), inject)
            })
            .prop_map(|(rings, mut outputs, height, inject)| {
                if let Some((k, (c, w), replace)) = inject {
                    let tags = &mut outputs[k].tags;
                    if replace || tags.is_empty() {
                        *tags = vec![(c, w.min(TAG_WEIGHT_SCALE))];
                    } else {
                        // Keep the vector sorted by weight, descending.
                        let pos = tags.iter().position(|&(_, x)| x < w).unwrap_or(tags.len());
                        tags.insert(pos, (c, w));
                    }
                }
                InheritIn {
                    rings,
                    outputs,
                    height,
                }
            })
            .boxed()
    }
    fn args(_: &Fixtures, i: &InheritIn) -> Value {
        json!({
            "rings": i.rings.iter().map(|r| r.iter().map(CoinIn::wire).collect::<Vec<_>>()).collect::<Vec<_>>(),
            "outputs": i.outputs.iter().map(CoinIn::wire).collect::<Vec<_>>(),
            "height": n(i.height),
        })
    }
    fn rust(_: &Fixtures, i: &InheritIn) -> Value {
        let rings: Vec<Vec<formal_drt_tags::Coin>> = i
            .rings
            .iter()
            .map(|r| r.iter().map(CoinIn::rust).collect())
            .collect();
        let outs: Vec<formal_drt_tags::Coin> = i.outputs.iter().map(CoinIn::rust).collect();
        // Rule 1 first, as `validate_ordinary_body` applies it.
        if let Some(k) = formal_drt_tags::malformed_output(&outs) {
            return json!({ "rule1": n(k) });
        }
        match formal_drt_tags::cluster_tag_inheritance(&rings, &outs, i.height) {
            Ok(()) => json!({ "accepted": true }),
            Err(LedgerError::InvalidBlock(msg)) => rejection_wire(&msg),
            Err(e) => json!({ "rust_err": e.to_string() }),
        }
    }
    fn classify(_: &InheritIn, r: &Value) -> String {
        if r.get("accepted").is_some() {
            "accepted".into()
        } else if r.get("rule1").is_some() {
            "rejected: rule 1".into()
        } else if r.get("rule2").is_some() {
            "rejected: rule 2".into()
        } else if r.get("cluster").is_some() {
            "rejected: C6".into()
        } else {
            "other".into()
        }
    }
}

/// The numbers of Rule 2's error (`"output {i} names cluster {c} absent from
/// every ring (not the epoch import cluster)"`) or of C6's (below).
fn rejection_wire(msg: &str) -> Value {
    if msg.contains("absent from every ring") {
        let nums: Vec<&str> = msg
            .split(|ch: char| !ch.is_ascii_digit())
            .filter(|s| !s.is_empty())
            .collect();
        return match nums.as_slice() {
            [i, c] => json!({ "rule2": { "output": i, "cluster": c } }),
            _ => json!({ "rust_err": msg }),
        };
    }
    inflation_wire(msg)
}

/// The numbers of C6's error, `"cluster {c} tag inflation: output mass {a}
/// exceeds expected {e} (input mass {i}, tolerance {t})"`.
fn inflation_wire(msg: &str) -> Value {
    let nums: Vec<&str> = msg
        .split(|ch: char| !ch.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .collect();
    match nums.as_slice() {
        [c, a, e, i, t] if msg.contains("tag inflation") => json!({
            "cluster": c, "actual": a, "expected": e, "input": i, "tolerance": t,
        }),
        _ => json!({ "rust_err": msg }),
    }
}

// ---- effective_cluster_wealth ----------------------------------------------

pub struct EffectiveWealth;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WealthRow {
    pub cluster: u64,
    #[serde(with = "wire::dec_u128")]
    pub wealth: u128,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WealthIn {
    pub outputs: Vec<CoinIn>,
    /// Distinct clusters.
    pub wealth: Vec<WealthRow>,
}

impl Leg for EffectiveWealth {
    const NAME: &'static str = "effective_cluster_wealth";
    const RUST: &'static str =
        "botho::ledger::store::validation::ValidationReads::effective_cluster_wealth_from_outputs";
    type Input = WealthIn;
    fn strategy() -> BoxedStrategy<WealthIn> {
        (
            vec(coin(), 0..=4),
            proptest::collection::btree_map(0u64..8, gen::u128_edge(), 0..=8),
        )
            .prop_map(|(outputs, wealth)| WealthIn {
                outputs,
                wealth: wealth
                    .into_iter()
                    .map(|(cluster, wealth)| WealthRow { cluster, wealth })
                    .collect(),
            })
            .boxed()
    }
    fn args(_: &Fixtures, i: &WealthIn) -> Value {
        json!({
            "outputs": i.outputs.iter().map(CoinIn::wire).collect::<Vec<_>>(),
            "wealth": i.wealth.iter().map(|r| json!([n(r.cluster), n(r.wealth)])).collect::<Vec<_>>(),
        })
    }
    fn rust(_: &Fixtures, i: &WealthIn) -> Value {
        let outs: Vec<formal_drt_tags::Coin> = i.outputs.iter().map(CoinIn::rust).collect();
        let table: Vec<(u64, u128)> = i.wealth.iter().map(|r| (r.cluster, r.wealth)).collect();
        match formal_drt_tags::effective_cluster_wealth(&outs, &table) {
            Ok(w) => n(w),
            Err(e) => json!({ "rust_err": e.to_string() }),
        }
    }
    fn classify(_: &WealthIn, r: &Value) -> String {
        match r.as_str() {
            Some("0") => "zero".into(),
            Some(s) if s.parse::<u128>().ok() == Some(u128::MAX) => "saturated".into(),
            Some(_) => "positive".into(),
            None => "error".into(),
        }
    }
}

// ---- import_floor_factor ----------------------------------------------------

pub struct ImportFloor;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportIn {
    pub outputs: Vec<CoinIn>,
    pub imports: Vec<u64>,
}

impl Leg for ImportFloor {
    const NAME: &'static str = "import_floor_factor";
    const RUST: &'static str =
        "botho::ledger::store::validation::ValidationReads::import_floor_factor_from_outputs";
    type Input = ImportIn;
    fn strategy() -> BoxedStrategy<ImportIn> {
        (vec(coin(), 0..=4), vec(cluster(), 0..=4))
            .prop_map(|(outputs, imports)| ImportIn { outputs, imports })
            .boxed()
    }
    fn args(_: &Fixtures, i: &ImportIn) -> Value {
        json!({
            "outputs": i.outputs.iter().map(CoinIn::wire).collect::<Vec<_>>(),
            "imports": i.imports.iter().map(|c| n(*c)).collect::<Vec<_>>(),
        })
    }
    fn rust(_: &Fixtures, i: &ImportIn) -> Value {
        let outs: Vec<formal_drt_tags::Coin> = i.outputs.iter().map(CoinIn::rust).collect();
        match formal_drt_tags::import_floor_factor(&outs, &i.imports) {
            Ok(f) => n(f),
            Err(e) => json!({ "rust_err": e.to_string() }),
        }
    }
    fn classify(_: &ImportIn, r: &Value) -> String {
        match r.as_str() {
            Some("1000") => "1x".into(),
            Some("1500") => "F".into(),
            Some(_) => "blend".into(),
            None => "error".into(),
        }
    }
}

// ---- cluster_tag_retention, import_floor_from_inputs (#1687) ---------------

/// One input: its pseudo-output amount and its resolved ring.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpendIn {
    pub pseudo: u64,
    pub ring: Vec<CoinIn>,
}

impl SpendIn {
    fn rust(&self) -> (u64, Vec<formal_drt_tags::Coin>) {
        (self.pseudo, self.ring.iter().map(CoinIn::rust).collect())
    }
    fn wire(&self) -> Value {
        json!({
            "pseudo": n(self.pseudo),
            "ring": self.ring.iter().map(CoinIn::wire).collect::<Vec<_>>(),
        })
    }
}

/// An input whose pseudo-output amount is (mostly) one member's value, and
/// sometimes also a second member's (a same-amount decoy). The spent member's
/// index is returned with it.
fn spend_input() -> BoxedStrategy<(SpendIn, usize)> {
    let ring = prop_oneof![
        4 => vec(rule_coin(), 1..=5),
        1 => vec(rule_coin(), 20..=20),
    ];
    (
        ring,
        any::<prop::sample::Index>(),
        any::<prop::sample::Index>(),
        0u8..10,
        value(),
    )
        .prop_map(|(mut ring, real, twin, mode, other)| {
            let k = real.index(ring.len());
            let pseudo = match mode {
                // No member of the pseudo amount: every member is a candidate.
                0 => other,
                // A same-amount decoy.
                1 | 2 => {
                    let j = twin.index(ring.len());
                    ring[j].value = ring[k].value;
                    ring[k].value
                }
                _ => ring[k].value,
            };
            (SpendIn { pseudo, ring }, k)
        })
        .boxed()
}

fn inputs_wire(inputs: &[SpendIn]) -> Value {
    Value::Array(inputs.iter().map(SpendIn::wire).collect())
}

fn inputs_rust(inputs: &[SpendIn]) -> Vec<(u64, Vec<formal_drt_tags::Coin>)> {
    inputs.iter().map(SpendIn::rust).collect()
}

pub struct Retention;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RetentionIn {
    pub inputs: Vec<SpendIn>,
    pub outputs: Vec<CoinIn>,
}

impl Leg for Retention {
    const NAME: &'static str = "cluster_tag_retention";
    const RUST: &'static str =
        "botho::ledger::store::validation::ValidationReads::verify_cluster_tag_retention (bth_cluster_tax::first_tag_retention_violation)";
    type Input = RetentionIn;
    fn strategy() -> BoxedStrategy<RetentionIn> {
        vec(spend_input(), 1..=3)
            .prop_flat_map(|spent| {
                let real: Vec<CoinIn> = spent.iter().map(|(i, k)| i.ring[*k].clone()).collect();
                let merged: Tags = {
                    let inputs: Vec<(ClusterTagVector, u64)> = spent
                        .iter()
                        .zip(&real)
                        .map(|((i, _), m)| (rust_tags(&m.tags), i.pseudo))
                        .collect();
                    ClusterTagVector::merge_weighted(&inputs, 50_000)
                        .entries
                        .iter()
                        .map(|e| (e.cluster_id.0, e.weight))
                        .collect()
                };
                let inputs: Vec<SpendIn> = spent.into_iter().map(|(i, _)| i).collect();
                let n_real = real.len();
                let derived = (
                    0..n_real,
                    prop_oneof![
                        Just(0u64),
                        Just(949u64),
                        Just(950u64),
                        Just(951u64),
                        Just(1000u64)
                    ],
                    value(),
                )
                    .prop_map(move |(k, num, v)| CoinIn {
                        value: v,
                        tags: scaled(&real[k], num).tags,
                    });
                let honest =
                    (value(), Just(merged)).prop_map(|(value, tags)| CoinIn { value, tags });
                let output = prop_oneof![3 => derived, 2 => honest, 1 => rule_coin()];
                (Just(inputs), vec(output, 1..=3))
            })
            .prop_map(|(inputs, outputs)| RetentionIn { inputs, outputs })
            .boxed()
    }
    fn args(_: &Fixtures, i: &RetentionIn) -> Value {
        json!({
            "inputs": inputs_wire(&i.inputs),
            "outputs": i.outputs.iter().map(CoinIn::wire).collect::<Vec<_>>(),
        })
    }
    fn rust(_: &Fixtures, i: &RetentionIn) -> Value {
        let outs: Vec<formal_drt_tags::Coin> = i.outputs.iter().map(CoinIn::rust).collect();
        match formal_drt_tags::cluster_tag_retention(&inputs_rust(&i.inputs), &outs) {
            Ok(()) => json!({ "accepted": true }),
            Err(LedgerError::InvalidBlock(msg)) => retention_wire(&msg),
            Err(e) => json!({ "rust_err": e.to_string() }),
        }
    }
    fn classify(_: &RetentionIn, r: &Value) -> String {
        if r.get("accepted").is_some() {
            "accepted".into()
        } else if r.get("required").is_some() {
            "rejected: below the floor".into()
        } else {
            "other".into()
        }
    }
}

/// The numbers of the retention error, `"output {o} keeps weight {a} on
/// cluster {c}, below the retained-weight floor {r} (protocol 10.0.0,
/// #1687)"`.
fn retention_wire(msg: &str) -> Value {
    let nums: Vec<&str> = msg
        .split(|ch: char| !ch.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .collect();
    match nums.as_slice() {
        [o, a, c, r, ..] if msg.contains("retained-weight floor") => json!({
            "output": o, "actual": a, "cluster": c, "required": r,
        }),
        _ => json!({ "rust_err": msg }),
    }
}

pub struct ImportFloorInputs;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportInputsIn {
    pub inputs: Vec<SpendIn>,
    pub imports: Vec<u64>,
}

impl Leg for ImportFloorInputs {
    const NAME: &'static str = "import_floor_from_inputs";
    const RUST: &'static str =
        "botho::ledger::store::validation::ValidationReads::import_floor_factor_from_inputs (bth_cluster_tax::import_floor_factor_from_inputs)";
    type Input = ImportInputsIn;
    fn strategy() -> BoxedStrategy<ImportInputsIn> {
        (vec(spend_input(), 0..=3), vec(cluster(), 0..=4))
            .prop_map(|(spent, imports)| ImportInputsIn {
                inputs: spent.into_iter().map(|(i, _)| i).collect(),
                imports,
            })
            .boxed()
    }
    fn args(_: &Fixtures, i: &ImportInputsIn) -> Value {
        json!({
            "inputs": inputs_wire(&i.inputs),
            "imports": i.imports.iter().map(|c| n(*c)).collect::<Vec<_>>(),
        })
    }
    fn rust(_: &Fixtures, i: &ImportInputsIn) -> Value {
        match formal_drt_tags::import_floor_factor_from_inputs(&inputs_rust(&i.inputs), &i.imports)
        {
            Ok(f) => n(f),
            Err(e) => json!({ "rust_err": e.to_string() }),
        }
    }
    fn classify(_: &ImportInputsIn, r: &Value) -> String {
        match r.as_str() {
            Some("1000") => "1x".into(),
            Some("1500") => "F".into(),
            Some(_) => "blend".into(),
            None => "error".into(),
        }
    }
}
