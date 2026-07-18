// Copyright (c) 2024 The Botho Foundation

//! Bridge-multisig elections: on-chain ballots + deterministic tally
//! (ADR 0010 option C / sub-variant A1).
//!
//! The mechanism, end to end:
//!
//! - [`memo`] — the two memo-convention formats (self-nomination and ballot)
//!   that ride inside ordinary Botho transaction memos, plus their
//!   domain-separated signing/verification.
//! - [`snapshot`] — the pinned P4.4 curated-node electorate (#709) the tally
//!   binds to, one node one vote.
//! - [`tally`] — the deterministic `approval-top-N-v1` pure function:
//!   reproducible from a ledger replay, no wall-clock, no randomness, with a
//!   deterministic (ascending-`nodeId`) tie-break per ADR 0010 §6.3.
//! - [`term_doc`] — assembly of the `elected`-status v2 term document
//!   (membership only) that feeds the seal / keygen step (#1066).

pub mod memo;
pub mod snapshot;
pub mod tally;
pub mod term_doc;

#[cfg(test)]
mod tests;

pub use memo::{
    canonical_ballot_memo, canonical_nomination_memo, election_signed_message, parse_election_memo,
    sign_election_memo_ed25519, verify_election_memo, verifying_key_from_hex, ElectionMemo,
    ELECTION_MEMO_DOMAIN, ELECTION_MEMO_VERSION, MEMO_KIND_BALLOT, MEMO_KIND_NOMINATE,
};
pub use snapshot::{CuratedNode, CurationSnapshot};
pub use tally::{
    tally, CandidateStanding, ElectionKind, ElectionParams, MemoTransaction, RejectedMemo,
    TallyResult, TallyStatus, Validity, TALLY_TRANSCRIPT_DOMAIN,
};
pub use term_doc::{
    assemble_elected_term_doc, ElectedTermDoc, Electorate, Member, TallySummary, TermStatus,
    ValidityDoc, TALLY_RULE, TERM_DOC_VERSION,
};
