//! `quorum-sim` — CLI for Botho's quorum-health analyzer + dynamic SCP
//! simulator.
//!
//! Static subcommands (#511/#512):
//! - `compare` — threshold-rule comparison table (Botho BFT vs ceil(0.67n) vs
//!   unanimity) over a range of federation sizes.
//! - `analyze` — full static-health report for a single symmetric federation.
//! - `churn` — growth/churn timeline (admit / shun) starting from a symmetric
//!   federation, flagging any quorum-intersection break.
//!
//! Dynamic subcommand (#514):
//! - `simulate` — run the message-level SCP-ish round simulator over many seeds
//!   and report empirical fork (safety) / stall (liveness) counts,
//!   rounds-to-decide, and leadership fairness. Sweeps all four proposer models
//!   by default, or a single one via `--proposer`.
//!
//! Every subcommand supports `--json` for machine-readable output (CI /
//! monitoring); otherwise a human-readable table is printed.

use bth_quorum_sim::{
    model::Fbas,
    report::{
        compare_thresholds, render_churn_table, render_threshold_table, simulate_churn, ChurnAction,
    },
    sim::{render_sim_table, run_many, FaultKind, NetworkModel, ProposerModel, SimConfig},
};
use clap::{Parser, Subcommand, ValueEnum};

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
    /// Dynamic message-level SCP simulation: empirical fork / stall detection.
    Simulate {
        /// Federation size.
        #[arg(long, default_value_t = 4)]
        n: usize,
        /// Threshold; defaults to Botho's BFT rule for `n`.
        #[arg(long)]
        threshold: Option<usize>,
        /// Proposer model. Omit to sweep all four.
        #[arg(long, value_enum)]
        proposer: Option<ProposerArg>,
        /// Number of seeds to run per config (`0..seeds`).
        #[arg(long, default_value_t = 200)]
        seeds: u64,
        /// Faulty node indices (repeatable).
        #[arg(long)]
        faulty: Vec<usize>,
        /// Fault kind for the faulty nodes.
        #[arg(long, value_enum, default_value_t = FaultArg::Crash)]
        fault: FaultArg,
        /// Max message delay in rounds (>0 ⇒ partially-synchronous network).
        #[arg(long, default_value_t = 0)]
        max_delay: u32,
        /// Per-message drop probability in [0,1] (>0 ⇒ partially-synchronous).
        #[arg(long, default_value_t = 0.0)]
        drop_prob: f64,
        /// Liveness budget: max rounds before declaring a stall.
        #[arg(long, default_value_t = 64)]
        max_rounds: u32,
        /// Enable leader-timeout / view-change recovery (#519): if the current
        /// leader fails to drive a decision within `--view-budget` rounds,
        /// rotate round-robin to the next leader and retry the slot.
        /// Omit to run WITHOUT view-change (the v1 behavior, for
        /// comparison). No effect on the leaderless
        /// `competing-coinbase` model.
        #[arg(long)]
        view_change: bool,
        /// Per-view round budget for view-change (only used with
        /// `--view-change`). Rounds a leader gets before the view rotates.
        #[arg(long, default_value_t = 4)]
        view_budget: u32,
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
}

/// CLI adapter for [`ProposerModel`].
#[derive(Clone, Copy, Debug, ValueEnum)]
enum ProposerArg {
    CompetingCoinbase,
    HashPriorityLeader,
    RoundRobinLeader,
    VrfLeader,
}

impl From<ProposerArg> for ProposerModel {
    fn from(p: ProposerArg) -> Self {
        match p {
            ProposerArg::CompetingCoinbase => ProposerModel::CompetingCoinbase,
            ProposerArg::HashPriorityLeader => ProposerModel::HashPriorityLeader,
            ProposerArg::RoundRobinLeader => ProposerModel::RoundRobinLeader,
            ProposerArg::VrfLeader => ProposerModel::VrfLeader,
        }
    }
}

/// CLI adapter for [`FaultKind`].
#[derive(Clone, Copy, Debug, ValueEnum)]
enum FaultArg {
    Crash,
    Equivocate,
}

impl From<FaultArg> for FaultKind {
    fn from(f: FaultArg) -> Self {
        match f {
            FaultArg::Crash => FaultKind::Crash,
            FaultArg::Equivocate => FaultKind::Equivocate,
        }
    }
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
        Command::Simulate {
            n,
            threshold,
            proposer,
            seeds,
            faulty,
            fault,
            max_delay,
            drop_prob,
            max_rounds,
            view_change,
            view_budget,
            json,
        } => {
            let network = if max_delay == 0 && drop_prob == 0.0 {
                NetworkModel::Synchronous
            } else {
                NetworkModel::PartiallySynchronous {
                    max_delay,
                    drop_prob,
                }
            };
            // View-change is opt-in (#519); `--view-budget` only matters when on.
            let view_change = if view_change { Some(view_budget) } else { None };
            // Sweep all proposer models unless one is pinned.
            let models: Vec<ProposerModel> = match proposer {
                Some(p) => vec![p.into()],
                None => ProposerModel::all().to_vec(),
            };
            let reports: Vec<_> = models
                .into_iter()
                .map(|proposer| {
                    let config = SimConfig {
                        n,
                        threshold,
                        proposer,
                        network,
                        faulty: faulty.clone(),
                        fault: fault.into(),
                        max_rounds,
                        view_change,
                    };
                    run_many(&config, seeds)
                })
                .collect();
            if json {
                println!("{}", serde_json::to_string_pretty(&reports).unwrap());
            } else {
                print!("{}", render_sim_table(&reports));
            }
        }
    }
}
