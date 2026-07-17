// Copyright (c) 2024 The Botho Foundation

//! Node-identical output scanning + release-tx construction helpers (#856).
//!
//! Pure, native-testable glue between the node's `chain_getOutputs` wire shape
//! ([`crate::bth_rpc::RpcOutput`]) and the node-identical CLSAG crypto in
//! [`bth_transaction_clsag`]. Nothing here re-implements stealth derivation,
//! commitment opening, memo decryption, key-image derivation, or CLSAG
//! signing — every one of those calls straight into the shared crate the node
//! and web wallet use, so the bridge can never drift from consensus.
//!
//! Two responsibilities:
//!
//! - **Deposit scanning** ([`scan_deposit_output`]): view-key-test an output
//!   for ownership by the bridge deposit account, and — for an owned output —
//!   decrypt its destination memo and read its factor-1 eligibility.
//! - **Release construction** ([`build_release_tx`]): assemble + CLSAG-sign a
//!   transaction spending reserve-owned factor-1 outputs to a FRESH one-time
//!   stealth output for the recipient (ADR 0004), change back to the reserve
//!   (ADR 0003 provenance).

use bth_account_keys::{AccountKey, PublicAddress};
use bth_crypto_keys::RistrettoPublic;
use bth_crypto_pq::MlKem768KeyPair;
use bth_transaction_clsag::{
    ClsagRingInput, EncryptedMemo, MemoPayload, RingMember, Transaction, TxOutput,
    DEFAULT_RING_SIZE, DUST_THRESHOLD, MIN_TX_FEE,
};
use bth_transaction_types::{ClusterId, ClusterTagVector};
use rand_core::{CryptoRng, RngCore};

use crate::bth_rpc::RpcOutput;

/// An owned reserve/deposit output the scan identified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedOutput {
    /// Transaction hash the output lives in (hex).
    pub tx_hash: String,
    /// Output index within its transaction.
    pub output_index: u32,
    /// Hex-encoded 32-byte one-time target key.
    pub target_key: String,
    /// Hex-encoded 32-byte ephemeral public key.
    pub public_key: String,
    /// Transparent amount in picocredits.
    pub amount: u64,
    /// Subaddress index that received the output (0 = default, 1 = change).
    pub subaddress_index: u64,
    /// Whether the output is factor-1 (background/commerce, wrap-eligible).
    pub factor_one: bool,
    /// Unified ML-KEM-768 ciphertext (1088 bytes) for a hybrid deposit, else
    /// `None` (classical). Retained so the release path can recover the
    /// one-time key via hybrid decapsulation (issue #970).
    pub kem_ciphertext: Option<Vec<u8>>,
}

/// The result of view-key testing a deposit-address output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedDeposit {
    /// The owned output.
    pub owned: OwnedOutput,
    /// Decrypted destination-memo bytes (64), if the output carried a
    /// decodable memo. The watcher reads the order UUID from the first 16
    /// bytes (see `BridgeOrder::order_id_from_memo`).
    pub memo: Option<[u8; 64]>,
}

fn parse_hex_32(field: &str, s: &str) -> Result<[u8; 32], String> {
    let bytes = hex::decode(s).map_err(|e| format!("{field}: invalid hex: {e}"))?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| format!("{field}: expected 32 bytes"))?;
    Ok(arr)
}

/// Reconstruct a node-identical [`TxOutput`] shell from an RPC output. The
/// commitment is the transparent amount (the node's transparent-amount model),
/// so `belongs_to` / `recover_spend_key` / memo decryption behave exactly as
/// on the node.
fn tx_output_from_rpc(out: &RpcOutput) -> Result<TxOutput, String> {
    let e_memo = match &out.e_memo {
        Some(hex_memo) => {
            let bytes =
                hex::decode(hex_memo).map_err(|e| format!("output e_memo: invalid hex: {e}"))?;
            Some(
                EncryptedMemo::from_bytes(&bytes)
                    .ok_or_else(|| "output e_memo: wrong length".to_string())?,
            )
        }
        None => None,
    };
    let kem_ciphertext = match &out.kem_ciphertext {
        Some(hex_ct) => Some(
            hex::decode(hex_ct).map_err(|e| format!("output kem_ciphertext: invalid hex: {e}"))?,
        ),
        None => None,
    };
    Ok(TxOutput {
        amount: out.amount,
        target_key: parse_hex_32("output.target_key", &out.target_key)?,
        public_key: parse_hex_32("output.public_key", &out.public_key)?,
        e_memo,
        cluster_tags: Default::default(),
        kem_ciphertext,
    })
}

