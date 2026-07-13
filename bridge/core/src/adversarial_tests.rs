// Copyright (c) 2024 The Botho Foundation

//! Adversarial / cross-domain attestation tests (bridge epic #816, Phase 3,
//! issue #829).
//!
//! These tests actively try to break the attestation authorization layer.
//! They complement the per-primitive negative tests already living in
//! `attestation.rs` (replay, tamper, unknown signer, threshold, staleness —
//! #847) by attacking the *domain-separation* and *aggregation* seams:
//!
//! - **Cross-domain signature confusion**: a signature produced for the
//!   operator-signed-action machinery (`botho/src/operator_action.rs`), for a
//!   wallet, or for a different bridge chain domain must NEVER verify as a
//!   bridge attestation. This is the vector the issue calls out explicitly:
//!   "verify domain tags actually differ from the operator_action domain".
//! - **Equivocation / double-signing**: a single federation member cannot
//!   inflate the distinct-signer count toward the threshold, and the
//!   aggregation primitive is byte-stable about which of a signer's submissions
//!   it keeps.
//!
//! Every domain tag in the bridge protocol is asserted pairwise-distinct and
//! distinct from the operator-action domain, so cross-domain confusion is
//! impossible by construction, not just by test.

use ed25519_dalek::{Signer as _, SigningKey};

use crate::{
    attestation::{
        attestation_signed_message, canonical_attestation_envelope, release_payload_digest,
        sign_attestation_ed25519, AttestationEnvelope, AttestationKind, AttestationRejectReason,
        AttestationSet, AttestationSignature, ATTEST_DOMAIN_BTH, ATTEST_DOMAIN_ETH,
        ATTEST_DOMAIN_SOL, MINT_ATTESTATION_DOMAIN_TAG_ETH, MINT_ATTESTATION_DOMAIN_TAG_SOL,
        RELEASE_ATTESTATION_DOMAIN_TAG,
    },
    chains::Chain,
    order::BridgeOrder,
};

/// The domain separator the operator-signed-action machinery signs under
/// (`botho/src/operator_action.rs` `DOMAIN_SEPARATOR`). Mirrored here as a
/// literal — `bth-bridge-core` deliberately does NOT depend on the `botho`
/// node crate — with an explicit assertion below that it differs from every
/// bridge tag. If the node ever changes its separator, that is fine; the
/// security property is only that the two families never COLLIDE.
const OPERATOR_ACTION_DOMAIN_SEPARATOR: &[u8] = b"botho-operator-action-v1";

fn signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn now() -> u64 {
    1_700_000_000
}

/// A confirmed burn order on Ethereum whose release pays out on BTH.
fn burn_order() -> BridgeOrder {
    BridgeOrder::new_burn(
        Chain::Ethereum,
        1_000_000_000_000,
        1_000_000_000,
        "0x1234567890abcdef1234567890abcdef12345678".to_string(),
        "bth_user_stealth_addr".to_string(),
        "0xburntx".to_string(),
    )
}

fn release_kind(order: &BridgeOrder) -> AttestationKind {
    AttestationKind::ReleaseBth {
        source_chain: order.source_chain,
        bth_address: order.dest_address.clone(),
        amount: order.net_amount(),
        order_id: order.id,
        source_tx: order.source_tx.clone().unwrap(),
    }
}

// ---------------------------------------------------------------------------
// Domain distinctness — cross-domain confusion is impossible by construction
// ---------------------------------------------------------------------------

