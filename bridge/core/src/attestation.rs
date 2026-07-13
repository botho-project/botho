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
//! This module defines both the **artifacts** the protocol produces
//! ([`MintAuthorization`] / [`ReleaseAuthorization`], consumed by the mint
//! and release submission paths) and the **attestation envelope protocol**
//! itself (#824): domain-separated, order-bound, single-use signed
//! envelopes mirroring the operator-signed-action machinery
//! (`botho/src/operator_action.rs`, P4.4), aggregated to the federation
//! threshold by [`AttestationSet`]. Signature *collection over the network*
//! between federation members is the #858 transport in
//! `bridge/service/src/federation.rs` (an authenticated `POST /api/attest`
//! endpoint in front of this pipeline, plus outbound peer push); everything
//! cryptographic — signing, verification, replay rejection, order binding,
//! thresholding — is implemented here and in
//! `bridge/service/src/attestation.rs`.

use ed25519_dalek::{Signature as Ed25519Signature, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::{
    chains::Chain,
    order::{derive_order_id, BridgeOrder, OrderType},
};

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

// ---------------------------------------------------------------------------
// Mint payload digest (the mint-side equivalent of `release_payload_digest`)
// ---------------------------------------------------------------------------

/// Domain-separation tag for the Solana wBTH-mint attestation payload.
///
/// Distinct from [`RELEASE_ATTESTATION_DOMAIN_TAG`] and from the Ethereum
/// mint path (which signs the Gnosis Safe EIP-712 transaction hash instead,
/// per ADR 0002), so a signature over one chain's mint can never be replayed
/// as another chain's mint or as a release.
pub const MINT_ATTESTATION_DOMAIN_TAG_SOL: &[u8] = b"botho-bridge-mint-sol-v1";

/// Domain-separation tag for the Ethereum wBTH-mint attestation payload.
///
/// NOTE: production Ethereum mint authorizations sign the **Gnosis Safe
/// EIP-712 transaction hash** (secp256k1 owner signatures, ADR 0002), which
/// already binds the chain id, Safe address, calldata (order id, amount,
/// recipient) and Safe nonce. This tag exists so the digest construction is
/// defined for both destination chains and provably differs per chain.
pub const MINT_ATTESTATION_DOMAIN_TAG_ETH: &[u8] = b"botho-bridge-mint-eth-v1";

/// Compute the digest a federation member signs to authorize one wBTH mint
/// on `dest_chain` (Ed25519 on Solana; see
/// [`MINT_ATTESTATION_DOMAIN_TAG_ETH`] for the Ethereum SafeTx caveat).
///
/// `sha256(domain_tag || order_id || amount_le || recipient_len_le ||
/// recipient)` — the exact structure of [`release_payload_digest`], with a
/// per-destination-chain domain tag, so:
/// - a mint signature for order X can never authorize order Y (order id);
/// - a mint signature for chain C can never authorize chain D (domain tag);
/// - a mint signature can never be replayed as a release (domain tag).
pub fn mint_payload_digest(
    dest_chain: Chain,
    order_id: &[u8; 32],
    amount: u64,
    recipient: &str,
) -> Result<[u8; 32], String> {
    use sha2::{Digest, Sha256};
    let tag = match dest_chain {
        Chain::Ethereum => MINT_ATTESTATION_DOMAIN_TAG_ETH,
        Chain::Solana => MINT_ATTESTATION_DOMAIN_TAG_SOL,
        Chain::Bth => return Err("cannot mint wBTH on the BTH chain".to_string()),
    };
    let mut hasher = Sha256::new();
    hasher.update(tag);
    hasher.update(order_id);
    hasher.update(amount.to_le_bytes());
    hasher.update((recipient.len() as u64).to_le_bytes());
    hasher.update(recipient.as_bytes());
    Ok(hasher.finalize().into())
}

// ---------------------------------------------------------------------------
// Attestation envelope protocol (#824) — mirrors botho/src/operator_action.rs
// ---------------------------------------------------------------------------

/// Envelope domain separator for attestations whose privileged action
/// executes on **Ethereum** (wBTH mint via the Gnosis Safe).
pub const ATTEST_DOMAIN_ETH: &[u8] = b"botho-bridge-attest-eth-v1";

/// Envelope domain separator for attestations whose privileged action
/// executes on **Solana** (wBTH mint via the bridge program).
pub const ATTEST_DOMAIN_SOL: &[u8] = b"botho-bridge-attest-sol-v1";

/// Envelope domain separator for attestations whose privileged action
/// executes on **BTH** (reserve release).
pub const ATTEST_DOMAIN_BTH: &[u8] = b"botho-bridge-attest-bth-v1";

/// The only envelope version v1 verifiers accept. Unknown versions are
/// rejected with no downgrade path.
pub const ATTESTATION_ENVELOPE_VERSION: u64 = 1;

/// Maximum lifetime of an attestation envelope in seconds
/// (`expiresAt - issuedAt`). Mirrors the operator-action bound; a captured
/// envelope expires and cannot be replayed after the window.
pub const MAX_ATTESTATION_LIFETIME_SECS: u64 = 300;

/// Clock-skew allowance for the freshness check (`issuedAt - skew <= now`).
pub const ATTESTATION_CLOCK_SKEW_SECS: u64 = 30;

/// The envelope domain separator for a given **target chain** (the chain the
/// privileged action executes on). A signature over one chain's envelope can
/// never verify under another chain's domain — cross-domain binding.
pub fn attestation_domain(target_chain: Chain) -> &'static [u8] {
    match target_chain {
        Chain::Ethereum => ATTEST_DOMAIN_ETH,
        Chain::Solana => ATTEST_DOMAIN_SOL,
        Chain::Bth => ATTEST_DOMAIN_BTH,
    }
}

/// The exact byte string an attestation envelope signature covers:
/// `attestation_domain(target_chain) || envelope_bytes`.
pub fn attestation_signed_message(target_chain: Chain, envelope_bytes: &[u8]) -> Vec<u8> {
    let domain = attestation_domain(target_chain);
    let mut msg = Vec::with_capacity(domain.len() + envelope_bytes.len());
    msg.extend_from_slice(domain);
    msg.extend_from_slice(envelope_bytes);
    msg
}

/// The v1 attestation action allowlist. Any action outside this enum is
/// rejected fail-closed — these are the ONLY two privileged actions the
/// bridge federation can authorize (mirrors `operator_action::ActionKind`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttestationKind {
    /// `bridge.mint_wbth` — "the BTH deposit funding `order_id` is final;
    /// mint `amount` wBTH to `dest_address` on `dest_chain`".
    MintWbth {
        /// Destination chain (Ethereum or Solana — never Bth).
        dest_chain: Chain,
        /// Recipient address on the destination chain.
        dest_address: String,
        /// Net amount to mint, in picocredits.
        amount: u64,
        /// The bridge order UUID (the 32-byte on-chain id is derived via
        /// [`derive_order_id`]).
        order_id: Uuid,
        /// The finalized BTH deposit transaction hash being attested.
        source_tx: String,
        /// Ethereum only: the Gnosis Safe nonce the secp256k1 payload
        /// signature is bound to (the SafeTx hash covers it). REQUIRED for
        /// `dest_chain: Ethereum`, and must be ABSENT for Solana, so every
        /// logical attestation has exactly one canonical encoding.
        safe_nonce: Option<u64>,
    },
    /// `bridge.release_bth` — "the wBTH burn funding `order_id` is final;
    /// release `amount` picocredits from the reserve to `bth_address`".
    ReleaseBth {
        /// Chain the wBTH was burned on (Ethereum or Solana).
        source_chain: Chain,
        /// Recipient BTH address (paid as a fresh one-time stealth output).
        bth_address: String,
        /// Net amount to release, in picocredits.
        amount: u64,
        /// The bridge order UUID.
        order_id: Uuid,
        /// The finalized burn transaction hash being attested.
        source_tx: String,
    },
}

