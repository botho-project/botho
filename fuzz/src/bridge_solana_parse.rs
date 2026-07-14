// Copyright (c) 2024 The Botho Foundation

//! Shared harness body for the `bth-bridge-service` mint-path byte parsers
//! (fuzz target `fuzz_bridge_solana_parse`; #897, Tier B of #892).
//!
//! Security rationale: these are the service-side parsers that sit directly
//! on adversarial input paths in the custody-relevant mint pipeline
//! (ADR 0002):
//!
//!   * [`parse_bridge_mint`] / [`parse_bridge_mint_authority`] slice pubkeys
//!     out of raw Solana `Bridge` account bytes fetched over RPC — a malicious
//!     or buggy RPC endpoint fully controls this input. A panic is a remote DoS
//!     of the bridge node; a wrong slice is worse (it feeds the ADR-0002
//!     mint-authority custody guard).
//!   * [`check_attested_nonce`] is the pre-broadcast Gnosis Safe nonce
//!     cross-check (#848) between collected federation signatures and the live
//!     chain — the last gate before a doomed/replayable `execTransaction`
//!     broadcast.
//!   * [`reject_duplicate_signers`] is the federation-construction dedup check
//!     (#848): missing a duplicate silently yields an unsatisfiable t-of-n
//!     federation (wedged orders); a false positive bricks a valid config at
//!     startup.
//!
//! One target covers the whole mint-path parser family via an `Arbitrary`
//! mode enum (the grouping sanctioned by #897: the status-line parser gets
//! its own target because its input shape — a peer HTTP response — is
//! unrelated). Each arm asserts *semantic* postconditions against a
//! reference model, not just panic-freedom:
//!
//!   * Solana parsers: `Ok` exactly when the account data covers `offset + 32`
//!     bytes, and the returned pubkey is byte-identical to the
//!     `data[offset..offset + 32]` window; truncated / boundary / oversized
//!     inputs never panic and errors are always `MintError::Rpc`.
//!   * `check_attested_nonce`: `Ok` exactly when no nonce is attested (Solana /
//!     legacy) or the attested nonce equals the on-chain nonce; every failure
//!     is the retryable `MintError::StaleNonce`.
//!   * `reject_duplicate_signers`: `Err` exactly when the identity list
//!     contains a duplicate (reference: set cardinality), independent of
//!     ordering.
//!
//! Issue refs: #897 (this target + the service lib split), #892 (Tier A/B
//! fuzz plan), #848 / #850 (nonce pinning + Solana account layout), #920
//! (shared-harness extraction so the native-smoke driver runs the exact same
//! body on macOS).

use std::collections::BTreeSet;

use arbitrary::Arbitrary;

use bth_bridge_core::{AttestationSignature, MintAuthorization, SignatureScheme};
use bth_bridge_service::{
    check_attested_nonce, parse_bridge_mint, parse_bridge_mint_authority, reject_duplicate_signers,
    MintError, BRIDGE_MINT_AUTHORITY_OFFSET, BRIDGE_MINT_OFFSET, U256,
};

// ============================================================================
// Structured Fuzzing Types
// ============================================================================

/// One parser-family exercise per fuzz input.
#[derive(Debug, Arbitrary)]
pub enum Mode {
    /// Raw Solana `Bridge` account bytes -> both offset slicers.
    SolanaAccount { data: Vec<u8> },
    /// A `MintAuthorization` nonce vs an on-chain Safe nonce.
    AttestedNonce {
        order_id: [u8; 32],
        threshold: u32,
        attested_nonce: Option<u64>,
        on_chain: OnChainNonce,
        is_eth: bool,
    },
    /// A federation signer-identity list -> duplicate rejection.
    DuplicateSigners { identities: Vec<Vec<u8>> },
}

/// How the on-chain Safe nonce relates to the attested one.
#[derive(Debug, Arbitrary)]
pub enum OnChainNonce {
    /// Byte-equal to the attested nonce (must pass when one is attested).
    MatchingAttested,
    /// An arbitrary independent value (may or may not collide).
    Arbitrary(u128),
}

// ============================================================================
// Entry point (shared by the libFuzzer target and the native-smoke driver)
// ============================================================================

/// Decode a [`Mode`] from raw bytes the same way libFuzzer would and run the
/// harness. Bytes that cannot form a `Mode` are skipped.
pub fn run_from_bytes(data: &[u8]) {
    if let Some(mode) = crate::decode_take_rest::<Mode>(data) {
        run(mode);
    }
}

