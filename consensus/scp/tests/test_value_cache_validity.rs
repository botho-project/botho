// Copyright (c) 2024 Botho Foundation

//! Regression test for issue #409: real multi-node SCP never externalizes a
//! block (stuck in NominatePrepare) when a node cannot validate the value a
//! peer is nominating.
//!
//! ## What this reproduces
//!
//! In Botho the SCP `validity_fn` validates a `ConsensusValue` by looking up
//! the referenced transaction bytes in a per-node cache
//! (`ConsensusService::shared_state.tx_cache`). SCP messages carry only the
//! value's `tx_hash`, not the bytes. A minting node populates its OWN cache
//! when it proposes, but before issue #409 it never broadcast the minting-tx
//! bytes to peers. So when node A nominates its minting value, node B receives
//! A's SCP message, tries to validate the referenced value, fails ("not in
//! cache"), and **drops A's message** in `Slot::handle_messages` (the message
//! is never inserted into `self.M`).
//!
//! Because the SCP quorum/blocking-set math counts the local node implicitly
//! (see `QuorumSet::findQuorum`, which seeds `nodes_so_far` with the local
//! node) but requires a *message* from each peer to count that peer, dropping
//! the peer's message means neither node ever sees a quorum of nominate votes.
//! `Z` (confirmed-nominated) stays empty, balloting never starts, and nothing
//! externalizes — exactly the observed symptom.
//!
//! ## What this test proves
//!
//! - `stuck_when_peer_value_uncached`: with the pre-#409 wiring (peers never
//!   learn each other's proposed value), no node externalizes. This is the bug.
//! - `externalizes_when_peer_value_cached`: with the #409 fix (the proposer's
//!   value is registered in every node's cache, as the new minting-tx gossip
//!   path does via `ConsensusService::register_minting_tx`), both nodes
//!   externalize the same value over a bounded number of ticks.
//!
//! The harness deliberately shuttles each node's emitted messages ONLY to its
//! peers (never back to itself), proving SCP counts self-votes implicitly and
//! that the host does NOT need to re-deliver self-messages — the
//! `peer_count() == 0` self-loopback guard in `run.rs` is therefore not the
//! root cause.

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
    collections::{BTreeSet, HashMap, HashSet},
    fmt,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// Validation error used by the cache-backed validity fn.
#[derive(Clone, Debug)]
struct NotCached;
impl fmt::Display for NotCached {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("value not in local cache")
    }
}

/// A shared, per-node value cache. A node can only validate (and therefore
/// accept a peer's vote for) a value whose hash is present in its cache. This
/// mirrors Botho's `tx_cache`-backed `validity_fn`.
type Cache = Arc<Mutex<HashSet<u32>>>;

fn cache_backed_validity_fn(cache: Cache) -> ValidityFn<u32, NotCached> {
    Arc::new(move |value: &u32| {
        if cache.lock().expect("cache lock").contains(value) {
            Ok(())
        } else {
            Err(NotCached)
        }
    })
}

fn sorted_combine_fn() -> CombineFn<u32, NotCached> {
    Arc::new(|values: &[u32]| {
        let mut v: Vec<u32> = values.to_vec();
        v.sort_unstable();
        v.dedup();
        Ok(v)
    })
}

/// One simulated node: an SCP `Node` plus its private value cache.
struct SimNode {
    id: NodeID,
    node: Node<u32, NotCached>,
    cache: Cache,
}

fn make_node(id: NodeID, quorum_set: QuorumSet, logger: &Logger) -> SimNode {
    let cache: Cache = Arc::new(Mutex::new(HashSet::new()));
    let mut node = Node::new(
        id.clone(),
        quorum_set,
        cache_backed_validity_fn(cache.clone()),
        sorted_combine_fn(),
        0,
        logger.clone(),
    );
    // Run the protocol fast so a bounded real-time test converges quickly.
    node.scp_timebase = Duration::from_millis(20);
    SimNode { id, node, cache }
}