impl AttestationKind {
    /// The wire `action` string for this variant.
    pub fn name(&self) -> &'static str {
        match self {
            AttestationKind::MintWbth { .. } => "bridge.mint_wbth",
            AttestationKind::ReleaseBth { .. } => "bridge.release_bth",
        }
    }

    /// The chain the privileged action executes on — the envelope
    /// domain-separator selector.
    pub fn target_chain(&self) -> Chain {
        match self {
            AttestationKind::MintWbth { dest_chain, .. } => *dest_chain,
            AttestationKind::ReleaseBth { .. } => Chain::Bth,
        }
    }

    /// The bridge order UUID this attestation is bound to.
    pub fn order_uuid(&self) -> Uuid {
        match self {
            AttestationKind::MintWbth { order_id, .. }
            | AttestationKind::ReleaseBth { order_id, .. } => *order_id,
        }
    }

    /// The deterministic 32-byte on-chain order id ([`derive_order_id`]).
    pub fn order_id_bytes(&self) -> [u8; 32] {
        derive_order_id(&self.order_uuid())
    }

    /// The attested net amount in picocredits.
    pub fn amount(&self) -> u64 {
        match self {
            AttestationKind::MintWbth { amount, .. }
            | AttestationKind::ReleaseBth { amount, .. } => *amount,
        }
    }

    /// The attested recipient (destination address / BTH payout address).
    pub fn recipient(&self) -> &str {
        match self {
            AttestationKind::MintWbth { dest_address, .. } => dest_address,
            AttestationKind::ReleaseBth { bth_address, .. } => bth_address,
        }
    }

    /// The finalized source transaction being attested.
    pub fn source_tx(&self) -> &str {
        match self {
            AttestationKind::MintWbth { source_tx, .. }
            | AttestationKind::ReleaseBth { source_tx, .. } => source_tx,
        }
    }

    /// The action's `params` object as it appears in the envelope, for the
    /// audit log. Reconstructed from the parsed action so an audit entry
    /// never re-reads untrusted bytes.
    pub fn params_value(&self) -> Value {
        match self {
            AttestationKind::MintWbth {
                dest_chain,
                dest_address,
                amount,
                order_id,
                source_tx,
                safe_nonce,
            } => {
                let mut v = serde_json::json!({
                    "amount": amount,
                    "destAddress": dest_address,
                    "destChain": dest_chain.to_string(),
                    "orderId": order_id.to_string(),
                    "sourceTx": source_tx,
                });
                if let Some(n) = safe_nonce {
                    v["safeNonce"] = Value::from(*n);
                }
                v
            }
            AttestationKind::ReleaseBth {
                source_chain,
                bth_address,
                amount,
                order_id,
                source_tx,
            } => serde_json::json!({
                "amount": amount,
                "bthAddress": bth_address,
                "orderId": order_id.to_string(),
                "sourceChain": source_chain.to_string(),
                "sourceTx": source_tx,
            }),
        }
    }

    /// The canonical (lexicographically key-ordered, whitespace-free)
    /// `params` JSON for the signed envelope bytes. Built by hand so the
    /// canonical form never depends on `serde_json` feature flags.
    fn canonical_params(&self) -> String {
        let s = |v: &str| serde_json::to_string(v).expect("string serialization is infallible");
        match self {
            AttestationKind::MintWbth {
                dest_chain,
                dest_address,
                amount,
                order_id,
                source_tx,
                safe_nonce,
            } => {
                let safe_nonce = match safe_nonce {
                    Some(n) => format!("\"safeNonce\":{n},"),
                    None => String::new(),
                };
                format!(
                    "{{\"amount\":{amount},\"destAddress\":{dest},\"destChain\":{chain},\
                     \"orderId\":{order},{safe_nonce}\"sourceTx\":{src}}}",
                    dest = s(dest_address),
                    chain = s(&dest_chain.to_string()),
                    order = s(&order_id.to_string()),
                    src = s(source_tx),
                )
            }
            AttestationKind::ReleaseBth {
                source_chain,
                bth_address,
                amount,
                order_id,
                source_tx,
            } => format!(
                "{{\"amount\":{amount},\"bthAddress\":{addr},\"orderId\":{order},\
                 \"sourceChain\":{chain},\"sourceTx\":{src}}}",
                addr = s(bth_address),
                order = s(&order_id.to_string()),
                chain = s(&source_chain.to_string()),
                src = s(source_tx),
            ),
        }
    }

    /// The Ed25519 payload digest this attestation's `payloadSignature`
    /// must cover:
    ///
    /// - `ReleaseBth` → [`release_payload_digest`] (verified downstream by
    ///   `validate_release_attestation` before any reserve spend);
    /// - `MintWbth { dest_chain: Solana }` → [`mint_payload_digest`].
    ///
    /// `MintWbth { dest_chain: Ethereum }` has NO Ed25519 payload digest:
    /// per ADR 0002 its payload signature is a secp256k1 owner signature
    /// over the Gnosis Safe EIP-712 transaction hash (computed by the
    /// service layer, which knows the Safe/chain parameters).
    pub fn ed25519_payload_digest(&self) -> Result<[u8; 32], String> {
        match self {
            AttestationKind::ReleaseBth {
                bth_address,
                amount,
                ..
            } => Ok(release_payload_digest(
                &self.order_id_bytes(),
                *amount,
                bth_address,
            )),
            AttestationKind::MintWbth {
                dest_chain: Chain::Solana,
                dest_address,
                amount,
                ..
            } => mint_payload_digest(Chain::Solana, &self.order_id_bytes(), *amount, dest_address),
            AttestationKind::MintWbth { .. } => Err(
                "Ethereum mint attestations sign the Gnosis Safe transaction hash \
                 (secp256k1); there is no Ed25519 payload digest"
                    .to_string(),
            ),
        }
    }
}

/// Why an attestation was rejected. Mirrors `operator_action::RejectReason`:
/// [`AttestationRejectReason::is_authenticated`] discriminates pre-signature
/// failures (reachable by any unauthenticated caller — rate-limit + count
/// only) from authenticated refusals (audit-logged).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttestationRejectReason {
    /// No federation is configured for the attestation's target chain.
    NotConfigured,
    /// `signerKeyId` matched no configured federation member (pre-signature).
    UnknownSigner,
    /// The envelope could not be read far enough to attempt signature
    /// verification (pre-signature — unauthenticated caller).
    Malformed(String),
    /// A signature (envelope or payload) did not verify (pre-signature).
    BadSignature,
    /// The attestation is bound to a different order / amount / recipient /
    /// chain / source tx than the on-record order (post-signature).
    WrongOrder(String),
    /// Freshness failed: expired, not-yet-valid beyond skew, or lifetime
    /// exceeded (post-signature).
    Stale(String),
    /// The `(signerKeyId, nonce)` pair was already consumed — a replay
    /// (post-signature).
    ReplayedNonce,
    /// Fewer than `t` distinct valid signers have attested (post-signature;
    /// not a per-envelope failure but part of the outcome taxonomy).
    BelowThreshold(String),
    /// Payload validity failed: unknown/duplicate keys, unknown action or
    /// version, out-of-range values (post-signature).
    InvalidPayload(String),
    /// Infrastructure failure (e.g. the nonce store could not persist).
    /// Never a clean rejection — the caller must retry, not conclude.
    Internal(String),
}

impl AttestationRejectReason {
    /// Whether this rejection is for an AUTHENTICATED envelope (signature
    /// already verified). Only authenticated outcomes are audit-logged;
    /// pre-signature failures are rate-limited + counted.
    pub fn is_authenticated(&self) -> bool {
        !matches!(
            self,
            AttestationRejectReason::NotConfigured
                | AttestationRejectReason::UnknownSigner
                | AttestationRejectReason::Malformed(_)
                | AttestationRejectReason::BadSignature
        )
    }

