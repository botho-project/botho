// Copyright (c) 2024 Botho Foundation
//
//! Parameterized N-node loopback consensus convergence harness (N = 2, 3, 4).
//!
//! This is a permanent regression test for the small-cluster convergence
//! behavior observed during the #427 investigation (Finding 3): clusters where
//! every node starts simultaneously and every node mints a competing coinbase
//! converge cleanly — no fork, no stall — at N = 2, 3, and 4.
//!
//! It exercises the *real* botho consensus path (`bth_consensus_scp` driving
//! block application through the production `Ledger` / `BlockBuilder`) over the
//! in-process loopback `TestNetwork` harness; it does **not** touch the
//! standalone `quorum-sim` modelling crate.
//!
//! Invariants asserted per N, for every settled height:
//!   * No fork    — every node agrees on the block hash at every height.
//!   * No stall   — the chain height strictly advances to the target within a
//!     bounded time budget.
//!   * One coinbase per height — exactly one minting tx is externalized per
//!     block (single-proposer invariant from #427 Finding 3), even though every
//!     node proposes a competing coinbase.
//!
//! Determinism note: the harness uses the existing crossbeam message-pump model
//! and trivial PoW (`TRIVIAL_DIFFICULTY`), so block production does not depend
//! on wall-clock mining. The only time bound is the no-stall deadline, which is
//! generously sized relative to the SCP timebase.

mod common;

use std::time::Duration;

use crate::common::{mine_block_all_minters, TestNetwork, TestNetworkConfig};

/// Cluster sizes to assert convergence for. N = 2, 3, 4 is the small-cluster
/// regime that #427 flagged as degenerate (n-of-n below 4); N = 5 is already
/// covered by `e2e_consensus_integration.rs`.
const CLUSTER_SIZES: [usize; 3] = [2, 3, 4];

/// Number of blocks to drive per cluster. Enough rounds to surface a
/// fork/stall if competing coinbases were mishandled, while keeping the test
/// fast (each block is a single trivial-PoW round).
const BLOCKS_PER_CLUSTER: u64 = 6;

/// Per-block no-stall deadline. The SCP timebase in tests is 100ms
/// (`SCP_TIMEBASE_MS`); 30s per block leaves ample headroom for CI while still
/// failing loudly on a genuine stall.
const PER_BLOCK_TIMEOUT: Duration = Duration::from_secs(30);

/// Recommended BFT peer-threshold for an `n`-node cluster under the harness's
/// convention, where a node's quorum set lists only its `n - 1` peers (self is
/// implicit in SCP).
///
/// The production recommended threshold (see
/// `QuorumConfig::effective_threshold` / `recommended_quorum` in
/// `consensus/service.rs`) is `n - floor((n-1)/3)` counting self as a member.
/// Subtracting the implicit self gives the peer-only threshold used here:
///
/// | n | total t = n - f | peer_k = t - 1 |
/// |---|-----------------|----------------|
/// | 2 |        2        |       1        |
/// | 3 |        3        |       2        |
/// | 4 |        4        |       3        |
///
/// This matches the default 5-node harness config (k = 3 over 4 peers).
fn recommended_peer_quorum_k(n: usize) -> usize {
    let f = n.saturating_sub(1) / 3;
    let total_threshold = n - f;
    total_threshold - 1
}

/// Drive an `n`-node all-minter cluster and assert no-fork / no-stall
/// convergence across `BLOCKS_PER_CLUSTER` blocks.
fn assert_cluster_converges(n: usize) {
    let config = TestNetworkConfig {
        num_nodes: n,
        quorum_k: recommended_peer_quorum_k(n),
        ..Default::default()
    };

    let mut network = TestNetwork::build(config);

    for round in 1..=BLOCKS_PER_CLUSTER {
        // No stall: every node must reach the new height within the deadline.
        let reached = mine_block_all_minters(&network, PER_BLOCK_TIMEOUT);
        assert!(
            reached,
            "N={n}: STALL at round {round} — cluster failed to reach height {round} within {:?}",
            PER_BLOCK_TIMEOUT
        );

        // No fork: every node agrees on every block hash up to this height, and
        // tip-level chain state (height, tip hash, totals) is identical.
        network.verify_no_fork_through(round);
        network.verify_consistency();

        // Exactly one coinbase externalized per height, despite every node
        // proposing a competing coinbase this round.
        network.verify_single_coinbase_per_height(round);
    }

    // Final height landed exactly where we drove it (strict advance, no extra
    // or missing blocks).
    let final_height = network.get_node(0).chain_state().height;
    assert_eq!(
        final_height, BLOCKS_PER_CLUSTER,
        "N={n}: expected chain to advance to height {BLOCKS_PER_CLUSTER}, got {final_height}"
    );

    network.stop();
}

#[test]
fn test_2_node_cluster_converges_no_fork_no_stall() {
    assert_cluster_converges(2);
}

#[test]
fn test_3_node_cluster_converges_no_fork_no_stall() {
    assert_cluster_converges(3);
}

#[test]
fn test_4_node_cluster_converges_no_fork_no_stall() {
    assert_cluster_converges(4);
}

/// Sanity check that the parameterization itself is exercised for every
/// targeted N in a single pass (guards against a future edit silently dropping
/// one of the cluster sizes from the dedicated per-N tests above).
#[test]
fn test_all_targeted_cluster_sizes_converge() {
    for n in CLUSTER_SIZES {
        assert_cluster_converges(n);
    }
}