/// View-key test one output for ownership by `account`; on a match, decrypt
/// its destination memo and record factor-1 eligibility.
///
/// `explicit_cluster_weight` is the output's non-background cluster weight in
/// ppm as reported by the node (0 == factor-1 / background, ADR 0003). It is
/// passed alongside because the reconstructed [`TxOutput`] shell does not carry
/// cluster tags (the RPC returns them separately).
pub fn scan_deposit_output(
    out: &RpcOutput,
    account: &AccountKey,
    kem_keypair: Option<&MlKem768KeyPair>,
) -> Result<Option<ScannedDeposit>, String> {
    let tx_out = tx_output_from_rpc(out)?;

    // Unified hybrid scan path (issue #970). A hybrid deposit (KEM ciphertext
    // present) can only be detected by ML-KEM decapsulation with the reserve's
    // secret; without it the deposit is NOT silently dropped — we warn loudly so
    // the operator wires the reserve KEM key rather than losing funds.
    let subaddress_index = if tx_out.kem_ciphertext.is_some() {
        match kem_keypair {
            Some(kp) => match tx_out.belongs_to_account(account, kp, out.output_index) {
                Some(idx) => idx,
                None => return Ok(None),
            },
            None => {
                tracing::warn!(
                    tx_hash = %out.tx_hash,
                    output_index = out.output_index,
                    "hybrid (ML-KEM) output seen but reserve has no ML-KEM secret \
                     configured; cannot scan it — configure the reserve KEM key to \
                     detect hybrid deposits"
                );
                return Ok(None);
            }
        }
    } else {
        match tx_out.belongs_to(account) {
            Some(idx) => idx,
            None => return Ok(None),
        }
    };

    // Destination memo (order UUID). `decrypt_memo` returns None for an
    // output with no memo or one that does not decrypt for us.
    let memo: Option<[u8; 64]> = tx_out
        .decrypt_memo(account)
        .filter(|m| !m.is_unused())
        .map(|m| *m.memo_data());

    Ok(Some(ScannedDeposit {
        owned: OwnedOutput {
            tx_hash: out.tx_hash.clone(),
            output_index: out.output_index,
            target_key: out.target_key.clone(),
            public_key: out.public_key.clone(),
            amount: out.amount,
            subaddress_index,
            factor_one: out.explicit_cluster_weight() == 0,
            kem_ciphertext: tx_out.kem_ciphertext.clone(),
        },
        memo,
    }))
}

/// Decode a base58 classical BTH address (`<view32><spend32>`) into a
/// [`PublicAddress`]. Accepts an optional URI prefix (`botho://` /
/// `tbotho://`) or a bare base58 body, matching the node's
/// `parse_classical_address` layout (64 bytes: view || spend).
pub fn decode_recipient_address(address: &str) -> Result<PublicAddress, String> {
    // Prefer the canonical address codec: it parses both v1 (classical) and v2
    // (hybrid ML-KEM/ML-DSA) addresses and returns a `PublicAddress` carrying
    // the recipient's post-quantum public keys. On the protocol-6.0.0 chain the
    // release recipient is the user's published v2 address (`tbotho://2/…`,
    // ~3.2 KB), which the legacy 64-byte path below cannot represent (#972/#1025).
    if let Ok((addr, _network)) = bth_address_codec::decode_address(address) {
        return Ok(addr);
    }

    // Legacy fallback: a bare base58 `view || spend` (64 bytes), no PQ keys.
    // Kept so classical callers/tests that pass a raw address still work.
    let body = address.rsplit('/').next().unwrap_or(address);
    let bytes = bs58::decode(body)
        .into_vec()
        .map_err(|e| format!("recipient address: invalid base58: {e}"))?;
    if bytes.len() != 64 {
        return Err(format!(
            "recipient address: not a v1/v2 address and not a 64-byte view||spend (got {} bytes)",
            bytes.len()
        ));
    }
    let view = RistrettoPublic::try_from(&bytes[0..32])
        .map_err(|e| format!("recipient view key: {e:?}"))?;
    let spend = RistrettoPublic::try_from(&bytes[32..64])
        .map_err(|e| format!("recipient spend key: {e:?}"))?;
    Ok(PublicAddress::new(&spend, &view))
}

