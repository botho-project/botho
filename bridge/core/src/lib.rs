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
pub mod nonce;
pub mod order;

pub use attestation::{
    attestation_domain, attestation_signed_message, canonical_attestation_envelope,
    check_attestation_freshness, check_order_binding, mint_payload_digest,
    parse_attestation_envelope, peek_signer_key_id, peek_target_chain, release_payload_digest,
    sign_attestation_ed25519, AttestationEnvelope, AttestationKind, AttestationOutcome,
    AttestationRejectReason, AttestationSet, AttestationSignature, MintAuthorization,
    ParsedAttestation, ReleaseAuthorization, SignatureScheme, ATTESTATION_ENVELOPE_VERSION,
    ATTEST_DOMAIN_BTH, ATTEST_DOMAIN_ETH, ATTEST_DOMAIN_SOL, MAX_ATTESTATION_LIFETIME_SECS,
    RELEASE_ATTESTATION_DOMAIN_TAG,
};
pub use chains::{Chain, ChainAddress};
pub use config::{
    BridgeConfig, BthConfig, EthereumConfig, GasPriceStrategy, ReserveSettings, SolanaCommitment,
    SolanaConfig,
};
pub use nonce::{NonceStore, ReserveOutcome};
pub use order::{derive_order_id, BridgeOrder, OrderStatus, OrderType};
