// Copyright (c) 2024 The Botho Foundation

//! Unit tests over synthetic ledgers for the election tally + term-document
//! assembly. No live chain is needed: nodes, curation snapshots, and
//! memo-convention transactions are all constructed in-process.

use ed25519_dalek::SigningKey;

use super::*;

/// A test node with a deterministic curated identity key.
struct TestNode {
    id: String,
    key: SigningKey,
}

fn mk_node(id: &str, seed: u8) -> TestNode {
    let mut bytes = [0u8; 32];
    bytes[0] = seed;
    bytes[31] = seed.wrapping_add(7);
    TestNode {
        id: id.to_string(),
        key: SigningKey::from_bytes(&bytes),
    }
}

fn snapshot_of(nodes: &[&TestNode], height: u64) -> CurationSnapshot {
    CurationSnapshot {
        curation_doc_hash: "curation-doc-hash-abc123".to_string(),
        snapshot_height: height,
        nodes: nodes
            .iter()
            .map(|n| CuratedNode {
                node_id: n.id.clone(),
                identity_pubkey_hex: hex::encode(n.key.verifying_key().as_bytes()),
            })
            .collect(),
    }
}

fn nominate_tx(txid: &str, height: u64, node: &TestNode, term: u64) -> MemoTransaction {
    let memo = canonical_nomination_memo(&node.id, term);
    let signature_hex = sign_election_memo_ed25519(&memo, &node.key);
    MemoTransaction {
        txid: txid.to_string(),
        height,
        memo,
        signature_hex,
    }
}

fn ballot_tx(
    txid: &str,
    height: u64,
    voter: &TestNode,
    term: u64,
    approvals: &[&str],
) -> MemoTransaction {
    let approvals: Vec<String> = approvals.iter().map(|s| s.to_string()).collect();
    let memo = canonical_ballot_memo(&voter.id, term, &approvals);
    let signature_hex = sign_election_memo_ed25519(&memo, &voter.key);
    MemoTransaction {
        txid: txid.to_string(),
        height,
        memo,
        signature_hex,
    }
}

fn params(term: u64, open: u64, close: u64, seats: usize, threshold: u32) -> ElectionParams {
    ElectionParams {
        term,
        election_kind: ElectionKind::Scheduled,
        open_height: open,
        close_height: close,
        seats,
        threshold,
        validity: Validity {
            elected_at: 1_760_400_000,
            handover_deadline: 1_760_659_200,
            term_end: 1_768_435_200,
        },
    }
}

/// Build the canonical five-node scenario used by several tests. Electorate =
/// {a,b,c,d,e} (quorum 3); candidates a,b,c,d nominate before openHeight.
/// Approvals: a=5, b=3, c=1, d=1 — a clean top-2 plus a c/d tie at seat 3.
fn five_node_scenario() -> (
    Vec<TestNode>,
    CurationSnapshot,
    ElectionParams,
    Vec<MemoTransaction>,
) {
    let nodes = vec![
        mk_node("node-a", 1),
        mk_node("node-b", 2),
        mk_node("node-c", 3),
        mk_node("node-d", 4),
        mk_node("node-e", 5),
    ];
    let refs: Vec<&TestNode> = nodes.iter().collect();
    let snap = snapshot_of(&refs, 41_800);
    let p = params(3, 41_810, 42_520, 3, 2);

    let (a, b, c, d, e) = (&nodes[0], &nodes[1], &nodes[2], &nodes[3], &nodes[4]);
    let ledger = vec![
        // Nominations (before openHeight 41_810).
        nominate_tx("nom-a", 41_805, a, 3),
        nominate_tx("nom-b", 41_805, b, 3),
        nominate_tx("nom-c", 41_806, c, 3),
        nominate_tx("nom-d", 41_806, d, 3),
        // Ballots (within 41_810..=42_520).
        ballot_tx("bal-a", 41_900, a, 3, &["node-a", "node-b"]),
        ballot_tx("bal-b", 41_900, b, 3, &["node-a", "node-b"]),
        ballot_tx("bal-c", 41_901, c, 3, &["node-a", "node-c"]),
        ballot_tx("bal-d", 41_901, d, 3, &["node-a", "node-d"]),
        ballot_tx("bal-e", 41_902, e, 3, &["node-a", "node-b"]),
    ];
    (nodes, snap, p, ledger)
}

