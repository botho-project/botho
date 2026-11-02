//! DRT legs for the difficulty controller and the dynamic slot duration
//! (#1841): the Lean model in `formal/Botho/Difficulty/` and
//! `formal/Botho/Consensus/SlotDuration.lean` against the Rust code, reached
//! through `botho::formal_drt_difficulty` (`test-fixtures` only).

use crate::{fixtures::Fixtures, gen, legs::Leg, wire::n};
use botho::{
    block::difficulty::MAX_DIFFICULTY, formal_drt_difficulty as rs,
    node::minter::INITIAL_DIFFICULTY,
};
use proptest::{collection::vec, prelude::*};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Difficulty values biased to the controller's edges: the clamp bounds,
/// genesis, the `2^63` point where doubling leaves `u64`, and `u64::MAX`.
fn difficulty() -> BoxedStrategy<u64> {
    prop_oneof![
        2 => prop::sample::select(vec![
            0u64,
            1,
            2,
            3,
            INITIAL_DIFFICULTY,
            MAX_DIFFICULTY / 2,
            MAX_DIFFICULTY / 2 + 1,
            (1 << 63) - 1,
            1 << 63,
            MAX_DIFFICULTY - 1,
            MAX_DIFFICULTY,
            MAX_DIFFICULTY + 1,
            u64::MAX,
        ]),
        2 => 1..=MAX_DIFFICULTY,
        1 => gen::u64_edge(),
    ]
    .boxed()
}

/// Observed gaps: every value around the 5 s target and the 60 s cap, the
/// pacing tiers, the 2 h future bound, and arbitrary `u64`s.
fn gap() -> BoxedStrategy<u64> {
    prop_oneof![
        3 => 0u64..=12,
        2 => prop::sample::select(vec![3u64, 5, 10, 20, 40, 59, 60, 61, 7200]),
        1 => 0u64..=200,
        1 => gen::u64_edge(),
    ]
    .boxed()
}

// ---- compute_time_adjusted_difficulty
// ----------------------------------------

pub struct DifficultyStep;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StepIn {
    pub current: u64,
    pub observed: u64,
    pub target: u64,
}

impl Leg for DifficultyStep {
    const NAME: &'static str = "difficulty_step";
    const RUST: &'static str =
        "botho::block::difficulty::EmissionController::compute_time_adjusted_difficulty";
    type Input = StepIn;
    fn strategy() -> BoxedStrategy<StepIn> {
        let target = prop_oneof![
            4 => Just(5u64),
            1 => prop::sample::select(vec![0u64, 1, 3, 10, u64::MAX / 12, u64::MAX / 12 + 1]),
            1 => gen::u64_edge(),
        ];
        (difficulty(), gap(), target)
            .prop_map(|(current, observed, target)| StepIn {
                current,
                observed,
                target,
            })
            .boxed()
    }
    fn args(_: &Fixtures, i: &StepIn) -> Value {
        json!({ "current": n(i.current), "observed": n(i.observed), "target": n(i.target) })
    }
    fn rust(_: &Fixtures, i: &StepIn) -> Value {
        n(rs::step_rust(i.current, i.observed, i.target))
    }
    fn classify(i: &StepIn, r: &Value) -> String {
        let v: u64 = r.as_str().and_then(|x| x.parse().ok()).unwrap_or(0);
        if v == 1 {
            "at floor".into()
        } else if v == MAX_DIFFICULTY {
            if i.current as u128 * 2 > u64::MAX as u128 && i.target == 5 && i.observed >= 10 {
                "at ceiling via unwrap_or".into()
            } else {
                "at ceiling".into()
            }
        } else if v > i.current {
            "eased".into()
        } else if v < i.current {
            "hardened".into()
        } else {
            "unchanged".into()
        }
    }
}

// ---- EmissionController::record_block trajectory ----------------------------

pub struct DifficultyRun;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunIn {
    pub start: u64,
    pub gaps: Vec<u64>,
}

impl Leg for DifficultyRun {
    const NAME: &'static str = "difficulty_run";
    const RUST: &'static str = "botho::block::difficulty::EmissionController::record_block";
    type Input = RunIn;
    fn strategy() -> BoxedStrategy<RunIn> {
        let start = prop_oneof![
            2 => Just(INITIAL_DIFFICULTY),
            2 => Just(MAX_DIFFICULTY),
            1 => 1u64..=1000,
            2 => 1..=MAX_DIFFICULTY,
        ];
        (start, vec(gap(), 0..=80))
            .prop_map(|(start, gaps)| RunIn { start, gaps })
            .boxed()
    }
    fn args(_: &Fixtures, i: &RunIn) -> Value {
        json!({ "start": n(i.start), "gaps": i.gaps.iter().map(n).collect::<Vec<_>>() })
    }
    fn rust(_: &Fixtures, i: &RunIn) -> Value {
        n(rs::run_rust(i.start, &i.gaps))
    }
    fn classify(_: &RunIn, r: &Value) -> String {
        let v: u64 = r.as_str().and_then(|x| x.parse().ok()).unwrap_or(0);
        match v {
            1 => "ends at floor".into(),
            v if v == MAX_DIFFICULTY => "ends at ceiling".into(),
            _ => "ends inside".into(),
        }
    }
}

// ---- ConsensusService::current_slot_duration --------------------------------

pub struct SlotDuration;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SlotIn {
    pub dynamic: bool,
    pub cfg: u64,
    pub feed: Vec<(u64, u64)>,
}

/// A recorded-block feed whose timestamps move by tier gaps, equal stamps,
/// small steps back (the service does not check order) and 2 h jumps.
fn feed() -> BoxedStrategy<Vec<(u64, u64)>> {
    let step = prop_oneof![
        2 => Just(0i64),
        3 => prop::sample::select(vec![3i64, 5, 10, 20, 40]),
        2 => 0i64..=60,
        1 => -50i64..=0,
        1 => Just(7200i64),
    ];
    let tx = prop_oneof![
        3 => 0u64..=30,
        1 => 0u64..=1000,
        1 => Just(0u64),
    ];
    (1_700_000_000u64..=1_800_000_000, vec((step, tx), 0..=14))
        .prop_map(|(t0, steps)| {
            let mut ts = t0;
            steps
                .into_iter()
                .map(|(d, tx)| {
                    ts = ts.saturating_add_signed(d);
                    (ts, tx)
                })
                .collect()
        })
        .boxed()
}

impl Leg for SlotDuration {
    const NAME: &'static str = "slot_duration";
    const RUST: &'static str = "botho::consensus::ConsensusService::current_slot_duration";
    type Input = SlotIn;
    fn strategy() -> BoxedStrategy<SlotIn> {
        (
            prop_oneof![9 => Just(true), 1 => Just(false)],
            prop::sample::select(vec![1u64, 3, 7, 20, 40]),
            feed(),
        )
            .prop_map(|(dynamic, cfg, feed)| SlotIn { dynamic, cfg, feed })
            .boxed()
    }
    fn args(_: &Fixtures, i: &SlotIn) -> Value {
        let feed: Vec<Value> = i
            .feed
            .iter()
            .map(|(ts, tx)| json!({ "ts": n(ts), "tx": n(tx) }))
            .collect();
        json!({ "dynamic": i.dynamic, "cfg": n(i.cfg), "feed": feed })
    }
    fn rust(_: &Fixtures, i: &SlotIn) -> Value {
        n(rs::slot_rust(i.dynamic, i.cfg, &i.feed))
    }
    fn classify(_: &SlotIn, r: &Value) -> String {
        format!("{} s", r.as_str().unwrap_or("?"))
    }
}