    /// A short, stable machine tag for the audit log. Contains no secret
    /// material and no attacker-controlled free text.
    pub fn tag(&self) -> &'static str {
        match self {
            AttestationRejectReason::NotConfigured => "not_configured",
            AttestationRejectReason::UnknownSigner => "unknown_signer",
            AttestationRejectReason::Malformed(_) => "malformed",
            AttestationRejectReason::BadSignature => "bad_signature",
            AttestationRejectReason::WrongOrder(_) => "wrong_order",
            AttestationRejectReason::Stale(_) => "stale",
            AttestationRejectReason::ReplayedNonce => "replayed_nonce",
            AttestationRejectReason::BelowThreshold(_) => "below_threshold",
            AttestationRejectReason::InvalidPayload(_) => "invalid_payload",
            AttestationRejectReason::Internal(_) => "internal",
        }
    }

    /// A human-facing message (safe to return to the submitter).
    pub fn message(&self) -> String {
        match self {
            AttestationRejectReason::NotConfigured => {
                "no federation configured for this target chain".to_string()
            }
            AttestationRejectReason::UnknownSigner => {
                "signer is not a configured federation member".to_string()
            }
            AttestationRejectReason::Malformed(d) => format!("malformed attestation: {d}"),
            AttestationRejectReason::BadSignature => "signature verification failed".to_string(),
            AttestationRejectReason::WrongOrder(d) => {
                format!("attestation does not match the order: {d}")
            }
            AttestationRejectReason::Stale(d) => format!("attestation not fresh: {d}"),
            AttestationRejectReason::ReplayedNonce => "nonce already used (replay)".to_string(),
            AttestationRejectReason::BelowThreshold(d) => format!("below threshold: {d}"),
            AttestationRejectReason::InvalidPayload(d) => format!("invalid payload: {d}"),
            AttestationRejectReason::Internal(d) => format!("internal error: {d}"),
        }
    }
}

/// The structured verdict of ingesting one attestation envelope — the seam
/// the audit log hooks onto (mirrors `operator_action::OperatorActionOutcome`).
#[derive(Debug, Clone)]
pub struct AttestationOutcome {
    /// Whether the attestation was accepted into its threshold set.
    pub accepted: bool,
    /// Whether the envelope's signature verified (audit-log gate: only
    /// authenticated outcomes are logged).
    pub authenticated: bool,
    /// Stable machine tag: `accepted` or `refused:<reason-tag>`.
    pub tag: String,
    /// Human-facing detail.
    pub message: String,
    /// Signer identity, when authenticated.
    pub signer_key_id: Option<String>,
    /// The attempted action name, when parsed.
    pub action: Option<String>,
    /// The bound order UUID, when parsed.
    pub order_id: Option<Uuid>,
    /// Distinct valid signers collected for this `(order, action)` so far.
    pub signers: u32,
    /// The configured federation threshold `t`.
    pub threshold: u32,
}

impl AttestationOutcome {
    /// Build an accepted outcome with threshold progress `signers/threshold`.
    pub fn accept(parsed: &ParsedAttestation, signers: u32, threshold: u32) -> Self {
        Self {
            accepted: true,
            authenticated: true,
            tag: "accepted".to_string(),
            message: format!("attestation accepted ({signers}/{threshold} distinct signers)"),
            signer_key_id: Some(parsed.signer_key_id.clone()),
            action: Some(parsed.action.name().to_string()),
            order_id: Some(parsed.action.order_uuid()),
            signers,
            threshold,
        }
    }

    /// Build a refusal outcome. `parsed` is `Some` once the envelope parsed
    /// (post-signature), giving the signer + action for the audit log.
    pub fn refuse(
        reason: &AttestationRejectReason,
        parsed: Option<&ParsedAttestation>,
        signers: u32,
        threshold: u32,
    ) -> Self {
        Self {
            accepted: false,
            authenticated: reason.is_authenticated(),
            tag: format!("refused:{}", reason.tag()),
            message: reason.message(),
            signer_key_id: parsed.map(|p| p.signer_key_id.clone()),
            action: parsed.map(|p| p.action.name().to_string()),
            order_id: parsed.map(|p| p.action.order_uuid()),
            signers,
            threshold,
        }
    }
}

/// A signed attestation envelope: the canonical JSON string exactly as the
/// federation member signed it, plus TWO detached signatures.
///
/// - `signature_hex` covers `attestation_domain(target_chain) ||
///   envelope_bytes` — it authenticates the envelope (nonce, freshness window,
///   action) and provides the cross-domain binding.
/// - `payload_signature_hex` covers the chain-specific payload digest
///   ([`AttestationKind::ed25519_payload_digest`], or the Gnosis Safe
///   transaction hash for Ethereum mints). This is the signature that survives
///   into [`MintAuthorization`]/[`ReleaseAuthorization`] and is re-verified by
///   the mint/release path before any privileged action.
///
/// Verification is always over the RECEIVED bytes; the envelope is parsed
/// only AFTER its signature verifies (parse-after-verify), and the parser
/// rejects unknown or duplicate keys so one signed byte string can never
/// carry two logical attestations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttestationEnvelope {
    /// The canonical JSON envelope string, verbatim, as signed.
    pub envelope: String,
    /// Detached signature over the domain-separated envelope bytes
    /// (lowercase hex; 64-byte Ed25519 or 65-byte secp256k1 `{r, s, v}`).
    pub signature_hex: String,
    /// Detached signature over the chain-specific payload digest
    /// (lowercase hex; same scheme as `signature_hex`).
    pub payload_signature_hex: String,
}

/// A fully-parsed, structurally-validated attestation (after signature
/// verification).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedAttestation {
    /// The attested action (allowlisted, order-bound).
    pub action: AttestationKind,
    /// Unix seconds the envelope was issued.
    pub issued_at: u64,
    /// Unix seconds the envelope expires (freshness window).
    pub expires_at: u64,
    /// Single-use nonce (per signer), consumed via the nonce store.
    pub nonce: String,
    /// Signer identity: lowercase hex of the Ed25519 public key (64 chars)
    /// or of the 20-byte Ethereum owner address (40 chars).
    pub signer_key_id: String,
    /// Envelope version (always [`ATTESTATION_ENVELOPE_VERSION`]).
    pub v: u64,
}

impl ParsedAttestation {
    /// The chain the privileged action executes on.
    pub fn target_chain(&self) -> Chain {
        self.action.target_chain()
    }
}

impl AttestationEnvelope {
    /// Verify an Ed25519-signed attestation (BTH releases and Solana mints,
    /// per ADR 0002) and parse it.
    ///
    /// Pipeline (no secret-dependent check precedes signature verification):
    /// 1. decode both signatures (pre-signature `Malformed`);
    /// 2. peek the target chain from the (unverified) bytes purely to select
    ///    the domain separator — a lying peek changes the signed message and
    ///    fails verification;
    /// 3. verify the envelope signature over `domain || received_bytes`
    ///    (`verify_strict`, rejecting malleable/low-order signatures);
    /// 4. parse THOSE EXACT bytes (parse-after-verify; unknown/duplicate keys
    ///    and unknown versions rejected);
    /// 5. verify the payload signature over the parsed action's Ed25519 payload
    ///    digest with the SAME key.
    pub fn verify_and_parse_ed25519(
        &self,
        verifying_key: &VerifyingKey,
    ) -> Result<ParsedAttestation, AttestationRejectReason> {
        let env_sig =
            decode_sig64(&self.signature_hex).map_err(AttestationRejectReason::Malformed)?;
        let payload_sig = decode_sig64(&self.payload_signature_hex)
            .map_err(AttestationRejectReason::Malformed)?;

        let target = peek_target_chain(&self.envelope)?;
        if target == Chain::Ethereum {
            // Ethereum-target attestations are secp256k1 (SafeTx); handing
            // one to the Ed25519 verifier is a routing error, pre-signature.
            return Err(AttestationRejectReason::Malformed(
                "Ethereum-target attestations use the secp256k1 path".to_string(),
            ));
        }

        let msg = attestation_signed_message(target, self.envelope.as_bytes());
        verifying_key
            .verify_strict(&msg, &Ed25519Signature::from_bytes(&env_sig))
            .map_err(|_| AttestationRejectReason::BadSignature)?;

        // Parse-after-verify: the signature is valid over exactly these
        // bytes; NOW parse them.
        let parsed = parse_attestation_envelope(&self.envelope)
            .map_err(AttestationRejectReason::InvalidPayload)?;

        // The payload signature must cover the digest derived from the
        // (now-verified) action, with the same key.
        let digest = parsed
            .action
            .ed25519_payload_digest()
            .map_err(AttestationRejectReason::InvalidPayload)?;
        verifying_key
            .verify_strict(&digest, &Ed25519Signature::from_bytes(&payload_sig))
            .map_err(|_| AttestationRejectReason::BadSignature)?;

        Ok(parsed)
    }
}