/// Run the harness against an already-decoded input.
pub fn run(mode: Mode) {
    match mode {
        Mode::SolanaAccount { data } => exercise_solana_parsers(&data),
        Mode::AttestedNonce {
            order_id,
            threshold,
            attested_nonce,
            on_chain,
            is_eth,
        } => exercise_attested_nonce(order_id, threshold, attested_nonce, on_chain, is_eth),
        Mode::DuplicateSigners { identities } => exercise_duplicate_signers(&identities),
    }
}

/// Both `Bridge`-account offset slicers against one arbitrary byte blob:
/// exact `Ok`/`Err` boundary at `offset + 32`, and the `Ok` pubkey must be
/// the identity slice of the account window (never shifted, never
/// truncated-and-padded).
fn exercise_solana_parsers(data: &[u8]) {
    for (offset, result) in [
        (BRIDGE_MINT_OFFSET, parse_bridge_mint(data)),
        (
            BRIDGE_MINT_AUTHORITY_OFFSET,
            parse_bridge_mint_authority(data),
        ),
    ] {
        let end = offset + 32;
        match result {
            Ok(pubkey) => {
                assert!(
                    data.len() >= end,
                    "parser returned Ok on a {}-byte account that cannot hold [{}, {})",
                    data.len(),
                    offset,
                    end
                );
                assert_eq!(
                    &pubkey.0[..],
                    &data[offset..end],
                    "parsed pubkey is not the exact account byte window"
                );
            }
            Err(e) => {
                assert!(
                    data.len() < end,
                    "parser returned Err on a {}-byte account that covers [{}, {})",
                    data.len(),
                    offset,
                    end
                );
                assert!(
                    matches!(e, MintError::Rpc(_)),
                    "truncated account data must surface as a retryable Rpc error"
                );
            }
        }
    }
}

/// `check_attested_nonce` agrees with the #848 contract: no attested nonce
/// always passes; an attested nonce passes iff it equals the on-chain nonce;
/// every mismatch is the retryable `StaleNonce` (never a panic, never a
/// silent pass).
fn exercise_attested_nonce(
    order_id: [u8; 32],
    threshold: u32,
    attested_nonce: Option<u64>,
    on_chain: OnChainNonce,
    is_eth: bool,
) {
    let on_chain_nonce = match on_chain {
        OnChainNonce::MatchingAttested => U256::from(attested_nonce.unwrap_or(0)),
        OnChainNonce::Arbitrary(n) => U256::from(n),
    };
    let auth = MintAuthorization {
        order_id,
        scheme: if is_eth {
            SignatureScheme::Secp256k1
        } else {
            SignatureScheme::Ed25519
        },
        threshold,
        signatures: vec![AttestationSignature {
            signer: order_id[..4].to_vec(),
            signature: vec![0u8; 65],
        }],
        safe_nonce: attested_nonce,
    };

    let expected_ok = match attested_nonce {
        None => true,
        Some(n) => U256::from(n) == on_chain_nonce,
    };
    match check_attested_nonce(&auth, on_chain_nonce) {
        Ok(()) => assert!(
            expected_ok,
            "nonce mismatch (attested {:?} vs on-chain {}) passed the pre-broadcast check",
            attested_nonce, on_chain_nonce
        ),
        Err(e) => {
            assert!(
                !expected_ok,
                "matching/absent nonce (attested {:?} vs on-chain {}) was rejected",
                attested_nonce, on_chain_nonce
            );
            assert!(
                matches!(e, MintError::StaleNonce(_)),
                "a nonce mismatch must be the retryable StaleNonce error"
            );
        }
    }
}

/// `reject_duplicate_signers` agrees with a set-cardinality reference model
/// and never panics on arbitrary identity lists (empty, huge, colliding).
fn exercise_duplicate_signers(identities: &[Vec<u8>]) {
    let distinct: BTreeSet<&[u8]> = identities.iter().map(|i| i.as_slice()).collect();
    let has_duplicate = distinct.len() < identities.len();
    let result = reject_duplicate_signers(identities, "fuzz.federation");
    assert_eq!(
        result.is_err(),
        has_duplicate,
        "duplicate-signer verdict disagrees with the set-cardinality reference \
         ({} identities, {} distinct)",
        identities.len(),
        distinct.len()
    );
}