#[test]
fn clean_win_produces_correct_top_n() {
    let (_nodes, snap, p, ledger) = five_node_scenario();
    let result = tally(&snap, &p, &ledger).unwrap();

    assert_eq!(result.status, TallyStatus::Elected);
    assert_eq!(result.turnout, 5);
    assert_eq!(result.eligible, 5);
    assert_eq!(result.quorum_required, 3);

    // a=5, b=3 clearly top-2; seat 3 is the c/d tie resolved to c.
    let winners: Vec<&str> = result.winners.iter().map(|w| w.node_id.as_str()).collect();
    assert_eq!(winners, vec!["node-a", "node-b", "node-c"]);
    assert_eq!(result.winners[0].approvals, 5);
    assert_eq!(result.winners[1].approvals, 3);
    assert_eq!(result.winners[2].approvals, 1);

    // Full ranking includes the loser d at rank 4.
    assert_eq!(result.ranking.len(), 4);
    assert_eq!(result.ranking[3].node_id, "node-d");
    assert_eq!(result.ranking[3].rank, 4);
}

#[test]
fn tie_break_is_ascending_node_id() {
    let (_nodes, snap, p, ledger) = five_node_scenario();
    let result = tally(&snap, &p, &ledger).unwrap();

    // c and d both have exactly 1 approval; the Nth (3rd) seat must go to the
    // lexicographically smaller nodeId — node-c — deterministically.
    let c = result
        .ranking
        .iter()
        .find(|r| r.node_id == "node-c")
        .unwrap();
    let d = result
        .ranking
        .iter()
        .find(|r| r.node_id == "node-d")
        .unwrap();
    assert_eq!(c.approvals, d.approvals);
    assert!(
        c.rank < d.rank,
        "node-c must outrank node-d on the tie-break"
    );
    assert_eq!(c.rank, 3);
    assert_eq!(d.rank, 4);
    assert!(result.winners.iter().any(|w| w.node_id == "node-c"));
    assert!(!result.winners.iter().any(|w| w.node_id == "node-d"));
}

#[test]
fn tie_break_is_symmetric_regardless_of_input() {
    // Swap which of two equal candidates appears first in the ledger and
    // confirm the deterministic tie-break still seats node-c, not whoever was
    // processed first.
    let (nodes, snap, p, mut ledger) = five_node_scenario();
    let _ = &nodes;
    ledger.reverse();
    let result = tally(&snap, &p, &ledger).unwrap();
    let winners: Vec<&str> = result.winners.iter().map(|w| w.node_id.as_str()).collect();
    assert_eq!(winners, vec!["node-a", "node-b", "node-c"]);
}

#[test]
fn reproducible_under_input_reordering() {
    let (_nodes, snap, p, ledger) = five_node_scenario();
    let baseline = tally(&snap, &p, &ledger).unwrap();

    // Every rotation of the ledger must yield the identical result_hash and
    // winners — the tally is order-independent.
    for shift in 0..ledger.len() {
        let mut rotated = ledger.clone();
        rotated.rotate_left(shift);
        let r = tally(&snap, &p, &rotated).unwrap();
        assert_eq!(r.result_hash, baseline.result_hash, "shift={shift}");
        assert_eq!(r.winners, baseline.winners, "shift={shift}");
        assert_eq!(r.ranking, baseline.ranking, "shift={shift}");
    }

    // And running the exact same inputs twice is byte-identical.
    let again = tally(&snap, &p, &ledger).unwrap();
    assert_eq!(again, baseline);
}