fn decode_sig64(hex_str: &str) -> Result<[u8; 64], String> {
    let bytes =
        hex::decode(hex_str.trim()).map_err(|_| "signature is not valid hex".to_string())?;
    bytes
        .as_slice()
        .try_into()
        .map_err(|_| "ed25519 signature must be 64 bytes".to_string())
}

/// Peek the `signerKeyId` out of the (still-unverified) envelope bytes,
/// ONLY to select which configured federation key to verify against. Drives
/// no security decision on its own: if the presented id names a key the
/// bytes were not signed with, signature verification fails.
pub fn peek_signer_key_id(envelope: &str) -> Result<String, AttestationRejectReason> {
    let value: Value = serde_json::from_str(envelope).map_err(|_| {
        AttestationRejectReason::Malformed("envelope is not valid JSON".to_string())
    })?;
    value
        .as_object()
        .and_then(|o| o.get("signerKeyId"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            AttestationRejectReason::Malformed("envelope missing string `signerKeyId`".to_string())
        })
}

/// Peek the target chain out of the (still-unverified) envelope bytes, ONLY
/// to select the domain separator / verification scheme. Like
/// [`peek_signer_key_id`] this is a hint: a lying value changes the signed
/// message (different domain) and fails signature verification.
pub fn peek_target_chain(envelope: &str) -> Result<Chain, AttestationRejectReason> {
    let value: Value = serde_json::from_str(envelope).map_err(|_| {
        AttestationRejectReason::Malformed("envelope is not valid JSON".to_string())
    })?;
    let obj = value.as_object().ok_or_else(|| {
        AttestationRejectReason::Malformed("envelope must be a JSON object".to_string())
    })?;
    let action = obj.get("action").and_then(|v| v.as_str()).ok_or_else(|| {
        AttestationRejectReason::Malformed("envelope missing string `action`".to_string())
    })?;
    match action {
        "bridge.release_bth" => Ok(Chain::Bth),
        "bridge.mint_wbth" => {
            let dest = obj
                .get("params")
                .and_then(|p| p.as_object())
                .and_then(|p| p.get("destChain"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    AttestationRejectReason::Malformed(
                        "mint envelope missing string `params.destChain`".to_string(),
                    )
                })?;
            parse_canonical_chain(dest).map_err(AttestationRejectReason::Malformed)
        }
        other => Err(AttestationRejectReason::Malformed(format!(
            "action `{other}` is not in the v1 allowlist"
        ))),
    }
}

/// Peek the bound order UUID out of the (still-unverified) envelope bytes,
/// ONLY to route the received envelope to the on-record order the ingest
/// pipeline then binds it against (#858 transport). Like [`peek_signer_key_id`]
/// this is a hint that drives no security decision on its own: the routed
/// order is re-checked field-by-field by `check_order_binding` AFTER the
/// signature verifies, so a lying `orderId` selects an order the signature —
/// or the order binding — then rejects.
pub fn peek_order_id(envelope: &str) -> Result<Uuid, AttestationRejectReason> {
    let value: Value = serde_json::from_str(envelope).map_err(|_| {
        AttestationRejectReason::Malformed("envelope is not valid JSON".to_string())
    })?;
    let order_id = value
        .as_object()
        .and_then(|o| o.get("params"))
        .and_then(|p| p.as_object())
        .and_then(|p| p.get("orderId"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            AttestationRejectReason::Malformed(
                "envelope missing string `params.orderId`".to_string(),
            )
        })?;
    Uuid::parse_str(order_id).map_err(|_| {
        AttestationRejectReason::Malformed("`params.orderId` is not a valid UUID".to_string())
    })
}

/// Parse a chain from its canonical wire string ONLY (`ethereum` /
/// `solana` / `bth`). Aliases are rejected so every logical attestation has
/// exactly one canonical byte encoding.
fn parse_canonical_chain(s: &str) -> Result<Chain, String> {
    match s {
        "bth" => Ok(Chain::Bth),
        "ethereum" => Ok(Chain::Ethereum),
        "solana" => Ok(Chain::Solana),
        other => Err(format!("`{other}` is not a canonical chain name")),
    }
}

/// Build the canonical (lexicographically key-ordered, whitespace-free,
/// integers-only) envelope string for signing.
pub fn canonical_attestation_envelope(
    kind: &AttestationKind,
    signer_key_id: &str,
    nonce: &str,
    issued_at: u64,
    expires_at: u64,
) -> String {
    let s = |v: &str| serde_json::to_string(v).expect("string serialization is infallible");
    format!(
        "{{\"action\":{action},\"expiresAt\":{expires_at},\"issuedAt\":{issued_at},\
         \"nonce\":{nonce},\"params\":{params},\"signerKeyId\":{signer},\"v\":{v}}}",
        action = s(kind.name()),
        nonce = s(nonce),
        params = kind.canonical_params(),
        signer = s(signer_key_id),
        v = ATTESTATION_ENVELOPE_VERSION,
    )
}

/// Build and Ed25519-sign a complete attestation envelope (BTH releases and
/// Solana mints). The signer identity is the lowercase hex of the Ed25519
/// public key. Fails for Ethereum-target kinds (secp256k1, service layer).
pub fn sign_attestation_ed25519(
    kind: &AttestationKind,
    signing_key: &SigningKey,
    nonce: &str,
    issued_at: u64,
    expires_at: u64,
) -> Result<AttestationEnvelope, String> {
    use ed25519_dalek::Signer as _;

    let payload_digest = kind.ed25519_payload_digest()?;
    let signer_key_id = hex::encode(signing_key.verifying_key().as_bytes());
    let envelope =
        canonical_attestation_envelope(kind, &signer_key_id, nonce, issued_at, expires_at);
    let msg = attestation_signed_message(kind.target_chain(), envelope.as_bytes());

    Ok(AttestationEnvelope {
        signature_hex: hex::encode(signing_key.sign(&msg).to_bytes()),
        payload_signature_hex: hex::encode(signing_key.sign(&payload_digest).to_bytes()),
        envelope,
    })
}

/// Parse the canonical envelope bytes, REJECTING unknown or duplicate keys
/// (at every nesting level) and any type/shape error, so a single signed
/// byte string can never carry two logical attestations.
pub fn parse_attestation_envelope(bytes: &str) -> Result<ParsedAttestation, String> {
    reject_duplicate_keys(bytes)?;

    let value: Value =
        serde_json::from_str(bytes).map_err(|e| format!("envelope is not valid JSON: {e}"))?;
    let obj: &Map<String, Value> = value
        .as_object()
        .ok_or_else(|| "envelope must be a JSON object".to_string())?;

    const KNOWN_KEYS: &[&str] = &[
        "action",
        "expiresAt",
        "issuedAt",
        "nonce",
        "params",
        "signerKeyId",
        "v",
    ];
    for key in obj.keys() {
        if !KNOWN_KEYS.contains(&key.as_str()) {
            return Err(format!("unknown envelope field `{key}`"));
        }
    }

    let v = get_u64(obj, "v")?;
    if v != ATTESTATION_ENVELOPE_VERSION {
        return Err(format!(
            "unsupported envelope version {v} (expected {ATTESTATION_ENVELOPE_VERSION})"
        ));
    }

    let issued_at = get_u64(obj, "issuedAt")?;
    let expires_at = get_u64(obj, "expiresAt")?;
    let nonce = get_str(obj, "nonce")?.to_string();
    let signer_key_id = get_str(obj, "signerKeyId")?.to_string();
    let action_name = get_str(obj, "action")?.to_string();

    let params_obj = obj
        .get("params")
        .ok_or_else(|| "missing field `params`".to_string())?
        .as_object()
        .ok_or_else(|| "`params` must be an object".to_string())?;

    let action = parse_attestation_action(&action_name, params_obj)?;

    Ok(ParsedAttestation {
        action,
        issued_at,
        expires_at,
        nonce,
        signer_key_id,
        v,
    })
}

