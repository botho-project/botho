// Copyright (c) 2024 The Botho Foundation

//! Mint authorization types produced by the validator attestation protocol.
//!
//! Per ADR 0002 (bridge custody), every wBTH mint must be authorized by a
//! t-of-n threshold of the SCP validator federation:
//!
//! - **Ethereum**: each validator operates a secp256k1 signer; the collected
//!   signatures are the owner signatures for the Gnosis Safe that holds
//!   `MINTER_ROLE` on `WrappedBTH.sol`.
//! - **Solana**: validators sign natively with their Ed25519 node keys.
//!
//! The attestation *protocol* (signature collection, envelopes, nonces) is
//! issue #824 and lives outside this crate. This module only defines the
//! artifact that protocol produces — [`MintAuthorization`] — which the mint
//! submission path (issue #821) consumes.

use serde::{Deserialize, Serialize};

/// The signature scheme used by an attestation, per destination chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignatureScheme {
    /// secp256k1 ECDSA (Ethereum Gnosis Safe owner signatures).
    Secp256k1,
    /// Ed25519 (Solana native validator keys).
    Ed25519,
}

/// A single validator's signature over a mint authorization payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttestationSignature {
    /// Signer identity. For secp256k1 this is the 20-byte Ethereum address
    /// of the Safe owner; for Ed25519 this is the 32-byte public key.
    #[serde(with = "hex_bytes")]
    pub signer: Vec<u8>,

    /// Signature bytes. 65 bytes ({r, s, v}) for secp256k1 Safe owner
    /// signatures; 64 bytes for Ed25519.
    #[serde(with = "hex_bytes")]
    pub signature: Vec<u8>,
}

/// Threshold authorization for a single wBTH mint, produced by the #824
/// attestation protocol and bound to one bridge order.
///
/// The signed payload is chain-specific:
/// - Ethereum: the Gnosis Safe transaction hash (EIP-712) wrapping
///   `bridgeMint(to, amount, orderId)`.
/// - Solana: the transaction message containing the `bridge_mint` instruction
///   with the same `orderId`.
///
/// Binding to the on-chain `orderId` (not just the BTH deposit tx) means a
/// replayed authorization can never mint twice: the destination contract
/// rejects a duplicate order id (#826), and the Safe nonce / Solana
/// blockhash rejects a replayed transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintAuthorization {
    /// The deterministic 32-byte on-chain order id this authorization is
    /// bound to. Must equal [`crate::order::BridgeOrder::order_id_bytes`]
    /// for the order being minted.
    #[serde(with = "hex_array_32")]
    pub order_id: [u8; 32],

    /// Signature scheme (implied by the destination chain).
    pub scheme: SignatureScheme,

    /// The threshold `t` required by the federation configuration. Per
    /// ADR 0002 this is never lower than the SCP safety threshold.
    pub threshold: u32,

    /// Collected validator signatures. Must contain at least `threshold`
    /// entries from distinct signers to be usable.
    pub signatures: Vec<AttestationSignature>,
}

impl MintAuthorization {
    /// Whether enough distinct signers have signed to meet the threshold.
    pub fn meets_threshold(&self) -> bool {
        let mut signers: Vec<&[u8]> = self
            .signatures
            .iter()
            .map(|s| s.signer.as_slice())
            .collect();
        signers.sort();
        signers.dedup();
        signers.len() as u32 >= self.threshold
    }
}

/// Domain-separation tag for the BTH reserve-release attestation payload.
///
/// Mirrors the operator-signed-action pattern (`botho/src/operator_action.rs`
/// `DOMAIN_SEPARATOR`, per ADR 0002): every federation signature covers this
/// tag so a release signature can never be confused with (or replayed as) an
/// operator action, a mint attestation, or any other Ed25519 payload signed
/// by a validator node key. Changing this tag invalidates all in-flight
/// release authorizations.
pub const RELEASE_ATTESTATION_DOMAIN_TAG: &[u8] = b"botho-bridge-release-v1";

/// Compute the digest the federation signs to authorize one BTH release.
///
/// `sha256(domain_tag || order_id || amount_le || recipient_len_le ||
/// recipient)`
///
/// The digest binds the authorization to:
/// - the deterministic on-chain **order id** (replay safety: the release
///   engine's `release_claims` table plus the order state machine allow at most
///   one release per order id, so a replayed authorization cannot trigger a
///   second reserve spend);
/// - the exact **net amount** in picocredits; and
/// - the exact **recipient** BTH address (length-prefixed so the encoding is
///   unambiguous).
///
/// The #824 attestation protocol produces signatures over exactly these
/// bytes; the release engine verifies them before any reserve key material
/// is touched.
pub fn release_payload_digest(order_id: &[u8; 32], amount: u64, recipient: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(RELEASE_ATTESTATION_DOMAIN_TAG);
    hasher.update(order_id);
    hasher.update(amount.to_le_bytes());
    hasher.update((recipient.len() as u64).to_le_bytes());
    hasher.update(recipient.as_bytes());
    hasher.finalize().into()
}

