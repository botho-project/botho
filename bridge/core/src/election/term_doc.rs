// Copyright (c) 2024 The Botho Foundation

//! Assembly of the `elected`-status v2 term document from a tally result.
//!
//! Per ADR 0010 §5.1 the election has a two-stage document lifecycle:
//!
//! 1. **`elected`** — produced *here*, by the tally. It pins **membership**
//!    only: term, electorate snapshot reference, tally proof (`resultHash`),
//!    threshold, and the elected member *identities* with their approval
//!    counts. It carries **no keys** — fresh per-term keys do not exist yet.
//! 2. **`sealed`** — produced by the seal / keygen step (issue #1066, out of
//!    scope here). Each winner submits fresh per-surface keys bound to this
//!    document, the outgoing federation counter-signs, and only then may the
//!    handover execute.
//!
//! This module owns stage 1 exactly. The `elected` document it emits is the
//! **input** to the seal step: `execution` intents, member `keys`,
//! `keySubmissionSig`, and the `signatures` block are all added during
//! sealing and are deliberately absent here. Keeping that seam clean is the
//! whole point of the `elected` → `sealed` split.

use serde::{Deserialize, Serialize};

use super::{
    snapshot::CurationSnapshot,
    tally::{ElectionParams, TallyResult, TallyStatus},
};

/// The tally rule identifier recorded in the term document.
pub const TALLY_RULE: &str = "approval-top-N-v1";

/// The term-document schema version this assembler emits.
pub const TERM_DOC_VERSION: u8 = 2;

/// Document status. Only `elected` is produced here; `sealed` is the seal
/// step's output (#1066).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TermStatus {
    /// Membership pinned by the tally; no keys yet.
    #[serde(rename = "elected")]
    Elected,
}

/// The `electorate` block: a reference to the pinned curation snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Electorate {
    /// Hash of the operator-signed curation document.
    pub curation_doc_hash: String,
    /// The height the electorate was pinned at.
    pub snapshot_height: u64,
    /// The eligible voter ids, sorted.
    pub eligible: Vec<String>,
}

/// The `tally` block: the rule, window, ballot count, and result hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TallySummary {
    /// The tally rule (`approval-top-N-v1`).
    pub rule: String,
    /// First height ballots were valid.
    pub open_height: u64,
    /// The tally cutoff height.
    pub close_height: u64,
    /// Number of counted ballots.
    pub ballots: u32,
    /// The tally transcript hash ([`TallyResult::result_hash`]).
    pub result_hash: String,
}

/// One elected member. At `elected` status this is identity-only: `keys` and
/// `keySubmissionSig` are added at seal time (#1066) and are absent here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    /// 1-based seat index.
    pub index: u32,
    /// The member's curated identity id.
    pub node_id: String,
    /// Approvals the member received in the tally.
    pub approvals: u32,
}

/// The `validity` block (Unix-seconds timestamps carried from the election
/// parameters — never clock-derived here).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidityDoc {
    /// When the term is considered elected.
    pub elected_at: i64,
    /// Hard handover deadline.
    pub handover_deadline: i64,
    /// Target term end.
    pub term_end: i64,
}

/// An `elected`-status v2 term document (membership only).
///
/// Field order is fixed by struct declaration (serde serialises fields in
/// declaration order regardless of the `serde_json` `preserve_order` feature),
/// so the emitted JSON is deterministic. `execution` and `signatures` are
/// intentionally omitted — they belong to the seal step (#1066), which
/// consumes this document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElectedTermDoc {
    /// Schema version (always `2`).
    pub v: u8,
    /// The elected term.
    pub term: u64,
    /// The election kind (`scheduled` / `emergency` / `mock-same-set`).
    #[serde(rename = "electionKind")]
    pub election_kind: String,
    /// Document status (`elected`).
    pub status: TermStatus,
    /// Electorate snapshot reference.
    pub electorate: Electorate,
    /// Tally proof.
    pub tally: TallySummary,
    /// Signing threshold k.
    pub threshold: u32,
    /// The elected board, in seat order (rank 1 = seat index 1).
    pub members: Vec<Member>,
    /// Validity timestamps.
    pub validity: ValidityDoc,
}

impl ElectedTermDoc {
    /// Serialise to a pretty-printed JSON string (for the CLI / drills).
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).expect("ElectedTermDoc serialises")
    }

    /// Serialise to a compact JSON string.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("ElectedTermDoc serialises")
    }
}

/// Assemble the `elected` term document from a successful tally.
///
/// Returns `Err` unless `result.status == Elected` — a term document can only
/// be cut from an election that produced a viable board. The members are the
/// tally winners in rank order (seat index = rank).
pub fn assemble_elected_term_doc(
    snapshot: &CurationSnapshot,
    params: &ElectionParams,
    result: &TallyResult,
) -> Result<ElectedTermDoc, String> {
    if result.status != TallyStatus::Elected {
        return Err(format!(
            "cannot assemble an elected term document from a {:?} tally",
            result.status
        ));
    }
    if result.winners.is_empty() {
        return Err("elected tally has no winners".to_string());
    }

    let members = result
        .winners
        .iter()
        .enumerate()
        .map(|(i, w)| Member {
            index: (i + 1) as u32,
            node_id: w.node_id.clone(),
            approvals: w.approvals,
        })
        .collect();

    Ok(ElectedTermDoc {
        v: TERM_DOC_VERSION,
        term: params.term,
        election_kind: params.election_kind.wire().to_string(),
        status: TermStatus::Elected,
        electorate: Electorate {
            curation_doc_hash: snapshot.curation_doc_hash.clone(),
            snapshot_height: snapshot.snapshot_height,
            eligible: snapshot.eligible_ids(),
        },
        tally: TallySummary {
            rule: TALLY_RULE.to_string(),
            open_height: params.open_height,
            close_height: params.close_height,
            ballots: result.counted_ballot_txids.len() as u32,
            result_hash: result.result_hash.clone(),
        },
        threshold: params.threshold,
        members,
        validity: ValidityDoc {
            elected_at: params.validity.elected_at,
            handover_deadline: params.validity.handover_deadline,
            term_end: params.validity.term_end,
        },
    })
}
