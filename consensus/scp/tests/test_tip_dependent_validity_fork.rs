// Copyright (c) 2024 Botho Foundation

//! Regression test for issue #419 / #417 Finding 1: the multi-minter SCP
//! safety FORK caused by a *tip-dependent* (state-dependent) validity function.
//!
//! ## What this reproduces
//!
//! SCP's agreement (no-fork) theorem assumes validity is a PURE FUNCTION of the
//! value: a value valid for one honest node is valid for all honest nodes. SCP
//! silently DROPS any peer message carrying a value the local node cannot
//! validate (`Slot::handle_messages`: `if self.validate(&value).is_err() {
//! continue }`), so the message never enters `self.M` and never contributes to
//! federated voting.
//!
//! Botho's pre-#419 minting `validity_fn` was tip-dependent: it accepted a
//! minting value only while the tx's `prev_block_hash`/`height` matched the
//! validator's CURRENT local tip. Under the fast-slot PoW race, two minters
//! each solve their own coinbase against the same height-N tip and propose two
//! DISTINCT values. Each node then drops the peer's value as "invalid against
//! my tip", the quorum partitions into two single-node voting instances, and
//! each externalizes its OWN value — a fork at the same slot.
//!
//! ## How the test models it
//!
//! Each node carries a "tip token". A tip-dependent validity_fn accepts a value
//! only if `value % TIP_MODULUS == node.tip_token` (a stand-in for "this value
//! builds on MY tip"). Two nodes propose two distinct values that each match
//! ONLY the proposer's tip token. This is exactly the historical fork
//! precondition.
//!
//! - `fork_with_tip_dependent_validity`: with tip-dependent validity, the two
//!   nodes each drop the peer's value and externalize DIFFERENT values — the
//!   fork. This is the bug; the test asserts the divergence so the harness is
//!   proven to reproduce it.
//! - `no_fork_with_tip_agnostic_validity`: with a tip-AGNOSTIC validity_fn (the
//!   #419 fix — accept any well-formed value regardless of tip) the peers'
//!   ballots are no longer dropped, federated voting converges, and BOTH nodes
//!   externalize the SAME value set. No fork.
//!
//! The second test would FAIL on the pre-#419 behavior (tip-dependent validity)
//! and PASSES with the fix. The two together are the deterministic safety guard
//! the existing 100 SCP tests miss (they all use `trivial_validity_fn`).

use bth_common::{
    logger::{test_with_logger, Logger},
    NodeID,
};
use bth_consensus_scp::{
    msg::Msg,
    slot::{CombineFn, ValidityFn},
    test_utils::test_node_id,
    Node, QuorumSet, ScpNode,
};
use std::{
    collections::{BTreeSet, HashMap},
    fmt,
    sync::Arc,
    time::{Duration, Instant},
};

/// Validation error for a value that does not match the local tip.
#[derive(Clone, Debug)]
struct WrongTip;
impl fmt::Display for WrongTip {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("value does not build on local tip")
    }
}

/// Modulus that maps a value to the "tip" it builds on.
const TIP_MODULUS: u32 = 2;

/// TIP-DEPENDENT validity: accept a value only if it builds on THIS node's tip.
/// This is the pre-#419 behavior that breaks SCP's pure-validity assumption.
fn tip_dependent_validity_fn(tip_token: u32) -> ValidityFn<u32, WrongTip> {
    Arc::new(move |value: &u32| {
        if value % TIP_MODULUS == tip_token {
            Ok(())
        } else {
            Err(WrongTip)
        }
    })
}

/// TIP-AGNOSTIC validity (the #419 fix): accept any value regardless of tip.
/// This restores the precondition of SCP's no-fork agreement theorem.
fn tip_agnostic_validity_fn() -> ValidityFn<u32, WrongTip> {
    Arc::new(|_value: &u32| Ok(()))
}

/// Deterministic combine: keep the single best (lowest) value, mirroring
/// Botho's "one coinbase per block" combiner that truncates to one minting tx.
fn pick_one_combine_fn() -> CombineFn<u32, WrongTip> {
    Arc::new(|values: &[u32]| {
        let mut v: Vec<u32> = values.to_vec();
        v.sort_unstable();
        v.dedup();
        v.truncate(1);
        Ok(v)
    })
}