/// A reserve output selected to fund a release, with its ring decoys.
#[derive(Debug, Clone)]
pub struct ReleaseInput {
    /// The owned reserve output being spent.
    pub owned: OwnedOutput,
    /// Decoys for this input's ring (need at least `DEFAULT_RING_SIZE - 1`).
    pub decoys: Vec<RpcOutput>,
}

/// Build and CLSAG-sign a reserve-release transaction.
///
/// Pays `amount` picocredits to a FRESH one-time stealth output for
/// `recipient` (ADR 0004) with change back to the reserve's default
/// subaddress (ADR 0003 provenance — the change keeps the reserve
/// zero-demurrage). Reuses the node-identical builder / signer exactly like
/// the web wallet's `build_and_sign`, so the produced tx bincode-round-trips
/// through the node's `tx_submit`.
///
/// `account` is the RESERVE account (its view/spend private keys). Every input
/// in `inputs` must be a reserve-owned output; `recover_spend_key` recovers the
/// one-time key for each. The function self-verifies the signed tx before
/// returning, so it never hands back a tx the node would reject.
pub fn build_release_tx<R: RngCore + CryptoRng>(
    account: &AccountKey,
    recipient: &PublicAddress,
    amount: u64,
    fee: u64,
    inputs: &[ReleaseInput],
    created_at_height: u64,
    kem_keypair: Option<&MlKem768KeyPair>,
    rng: &mut R,
) -> Result<Transaction, String> {
    if inputs.is_empty() {
        return Err("at least one reserve input is required".to_string());
    }
    if amount == 0 {
        return Err("release amount must be greater than 0".to_string());
    }
    if amount < DUST_THRESHOLD {
        return Err(format!(
            "release amount {amount} is below dust threshold {DUST_THRESHOLD}"
        ));
    }
    if fee < MIN_TX_FEE {
        return Err(format!("release fee {fee} is below minimum {MIN_TX_FEE}"));
    }

    // Balance equation.
    let mut input_total: u64 = 0;
    for input in inputs {
        input_total = input_total
            .checked_add(input.owned.amount)
            .ok_or("input amount sum overflow")?;
    }
    let spent = amount.checked_add(fee).ok_or("amount + fee overflow")?;
    let change = input_total
        .checked_sub(spent)
        .ok_or("insufficient reserve inputs: do not cover amount + fee")?;

    // Recipient output: a FRESH one-time stealth output (ADR 0004), tagged
    // 100% to the block-epoch bridge-import cluster (ADR 0007). Before ADR 0007
    // this output carried NO cluster tag, so unwrapped BTH returned at factor-1
    // (background) — the entry leak / round-trip-laundromat the ADR closes. Now
    // it joins `c_import(⌊height/K⌋)`, an accumulating shared origin whose
    // factor is `max(F=1.5x, curve(Σ epoch unwrap volume))`; it normalizes to
    // background only by circulating (the existing value-weighted tag blend on
    // spends), never by sitting idle. The derivation is the single
    // consensus-canonical helper the ledger also uses, so the tag can never
    // drift from the node's enforcement.
    let import_cluster_id = bth_cluster_tax::import_cluster_id_for_height(created_at_height);
    let import_tags = ClusterTagVector::single(ClusterId(import_cluster_id.0));
    // On the protocol-6.0.0 chain the recipient publishes an ML-KEM key, so the
    // release pays a HYBRID stealth output (ADR 0004): the ciphertext is
    // encapsulated to the recipient's KEM key and folded into the one-time key,
    // and the recipient scans it back with its ML-KEM secret (the test's
    // assertion 2). A classical (v1) recipient falls back to a pure-DH stealth
    // output. The recipient is output index 0, bound into the hybrid derivation.
    let recipient_output = if recipient.has_pq_keys() {
        TxOutput::new_hybrid_to_address(amount, recipient, 0, None, import_tags)
            .map_err(|e| format!("recipient hybrid output: {e:?}"))?
    } else {
        TxOutput::new_with_cluster_tags(amount, recipient, None, import_tags)
    };
    let mut outputs = vec![recipient_output];

    // Change back to the reserve's default subaddress (ADR 0003). Sub-dust
    // change is folded into the fee (never an unspendable output).
    //
    // On the protocol-6.0.0 chain EVERY value output must carry an ML-KEM
    // ciphertext (`KEM_CIPHERTEXT_ENFORCED`, #973) or the node rejects the tx
    // at intrinsic validation — so the change is a HYBRID self-send encapsulated
    // to the reserve's OWN ML-KEM key (the reserve's `AccountKey` does not carry
    // its PQ public key inline, so we pass the keypair's public key directly to
    // `new_hybrid`). The reserve scans this change back with its ML-KEM secret
    // for a later release. Change is output index 1, bound into the derivation.
    // A classical-only reserve (pre-6.0.0 / no PQ seed) falls back to a pure-DH
    // output (#972/#1025).
    let change_output = |amount: u64| -> TxOutput {
        match kem_keypair {
            Some(kp) => TxOutput::new_hybrid(
                amount,
                &account.default_subaddress(),
                kp.public_key(),
                1,
                None,
                ClusterTagVector::default(),
            ),
            None => TxOutput::new(amount, &account.default_subaddress()),
        }
    };
    let actual_fee = if change >= DUST_THRESHOLD {
        outputs.push(change_output(change));
        fee
    } else {
        fee + change
    };

    // Preliminary (input-less) tx to compute the signing hash the ring
    // signatures commit to (depends only on outputs, fee, height).
    let preliminary =
        Transaction::new_clsag(Vec::new(), outputs.clone(), actual_fee, created_at_height);
    let signing_hash = preliminary.signing_hash();

    let mut ring_inputs = Vec::with_capacity(inputs.len());
    for input in inputs {
        let needed = DEFAULT_RING_SIZE - 1;
        if input.decoys.len() < needed {
            return Err(format!(
                "not enough decoys for reserve input: need {needed}, got {}",
                input.decoys.len()
            ));
        }

        let real_output = TxOutput {
            amount: input.owned.amount,
            target_key: parse_hex_32("input.target_key", &input.owned.target_key)?,
            public_key: parse_hex_32("input.public_key", &input.owned.public_key)?,
            e_memo: None,
            cluster_tags: Default::default(),
            kem_ciphertext: input.owned.kem_ciphertext.clone(),
        };
        // Unified recovery (issue #970): a hybrid reserve input needs the ML-KEM
        // secret to reconstruct its one-time key; a classical input uses the
        // pure view-key path. Both dispatch through `recover_spend_key_for`.
        let onetime_private = if real_output.kem_ciphertext.is_some() {
            let kp = kem_keypair.ok_or(
                "hybrid reserve input requires an ML-KEM secret to recover its one-time key",
            )?;
            real_output.recover_spend_key_for(
                account,
                kp,
                input.owned.subaddress_index,
                input.owned.output_index,
            )
        } else {
            real_output.recover_spend_key(account, input.owned.subaddress_index)
        }
        .ok_or("failed to recover one-time private key for reserve input")?;

        let real_member = RingMember::from_output(&real_output);
        let real_target = real_member.target_key;
        let mut ring: Vec<RingMember> = Vec::with_capacity(DEFAULT_RING_SIZE);
        ring.push(real_member);
        for decoy in input.decoys.iter().take(needed) {
            ring.push(RingMember::from_output(&TxOutput {
                amount: decoy.amount,
                target_key: parse_hex_32("decoy.target_key", &decoy.target_key)?,
                public_key: parse_hex_32("decoy.public_key", &decoy.public_key)?,
                e_memo: None,
                cluster_tags: Default::default(),
                kem_ciphertext: None,
            }));
        }
        shuffle(&mut ring, rng);
        let real_index = ring
            .iter()
            .position(|m| m.target_key == real_target)
            .ok_or("internal error: real reserve input lost during shuffle")?;

        let ring_input = ClsagRingInput::new(
            ring,
            real_index,
            &onetime_private,
            input.owned.amount,
            &signing_hash,
            rng,
        )
        .map_err(|e| format!("CLSAG signing failed: {e}"))?;
        ring_inputs.push(ring_input);
    }

    let tx = Transaction::new_clsag(ring_inputs, outputs, actual_fee, created_at_height);

    // Self-verify under the same code path the node runs before returning.
    tx.is_valid_structure()
        .map_err(|e| format!("produced an invalid release tx structure: {e}"))?;
    tx.verify_ring_signatures()
        .map_err(|e| format!("produced a release tx that fails verification: {e}"))?;

    Ok(tx)
}

