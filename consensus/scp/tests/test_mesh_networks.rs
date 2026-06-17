// Copyright (c) 2018-2022 The Botho Foundation

mod mock_network;

use bth_common::logger::{test_with_logger, Logger};
use bth_consensus_scp::{msg::Msg, slot::CombineFn, test_utils, Node, QuorumSet, ScpNode};
use serial_test::serial;
use std::{
    collections::BTreeSet,
    sync::Arc,
    time::{Duration, Instant},
};

/// Performs a consensus test for a mesh network of (n) nodes.
fn mesh_test_helper(
    n: usize, // the number of nodes in the network
    k: usize, // the number of nodes that must agree within the network
    logger: Logger,
) {
    assert!(k <= n);

    if n > 3 && mock_network::skip_slow_tests() {
        return;
    }

    let mut test_options = mock_network::TestOptions::new();
    test_options.values_to_submit = 10000;
    let network_config = mock_network::mesh_topology::dense_mesh(n, k);
    mock_network::build_and_test(&network_config, &test_options, logger);
}

#[test_with_logger]
#[serial]
fn mesh_1(logger: Logger) {
    mesh_test_helper(1, 0, logger);
}

#[test_with_logger]
#[serial]
fn mesh_2k1(logger: Logger) {
    mesh_test_helper(2, 1, logger);
}

#[test_with_logger]
#[serial]
fn mesh_3k1(logger: Logger) {
    mesh_test_helper(3, 1, logger);
}

#[test_with_logger]
#[serial]
fn mesh_3k2(logger: Logger) {
    mesh_test_helper(3, 2, logger);
}

#[test_with_logger]
#[serial]
fn mesh_4k3(logger: Logger) {
    mesh_test_helper(4, 3, logger);
}

#[test_with_logger]
#[serial]
fn mesh_5k3(logger: Logger) {
    mesh_test_helper(5, 3, logger);
}

#[test_with_logger]
#[serial]
fn mesh_5k4(logger: Logger) {
    mesh_test_helper(5, 4, logger);
}

/// A combine function that mimics Botho's `ConsensusValue` combiner for
/// minting: it sorts the candidate set deterministically and keeps exactly ONE
/// winner. Because it is deterministic and order-independent, all honest nodes
/// that combine the *same* confirmed-nominated set `Z` produce the *same*
/// output.
fn pick_one_winner_combine_fn() -> CombineFn<String, test_utils::TransactionValidationError> {
    Arc::new(
        |values: &[String]| -> Result<Vec<String>, test_utils::TransactionValidationError> {
            let mut v: Vec<String> = values.to_vec();
            v.sort();
            v.dedup();
            v.truncate(1);
            Ok(v)
        },
    )
}