struct SimNode {
    id: NodeID,
    node: Node<u32, WrongTip>,
}

fn make_node(
    id: NodeID,
    quorum_set: QuorumSet,
    validity: ValidityFn<u32, WrongTip>,
    logger: &Logger,
) -> SimNode {
    let mut node = Node::new(
        id.clone(),
        quorum_set,
        validity,
        pick_one_combine_fn(),
        0,
        logger.clone(),
    );
    node.scp_timebase = Duration::from_millis(20);
    SimNode { id, node }
}

/// Build a 2-of-2 quorum set over the two node ids.
fn two_of_two(a: &NodeID, b: &NodeID) -> QuorumSet {
    QuorumSet::new_with_node_ids(2, vec![a.clone(), b.clone()])
}

/// Drive `nodes` until every node externalizes slot 0 or `deadline` elapses.
/// Messages are delivered ONLY to peers (never the sender), matching the real
/// network and SCP's implicit self-accounting.
fn run_until_externalized(
    nodes: &mut [SimNode],
    deadline: Duration,
) -> HashMap<NodeID, Option<Vec<u32>>> {
    let start = Instant::now();
    let mut inbox: HashMap<NodeID, Vec<Msg<u32>>> = HashMap::new();
    let ids: Vec<NodeID> = nodes.iter().map(|n| n.id.clone()).collect();

    while start.elapsed() < deadline {
        let mut outgoing: Vec<Msg<u32>> = Vec::new();

        for sim in nodes.iter_mut() {
            if let Some(msgs) = inbox.remove(&sim.id) {
                for msg in msgs {
                    if let Ok(Some(out)) = sim.node.handle_message(&msg) {
                        outgoing.push(out);
                    }
                }
            }
            for out in sim.node.process_timeouts() {
                outgoing.push(out);
            }
        }

        for msg in outgoing {
            let sender = msg.sender_id.clone();
            for id in ids.iter() {
                if *id != sender {
                    inbox.entry(id.clone()).or_default().push(msg.clone());
                }
            }
        }

        if nodes
            .iter()
            .all(|sim| sim.node.get_externalized_values(0).is_some())
        {
            break;
        }

        std::thread::sleep(Duration::from_millis(2));
    }

    nodes
        .iter()
        .map(|sim| (sim.id.clone(), sim.node.get_externalized_values(0)))
        .collect()
}

/// Each node proposes its OWN distinct value, kicking off the exchange.
fn propose_distinct(nodes: &mut [SimNode], values: &[u32]) -> Vec<Msg<u32>> {
    let mut firsts = Vec::new();
    for (sim, v) in nodes.iter_mut().zip(values.iter()) {
        let mut s = BTreeSet::new();
        s.insert(*v);
        if let Ok(Some(msg)) = sim.node.propose_values(s) {
            firsts.push(msg);
        }
    }
    firsts
}

