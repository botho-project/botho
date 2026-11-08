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
    /// Generated shrink-attempt allowance, reduced by every failing replay.
    pub max_mismatches: usize,
}

pub struct Report {
    pub name: &'static str,
    pub rust: &'static str,
    pub replayed: u64,
    /// Generated inputs, including an unexamined final batch suffix.
    pub cases: u64,
    /// Generated inputs compared with Rust (excludes replays/shrink probes).
    pub compared: u64,
    /// Generated shrink invocations, including duplicate witnesses.
    pub shrink_attempts: u64,
    pub mismatches: Vec<Value>,
    pub elapsed: Duration,
    /// Outcome classes of compared generated cases (`Leg::classify`).
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
    assert!(opts.batch > 0, "batch must be positive");
    assert!(
        opts.max_mismatches > 0,
        "failure allowance must be positive"
    );
    let start = Instant::now();
    let mut report = Report {
        name: L::NAME,
        rust: L::RUST,
        replayed: 0,
        cases: 0,
        compared: 0,
        shrink_attempts: 0,
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
    // Replay is complete above; even duplicate replay failures debit the budget.
    let mut attempts_left = opts.max_mismatches.saturating_sub(report.mismatches.len());
    let mut remaining = opts.cases;
    while remaining > 0 && attempts_left > 0 {
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
            if attempts_left == 0 {
                break;
            }
            report.compared += 1;
            let rust = rust_caught::<L>(fx, input);
            let class = L::classify(input, &rust);
            if !class.is_empty() {
                *report.classes.entry(class).or_default() += 1;
            }
            if lean == rust {
                continue;
            }
            // Consume work before shrinking, independently of later deduplication.
            attempts_left -= 1;
            report.shrink_attempts += 1;
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use proptest::strategy::{BoxedStrategy, NewTree};
    use std::os::unix::fs::PermissionsExt;

    #[derive(Debug)]
    struct OneStep;
    #[derive(Debug)]
    struct StepTree(u64);
    impl Strategy for OneStep {
        type Tree = StepTree;
        type Value = u64;
        fn new_tree(&self, _: &mut TestRunner) -> NewTree<Self> {
            Ok(StepTree(1))
        }
    }
    impl ValueTree for StepTree {
        type Value = u64;
        fn current(&self) -> u64 {
            self.0
        }
        fn simplify(&mut self) -> bool {
            if self.0 == 1 {
                self.0 = 0;
                true
            } else {
                false
            }
        }
        fn complicate(&mut self) -> bool {
            false
        }
    }
    struct Collapse;
    impl Leg for Collapse {
        const NAME: &'static str = "budget_test";
        const RUST: &'static str = "synthetic::identity";
        type Input = u64;
        fn strategy() -> BoxedStrategy<u64> {
            OneStep.boxed()
        }
        fn args(_: &Fixtures, input: &u64) -> Value {
            json!(input)
        }
        fn rust(_: &Fixtures, input: &u64) -> Value {
            json!(input)
        }
        fn classify(_: &u64, _: &Value) -> String {
            "compared".into()
        }
    }
    fn model(dir: &std::path::Path) -> Model {
        model_with_response(dir, false)
    }
    fn model_with_response(dir: &std::path::Path, agree: bool) -> Model {
        let path = dir.join("model.py");
        fs::write(
            &path,
            format!("#!/usr/bin/env python3\nimport json, sys\nfor line in sys.stdin:\n print(json.dumps({{'ok': {}}}), flush=True)\n", if agree { "json.loads(line)['args']" } else { "99" }),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        Model::spawn(&path).unwrap()
    }
    fn options(dir: &std::path::Path) -> Options {
        Options {
            cases: 64,
            seed: 1707,
            batch: 16,
            regressions: dir.join("fixtures"),
            write_regressions: true,
            max_mismatches: 5,
        }
    }
    #[test]
    fn repeated_collapses_consume_the_attempt_budget() {
        let dir = tempfile::tempdir().unwrap();
        let opts = options(dir.path());
        let mut model = model(dir.path());
        let r = run::<Collapse>(&Fixtures::new(), &mut model, &opts);
        println!(
            "duplicate-control: generated={} model_calls={} classes={:?} unique={}",
            r.cases,
            model.calls,
            r.classes,
            r.mismatches.len()
        );
        // One generated batch plus exactly five one-step shrink probes.
        assert_eq!(model.calls, 21);
        assert_eq!(r.cases, 16);
        assert_eq!((r.compared, r.shrink_attempts), (5, 5));
        assert_eq!(r.classes["compared"], 5);
        assert_eq!(r.mismatches.len(), 1);
        assert_eq!(r.mismatches[0]["input"], json!(0));
        assert_eq!(
            fs::read_to_string(fixture_path(&opts, Collapse::NAME))
                .unwrap()
                .lines()
                .count(),
            1
        );
    }

    #[test]
    fn clean_batches_compare_every_case() {
        for (cases, batch) in [(19, 16), (10_000, 256), (3, 1), (3, 20)] {
            let dir = tempfile::tempdir().unwrap();
            let mut opts = options(dir.path());
            opts.cases = cases;
            opts.batch = batch;
            let mut model = model_with_response(dir.path(), true);
            let r = run::<Collapse>(&Fixtures::new(), &mut model, &opts);
            assert_eq!((r.cases, r.compared, r.shrink_attempts), (cases, cases, 0));
            assert_eq!(r.classes["compared"], cases);
            assert_eq!(model.calls, cases);
            assert!(r.mismatches.is_empty());
        }
    }

    #[test]
    fn small_budgets_and_batches_still_consume_duplicate_attempts() {
        for (cases, batch, limit, generated, attempts) in
            [(64, 1, 5, 5, 5), (3, 20, 5, 3, 3), (64, 16, 1, 16, 1)]
        {
            let dir = tempfile::tempdir().unwrap();
            let mut opts = options(dir.path());
            opts.cases = cases;
            opts.batch = batch;
            opts.max_mismatches = limit;
            let mut model = model(dir.path());
            let r = run::<Collapse>(&Fixtures::new(), &mut model, &opts);
            assert_eq!(
                (r.cases, r.compared, r.shrink_attempts),
                (generated, attempts, attempts)
            );
            assert_eq!(model.calls, generated + attempts);
            assert_eq!(r.classes["compared"], attempts);
            assert_eq!(r.mismatches.len(), 1);
        }
    }

    #[derive(Debug)]
    struct Sequence(std::sync::atomic::AtomicU64);
    impl Strategy for Sequence {
        type Tree = proptest::strategy::Just<u64>;
        type Value = u64;
        fn new_tree(&self, _: &mut TestRunner) -> NewTree<Self> {
            Ok(proptest::strategy::Just(
                self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            ))
        }
    }
    struct Distinct;
    impl Leg for Distinct {
        const NAME: &'static str = "distinct_test";
        const RUST: &'static str = Collapse::RUST;
        type Input = u64;
        fn strategy() -> BoxedStrategy<u64> {
            Sequence(std::sync::atomic::AtomicU64::new(0)).boxed()
        }
        fn args(fx: &Fixtures, input: &u64) -> Value {
            Collapse::args(fx, input)
        }
        fn rust(fx: &Fixtures, input: &u64) -> Value {
            Collapse::rust(fx, input)
        }
    }
    #[test]
    fn distinct_unsimplifiable_failures_are_retained() {
        let dir = tempfile::tempdir().unwrap();
        let opts = options(dir.path());
        let mut model = model(dir.path());
        let r = run::<Distinct>(&Fixtures::new(), &mut model, &opts);
        assert_eq!((r.cases, r.compared, r.shrink_attempts), (16, 5, 5));
        assert_eq!(model.calls, 16); // No simplification probes, still five attempts.
        assert_eq!(
            r.mismatches
                .iter()
                .map(|m| m["input"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            (0..5).collect::<Vec<_>>()
        );
        assert_eq!(
            fs::read_to_string(fixture_path(&opts, Distinct::NAME))
                .unwrap()
                .lines()
                .count(),
            5
        );
    }

    #[test]
    fn replay_is_complete_and_debits_duplicates_saturating() {
        for (replays, cases, attempts, generated) in [(7, 64, 0, 0), (1, 64, 4, 16), (2, 0, 0, 0)] {
            let dir = tempfile::tempdir().unwrap();
            let mut opts = options(dir.path());
            opts.cases = cases;
            fs::create_dir_all(&opts.regressions).unwrap();
            let path = fixture_path(&opts, Collapse::NAME);
            let original = "{\"input\":0}\n".repeat(replays);
            fs::write(&path, &original).unwrap();
            let mut model = model(dir.path());
            let r = run::<Collapse>(&Fixtures::new(), &mut model, &opts);
            assert_eq!(r.replayed, replays as u64);
            assert_eq!(
                (r.cases, r.compared, r.shrink_attempts),
                (generated, attempts, attempts)
            );
            assert_eq!(model.calls, replays as u64 + generated + attempts);
            assert_eq!(r.mismatches.len(), replays);
            assert!(r
                .mismatches
                .iter()
                .all(|m| m.get("replayed_fixture").is_some()));
            assert_eq!(fs::read_to_string(path).unwrap(), original);
        }
    }

    #[test]
    fn no_write_retains_failures_without_modifying_fixtures() {
        let dir = tempfile::tempdir().unwrap();
        let mut opts = options(dir.path());
        opts.write_regressions = false;
        fs::create_dir_all(&opts.regressions).unwrap();
        let path = fixture_path(&opts, Collapse::NAME);
        fs::write(&path, "# untouched\n").unwrap();
        let mut model = model(dir.path());
        let r = run::<Collapse>(&Fixtures::new(), &mut model, &opts);
        assert_eq!(r.shrink_attempts, 5);
        assert_eq!(r.mismatches.len(), 1);
        assert_eq!(fs::read_to_string(path).unwrap(), "# untouched\n");
    }

    #[test]
    fn zero_internal_options_are_rejected_before_model_work() {
        for (batch, limit) in [(0, 5), (16, 0)] {
            let dir = tempfile::tempdir().unwrap();
            let mut opts = options(dir.path());
            opts.batch = batch;
            opts.max_mismatches = limit;
            let mut model = model(dir.path());
            assert!(catch_unwind(AssertUnwindSafe(|| run::<Collapse>(
                &Fixtures::new(),
                &mut model,
                &opts
            )))
            .is_err());
            assert_eq!(model.calls, 0);
        }
    }

    #[test]
    fn pathological_tree_still_has_5000_check_bound() {
        struct Endless;
        impl ValueTree for Endless {
            type Value = u64;
            fn current(&self) -> u64 {
                0
            }
            fn simplify(&mut self) -> bool {
                true
            }
            fn complicate(&mut self) -> bool {
                false
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let mut model = model(dir.path());
        let best = shrink::<Collapse>(
            &Fixtures::new(),
            &mut model,
            &mut Endless,
            (0, json!(99), json!(0)),
        );
        assert_eq!(best.0, 0);
        assert_eq!(model.calls, 5_000);
    }
}
