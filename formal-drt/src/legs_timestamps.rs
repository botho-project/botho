//! Five public timestamp helpers and three production scalar predicates.
//! No full validator, wall-clock read, or SCP/physical-clock history
//! is executed.

use crate::{
    fixtures::Fixtures,
    legs::Leg,
    wire::{self, n},
};
use botho::{
    block::difficulty::observed_block_time,
    ledger::{
        ledger_timestamp_within_future_bound, median_time_past_of,
        minting_timestamp_within_future_bound, timestamp_after_mtp, ChainState,
        MAX_FUTURE_TIMESTAMP_SECS,
    },
    node::minter::minting_timestamp,
};
use proptest::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
struct Time(#[serde(with = "wire::dec_u64")] u64);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    window: Vec<Time>,
    parent: Time,
    stamp: Time,
    minter_now: Time,
    validator_now: Time,
}

impl Input {
    fn new(window: Vec<u64>, parent: u64, stamp: u64, minter: u64, validator: u64) -> Self {
        Self {
            window: window.into_iter().map(Time).collect(),
            parent: Time(parent),
            stamp: Time(stamp),
            minter_now: Time(minter),
            validator_now: Time(validator),
        }
    }
}

fn result(i: &Input) -> Value {
    let window: Vec<_> = i.window.iter().map(|t| t.0).collect();
    let state = ChainState {
        recent_timestamps: window.clone(),
        ..Default::default()
    };
    let mtp = median_time_past_of(&window);
    let next_min = state.next_min_timestamp();
    let honest = minting_timestamp(i.minter_now.0, next_min);
    let rolled = state.rolled_timestamps(i.stamp.0);
    let observed_gap = observed_block_time(i.parent.0, i.stamp.0);
    // Evaluate each primitive independently, even when the conjunction fails.
    let lower_candidate = timestamp_after_mtp(i.stamp.0, mtp);
    let lower_honest = timestamp_after_mtp(honest, mtp);
    let future_candidate = ledger_timestamp_within_future_bound(i.stamp.0, i.validator_now.0);
    let future_honest = ledger_timestamp_within_future_bound(honest, i.validator_now.0);
    // This guard belongs only to DRT. Production retains ordinary addition.
    let minting = if i.validator_now.0 > u64::MAX - MAX_FUTURE_TIMESTAMP_SECS {
        json!({"error": "validator_now_out_of_domain"})
    } else {
        let candidate = minting_timestamp_within_future_bound(i.stamp.0, i.validator_now.0);
        let honest = minting_timestamp_within_future_bound(honest, i.validator_now.0);
        json!({"contextual_candidate": lower_candidate && candidate,
            "contextual_honest": lower_honest && honest,
            "intrinsic_candidate": candidate, "intrinsic_honest": honest})
    };
    json!({"mtp": n(mtp), "next_min": n(next_min), "honest_stamp": n(honest),
        "rolled": rolled.into_iter().map(n).collect::<Vec<_>>(),
        "observed_gap": observed_gap.map(n),
        "lower_candidate": lower_candidate, "lower_honest": lower_honest,
        "ledger_future_candidate": future_candidate, "ledger_future_honest": future_honest,
        "ledger_candidate": lower_candidate && future_candidate,
        "ledger_honest": lower_honest && future_honest, "minting": minting})
}

fn edge() -> BoxedStrategy<u64> {
    prop_oneof![2 => any::<u64>(), 5 => prop::sample::select(vec![
        0, 1, 2, 3, 4, 59, 60, 61, 999, 1000, 1060, 1061, (1 << 53) + 1,
        u64::MAX - 61, u64::MAX - 60, u64::MAX - 59, u64::MAX - 1, u64::MAX])]
    .boxed()
}

fn lengths() -> BoxedStrategy<usize> {
    prop_oneof![3 => prop::sample::select(vec![0, 1, 5, 6, 10, 11, 12, 32]), 1 => 0usize..=32]
        .boxed()
}

fn windows() -> BoxedStrategy<Vec<u64>> {
    prop_oneof![
        3 => lengths().prop_flat_map(|len| prop::collection::vec(edge(), len)),
        3 => (lengths(), edge(), 0u8..5).prop_map(|(len, base, shape)| {
            let mut w = vec![base; len];
            for (j, t) in w.iter_mut().enumerate() {
                *t = match shape {
                    1 => base.saturating_add(j as u64),
                    2 => base.saturating_add((len - j) as u64),
                    3 if j == 0 => u64::MAX,
                    4 if j % 2 == 0 => 0,
                    _ => base,
                };
            }
            w
        }),
        2 => (lengths(), prop::sample::select(vec![u64::MAX, u64::MAX - 1]))
            .prop_map(|(len, t)| vec![t; len]),
    ]
    .boxed()
}

