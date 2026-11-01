//! Differential random testing (DRT) of the Lean LotteryV2 model
//! (`formal/Botho/Lottery/`) against the Rust node and `cluster-tax`
//! (#1706, #1707). Dev-only: this crate is outside the root workspace and is
//! never linked into the node.
//!
//! Usage (from `formal-drt/`, after `lake build` in `formal-drt/lean`):
//!
//! ```text
//! cargo run --release -- [--cases N] [--seed S|random] [--legs a,b,...]
//!                        [--batch B] [--model PATH] [--regressions DIR]
//!                        [--no-write-regressions] [--json PATH] [--list]
//! ```
//!
//! Exit status 0 when every leg agrees on every case, 1 on any mismatch.
//! See `formal/README.md` ("Differential random testing") for the tiers and
//! for adding a leg.

mod fixtures;
mod gen;
mod legs;
mod model;
mod runner;
mod wire;

use legs::Leg;
use runner::{Options, Report};
use std::{path::PathBuf, process::ExitCode};

type LegFn = fn(&fixtures::Fixtures, &mut model::Model, &Options) -> Report;

/// Every leg: `(name, runner)`. To add a leg, implement `legs::Leg` and list
/// it here.
const LEGS: &[(&str, LegFn)] = &[
    (legs::SeedV2::NAME, runner::run::<legs::SeedV2>),
    (legs::IsEligible::NAME, runner::run::<legs::IsEligible>),
    (
        legs::CountEligible::NAME,
        runner::run::<legs::CountEligible>,
    ),
    (legs::SybilCap::NAME, runner::run::<legs::SybilCap>),
    (legs::SplitFees::NAME, runner::run::<legs::SplitFees>),
    (legs::RewardCap::NAME, runner::run::<legs::RewardCap>),
    (
        legs::PoolAccounting::NAME,
        runner::run::<legs::PoolAccounting>,
    ),
    (legs::Carryover::NAME, runner::run::<legs::Carryover>),
    (legs::DrawWithSeed::NAME, runner::run::<legs::DrawWithSeed>),
    (legs::DrawSeeded::NAME, runner::run::<legs::DrawSeeded>),
    (legs::Window::NAME, runner::run::<legs::Window>),
    (legs::Drawing::NAME, runner::run::<legs::Drawing>),
];

fn usage() -> ExitCode {
    eprintln!(
        "usage: botho-formal-drt [--cases N] [--seed S|random] [--legs a,b] [--batch B] \
         [--model PATH] [--regressions DIR] [--no-write-regressions] [--json PATH] [--list]"
    );
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut opts = Options {
        cases: 10_000,
        seed: 1707,
        batch: 256,
        regressions: crate_dir.join("regressions"),
        write_regressions: true,
        max_mismatches: 5,
    };
    let mut model_path = crate_dir.join("lean/.lake/build/bin/botho-model");
    let mut only: Option<Vec<String>> = None;
    let mut json_out: Option<PathBuf> = None;

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = || args.next();
        match a.as_str() {
            "--cases" => match val().and_then(|v| v.parse().ok()) {
                Some(v) => opts.cases = v,
                None => return usage(),
            },
            "--seed" => match val() {
                Some(v) if v == "random" => opts.seed = rand::random(),
                Some(v) => match v.parse() {
                    Ok(s) => opts.seed = s,
                    Err(_) => return usage(),
                },
                None => return usage(),
            },
            "--batch" => match val().and_then(|v| v.parse().ok()) {
                Some(v) if v > 0 => opts.batch = v,
                _ => return usage(),
            },
            "--legs" => match val() {
                Some(v) => only = Some(v.split(',').map(str::to_string).collect()),
                None => return usage(),
            },
            "--model" => match val() {
                Some(v) => model_path = PathBuf::from(v),
                None => return usage(),
            },
            "--regressions" => match val() {
                Some(v) => opts.regressions = PathBuf::from(v),
                None => return usage(),
            },
            "--json" => match val() {
                Some(v) => json_out = Some(PathBuf::from(v)),
                None => return usage(),
            },
            "--no-write-regressions" => opts.write_regressions = false,
            "--list" => {
                for (name, _) in LEGS {
                    println!("{name}");
                }
                return ExitCode::SUCCESS;
            }
            _ => return usage(),
        }
    }
    if let Some(only) = &only {
        for name in only {
            if !LEGS.iter().any(|(n, _)| n == name) {
                eprintln!("unknown leg {name} (see --list)");
                return ExitCode::from(2);
            }
        }
    }

    // Quiet the default panic hook: Rust panics are caught and reported as
    // mismatches (`rust_panic`), with the message.
    std::panic::set_hook(Box::new(|_| {}));

    let mut model = match model::Model::spawn(&model_path) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let fx = fixtures::Fixtures::new();
    println!(
        "botho-formal-drt: seed {} | {} cases per leg | model {}",
        opts.seed,
        opts.cases,
        model_path.display()
    );
    let mut reports = Vec::new();
    for (name, run) in LEGS {
        if only.as_ref().is_some_and(|o| !o.iter().any(|x| x == name)) {
            continue;
        }
        let r = run(&fx, &mut model, &opts);
        println!(
            "{:<30} {:>9} cases {:>4} replayed {:>3} mismatches {:>8.1}s  ({})",
            r.name,
            r.cases,
            r.replayed,
            r.mismatches.len(),
            r.elapsed.as_secs_f64(),
            r.rust
        );
        for (class, count) in &r.classes {
            println!("    {count:>9}  {class}");
        }
        for m in &r.mismatches {
            println!("  MISMATCH {}", m);
        }
        reports.push(r);
    }
    let failed = reports.iter().any(|r| !r.mismatches.is_empty());
    if let Some(path) = json_out {
        let v = serde_json::json!({
            "seed": opts.seed.to_string(),
            "cases_per_leg": opts.cases,
            "legs": reports.iter().map(|r| serde_json::json!({
                "leg": r.name,
                "rust": r.rust,
                "cases": r.cases,
                "replayed": r.replayed,
                "mismatches": r.mismatches,
                "classes": r.classes,
                "seconds": r.elapsed.as_secs_f64(),
            })).collect::<Vec<_>>(),
        });
        if let Err(e) = std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap()) {
            eprintln!("cannot write {}: {e}", path.display());
        }
    }
    println!(
        "total: {} Lean calls; {}",
        model.calls,
        if failed {
            "MISMATCHES FOUND"
        } else {
            "all legs agree"
        }
    );
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