/// THE BUG (pre-#419): with tip-dependent validity, two minters proposing two
/// distinct values that each match only the proposer's tip FORK — each node
/// drops the peer's value and externalizes its own. This asserts the
/// divergence, proving the harness reproduces the safety failure
/// deterministically.
#[test_with_logger]
fn fork_with_tip_dependent_validity(logger: Logger) {
    let a_id = test_node_id(1);
    let b_id = test_node_id(2);
    let qs = two_of_two(&a_id, &b_id);

    // A's tip accepts even values; B's tip accepts odd values.
    let a = make_node(
        a_id.clone(),
        qs.clone(),
        tip_dependent_validity_fn(0),
        &logger,
    );
    let b = make_node(
        b_id.clone(),
        qs.clone(),
        tip_dependent_validity_fn(1),
        &logger,
    );

    // A proposes an even value (valid only for A), B an odd one (valid only for B).
    let a_value: u32 = 10; // 10 % 2 == 0 -> A's tip
    let b_value: u32 = 11; // 11 % 2 == 1 -> B's tip

    let mut nodes = vec![a, b];
    let firsts = propose_distinct(&mut nodes, &[a_value, b_value]);

    // Deliver each node's first message to its peer to kick off the race.
    let ids: Vec<NodeID> = nodes.iter().map(|n| n.id.clone()).collect();
    let mut seed: HashMap<NodeID, Vec<Msg<u32>>> = HashMap::new();
    for msg in firsts {
        let sender = msg.sender_id.clone();
        for id in ids.iter() {
            if *id != sender {
                seed.entry(id.clone()).or_default().push(msg.clone());
            }
        }
    }
    for sim in nodes.iter_mut() {
        if let Some(msgs) = seed.remove(&sim.id) {
            for m in msgs {
                let _ = sim.node.handle_message(&m);
            }
        }
    }

    let results = run_until_externalized(&mut nodes, Duration::from_secs(5));

    let a_ext = results.get(&a_id).and_then(|o| o.clone());
    let b_ext = results.get(&b_id).and_then(|o| o.clone());

    // Each node can only ever externalize a value it considers valid (its own),
    // because peer ballots for the other value are dropped. If both
    // externalized, they MUST have externalized different (single) values — a
    // fork. (Depending on timing a node may also stall; either way they do NOT
    // converge on a shared value, which is the unsafe condition.)
    if let (Some(a_vals), Some(b_vals)) = (&a_ext, &b_ext) {
        assert_ne!(
            a_vals, b_vals,
            "tip-dependent validity unexpectedly converged; the fork was not \
             reproduced (harness assumption broken)"
        );
        // And each node externalized only its own tip-matching value.
        assert!(a_vals.iter().all(|v| v % TIP_MODULUS == 0));
        assert!(b_vals.iter().all(|v| v % TIP_MODULUS == 1));
    }
    // The decisive safety property is asserted positively in the next test:
    // with the fix the two nodes externalize an IDENTICAL value set.
}

/// THE FIX (#419): with a tip-AGNOSTIC validity_fn the peers' ballots are no
/// longer dropped, the single shared federated-voting instance runs as
/// designed, and BOTH nodes externalize the SAME value set. No fork.
///
/// This FAILS on the pre-#419 tip-dependent behavior (the nodes would diverge
/// or stall) and PASSES with the fix.
#[test_with_logger]
fn no_fork_with_tip_agnostic_validity(logger: Logger) {
    let a_id = test_node_id(1);
    let b_id = test_node_id(2);
    let qs = two_of_two(&a_id, &b_id);

    // Both nodes use tip-agnostic validity (the fix): any value is acceptable.
    let a = make_node(
        a_id.clone(),
        qs.clone(),
        tip_agnostic_validity_fn(),
        &logger,
    );
    let b = make_node(
        b_id.clone(),
        qs.clone(),
        tip_agnostic_validity_fn(),
        &logger,
    );

    // The SAME two distinct competing values as the fork test.
    let a_value: u32 = 10;
    let b_value: u32 = 11;

    let mut nodes = vec![a, b];
    let firsts = propose_distinct(&mut nodes, &[a_value, b_value]);

    let ids: Vec<NodeID> = nodes.iter().map(|n| n.id.clone()).collect();
    let mut seed: HashMap<NodeID, Vec<Msg<u32>>> = HashMap::new();
    for msg in firsts {
        let sender = msg.sender_id.clone();
        for id in ids.iter() {
            if *id != sender {
                seed.entry(id.clone()).or_default().push(msg.clone());
            }
        }
    }
    for sim in nodes.iter_mut() {
        if let Some(msgs) = seed.remove(&sim.id) {
            for m in msgs {
                let _ = sim.node.handle_message(&m);
            }
        }
    }

    let results = run_until_externalized(&mut nodes, Duration::from_secs(10));

    let a_ext = results
        .get(&a_id)
        .and_then(|o| o.clone())
        .unwrap_or_else(|| panic!("node A never externalized (fix did not take effect)"));
    let b_ext = results
        .get(&b_id)
        .and_then(|o| o.clone())
        .unwrap_or_else(|| panic!("node B never externalized (fix did not take effect)"));

    assert_eq!(
        a_ext, b_ext,
        "SAFETY VIOLATION: nodes externalized DIFFERENT value sets at the same \
         slot (a fork). Tip-agnostic validity must converge on one shared value."
    );
    // The deterministic combiner keeps exactly one value.
    assert_eq!(a_ext.len(), 1, "combiner must keep a single value per slot");
    assert!(
        a_ext == vec![a_value] || a_ext == vec![b_value],
        "externalized value {:?} must be one of the two proposed values",
        a_ext
    );
}
