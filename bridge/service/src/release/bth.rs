// Copyright (c) 2024 The Botho Foundation

//! BTH reserve release (threshold-signed reserve spend, ADR 0002).
//!
//! On a confirmed wBTH burn, the bridge pays `net_amount()` picocredits
//! from the locked BTH reserve to the burn's `bthAddress`, as a **fresh
//! one-time stealth output** (ADR 0004), spending only **factor-1 /
//! background-provenance** reserve outputs (ADR 0003) so the release cannot
//! launder cluster provenance and the change keeps the reserve
//! zero-demurrage.
//!
//! ## Implementation status
//!
//! The deterministic, consensus-critical pieces are implemented and
//! unit-tested here:
//!
//! - **Federation attestation verification**
//!   ([`validate_release_attestation`]): real Ed25519 verification of every
//!   signature over the domain-separated release digest
//!   ([`bth_bridge_core::release_payload_digest`]), bound to the exact order id
//!   + amount + recipient; federation-membership and distinct-signer threshold
//!   checks. No reserve key material is touched until this passes — no single
//!   node can spend the reserve.
//! - **Configuration validation** ([`BthReleaser::new`]): reserve address
//!   present, federation keys parse to valid Ed25519 points, threshold is
//!   satisfiable by the configured signer set.
//!
//! The RPC-dependent bodies (reserve UTXO scan, transaction construction,
//! CLSAG signing, `tx_submit`, confirmation polling) are `TODO(#828)` stubs
//! returning [`ReleaseError::NotImplemented`] — they need a live BTH node
//! and the wallet transaction-builder stack, wired and validated end to end
//! by the #828 test harness. Each stub documents the exact pipeline it must
//! implement; the engine-side exactly-once machinery (release claims,
//! record-before-broadcast, submit/confirm split) is fully live and tested
//! against a mock releaser.

use async_trait::async_trait;
use bth_bridge_core::{BridgeOrder, BthConfig, ReleaseAuthorization};
use ed25519_dalek::{Signature, VerifyingKey};

use super::{PreparedRelease, ReleaseConfirmation, ReleaseError, Releaser};

/// Validate that `auth` authorizes releasing `order`'s BTH, verifying every
/// federation signature.
///
/// Checks, in order:
/// 1. the authorization is bound to THIS order's deterministic on-chain id (a
///    replayed authorization for a different order is rejected);
/// 2. the authorized amount equals the order's `net_amount()` and the recipient
///    equals the order's `dest_address` (the digest binds both, but the
///    explicit field check gives a precise error before verifying);
/// 3. the claimed threshold is at least the configured federation floor
///    (`bth.release_threshold`, per ADR 0002 never lower than the SCP safety
///    threshold);
/// 4. every signature is a valid Ed25519 signature by a configured federation
///    member over the domain-separated release digest; and
/// 5. at least `threshold` DISTINCT federation members signed (duplicate or
///    repeated signers count once).
pub fn validate_release_attestation(
    order: &BridgeOrder,
    auth: &ReleaseAuthorization,
    federation: &[VerifyingKey],
    threshold_floor: u32,
) -> Result<(), ReleaseError> {
    if auth.order_id != order.order_id_bytes() {
        return Err(ReleaseError::Attestation(
            "attestation order id does not match order".to_string(),
        ));
    }
    if auth.amount != order.net_amount() {
        return Err(ReleaseError::Attestation(format!(
            "attestation authorizes {} picocredits, order releases {}",
            auth.amount,
            order.net_amount()
        )));
    }
    if auth.recipient != order.dest_address {
        return Err(ReleaseError::Attestation(
            "attestation recipient does not match order destination".to_string(),
        ));
    }
    if auth.threshold < threshold_floor {
        return Err(ReleaseError::Attestation(format!(
            "attestation threshold {} is below the configured federation threshold {}",
            auth.threshold, threshold_floor
        )));
    }

    let digest = auth.digest();
    let mut valid_signers: Vec<[u8; 32]> = Vec::with_capacity(auth.signatures.len());

    for sig in &auth.signatures {
        let signer_bytes: [u8; 32] = sig.signer.as_slice().try_into().map_err(|_| {
            ReleaseError::Attestation(format!(
                "ed25519 signer must be a 32-byte pubkey, got {} bytes",
                sig.signer.len()
            ))
        })?;
        let sig_bytes: [u8; 64] = sig.signature.as_slice().try_into().map_err(|_| {
            ReleaseError::Attestation(format!(
                "ed25519 signature must be 64 bytes, got {}",
                sig.signature.len()
            ))
        })?;

        // Federation membership: an empty configured set is development
        // mode (no membership pinning); otherwise every signer must be a
        // configured validator key.
        let key = if federation.is_empty() {
            VerifyingKey::from_bytes(&signer_bytes).map_err(|e| {
                ReleaseError::Attestation(format!("invalid ed25519 public key: {}", e))
            })?
        } else {
            *federation
                .iter()
                .find(|k| k.as_bytes() == &signer_bytes)
                .ok_or_else(|| {
                    ReleaseError::Attestation(format!(
                        "signer {} is not a configured federation member",
                        hex::encode(signer_bytes)
                    ))
                })?
        };

        // Every carried signature must verify — a single forged signature
        // fails the whole attestation rather than being silently skipped.
        key.verify_strict(&digest, &Signature::from_bytes(&sig_bytes))
            .map_err(|e| {
                ReleaseError::Attestation(format!(
                    "signature by {} does not verify: {}",
                    hex::encode(signer_bytes),
                    e
                ))
            })?;

        if !valid_signers.contains(&signer_bytes) {
            valid_signers.push(signer_bytes);
        }
    }

    if (valid_signers.len() as u32) < auth.threshold {
        return Err(ReleaseError::Attestation(format!(
            "attestation has {} distinct valid signer(s), threshold is {}",
            valid_signers.len(),
            auth.threshold
        )));
    }

    Ok(())
}

