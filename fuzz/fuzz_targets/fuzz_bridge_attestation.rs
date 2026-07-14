#![no_main]

//! Fuzzing target for the bridge federation [`AttestationSet`] threshold logic.
//!
//! Security rationale: `AttestationSet` is the funds-path chokepoint that
//! decides whether a t-of-n federation has authorized a wBTH mint / BTH
//! release. It ingests VERIFIED-but-adversarial per-signer attestations and
//! must answer the threshold question without ever being tricked into
//! double-counting a signer, mixing Gnosis Safe nonces, or spamming the
//! equivocation alarm. A single logic slip here is custody-relevant: it could
//! let `threshold - 1` colluding signers mint by replaying/equivocating, or
//! wedge liveness by rejecting honest signatures.
//!
//! This target drives `Arbitrary`-generated sequences of `insert` /
//! `insert_classified` / `flag_equivocation` operations against one set keyed
//! to a fixed `(order, action, chain, safe_nonce)`, plus a
//! `MintAuthorization` serde round-trip, and asserts the semantic invariants
//! established by #848 / #859:
//!
//!   * `distinct_signers()` never exceeds the number of distinct signer
//!     identities that were accepted into the set;
//!   * `is_threshold_met(t)` is monotonic (never true below `t` distinct
//!     signers, never flips false once met) and consistent with
//!     `distinct_signers()`;
//!   * the set never mixes Safe nonces — `safe_nonce()` stays pinned to the
//!     value the set was created with, and a conflicting-nonce insert is
//!     rejected (`Err`) rather than silently repinning;
//!   * `flag_equivocation` fires at most once per signer identity;
//!   * an already-counted signer never increments `distinct_signers()`, whether
//!     the re-send is `DuplicateBenign` or `Equivocation`;
//!   * no operation sequence panics.
//!
//! Issue refs: #892 (this target), #848 / #859 (Safe-nonce pinning +
//! equivocation dedup invariants), #874 / #879 (bridge hardening sweep).

use std::collections::BTreeSet;

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;

use bth_bridge_core::{
    attestation::{
        AttestationKind, AttestationSet, AttestationSignature, InsertOutcome, MintAuthorization,
        ParsedAttestation, SignatureScheme,
    },
    Chain,
};
use uuid::Uuid;

// ============================================================================
// Structured Fuzzing Types
// ============================================================================

/// One of the three supported destination chains for a mint attestation.
/// Solana / BTH mints are blockhash-bound (no Safe nonce); Ethereum mints
/// carry a Safe nonce that the set must pin.
#[derive(Debug, Clone, Copy, Arbitrary)]
enum FuzzChain {
    Ethereum,
    Solana,
}

impl FuzzChain {
    fn to_chain(self) -> Chain {
        match self {
            FuzzChain::Ethereum => Chain::Ethereum,
            FuzzChain::Solana => Chain::Solana,
        }
    }
}

/// A single operation applied to the set under test.
#[derive(Debug, Arbitrary)]
enum Op {
    /// Insert a payload signature via the classifying entry point. `signer`
    /// selects one of a small pool of signer identities (so collisions /
    /// re-sends actually happen); `sig_byte` selects the payload signature
    /// bytes (equal bytes => benign re-send, differing => equivocation);
    /// `nonce_delta` optionally perturbs the Safe nonce to exercise the
    /// mismatch-rejection path.
    InsertClassified {
        signer: u8,
        sig_byte: u8,
        nonce_delta: NonceChoice,
    },
    /// Insert via the funds-path `insert` (collapses the two already-counted
    /// cases into `Ok(false)`).
    Insert {
        signer: u8,
        sig_byte: u8,
        nonce_delta: NonceChoice,
    },
    /// Directly flag a signer as an equivocator (detection bookkeeping).
    FlagEquivocation { signer: u8 },
    /// Query the threshold at an arbitrary `t` (exercises the monotonicity
    /// and consistency assertions).
    CheckThreshold { threshold: u32 },
}

/// How an operation's Safe nonce relates to the set's pinned nonce.
#[derive(Debug, Clone, Copy, Arbitrary)]
enum NonceChoice {
    /// Use the set's pinned nonce (matches — will be accepted).
    Matching,
    /// Use a deliberately different nonce (mismatch — must be rejected).
    Mismatched,
}