/// Drive `nodes` until every node externalizes slot 0, or until `deadline`.
///
/// Messages emitted by a node are delivered ONLY to its peers (never to
/// itself), matching the real network and the contract that SCP accounts for
/// self implicitly. Returns the per-node externalized value sets (empty for a
/// node that never externalized).
fn run_until_externalized(
    nodes: &mut [SimNode],
    deadline: Duration,
) -> HashMap<NodeID, Option<Vec<u32>>> {
    let start = Instant::now();
    let mut inbox: HashMap<NodeID, Vec<Msg<u32>>> = HashMap::new();

    // Kick off: each node proposes its single value (already in its own cache).
    // We seed proposals from the caller via the cache contents below; here we
    // just record that nothing has externalized yet.
    let ids: Vec<NodeID> = nodes.iter().map(|n| n.id.clone()).collect();

    while start.elapsed() < deadline {
        let mut outgoing: Vec<Msg<u32>> = Vec::new();

        for sim in nodes.iter_mut() {
            // Deliver any pending messages addressed to this node.
            if let Some(msgs) = inbox.remove(&sim.id) {
                for msg in msgs {
                    if let Ok(Some(out)) = sim.node.handle_message(&msg) {
                        outgoing.push(out);
                    }
                }
            }

            // Drive timeouts (nomination rounds / ballot timers).
            for out in sim.node.process_timeouts() {
                outgoing.push(out);
            }
        }

        // Fan out emitted messages to peers only (never to the sender).
        for msg in outgoing {
            let sender = msg.sender_id.clone();
            for id in ids.iter() {
                if *id != sender {
                    inbox.entry(id.clone()).or_default().push(msg.clone());
                }
            }
        }

        // Stop early once everyone has externalized.
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

/// Build a 2-of-2 quorum set over the two node ids.
fn two_of_two(a: &NodeID, b: &NodeID) -> QuorumSet {
    QuorumSet::new_with_node_ids(2, vec![a.clone(), b.clone()])
}

/// PRE-FIX BEHAVIOR: node A proposes a minting value that only A has in its
/// cache. B can never validate A's value, so B drops A's nominate messages and
/// neither node reaches a confirmed-nominated quorum. Nothing externalizes.
///
/// This asserts the original stall. It MUST hold on the pre-#409 code path
/// (peers never learn each other's proposed value).
#[test_with_logger]
fn stuck_when_peer_value_uncached(logger: Logger) {
    let a_id = test_node_id(1);
    let b_id = test_node_id(2);
    let qs = two_of_two(&a_id, &b_id);

    let mut a = make_node(a_id.clone(), qs.clone(), &logger);
    let b = make_node(b_id.clone(), qs.clone(), &logger);

    // The proposed value (e.g. A's minting tx hash, abstracted to a u32).
    let value: u32 = 42;

    // Only A has the value in its cache — peers were NOT told about it.
    a.cache.lock().unwrap().insert(value);

    // A proposes; B never proposes anything (Case 2: single proposer).
    let mut to_propose = BTreeSet::new();
    to_propose.insert(value);
    let first = a.node.propose_values(to_propose).expect("propose failed");

    let mut nodes = vec![a, b];

    // Seed B's inbox with A's first nominate message, if any.
    if let Some(msg) = first {
        // Deliver to B only (peers, not self).
        let b_idx = 1;
        if let Ok(Some(out)) = nodes[b_idx].node.handle_message(&msg) {
            // B should NOT be able to produce a meaningful accept here.
            let _ = out;
        }
    }

    let results = run_until_externalized(&mut nodes, Duration::from_secs(3));

    for (id, externalized) in &results {
        assert!(
            externalized.is_none(),
            "node {:?} unexpectedly externalized {:?} although the proposed \
             value was never cached on peers (this is the #409 stall; if this \
             fails the bug is not reproduced)",
            id,
            externalized
        );
    }
}

/// POST-FIX BEHAVIOR: the proposer's value is registered in EVERY node's cache
/// (as the #409 minting-tx gossip path does via `register_minting_tx`). Now B
/// can validate A's value, accepts A's nominate votes, the 2-of-2 quorum forms,
/// `Z` becomes non-empty, balloting runs, and both nodes externalize the same
/// value.
#[test_with_logger]
fn externalizes_when_peer_value_cached(logger: Logger) {
    let a_id = test_node_id(1);
    let b_id = test_node_id(2);
    let qs = two_of_two(&a_id, &b_id);

    let mut a = make_node(a_id.clone(), qs.clone(), &logger);
    let b = make_node(b_id.clone(), qs.clone(), &logger);

    let value: u32 = 42;

    // The #409 fix: every consensus participant learns the proposed value
    // (minting tx bytes are gossiped and registered into each node's cache).
    a.cache.lock().unwrap().insert(value);
    b.cache.lock().unwrap().insert(value);

    let mut to_propose = BTreeSet::new();
    to_propose.insert(value);
    let first = a.node.propose_values(to_propose).expect("propose failed");

    let mut nodes = vec![a, b];

    if let Some(msg) = first {
        let b_idx = 1;
        if let Ok(Some(out)) = nodes[b_idx].node.handle_message(&msg) {
            // Fan B's response back to A so the exchange starts immediately.
            if let Ok(Some(_)) = nodes[0].node.handle_message(&out) {}
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
        "nodes externalized different value sets — determinism / shared-tip violation"
    );
    assert!(
        a_ext.contains(&value),
        "externalized set {:?} does not contain the proposed value {}",
        a_ext,
        value
    );
}
