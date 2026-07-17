// Copyright (c) 2024 The Botho Foundation

//! End-to-end test of the `bridge-tally` CLI: build a synthetic ledger with
//! real signatures using the library, write the `{ params, snapshot, ledger }`
//! bundle to a temp file, run the compiled binary, and assert it emits the
//! correct `elected` term document with exit code 0.

use std::process::Command;

use bth_bridge_core::election::{
    canonical_ballot_memo, canonical_nomination_memo, sign_election_memo_ed25519, CuratedNode,
    CurationSnapshot, ElectionKind, ElectionParams, MemoTransaction, Validity,
};
use ed25519_dalek::SigningKey;
use serde_json::json;

fn key(seed: u8) -> SigningKey {
    let mut b = [0u8; 32];
    b[0] = seed;
    b[31] = seed.wrapping_add(7);
    SigningKey::from_bytes(&b)
}

fn nominate(txid: &str, height: u64, id: &str, k: &SigningKey, term: u64) -> MemoTransaction {
    let memo = canonical_nomination_memo(id, term);
    MemoTransaction {
        txid: txid.into(),
        height,
        signature_hex: sign_election_memo_ed25519(&memo, k),
        memo,
    }
}

fn ballot(
    txid: &str,
    height: u64,
    id: &str,
    k: &SigningKey,
    term: u64,
    approve: &[&str],
) -> MemoTransaction {
    let a: Vec<String> = approve.iter().map(|s| s.to_string()).collect();
    let memo = canonical_ballot_memo(id, term, &a);
    MemoTransaction {
        txid: txid.into(),
        height,
        signature_hex: sign_election_memo_ed25519(&memo, k),
        memo,
    }
}

#[test]
fn cli_tally_emits_elected_term_document() {
    let ids = ["node-a", "node-b", "node-c"];
    let keys: Vec<SigningKey> = (0..3).map(|i| key((i + 1) as u8)).collect();

    let snapshot = CurationSnapshot {
        curation_doc_hash: "cli-curation-hash".into(),
        snapshot_height: 100,
        nodes: ids
            .iter()
            .zip(&keys)
            .map(|(id, k)| CuratedNode {
                node_id: id.to_string(),
                identity_pubkey_hex: hex::encode(k.verifying_key().as_bytes()),
            })
            .collect(),
    };

    let params = ElectionParams {
        term: 5,
        election_kind: ElectionKind::Scheduled,
        open_height: 110,
        close_height: 200,
        seats: 2,
        threshold: 2,
        validity: Validity {
            elected_at: 1000,
            handover_deadline: 2000,
            term_end: 3000,
        },
    };

    let ledger = vec![
        nominate("nom-a", 105, "node-a", &keys[0], 5),
        nominate("nom-b", 105, "node-b", &keys[1], 5),
        nominate("nom-c", 106, "node-c", &keys[2], 5),
        ballot("bal-a", 150, "node-a", &keys[0], 5, &["node-a", "node-b"]),
        ballot("bal-b", 150, "node-b", &keys[1], 5, &["node-a", "node-b"]),
        ballot("bal-c", 151, "node-c", &keys[2], 5, &["node-a"]),
    ];

    let input = json!({
        "params": params,
        "snapshot": snapshot,
        "ledger": ledger,
    });

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("input.json");
    std::fs::write(&path, serde_json::to_string(&input).unwrap()).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_bridge-tally"))
        .arg("tally")
        .arg(&path)
        .output()
        .expect("run bridge-tally");

    assert!(
        out.status.success(),
        "expected success, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&stdout).expect("valid term-doc JSON");

    assert_eq!(doc["v"], 2);
    assert_eq!(doc["term"], 5);
    assert_eq!(doc["status"], "elected");
    assert_eq!(doc["members"].as_array().unwrap().len(), 2);
    // node-a (3 approvals) and node-b (2) win; node-c (0) does not.
    assert_eq!(doc["members"][0]["nodeId"], "node-a");
    assert_eq!(doc["members"][0]["approvals"], 3);
    assert_eq!(doc["members"][1]["nodeId"], "node-b");
    // No key/execution/signature material at the elected stage.
    assert!(doc.get("execution").is_none());
    assert!(doc.get("signatures").is_none());
}

#[test]
fn cli_tally_reports_no_quorum_exit_code() {
    let ids = ["node-a", "node-b", "node-c", "node-d", "node-e"];
    let keys: Vec<SigningKey> = (0..5).map(|i| key((i + 1) as u8)).collect();
    let snapshot = CurationSnapshot {
        curation_doc_hash: "h".into(),
        snapshot_height: 100,
        nodes: ids
            .iter()
            .zip(&keys)
            .map(|(id, k)| CuratedNode {
                node_id: id.to_string(),
                identity_pubkey_hex: hex::encode(k.verifying_key().as_bytes()),
            })
            .collect(),
    };
    let params = ElectionParams {
        term: 1,
        election_kind: ElectionKind::Scheduled,
        open_height: 110,
        close_height: 200,
        seats: 3,
        threshold: 2,
        validity: Validity {
            elected_at: 1,
            handover_deadline: 2,
            term_end: 3,
        },
    };
    // Electorate of 5 (quorum 3) but only one ballot.
    let ledger = vec![
        nominate("nom-a", 105, "node-a", &keys[0], 1),
        nominate("nom-b", 105, "node-b", &keys[1], 1),
        ballot("bal-a", 150, "node-a", &keys[0], 1, &["node-a", "node-b"]),
    ];
    let input = json!({ "params": params, "snapshot": snapshot, "ledger": ledger });
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("input.json");
    std::fs::write(&path, serde_json::to_string(&input).unwrap()).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_bridge-tally"))
        .arg("tally")
        .arg(&path)
        .output()
        .expect("run bridge-tally");
    assert_eq!(out.status.code(), Some(3), "no-quorum must exit 3");
}