/// BTH reserve-release backend.
///
/// See the module docs: attestation verification and configuration
/// validation are live; transaction construction / submission /
/// confirmation are `TODO(#828)` stubs.
pub struct BthReleaser {
    config: BthConfig,
    /// Parsed federation verifying keys (from `bth.release_signers`).
    federation: Vec<VerifyingKey>,
}

impl BthReleaser {
    /// Build a releaser from configuration. Does not perform network I/O.
    ///
    /// Fails (disabling release submission — burn orders stay
    /// `BurnConfirmed`, never dropped) if the reserve address is missing,
    /// any federation key is malformed, or the threshold exceeds the
    /// configured signer count.
    pub fn new(config: BthConfig) -> Result<Self, ReleaseError> {
        if config.reserve_address.as_deref().unwrap_or("").is_empty() {
            return Err(ReleaseError::Config(
                "bth.reserve_address is not configured".to_string(),
            ));
        }

        let mut federation = Vec::with_capacity(config.release_signers.len());
        for hex_key in &config.release_signers {
            let bytes: [u8; 32] = hex::decode(hex_key)
                .map_err(|e| {
                    ReleaseError::Config(format!("bad federation key hex {}: {}", hex_key, e))
                })?
                .try_into()
                .map_err(|_| {
                    ReleaseError::Config(format!("federation key {} is not 32 bytes", hex_key))
                })?;
            let key = VerifyingKey::from_bytes(&bytes).map_err(|e| {
                ReleaseError::Config(format!("federation key {} is invalid: {}", hex_key, e))
            })?;
            federation.push(key);
        }

        if !federation.is_empty() && config.release_threshold as usize > federation.len() {
            return Err(ReleaseError::Config(format!(
                "release_threshold {} exceeds the {} configured federation signer(s)",
                config.release_threshold,
                federation.len()
            )));
        }

        Ok(Self { config, federation })
    }

    /// The confirmation depth a release must reach before `Released`
    /// (0 = SCP externalization finality).
    #[allow(dead_code)]
    pub fn required_confirmations(&self) -> u32 {
        self.config.release_confirmations_required
    }
}