#[test]
fn no_quorum_when_turnout_below_majority() {
    let (nodes, snap, p, _ledger) = five_node_scenario();
    let (a, b, c, d) = (&nodes[0], &nodes[1], &nodes[2], &nodes[3]);
    // Only two of five vote — below the quorum of 3.
    let ledger = vec![
        nominate_tx("nom-a", 41_805, a, 3),
        nominate_tx("nom-b", 41_805, b, 3),
        nominate_tx("nom-c", 41_806, c, 3),
        nominate_tx("nom-d", 41_806, d, 3),
        ballot_tx("bal-a", 41_900, a, 3, &["node-a", "node-b"]),
        ballot_tx("bal-b", 41_900, b, 3, &["node-a", "node-b"]),
    ];
    let result = tally(&snap, &p, &ledger).unwrap();
    assert_eq!(result.status, TallyStatus::NoQuorum);
    assert_eq!(result.turnout, 2);
    assert!(result.winners.is_empty());

    // A term document cannot be cut from a no-quorum election.
    assert!(assemble_elected_term_doc(&snap, &p, &result).is_err());
}

#[test]
fn ineligible_voter_ballot_rejected() {
    let (nodes, snap, p, mut ledger) = five_node_scenario();
    let _ = &nodes;
    // An outsider not in the curation snapshot casts a ballot.
    let outsider = mk_node("node-intruder", 99);
    ledger.push(ballot_tx("bal-x", 41_950, &outsider, 3, &["node-d"]));

    let result = tally(&snap, &p, &ledger).unwrap();
    assert_eq!(result.turnout, 5, "outsider must not count toward turnout");
    assert!(result
        .rejected
        .iter()
        .any(|r| r.txid == "bal-x" && r.reason == "ballot_not_curated"));
    // node-d gained nothing from the outsider's approval.
    let d = result
        .ranking
        .iter()
        .find(|r| r.node_id == "node-d")
        .unwrap();
    assert_eq!(d.approvals, 1);
}

#[test]
fn duplicate_voter_first_ballot_wins() {
    let (nodes, snap, p, mut ledger) = five_node_scenario();
    let e = &nodes[4];
    // node-e already voted (bal-e approves a,b). A second ballot approving c
    // must be rejected — one node, one vote — and c must not gain from it.
    ledger.push(ballot_tx("bal-e2", 42_000, e, 3, &["node-c"]));

    let result = tally(&snap, &p, &ledger).unwrap();
    assert_eq!(result.turnout, 5);
    assert!(result
        .rejected
        .iter()
        .any(|r| r.txid == "bal-e2" && r.reason == "ballot_duplicate_voter"));
    let c = result
        .ranking
        .iter()
        .find(|r| r.node_id == "node-c")
        .unwrap();
    assert_eq!(c.approvals, 1, "the rejected second ballot must not count");
}

#[test]
fn tampered_signature_rejected() {
    let (nodes, snap, p, _ledger) = five_node_scenario();
    let (a, b, c, d, e) = (&nodes[0], &nodes[1], &nodes[2], &nodes[3], &nodes[4]);
    let mut bad = ballot_tx("bal-bad", 41_900, e, 3, &["node-a"]);
    // Flip a byte of the signature.
    bad.signature_hex.replace_range(0..2, "00");
    let ledger = vec![
        nominate_tx("nom-a", 41_805, a, 3),
        nominate_tx("nom-b", 41_805, b, 3),
        nominate_tx("nom-c", 41_806, c, 3),
        nominate_tx("nom-d", 41_806, d, 3),
        ballot_tx("bal-a", 41_900, a, 3, &["node-a"]),
        ballot_tx("bal-b", 41_900, b, 3, &["node-b"]),
        ballot_tx("bal-c", 41_901, c, 3, &["node-c"]),
        bad,
    ];
    let result = tally(&snap, &p, &ledger).unwrap();
    assert!(result
        .rejected
        .iter()
        .any(|r| r.txid == "bal-bad" && r.reason == "ballot_bad_signature"));
    assert_eq!(result.turnout, 3, "the forged ballot must not count");
}