/// Map an `action` string + `params` to a v1 [`AttestationKind`], failing
/// closed on anything outside the allowlist.
fn parse_attestation_action(
    action: &str,
    params: &Map<String, Value>,
) -> Result<AttestationKind, String> {
    match action {
        "bridge.mint_wbth" => {
            const KNOWN: &[&str] = &[
                "amount",
                "destAddress",
                "destChain",
                "orderId",
                "safeNonce",
                "sourceTx",
            ];
            for key in params.keys() {
                if !KNOWN.contains(&key.as_str()) {
                    return Err(format!("unknown mint param `{key}`"));
                }
            }
            let dest_chain = parse_canonical_chain(get_str(params, "destChain")?)?;
            let safe_nonce = match params.get("safeNonce") {
                None => None,
                Some(v) => Some(
                    v.as_u64()
                        .ok_or_else(|| "`safeNonce` must be a non-negative integer".to_string())?,
                ),
            };
            match dest_chain {
                Chain::Bth => return Err("cannot mint wBTH on the BTH chain".to_string()),
                Chain::Ethereum if safe_nonce.is_none() => {
                    return Err("Ethereum mint attestations require `safeNonce`".to_string())
                }
                Chain::Solana if safe_nonce.is_some() => {
                    return Err("`safeNonce` is only valid for Ethereum mints".to_string())
                }
                _ => {}
            }
            Ok(AttestationKind::MintWbth {
                dest_chain,
                dest_address: get_str(params, "destAddress")?.to_string(),
                amount: get_u64(params, "amount")?,
                order_id: parse_uuid(get_str(params, "orderId")?)?,
                source_tx: get_str(params, "sourceTx")?.to_string(),
                safe_nonce,
            })
        }
        "bridge.release_bth" => {
            const KNOWN: &[&str] = &["amount", "bthAddress", "orderId", "sourceChain", "sourceTx"];
            for key in params.keys() {
                if !KNOWN.contains(&key.as_str()) {
                    return Err(format!("unknown release param `{key}`"));
                }
            }
            let source_chain = parse_canonical_chain(get_str(params, "sourceChain")?)?;
            if source_chain == Chain::Bth {
                return Err("release source chain cannot be bth".to_string());
            }
            Ok(AttestationKind::ReleaseBth {
                source_chain,
                bth_address: get_str(params, "bthAddress")?.to_string(),
                amount: get_u64(params, "amount")?,
                order_id: parse_uuid(get_str(params, "orderId")?)?,
                source_tx: get_str(params, "sourceTx")?.to_string(),
            })
        }
        other => Err(format!("action `{other}` is not in the v1 allowlist")),
    }
}

fn parse_uuid(s: &str) -> Result<Uuid, String> {
    Uuid::parse_str(s).map_err(|_| "`orderId` is not a valid UUID".to_string())
}

fn get_str<'a>(obj: &'a Map<String, Value>, key: &str) -> Result<&'a str, String> {
    obj.get(key)
        .ok_or_else(|| format!("missing field `{key}`"))?
        .as_str()
        .ok_or_else(|| format!("`{key}` must be a string"))
}

/// Read an integer field, REJECTING non-integers (`serde_json` parses `5.0`
/// as a float, so `as_u64` returns `None` — fail-closed).
fn get_u64(obj: &Map<String, Value>, key: &str) -> Result<u64, String> {
    obj.get(key)
        .ok_or_else(|| format!("missing field `{key}`"))?
        .as_u64()
        .ok_or_else(|| format!("`{key}` must be a non-negative integer"))
}

/// Reject any JSON value containing a duplicated object key at ANY nesting
/// level. `serde_json::Map` silently collapses duplicates (last-wins), which
/// would let two logical envelopes share one signed byte string.
fn reject_duplicate_keys(bytes: &str) -> Result<(), String> {
    use serde::de::{DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};

    struct AnyChecker;
    impl<'de> DeserializeSeed<'de> for AnyChecker {
        type Value = ();
        fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
            d.deserialize_any(AnyVisitor)
        }
    }

    struct AnyVisitor;
    impl<'de> Visitor<'de> for AnyVisitor {
        type Value = ();
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("any JSON value with unique object keys")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
            let mut seen = std::collections::HashSet::new();
            while let Some(key) = map.next_key::<String>()? {
                map.next_value_seed(AnyChecker)?;
                if !seen.insert(key.clone()) {
                    return Err(serde::de::Error::custom(format!("duplicate key `{key}`")));
                }
            }
            Ok(())
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
            while seq.next_element_seed(AnyChecker)?.is_some() {}
            Ok(())
        }
        fn visit_bool<E>(self, _: bool) -> Result<(), E> {
            Ok(())
        }
        fn visit_i64<E>(self, _: i64) -> Result<(), E> {
            Ok(())
        }
        fn visit_u64<E>(self, _: u64) -> Result<(), E> {
            Ok(())
        }
        fn visit_f64<E>(self, _: f64) -> Result<(), E> {
            Ok(())
        }
        fn visit_str<E>(self, _: &str) -> Result<(), E> {
            Ok(())
        }
        fn visit_unit<E>(self) -> Result<(), E> {
            Ok(())
        }
    }

    let mut de = serde_json::Deserializer::from_str(bytes);
    AnyChecker
        .deserialize(&mut de)
        .map_err(|e| format!("{e}"))?;
    Ok(())
}

/// Freshness check: `issuedAt - skew <= now <= expiresAt` and
/// `expiresAt - issuedAt <= MAX_ATTESTATION_LIFETIME_SECS`. All arithmetic
/// is saturating so crafted timestamps can never panic. A captured
/// attestation expires and cannot be replayed after the window.
pub fn check_attestation_freshness(
    parsed: &ParsedAttestation,
    now: u64,
) -> Result<(), AttestationRejectReason> {
    if parsed.expires_at < parsed.issued_at {
        return Err(AttestationRejectReason::Stale(
            "expiresAt precedes issuedAt".to_string(),
        ));
    }
    if parsed.expires_at.saturating_sub(parsed.issued_at) > MAX_ATTESTATION_LIFETIME_SECS {
        return Err(AttestationRejectReason::Stale(format!(
            "lifetime exceeds {MAX_ATTESTATION_LIFETIME_SECS}s"
        )));
    }
    if now < parsed.issued_at.saturating_sub(ATTESTATION_CLOCK_SKEW_SECS) {
        return Err(AttestationRejectReason::Stale(
            "issuedAt is in the future".to_string(),
        ));
    }
    if now > parsed.expires_at {
        return Err(AttestationRejectReason::Stale("expired".to_string()));
    }
    Ok(())
}

/// Bind an attestation to its on-record [`BridgeOrder`]: every field the
/// federation attested must match the order the engine is processing. A
/// validly-signed attestation for order A presented against order B is
/// rejected here even with a fresh nonce.
pub fn check_order_binding(
    parsed: &ParsedAttestation,
    order: &BridgeOrder,
) -> Result<(), AttestationRejectReason> {
    let wrong = |d: String| Err(AttestationRejectReason::WrongOrder(d));
    let action = &parsed.action;

    if action.order_uuid() != order.id {
        return wrong(format!(
            "attestation is bound to order {}, not {}",
            action.order_uuid(),
            order.id
        ));
    }
    if action.amount() != order.net_amount() {
        return wrong(format!(
            "attestation authorizes {} picocredits, order nets {}",
            action.amount(),
            order.net_amount()
        ));
    }
    if action.recipient() != order.dest_address {
        return wrong("attestation recipient does not match order destination".to_string());
    }
    match order.source_tx.as_deref() {
        Some(tx) if tx == action.source_tx() => {}
        Some(_) => return wrong("attestation source tx does not match order".to_string()),
        None => return wrong("order has no confirmed source tx on record".to_string()),
    }

    match action {
        AttestationKind::MintWbth { dest_chain, .. } => {
            if order.order_type != OrderType::Mint {
                return wrong("mint attestation presented for a non-mint order".to_string());
            }
            if *dest_chain != order.dest_chain {
                return wrong(format!(
                    "attestation targets {}, order mints on {}",
                    dest_chain, order.dest_chain
                ));
            }
        }
        AttestationKind::ReleaseBth { source_chain, .. } => {
            if order.order_type != OrderType::Burn {
                return wrong("release attestation presented for a non-burn order".to_string());
            }
            if order.dest_chain != Chain::Bth {
                return wrong("release attestation for an order not paying out on BTH".to_string());
            }
            if *source_chain != order.source_chain {
                return wrong(format!(
                    "attestation sources from {}, order burned on {}",
                    source_chain, order.source_chain
                ));
            }
        }
    }

    Ok(())
}

