//! `quorum-sim` — CLI for Botho's static quorum-health analyzer (v1).
//!
//! Subcommands:
//! - `compare` — threshold-rule comparison table (Botho BFT vs ceil(0.67n) vs
//!   unanimity) over a range of federation sizes.
//! - `analyze` — full static-health report for a single symmetric federation.
//! - `churn` — growth/churn timeline (admit / shun) starting from a symmetric
//!   federation, flagging any quorum-intersection break.
//!
//! Every subcommand supports `--json` for machine-readable output (CI /
//! monitoring); otherwise a human-readable table is printed.

use bth_quorum_sim::{
    model::Fbas,
    report::{
        compare_thresholds, render_churn_table, render_threshold_table, simulate_churn, ChurnAction,
    },
};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "quorum-sim",
    about = "Static quorum-health analyzer for Botho's curated FBAS federation (v1)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Compare threshold rules over n = min..=max.
    Compare {
        /// Smallest federation size to evaluate.
        #[arg(long, default_value_t = 2)]
        min: usize,
        /// Largest federation size to evaluate.
        #[arg(long, default_value_t = 12)]
        max: usize,
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Full static-health report for a single symmetric federation.
    Analyze {
        /// Federation size.
        #[arg(long)]
        n: usize,
        /// Threshold; defaults to Botho's BFT rule for `n`.
        #[arg(long)]
        threshold: Option<usize>,
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Growth/churn timeline starting from a symmetric Botho-BFT federation.
    Churn {
        /// Initial federation size.
        #[arg(long, default_value_t = 3)]
        initial: usize,
        /// Number of validators to admit (applied first).
        #[arg(long, default_value_t = 0)]
        admit: usize,
        /// Node indices to shun, applied after admissions (repeatable).
        #[arg(long)]
        shun: Vec<usize>,
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Compare { min, max, json } => {
            let rows = compare_thresholds(min..=max);
            if json {
                println!("{}", serde_json::to_string_pretty(&rows).unwrap());
            } else {
                print!("{}", render_threshold_table(&rows));
            }
        }
        Command::Analyze { n, threshold, json } => {
            let fbas = match threshold {
                Some(t) => Fbas::symmetric(n, t),
                None => Fbas::symmetric_botho(n),
            };
            let report = fbas.health_report();
            if json {
                println!("{}", serde_json::to_string_pretty(&report).unwrap());
            } else {
                println!("symmetric federation n={n}");
                println!("  quorum_intersection:   {}", report.quorum_intersection);
                println!(
                    "  min quorum card:       {:?}",
                    report.min_quorum_cardinality
                );
                println!(
                    "  min blocking card:     {:?}  (liveness buffer)",
                    report.min_blocking_set_cardinality
                );
                println!(
                    "  min splitting card:    {:?}  (safety buffer)",
                    report.min_splitting_set_cardinality
                );
                println!("  #minimal quorums:      {}", report.num_minimal_quorums);
                println!(
                    "  #minimal blocking:     {}",
                    report.num_minimal_blocking_sets
                );
                println!(
                    "  #minimal splitting:    {}",
                    report.num_minimal_splitting_sets
                );
            }
        }
        Command::Churn {
            initial,
            admit,
            shun,
            json,
        } => {
            let mut actions: Vec<ChurnAction> = Vec::new();
            for _ in 0..admit {
                actions.push(ChurnAction::AdmitSymmetric);
            }
            for idx in shun {
                actions.push(ChurnAction::ShunSymmetric(idx));
            }
            let steps = simulate_churn(initial, &actions);
            if json {
                println!("{}", serde_json::to_string_pretty(&steps).unwrap());
            } else {
                print!("{}", render_churn_table(&steps));
            }
        }
    }
}
