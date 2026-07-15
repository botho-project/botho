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
//!   ([`bth_bridge_core::release_payload_digest`]), bound to the exact order
//!   id, amount, and recipient; federation-membership and distinct-signer
//!   threshold checks. No reserve key material is touched until this passes —
//!   no single node can spend the reserve.
//! - **Configuration validation** ([`BthReleaser::new`]): reserve address
//!   present, federation keys parse to valid Ed25519 points, threshold is
//!   satisfiable by the configured signer set.
//!
//! The RPC-dependent stages are now LIVE (#856), reusing the node-identical
//! crypto in [`bth_transaction_clsag`] via [`crate::bth_scan`]:
//!
//! - **`prepare_release`**: after the attestation gate, scan the recent reserve
//!   window (`chain_getOutputs`) for spendable, factor-1, reserve-owned outputs
//!   (ADR 0003; spent/pending inputs dropped via `chain_areKeyImagesSpent`),
//!   select inputs, gather ring decoys, and build + CLSAG-sign a tx paying a
//!   FRESH one-time stealth output to the recipient (ADR 0004) with change back
//!   to the reserve. The signed tx is self-verified against the node's verifier
//!   before it is returned.
//! - **`broadcast`**: submit the bincode wire bytes via `tx_submit`; idempotent
//!   (an "already known" / duplicate-key-image rejection of our OWN recorded tx
//!   is success on resume).
//! - **`check_confirmation`**: poll `getTransaction` for the configured depth
//!   (`release_confirmations_required`; 0 = SCP externalization finality).
//!
//! Without a configured RPC URL + reserve key files the stages fail safe
//! (return [`ReleaseError::NotImplemented`] / leave orders `BurnConfirmed`),
//! exactly as the pre-#856 stubs did. The transport is exercised by native
//! unit tests (attestation gating here, tx construction in
//! [`crate::bth_scan`]) plus an `#[ignore]`d live-node test
//! (`crate::bth_fork_tests`). The engine-side exactly-once machinery (release
//! claims, record-before-broadcast, submit/confirm split) is fully live and
//! tested against a mock releaser.

use async_trait::async_trait;
use bth_bridge_core::{BridgeOrder, BthConfig, ReleaseAuthorization};
use bth_crypto_ring_signature::KeyImage;
use bth_transaction_clsag::{TxOutput, MIN_TX_FEE};
use ed25519_dalek::{Signature, VerifyingKey};
use tracing::{debug, info, warn};

use super::{PreparedRelease, ReleaseConfirmation, ReleaseError, Releaser};
use crate::{
    bth_keys::ReserveKeys,
    bth_rpc::{BthNodeRpc, RpcError, TxInclusion},
    bth_scan::{
        build_release_tx, decode_recipient_address, scan_deposit_output, OwnedOutput, ReleaseInput,
    },
};

impl From<RpcError> for ReleaseError {
    fn from(e: RpcError) -> Self {
        match e {
            RpcError::Transport(m) | RpcError::Decode(m) => ReleaseError::Rpc(m),
            RpcError::Node { code, message } => {
                ReleaseError::Rpc(format!("node rpc {code}: {message}"))
            }
        }
    }
}

/// How many recent blocks to scan for reserve-owned outputs and decoys per
/// release. The reserve holds a bounded set of factor-1 outputs; scanning a
/// wide recent window keeps input/decoy selection cheap while still covering
/// realistic reserve depth. Operators funding a long-lived reserve seed the
/// window by keeping the reserve's outputs within it.
const RESERVE_SCAN_WINDOW: u64 = 10_000;

/// The BTH fee a release transaction pays. The minimum network fee suffices:
/// a factor-1 reserve spend pays zero demurrage (ADR 0003) and carries no
/// memos, so there is no progressive surcharge.
const RELEASE_FEE: u64 = MIN_TX_FEE;

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
/// confirmation are `TODO(#856)` stubs.
pub struct BthReleaser {
    config: BthConfig,
    /// Parsed federation verifying keys (from `bth.release_signers`).
    federation: Vec<VerifyingKey>,
    /// JSON-RPC client to the node (reserve UTXO scan, decoys, tx_submit,
    /// confirmation polling). `None` disables the live transport: the
    /// releaser still validates attestations but returns `NotImplemented`
    /// from the RPC-dependent stages, keeping burn orders `BurnConfirmed`.
    rpc: Option<BthNodeRpc>,
    /// Reserve wallet keys (view + spend). `None` (no key files) disables
    /// release construction — burn orders stay `BurnConfirmed`.
    reserve: Option<ReserveKeys>,
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

