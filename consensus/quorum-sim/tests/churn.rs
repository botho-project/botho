//! Acceptance tests for the **coinbase-churn** model on the competing-coinbase
//! proposer (issue #535, the simulation arm of #532). These reproduce and
//! measure the #419 slot-stall and validate the production pinning fix.
//!
//! Background (grounded in `botho/src/consensus/service.rs`): under the
//! competing-coinbase proposer each validator's local RandomX miner produces a
//! fresh, strictly higher-priority coinbase several times a second. Production
//! **pins** the first-proposed coinbase per slot (`propose_pending_values`, the
//! #419 fix); without pinning, every node re-nominates each newly-mined higher
//! value, the candidate set never stabilizes, and the slot jams (a stall).
//!
//! The acceptance bar (from the issue):
//!  1. **unpinned + churn → measurable stalls**, increasing with churn rate and
//!     message delay (reproduces #419).
//!  2. **pinned + churn → ~0 stalls** across the stress range (validates the
//!     production fix).
//!  3. **Safety**: 0 forks with no faulty nodes in BOTH modes (churn is
//!     value-selection, not a safety variable — must not regress the #518
//!     two-phase-commit invariant).
//!  4. **Reproducibility**: same seed → same outcome.

use bth_quorum_sim::sim::{
    run_many, run_tracked, FaultKind, NetworkModel, ProposerModel, SimConfig,
};

const SEEDS: u64 = 300;

fn psync(max_delay: u32, drop_prob: f64) -> NetworkModel {
    NetworkModel::PartiallySynchronous {
        max_delay,
        drop_prob,
    }
}

/// Competing-coinbase churn config at federation size `n`.
fn churn_config(
    n: usize,
    churn_rate: f64,
    pin_coinbase: bool,
    network: NetworkModel,
    faulty: Vec<usize>,
) -> SimConfig {
    SimConfig {
        n,
        threshold: None,
        proposer: ProposerModel::CompetingCoinbase,
        network,
        faulty,
        fault: FaultKind::Crash,
        max_rounds: 96,
        view_change: None,
        churn_rate,
        pin_coinbase,
    }
}

/// (1) **Reproduce #419**: UNPINNED competing-coinbase under coinbase-churn and
/// partial synchrony produces *measurable* stalls, and the stall rate increases
/// monotonically with the churn rate. This is the
/// candidate-set-never-stabilizes jam the production pinning fix targets.
#[test]
fn unpinned_churn_stalls_and_grows_with_churn_rate() {
    let net = psync(3, 0.0);
    // A ladder of churn rates; stall rate must be strictly increasing.
    let rates = [0.1, 0.3, 0.6, 0.9];
    let mut prev = -1.0f64;
    let mut any_stall = false;
    for &rate in &rates {
        let cfg = churn_config(4, rate, /* pin */ false, net, vec![]);
        let report = run_many(&cfg, SEEDS);
        let sr = report.stall_rate();
        assert_eq!(
            report.forks, 0,
            "churn is value-selection only: unpinned churn must never fork (rate {rate})"
        );
        assert!(
            sr >= prev,
            "stall rate must be non-decreasing in churn rate: rate {rate} gave {sr:.3} \
             after previous {prev:.3}"
        );
        if sr > 0.0 {
            any_stall = true;
        }
        prev = sr;
    }
    assert!(
        any_stall,
        "unpinned competing-coinbase + churn under partial synchrony must produce \
         measurable stalls (reproducing #419)"
    );
    // The high-churn end must stall substantially (not just a stray seed).
    let high = run_many(&churn_config(4, 0.9, false, net, vec![]), SEEDS);
    assert!(
        high.stall_rate() > 0.5,
        "high churn (0.9) unpinned must stall heavily, got {:.3}",
        high.stall_rate()
    );
}

/// (1b) **Stalls also grow with message delay** at a fixed churn rate: more
/// asynchrony = the moving candidate set outruns convergence more often.
#[test]
fn unpinned_churn_stalls_grow_with_delay() {
    let rate = 0.5;
    let delays = [1u32, 3, 6];
    let mut prev = -1.0f64;
    for &d in &delays {
        let cfg = churn_config(4, rate, false, psync(d, 0.0), vec![]);
        let report = run_many(&cfg, SEEDS);
        assert_eq!(report.forks, 0, "no faults: must never fork (delay {d})");
        let sr = report.stall_rate();
        assert!(
            sr >= prev,
            "stall rate must be non-decreasing in delay: delay {d} gave {sr:.3} \
             after previous {prev:.3}"
        );
        prev = sr;
    }
    assert!(
        prev > 0.0,
        "with delay 6 + churn 0.5, some seeds must stall"
    );
}