/// Threshold authorization for a single BTH reserve release, produced by the
/// #824 attestation protocol and bound to one bridge burn order.
///
/// Per ADR 0002, releases are authorized by a t-of-n threshold of the SCP
/// validators' **Ed25519 node keys** (the scheme is always Ed25519 on the
/// BTH side, so no scheme field is carried). Each signature covers
/// [`release_payload_digest`], binding the order id, net amount, and
/// recipient address — a signature for one order can never authorize a
/// different amount, recipient, or a second release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseAuthorization {
    /// The deterministic 32-byte order id this authorization is bound to.
    /// Must equal [`crate::order::BridgeOrder::order_id_bytes`] for the
    /// burn order being released.
    #[serde(with = "hex_array_32")]
    pub order_id: [u8; 32],

    /// Net amount to pay the recipient, in picocredits
    /// ([`crate::order::BridgeOrder::net_amount`]).
    pub amount: u64,

    /// The recipient's BTH address (`bridgeBurn`'s `bthAddress`). The
    /// release transaction pays a **fresh one-time stealth output** derived
    /// from this address (ADR 0004) — never a static payout key.
    pub recipient: String,

    /// The threshold `t` required by the federation configuration. Per
    /// ADR 0002 this is never lower than the SCP safety threshold.
    pub threshold: u32,

    /// Collected validator Ed25519 signatures over
    /// [`ReleaseAuthorization::digest`]. Must contain at least `threshold`
    /// entries from distinct signers to be usable.
    pub signatures: Vec<AttestationSignature>,
}

impl ReleaseAuthorization {
    /// The digest every federation signature must cover.
    pub fn digest(&self) -> [u8; 32] {
        release_payload_digest(&self.order_id, self.amount, &self.recipient)
    }

    /// Whether enough distinct signers have signed to meet the threshold.
    ///
    /// This only counts distinct signer identities — cryptographic
    /// verification of each signature (and federation membership) is the
    /// release engine's job (`validate_release_attestation`).
    pub fn meets_threshold(&self) -> bool {
        let mut signers: Vec<&[u8]> = self
            .signatures
            .iter()
            .map(|s| s.signer.as_slice())
            .collect();
        signers.sort();
        signers.dedup();
        signers.len() as u32 >= self.threshold
    }
}

/// Hex serde for `Vec<u8>`.
mod hex_bytes {
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(deserializer)?;
        hex::decode(&s).map_err(serde::de::Error::custom)
    }
}

/// Hex serde for `[u8; 32]`.
mod hex_array_32 {
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8; 32], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<[u8; 32], D::Error> {
        let s = String::deserialize(deserializer)?;
        let bytes = hex::decode(&s).map_err(serde::de::Error::custom)?;
        bytes
            .try_into()
            .map_err(|_| serde::de::Error::custom("order_id must be 32 bytes"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(signer: u8) -> AttestationSignature {
        AttestationSignature {
            signer: vec![signer; 20],
            signature: vec![0u8; 65],
        }
    }

    #[test]
    fn test_meets_threshold() {
        let mut auth = MintAuthorization {
            order_id: [7u8; 32],
            scheme: SignatureScheme::Secp256k1,
            threshold: 2,
            signatures: vec![sig(1)],
        };
        assert!(!auth.meets_threshold());

        auth.signatures.push(sig(2));
        assert!(auth.meets_threshold());

        // Duplicate signers do not count twice.
        auth.signatures = vec![sig(1), sig(1)];
        assert!(!auth.meets_threshold());
    }

    #[test]
    fn test_serde_roundtrip() {
        let auth = MintAuthorization {
            order_id: [9u8; 32],
            scheme: SignatureScheme::Ed25519,
            threshold: 3,
            signatures: vec![sig(1), sig(2)],
        };
        let json = serde_json::to_string(&auth).unwrap();
        let back: MintAuthorization = serde_json::from_str(&json).unwrap();
        assert_eq!(auth, back);
    }

    #[test]
    fn test_release_digest_golden_vector() {
        use sha2::{Digest, Sha256};

        // Pinned construction: sha256(tag || order_id || amount_le ||
        // recipient_len_le || recipient). This must never change — in-flight
        // release authorizations bind to it.
        let order_id = [3u8; 32];
        let digest = release_payload_digest(&order_id, 999_000_000_000, "bth_stealth_addr");

        let expected: [u8; 32] = {
            let mut hasher = Sha256::new();
            hasher.update(b"botho-bridge-release-v1");
            hasher.update([3u8; 32]);
            hasher.update(999_000_000_000u64.to_le_bytes());
            hasher.update((b"bth_stealth_addr".len() as u64).to_le_bytes());
            hasher.update(b"bth_stealth_addr");
            hasher.finalize().into()
        };
        assert_eq!(digest, expected);
    }

    #[test]
    fn test_release_digest_binds_all_fields() {
        let base = release_payload_digest(&[1u8; 32], 100, "addr");
        // Different order id, amount, or recipient each produce a different
        // digest — a signature cannot be replayed across any of them.
        assert_ne!(base, release_payload_digest(&[2u8; 32], 100, "addr"));
        assert_ne!(base, release_payload_digest(&[1u8; 32], 101, "addr"));
        assert_ne!(base, release_payload_digest(&[1u8; 32], 100, "addr2"));
    }

    #[test]
    fn test_release_authorization_threshold_and_serde() {
        let mut auth = ReleaseAuthorization {
            order_id: [7u8; 32],
            amount: 42,
            recipient: "bth_addr".to_string(),
            threshold: 2,
            signatures: vec![sig(1)],
        };
        assert!(!auth.meets_threshold());

        auth.signatures.push(sig(2));
        assert!(auth.meets_threshold());

        // Duplicate signers do not count twice.
        auth.signatures = vec![sig(1), sig(1)];
        assert!(!auth.meets_threshold());

        auth.signatures = vec![sig(1), sig(2)];
        let json = serde_json::to_string(&auth).unwrap();
        let back: ReleaseAuthorization = serde_json::from_str(&json).unwrap();
        assert_eq!(auth, back);
        assert_eq!(auth.digest(), back.digest());
    }
}