        // #842: a configured federation with threshold 0 would accept an
        // authorization carrying ZERO signatures (0 distinct signers >=
        // threshold 0) — i.e. an enabled releaser that spends the reserve
        // with no federation authorization at all. Refuse at construction;
        // burn orders then stay BurnConfirmed like any other misconfig.
        // (threshold 0 with NO signers remains the documented dev-only
        // no-pinning mode.)
        if !federation.is_empty() && config.release_threshold == 0 {
            return Err(ReleaseError::Config(
                "release_threshold must be >= 1 when release_signers are configured \
                 (threshold 0 would authorize reserve spends with no signatures)"
                    .to_string(),
            ));
        }

        // Live transport (#856): the JSON-RPC client and the reserve wallet
        // keys. A malformed rpc_url or key file disables the RPC-dependent
        // stages (they return NotImplemented) without failing construction,
        // so attestation verification and config validation still run and
        // burn orders stay BurnConfirmed rather than being dropped.
        let rpc = match BthNodeRpc::new(config.rpc_url.clone()) {
            Ok(rpc) => Some(rpc),
            Err(e) => {
                warn!("BTH release RPC disabled ({e}); releases will not submit");
                None
            }
        };
        let reserve = match ReserveKeys::load(
            config.view_key_file.as_deref(),
            config.spend_key_file.as_deref(),
        ) {
            Ok(keys) => keys,
            Err(e) => {
                warn!("BTH reserve keys unavailable ({e}); releases will not submit");
                None
            }
        };

        Ok(Self {
            config,
            federation,
            rpc,
            reserve,
        })
    }

    /// The confirmation depth a release must reach before `Released`
    /// (0 = SCP externalization finality).
    #[allow(dead_code)]
    pub fn required_confirmations(&self) -> u32 {
        self.config.release_confirmations_required
    }

    /// Scan the recent reserve window for spendable, factor-1, reserve-owned
    /// outputs (ADR 0003) and gather a pool of decoy outputs to build rings
    /// from. Returns `(owned, decoy_pool)`.
    ///
    /// "Spendable" excludes outputs whose key image is already spent or
    /// pending (`chain_areKeyImagesSpent`), so a resume never re-selects an
    /// input a prior release already consumed.
    async fn load_reserve_state(
        &self,
        rpc: &BthNodeRpc,
        reserve: &ReserveKeys,
    ) -> Result<(Vec<OwnedOutput>, Vec<crate::bth_rpc::RpcOutput>), ReleaseError> {
        let tip = rpc.chain_tip().await?;
        let start = tip.saturating_sub(RESERVE_SCAN_WINDOW);
        let blocks = rpc.get_outputs(start, tip).await?;

        let account = reserve.account();
        let mut owned: Vec<OwnedOutput> = Vec::new();
        let mut decoy_pool: Vec<crate::bth_rpc::RpcOutput> = Vec::new();

        for block in &blocks {
            for out in &block.outputs {
                match scan_deposit_output(out, account).map_err(ReleaseError::Config)? {
                    // A reserve-owned, factor-1 output is a candidate input.
                    Some(scanned) if scanned.owned.factor_one => owned.push(scanned.owned),
                    // A reserve-owned but NON-factor-1 output must never be
                    // spent (it would launder cluster provenance, ADR 0003);
                    // it is neither an input nor a decoy.
                    Some(_) => {}
                    // Everyone else's outputs are decoy candidates.
                    None => decoy_pool.push(out.clone()),
                }
            }
        }

        if owned.is_empty() {
            return Ok((owned, decoy_pool));
        }

        // Drop already-spent / pending inputs so we never double-spend the
        // reserve across releases.
        let key_images: Vec<String> = owned
            .iter()
            .map(|o| release_input_key_image(account, o))
            .collect::<Result<_, _>>()?;
        let statuses = rpc.are_key_images_spent(&key_images).await?;
        let spendable: Vec<OwnedOutput> = owned
            .into_iter()
            .zip(statuses)
            .filter(|(_, s)| !s.spent && !s.pending)
            .map(|(o, _)| o)
            .collect();

        Ok((spendable, decoy_pool))
    }
}