/// (2) **Validate the production fix**: PINNED competing-coinbase (the #419
/// `propose_pending_values` behavior) drives stalls to ~0 across the entire
/// churn × network stress matrix, while never forking. This is the headline
/// result for #532: if pinning shows ~0 stalls, the view-change escape-hatch is
/// not needed for competing-coinbase liveness.
#[test]
fn pinned_churn_does_not_stall() {
    let networks = [
        NetworkModel::Synchronous,
        psync(3, 0.0),
        psync(6, 0.1),
        psync(3, 0.2),
    ];
    for n in [4usize, 7] {
        for net in networks {
            for &rate in &[0.1, 0.3, 0.6, 0.9] {
                let cfg = churn_config(n, rate, /* pin */ true, net, vec![]);
                let report = run_many(&cfg, SEEDS);
                assert_eq!(
                    report.forks, 0,
                    "pinned churn must never fork (n={n} net={net:?} rate={rate})"
                );
                assert_eq!(
                    report.stalls, 0,
                    "pinned coinbase must NOT stall under churn (the #419 fix): \
                     n={n} net={net:?} rate={rate} stalled {}/{} \
                     (first stall would jam the slot)",
                    report.stalls, report.seeds
                );
            }
        }
    }
}

/// (2b) **Direct pinned-vs-unpinned contrast** under identical stress: at a
/// churn rate where unpinned jams heavily, pinning collapses the stall rate to
/// zero — the production fix in one assertion.
#[test]
fn pinning_collapses_the_stall_rate() {
    let net = psync(3, 0.0);
    let unpinned = run_many(&churn_config(4, 0.6, false, net, vec![]), SEEDS);
    let pinned = run_many(&churn_config(4, 0.6, true, net, vec![]), SEEDS);
    assert!(
        unpinned.stall_rate() > 0.3,
        "unpinned at churn 0.6 should jam substantially, got {:.3}",
        unpinned.stall_rate()
    );
    assert_eq!(
        pinned.stalls, 0,
        "pinned at churn 0.6 must not stall at all, got {}/{}",
        pinned.stalls, pinned.seeds
    );
}

/// (2c) Pinning keeps the slot live even with **one crash below the blocking
/// set** (n=4, 3-of-4, blocking set = 2) under churn, while unpinned jams.
#[test]
fn pinned_survives_one_crash_under_churn() {
    let net = psync(3, 0.0);
    let pinned = run_many(&churn_config(4, 0.3, true, net, vec![3]), SEEDS);
    let unpinned = run_many(&churn_config(4, 0.3, false, net, vec![3]), SEEDS);
    assert_eq!(pinned.forks, 0);
    assert_eq!(
        pinned.stalls, 0,
        "1 crash < blocking set 2, pinned + churn → stays live, got {}/{}",
        pinned.stalls, pinned.seeds
    );
    assert!(
        unpinned.stall_rate() > 0.0,
        "unpinned + crash + churn should still jam at least some seeds"
    );
}

/// (3) **Safety with no faults in BOTH modes** across network models: churn is
/// value-selection, not a safety variable, so it must never induce a fork —
/// guarding the #518 two-phase-commit invariant against regression.
#[test]
fn churn_never_forks_with_no_faults() {
    let networks = [
        NetworkModel::Synchronous,
        psync(3, 0.0),
        psync(0, 0.2),
        psync(4, 0.2),
    ];
    for pin in [true, false] {
        for n in [4usize, 7, 10] {
            for net in networks {
                let cfg = churn_config(n, 0.7, pin, net, vec![]);
                let report = run_many(&cfg, 200);
                assert_eq!(
                    report.forks, 0,
                    "churn must never fork with zero faults: \
                     pin={pin} n={n} net={net:?} (first fork seed {:?})",
                    report.first_fork_seed
                );
            }
        }
    }
}

/// (4) **Reproducibility**: a `(config, seed)` is bit-for-bit reproducible with
/// churn active, in both pinned and unpinned modes, and `run_many` over a seed
/// range is deterministic.
#[test]
fn churn_is_reproducible() {
    for pin in [true, false] {
        let cfg = churn_config(4, 0.5, pin, psync(3, 0.1), vec![]);
        for seed in [0u64, 1, 7, 42, 999] {
            let a = run_tracked(&cfg, seed);
            let b = run_tracked(&cfg, seed);
            assert_eq!(a, b, "pin={pin} seed {seed} must reproduce exactly");
        }
        let r0 = run_many(&cfg, 128);
        let r1 = run_many(&cfg, 128);
        assert_eq!(r0, r1, "pin={pin}: run_many must be deterministic");
    }
}

/// (5) **Churn-free is unchanged**: with `churn_rate = 0` the
/// competing-coinbase proposer behaves exactly as before — no stalls, no forks
/// under the healthy synchronous baseline — so the new model is strictly
/// additive.
#[test]
fn zero_churn_matches_baseline() {
    let cfg = churn_config(7, 0.0, true, NetworkModel::Synchronous, vec![]);
    let report = run_many(&cfg, SEEDS);
    assert_eq!(report.forks, 0);
    assert_eq!(report.stalls, 0, "churn-free healthy net must not stall");
    assert_eq!(report.agreements, SEEDS);
}