#[test]
fn nomination_after_open_height_rejected() {
    let (nodes, snap, p, _ledger) = five_node_scenario();
    let (a, b, c) = (&nodes[0], &nodes[1], &nodes[2]);
    let ledger = vec![
        nominate_tx("nom-a", 41_805, a, 3),
        nominate_tx("nom-b", 41_805, b, 3),
        // node-c nominates exactly at openHeight — too late.
        nominate_tx("nom-c-late", 41_810, c, 3),
        ballot_tx("bal-a", 41_900, a, 3, &["node-a", "node-b", "node-c"]),
        ballot_tx("bal-b", 41_900, b, 3, &["node-a", "node-b", "node-c"]),
        ballot_tx("bal-c", 41_900, c, 3, &["node-a", "node-b", "node-c"]),
    ];
    let result = tally(&snap, &p, &ledger).unwrap();
    assert!(result
        .rejected
        .iter()
        .any(|r| r.txid == "nom-c-late" && r.reason == "nomination_after_open"));
    // node-c never became a candidate, so approvals for it were dropped.
    assert!(!result.ranking.iter().any(|r| r.node_id == "node-c"));
    assert_eq!(result.ranking.len(), 2);
}

#[test]
fn ballot_outside_window_rejected() {
    let (nodes, snap, p, _ledger) = five_node_scenario();
    let (a, b, c, d, e) = (&nodes[0], &nodes[1], &nodes[2], &nodes[3], &nodes[4]);
    let ledger = vec![
        nominate_tx("nom-a", 41_805, a, 3),
        nominate_tx("nom-b", 41_805, b, 3),
        nominate_tx("nom-c", 41_806, c, 3),
        nominate_tx("nom-d", 41_806, d, 3),
        ballot_tx("bal-a", 41_900, a, 3, &["node-a"]),
        ballot_tx("bal-b", 41_900, b, 3, &["node-b"]),
        ballot_tx("bal-c", 41_901, c, 3, &["node-c"]),
        // After closeHeight (42_520) — must be rejected.
        ballot_tx("bal-late", 42_521, d, 3, &["node-d"]),
        // Before openHeight — must be rejected.
        ballot_tx("bal-early", 41_800, e, 3, &["node-a"]),
    ];
    let result = tally(&snap, &p, &ledger).unwrap();
    assert!(result
        .rejected
        .iter()
        .any(|r| r.txid == "bal-late" && r.reason == "ballot_outside_window"));
    assert!(result
        .rejected
        .iter()
        .any(|r| r.txid == "bal-early" && r.reason == "ballot_outside_window"));
    assert_eq!(result.turnout, 3);
}

#[test]
fn duplicate_nomination_rejected() {
    let (nodes, snap, p, _ledger) = five_node_scenario();
    let (a, b, c) = (&nodes[0], &nodes[1], &nodes[2]);
    let ledger = vec![
        nominate_tx("nom-a", 41_805, a, 3),
        nominate_tx("nom-a-again", 41_806, a, 3),
        nominate_tx("nom-b", 41_805, b, 3),
        ballot_tx("bal-a", 41_900, a, 3, &["node-a", "node-b"]),
        ballot_tx("bal-b", 41_900, b, 3, &["node-a", "node-b"]),
        ballot_tx("bal-c", 41_900, c, 3, &["node-a"]),
    ];
    let result = tally(&snap, &p, &ledger).unwrap();
    assert!(result
        .rejected
        .iter()
        .any(|r| r.txid == "nom-a-again" && r.reason == "nomination_duplicate"));
    // Exactly two candidates, node-a counted once.
    assert_eq!(result.ranking.len(), 2);
}

#[test]
fn insufficient_candidates_when_below_threshold() {
    let (nodes, snap, p, _ledger) = five_node_scenario();
    let (a, b, c) = (&nodes[0], &nodes[1], &nodes[2]);
    // Quorum is met (3 voters) but only ONE candidate stands; threshold is 2.
    let ledger = vec![
        nominate_tx("nom-a", 41_805, a, 3),
        ballot_tx("bal-a", 41_900, a, 3, &["node-a"]),
        ballot_tx("bal-b", 41_900, b, 3, &["node-a"]),
        ballot_tx("bal-c", 41_900, c, 3, &["node-a"]),
    ];
    let result = tally(&snap, &p, &ledger).unwrap();
    assert_eq!(result.status, TallyStatus::InsufficientCandidates);
    assert!(result.winners.is_empty());
    assert!(assemble_elected_term_doc(&snap, &p, &result).is_err());
}