/// The full fuzz input: how to seed the set, then a sequence of operations.
#[derive(Debug, Arbitrary)]
struct FuzzInput {
    /// Order UUID bytes (fixed for the whole set).
    order_bytes: [u8; 16],
    /// Destination chain for the mint attestation.
    chain: FuzzChain,
    /// The Safe nonce the set is pinned to (Ethereum only; Solana => None).
    seed_nonce: u64,
    /// Operations to apply.
    ops: Vec<Op>,
    /// A `MintAuthorization` to round-trip through serde.
    auth: FuzzAuth,
}

/// Inputs for a `MintAuthorization` construction + serde round-trip.
#[derive(Debug, Arbitrary)]
struct FuzzAuth {
    order_id: [u8; 32],
    threshold: u32,
    signers: Vec<[u8; 4]>,
    safe_nonce: Option<u64>,
    is_eth: bool,
}

// ============================================================================
// Helpers
// ============================================================================

/// Build a canonical `ParsedAttestation` for the given signer identity /
/// destination chain / Safe nonce, bound to `order`. All other fields are
/// held constant so the only variation the set sees is signer identity and
/// (optionally) the Safe nonce.
fn make_parsed(
    order: Uuid,
    chain: Chain,
    signer_key_id: String,
    safe_nonce: Option<u64>,
) -> ParsedAttestation {
    ParsedAttestation {
        action: AttestationKind::MintWbth {
            dest_chain: chain,
            dest_address: "0xrecipient".to_string(),
            amount: 1_000,
            order_id: order,
            source_tx: "bth-source-tx".to_string(),
            safe_nonce,
        },
        issued_at: 1_000,
        expires_at: 2_000,
        nonce: "fuzz-nonce".to_string(),
        signer_key_id,
        v: 1,
    }
}

/// Stable signer identity for a pool index (16 distinct identities).
fn signer_id(idx: u8) -> String {
    format!("signer-{:02x}", idx % 16)
}

// ============================================================================
// Fuzz Target
// ============================================================================

fuzz_target!(|input: FuzzInput| {
    exercise_attestation_set(&input);
    exercise_mint_authorization(&input.auth);
});

