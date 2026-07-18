// Copyright (c) 2024 The Botho Foundation

//! Core types and logic for the BTH bridge.
//!
//! This crate provides the domain types for bridging BTH to wrapped tokens
//! on Ethereum and Solana, including:
//!
//! - Bridge orders and their state machine
//! - Chain-specific types
//! - Configuration structures
//! - Rate limiting logic

pub mod attestation;
pub mod chains;
pub mod config;
pub mod election;
#[cfg(test)]
mod happy_path_tests;
pub mod nonce;
pub mod order;

/// Adversarial / cross-domain attestation tests (bridge epic #816, Phase 3).
#[cfg(test)]
mod adversarial_tests;

pub use attestation::{
    attestation_domain, attestation_signed_message, canonical_attestation_envelope,
    check_attestation_freshness, check_order_binding, mint_payload_digest,
    parse_attestation_envelope, peek_order_id, peek_signer_key_id, peek_target_chain,
    release_payload_digest, sign_attestation_ed25519, AttestationEnvelope, AttestationKind,
    AttestationOutcome, AttestationRejectReason, AttestationSet, AttestationSignature,
    InsertOutcome, MintAuthorization, ParsedAttestation, ReleaseAuthorization, SignatureScheme,
    ATTESTATION_ENVELOPE_VERSION, ATTEST_DOMAIN_BTH, ATTEST_DOMAIN_ETH, ATTEST_DOMAIN_SOL,
    MAX_ATTESTATION_LIFETIME_SECS, RELEASE_ATTESTATION_DOMAIN_TAG,
};
pub use chains::{Chain, ChainAddress};
pub use config::{
    BridgeConfig, BthConfig, EthereumConfig, FederationSettings, GasPriceStrategy,
    PublicApiSettings, ReserveSettings, SolanaCommitment, SolanaConfig,
};
pub use election::{
    assemble_elected_term_doc, canonical_ballot_memo, canonical_nomination_memo,
    parse_election_memo, sign_election_memo_ed25519, tally, verify_election_memo,
    CandidateStanding, CuratedNode, CurationSnapshot, ElectedTermDoc, ElectionKind, ElectionMemo,
    ElectionParams, MemoTransaction, TallyResult, TallyStatus, Validity,
};
pub use nonce::{NonceStore, ReserveOutcome};
pub use order::{derive_order_id, BridgeOrder, OrderStatus, OrderType};