/// Collects verified per-signer attestations for ONE `(order, action)` and
/// answers the threshold question. Deduplicates by signer identity: the
/// same federation member attesting twice (even with distinct nonces)
/// counts ONCE toward `t`.
#[derive(Debug, Clone)]
pub struct AttestationSet {
    order_id: Uuid,
    action: String,
    target_chain: Chain,
    /// Ethereum mints only: the Safe nonce every collected payload
    /// signature is bound to. Signatures over different Safe nonces cannot
    /// be combined into one `execTransaction`, so mismatches are rejected.
    safe_nonce: Option<u64>,
    entries: Vec<(String, AttestationSignature)>,
}

impl AttestationSet {
    /// Start a set keyed to `parsed`'s order, action, and target chain.
    pub fn for_attestation(parsed: &ParsedAttestation) -> Self {
        let safe_nonce = match &parsed.action {
            AttestationKind::MintWbth { safe_nonce, .. } => *safe_nonce,
            AttestationKind::ReleaseBth { .. } => None,
        };
        Self {
            order_id: parsed.action.order_uuid(),
            action: parsed.action.name().to_string(),
            target_chain: parsed.action.target_chain(),
            safe_nonce,
            entries: Vec::new(),
        }
    }

    /// Whether `parsed` belongs to this set (same order, action, chain, and
    /// — for Ethereum — the same Safe nonce).
    fn matches(&self, parsed: &ParsedAttestation) -> Result<(), String> {
        if parsed.action.order_uuid() != self.order_id
            || parsed.action.name() != self.action
            || parsed.action.target_chain() != self.target_chain
        {
            return Err("attestation does not match this set's (order, action)".to_string());
        }
        if let AttestationKind::MintWbth { safe_nonce, .. } = &parsed.action {
            if *safe_nonce != self.safe_nonce {
                return Err(format!(
                    "attestation is bound to Safe nonce {:?}, set collects {:?} — signatures \
                     over different Safe nonces cannot be combined",
                    safe_nonce, self.safe_nonce
                ));
            }
        }
        Ok(())
    }

    /// Insert a VERIFIED attestation's payload signature. Returns `Ok(true)`
    /// for a new distinct signer, `Ok(false)` if this signer already counted
    /// (double-count resistance), `Err` if the attestation does not belong
    /// to this set.
    pub fn insert(
        &mut self,
        parsed: &ParsedAttestation,
        signature: AttestationSignature,
    ) -> Result<bool, String> {
        self.matches(parsed)?;
        if self
            .entries
            .iter()
            .any(|(id, _)| *id == parsed.signer_key_id)
        {
            return Ok(false);
        }
        self.entries.push((parsed.signer_key_id.clone(), signature));
        Ok(true)
    }

    /// Distinct signers collected so far.
    pub fn distinct_signers(&self) -> u32 {
        self.entries.len() as u32
    }

    /// Whether `signer_key_id` has already been counted toward the
    /// threshold.
    pub fn contains_signer(&self, signer_key_id: &str) -> bool {
        self.entries.iter().any(|(id, _)| id == signer_key_id)
    }

    /// Whether at least `threshold` DISTINCT verified signers have attested.
    /// A zero threshold NEVER authorizes — a t-of-n federation always
    /// requires at least one signature.
    pub fn is_threshold_met(&self, threshold: u32) -> bool {
        threshold >= 1 && self.distinct_signers() >= threshold
    }

    /// The collected payload signatures (one per distinct signer), for
    /// assembly into a [`MintAuthorization`] / [`ReleaseAuthorization`].
    pub fn signatures(&self) -> Vec<AttestationSignature> {
        self.entries.iter().map(|(_, s)| s.clone()).collect()
    }