pub struct Timestamps;
impl Leg for Timestamps {
    const NAME: &'static str = "timestamps";
    const RUST: &'static str = "botho::{ledger::median_time_past_of, ChainState::{next_min_timestamp,rolled_timestamps}, node::minter::minting_timestamp, block::difficulty::observed_block_time}; ledger::{timestamp_after_mtp,ledger_timestamp_within_future_bound,minting_timestamp_within_future_bound}; scalar predicates, not full validators";
    type Input = Input;

    fn strategy() -> BoxedStrategy<Input> {
        prop_oneof![
            2 => (prop::collection::vec(any::<u64>(), 0..=32), any::<u64>(), any::<u64>(), any::<u64>(), any::<u64>())
                .prop_map(|(w,p,t,n,v)| Input::new(w,p,t,n,v)),
            2 => (windows(), edge(), edge(), edge(), edge())
                .prop_map(|(w,p,t,n,v)| Input::new(w,p,t,n,v)),
            5 => (windows(), edge(), edge(), 0u8..9, 0u8..5).prop_map(|(w,p,v,g,c)| {
                // Correlated input generation targets boundaries; results still call all real helpers.
                let m = median_time_past_of(&w);
                let next = m.saturating_add(1);
                let cap = v.saturating_add(60);
                let t = match g { 0 => p.saturating_sub(1), 1 => p, 2..=4 => p.saturating_add(g as u64),
                    5 => m, 6 => next, 7 => cap, _ => cap.saturating_add(1) };
                let n = match c { 0 => next.saturating_sub(1), 1 => next, 2 => next.saturating_add(1),
                    3 => v, _ => cap.saturating_add(1) };
                Input::new(w,p,t,n,v)
            }),
        ].boxed()
    }

