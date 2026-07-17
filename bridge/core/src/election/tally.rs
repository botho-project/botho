// Copyright (c) 2024 The Botho Foundation

//! The deterministic, off-consensus tally (`approval-top-N-v1`).
//!
//! [`tally`] is a **pure function**: given a curation snapshot and the ledger
//! of election memos up to `closeHeight`, it returns the ranking and winners
//! with:
//!
//! - **no wall-clock and no randomness** — validity timestamps are inputs;
//! - **input-order independence** — memos are sorted by `(height, txid)` before
//!   counting, so the same ledger in any order yields the same result;
//! - a **deterministic, grind-free tie-break** — ties at the Nth seat break by
//!   ascending lexicographic `nodeId` (ADR 0010 §6.3), never by ledger-derived
//!   entropy a block producer could grind;
//! - a **full ranking** (not just the top N), so a member who fails key
//!   submission can be replaced by the next-ranked candidate without a re-vote
//!   (ADR 0010 §6.2 exclusion-and-retry).
//!
//! Anyone replaying the ledger recomputes the identical `resultHash`.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    memo::{parse_election_memo, verify_election_memo, ElectionMemo},
    snapshot::CurationSnapshot,
};

/// Domain prefix for the tally transcript hash (`resultHash`). Distinct from
/// the memo-signing domain so the two can never collide.
pub const TALLY_TRANSCRIPT_DOMAIN: &[u8] = b"botho.bridge.tally.v1:";

/// The election kind, matching the term-document `electionKind` wire values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ElectionKind {
    /// A regular scheduled (quarterly) election.
    #[serde(rename = "scheduled")]
    Scheduled,
    /// A compressed-window emergency election (ADR 0010 §6.5).
    #[serde(rename = "emergency")]
    Emergency,
    /// The #1063 drill's same-set mock (membership-stable, keys-fresh).
    #[serde(rename = "mock-same-set")]
    MockSameSet,
}

impl ElectionKind {
    /// The wire string used in the term document's `electionKind` field.
    pub fn wire(&self) -> &'static str {
        match self {
            ElectionKind::Scheduled => "scheduled",
            ElectionKind::Emergency => "emergency",
            ElectionKind::MockSameSet => "mock-same-set",
        }
    }
}

/// The term-document `validity` timestamps. These are POLICY INPUTS to the
/// tally (Unix seconds), never read from the clock, so the emitted document is
/// reproducible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Validity {
    /// When the term is considered elected (Unix seconds).
    pub elected_at: i64,
    /// Hard handover deadline (Unix seconds); breach is objective (ADR 0010).
    pub handover_deadline: i64,
    /// Target term end (Unix seconds).
    pub term_end: i64,
}

/// The parameters that pin one election: term, window, board shape, and the
/// (input) validity timestamps. `seats` is N (target 5) and `threshold` is k
/// (target 3) for the ratified 3-of-5 shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ElectionParams {
    /// The term being elected (≥ 1).
    pub term: u64,
    /// The election kind.
    pub election_kind: ElectionKind,
    /// First height at which ballots are valid (and after which nominations
    /// are closed).
    pub open_height: u64,
    /// Last height at which ballots are valid (the tally cutoff).
    pub close_height: u64,
    /// Board size N (number of seats to fill).
    pub seats: usize,
    /// Signing threshold k (the elected board must have at least this many
    /// members to be viable).
    pub threshold: u32,
    /// Term-document validity timestamps (input, not clock-derived).
    pub validity: Validity,
}

impl ElectionParams {
    /// Structural validation of the parameters (independent of any ledger).
    pub fn validate(&self) -> Result<(), String> {
        if self.term < 1 {
            return Err("term must be >= 1".to_string());
        }
        if self.close_height < self.open_height {
            return Err("closeHeight precedes openHeight".to_string());
        }
        if self.seats < 2 {
            return Err("seats (N) must be >= 2".to_string());
        }
        if self.threshold < 2 {
            return Err("threshold (k) must be >= 2".to_string());
        }
        if (self.threshold as usize) > self.seats {
            return Err("threshold (k) exceeds seats (N)".to_string());
        }
        Ok(())
    }
}

/// One memo-convention transaction as it appears on the ledger. The tally
/// treats `memo` as opaque signed bytes; `signature_hex` is the detached
/// curated-identity signature over `ELECTION_MEMO_DOMAIN || memo`.
///
/// `txid` is the ledger transaction id — it fixes the deterministic ordering
/// (with `height`) and is the unit recorded in the tally transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoTransaction {
    /// The ledger transaction id.
    pub txid: String,
    /// The height the transaction confirmed at.
    pub height: u64,
    /// The canonical election-memo JSON string, verbatim as signed.
    pub memo: String,
    /// The detached lowercase-hex Ed25519 signature over the domain-separated
    /// memo bytes.
    pub signature_hex: String,
}