#[test]
fn approval_of_non_candidate_is_dropped_but_ballot_counts() {
    let (nodes, snap, p, _ledger) = five_node_scenario();
    let (a, b, c) = (&nodes[0], &nodes[1], &nodes[2]);
    let ledger = vec![
        nominate_tx("nom-a", 41_805, a, 3),
        nominate_tx("nom-b", 41_805, b, 3),
        // node-c is NOT nominated; approvals for it must be dropped.
        ballot_tx("bal-a", 41_900, a, 3, &["node-a", "node-c"]),
        ballot_tx("bal-b", 41_900, b, 3, &["node-b", "node-c"]),
        ballot_tx("bal-c", 41_900, c, 3, &["node-a", "node-b"]),
    ];
    let result = tally(&snap, &p, &ledger).unwrap();
    assert_eq!(
        result.turnout, 3,
        "all three ballots still count as turnout"
    );
    assert!(!result.ranking.iter().any(|r| r.node_id == "node-c"));
    let a_standing = result
        .ranking
        .iter()
        .find(|r| r.node_id == "node-a")
        .unwrap();
    assert_eq!(a_standing.approvals, 2);
}

#[test]
fn curation_snapshot_binding_is_the_electorate() {
    // The tally binds to the snapshot, not to whoever holds a key. A ballot
    // from a validly-signed node absent from THIS snapshot does not count,
    // even though its signature is cryptographically valid.
    let a = mk_node("node-a", 1);
    let b = mk_node("node-b", 2);
    let c = mk_node("node-c", 3);
    // Snapshot pins only a and b.
    let snap = snapshot_of(&[&a, &b], 41_800);
    let p = params(3, 41_810, 42_520, 2, 2);
    let ledger = vec![
        nominate_tx("nom-a", 41_805, &a, 3),
        nominate_tx("nom-b", 41_805, &b, 3),
        // c is not in the snapshot — its nomination and ballot are ignored.
        nominate_tx("nom-c", 41_805, &c, 3),
        ballot_tx("bal-a", 41_900, &a, 3, &["node-a", "node-b"]),
        ballot_tx("bal-b", 41_900, &b, 3, &["node-a", "node-b"]),
        ballot_tx("bal-c", 41_900, &c, 3, &["node-a"]),
    ];
    let result = tally(&snap, &p, &ledger).unwrap();
    assert_eq!(result.eligible, 2);
    assert_eq!(result.quorum_required, 2);
    assert_eq!(result.turnout, 2, "c is not in the electorate");
    assert!(result
        .rejected
        .iter()
        .any(|r| r.txid == "nom-c" && r.reason == "nomination_not_curated"));
    assert!(result
        .rejected
        .iter()
        .any(|r| r.txid == "bal-c" && r.reason == "ballot_not_curated"));
    assert_eq!(result.status, TallyStatus::Elected);
    assert_eq!(result.winners.len(), 2);
}

#[test]
fn assemble_elected_term_document() {
    let (_nodes, snap, p, ledger) = five_node_scenario();
    let result = tally(&snap, &p, &ledger).unwrap();
    let doc = assemble_elected_term_doc(&snap, &p, &result).unwrap();

    assert_eq!(doc.v, 2);
    assert_eq!(doc.term, 3);
    assert_eq!(doc.election_kind, "scheduled");
    assert_eq!(doc.status, TermStatus::Elected);
    assert_eq!(doc.threshold, 2);

    // Electorate = sorted eligible ids.
    assert_eq!(
        doc.electorate.eligible,
        vec!["node-a", "node-b", "node-c", "node-d", "node-e"]
    );
    assert_eq!(doc.electorate.snapshot_height, 41_800);
    assert_eq!(doc.electorate.curation_doc_hash, "curation-doc-hash-abc123");

    // Tally block mirrors the result.
    assert_eq!(doc.tally.rule, "approval-top-N-v1");
    assert_eq!(doc.tally.open_height, 41_810);
    assert_eq!(doc.tally.close_height, 42_520);
    assert_eq!(doc.tally.ballots, 5);
    assert_eq!(doc.tally.result_hash, result.result_hash);

    // Members are the winners in seat order (index 1..=N), keys absent.
    assert_eq!(doc.members.len(), 3);
    assert_eq!(doc.members[0].index, 1);
    assert_eq!(doc.members[0].node_id, "node-a");
    assert_eq!(doc.members[2].index, 3);
    assert_eq!(doc.members[2].node_id, "node-c");

    // validity carried verbatim.
    assert_eq!(doc.validity.elected_at, 1_760_400_000);

    // Round-trips through JSON, and the emitted JSON is deterministic (no
    // execution/signatures blocks — those belong to the seal step).
    let json = doc.to_json();
    let back: ElectedTermDoc = serde_json::from_str(&json).unwrap();
    assert_eq!(back, doc);
    assert!(!json.contains("execution"));
    assert!(!json.contains("signatures"));
    assert!(json.contains("\"status\":\"elected\""));
}