    /// The Safe nonce this set's Ethereum payload signatures are bound to.
    pub fn safe_nonce(&self) -> Option<u64> {
        self.safe_nonce
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

    // === #824 attestation envelope protocol ===

    fn signing_key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn mint_order_sol() -> BridgeOrder {
        let mut order = BridgeOrder::new_mint(
            Chain::Solana,
            1_000_000_000_000,
            1_000_000_000,
            "bth_deposit_addr".to_string(),
            "So11111111111111111111111111111111111111112".to_string(),
        );
        order.source_tx = Some("bth_deposit_tx".to_string());
        order
    }

    fn burn_order_from_eth() -> BridgeOrder {
        BridgeOrder::new_burn(
            Chain::Ethereum,
            1_000_000_000_000,
            1_000_000_000,
            "0x1234567890abcdef1234567890abcdef12345678".to_string(),
            "bth_user_stealth_addr".to_string(),
            "0xburntx".to_string(),
        )
    }

    fn mint_kind(order: &BridgeOrder) -> AttestationKind {
        AttestationKind::MintWbth {
            dest_chain: order.dest_chain,
            dest_address: order.dest_address.clone(),
            amount: order.net_amount(),
            order_id: order.id,
            source_tx: order.source_tx.clone().unwrap(),
            safe_nonce: if order.dest_chain == Chain::Ethereum {
                Some(7)
            } else {
                None
            },
        }
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

    fn now() -> u64 {
        1_800_000_000
    }

    fn sign(kind: &AttestationKind, sk: &SigningKey, nonce: &str) -> AttestationEnvelope {
        sign_attestation_ed25519(kind, sk, nonce, now(), now() + 120).unwrap()
    }

    #[test]
    fn test_mint_digest_golden_vector_and_binding() {
        use sha2::{Digest, Sha256};

        // Pinned construction: sha256(tag || order_id || amount_le ||
        // recipient_len_le || recipient). Must never change — in-flight
        // mint attestations bind to it.
        let order_id = [5u8; 32];
        let digest =
            mint_payload_digest(Chain::Solana, &order_id, 999_000_000_000, "sol_addr").unwrap();
        let expected: [u8; 32] = {
            let mut hasher = Sha256::new();
            hasher.update(b"botho-bridge-mint-sol-v1");
            hasher.update([5u8; 32]);
            hasher.update(999_000_000_000u64.to_le_bytes());
            hasher.update((b"sol_addr".len() as u64).to_le_bytes());
            hasher.update(b"sol_addr");
            hasher.finalize().into()
        };
        assert_eq!(digest, expected);

        // Binds every field.
        let base = mint_payload_digest(Chain::Solana, &[1u8; 32], 100, "addr").unwrap();
        assert_ne!(
            base,
            mint_payload_digest(Chain::Solana, &[2u8; 32], 100, "addr").unwrap()
        );
        assert_ne!(
            base,
            mint_payload_digest(Chain::Solana, &[1u8; 32], 101, "addr").unwrap()
        );
        assert_ne!(
            base,
            mint_payload_digest(Chain::Solana, &[1u8; 32], 100, "addr2").unwrap()
        );

        // Cross-chain and cross-action domain separation: the same fields
        // produce a DIFFERENT digest per destination chain, and differ from
        // the release digest — no signature is reusable across chains or
        // between mint and release.
        let eth = mint_payload_digest(Chain::Ethereum, &[1u8; 32], 100, "addr").unwrap();
        assert_ne!(base, eth);
        assert_ne!(base, release_payload_digest(&[1u8; 32], 100, "addr"));
        assert_ne!(eth, release_payload_digest(&[1u8; 32], 100, "addr"));

        // No BTH-destination mints.
        assert!(mint_payload_digest(Chain::Bth, &[1u8; 32], 100, "addr").is_err());
    }

    #[test]
    fn test_envelope_roundtrip_release_and_solana_mint() {
        let sk = signing_key(1);

        for (order, kind) in [(burn_order_from_eth(), None), (mint_order_sol(), Some(()))] {
            let kind = if kind.is_some() {
                mint_kind(&order)
            } else {
                release_kind(&order)
            };
            let env = sign(&kind, &sk, "0011223344556677");
            let parsed = env.verify_and_parse_ed25519(&sk.verifying_key()).unwrap();
            assert_eq!(parsed.action, kind);
            assert_eq!(parsed.v, ATTESTATION_ENVELOPE_VERSION);
            assert_eq!(parsed.nonce, "0011223344556677");
            assert_eq!(
                parsed.signer_key_id,
                hex::encode(sk.verifying_key().as_bytes())
            );
            assert!(check_attestation_freshness(&parsed, now()).is_ok());
            assert!(check_order_binding(&parsed, &order).is_ok());
        }
    }

    #[test]
    fn test_tampered_envelope_fails_signature() {
        let sk = signing_key(1);
        let order = burn_order_from_eth();
        let mut env = sign(&release_kind(&order), &sk, "aa");

        // Inflate the attested amount in the raw bytes without re-signing.
        env.envelope = env.envelope.replace("999000000000", "999000000001");
        assert_eq!(
            env.verify_and_parse_ed25519(&sk.verifying_key()),
            Err(AttestationRejectReason::BadSignature)
        );
    }

    #[test]
    fn test_wrong_key_fails_signature() {
        let sk = signing_key(1);
        let other = signing_key(2);
        let order = burn_order_from_eth();
        let env = sign(&release_kind(&order), &sk, "aa");
        assert_eq!(
            env.verify_and_parse_ed25519(&other.verifying_key()),
            Err(AttestationRejectReason::BadSignature)
        );
    }

    #[test]
    fn test_cross_domain_signature_reuse_fails() {
        use ed25519_dalek::Signer as _;

        // A signature produced under the WRONG chain domain (e.g. an
        // attacker replaying bytes signed for another chain's domain, or a
        // buggy signer) must not verify: the verifier derives the domain
        // from the envelope's own target chain.
        let sk = signing_key(1);
        let order = burn_order_from_eth();
        let kind = release_kind(&order); // target chain = Bth
        let signer_key_id = hex::encode(sk.verifying_key().as_bytes());
        let envelope =
            canonical_attestation_envelope(&kind, &signer_key_id, "aa", now(), now() + 120);

        // Sign under the SOLANA domain instead of the BTH domain.
        let wrong_msg = attestation_signed_message(Chain::Solana, envelope.as_bytes());
        let payload = kind.ed25519_payload_digest().unwrap();
        let env = AttestationEnvelope {
            envelope,
            signature_hex: hex::encode(sk.sign(&wrong_msg).to_bytes()),
            payload_signature_hex: hex::encode(sk.sign(&payload).to_bytes()),
        };
        assert_eq!(
            env.verify_and_parse_ed25519(&sk.verifying_key()),
            Err(AttestationRejectReason::BadSignature)
        );
    }

    #[test]
    fn test_ethereum_kind_rejected_on_ed25519_path() {
        let sk = signing_key(1);
        let mut order = mint_order_sol();
        order.dest_chain = Chain::Ethereum;
        order.dest_address = "0x1234567890abcdef1234567890abcdef12345678".to_string();
        let kind = mint_kind(&order);
        // Ethereum kinds cannot be Ed25519-signed at all.
        assert!(sign_attestation_ed25519(&kind, &sk, "aa", now(), now() + 120).is_err());

        // And a hand-built Ethereum-target envelope is refused pre-signature
        // by the Ed25519 verifier (routing error).
        let envelope = canonical_attestation_envelope(&kind, "someid", "aa", now(), now() + 120);
        let env = AttestationEnvelope {
            envelope,
            signature_hex: hex::encode([0u8; 64]),
            payload_signature_hex: hex::encode([0u8; 64]),
        };
        assert!(matches!(
            env.verify_and_parse_ed25519(&sk.verifying_key()),
            Err(AttestationRejectReason::Malformed(_))
        ));
    }

    #[test]
    fn test_payload_signature_over_wrong_digest_fails() {
        use ed25519_dalek::Signer as _;

        // Valid envelope signature, but the payload signature covers a
        // DIFFERENT digest (another amount): rejected — the authorization
        // artifact must be signed over exactly the attested payload.
        let sk = signing_key(1);
        let order = burn_order_from_eth();
        let kind = release_kind(&order);
        let mut env = sign(&kind, &sk, "aa");

        let wrong_digest =
            release_payload_digest(&kind.order_id_bytes(), kind.amount() + 1, kind.recipient());
        env.payload_signature_hex = hex::encode(sk.sign(&wrong_digest).to_bytes());
        assert_eq!(
            env.verify_and_parse_ed25519(&sk.verifying_key()),
            Err(AttestationRejectReason::BadSignature)
        );
    }

    #[test]
    fn test_parser_rejects_structural_attacks() {
        // Unknown top-level key.
        assert!(parse_attestation_envelope(
            "{\"action\":\"bridge.release_bth\",\"evil\":1,\"expiresAt\":10,\"issuedAt\":5,\
             \"nonce\":\"n\",\"params\":{},\"signerKeyId\":\"s\",\"v\":1}"
        )
        .unwrap_err()
        .contains("unknown envelope field"));

        // Duplicate top-level key (serde_json would silently last-win).
        assert!(parse_attestation_envelope(
            "{\"action\":\"bridge.release_bth\",\"action\":\"bridge.mint_wbth\",\
             \"expiresAt\":10,\"issuedAt\":5,\"nonce\":\"n\",\"params\":{},\
             \"signerKeyId\":\"s\",\"v\":1}"
        )
        .unwrap_err()
        .contains("duplicate key"));

        // Duplicate NESTED key inside params.
        let dup_nested = format!(
            "{{\"action\":\"bridge.release_bth\",\"expiresAt\":10,\"issuedAt\":5,\
             \"nonce\":\"n\",\"params\":{{\"amount\":1,\"amount\":2,\"bthAddress\":\"a\",\
             \"orderId\":\"{}\",\"sourceChain\":\"ethereum\",\"sourceTx\":\"t\"}},\
             \"signerKeyId\":\"s\",\"v\":1}}",
            Uuid::nil()
        );
        assert!(parse_attestation_envelope(&dup_nested)
            .unwrap_err()
            .contains("duplicate key"));

        // Unknown version: no downgrade path.
        assert!(parse_attestation_envelope(
            "{\"action\":\"bridge.release_bth\",\"expiresAt\":10,\"issuedAt\":5,\
             \"nonce\":\"n\",\"params\":{},\"signerKeyId\":\"s\",\"v\":2}"
        )
        .unwrap_err()
        .contains("unsupported envelope version"));

        // Floats are not integers.
        assert!(parse_attestation_envelope(
            "{\"action\":\"bridge.release_bth\",\"expiresAt\":10.0,\"issuedAt\":5,\
             \"nonce\":\"n\",\"params\":{},\"signerKeyId\":\"s\",\"v\":1}"
        )
        .unwrap_err()
        .contains("must be a non-negative integer"));

        // Unknown action.
        assert!(parse_attestation_envelope(
            "{\"action\":\"bridge.self_destruct\",\"expiresAt\":10,\"issuedAt\":5,\
             \"nonce\":\"n\",\"params\":{},\"signerKeyId\":\"s\",\"v\":1}"
        )
        .unwrap_err()
        .contains("not in the v1 allowlist"));

        // Chain aliases are rejected: one canonical encoding per attestation.
        let alias = format!(
            "{{\"action\":\"bridge.mint_wbth\",\"expiresAt\":10,\"issuedAt\":5,\
             \"nonce\":\"n\",\"params\":{{\"amount\":1,\"destAddress\":\"a\",\
             \"destChain\":\"sol\",\"orderId\":\"{}\",\"sourceTx\":\"t\"}},\
             \"signerKeyId\":\"s\",\"v\":1}}",
            Uuid::nil()
        );
        assert!(parse_attestation_envelope(&alias)
            .unwrap_err()
            .contains("not a canonical chain name"));

        // safeNonce rules: required for Ethereum, forbidden for Solana.
        let eth_missing = format!(
            "{{\"action\":\"bridge.mint_wbth\",\"expiresAt\":10,\"issuedAt\":5,\
             \"nonce\":\"n\",\"params\":{{\"amount\":1,\"destAddress\":\"a\",\
             \"destChain\":\"ethereum\",\"orderId\":\"{}\",\"sourceTx\":\"t\"}},\
             \"signerKeyId\":\"s\",\"v\":1}}",
            Uuid::nil()
        );
        assert!(parse_attestation_envelope(&eth_missing)
            .unwrap_err()
            .contains("require `safeNonce`"));
        let sol_with_nonce = format!(
            "{{\"action\":\"bridge.mint_wbth\",\"expiresAt\":10,\"issuedAt\":5,\
             \"nonce\":\"n\",\"params\":{{\"amount\":1,\"destAddress\":\"a\",\
             \"destChain\":\"solana\",\"orderId\":\"{}\",\"safeNonce\":3,\
             \"sourceTx\":\"t\"}},\"signerKeyId\":\"s\",\"v\":1}}",
            Uuid::nil()
        );
        assert!(parse_attestation_envelope(&sol_with_nonce)
            .unwrap_err()
            .contains("only valid for Ethereum"));
    }

    #[test]
    fn test_freshness_window() {
        let sk = signing_key(1);
        let order = burn_order_from_eth();
        let kind = release_kind(&order);

        let fresh = |issued: u64, expires: u64| {
            sign_attestation_ed25519(&kind, &sk, "aa", issued, expires)
                .unwrap()
                .verify_and_parse_ed25519(&sk.verifying_key())
                .unwrap()
        };

        // Valid window.
        assert!(check_attestation_freshness(&fresh(now(), now() + 200), now()).is_ok());
        // Expired.
        assert!(matches!(
            check_attestation_freshness(&fresh(now() - 300, now() - 100), now()),
            Err(AttestationRejectReason::Stale(_))
        ));
        // Issued in the future beyond skew.
        assert!(matches!(
            check_attestation_freshness(&fresh(now() + 100, now() + 300), now()),
            Err(AttestationRejectReason::Stale(_))
        ));
        // Lifetime over the cap.
        assert!(matches!(
            check_attestation_freshness(&fresh(now(), now() + 400), now()),
            Err(AttestationRejectReason::Stale(_))
        ));
        // expiresAt precedes issuedAt (saturating, no panic).
        assert!(matches!(
            check_attestation_freshness(&fresh(now(), now() - 1), now()),
            Err(AttestationRejectReason::Stale(_))
        ));
    }

    #[test]
    fn test_order_binding_rejects_cross_order_reuse() {
        let sk = signing_key(1);
        let order_a = burn_order_from_eth();
        let order_b = burn_order_from_eth(); // fresh UUID, same shape

        let env = sign(&release_kind(&order_a), &sk, "aa");
        let parsed = env.verify_and_parse_ed25519(&sk.verifying_key()).unwrap();

        // Bound to order A: order B is rejected even though every other
        // field (amount, recipient, chains, source tx) matches.
        assert!(check_order_binding(&parsed, &order_a).is_ok());
        assert!(matches!(
            check_order_binding(&parsed, &order_b),
            Err(AttestationRejectReason::WrongOrder(_))
        ));
    }

    #[test]
    fn test_order_binding_rejects_field_mismatches() {
        let sk = signing_key(1);
        let order = burn_order_from_eth();
        let env = sign(&release_kind(&order), &sk, "aa");
        let parsed = env.verify_and_parse_ed25519(&sk.verifying_key()).unwrap();

        let wrong = |mutate: &dyn Fn(&mut BridgeOrder)| {
            let mut o = order.clone();
            mutate(&mut o);
            matches!(
                check_order_binding(&parsed, &o),
                Err(AttestationRejectReason::WrongOrder(_))
            )
        };

        assert!(wrong(&|o| o.fee += 1)); // net amount shifts
        assert!(wrong(&|o| o.dest_address = "attacker_addr".to_string()));
        assert!(wrong(&|o| o.source_tx = Some("0xothertx".to_string())));
        assert!(wrong(&|o| o.source_tx = None));
        assert!(wrong(&|o| o.source_chain = Chain::Solana));
        assert!(wrong(&|o| o.order_type = OrderType::Mint));
    }

    #[test]
    fn test_mint_attestation_does_not_bind_to_other_chain_order() {
        // A Solana mint attestation presented against an (otherwise
        // identical) Ethereum order fails order binding: chain/domain +
        // order binding together make cross-chain replay impossible.
        let sk = signing_key(1);
        let sol_order = mint_order_sol();
        let env = sign(&mint_kind(&sol_order), &sk, "aa");
        let parsed = env.verify_and_parse_ed25519(&sk.verifying_key()).unwrap();

        let mut eth_order = sol_order.clone();
        eth_order.dest_chain = Chain::Ethereum;
        assert!(matches!(
            check_order_binding(&parsed, &eth_order),
            Err(AttestationRejectReason::WrongOrder(_))
        ));
    }

    #[test]
    fn test_attestation_set_thresholding_and_double_count() {
        let order = burn_order_from_eth();
        let kind = release_kind(&order);
        let (k1, k2) = (signing_key(1), signing_key(2));

        let parse = |sk: &SigningKey, nonce: &str| {
            sign(&kind, sk, nonce)
                .verify_and_parse_ed25519(&sk.verifying_key())
                .unwrap()
        };
        let sig_of = |sk: &SigningKey| AttestationSignature {
            signer: sk.verifying_key().as_bytes().to_vec(),
            signature: vec![0u8; 64],
        };

        let p1 = parse(&k1, "n1");
        let mut set = AttestationSet::for_attestation(&p1);
        assert!(!set.is_threshold_met(2));
        assert!(!set.is_threshold_met(0), "threshold 0 NEVER authorizes");

        assert!(set.insert(&p1, sig_of(&k1)).unwrap());
        assert_eq!(set.distinct_signers(), 1);
        assert!(!set.is_threshold_met(2), "t-1 signers must not authorize");
        assert!(!set.is_threshold_met(0), "threshold 0 NEVER authorizes");

        // The SAME signer with a fresh nonce counts ONCE.
        let p1b = parse(&k1, "n2");
        assert!(!set.insert(&p1b, sig_of(&k1)).unwrap());
        assert_eq!(set.distinct_signers(), 1);
        assert!(!set.is_threshold_met(2));

        // The t-th DISTINCT signer flips it to authorized.
        let p2 = parse(&k2, "n3");
        assert!(set.insert(&p2, sig_of(&k2)).unwrap());
        assert!(set.is_threshold_met(2));
        assert_eq!(set.signatures().len(), 2);

        // A different order's attestation cannot enter this set.
        let other = burn_order_from_eth();
        let env = sign(&release_kind(&other), &k1, "n4");
        let p_other = env.verify_and_parse_ed25519(&k1.verifying_key()).unwrap();
        assert!(set.insert(&p_other, sig_of(&k1)).is_err());
    }

    #[test]
    fn test_reject_taxonomy_authentication_split() {
        // Pre-signature reasons are unauthenticated (rate-limit + count
        // only); post-signature reasons are audit-logged.
        for r in [
            AttestationRejectReason::NotConfigured,
            AttestationRejectReason::UnknownSigner,
            AttestationRejectReason::Malformed("x".into()),
            AttestationRejectReason::BadSignature,
        ] {
            assert!(!r.is_authenticated(), "{:?}", r);
        }
        for r in [
            AttestationRejectReason::WrongOrder("x".into()),
            AttestationRejectReason::Stale("x".into()),
            AttestationRejectReason::ReplayedNonce,
            AttestationRejectReason::BelowThreshold("x".into()),
            AttestationRejectReason::InvalidPayload("x".into()),
        ] {
            assert!(r.is_authenticated(), "{:?}", r);
        }
    }

    #[test]
    fn test_peeks_match_parsed_values() {
        let sk = signing_key(1);
        let order = mint_order_sol();
        let env = sign(&mint_kind(&order), &sk, "aa");

        assert_eq!(
            peek_signer_key_id(&env.envelope).unwrap(),
            hex::encode(sk.verifying_key().as_bytes())
        );
        assert_eq!(peek_target_chain(&env.envelope).unwrap(), Chain::Solana);
        assert!(peek_target_chain("not json").is_err());
    }
}