fn exercise_attestation_set(input: &FuzzInput) {
    let order = Uuid::from_bytes(input.order_bytes);
    let chain = input.chain.to_chain();

    // Ethereum mints pin a Safe nonce; Solana mints are nonce-free.
    let pinned_nonce: Option<u64> = match chain {
        Chain::Ethereum => Some(input.seed_nonce),
        _ => None,
    };
    // A nonce guaranteed to differ from the pinned one (for the mismatch path).
    let other_nonce: Option<u64> = pinned_nonce.map(|n| n.wrapping_add(1));

    // Seed the set from a canonical attestation.
    let seed = make_parsed(order, chain, signer_id(0), pinned_nonce);
    let mut set = AttestationSet::for_attestation(&seed);

    // The set MUST start empty and pinned to the seed nonce.
    assert_eq!(set.distinct_signers(), 0, "fresh set has no signers");
    assert_eq!(
        set.safe_nonce(),
        pinned_nonce,
        "safe_nonce pinned at creation"
    );
    assert!(
        !set.is_threshold_met(1),
        "empty set never meets threshold 1"
    );

    // Reference model of accepted distinct signers and flagged equivocators.
    let mut accepted_signers: BTreeSet<String> = BTreeSet::new();
    let mut flagged: BTreeSet<String> = BTreeSet::new();
    // Highest threshold observed as met, to check monotonicity (once
    // `distinct_signers >= t`, it can never drop below `t` because inserts
    // never remove signers).
    let mut prev_distinct: u32 = 0;

    for op in &input.ops {
        match op {
            Op::InsertClassified {
                signer,
                sig_byte,
                nonce_delta,
            } => {
                let id = signer_id(*signer);
                let nonce = pick_nonce(pinned_nonce, other_nonce, *nonce_delta);
                let parsed = make_parsed(order, chain, id.clone(), nonce);
                let sig = AttestationSignature {
                    signer: vec![*signer],
                    signature: vec![*sig_byte],
                };
                let before = set.distinct_signers();
                match set.insert_classified(&parsed, sig) {
                    Ok(outcome) => {
                        // A matching insert. Update the reference model.
                        apply_outcome(
                            outcome,
                            &id,
                            &mut set,
                            &mut accepted_signers,
                            &mut flagged,
                            before,
                        );
                    }
                    Err(_) => {
                        // Rejected — must be a nonce mismatch (Ethereum only).
                        // Nothing counted; the set is unchanged.
                        assert_eq!(
                            set.distinct_signers(),
                            before,
                            "a rejected insert must not change the signer count"
                        );
                        assert!(
                            matches!(nonce_delta, NonceChoice::Mismatched)
                                && chain == Chain::Ethereum,
                            "insert_classified only rejects on a Safe-nonce mismatch"
                        );
                    }
                }
            }
            Op::Insert {
                signer,
                sig_byte,
                nonce_delta,
            } => {
                let id = signer_id(*signer);
                let nonce = pick_nonce(pinned_nonce, other_nonce, *nonce_delta);
                let parsed = make_parsed(order, chain, id.clone(), nonce);
                let sig = AttestationSignature {
                    signer: vec![*signer],
                    signature: vec![*sig_byte],
                };
                let before = set.distinct_signers();
                let was_known = accepted_signers.contains(&id);
                match set.insert(&parsed, sig) {
                    Ok(true) => {
                        // New distinct signer.
                        assert!(!was_known, "insert returned NewSigner for a known signer");
                        accepted_signers.insert(id);
                        assert_eq!(
                            set.distinct_signers(),
                            before + 1,
                            "a new signer increments the count by exactly one"
                        );
                    }
                    Ok(false) => {
                        // Already counted OR a re-send. Count unchanged.
                        assert!(
                            was_known,
                            "insert returned Ok(false) for a signer never accepted"
                        );
                        assert_eq!(
                            set.distinct_signers(),
                            before,
                            "an already-counted signer never inflates the count"
                        );
                    }
                    Err(_) => {
                        assert_eq!(
                            set.distinct_signers(),
                            before,
                            "a rejected insert must not change the signer count"
                        );
                    }
                }
            }
            Op::FlagEquivocation { signer } => {
                let id = signer_id(*signer);
                let before = set.distinct_signers();
                let fired = set.flag_equivocation(&id);
                // flag_equivocation is detection-only: it never touches counting.
                assert_eq!(
                    set.distinct_signers(),
                    before,
                    "flag_equivocation must never change the signer count"
                );
                // At-most-once-per-signer: a `true` return may only happen the
                // FIRST time this identity is ever flagged (whether the prior
                // flag came from an explicit call here or from an internal
                // equivocation during insert/insert_classified). Once flagged,
                // it must never fire `true` again.
                if fired {
                    assert!(
                        flagged.insert(id),
                        "flag_equivocation fired true twice for one signer identity"
                    );
                }
            }
            Op::CheckThreshold { threshold } => {
                check_threshold_invariants(&set, *threshold);
            }
        }

        // --- Invariants that must hold after EVERY operation ---

        // 1. distinct_signers equals (and thus never exceeds) the reference set of
        //    distinct accepted signer identities.
        assert_eq!(
            set.distinct_signers() as usize,
            accepted_signers.len(),
            "distinct_signers must equal the reference count of accepted signers"
        );

        // 2. safe_nonce stays pinned forever.
        assert_eq!(
            set.safe_nonce(),
            pinned_nonce,
            "safe_nonce drifted from its pinned value"
        );

        // 3. distinct_signers is monotonic non-decreasing (no op removes a signer).
        assert!(
            set.distinct_signers() >= prev_distinct,
            "distinct_signers decreased — a signer was dropped"
        );
        prev_distinct = set.distinct_signers();
    }

    // Final threshold consistency sweep across a range of t values.
    for t in 0..=(set.distinct_signers() + 2) {
        check_threshold_invariants(&set, t);
    }
}