    fn args(_: &Fixtures, i: &Input) -> Value {
        serde_json::to_value(i).unwrap()
    }
    fn rust(_: &Fixtures, i: &Input) -> Value {
        result(i)
    }
    fn classify(i: &Input, r: &Value) -> String {
        let size = match i.window.len() {
            0 => "empty",
            1..=5 => "1..5",
            6..=10 => "6..10",
            11 => "11",
            _ => "overlong",
        };
        let gap = if r["observed_gap"].is_null() {
            "none"
        } else if r["observed_gap"] == "3" {
            "floored"
        } else {
            "unfloored"
        };
        let domain = if r["minting"].get("error").is_some() {
            "out_of_domain"
        } else {
            "common"
        };
        let mut label = format!(
            "window={size}/gap={gap}/successor_saturated={}/minting_domain={domain}",
            r["mtp"] == n(u64::MAX)
        );
        for field in [
            "lower_candidate",
            "lower_honest",
            "ledger_future_candidate",
            "ledger_future_honest",
            "ledger_candidate",
            "ledger_honest",
        ] {
            label.push_str(&format!("/{field}={}", r[field]));
        }
        if domain == "common" {
            for field in [
                "contextual_candidate",
                "contextual_honest",
                "intrinsic_candidate",
                "intrinsic_honest",
            ] {
                label.push_str(&format!("/minting_{field}={}", r["minting"][field]));
            }
        }
        label
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use proptest::{strategy::ValueTree, test_runner::TestRunner};
    use std::path::Path;

    fn model() -> Model {
        Model::spawn(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("lean/.lake/build/bin/botho-model"),
        )
        .expect("build formal-drt/lean before testing")
    }
    fn roundtrip(i: &Input) -> Value {
        let v = serde_json::to_value(i).unwrap();
        assert_eq!(serde_json::from_value::<Input>(v.clone()).unwrap(), *i);
        for field in ["parent", "stamp", "minter_now", "validator_now"] {
            assert!(v[field].is_string());
        }
        assert!(v["window"].as_array().unwrap().iter().all(Value::is_string));
        v
    }

    #[test]
    fn timestamps_model_entrypoint() {
        let input = json!({"window": [], "parent": "0", "stamp": "1", "minter_now": "0", "validator_now": "0"});
        assert_eq!(
            model().call_batch(&[wire::request("timestamps", &input)]),
            vec![json!({
                "mtp": "0", "next_min": "1", "honest_stamp": "1", "rolled": ["1"],
                "observed_gap": null, "lower_candidate": true, "lower_honest": true,
                "ledger_future_candidate": true, "ledger_future_honest": true,
                "ledger_candidate": true, "ledger_honest": true,
                "minting": {"contextual_candidate": true, "contextual_honest": true,
                    "intrinsic_candidate": true, "intrinsic_honest": true}
            })]
        );
    }

    #[test]
    fn minting_domain_and_independent_primitives() {
        let mut model = model();
        for now in [u64::MAX - 60, u64::MAX - 59, u64::MAX - 1, u64::MAX] {
            let i = Input::new(vec![u64::MAX; 11], 0, u64::MAX, u64::MAX, now);
            let r = result(&i);
            assert_eq!(r["lower_candidate"], false);
            assert_eq!(r["ledger_future_candidate"], true);
            assert_eq!(r["ledger_candidate"], false);
            if now == u64::MAX - 60 {
                assert_eq!(
                    r["minting"],
                    json!({"contextual_candidate": false,
                    "contextual_honest": false, "intrinsic_candidate": true, "intrinsic_honest": true})
                );
            } else {
                assert_eq!(
                    r["minting"],
                    json!({"error": "validator_now_out_of_domain"})
                );
            }
            assert_eq!(
                model.call_batch(&[wire::request(Timestamps::NAME, &roundtrip(&i))]),
                vec![r]
            );
        }
        // A failed lower clause must not mask a changed upper clause.
        for (stamp, future) in [(1060, true), (1061, false)] {
            let r = result(&Input::new(vec![2000; 11], 0, stamp, stamp, 1000));
            assert_eq!(r["lower_candidate"], false);
            assert_eq!(r["ledger_future_candidate"], future);
            assert_eq!(r["ledger_candidate"], false);
            assert_eq!(r["minting"]["intrinsic_candidate"], future);
        }
    }

    #[test]
    fn strict_scalar_and_array_codecs() {
        let mut model = model();
        for t in [
            0,
            (1 << 53) + 1,
            u64::MAX - 60,
            u64::MAX - 59,
            u64::MAX - 1,
            u64::MAX,
        ] {
            // Decode/replay accepts windows longer than the random generation bound.
            let i = Input::new(vec![t; 64], t, t, t, t);
            let v = roundtrip(&i);
            let r = result(&i);
            assert_eq!(
                model.call_batch(&[wire::request(Timestamps::NAME, &v)]),
                vec![r.clone()]
            );
            for field in ["mtp", "next_min", "honest_stamp"] {
                assert!(r[field].is_string());
            }
            assert!(r["rolled"].as_array().unwrap().iter().all(Value::is_string));
            assert_eq!(r["observed_gap"].is_null(), t == 0);
            assert!(r["observed_gap"].is_null() || r["observed_gap"].is_string());
        }
        let valid = roundtrip(&Input::new(vec![0], 0, 1, 0, 0));
        let mut invalid = Vec::new();
        for field in ["parent", "stamp", "minter_now", "validator_now", "window"] {
            for bad in [
                json!(1),
                json!(null),
                json!(true),
                json!(""),
                json!("-1"),
                json!("+1"),
                json!("1.5"),
                json!("1e3"),
                json!(" 1"),
                json!("1 "),
                json!("١"),
                json!("１"),
                json!("18446744073709551616"),
            ] {
                let mut v = valid.clone();
                if field == "window" {
                    v[field][0] = bad;
                } else {
                    v[field] = bad;
                }
                invalid.push(v);
            }
            let mut v = valid.clone();
            v.as_object_mut().unwrap().remove(field);
            invalid.push(v);
        }
        let mut v = valid.clone();
        v["unknown"] = json!("0");
        invalid.push(v);
        for shape in [
            json!(null),
            json!(true),
            json!(0),
            json!("0"),
            json!({}),
            json!([[]]),
        ] {
            let mut v = valid.clone();
            v["window"] = shape;
            invalid.push(v);
        }
        let lines: Vec<_> = invalid
            .iter()
            .map(|v| {
                assert!(serde_json::from_value::<Input>(v.clone()).is_err(), "{v}");
                wire::request(Timestamps::NAME, v)
            })
            .collect();
        let responses = model.call_batch(&lines);
        for (v, r) in invalid.iter().zip(responses) {
            assert!(r.get("lean_err").is_some(), "accepted {v}: {r}");
        }
    }

    #[test]
    fn strategy_and_shrinks_preserve_lossless_inputs() {
        let mut runner = TestRunner::deterministic();
        let (mut long, mut wide, mut independent) = (false, false, false);
        for _ in 0..256 {
            let mut tree = Timestamps::strategy().new_tree(&mut runner).unwrap();
            let i = tree.current();
            long |= i.window.len() > 11;
            wide |= i.window.iter().any(|t| t.0 > 1 << 53);
            independent |= i.window.last().is_some_and(|t| *t != i.parent);
            for step in 0..256 {
                let i = tree.current();
                assert!(i.window.len() <= 32);
                roundtrip(&i);
                assert!(!Timestamps::classify(&i, &result(&i)).is_empty());
                if !tree.simplify() {
                    break;
                }
                roundtrip(&tree.current());
                if step % 3 == 0 {
                    tree.complicate();
                    roundtrip(&tree.current());
                }
            }
        }
        assert!(long && wide && independent);
    }

    #[test]
    fn seeded_coverage_exercises_domains_and_predicate_outcomes() {
        use proptest::test_runner::{Config, RngAlgorithm, TestRng};
        use sha2::{Digest, Sha256};
        use std::collections::BTreeSet;
        // Same seed derivation as the CLI runner, including the leg name.
        let mut hash = Sha256::new();
        hash.update(b"botho-formal-drt");
        hash.update(1915u64.to_le_bytes());
        hash.update(Timestamps::NAME.as_bytes());
        let seed: [u8; 32] = hash.finalize().into();
        let mut runner = TestRunner::new_with_rng(
            Config::default(),
            TestRng::from_seed(RngAlgorithm::ChaCha, &seed),
        );
        let mut seen = BTreeSet::new();
        for _ in 0..10_000 {
            let i = Timestamps::strategy()
                .new_tree(&mut runner)
                .unwrap()
                .current();
            seen.extend(
                Timestamps::classify(&i, &result(&i))
                    .split('/')
                    .map(str::to_owned),
            );
        }
        for domain in ["common", "out_of_domain"] {
            assert!(seen.contains(&format!("minting_domain={domain}")));
        }
        for field in [
            "lower_candidate",
            "lower_honest",
            "ledger_future_candidate",
            "ledger_future_honest",
            "ledger_candidate",
            "ledger_honest",
            "minting_contextual_candidate",
            "minting_contextual_honest",
            "minting_intrinsic_candidate",
            "minting_intrinsic_honest",
        ] {
            for outcome in [false, true] {
                assert!(
                    seen.contains(&format!("{field}={outcome}")),
                    "{field}={outcome}"
                );
            }
        }
    }

    #[test]
    fn pinned_expectations_and_linked_history() {
        let rows: Vec<Value> = include_str!("../regressions/timestamps.jsonl")
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert!(!rows.is_empty());
        for row in &rows {
            let i: Input = serde_json::from_value(row["input"].clone()).unwrap();
            assert_eq!(result(&i), row["expected"], "case {}", row["case"]);
        }
        let history: Vec<_> = rows
            .iter()
            .filter(|r| r["case"].as_str().unwrap().starts_with("zero_skew_step_"))
            .collect();
        assert_eq!(history.len(), 7);
        for (k, row) in history.iter().enumerate() {
            let i: Input = serde_json::from_value(row["input"].clone()).unwrap();
            assert_eq!(row["case"], format!("zero_skew_step_{k}"));
            assert_eq!(
                i,
                Input::new(
                    vec![1060; k],
                    if k == 0 { 0 } else { 1060 },
                    if k < 6 { 1060 } else { 1061 },
                    1000,
                    1000
                )
            );
            let r = result(&i);
            if k < 6 {
                assert_eq!(r["rolled"], history[k + 1]["input"]["window"]);
                assert_eq!(r["ledger_candidate"], true);
            } else {
                assert_eq!(r["mtp"], "1060");
                assert_eq!(r["honest_stamp"], "1061");
                assert_eq!(r["ledger_candidate"], false);
                assert_eq!(r["ledger_honest"], false);
            }
        }
    }
}