/// Encode a [`MemoPayload::destination`] onto a fresh output. Currently unused
/// by the release path (releases carry no memo) but kept for symmetry with the
/// deposit side and to document that the primitive is available.
#[allow(dead_code)]
pub fn destination_memo(message: &str) -> MemoPayload {
    MemoPayload::destination(message)
}

/// Fisher-Yates shuffle (mirrors the wasm-signer core so the real-input
/// position is hidden without pulling in `rand`'s `SliceRandom`).
fn shuffle<T, R: RngCore>(items: &mut [T], rng: &mut R) {
    let len = items.len();
    if len <= 1 {
        return;
    }
    for i in (1..len).rev() {
        let bound = (i + 1) as u64;
        let zone = u64::MAX - (u64::MAX % bound);
        let mut r = rng.next_u64();
        while r >= zone {
            r = rng.next_u64();
        }
        let j = (r % bound) as usize;
        items.swap(i, j);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bth_transaction_types::{ClusterId, ClusterTagEntry, ClusterTagVector};
    use rand::{rngs::StdRng, SeedableRng};

    fn rpc_output_from_txoutput(out: &TxOutput, tx_hash: &str, index: u32) -> RpcOutput {
        RpcOutput {
            tx_hash: tx_hash.to_string(),
            output_index: index,
            target_key: hex::encode(out.target_key),
            public_key: hex::encode(out.public_key),
            amount: out.amount,
            cluster_tags: out
                .cluster_tags
                .entries
                .iter()
                .map(|e| (e.cluster_id.0, e.weight as u64))
                .collect(),
            e_memo: out.e_memo.as_ref().map(|m| hex::encode(m.as_bytes())),
            kem_ciphertext: out.kem_ciphertext.as_ref().map(hex::encode),
        }
    }

    fn recipient_string(addr: &PublicAddress) -> String {
        // view || spend, base58, matching parse_classical_address.
        let mut bytes = Vec::with_capacity(64);
        bytes.extend_from_slice(&addr.view_public_key().to_bytes());
        bytes.extend_from_slice(&addr.spend_public_key().to_bytes());
        bs58::encode(bytes).into_string()
    }

    /// A hybrid (ML-KEM) deposit to the reserve is detected only with the
    /// reserve's ML-KEM secret, and the detected output is spendable via hybrid
    /// one-time-key recovery in a release tx (issue #970). Without the secret
    /// the deposit is skipped (with a loud warning) rather than silently
    /// treated as classical.
    #[test]
    fn hybrid_deposit_is_detected_and_released_with_kem_secret() {
        let mut rng = StdRng::from_seed([73u8; 32]);
        let reserve = AccountKey::random(&mut rng);
        let stranger = AccountKey::random(&mut rng);
        let recipient = AccountKey::random(&mut rng).default_subaddress();
        let kem = MlKem768KeyPair::from_seed(&[0x33u8; 32]);

        // Send path (sub-issue 4): a hybrid output paying the reserve, KEM ct
        // encapsulated to the reserve's ML-KEM key.
        let owned_amount = 10_000_000_000_000u64;
        let out = TxOutput::new_hybrid(
            owned_amount,
            &reserve.default_subaddress(),
            kem.public_key(),
            0,
            None,
            ClusterTagVector::empty(),
        );
        assert!(
            out.kem_ciphertext.is_some(),
            "hybrid output carries a KEM ct"
        );
        let rpc = rpc_output_from_txoutput(&out, "0xhybrid", 0);

        // Without the KEM secret the reserve cannot detect it (warns, not silent).
        assert!(
            scan_deposit_output(&rpc, &reserve, None).unwrap().is_none(),
            "hybrid deposit must not be seen without the ML-KEM secret",
        );
        // A stranger with the wrong classical keys never detects it either.
        assert!(
            scan_deposit_output(&rpc, &stranger, Some(&kem))
                .unwrap()
                .is_none(),
            "a stranger must not detect the reserve's hybrid deposit",
        );

        // With the reserve's KEM secret the deposit is detected + factor-1.
        let scanned = scan_deposit_output(&rpc, &reserve, Some(&kem))
            .unwrap()
            .expect("reserve must detect its hybrid deposit with the KEM secret");
        assert_eq!(scanned.owned.amount, owned_amount);
        assert!(scanned.owned.factor_one, "no cluster tags => factor-1");
        assert!(
            scanned.owned.kem_ciphertext.is_some(),
            "owned output must retain the KEM ciphertext for release",
        );

        // Release path: the hybrid reserve input is spendable via hybrid
        // one-time-key recovery, and the signed tx self-verifies.
        let inputs = vec![ReleaseInput {
            owned: scanned.owned,
            decoys: make_decoys(DEFAULT_RING_SIZE - 1, owned_amount, &mut rng),
        }];
        let tx = build_release_tx(
            &reserve,
            &recipient,
            4_000_000_000_000,
            MIN_TX_FEE,
            &inputs,
            5_000,
            Some(&kem),
            &mut rng,
        )
        .expect("hybrid reserve release builds and self-verifies");
        assert_eq!(tx.inputs.len(), 1);
    }

    /// #1037 round-trip proof: a deposit built the way the WASM signer builds it
    /// — a hybrid output to the reserve carrying the order memo via
    /// `MemoPayload::destination_bytes` (the exact call
    /// `bth_wasm_signer::core` makes) — is view-key-matched by the watcher back
    /// to its order UUID.
    ///
    /// The memo is the *real* wire memo `BridgeOrder::generate_memo` produces
    /// (UUID in the first 16 bytes), so this asserts the wallet embed format
    /// and the watcher read format agree end to end: `scan_deposit_output`
    /// decrypts the memo and `BridgeOrder::order_id_from_memo` recovers the
    /// order's id.
    #[test]
    fn scan_matches_wallet_built_deposit_memo_to_order() {
        use bth_bridge_core::{BridgeOrder, Chain};

        let mut rng = StdRng::from_seed([37u8; 32]);
        let reserve = AccountKey::random(&mut rng);
        let kem = MlKem768KeyPair::from_seed(&[0x51u8; 32]);

        // A real mint order; its generated memo carries the order UUID.
        let mut order = BridgeOrder::new_mint(
            Chain::Ethereum,
            1_000_000_000_000,
            1_000_000_000,
            "bth_deposit_addr".to_string(),
            "0x1234567890abcdef1234567890abcdef12345678".to_string(),
        );
        let order_memo: [u8; 64] = order.generate_memo();

        // The wallet embeds the 64-byte memo via `destination_bytes` (the exact
        // WASM construction path) on a hybrid deposit output to the reserve.
        let deposit = TxOutput::new_hybrid(
            1_000_000_000_000,
            &reserve.default_subaddress(),
            kem.public_key(),
            0,
            Some(MemoPayload::destination_bytes(&order_memo)),
            ClusterTagVector::empty(),
        );
        let rpc = rpc_output_from_txoutput(&deposit, "0xdeposit", 0);

        // The watcher scans it with the reserve account + KEM secret.
        let scanned = scan_deposit_output(&rpc, &reserve, Some(&kem))
            .unwrap()
            .expect("reserve detects its hybrid deposit");
        let memo = scanned.memo.expect("the deposit carries a decodable memo");
        assert_eq!(memo, order_memo, "decrypted memo must equal the order memo");

        // The watcher recovers the order UUID and it matches the order.
        let recovered =
            BridgeOrder::order_id_from_memo(&memo).expect("watcher recovers a UUID from the memo");
        assert_eq!(
            recovered, order.id,
            "the recovered order id must match the order the wallet was told to reference"
        );
    }

    #[test]
    fn scan_detects_owned_output_and_decrypts_memo() {
        let mut rng = StdRng::from_seed([3u8; 32]);
        let reserve = AccountKey::random(&mut rng);
        let stranger = AccountKey::random(&mut rng);

        // A deposit to the reserve's default subaddress carrying the order memo.
        let memo = MemoPayload::destination("order-1234");
        let deposit = TxOutput::new_with_memo(
            1_000_000_000_000,
            &reserve.default_subaddress(),
            Some(memo.clone()),
        );
        let rpc = rpc_output_from_txoutput(&deposit, "0xdep", 0);

        let scanned = scan_deposit_output(&rpc, &reserve, None)
            .unwrap()
            .expect("reserve owns the deposit");
        assert_eq!(scanned.owned.amount, 1_000_000_000_000);
        assert_eq!(scanned.owned.subaddress_index, 0);
        assert!(scanned.owned.factor_one, "no cluster tags => factor-1");
        assert_eq!(scanned.memo.unwrap(), *memo.memo_data());

        // A stranger's identical output is invisible to the reserve scan.
        let theirs = TxOutput::new(1_000_000_000_000, &stranger.default_subaddress());
        assert!(
            scan_deposit_output(&rpc_output_from_txoutput(&theirs, "0xx", 0), &reserve, None)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn scan_flags_non_factor1_output() {
        let mut rng = StdRng::from_seed([5u8; 32]);
        let reserve = AccountKey::random(&mut rng);

        // An output carrying an explicit cluster tag is NOT factor-1.
        let tagged = TxOutput::new_with_cluster_tags(
            1_000,
            &reserve.default_subaddress(),
            None,
            ClusterTagVector {
                entries: vec![ClusterTagEntry {
                    cluster_id: ClusterId(7),
                    weight: 250_000,
                }],
                decay_state: None,
            },
        );
        let rpc = rpc_output_from_txoutput(&tagged, "0xtag", 0);
        let scanned = scan_deposit_output(&rpc, &reserve, None).unwrap().unwrap();
        assert!(!scanned.owned.factor_one);
    }

    #[test]
    fn scan_owned_output_without_memo_returns_none_memo() {
        let mut rng = StdRng::from_seed([7u8; 32]);
        let reserve = AccountKey::random(&mut rng);
        let no_memo = TxOutput::new(500, &reserve.default_subaddress());
        let scanned = scan_deposit_output(
            &rpc_output_from_txoutput(&no_memo, "0xn", 0),
            &reserve,
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(scanned.memo, None);
    }

    #[test]
    fn recipient_address_roundtrips() {
        let mut rng = StdRng::from_seed([9u8; 32]);
        let account = AccountKey::random(&mut rng);
        let addr = account.default_subaddress();
        let decoded = decode_recipient_address(&recipient_string(&addr)).unwrap();
        assert_eq!(
            decoded.spend_public_key().to_bytes(),
            addr.spend_public_key().to_bytes()
        );
        assert_eq!(
            decoded.view_public_key().to_bytes(),
            addr.view_public_key().to_bytes()
        );
        // A scheme prefix is tolerated.
        let with_scheme = format!("tbotho://{}", recipient_string(&addr));
        assert!(decode_recipient_address(&with_scheme).is_ok());
    }

    fn make_decoys(count: usize, amount: u64, rng: &mut StdRng) -> Vec<RpcOutput> {
        (0..count)
            .map(|i| {
                let decoy_account = AccountKey::random(rng);
                let out = TxOutput::new(amount, &decoy_account.default_subaddress());
                rpc_output_from_txoutput(&out, "0xdecoy", i as u32)
            })
            .collect()
    }

    #[test]
    fn build_release_tx_pays_recipient_and_change_to_reserve() {
        let mut rng = StdRng::from_seed([11u8; 32]);
        let reserve = AccountKey::random(&mut rng);
        let recipient_account = AccountKey::random(&mut rng);
        let recipient = recipient_account.default_subaddress();

        // Reserve owns one factor-1 output.
        let owned_amount = 10_000_000_000_000u64;
        let reserve_out = TxOutput::new(owned_amount, &reserve.default_subaddress());
        let owned = scan_deposit_output(
            &rpc_output_from_txoutput(&reserve_out, "0xreserve", 0),
            &reserve,
            None,
        )
        .unwrap()
        .unwrap()
        .owned;

        let decoys = make_decoys(DEFAULT_RING_SIZE - 1, owned_amount, &mut rng);
        let inputs = vec![ReleaseInput { owned, decoys }];

        let amount = 4_000_000_000_000u64;
        let fee = MIN_TX_FEE;
        let tx = build_release_tx(
            &reserve, &recipient, amount, fee, &inputs, 5_000, None, &mut rng,
        )
        .expect("release tx builds and self-verifies");

        // Node verifier accepts it.
        tx.verify_ring_signatures().unwrap();
        tx.is_valid_structure().unwrap();

        // Recipient + change, balance exact.
        assert_eq!(tx.inputs.len(), 1);
        assert_eq!(tx.inputs.clsag()[0].ring.len(), DEFAULT_RING_SIZE);
        let out_total: u64 = tx.outputs.iter().map(|o| o.amount).sum();
        assert_eq!(out_total + tx.fee, owned_amount);
        assert_eq!(tx.outputs.len(), 2, "recipient + change");

        // The recipient output is a FRESH stealth output the recipient owns...
        let recipient_out = &tx.outputs[0];
        assert_eq!(recipient_out.amount, amount);
        assert!(recipient_out.belongs_to(&recipient_account).is_some());
        // ...and it is tagged 100% to the block-epoch bridge-import cluster
        // (ADR 0007, #938): height 5,000 is in epoch 0 (< K = 17,280).
        let expected_import = bth_cluster_tax::import_cluster_id_for_height(5_000).0;
        assert_eq!(
            recipient_out.cluster_tags.entries.len(),
            1,
            "recipient output must carry exactly the import cluster tag"
        );
        assert_eq!(
            recipient_out.cluster_tags.entries[0].cluster_id.0, expected_import,
            "recipient output must be tagged to c_import(epoch 0)"
        );
        assert_eq!(
            recipient_out.cluster_tags.entries[0].weight,
            bth_transaction_types::TAG_WEIGHT_SCALE,
            "the import tag must be 100% weight"
        );
        // ...and it must NOT be detectable by the reserve (fresh one-time key).
        assert!(recipient_out.belongs_to(&reserve).is_none());

        // The change output returns to the reserve.
        let change_out = &tx.outputs[1];
        assert!(
            change_out.belongs_to(&reserve).is_some(),
            "change must return to the reserve (ADR 0003 provenance)"
        );

        // Two releases to the SAME recipient produce DISTINCT one-time keys.
        let inputs2 = {
            let reserve_out2 = TxOutput::new(owned_amount, &reserve.default_subaddress());
            let owned2 = scan_deposit_output(
                &rpc_output_from_txoutput(&reserve_out2, "0xreserve2", 0),
                &reserve,
                None,
            )
            .unwrap()
            .unwrap()
            .owned;
            vec![ReleaseInput {
                owned: owned2,
                decoys: make_decoys(DEFAULT_RING_SIZE - 1, owned_amount, &mut rng),
            }]
        };
        let tx2 = build_release_tx(
            &reserve, &recipient, amount, fee, &inputs2, 5_001, None, &mut rng,
        )
        .unwrap();
        assert_ne!(
            tx.outputs[0].target_key, tx2.outputs[0].target_key,
            "two releases to the same address must use distinct one-time keys (ADR 0004)"
        );
    }

    #[test]
    fn build_release_tx_rejects_insufficient_inputs() {
        let mut rng = StdRng::from_seed([13u8; 32]);
        let reserve = AccountKey::random(&mut rng);
        let recipient = AccountKey::random(&mut rng).default_subaddress();

        let owned_amount = 1_000_000_000u64;
        let reserve_out = TxOutput::new(owned_amount, &reserve.default_subaddress());
        let owned = scan_deposit_output(
            &rpc_output_from_txoutput(&reserve_out, "0xr", 0),
            &reserve,
            None,
        )
        .unwrap()
        .unwrap()
        .owned;
        let inputs = vec![ReleaseInput {
            owned,
            decoys: make_decoys(DEFAULT_RING_SIZE - 1, owned_amount, &mut rng),
        }];

        // amount + fee exceeds the single input.
        let err = build_release_tx(
            &reserve,
            &recipient,
            owned_amount,
            MIN_TX_FEE,
            &inputs,
            1,
            None,
            &mut rng,
        )
        .unwrap_err();
        assert!(err.contains("insufficient reserve inputs"), "got: {err}");
    }

    #[test]
    fn build_release_tx_rejects_too_few_decoys() {
        let mut rng = StdRng::from_seed([17u8; 32]);
        let reserve = AccountKey::random(&mut rng);
        let recipient = AccountKey::random(&mut rng).default_subaddress();

        let owned_amount = 10_000_000_000_000u64;
        let reserve_out = TxOutput::new(owned_amount, &reserve.default_subaddress());
        let owned = scan_deposit_output(
            &rpc_output_from_txoutput(&reserve_out, "0xr", 0),
            &reserve,
            None,
        )
        .unwrap()
        .unwrap()
        .owned;
        let mut short = make_decoys(DEFAULT_RING_SIZE - 1, owned_amount, &mut rng);
        short.truncate(2);
        let inputs = vec![ReleaseInput {
            owned,
            decoys: short,
        }];
        let err = build_release_tx(
            &reserve,
            &recipient,
            4_000_000_000_000,
            MIN_TX_FEE,
            &inputs,
            1,
            None,
            &mut rng,
        )
        .unwrap_err();
        assert!(err.contains("not enough decoys"), "got: {err}");
    }

    #[test]
    fn shuffle_is_a_permutation() {
        let mut rng = StdRng::from_seed([23u8; 32]);
        let mut items: Vec<u32> = (0..40).collect();
        let original = items.clone();
        super::shuffle(&mut items, &mut rng);
        items.sort();
        assert_eq!(items, original);
    }
}