// --- memo-format unit tests ---

#[test]
fn nomination_memo_round_trips() {
    let memo = canonical_nomination_memo("node-a", 7);
    assert_eq!(
        memo,
        r#"{"kind":"bridge.nominate","nodeId":"node-a","term":7,"v":1}"#
    );
    let parsed = parse_election_memo(&memo).unwrap();
    assert_eq!(
        parsed,
        ElectionMemo::Nomination {
            term: 7,
            node_id: "node-a".to_string()
        }
    );
}

#[test]
fn ballot_memo_sorts_and_dedups_approvals() {
    let memo = canonical_ballot_memo(
        "node-a",
        7,
        &[
            "node-c".to_string(),
            "node-a".to_string(),
            "node-c".to_string(),
            "node-b".to_string(),
        ],
    );
    assert_eq!(
        memo,
        r#"{"approvals":["node-a","node-b","node-c"],"kind":"bridge.ballot","nodeId":"node-a","term":7,"v":1}"#
    );
    let parsed = parse_election_memo(&memo).unwrap();
    match parsed {
        ElectionMemo::Ballot { approvals, .. } => {
            assert_eq!(approvals, vec!["node-a", "node-b", "node-c"]);
        }
        _ => panic!("expected a ballot"),
    }
}

#[test]
fn memo_parse_rejects_unknown_and_duplicate_and_bad_version() {
    // Unknown field.
    assert!(parse_election_memo(
        r#"{"kind":"bridge.nominate","nodeId":"node-a","term":7,"v":1,"x":1}"#
    )
    .is_err());
    // Duplicate key (serde would silently last-wins).
    assert!(parse_election_memo(
        r#"{"kind":"bridge.nominate","kind":"bridge.ballot","nodeId":"node-a","term":7,"v":1}"#
    )
    .is_err());
    // Wrong version.
    assert!(
        parse_election_memo(r#"{"kind":"bridge.nominate","nodeId":"node-a","term":7,"v":2}"#)
            .is_err()
    );
    // Unknown kind.
    assert!(
        parse_election_memo(r#"{"kind":"bridge.bribe","nodeId":"node-a","term":7,"v":1}"#).is_err()
    );
    // Float where an integer is required.
    assert!(parse_election_memo(
        r#"{"kind":"bridge.nominate","nodeId":"node-a","term":7.0,"v":1}"#
    )
    .is_err());
}

#[test]
fn memo_signature_verify_roundtrip() {
    let node = mk_node("node-a", 42);
    let memo = canonical_nomination_memo("node-a", 3);
    let sig = sign_election_memo_ed25519(&memo, &node.key);
    let vk = node.key.verifying_key();
    assert!(verify_election_memo(&memo, &sig, &vk).is_ok());

    // A different memo does not verify under the same signature.
    let other = canonical_nomination_memo("node-a", 4);
    assert!(verify_election_memo(&other, &sig, &vk).is_err());

    // A different key does not verify.
    let other_key = mk_node("node-b", 43).key.verifying_key();
    assert!(verify_election_memo(&memo, &sig, &other_key).is_err());
}

#[test]
fn params_validation_rejects_bad_shapes() {
    // close < open
    assert!(params(1, 100, 50, 3, 2).validate().is_err());
    // threshold > seats
    assert!(params(1, 10, 20, 2, 3).validate().is_err());
    // seats < 2
    assert!(params(1, 10, 20, 1, 1).validate().is_err());
    // valid
    assert!(params(1, 10, 20, 5, 3).validate().is_ok());
}
