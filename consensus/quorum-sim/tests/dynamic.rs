//! Mandatory acceptance tests for the **dynamic** message-level SCP simulator
//! (issue #514). These mirror the static `correctness.rs` style and form the
//! acceptance bar from the issue:
//!
//! 1. A known-safe config (n≥4, Botho BFT threshold) with faults below the
//!    splitting-set size NEVER forks (many seeds, every proposer model).
//! 2. An equivocating Byzantine node: below the splitting threshold → no fork;
//!    at/above it → a fork CAN occur (asserted detected). This cross-checks the
//!    static analyzer's splitting-set prediction against dynamic behavior.
//! 3. Unanimity below 4 nodes stalls when one node crashes (liveness).
//! 4. Reproducibility: same seed → identical outcome.

use bth_quorum_sim::{
    model::Fbas,
    sim::{run_many, run_tracked, FaultKind, NetworkModel, ProposerModel, SimConfig},
};

fn psync(max_delay: u32, drop_prob: f64) -> NetworkModel {
    NetworkModel::PartiallySynchronous {
        max_delay,
        drop_prob,
    }
}

/// (1) Known-safe configs never fork: for n ∈ {4,7,10}, with a number of
/// equivocators strictly below the static minimal splitting set, across every
/// proposer model and under partial synchrony, zero forks over many seeds.
#[test]
fn known_safe_below_splitting_never_forks() {
    for n in [4usize, 7, 10] {
        let fbas = Fbas::symmetric_botho(n);
        let split = fbas
            .health_report()
            .min_splitting_set_cardinality
            .expect("symmetric BFT federation has a splitting set");
        // Use the largest fault count strictly below the splitting set.
        let faulty: Vec<usize> = (0..split.saturating_sub(1)).collect();
        for proposer in ProposerModel::all() {
            let config = SimConfig {
                n,
                threshold: None,
                proposer,
                network: psync(3, 0.2),
                faulty: faulty.clone(),
                fault: FaultKind::Equivocate,
                max_rounds: 96,
            };
            let report = run_many(&config, 200);
            assert_eq!(
                report.forks,
                0,
                "n={n} {proposer:?}: {} equivocators < splitting set {split} must never fork",
                faulty.len()
            );
        }
    }
}

/// (2) Cross-check: below the splitting threshold → no fork; at/above it → a
/// fork CAN occur and is detected. Performed directly at n=4 (splitting set 2).
#[test]
fn equivocation_crosses_static_splitting_threshold() {
    let fbas = Fbas::symmetric_botho(4); // 3-of-4
    let split = fbas.health_report().min_splitting_set_cardinality.unwrap();
    assert_eq!(split, 2, "n=4 3-of-4 splitting set should be 2");

    // Below (1 equivocator): never forks, every proposer model.
    for proposer in ProposerModel::all() {
        let below = SimConfig {
            n: 4,
            threshold: None,
            proposer,
            network: psync(3, 0.2),
            faulty: vec![0],
            fault: FaultKind::Equivocate,
            max_rounds: 96,
        };
        assert_eq!(
            run_many(&below, 200).forks,
            0,
            "{proposer:?}: 1 equivocator (< {split}) must never fork"
        );
    }

    // At/above (2 equivocators incl. the leader): a fork CAN occur.
    let at = SimConfig {
        n: 4,
        threshold: None,
        proposer: ProposerModel::RoundRobinLeader,
        network: NetworkModel::Synchronous,
        faulty: vec![0, 2],
        fault: FaultKind::Equivocate,
        max_rounds: 96,
    };
    assert!(
        run_many(&at, 200).forks > 0,
        "2 equivocators (= splitting set {split}) must be able to fork dynamically"
    );
}

/// (3) Unanimity below 4 nodes stalls when one node crashes (liveness): 3-of-3
/// cannot reach quorum with only 2 live nodes.
#[test]
fn unanimity_below_four_stalls_on_crash() {
    let config = SimConfig {
        n: 3,
        threshold: None, // Botho BFT at n=3 is 3-of-3 (unanimity)
        proposer: ProposerModel::RoundRobinLeader,
        network: NetworkModel::Synchronous,
        faulty: vec![2],
        fault: FaultKind::Crash,
        max_rounds: 48,
    };
    let report = run_many(&config, 100);
    assert_eq!(report.stalls, 100, "every seed must stall (liveness)");
    assert_eq!(report.agreements, 0);
    assert_eq!(report.forks, 0, "a stall is not a fork");
}

/// (3b) Complement: a crash strictly below the blocking set keeps the network
/// LIVE — n=4 (3-of-4, blocking set 2) tolerates one crash and still decides.
#[test]
fn crash_below_blocking_set_stays_live() {
    let fbas = Fbas::symmetric_botho(4);
    let blocking = fbas.health_report().min_blocking_set_cardinality.unwrap();
    assert_eq!(blocking, 2);
    for proposer in ProposerModel::all() {
        let config = SimConfig {
            n: 4,
            threshold: None,
            proposer,
            network: NetworkModel::Synchronous,
            faulty: vec![3],
            fault: FaultKind::Crash,
            max_rounds: 48,
        };
        let report = run_many(&config, 100);
        assert_eq!(
            report.stalls, 0,
            "{proposer:?}: 1 crash < blocking set {blocking} → stays live"
        );
        assert_eq!(report.forks, 0, "{proposer:?}: crash never forks");
    }
}

/// (4) Reproducibility: the same `(config, seed)` yields an identical outcome,
/// and the aggregate report over a seed range is deterministic.
#[test]
fn reproducibility_same_seed_same_outcome() {
    let config = SimConfig {
        n: 7,
        threshold: None,
        proposer: ProposerModel::VrfLeader,
        network: psync(4, 0.3),
        faulty: vec![1, 5],
        fault: FaultKind::Equivocate,
        max_rounds: 96,
    };
    for seed in [0u64, 1, 7, 42, 1000] {
        assert_eq!(
            run_tracked(&config, seed),
            run_tracked(&config, seed),
            "seed {seed} must reproduce exactly"
        );
    }
    assert_eq!(
        run_many(&config, 128),
        run_many(&config, 128),
        "aggregate report must be deterministic over a seed range"
    );
}
