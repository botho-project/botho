//! DRT legs of the cluster-tag model (`formal/Botho/Tags/`, #1840).
//!
//! - `merge_weighted`: the wallet's (and the mempool's) tag merge,
//!   `ClusterTagVector::merge_weighted`, called directly. A Rust panic is
//!   compared as `{"panic": true}`, the model's `none`.
//! - `cluster_tag_inheritance`: consensus C6 as block validation runs it,
//!   `ValidationReads::verify_cluster_tag_inheritance` over the in-memory
//!   double `botho::ledger::formal_drt_tags`.
//! - `effective_cluster_wealth`, `import_floor_factor`: the fee floor's two
//!   reads of output tags, through the same double.
//!
//! Generators are biased to empty vectors, 16 and 17 entries, weights 999,
//! 1000, `TAG_WEIGHT_SCALE` and `u32::MAX`, duplicate cluster ids (a small id
//! pool), values 0, 1 and `u64::MAX`, rings of 1 and 20 members, outputs at
//! a ring member's mass and just above it, and (rarely) the 4,295-entry
//! vector that overflows `merge_weighted`.

use crate::{
    fixtures::Fixtures,
    gen,
    legs::Leg,
    wire::{self, n},
};
use botho::ledger::{formal_drt_tags, LedgerError};
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
/// the 4,295 entries of weight `u32::MAX` at which `merge_weighted`
/// overflows. Only the merge leg uses it: C6's cost grows with the square of
/// the entry count, in both implementations.
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
        let total: u128 = i.inputs.iter().map(|(_, v)| *v as u128).sum();
        if r.get("panic").is_some() {
            return if total > u64::MAX as u128 {
                "panic: value sum".into()
            } else {
                "panic: weight * retention".into()
            };
        }
        let len = r["entries"].as_array().map_or(0, Vec::len);
        match len {
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

fn coin() -> BoxedStrategy<CoinIn> {
    (value(), tags())
        .prop_map(|(value, tags)| CoinIn { value, tags })
        .boxed()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InheritIn {
    pub rings: Vec<Vec<CoinIn>>,
    pub outputs: Vec<CoinIn>,
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

fn ring() -> BoxedStrategy<Vec<CoinIn>> {
    prop_oneof![
        3 => vec(coin(), 1..=1),
        1 => vec(coin(), 20..=20),
        2 => vec(coin(), 0..=5),
    ]
    .boxed()
}

impl Leg for Inheritance {
    const NAME: &'static str = "cluster_tag_inheritance";
    const RUST: &'static str =
        "botho::ledger::store::validation::ValidationReads::verify_cluster_tag_inheritance (check_cluster_tag_inheritance)";
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
                let free = vec(coin(), 0..=4).boxed();
                let outs = if members.is_empty() {
                    free
                } else {
                    let copied = (
                        0..members.len(),
                        prop_oneof![Just(999u64), Just(1000u64), Just(1001u64), Just(2000u64)],
                        vec(coin(), 0..=2),
                    )
                        .prop_map(move |(k, num, mut rest)| {
                            rest.insert(0, scaled(&members[k], num));
                            rest
                        });
                    prop_oneof![1 => free, 2 => copied].boxed()
                };
                (Just(rings), outs)
            })
            .prop_map(|(rings, outputs)| InheritIn { rings, outputs })
            .boxed()
    }
    fn args(_: &Fixtures, i: &InheritIn) -> Value {
        json!({
            "rings": i.rings.iter().map(|r| r.iter().map(CoinIn::wire).collect::<Vec<_>>()).collect::<Vec<_>>(),
            "outputs": i.outputs.iter().map(CoinIn::wire).collect::<Vec<_>>(),
        })
    }
    fn rust(_: &Fixtures, i: &InheritIn) -> Value {
        let rings: Vec<Vec<formal_drt_tags::Coin>> = i
            .rings
            .iter()
            .map(|r| r.iter().map(CoinIn::rust).collect())
            .collect();
        let outs: Vec<formal_drt_tags::Coin> = i.outputs.iter().map(CoinIn::rust).collect();
        match formal_drt_tags::cluster_tag_inheritance(&rings, &outs) {
            Ok(()) => json!({ "accepted": true }),
            Err(LedgerError::InvalidBlock(msg)) => inflation_wire(&msg),
            Err(e) => json!({ "rust_err": e.to_string() }),
        }
    }
    fn classify(_: &InheritIn, r: &Value) -> String {
        if r.get("accepted").is_some() {
            "accepted".into()
        } else if r.get("cluster").is_some() {
            "rejected".into()
        } else {
            "other".into()
        }
    }
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