/// Derive the key image of a reserve-owned output (node-identical), for the
/// spent-status query. Reuses `recover_spend_key` + `KeyImage::from`, exactly
/// what the node records in its double-spend set.
fn release_input_key_image(
    account: &bth_account_keys::AccountKey,
    owned: &OwnedOutput,
) -> Result<String, ReleaseError> {
    let target_key: [u8; 32] = hex::decode(&owned.target_key)
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| ReleaseError::Config("owned output target_key not 32 bytes".into()))?;
    let public_key: [u8; 32] = hex::decode(&owned.public_key)
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| ReleaseError::Config("owned output public_key not 32 bytes".into()))?;
    let tx_out = TxOutput {
        amount: owned.amount,
        target_key,
        public_key,
        e_memo: None,
        cluster_tags: Default::default(),
        kem_ciphertext: None,
    };
    let onetime = tx_out
        .recover_spend_key(account, owned.subaddress_index)
        .ok_or_else(|| ReleaseError::Config("cannot recover reserve one-time key".into()))?;
    Ok(hex::encode(KeyImage::from(&onetime).as_bytes()))
}

/// Greedily select reserve outputs covering `amount + fee`, largest first (so
/// the fewest inputs — hence rings — are used).
fn select_inputs(mut owned: Vec<OwnedOutput>, required: u64) -> Option<Vec<OwnedOutput>> {
    owned.sort_by(|a, b| b.amount.cmp(&a.amount));
    let mut selected = Vec::new();
    let mut total = 0u64;
    for o in owned {
        if total >= required {
            break;
        }
        total = total.saturating_add(o.amount);
        selected.push(o);
    }
    if total >= required {
        Some(selected)
    } else {
        None
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

        // Live transport must be configured (RPC + reserve keys). Absent it,
        // stay fail-safe: the engine leaves the order BurnConfirmed for a
        // clean retry — no reserve funds move.
        let (Some(rpc), Some(reserve)) = (self.rpc.as_ref(), self.reserve.as_ref()) else {
            return Err(ReleaseError::NotImplemented(
                "BTH release not wired: configure bth.rpc_url + view_key_file + spend_key_file"
                    .to_string(),
            ));
        };

        // Recipient: a FRESH one-time stealth output is derived below from the
        // authorized recipient address (ADR 0004). We pay net_amount() — the
        // gross burn minus the bridge fee (the attestation binds this exact
        // value).
        let recipient = decode_recipient_address(&auth.recipient)
            .map_err(|e| ReleaseError::Config(format!("release recipient address: {e}")))?;
        let amount = auth.amount;
        let required = amount
            .checked_add(RELEASE_FEE)
            .ok_or_else(|| ReleaseError::Config("release amount + fee overflow".into()))?;

        // 1-3. Load spendable, factor-1, reserve-owned outputs (ADR 0003) +
        // a decoy pool; select inputs covering amount + fee.
        let (owned, decoy_pool) = self.load_reserve_state(rpc, reserve).await?;
        let selected = select_inputs(owned, required).ok_or_else(|| {
            ReleaseError::Rpc(format!(
                "reserve has insufficient spendable factor-1 outputs to release {amount} (+{RELEASE_FEE} fee)"
            ))
        })?;

        // 6. Ring decoys per input. Every input needs DEFAULT_RING_SIZE - 1
        // distinct decoys; a shared pool of everyone-else's outputs supplies
        // them. Reserve-owned outputs are never decoys (load_reserve_state
        // already excluded them from the pool).
        let needed_per_input = bth_transaction_clsag::DEFAULT_RING_SIZE - 1;
        let total_needed = needed_per_input.saturating_mul(selected.len());
        if decoy_pool.len() < total_needed {
            return Err(ReleaseError::Rpc(format!(
                "insufficient decoys for release: need {total_needed}, have {} in the recent window",
                decoy_pool.len()
            )));
        }
        let mut decoy_iter = decoy_pool.into_iter();
        let mut inputs = Vec::with_capacity(selected.len());
        for owned in selected {
            let decoys: Vec<_> = decoy_iter.by_ref().take(needed_per_input).collect();
            inputs.push(ReleaseInput { owned, decoys });
        }

        // 4-5, 7. Build + CLSAG-sign: fresh stealth recipient output, change
        // back to the reserve, self-verified against the node's verifier.
        let created_at_height = rpc.chain_tip().await?;
        let tx = build_release_tx(
            reserve.account(),
            &recipient,
            amount,
            RELEASE_FEE,
            &inputs,
            created_at_height,
            &mut rand_core::OsRng,
        )
        .map_err(|e| ReleaseError::Config(format!("release tx construction failed: {e}")))?;

        // 8. Serialize to the exact bincode wire format tx_submit accepts.
        let raw = bincode::serialize(&tx)
            .map_err(|e| ReleaseError::Config(format!("release tx serialize failed: {e}")))?;
        let tx_hash = hex::encode(tx.hash());
        info!(
            "Prepared BTH release for order {}: tx {} spending {} reserve input(s)",
            order.id,
            tx_hash,
            tx.inputs.len()
        );
        Ok(PreparedRelease { tx_hash, raw })
    }

    async fn broadcast(&self, prepared: &PreparedRelease) -> Result<(), ReleaseError> {
        let Some(rpc) = self.rpc.as_ref() else {
            return Err(ReleaseError::NotImplemented(
                "BTH release RPC not configured".to_string(),
            ));
        };
        let tx_hex = hex::encode(&prepared.raw);
        match rpc.submit_tx(&tx_hex).await {
            Ok(node_hash) => {
                if node_hash != prepared.tx_hash {
                    // The node hashes the tx it received; a mismatch means the
                    // wire bytes decode to a different tx than we recorded.
                    // Treat as a transport error so the engine retries the
                    // recorded bytes rather than trusting a divergent hash.
                    return Err(ReleaseError::Rpc(format!(
                        "node returned tx hash {node_hash}, expected {}",
                        prepared.tx_hash
                    )));
                }
                debug!("Broadcast BTH release tx {}", prepared.tx_hash);
                Ok(())
            }
            Err(RpcError::Node { code, message }) => {
                let lower = message.to_lowercase();
                // Idempotent re-broadcast: the node already has (or already
                // mined) this exact tx. On BTH the tell is a duplicate key
                // image or an "already"/"exists" rejection of OUR recorded tx
                // — success, the tx was submitted before a restart.
                if lower.contains("already")
                    || lower.contains("exists")
                    || lower.contains("duplicate")
                    || lower.contains("key image")
                {
                    warn!(
                        "BTH release tx {} already known to the node ({message}); treating as broadcast",
                        prepared.tx_hash
                    );
                    Ok(())
                } else {
                    Err(ReleaseError::Rpc(format!(
                        "tx_submit rejected ({code}): {message}"
                    )))
                }
            }
            Err(e) => Err(e.into()),
        }
    }

    async fn check_confirmation(
        &self,
        _order: &BridgeOrder,
        dest_tx: &str,
    ) -> Result<ReleaseConfirmation, ReleaseError> {
        let Some(rpc) = self.rpc.as_ref() else {
            return Err(ReleaseError::NotImplemented(
                "BTH release RPC not configured".to_string(),
            ));
        };

        match rpc.get_transaction(dest_tx).await? {
            TxInclusion::Confirmed { confirmations, .. } => {
                let required = self.config.release_confirmations_required as u64;
                // required == 0 (SCP externalization finality): in-a-block ==
                // final. Otherwise wait for the configured depth.
                if confirmations >= required.max(1) {
                    Ok(ReleaseConfirmation::Confirmed)
                } else {
                    Ok(ReleaseConfirmation::Pending { confirmations })
                }
            }
            TxInclusion::Pending => Ok(ReleaseConfirmation::Pending { confirmations: 0 }),
            TxInclusion::Unknown => {
                // The node has never seen this tx. It is NOT safe to unwind on
                // "unseen" alone — a re-broadcast may still land it, and BTH
                // has no on-chain order-id guard, so re-signing while the old
                // tx could land risks a double release. Only report Dropped
                // when the tx PROVABLY cannot land: its inputs' key images are
                // spent by a DIFFERENT transaction. We cannot recover the
                // input key images from `dest_tx` alone here, so we keep the
                // order Pending (the engine re-broadcasts the recorded bytes)
                // rather than fabricate a Dropped. A stuck release surfaces via
                // the reserve reconciliation / operator alerting (#825).
                debug!(
                    "BTH release tx {dest_tx} not yet seen by the node; holding Pending for re-broadcast"
                );
                Ok(ReleaseConfirmation::Pending { confirmations: 0 })
            }
        }
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

        // #842: a configured federation with threshold 0 must be refused —
        // it would authorize reserve spends with zero signatures.
        let cfg = test_config(&[&k1, &k2], 0);
        assert!(matches!(
            BthReleaser::new(cfg),
            Err(ReleaseError::Config(_))
        ));

        // The documented dev-only escape hatch (no signers, threshold 0)
        // still constructs.
        let cfg = test_config(&[], 0);
        assert!(BthReleaser::new(cfg).is_ok());
    }

    #[tokio::test]
    async fn test_prepare_release_gates_on_attestation_before_construction() {
        let (k1, k2) = (signing_key(1), signing_key(2));
        // test_config configures NO reserve key files, so the live transport
        // is unwired: the construction path fails safe with NotImplemented
        // (leaving the order BurnConfirmed), and the attestation gate must
        // still fire first.
        let releaser = BthReleaser::new(test_config(&[&k1, &k2], 2)).unwrap();
        let order = burn_order();

        // A bad attestation is rejected as Attestation (the gate fires BEFORE
        // any reserve key material is touched or any tx is constructed).
        let auth = signed_auth(&order, &[&k1], 2);
        assert!(matches!(
            releaser.prepare_release(&order, &auth).await,
            Err(ReleaseError::Attestation(_))
        ));

        // A valid attestation passes the gate and reaches the transport,
        // which — with no reserve keys configured — fails safe rather than
        // moving reserve funds.
        let auth = signed_auth(&order, &[&k1, &k2], 2);
        assert!(matches!(
            releaser.prepare_release(&order, &auth).await,
            Err(ReleaseError::NotImplemented(_))
        ));
    }

    #[tokio::test]
    async fn test_prepare_release_unreachable_rpc_is_retryable_not_false_success() {
        // A releaser WITH reserve keys but an unreachable node must surface a
        // retryable Rpc error — never a false success (which would let the
        // engine record a nonexistent release tx). This is the fail-safe
        // posture the exactly-once guard depends on.
        let dir = tempfile::tempdir().unwrap();
        let account = bth_account_keys::AccountKey::random(&mut rand::rngs::OsRng);
        let view_path = dir.path().join("view.hex");
        let spend_path = dir.path().join("spend.hex");
        std::fs::write(
            &view_path,
            hex::encode(account.view_private_key().to_bytes()),
        )
        .unwrap();
        std::fs::write(
            &spend_path,
            hex::encode(account.spend_private_key().to_bytes()),
        )
        .unwrap();

        let (k1, k2) = (signing_key(1), signing_key(2));
        let mut config = test_config(&[&k1, &k2], 2);
        // A port nothing listens on: the RPC call must fail (not hang forever
        // — reqwest's connect fails fast on a closed port).
        config.rpc_url = "http://127.0.0.1:1/".to_string();
        config.view_key_file = Some(view_path.to_string_lossy().into_owned());
        config.spend_key_file = Some(spend_path.to_string_lossy().into_owned());

        let releaser = BthReleaser::new(config).unwrap();

        // A decodable recipient address (view||spend base58) so the failure is
        // the RPC reach, not the recipient decode. Re-sign the auth to bind
        // this recipient.
        let recipient =
            bth_account_keys::AccountKey::random(&mut rand::rngs::OsRng).default_subaddress();
        let mut recipient_bytes = Vec::with_capacity(64);
        recipient_bytes.extend_from_slice(&recipient.view_public_key().to_bytes());
        recipient_bytes.extend_from_slice(&recipient.spend_public_key().to_bytes());
        let recipient_addr = bs58::encode(recipient_bytes).into_string();

        let mut order = burn_order();
        order.dest_address = recipient_addr;
        let auth = signed_auth(&order, &[&k1, &k2], 2);

        match releaser.prepare_release(&order, &auth).await {
            Err(ReleaseError::Rpc(_)) => {} // retryable, order stays BurnConfirmed
            other => panic!("expected a retryable Rpc error, got {other:?}"),
        }
    }
}