#[async_trait]
impl Releaser for BthReleaser {
    async fn prepare_release(
        &self,
        order: &BridgeOrder,
        auth: &ReleaseAuthorization,
    ) -> Result<PreparedRelease, ReleaseError> {
        // Threshold authorization FIRST: no reserve spend is ever
        // constructed without a verified federation attestation bound to
        // this exact order id, amount, and recipient (ADR 0002).
        validate_release_attestation(order, auth, &self.federation, self.config.release_threshold)?;

        // TODO(#828): construct and sign the release transaction against a
        // live BTH node. The pipeline (mirroring the web wallet's
        // send flow in web/packages/wasm-signer/src/send.ts and
        // botho/src/commands/send.rs):
        //   1. Load reserve-owned outputs: `chain_getOutputs` + ownership scan with the
        //      reserve view key (scanOwnedOutputs), derive key images, and drop
        //      spent/pending ones via `chain_areKeyImagesSpent`
        //      (spendableOwnedOutputs).
        //   2. Filter to FACTOR-1 / BACKGROUND-provenance outputs only (ADR 0003): keep
        //      outputs whose ClusterTagVector computes to factor 1x
        //      (transaction/types/src/cluster_tags.rs,
        //      transaction/core/src/validation/cluster_fee.rs) so the reserve never
        //      spends — or launders — wealthy-cluster coins.
        //   3. Greedily select inputs covering net_amount() + the BTH fee.
        //   4. Build a FRESH one-time stealth output to auth.recipient (ADR 0004):
        //      decode the address to a RecipientAddress and derive a per-release
        //      one-time key (TxOutput::new_with_memo in botho/src/commands/send.rs) —
        //      never a static payout key; two releases to the same address must produce
        //      distinct one-time keys. Assert the recipient output carries NO cluster
        //      tags.
        //   5. Route change back to self.config.reserve_address, preserving
        //      factor-1/background provenance for future releases.
        //   6. Gather ring decoys for each input from the on-chain pool, excluding the
        //      real inputs and the genesis placeholder (decoy-gathering + SpendInput
        //      assembly in send.ts).
        //   7. Threshold-sign per #824 (t-of-n federation signing of the tx digest),
        //      produce the node-verifiable CLSAG-signed tx (build_and_sign_inner in
        //      web/packages/wasm-signer/src/lib.rs), and SELF-VERIFY before returning.
        //   8. Return PreparedRelease { tx_hash, raw } — the engine persists BOTH
        //      before the first broadcast so a crash after signing resumes with the
        //      same signed tx and never re-signs with new inputs (double-spend risk).
        Err(ReleaseError::NotImplemented(
            "BTH release tx construction pending #824 (federation signing) and #828 \
             (live-node wallet-send wiring)"
                .to_string(),
        ))
    }

    async fn broadcast(&self, _prepared: &PreparedRelease) -> Result<(), ReleaseError> {
        // TODO(#828): submit via the node's `tx_submit` JSON-RPC
        // (botho/src/rpc/mod.rs) at self.config.rpc_url. Idempotent: an
        // "already known" / duplicate-key-image-for-the-same-tx rejection of
        // our OWN recorded hash is success (the tx was submitted before a
        // restart).
        Err(ReleaseError::NotImplemented(
            "BTH tx_submit wiring pending #828".to_string(),
        ))
    }