/// Why a memo transaction did not count. Surfaced for observability and tests
/// (never affects determinism — the set of rejects is itself a pure function
/// of the inputs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RejectedMemo {
    /// The rejected transaction id.
    pub txid: String,
    /// A short, stable reason tag.
    pub reason: String,
}

/// The high-level outcome of a tally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TallyStatus {
    /// A viable board was elected (winners ≥ threshold).
    Elected,
    /// Turnout fell below the required majority of the electorate — void
    /// election, incumbent term auto-extends (ADR 0010 §6.2).
    NoQuorum,
    /// Quorum was met but fewer than `threshold` candidates stood — no viable
    /// multisig can be sealed (ADR 0010 §6.2, candidate-list exhaustion).
    InsufficientCandidates,
}

/// A candidate's standing in the final ranking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateStanding {
    /// 1-based rank (1 = most approvals; ties resolved by ascending nodeId).
    pub rank: usize,
    /// The candidate's curated identity id.
    pub node_id: String,
    /// Distinct valid ballots approving this candidate.
    pub approvals: u32,
}

/// The complete, reproducible result of a tally.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TallyResult {
    /// The election outcome.
    pub status: TallyStatus,
    /// The full ranking of every valid candidate (deterministic order).
    pub ranking: Vec<CandidateStanding>,
    /// The elected board — the top `seats` of `ranking` — empty unless
    /// `status == Elected`.
    pub winners: Vec<CandidateStanding>,
    /// Distinct nodes that cast a valid ballot.
    pub turnout: usize,
    /// The electorate size (quorum denominator).
    pub eligible: usize,
    /// The minimum turnout for quorum (strict majority of the electorate).
    pub quorum_required: usize,
    /// The counted ballot txids, **sorted** — the tally transcript evidence.
    pub counted_ballot_txids: Vec<String>,
    /// Hash of the full transcript (counted ballot txids + derived ranking);
    /// lets a verifier re-check the exact evidence set (schema `resultHash`).
    pub result_hash: String,
    /// Memos that were dropped, with reasons (observability; not hashed).
    pub rejected: Vec<RejectedMemo>,
}