#[test]
fn all_attestation_domain_tags_are_pairwise_distinct_and_differ_from_operator_action() {
    // Every domain-separation tag the bridge signs under, plus the node's
    // operator-action separator. If ANY two collide, a signature over one
    // payload family could be replayed as another — the exact cross-domain
    // confusion this suite exists to rule out.
    let tags: &[(&str, &[u8])] = &[
        ("attest-eth", ATTEST_DOMAIN_ETH),
        ("attest-sol", ATTEST_DOMAIN_SOL),
        ("attest-bth", ATTEST_DOMAIN_BTH),
        ("release-payload", RELEASE_ATTESTATION_DOMAIN_TAG),
        ("mint-payload-eth", MINT_ATTESTATION_DOMAIN_TAG_ETH),
        ("mint-payload-sol", MINT_ATTESTATION_DOMAIN_TAG_SOL),
        ("operator-action", OPERATOR_ACTION_DOMAIN_SEPARATOR),
    ];

    for (i, (name_a, a)) in tags.iter().enumerate() {
        for (name_b, b) in tags.iter().skip(i + 1) {
            assert_ne!(
                a, b,
                "domain tags `{name_a}` and `{name_b}` collide — cross-domain \
                 signature confusion would be possible"
            );
            // Neither may be a prefix of the other: `verify(domain || bytes)`
            // must not let a longer domain's suffix masquerade as payload.
            assert!(
                !a.starts_with(b) && !b.starts_with(a),
                "domain tag `{name_a}` is a prefix of `{name_b}` (or vice \
                 versa) — the domain boundary is ambiguous"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Cross-domain signature confusion (#829 gap test a)
// ---------------------------------------------------------------------------

#[test]
fn operator_action_domain_signature_reused_as_bridge_attestation_is_rejected() {
    // A federation validator's Ed25519 node key ALSO signs operator actions
    // (`botho-operator-action-v1 || envelope`). If an attacker captures such
    // a signature and wraps the same bytes as a bridge attestation, the
    // bridge verifier — which signs `attestation_domain(chain) || bytes` —
    // must reject it: the signed messages differ in their domain prefix.
    let sk = signing_key(1);
    let order = burn_order();
    let kind = release_kind(&order); // target chain = Bth
    let signer_key_id = hex::encode(sk.verifying_key().as_bytes());
    let envelope = canonical_attestation_envelope(&kind, &signer_key_id, "n1", now(), now() + 120);

    // Sign the SAME envelope bytes, but under the operator-action domain.
    let mut operator_msg = OPERATOR_ACTION_DOMAIN_SEPARATOR.to_vec();
    operator_msg.extend_from_slice(envelope.as_bytes());
    let payload_digest = kind.ed25519_payload_digest().unwrap();

    let forged = AttestationEnvelope {
        envelope,
        signature_hex: hex::encode(sk.sign(&operator_msg).to_bytes()),
        payload_signature_hex: hex::encode(sk.sign(&payload_digest).to_bytes()),
    };

    assert_eq!(
        forged.verify_and_parse_ed25519(&sk.verifying_key()),
        Err(AttestationRejectReason::BadSignature),
        "an operator-action-domain signature must not authorize a bridge release"
    );
}

#[test]
fn wallet_style_raw_signature_reused_as_bridge_attestation_is_rejected() {
    // A validator key might also sign wallet/transaction payloads. A raw
    // signature over arbitrary bytes (no bridge domain prefix at all) must
    // never verify as an attestation envelope signature.
    let sk = signing_key(2);
    let order = burn_order();
    let kind = release_kind(&order);
    let signer_key_id = hex::encode(sk.verifying_key().as_bytes());
    let envelope = canonical_attestation_envelope(&kind, &signer_key_id, "n2", now(), now() + 120);

    // Sign the envelope bytes DIRECTLY (as a naive wallet would sign a blob),
    // omitting the domain separator entirely.
    let payload_digest = kind.ed25519_payload_digest().unwrap();
    let forged = AttestationEnvelope {
        signature_hex: hex::encode(sk.sign(envelope.as_bytes()).to_bytes()),
        payload_signature_hex: hex::encode(sk.sign(&payload_digest).to_bytes()),
        envelope,
    };

    assert_eq!(
        forged.verify_and_parse_ed25519(&sk.verifying_key()),
        Err(AttestationRejectReason::BadSignature),
        "a domainless (wallet-style) signature must not authorize a bridge release"
    );
}

#[test]
fn release_payload_signature_reused_as_the_envelope_signature_is_rejected() {
    // The two detached signatures cover DIFFERENT things (the envelope
    // signature covers `domain || envelope_bytes`; the payload signature
    // covers `release_payload_digest`). Swapping the payload signature into
    // the envelope-signature slot — a within-protocol cross-domain reuse —
    // must fail.
    let sk = signing_key(3);
    let order = burn_order();
    let kind = release_kind(&order);
    let good = sign_attestation_ed25519(&kind, &sk, "n3", now(), now() + 120).unwrap();

    let swapped = AttestationEnvelope {
        envelope: good.envelope.clone(),
        // envelope slot now holds the PAYLOAD signature
        signature_hex: good.payload_signature_hex.clone(),
        payload_signature_hex: good.payload_signature_hex,
    };
    assert_eq!(
        swapped.verify_and_parse_ed25519(&sk.verifying_key()),
        Err(AttestationRejectReason::BadSignature),
        "the payload signature must not double as the envelope signature"
    );
}

#[test]
fn a_release_attestation_never_verifies_under_a_mint_target_domain() {
    // Bind-check the two mint payload domains against the release domain by
    // signing a release envelope but selecting a mint chain's envelope
    // domain. Verification derives the domain from the envelope's own
    // action, so the mismatch fails closed.
    let sk = signing_key(4);
    let order = burn_order();
    let kind = release_kind(&order);
    let signer_key_id = hex::encode(sk.verifying_key().as_bytes());
    let envelope = canonical_attestation_envelope(&kind, &signer_key_id, "n4", now(), now() + 120);

    // Sign the release envelope but select the SOLANA (mint) envelope domain
    // instead of BTH. Verification derives the domain from the envelope's own
    // action, so the mismatch fails closed.
    let wrong_msg = attestation_signed_message(Chain::Solana, envelope.as_bytes());
    let payload = kind.ed25519_payload_digest().unwrap();
    let forged = AttestationEnvelope {
        envelope,
        signature_hex: hex::encode(sk.sign(&wrong_msg).to_bytes()),
        payload_signature_hex: hex::encode(sk.sign(&payload).to_bytes()),
    };
    assert_eq!(
        forged.verify_and_parse_ed25519(&sk.verifying_key()),
        Err(AttestationRejectReason::BadSignature),
    );
}

// ---------------------------------------------------------------------------
// Equivocation / double-signing (#829 gap test b)
// ---------------------------------------------------------------------------

#[test]
fn equivocating_signer_cannot_inflate_the_distinct_signer_count() {
    // A Byzantine federation member signs TWO different valid attestations
    // for the same (order, action) — the classic equivocation move to try to
    // count as two toward the threshold. The aggregation primitive
    // deduplicates by signer identity, so the second submission counts zero.
    //
    // NOTE ON DETECTION (follow-up filed): `AttestationSet` is *resistant* to
    // equivocation (a single malicious signer can never move the threshold),
    // but it does not *flag* the equivocation as an auditable event — it
    // silently keeps the first submission. Active detection (raising an
    // equivocation alarm when a signer presents conflicting bytes for the
    // same order id) is tracked as a hardening follow-up. See the threat
    // model doc: docs/security/bridge-threat-model.md.
    let sk = signing_key(5);
    let order = burn_order();
    let kind = release_kind(&order);

    // Two envelopes from the same signer, distinct nonces (both individually
    // valid), for the same order/action.
    let env_a = sign_attestation_ed25519(&kind, &sk, "eq-a", now(), now() + 120).unwrap();
    let env_b = sign_attestation_ed25519(&kind, &sk, "eq-b", now(), now() + 120).unwrap();
    let parsed_a = env_a.verify_and_parse_ed25519(&sk.verifying_key()).unwrap();
    let parsed_b = env_b.verify_and_parse_ed25519(&sk.verifying_key()).unwrap();

    let mut set = AttestationSet::for_attestation(&parsed_a);
    let sig = |hex_str: &str| AttestationSignature {
        signer: sk.verifying_key().as_bytes().to_vec(),
        signature: hex::decode(hex_str).unwrap(),
    };

    assert_eq!(
        set.insert(&parsed_a, sig(&env_a.payload_signature_hex)),
        Ok(true),
        "first submission from a signer counts"
    );
    assert_eq!(
        set.insert(&parsed_b, sig(&env_b.payload_signature_hex)),
        Ok(false),
        "the SAME signer's second (equivocating) submission must not count again"
    );
    assert_eq!(set.distinct_signers(), 1);
    assert!(
        !set.is_threshold_met(2),
        "a single equivocating signer can never satisfy a 2-of-n threshold"
    );
}

#[test]
fn a_zero_threshold_never_authorizes_even_with_signatures() {
    // Defense in depth against a misconfigured federation: even a populated
    // set must not be considered authorized at threshold 0 (a t-of-n
    // federation always requires at least one signature).
    let sk = signing_key(6);
    let order = burn_order();
    let kind = release_kind(&order);
    let env = sign_attestation_ed25519(&kind, &sk, "z1", now(), now() + 120).unwrap();
    let parsed = env.verify_and_parse_ed25519(&sk.verifying_key()).unwrap();

    let mut set = AttestationSet::for_attestation(&parsed);
    set.insert(
        &parsed,
        AttestationSignature {
            signer: sk.verifying_key().as_bytes().to_vec(),
            signature: hex::decode(&env.payload_signature_hex).unwrap(),
        },
    )
    .unwrap();
    assert!(set.distinct_signers() >= 1);
    assert!(!set.is_threshold_met(0), "threshold 0 must never authorize");
}

#[test]
fn conflicting_payload_for_the_same_order_cannot_cross_order_binding() {
    // The *upstream* guard that makes equivocation moot: a signer cannot
    // produce two binding-valid attestations for one order that differ in
    // amount/recipient, because `check_order_binding` pins every field to the
    // on-record order. Here a validly-signed release for a DIFFERENT amount
    // is rejected against the real order before it can ever reach a set.
    use crate::attestation::check_order_binding;

    let sk = signing_key(7);
    let order = burn_order();

    // A validly-signed attestation that inflates the amount by 1 picocredit.
    let inflated = AttestationKind::ReleaseBth {
        source_chain: order.source_chain,
        bth_address: order.dest_address.clone(),
        amount: order.net_amount() + 1,
        order_id: order.id,
        source_tx: order.source_tx.clone().unwrap(),
    };
    let env = sign_attestation_ed25519(&inflated, &sk, "cb1", now(), now() + 120).unwrap();
    let parsed = env.verify_and_parse_ed25519(&sk.verifying_key()).unwrap();

    // On-record order carries a confirmed source tx so binding can run.
    let mut on_record = order.clone();
    on_record.source_tx = Some("0xburntx".to_string());

    match check_order_binding(&parsed, &on_record) {
        Err(AttestationRejectReason::WrongOrder(_)) => {}
        other => panic!("expected WrongOrder for an amount-inflated attestation, got {other:?}"),
    }
    // Sanity: the digest an inflated attestation signs differs from the real
    // order's release digest, so the two can never share a signature.
    assert_ne!(
        release_payload_digest(
            &order.order_id_bytes(),
            order.net_amount(),
            &order.dest_address
        ),
        release_payload_digest(
            &order.order_id_bytes(),
            order.net_amount() + 1,
            &order.dest_address
        ),
    );
}