    async fn check_confirmation(
        &self,
        _order: &BridgeOrder,
        _dest_tx: &str,
    ) -> Result<ReleaseConfirmation, ReleaseError> {
        // TODO(#828): poll the node for the tx's block inclusion and depth:
        //   - depth >= self.config.release_confirmations_required (0 = SCP
        //     externalization finality: in-a-block == final) -> Confirmed.
        //   - not yet included -> Pending { confirmations }.
        //   - PROVABLY dead only (key images spent by a DIFFERENT tx per
        //     `chain_areKeyImagesSpent`, or permanently invalid) -> Dropped, which
        //     unwinds ReleasePending -> BurnConfirmed for a fresh submission. A
        //     merely-unseen tx must stay Pending and be re-broadcast — BTH has no
        //     on-chain order-id guard, so re-signing while the old tx could still land
        //     risks a double release.
        Err(ReleaseError::NotImplemented(
            "BTH confirmation polling pending #828".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bth_bridge_core::{AttestationSignature, Chain};
    use ed25519_dalek::{Signer, SigningKey};

    fn test_config(signers: &[&SigningKey], threshold: u32) -> BthConfig {
        BthConfig {
            rpc_url: "http://localhost:7101".to_string(),
            ws_url: "ws://localhost:7101/ws".to_string(),
            view_key_file: None,
            spend_key_file: None,
            confirmations_required: 0,
            reserve_address: Some("bth_reserve_addr".to_string()),
            release_signers: signers
                .iter()
                .map(|k| hex::encode(k.verifying_key().as_bytes()))
                .collect(),
            release_threshold: threshold,
            release_confirmations_required: 0,
        }
    }

    fn burn_order() -> BridgeOrder {
        let mut order = BridgeOrder::new_burn(
            Chain::Ethereum,
            1_000_000_000_000,
            1_000_000_000,
            "0x1234567890abcdef1234567890abcdef12345678".to_string(),
            "bth_user_stealth_addr".to_string(),
            "0xburntx".to_string(),
        );
        order.set_status(bth_bridge_core::OrderStatus::BurnConfirmed);
        order
    }

    fn signing_key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn signed_auth(
        order: &BridgeOrder,
        keys: &[&SigningKey],
        threshold: u32,
    ) -> ReleaseAuthorization {
        let mut auth = ReleaseAuthorization {
            order_id: order.order_id_bytes(),
            amount: order.net_amount(),
            recipient: order.dest_address.clone(),
            threshold,
            signatures: vec![],
        };
        let digest = auth.digest();
        for key in keys {
            auth.signatures.push(AttestationSignature {
                signer: key.verifying_key().as_bytes().to_vec(),
                signature: key.sign(&digest).to_bytes().to_vec(),
            });
        }
        auth
    }

    fn federation(keys: &[&SigningKey]) -> Vec<VerifyingKey> {
        keys.iter().map(|k| k.verifying_key()).collect()
    }

    #[test]
    fn test_valid_threshold_attestation_accepted() {
        let (k1, k2, k3) = (signing_key(1), signing_key(2), signing_key(3));
        let order = burn_order();
        let auth = signed_auth(&order, &[&k1, &k2], 2);
        let fed = federation(&[&k1, &k2, &k3]);

        assert!(validate_release_attestation(&order, &auth, &fed, 2).is_ok());
    }

    #[test]
    fn test_below_threshold_rejected() {
        let (k1, k2) = (signing_key(1), signing_key(2));
        let order = burn_order();
        let fed = federation(&[&k1, &k2]);

        // Fewer than t signatures.
        let auth = signed_auth(&order, &[&k1], 2);
        assert!(matches!(
            validate_release_attestation(&order, &auth, &fed, 2),
            Err(ReleaseError::Attestation(_))
        ));

        // The SAME signer twice does not meet a threshold of 2.
        let auth = signed_auth(&order, &[&k1, &k1], 2);
        assert!(matches!(
            validate_release_attestation(&order, &auth, &fed, 2),
            Err(ReleaseError::Attestation(_))
        ));

        // A claimed threshold below the configured federation floor is
        // rejected even if its own signature count is met.
        let auth = signed_auth(&order, &[&k1], 1);
        assert!(matches!(
            validate_release_attestation(&order, &auth, &fed, 2),
            Err(ReleaseError::Attestation(_))
        ));
    }

    #[test]
    fn test_wrong_binding_rejected() {
        let (k1, k2) = (signing_key(1), signing_key(2));
        let order = burn_order();
        let fed = federation(&[&k1, &k2]);

        // Bound to a DIFFERENT order id (replay from another order).
        let mut auth = signed_auth(&order, &[&k1, &k2], 2);
        auth.order_id = [0u8; 32];
        assert!(matches!(
            validate_release_attestation(&order, &auth, &fed, 2),
            Err(ReleaseError::Attestation(_))
        ));

        // Amount tampered after signing: field check catches the mismatch
        // with the order before any signature is even inspected.
        let mut auth = signed_auth(&order, &[&k1, &k2], 2);
        auth.amount += 1;
        assert!(matches!(
            validate_release_attestation(&order, &auth, &fed, 2),
            Err(ReleaseError::Attestation(_))
        ));

        // Recipient tampered after signing.
        let mut auth = signed_auth(&order, &[&k1, &k2], 2);
        auth.recipient = "attacker_addr".to_string();
        assert!(matches!(
            validate_release_attestation(&order, &auth, &fed, 2),
            Err(ReleaseError::Attestation(_))
        ));
    }

    #[test]
    fn test_signature_over_wrong_digest_rejected() {
        let (k1, k2) = (signing_key(1), signing_key(2));
        let order = burn_order();
        let fed = federation(&[&k1, &k2]);

        // Signatures made over a DIFFERENT payload (stale authorization for
        // another amount) pasted into an auth whose fields match the order:
        // Ed25519 verification over the digest catches it.
        let mut stale = signed_auth(&order, &[&k1, &k2], 2);
        stale.amount = order.net_amount() + 5; // signed digest binds this
        let digest = stale.digest();
        let mut auth = signed_auth(&order, &[], 2);
        for key in [&k1, &k2] {
            auth.signatures.push(AttestationSignature {
                signer: key.verifying_key().as_bytes().to_vec(),
                signature: key.sign(&digest).to_bytes().to_vec(),
            });
        }
        assert!(matches!(
            validate_release_attestation(&order, &auth, &fed, 2),
            Err(ReleaseError::Attestation(_))
        ));
    }

    #[test]
    fn test_non_federation_signer_rejected() {
        let (k1, k2, outsider) = (signing_key(1), signing_key(2), signing_key(9));
        let order = burn_order();
        let fed = federation(&[&k1, &k2]);

        // A valid signature from a key OUTSIDE the configured federation
        // must be rejected, even alongside a valid member signature.
        let auth = signed_auth(&order, &[&k1, &outsider], 2);
        assert!(matches!(
            validate_release_attestation(&order, &auth, &fed, 2),
            Err(ReleaseError::Attestation(_))
        ));
    }

    #[test]
    fn test_malformed_signature_material_rejected() {
        let (k1, k2) = (signing_key(1), signing_key(2));
        let order = burn_order();
        let fed = federation(&[&k1, &k2]);

        let mut auth = signed_auth(&order, &[&k1, &k2], 2);
        auth.signatures[0].signer = vec![1u8; 16]; // not 32 bytes
        assert!(matches!(
            validate_release_attestation(&order, &auth, &fed, 2),
            Err(ReleaseError::Attestation(_))
        ));

        let mut auth = signed_auth(&order, &[&k1, &k2], 2);
        auth.signatures[0].signature = vec![0u8; 63]; // not 64 bytes
        assert!(matches!(
            validate_release_attestation(&order, &auth, &fed, 2),
            Err(ReleaseError::Attestation(_))
        ));

        // A corrupted (non-verifying) signature fails the WHOLE attestation
        // even when enough other valid signatures exist.
        let k3 = signing_key(3);
        let fed3 = federation(&[&k1, &k2, &k3]);
        let mut auth = signed_auth(&order, &[&k1, &k2, &k3], 2);
        auth.signatures[2].signature = vec![0u8; 64];
        assert!(matches!(
            validate_release_attestation(&order, &auth, &fed3, 2),
            Err(ReleaseError::Attestation(_))
        ));
    }

    #[test]
    fn test_releaser_config_validation() {
        let (k1, k2) = (signing_key(1), signing_key(2));

        // Valid.
        assert!(BthReleaser::new(test_config(&[&k1, &k2], 2)).is_ok());

        // Missing reserve address disables release submission.
        let mut cfg = test_config(&[&k1, &k2], 2);
        cfg.reserve_address = None;
        assert!(matches!(
            BthReleaser::new(cfg),
            Err(ReleaseError::Config(_))
        ));

        // Threshold unsatisfiable by the configured signer set.
        let cfg = test_config(&[&k1, &k2], 3);
        assert!(matches!(
            BthReleaser::new(cfg),
            Err(ReleaseError::Config(_))
        ));

        // Malformed federation key.
        let mut cfg = test_config(&[&k1], 1);
        cfg.release_signers = vec!["zz-not-hex".to_string()];
        assert!(matches!(
            BthReleaser::new(cfg),
            Err(ReleaseError::Config(_))
        ));
    }

    #[tokio::test]
    async fn test_prepare_release_gates_on_attestation_before_stub() {
        let (k1, k2) = (signing_key(1), signing_key(2));
        let releaser = BthReleaser::new(test_config(&[&k1, &k2], 2)).unwrap();
        let order = burn_order();

        // A bad attestation is rejected as Attestation (the gate fires
        // BEFORE the NotImplemented construction stub).
        let auth = signed_auth(&order, &[&k1], 2);
        assert!(matches!(
            releaser.prepare_release(&order, &auth).await,
            Err(ReleaseError::Attestation(_))
        ));

        // A valid attestation reaches the #828 construction stub.
        let auth = signed_auth(&order, &[&k1, &k2], 2);
        assert!(matches!(
            releaser.prepare_release(&order, &auth).await,
            Err(ReleaseError::NotImplemented(_))
        ));
    }
}