/// Run the deterministic `approval-top-N-v1` tally.
///
/// `ledger` is every memo-convention transaction confirmed up to (and
/// including) `params.close_height`; extra transactions outside the windows
/// are ignored (and recorded in `rejected`), so passing a superset of the
/// ledger is safe. Returns `Err` only for structurally-invalid inputs
/// (bad params or a malformed snapshot); election failure modes are carried by
/// [`TallyStatus`], not errors.
pub fn tally(
    snapshot: &CurationSnapshot,
    params: &ElectionParams,
    ledger: &[MemoTransaction],
) -> Result<TallyResult, String> {
    params.validate()?;
    snapshot.validate()?;

    // Deterministic processing order — the ONLY ordering that matters. Input
    // vector order is irrelevant after this sort.
    let mut txs: Vec<&MemoTransaction> = ledger.iter().collect();
    txs.sort_by(|a, b| a.height.cmp(&b.height).then_with(|| a.txid.cmp(&b.txid)));

    let mut rejected: Vec<RejectedMemo> = Vec::new();
    let reject = |rejected: &mut Vec<RejectedMemo>, txid: &str, reason: &str| {
        rejected.push(RejectedMemo {
            txid: txid.to_string(),
            reason: reason.to_string(),
        });
    };

    // --- Pass 1: nominations (before openHeight) → the candidate set. ---
    // First valid nomination per curated identity wins; duplicates rejected.
    let mut candidates: Vec<String> = Vec::new();
    for tx in &txs {
        let memo = match parse_election_memo(&tx.memo) {
            Ok(m) => m,
            Err(_) => continue, // not our concern here; handled in pass 2 if a ballot
        };
        let (term, node_id) = match &memo {
            ElectionMemo::Nomination { term, node_id } => (*term, node_id.clone()),
            ElectionMemo::Ballot { .. } => continue,
        };
        if term != params.term {
            reject(&mut rejected, &tx.txid, "nomination_wrong_term");
            continue;
        }
        if tx.height >= params.open_height {
            reject(&mut rejected, &tx.txid, "nomination_after_open");
            continue;
        }
        let vk = match snapshot.verifying_key(&node_id) {
            Some(vk) => vk,
            None => {
                reject(&mut rejected, &tx.txid, "nomination_not_curated");
                continue;
            }
        };
        if verify_election_memo(&tx.memo, &tx.signature_hex, &vk).is_err() {
            reject(&mut rejected, &tx.txid, "nomination_bad_signature");
            continue;
        }
        if candidates.contains(&node_id) {
            reject(&mut rejected, &tx.txid, "nomination_duplicate");
            continue;
        }
        candidates.push(node_id);
    }

    // --- Pass 2: ballots (within openHeight..=closeHeight). ---
    // First valid ballot per voter wins; later ballots by the same voter are
    // rejected (per-term binding — no re-vote, ADR 0010 §6.3 Clique hygiene).
    let mut approvals: std::collections::HashMap<String, u32> =
        candidates.iter().map(|c| (c.clone(), 0u32)).collect();
    let mut voted: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut counted_ballot_txids: Vec<String> = Vec::new();

    for tx in &txs {
        let memo = match parse_election_memo(&tx.memo) {
            Ok(m) => m,
            Err(_) => {
                reject(&mut rejected, &tx.txid, "unparseable_memo");
                continue;
            }
        };
        let (term, node_id, ballot_approvals) = match &memo {
            ElectionMemo::Ballot {
                term,
                node_id,
                approvals,
            } => (*term, node_id.clone(), approvals.clone()),
            ElectionMemo::Nomination { .. } => continue, // handled in pass 1
        };
        if term != params.term {
            reject(&mut rejected, &tx.txid, "ballot_wrong_term");
            continue;
        }
        if tx.height < params.open_height || tx.height > params.close_height {
            reject(&mut rejected, &tx.txid, "ballot_outside_window");
            continue;
        }
        let vk = match snapshot.verifying_key(&node_id) {
            Some(vk) => vk,
            None => {
                reject(&mut rejected, &tx.txid, "ballot_not_curated");
                continue;
            }
        };
        if verify_election_memo(&tx.memo, &tx.signature_hex, &vk).is_err() {
            reject(&mut rejected, &tx.txid, "ballot_bad_signature");
            continue;
        }
        if voted.contains(&node_id) {
            reject(&mut rejected, &tx.txid, "ballot_duplicate_voter");
            continue;
        }

        // Count the ballot. De-duplicate approvals and drop any that do not
        // reference a standing candidate (a voter approving a non-candidate is
        // not an error — the approval simply has no seat to land in).
        voted.insert(node_id);
        counted_ballot_txids.push(tx.txid.clone());
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for approved in &ballot_approvals {
            if !seen.insert(approved.as_str()) {
                continue;
            }
            if let Some(count) = approvals.get_mut(approved) {
                *count += 1;
            }
        }
    }

    // --- Rank: approvals desc, then ascending nodeId (deterministic tie-break).
    // ---
    let mut ranking_pairs: Vec<(String, u32)> = candidates
        .iter()
        .map(|c| (c.clone(), *approvals.get(c).unwrap_or(&0)))
        .collect();
    ranking_pairs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let ranking: Vec<CandidateStanding> = ranking_pairs
        .into_iter()
        .enumerate()
        .map(|(i, (node_id, approvals))| CandidateStanding {
            rank: i + 1,
            node_id,
            approvals,
        })
        .collect();

    counted_ballot_txids.sort();
    let result_hash = compute_result_hash(&counted_ballot_txids, &ranking);

    let eligible = snapshot.eligible_count();
    let quorum_required = eligible / 2 + 1;
    let turnout = voted_count(&counted_ballot_txids);

    // --- Classify the outcome. ---
    let (status, winners) = if turnout < quorum_required {
        (TallyStatus::NoQuorum, Vec::new())
    } else {
        let take = params.seats.min(ranking.len());
        let winners: Vec<CandidateStanding> = ranking.iter().take(take).cloned().collect();
        if (winners.len() as u32) < params.threshold {
            (TallyStatus::InsufficientCandidates, Vec::new())
        } else {
            (TallyStatus::Elected, winners)
        }
    };

    Ok(TallyResult {
        status,
        ranking,
        winners,
        turnout,
        eligible,
        quorum_required,
        counted_ballot_txids,
        result_hash,
        rejected,
    })
}

/// Turnout is the number of counted ballots (one per distinct voter, enforced
/// during counting).
fn voted_count(counted_ballot_txids: &[String]) -> usize {
    counted_ballot_txids.len()
}

/// Hash the full tally transcript: the sorted counted ballot txids followed by
/// the derived ranking. Length-prefixed so no field boundary is ambiguous.
fn compute_result_hash(counted_ballot_txids: &[String], ranking: &[CandidateStanding]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(TALLY_TRANSCRIPT_DOMAIN);
    hasher.update((counted_ballot_txids.len() as u64).to_le_bytes());
    for txid in counted_ballot_txids {
        hasher.update((txid.len() as u64).to_le_bytes());
        hasher.update(txid.as_bytes());
    }
    hasher.update((ranking.len() as u64).to_le_bytes());
    for c in ranking {
        hasher.update((c.node_id.len() as u64).to_le_bytes());
        hasher.update(c.node_id.as_bytes());
        hasher.update(c.approvals.to_le_bytes());
    }
    hex::encode(hasher.finalize())
}