/// Update the reference model for a classified insert and assert its
/// threshold-neutral / count semantics.
fn apply_outcome(
    outcome: InsertOutcome,
    id: &str,
    set: &AttestationSet,
    accepted: &mut BTreeSet<String>,
    flagged: &mut BTreeSet<String>,
    before: u32,
) {
    match outcome {
        InsertOutcome::NewSigner => {
            assert!(
                !accepted.contains(id),
                "NewSigner reported for an already-accepted signer"
            );
            accepted.insert(id.to_string());
            assert_eq!(
                set.distinct_signers(),
                before + 1,
                "NewSigner must increment the distinct count by one"
            );
        }
        InsertOutcome::DuplicateBenign => {
            assert!(
                accepted.contains(id),
                "DuplicateBenign for a signer that was never accepted"
            );
            assert_eq!(
                set.distinct_signers(),
                before,
                "DuplicateBenign must not change the count"
            );
        }
        InsertOutcome::Equivocation => {
            assert!(
                accepted.contains(id),
                "Equivocation for a signer that was never accepted"
            );
            assert_eq!(
                set.distinct_signers(),
                before,
                "Equivocation must not inflate the count (signer still counts once)"
            );
            // Equivocation is reported at most once per signer: the classified
            // insert flags the identity, so a subsequent `flag_equivocation`
            // for the same id must return false.
            flagged.insert(id.to_string());
        }
    }
}

/// Threshold invariants that must hold for any `t`.
fn check_threshold_invariants(set: &AttestationSet, threshold: u32) {
    let met = set.is_threshold_met(threshold);
    let n = set.distinct_signers();
    if threshold == 0 {
        // A zero threshold NEVER authorizes.
        assert!(
            !met,
            "is_threshold_met(0) must be false — zero never authorizes"
        );
    } else if n >= threshold {
        assert!(
            met,
            "threshold {} should be met with {} distinct signers",
            threshold, n
        );
    } else {
        assert!(
            !met,
            "threshold {} must NOT be met with only {} distinct signers",
            threshold, n
        );
    }
}

/// Pick the Safe nonce for an operation.
fn pick_nonce(pinned: Option<u64>, other: Option<u64>, choice: NonceChoice) -> Option<u64> {
    match choice {
        NonceChoice::Matching => pinned,
        NonceChoice::Mismatched => other,
    }
}

/// Construct a `MintAuthorization` from arbitrary bytes and round-trip it
/// through JSON serde. Asserts the pre-`safe_nonce` compatibility invariant:
/// a payload with no `safe_nonce` key decodes to `safe_nonce: None`.
fn exercise_mint_authorization(a: &FuzzAuth) {
    let scheme = if a.is_eth {
        SignatureScheme::Secp256k1
    } else {
        SignatureScheme::Ed25519
    };
    let signatures: Vec<AttestationSignature> = a
        .signers
        .iter()
        .take(32)
        .map(|s| AttestationSignature {
            signer: s.to_vec(),
            signature: vec![0u8; 65],
        })
        .collect();

    let auth = MintAuthorization {
        order_id: a.order_id,
        scheme,
        threshold: a.threshold,
        signatures,
        safe_nonce: a.safe_nonce,
    };

    // meets_threshold must never panic and must agree with a reference count
    // of distinct signer identities. Note: meets_threshold uses a bare
    // `>= self.threshold` with NO zero guard (unlike AttestationSet::
    // is_threshold_met), so a threshold of 0 is trivially met — mirror that.
    let distinct: BTreeSet<&[u8]> = auth
        .signatures
        .iter()
        .map(|s| s.signer.as_slice())
        .collect();
    let expected = distinct.len() as u32 >= a.threshold;
    assert_eq!(
        auth.meets_threshold(),
        expected,
        "meets_threshold disagrees with distinct-signer reference count"
    );

    // JSON round-trip: encode then decode; the decoded value must equal the
    // original (serde stability of the funds-path payload).
    if let Ok(json) = serde_json::to_string(&auth) {
        if let Ok(decoded) = serde_json::from_str::<MintAuthorization>(&json) {
            assert_eq!(
                decoded, auth,
                "MintAuthorization serde round-trip changed the value"
            );
        }
    }

    // Pre-safe_nonce compatibility: a payload missing the `safe_nonce` key
    // must decode to `None` (the field is `#[serde(default)]`).
    let legacy = r#"{"order_id":"0000000000000000000000000000000000000000000000000000000000000000","scheme":"secp256k1","threshold":2,"signatures":[]}"#;
    if let Ok(decoded) = serde_json::from_str::<MintAuthorization>(legacy) {
        assert_eq!(
            decoded.safe_nonce, None,
            "a pre-safe_nonce payload must deserialize to safe_nonce: None"
        );
    }
}
