//! Runs one leg: replay its regression fixtures, then generate cases in
//! batches, send each batch to the Lean model, run the Rust function on each
//! case and compare. A mismatch is shrunk (proptest's value-tree shrinking,
//! one Lean call per step) and appended to `regressions/<leg>.jsonl`.

use crate::{fixtures::Fixtures, legs::Leg, model::Model, wire};
use proptest::{
    strategy::{Strategy, ValueTree},
    test_runner::{Config, RngAlgorithm, TestRng, TestRunner},
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    panic::{catch_unwind, AssertUnwindSafe},
    path::PathBuf,
    time::{Duration, Instant},
};

pub struct Options {
    pub cases: u64,
    pub seed: u64,
    pub batch: usize,
    pub regressions: PathBuf,
    pub write_regressions: bool,
    /// Stop generating after this many mismatches in one leg.
    pub max_mismatches: usize,
}

pub struct Report {
    pub name: &'static str,
    pub rust: &'static str,
    pub replayed: u64,
    pub cases: u64,
    pub mismatches: Vec<Value>,
    pub elapsed: Duration,
    /// Outcome classes of the generated cases (`Leg::classify`).
    pub classes: BTreeMap<String, u64>,
}

/// The Rust result, or `{"rust_panic": message}` if the function panicked.
fn rust_caught<L: Leg>(fx: &Fixtures, input: &L::Input) -> Value {
    match catch_unwind(AssertUnwindSafe(|| L::rust(fx, input))) {
        Ok(v) => v,
        Err(e) => {
            let msg = e
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "non-string panic".into());
            json!({ "rust_panic": msg })
        }
    }
}

/// `(lean, rust)` for one input, or `None` if they agree.
fn check_one<L: Leg>(fx: &Fixtures, model: &mut Model, input: &L::Input) -> Option<(Value, Value)> {
    let line = wire::request(L::NAME, &L::args(fx, input));
    let lean = model.call_batch(&[line]).pop().expect("one response");
    let rust = rust_caught::<L>(fx, input);
    (lean != rust).then_some((lean, rust))
}

/// The proptest RNG of leg `name` under run seed `seed`: independent of which
/// other legs run, and of their order.
fn leg_rng(seed: u64, name: &str) -> TestRng {
    let mut h = Sha256::new();
    h.update(b"botho-formal-drt");
    h.update(seed.to_le_bytes());
    h.update(name.as_bytes());
    let digest: [u8; 32] = h.finalize().into();
    TestRng::from_seed(RngAlgorithm::ChaCha, &digest)
}

fn fixture_path(opts: &Options, name: &str) -> PathBuf {
    opts.regressions.join(format!("{name}.jsonl"))
}

#[derive(Deserialize)]
struct FixtureLine {
    input: Value,
}

pub fn run<L: Leg>(fx: &Fixtures, model: &mut Model, opts: &Options) -> Report {
    let start = Instant::now();
    let mut report = Report {
        name: L::NAME,
        rust: L::RUST,
        replayed: 0,
        cases: 0,
        mismatches: Vec::new(),
        elapsed: Duration::ZERO,
        classes: BTreeMap::new(),
    };

    // 1. Replay every stored regression fixture of this leg.
    let path = fixture_path(opts, L::NAME);
    if let Ok(text) = fs::read_to_string(&path) {
        for (lineno, line) in text.lines().enumerate() {
            if line.trim().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            let fixture: FixtureLine = serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("{}:{}: {e}", path.display(), lineno + 1));
            let input: L::Input = serde_json::from_value(fixture.input)
                .unwrap_or_else(|e| panic!("{}:{}: {e}", path.display(), lineno + 1));
            report.replayed += 1;
            if let Some((lean, rust)) = check_one::<L>(fx, model, &input) {
                report.mismatches.push(json!({
                    "replayed_fixture": format!("{}:{}", path.display(), lineno + 1),
                    "input": input, "lean": lean, "rust": rust,
                }));
            }
        }
    }

    // 2. Generated cases.
    let strategy = L::strategy();
    let mut runner = TestRunner::new_with_rng(
        Config {
            failure_persistence: None,
            ..Config::default()
        },
        leg_rng(opts.seed, L::NAME),
    );
    let mut remaining = opts.cases;
    while remaining > 0 && report.mismatches.len() < opts.max_mismatches {
        let k = remaining.min(opts.batch as u64) as usize;
        let mut trees: Vec<_> = (0..k)
            .map(|_| strategy.new_tree(&mut runner).expect("generate a case"))
            .collect();
        let inputs: Vec<L::Input> = trees.iter().map(|t| t.current()).collect();
        let lines: Vec<String> = inputs
            .iter()
            .map(|i| wire::request(L::NAME, &L::args(fx, i)))
            .collect();
        let lean = model.call_batch(&lines);
        for (idx, (input, lean)) in inputs.iter().zip(lean).enumerate() {
            let rust = rust_caught::<L>(fx, input);
            let class = L::classify(input, &rust);
            if !class.is_empty() {
                *report.classes.entry(class).or_default() += 1;
            }
            if lean == rust {
                continue;
            }
            if report.mismatches.len() >= opts.max_mismatches {
                break;
            }
            let (input, lean, rust) =
                shrink::<L>(fx, model, &mut trees[idx], (input.clone(), lean, rust));
            // Several failing cases often shrink to the same minimal one.
            let input_json = serde_json::to_value(&input).expect("serialize input");
            if report.mismatches.iter().any(|m| m["input"] == input_json) {
                continue;
            }
            if opts.write_regressions {
                append_fixture(&fixture_path(opts, L::NAME), &input, &lean, &rust);
            }
            report
                .mismatches
                .push(json!({ "input": input, "lean": lean, "rust": rust }));
        }
        report.cases += k as u64;
        remaining -= k as u64;
    }
    report.elapsed = start.elapsed();
    report
}

/// Shrink a failing case with proptest's value tree: simplify while the case
/// still fails, step back (`complicate`) when it passes. Bounded by a step
/// budget so a pathological tree cannot stall the run.
fn shrink<L: Leg>(
    fx: &Fixtures,
    model: &mut Model,
    tree: &mut impl ValueTree<Value = L::Input>,
    mut best: (L::Input, Value, Value),
) -> (L::Input, Value, Value) {
    let mut budget = 5_000u32;
    'outer: while budget > 0 && tree.simplify() {
        loop {
            budget = budget.saturating_sub(1);
            let candidate = tree.current();
            if let Some((lean, rust)) = check_one::<L>(fx, model, &candidate) {
                best = (candidate, lean, rust);
                continue 'outer;
            }
            if budget == 0 || !tree.complicate() {
                break 'outer;
            }
        }
    }
    best
}

fn append_fixture<I: serde::Serialize>(path: &PathBuf, input: &I, lean: &Value, rust: &Value) {
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let line = json!({ "input": input, "lean": lean, "rust": rust }).to_string();
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap_or_else(|e| panic!("open {}: {e}", path.display()));
    writeln!(f, "{line}").expect("write regression fixture");
}