/// Drive two SCP nodes that nominate DISTINCT values until both externalize, or
/// the deadline elapses. Returns the externalized block (slot 0) for each node,
/// or `None` for a node that never externalized.
///
/// This is a deterministic, single-threaded message pump: it repeatedly lets
/// each node propose its own value, hand-delivers any emitted message to the
/// peer, and processes timeouts, advancing a virtual clock so SCP round/ballot
/// timers fire.
fn run_two_node_distinct_values(
    value_a: &str,
    value_b: &str,
    logger: Logger,
) -> (Option<Vec<String>>, Option<Vec<String>>) {
    let node_a_id = test_utils::test_node_id(1);
    let node_b_id = test_utils::test_node_id(2);

    // A 2-of-2 quorum: each node trusts both members.
    let quorum_set = QuorumSet::new_with_node_ids(2, vec![node_a_id.clone(), node_b_id.clone()]);

    let validity_fn = Arc::new(test_utils::trivial_validity_fn::<String>);
    let combine_fn = pick_one_winner_combine_fn();

    let mut node_a = Node::new(
        node_a_id.clone(),
        quorum_set.clone(),
        validity_fn.clone(),
        combine_fn.clone(),
        0,
        logger.clone(),
    );
    let mut node_b = Node::new(
        node_b_id.clone(),
        quorum_set,
        validity_fn,
        combine_fn,
        0,
        logger.clone(),
    );

    // Use a short timebase so round/ballot timers fire quickly in the virtual
    // clock. SCP advances even without timeouts when nomination converges; the
    // timers are a backstop.
    node_a.scp_timebase = Duration::from_millis(50);
    node_b.scp_timebase = Duration::from_millis(50);

    let mut a_out: Vec<Msg<String>> = Vec::new();
    let mut b_out: Vec<Msg<String>> = Vec::new();

    let value_a_set: BTreeSet<String> = BTreeSet::from([value_a.to_string()]);
    let value_b_set: BTreeSet<String> = BTreeSet::from([value_b.to_string()]);

    let start = Instant::now();
    let deadline = start + Duration::from_secs(20);

    loop {
        // Each node re-proposes its own (distinct) value every tick, just like a
        // minter re-proposing its coinbase each slot tick.
        if let Some(msg) = node_a
            .propose_values(value_a_set.clone())
            .expect("node_a propose_values failed")
        {
            a_out.push(msg);
        }
        if let Some(msg) = node_b
            .propose_values(value_b_set.clone())
            .expect("node_b propose_values failed")
        {
            b_out.push(msg);
        }

        // Deliver A's messages to B and vice-versa. Take the queues so we can
        // push replies into the peer's queue without an aliasing borrow.
        for msg in std::mem::take(&mut a_out) {
            if let Some(reply) = node_b
                .handle_message(&msg)
                .expect("node_b handle_message failed")
            {
                b_out.push(reply);
            }
        }
        for msg in std::mem::take(&mut b_out) {
            if let Some(reply) = node_a
                .handle_message(&msg)
                .expect("node_a handle_message failed")
            {
                a_out.push(reply);
            }
        }

        // Process timeouts on both nodes (this fires round/ballot timers).
        for msg in node_a.process_timeouts() {
            a_out.push(msg);
        }
        for msg in node_b.process_timeouts() {
            b_out.push(msg);
        }

        let a_ext = node_a.get_externalized_values(0);
        let b_ext = node_b.get_externalized_values(0);
        if a_ext.is_some() && b_ext.is_some() {
            return (a_ext, b_ext);
        }

        if Instant::now() > deadline {
            return (
                node_a.get_externalized_values(0),
                node_b.get_externalized_values(0),
            );
        }

        // Advance the virtual clock by sleeping a little. The nodes' SCP timers
        // use wall-clock `Instant`, so a real (short) sleep lets them fire.
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test_with_logger]
#[serial]
/// Two nodes nominating DISTINCT values (each its own coinbase-style value)
/// with a deterministic "pick one winner" combiner must converge and
/// externalize the SAME block on a SHARED tip (i.e. agree — no fork). This
/// guards the SCP-level convergence property that multi-node consensus relies
/// on: even when the two minters propose different single values, the protocol
/// must agree on one shared block.
fn two_node_distinct_values_converge(logger: Logger) {
    let value_a = "aaa_node_a_value";
    let value_b = "bbb_node_b_value";

    let (a_ext, b_ext) = run_two_node_distinct_values(value_a, value_b, logger);

    let a_block = a_ext.expect("node A never externalized slot 0 (stuck in NominatePrepare)");
    let b_block = b_ext.expect("node B never externalized slot 0 (stuck in NominatePrepare)");

    // Both nodes must externalize the SAME block (shared tip, not a fork). This
    // is the load-bearing SCP safety/agreement assertion.
    assert_eq!(
        a_block, b_block,
        "nodes externalized different blocks (fork!): A={a_block:?} B={b_block:?}"
    );

    // The combiner keeps exactly one winner, and it must be one of the two
    // nominated values.
    assert_eq!(a_block.len(), 1, "expected exactly one combined value");
    assert!(
        a_block[0] == value_a || a_block[0] == value_b,
        "externalized value {:?} is neither nominated value",
        a_block[0]
    );
}

#[test_with_logger]
#[serial]
/// Edge case: both nodes nominate the IDENTICAL value. They must still converge
/// and externalize that value (no regression for the already-agreeing case).
fn two_node_identical_values_converge(logger: Logger) {
    let value = "shared_value";
    let (a_ext, b_ext) = run_two_node_distinct_values(value, value, logger);

    let a_block = a_ext.expect("node A never externalized slot 0");
    let b_block = b_ext.expect("node B never externalized slot 0");

    assert_eq!(a_block, b_block);
    assert_eq!(a_block, vec![value.to_string()]);
}
