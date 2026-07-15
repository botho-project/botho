// Copyright (c) 2024 Botho Foundation

//! Pure-Rust transaction build + CLSAG sign core.
//!
//! This module has no `wasm-bindgen` dependency so it can be unit-tested
//! natively with `cargo test`. The wasm layer in `lib.rs` is a thin serde shim
//! over [`build_and_sign_inner`].

use bth_account_keys::{AccountKey, PublicAddress};
use bth_crypto_keys::{RistrettoPrivate, RistrettoPublic};
use bth_transaction_clsag::{
    ClsagRingInput, RingMember, Transaction, TxOutput, DEFAULT_RING_SIZE, DUST_THRESHOLD,
    MIN_TX_FEE,
};
use rand_core::{CryptoRng, RngCore};
use serde::{Deserialize, Serialize};

/// A ring member (decoy or real output) the wallet sourced from the chain.
///
/// `target_key` and `public_key` are the 32-byte stealth keys of a real
/// on-chain output, hex-encoded. The signer reconstructs the trivial Pedersen
/// commitment from `amount` (the transparent-amount model), so the caller must
/// supply the output's amount as well.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DecoyOutput {
    /// Hex-encoded 32-byte one-time target key of the output.
    pub target_key: String,
    /// Hex-encoded 32-byte ephemeral public key of the output.
    pub public_key: String,
    /// Amount in picocredits committed by this output.
    pub amount: u64,
}

/// One of the wallet's own outputs being spent, plus the subaddress that owns
/// it (so the signer can recover the one-time private key).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpendInput {
    /// Hex-encoded 32-byte one-time target key of the owned output.
    pub target_key: String,
    /// Hex-encoded 32-byte ephemeral public key of the owned output.
    pub public_key: String,
    /// Amount in picocredits of the owned output.
    pub amount: u64,
    /// Subaddress index that received this output (0 = default, 1 = change).
    pub subaddress_index: u64,
    /// Decoys for this input's ring. Must contain at least `ringSize - 1`
    /// distinct outputs (the signer uses exactly `ringSize - 1`).
    pub decoys: Vec<DecoyOutput>,
}

/// A recipient address, as the two 32-byte Ristretto public keys.
///
/// The browser wallet decodes whatever address format it uses (e.g. base58)
/// into these raw keys before calling the signer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecipientAddress {
    /// Hex-encoded 32-byte spend public key (`D`).
    pub spend_public_key: String,
    /// Hex-encoded 32-byte view public key (`C`).
    pub view_public_key: String,
}

/// Encode a [`PublicAddress`] as a `botho://2/<base58>` / `tbotho://2/<base58>`
/// address string via the shared [`bth_address_codec`] (ADR 0008 D5).
///
/// This is the browser wallet's Rust entry point for producing an address
/// string: routing through the shared codec guarantees the wasm build is
/// byte-identical to the node and mobile encoders (no hand-rolled base58 in
/// JavaScript). The address must carry both post-quantum keys.
pub fn encode_address_string(addr: &PublicAddress, testnet: bool) -> Result<String, String> {
    let network = if testnet {
        bth_address_codec::Network::Testnet
    } else {
        bth_address_codec::Network::Mainnet
    };
    bth_address_codec::encode_address(addr, network).map_err(|e| e.to_string())
}

/// Decode a `botho://2/…` / `tbotho://2/…` address string into a
/// [`PublicAddress`] via the shared [`bth_address_codec`].
///
/// Old 64-byte v1 (`botho://1/…`) and the retired quantum prefixes are rejected
/// with a clear error.
pub fn decode_address_string(s: &str) -> Result<PublicAddress, String> {
    bth_address_codec::decode_address(s)
        .map(|(addr, _network)| addr)
        .map_err(|e| e.to_string())
}

/// A v2 address decoded into its four raw components, hex-encoded, for the JS
/// boundary.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodedV2Address {
    /// `"mainnet"` or `"testnet"`.
    pub network: String,
    /// Hex-encoded 32-byte view public key.
    pub view_public_key: String,
    /// Hex-encoded 32-byte spend public key.
    pub spend_public_key: String,
    /// Hex-encoded raw ML-KEM-768 public key (1184 bytes).
    pub kem_public_key: String,
    /// Hex-encoded raw ML-DSA-65 public key (1952 bytes).
    pub dsa_public_key: String,
}

/// Decode a v2 address string into hex components for JavaScript.
///
/// The browser wallet uses this instead of a hand-rolled base58 decoder so its
/// parsing is byte-identical to the node/mobile/wallet encoders.
pub fn decode_address_to_dto(s: &str) -> Result<DecodedV2Address, String> {
    let (addr, network) = bth_address_codec::decode_address(s).map_err(|e| e.to_string())?;
    let network = match network {
        bth_address_codec::Network::Mainnet => "mainnet",
        bth_address_codec::Network::Testnet => "testnet",
    };
    Ok(DecodedV2Address {
        network: network.to_string(),
        view_public_key: hex::encode(addr.view_public_key().to_bytes()),
        spend_public_key: hex::encode(addr.spend_public_key().to_bytes()),
        kem_public_key: hex::encode(addr.kem_public_key()),
        dsa_public_key: hex::encode(addr.dsa_public_key()),
    })
}

/// A wallet's post-quantum public keys, derived from its BIP39 seed, hex-encoded
/// for the JS boundary.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DerivedPqPublicKeys {
    /// Hex-encoded raw ML-KEM-768 public key (1184 bytes).
    pub kem_public_key: String,
    /// Hex-encoded raw ML-DSA-65 public key (1952 bytes).
    pub dsa_public_key: String,
}

/// Parse a hex-encoded 64-byte BIP39 seed.
fn parse_bip39_seed(seed_hex: &str) -> Result<[u8; bth_crypto_pq::BIP39_SEED_SIZE], String> {
    let bytes = hex::decode(seed_hex.trim()).map_err(|e| format!("invalid seed hex: {e}"))?;
    if bytes.len() != bth_crypto_pq::BIP39_SEED_SIZE {
        return Err(format!(
            "BIP39 seed must be {} bytes, got {}",
            bth_crypto_pq::BIP39_SEED_SIZE,
            bytes.len()
        ));
    }
    let mut out = [0u8; bth_crypto_pq::BIP39_SEED_SIZE];
    out.copy_from_slice(&bytes);
    Ok(out)
}

/// Derive a wallet's account-wide post-quantum public keys from its 64-byte
/// BIP39 seed, using the **node-identical** derivation
/// ([`bth_crypto_pq::derive_pq_keys_from_seed`]).
///
/// The browser wallet computes the BIP39 seed from the mnemonic in JavaScript
/// (`@scure/bip39`, empty passphrase) and calls this to obtain the raw
/// ML-KEM-768 / ML-DSA-65 public keys that go into its address-format-v2
/// address. Reusing the node's `derive_pq_keys_from_seed` guarantees the
/// browser-emitted address carries the exact PQ keys the node would derive for
/// the same seed, so an output built to the address is receivable.
pub fn derive_pq_public_keys_from_seed(seed_hex: &str) -> Result<DerivedPqPublicKeys, String> {
    let seed = parse_bip39_seed(seed_hex)?;
    let pq = bth_crypto_pq::derive_pq_keys_from_seed(&seed);
    Ok(DerivedPqPublicKeys {
        kem_public_key: hex::encode(pq.kem_keypair.public_key().as_bytes()),
        dsa_public_key: hex::encode(pq.sig_keypair.public_key().as_bytes()),
    })
}

/// Derive a browser wallet's full address-format-v2 (`botho://2/…`) string from
/// its BIP39 seed and its classical default-subaddress public keys.
///
/// This is the browser wallet's single entry point for producing its own
/// shareable address. It mirrors the node's `WalletKeys::public_address_string`
/// exactly:
///
///   1. the classical view/spend keys are the account's **default-subaddress**
///      (index 0) Ristretto public keys — derived in TypeScript
///      (`deriveDefaultSubaddressPublicKeys`, pinned byte-identical to the node
///      by `derivation-parity.test.ts`) and passed in as hex;
///   2. the ML-KEM-768 / ML-DSA-65 public keys are derived from the same BIP39
///      seed via the node-identical [`bth_crypto_pq::derive_pq_keys_from_seed`];
///   3. the whole [`PublicAddress`] is encoded through the shared
///      [`bth_address_codec`] (ADR 0008 D5), so the string is byte-identical to
///      the node / mobile / CLI encoders.
///
/// The result is a `botho://2/…` (or `tbotho://2/…`) address the node accepts
/// and can receive to.
pub fn derive_address_from_seed(
    seed_hex: &str,
    view_hex: &str,
    spend_hex: &str,
    testnet: bool,
) -> Result<String, String> {
    let pq = derive_pq_public_keys_from_seed(seed_hex)?;
    encode_address_from_hex(view_hex, spend_hex, &pq.kem_public_key, &pq.dsa_public_key, testnet)
}

/// Encode a v2 address string from hex components (the JS boundary form).
///
/// `kem_hex` / `dsa_hex` must be the raw ML-KEM-768 (1184 B) / ML-DSA-65
/// (1952 B) public keys. Routes through the shared codec.
pub fn encode_address_from_hex(
    view_hex: &str,
    spend_hex: &str,
    kem_hex: &str,
    dsa_hex: &str,
    testnet: bool,
) -> Result<String, String> {
    let view = RistrettoPublic::try_from(
        hex::decode(view_hex.trim())
            .map_err(|e| format!("invalid view key hex: {e}"))?
            .as_slice(),
    )
    .map_err(|e| format!("invalid view key: {e}"))?;
    let spend = RistrettoPublic::try_from(
        hex::decode(spend_hex.trim())
            .map_err(|e| format!("invalid spend key hex: {e}"))?
            .as_slice(),
    )
    .map_err(|e| format!("invalid spend key: {e}"))?;
    let kem = hex::decode(kem_hex.trim()).map_err(|e| format!("invalid kem key hex: {e}"))?;
    let dsa = hex::decode(dsa_hex.trim()).map_err(|e| format!("invalid dsa key hex: {e}"))?;

    let addr = PublicAddress::new_with_pq(&spend, &view, kem, dsa);
    encode_address_string(&addr, testnet)
}

/// The full client-side signing request.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignRequest {
    /// Hex-encoded 32-byte account spend private key. **Stays client-side.**
    pub spend_private_key: String,
    /// Hex-encoded 32-byte account view private key. **Stays client-side.**
    pub view_private_key: String,
    /// Owned outputs being spent (one ring per input).
    pub inputs: Vec<SpendInput>,
    /// Recipient of the transfer.
    pub recipient: RecipientAddress,
    /// Amount to send to the recipient, in picocredits.
    pub amount: u64,
    /// Transaction fee in picocredits.
    pub fee: u64,
    /// Chain height to stamp the transaction with (replay protection).
    pub created_at_height: u64,
}

fn parse_hex_32(field: &str, s: &str) -> Result<[u8; 32], String> {
    let bytes = hex::decode(s).map_err(|e| format!("{field}: invalid hex: {e}"))?;
    if bytes.len() != 32 {
        return Err(format!("{field}: expected 32 bytes, got {}", bytes.len()));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

fn parse_private(field: &str, s: &str) -> Result<RistrettoPrivate, String> {
    let bytes = parse_hex_32(field, s)?;
    RistrettoPrivate::try_from(&bytes).map_err(|e| format!("{field}: invalid private key: {e:?}"))
}

fn parse_public(field: &str, s: &str) -> Result<RistrettoPublic, String> {
    let bytes = parse_hex_32(field, s)?;
    RistrettoPublic::try_from(&bytes).map_err(|e| format!("{field}: invalid public key: {e:?}"))
}

/// Reconstruct a [`RingMember`] from a chain output's keys + amount.
///
/// Uses the same trivial (zero-blinding) Pedersen commitment as
/// [`RingMember::from_output`], so commitments match the node's view of the
/// output.
fn ring_member_from_parts(
    target_key: &str,
    public_key: &str,
    amount: u64,
) -> Result<RingMember, String> {
    // Build a transparent TxOutput shell with the on-chain stealth keys, then
    // reuse RingMember::from_output so the commitment derivation is identical
    // to the node's.
    let output = TxOutput {
        amount,
        target_key: parse_hex_32("target_key", target_key)?,
        public_key: parse_hex_32("public_key", public_key)?,
        e_memo: None,
        cluster_tags: Default::default(),
        kem_ciphertext: None,
    };
    Ok(RingMember::from_output(&output))
}

/// Build and CLSAG-sign a transaction using a caller-supplied RNG.
///
/// Separated from [`build_and_sign_inner`] so tests can pass a deterministic
/// RNG for reproducible vectors.
pub fn build_and_sign_with_rng<R: RngCore + CryptoRng>(
    req: &SignRequest,
    rng: &mut R,
) -> Result<Transaction, String> {
    if req.inputs.is_empty() {
        return Err("at least one input is required".to_string());
    }
    if req.amount == 0 {
        return Err("amount must be greater than 0".to_string());
    }
    if req.amount < DUST_THRESHOLD {
        return Err(format!(
            "amount {} is below dust threshold {}",
            req.amount, DUST_THRESHOLD
        ));
    }
    if req.fee < MIN_TX_FEE {
        return Err(format!("fee {} is below minimum {}", req.fee, MIN_TX_FEE));
    }

    // Reconstruct the spending account from the client-side private keys.
    let spend_private = parse_private("spendPrivateKey", &req.spend_private_key)?;
    let view_private = parse_private("viewPrivateKey", &req.view_private_key)?;
    let account = AccountKey::new(&spend_private, &view_private);

    // Reconstruct the recipient address.
    let recipient = PublicAddress::new(
        &parse_public("recipient.spendPublicKey", &req.recipient.spend_public_key)?,
        &parse_public("recipient.viewPublicKey", &req.recipient.view_public_key)?,
    );

    // Sum inputs and validate the balance equation up front.
    let mut input_total: u64 = 0;
    for input in &req.inputs {
        input_total = input_total
            .checked_add(input.amount)
            .ok_or("input amount sum overflow")?;
    }
    let spent = req
        .amount
        .checked_add(req.fee)
        .ok_or("amount + fee overflow")?;
    let change = input_total
        .checked_sub(spent)
        .ok_or("insufficient funds: inputs do not cover amount + fee")?;

    // Build outputs: recipient + (optional) change back to our default
    // subaddress. Sub-dust change is folded into the fee so we never create an
    // unspendable output and the balance equation still holds exactly.
    let mut outputs = vec![TxOutput::new(req.amount, &recipient)];
    let actual_fee = if change >= DUST_THRESHOLD {
        outputs.push(TxOutput::new(change, &account.default_subaddress()));
        req.fee
    } else {
        req.fee + change
    };

    // Preliminary (input-less) tx to compute the signing hash the ring
    // signatures commit to. The signing hash only depends on outputs, fee, and
    // height (not on inputs/signatures), so this matches the final tx.
    let preliminary = Transaction::new_clsag(
        Vec::new(),
        outputs.clone(),
        actual_fee,
        req.created_at_height,
    );
    let signing_hash = preliminary.signing_hash();

    // Build one CLSAG ring input per spent output.
    let mut ring_inputs = Vec::with_capacity(req.inputs.len());
    for input in &req.inputs {
        let needed = DEFAULT_RING_SIZE - 1;
        if input.decoys.len() < needed {
            return Err(format!(
                "not enough decoys for input: need {}, got {}",
                needed,
                input.decoys.len()
            ));
        }

        // Recover the one-time private key for this owned output.
        let real_output = TxOutput {
            amount: input.amount,
            target_key: parse_hex_32("input.target_key", &input.target_key)?,
            public_key: parse_hex_32("input.public_key", &input.public_key)?,
            e_memo: None,
            cluster_tags: Default::default(),
            kem_ciphertext: None,
        };
        let onetime_private = real_output
            .recover_spend_key(&account, input.subaddress_index)
            .ok_or("failed to recover one-time private key for input")?;

        // Assemble the ring: real member + (ringSize - 1) decoys, then shuffle
        // so the real input's position is hidden.
        let real_member = RingMember::from_output(&real_output);
        let real_target = real_member.target_key;
        let mut ring: Vec<RingMember> = Vec::with_capacity(DEFAULT_RING_SIZE);
        ring.push(real_member);
        for decoy in input.decoys.iter().take(needed) {
            ring.push(ring_member_from_parts(
                &decoy.target_key,
                &decoy.public_key,
                decoy.amount,
            )?);
        }
        shuffle(&mut ring, rng);
        let real_index = ring
            .iter()
            .position(|m| m.target_key == real_target)
            .ok_or("internal error: real input lost during shuffle")?;

        let ring_input = ClsagRingInput::new(
            ring,
            real_index,
            &onetime_private,
            input.amount,
            &signing_hash,
            rng,
        )?;
        ring_inputs.push(ring_input);
    }

    let tx = Transaction::new_clsag(ring_inputs, outputs, actual_fee, req.created_at_height);

    // Defensive self-check: the produced tx must be structurally valid and its
    // ring signatures + balance equation must verify under the same code path
    // the node runs. This guarantees we never hand the caller a tx the node
    // would reject for a reason we could have caught locally.
    tx.is_valid_structure()
        .map_err(|e| format!("produced an invalid transaction structure: {e}"))?;
    tx.verify_ring_signatures()
        .map_err(|e| format!("produced a transaction that fails verification: {e}"))?;

    Ok(tx)
}

/// A chain output the wallet wants to test for ownership / use as a decoy.
///
/// These are exactly the fields the node returns from `chain_getOutputs`
/// (`targetKey`, `publicKey`, and the transparent `amount` recovered from
/// `amountCommitment`).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChainOutput {
    /// Hex-encoded 32-byte one-time target key of the output.
    pub target_key: String,
    /// Hex-encoded 32-byte ephemeral public key of the output.
    pub public_key: String,
    /// Amount in picocredits (recovered from the transparent commitment).
    pub amount: u64,
}

/// A scan request: the account's private keys plus candidate chain outputs.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanRequest {
    /// Hex-encoded 32-byte account spend private key. **Stays client-side.**
    pub spend_private_key: String,
    /// Hex-encoded 32-byte account view private key. **Stays client-side.**
    pub view_private_key: String,
    /// Candidate outputs (e.g. every output the node returned for a height
    /// range) to test for ownership.
    pub outputs: Vec<ChainOutput>,
}

/// An owned output the scan identified, ready to be turned into a spend input.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnedOutput {
    /// Hex-encoded 32-byte one-time target key of the owned output.
    pub target_key: String,
    /// Hex-encoded 32-byte ephemeral public key of the owned output.
    pub public_key: String,
    /// Amount in picocredits of the owned output.
    pub amount: u64,
    /// Subaddress index that received this output (0 = default, 1 = change).
    pub subaddress_index: u64,
}

/// Identify which of `outputs` belong to the account, using the
/// **node-identical** stealth-address ownership check
/// ([`TxOutput::belongs_to`]).
///
/// This runs the same Rust the node runs in `scan_utxos_for_account`, so a
/// thin client never has to re-implement the subaddress math in JavaScript (a
/// notorious source of cross-implementation drift). The view/spend keys are
/// used only to recover the stealth relationship and never leave the client.
pub fn scan_owned_outputs_inner(req: &ScanRequest) -> Result<Vec<OwnedOutput>, String> {
    let spend_private = parse_private("spendPrivateKey", &req.spend_private_key)?;
    let view_private = parse_private("viewPrivateKey", &req.view_private_key)?;
    let account = AccountKey::new(&spend_private, &view_private);

    let mut owned = Vec::new();
    for out in &req.outputs {
        let tx_out = TxOutput {
            amount: out.amount,
            target_key: parse_hex_32("output.target_key", &out.target_key)?,
            public_key: parse_hex_32("output.public_key", &out.public_key)?,
            e_memo: None,
            cluster_tags: Default::default(),
            kem_ciphertext: None,
        };
        if let Some(subaddress_index) = tx_out.belongs_to(&account) {
            owned.push(OwnedOutput {
                target_key: out.target_key.clone(),
                public_key: out.public_key.clone(),
                amount: out.amount,
                subaddress_index,
            });
        }
    }
    Ok(owned)
}

/// Request to compute key images for a set of owned outputs.
///
/// The wallet supplies its private keys plus the outputs it owns (as returned
/// by [`scan_owned_outputs_inner`]). The signer recovers each output's one-time
/// private key and derives its key image — exactly the value the node records
/// in its double-spend set when the output is spent.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyImageRequest {
    /// Hex-encoded 32-byte account spend private key. **Stays client-side.**
    pub spend_private_key: String,
    /// Hex-encoded 32-byte account view private key. **Stays client-side.**
    pub view_private_key: String,
    /// The wallet's owned outputs to derive key images for.
    pub outputs: Vec<OwnedOutput>,
}

/// An owned output paired with its derived key image.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnedOutputKeyImage {
    /// Hex-encoded 32-byte one-time target key of the owned output.
    pub target_key: String,
    /// Hex-encoded 32-byte ephemeral public key of the owned output.
    pub public_key: String,
    /// Amount in picocredits of the owned output.
    pub amount: u64,
    /// Subaddress index that received this output (0 = default, 1 = change).
    pub subaddress_index: u64,
    /// Hex-encoded 32-byte key image. Querying the node's
    /// `chain_areKeyImagesSpent` RPC with this value reveals whether the output
    /// has already been spent on-chain (or is pending in the mempool).
    pub key_image: String,
}

/// Derive the key image for each owned output.
///
/// Uses the **node-identical** derivation: recover the one-time private key via
/// [`TxOutput::recover_spend_key`] (same as the node's
/// `recover_spend_key`), then `KeyImage::from(&onetime_private)` —
/// byte-for-byte what the node records in its double-spend set and checks in
/// `wallet_getBalance` / `handle_are_key_images_spent`. This lets a thin wallet
/// learn which of its owned outputs are spent without re-implementing the
/// derivation in JS.
pub fn compute_owned_output_key_images_inner(
    req: &KeyImageRequest,
) -> Result<Vec<OwnedOutputKeyImage>, String> {
    use bth_crypto_ring_signature::KeyImage;

    let spend_private = parse_private("spendPrivateKey", &req.spend_private_key)?;
    let view_private = parse_private("viewPrivateKey", &req.view_private_key)?;
    let account = AccountKey::new(&spend_private, &view_private);

    let mut result = Vec::with_capacity(req.outputs.len());
    for out in &req.outputs {
        let tx_out = TxOutput {
            amount: out.amount,
            target_key: parse_hex_32("output.target_key", &out.target_key)?,
            public_key: parse_hex_32("output.public_key", &out.public_key)?,
            e_memo: None,
            cluster_tags: Default::default(),
            kem_ciphertext: None,
        };
        let onetime_private = tx_out
            .recover_spend_key(&account, out.subaddress_index)
            .ok_or("failed to recover one-time private key for owned output")?;
        let key_image = KeyImage::from(&onetime_private);
        result.push(OwnedOutputKeyImage {
            target_key: out.target_key.clone(),
            public_key: out.public_key.clone(),
            amount: out.amount,
            subaddress_index: out.subaddress_index,
            key_image: hex::encode(key_image.as_bytes()),
        });
    }
    Ok(result)
}

/// Build, CLSAG-sign, and bincode-serialize a transaction, returning hex.
///
/// The returned hex is the exact `tx_hex` payload accepted by the node's
/// `tx_submit` RPC.
pub fn build_and_sign_inner(req: &SignRequest) -> Result<String, String> {
    let mut rng = rand_core::OsRng;
    let tx = build_and_sign_with_rng(req, &mut rng)?;
    let bytes = bincode::serialize(&tx).map_err(|e| format!("serialization failed: {e}"))?;
    Ok(hex::encode(bytes))
}

/// Fisher-Yates shuffle using the provided RNG (avoids pulling in `rand`'s
/// `SliceRandom` so the wasm build stays lean).
fn shuffle<T, R: RngCore>(items: &mut [T], rng: &mut R) {
    let len = items.len();
    if len <= 1 {
        return;
    }
    for i in (1..len).rev() {
        // Unbiased index in [0, i] via rejection sampling.
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
    use bth_account_keys::AccountKey;
    use bth_transaction_clsag::TxOutput;
    use rand::{rngs::StdRng, SeedableRng};

    /// Create a recipient address request fragment from an account.
    fn recipient_of(account: &AccountKey) -> RecipientAddress {
        let addr = account.default_subaddress();
        RecipientAddress {
            spend_public_key: hex::encode(addr.spend_public_key().to_bytes()),
            view_public_key: hex::encode(addr.view_public_key().to_bytes()),
        }
    }

    /// Make `count` random decoy outputs paid to a throwaway recipient.
    fn make_decoys(count: usize, amount: u64, rng: &mut StdRng) -> Vec<DecoyOutput> {
        (0..count)
            .map(|_| {
                let mut seed = [0u8; 32];
                rng.fill_bytes(&mut seed);
                let decoy_account = AccountKey::random(rng);
                let out = TxOutput::new(amount, &decoy_account.default_subaddress());
                DecoyOutput {
                    target_key: hex::encode(out.target_key),
                    public_key: hex::encode(out.public_key),
                    amount,
                }
            })
            .collect()
    }

    /// Build a sender account, give it an owned output, and produce a full
    /// signing request that sends `send_amount` with `fee`.
    fn make_request(
        sender: &AccountKey,
        owned_amount: u64,
        send_amount: u64,
        fee: u64,
        rng: &mut StdRng,
    ) -> SignRequest {
        // The wallet's own output: a stealth output paid to the sender's
        // default subaddress, with a known ephemeral key so we can recover it.
        let mut eph = [0u8; 32];
        rng.fill_bytes(&mut eph);
        let owned = TxOutput::new(owned_amount, &sender.default_subaddress());

        let decoys = make_decoys(DEFAULT_RING_SIZE - 1, owned_amount, rng);

        let recipient_account = AccountKey::random(rng);

        SignRequest {
            spend_private_key: hex::encode(sender.spend_private_key().to_bytes()),
            view_private_key: hex::encode(sender.view_private_key().to_bytes()),
            inputs: vec![SpendInput {
                target_key: hex::encode(owned.target_key),
                public_key: hex::encode(owned.public_key),
                amount: owned_amount,
                subaddress_index: 0,
                decoys,
            }],
            recipient: recipient_of(&recipient_account),
            amount: send_amount,
            fee,
            created_at_height: 1000,
        }
    }

    #[test]
    fn sign_then_verify_roundtrip() {
        let mut rng = StdRng::from_seed([7u8; 32]);
        let sender = AccountKey::random(&mut rng);

        let owned_amount = 10_000_000_000u64; // 0.01 BTH
        let send_amount = 5_000_000_000u64;
        let fee = MIN_TX_FEE;
        let req = make_request(&sender, owned_amount, send_amount, fee, &mut rng);

        let tx = build_and_sign_with_rng(&req, &mut rng).expect("build+sign should succeed");

        // The produced tx must verify under the node's verifier.
        tx.verify_ring_signatures()
            .expect("node verifier must accept the signed tx");
        tx.is_valid_structure().expect("structure must be valid");

        // Balance: recipient + change == inputs - fee.
        assert_eq!(tx.inputs.len(), 1);
        let out_total: u64 = tx.outputs.iter().map(|o| o.amount).sum();
        assert_eq!(out_total + tx.fee, owned_amount);
        assert_eq!(tx.inputs.clsag()[0].ring.len(), DEFAULT_RING_SIZE);
    }

    #[test]
    fn bincode_roundtrips_to_same_transaction() {
        let mut rng = StdRng::from_seed([9u8; 32]);
        let sender = AccountKey::random(&mut rng);
        let req = make_request(&sender, 10_000_000_000, 5_000_000_000, MIN_TX_FEE, &mut rng);

        let tx = build_and_sign_with_rng(&req, &mut rng).unwrap();
        let bytes = bincode::serialize(&tx).unwrap();

        // The hex output of build_and_sign_inner must deserialize to a verifying tx.
        let decoded: Transaction = bincode::deserialize(&bytes).unwrap();
        decoded
            .verify_ring_signatures()
            .expect("deserialized tx must still verify");
        assert_eq!(decoded.fee, tx.fee);
        assert_eq!(decoded.outputs.len(), tx.outputs.len());
    }

    /// #392: the key image the wallet computes for an owned output must equal
    /// the key image embedded in a CLSAG signature spending that same output.
    /// If they match, the wallet can reliably query the node's
    /// `chain_areKeyImagesSpent` and exclude spent outputs from its balance.
    #[test]
    fn computed_key_image_matches_signed_input() {
        let mut rng = StdRng::from_seed([13u8; 32]);
        let sender = AccountKey::random(&mut rng);

        // The wallet's own output (default subaddress, index 0).
        let owned_amount = 10_000_000_000u64;
        let owned = TxOutput::new(owned_amount, &sender.default_subaddress());

        // Compute the key image via the wallet path.
        let ki_req = KeyImageRequest {
            spend_private_key: hex::encode(sender.spend_private_key().to_bytes()),
            view_private_key: hex::encode(sender.view_private_key().to_bytes()),
            outputs: vec![OwnedOutput {
                target_key: hex::encode(owned.target_key),
                public_key: hex::encode(owned.public_key),
                amount: owned_amount,
                subaddress_index: 0,
            }],
        };
        let computed = compute_owned_output_key_images_inner(&ki_req).unwrap();
        assert_eq!(computed.len(), 1);

        // Build + sign a tx spending the same output, then read the key image
        // the CLSAG signature actually used.
        let decoys = make_decoys(DEFAULT_RING_SIZE - 1, owned_amount, &mut rng);
        let recipient_account = AccountKey::random(&mut rng);
        let req = SignRequest {
            spend_private_key: hex::encode(sender.spend_private_key().to_bytes()),
            view_private_key: hex::encode(sender.view_private_key().to_bytes()),
            inputs: vec![SpendInput {
                target_key: hex::encode(owned.target_key),
                public_key: hex::encode(owned.public_key),
                amount: owned_amount,
                subaddress_index: 0,
                decoys,
            }],
            recipient: recipient_of(&recipient_account),
            amount: 5_000_000_000,
            fee: MIN_TX_FEE,
            created_at_height: 1000,
        };
        let tx = build_and_sign_with_rng(&req, &mut rng).unwrap();
        let signed_ki = hex::encode(tx.inputs.clsag()[0].key_image());

        assert_eq!(
            computed[0].key_image, signed_ki,
            "wallet-computed key image must match the CLSAG signature's key image"
        );
    }

    /// #392: ownership scan + key-image derivation must agree on which outputs
    /// are the wallet's. An output paid to a different account yields no owned
    /// outputs, hence no key images.
    #[test]
    fn key_images_only_for_owned_outputs() {
        let mut rng = StdRng::from_seed([17u8; 32]);
        let me = AccountKey::random(&mut rng);
        let other = AccountKey::random(&mut rng);

        let mine = TxOutput::new(1_000_000_000, &me.default_subaddress());
        let theirs = TxOutput::new(2_000_000_000, &other.default_subaddress());

        let scan = ScanRequest {
            spend_private_key: hex::encode(me.spend_private_key().to_bytes()),
            view_private_key: hex::encode(me.view_private_key().to_bytes()),
            outputs: vec![
                ChainOutput {
                    target_key: hex::encode(mine.target_key),
                    public_key: hex::encode(mine.public_key),
                    amount: 1_000_000_000,
                },
                ChainOutput {
                    target_key: hex::encode(theirs.target_key),
                    public_key: hex::encode(theirs.public_key),
                    amount: 2_000_000_000,
                },
            ],
        };
        let owned = scan_owned_outputs_inner(&scan).unwrap();
        assert_eq!(owned.len(), 1, "only my output should be owned");

        let ki_req = KeyImageRequest {
            spend_private_key: scan.spend_private_key.clone(),
            view_private_key: scan.view_private_key.clone(),
            outputs: owned,
        };
        let kis = compute_owned_output_key_images_inner(&ki_req).unwrap();
        assert_eq!(kis.len(), 1);
        assert_eq!(kis[0].amount, 1_000_000_000);
    }

    #[test]
    fn dust_change_is_folded_into_fee() {
        let mut rng = StdRng::from_seed([11u8; 32]);
        let sender = AccountKey::random(&mut rng);

        // Pick amounts so change is below the dust threshold.
        let owned_amount = 10_000_000_000u64;
        let fee = MIN_TX_FEE;
        // change = owned - amount - fee, want change < DUST_THRESHOLD but > 0
        let send_amount = owned_amount - fee - (DUST_THRESHOLD / 2);
        let req = make_request(&sender, owned_amount, send_amount, fee, &mut rng);

        let tx = build_and_sign_with_rng(&req, &mut rng).unwrap();

        // No change output: only the recipient output.
        assert_eq!(tx.outputs.len(), 1);
        // Dust was folded into the fee; balance still holds exactly.
        let out_total: u64 = tx.outputs.iter().map(|o| o.amount).sum();
        assert_eq!(out_total + tx.fee, owned_amount);
        assert!(tx.fee > fee);
        tx.verify_ring_signatures().unwrap();
    }

    #[test]
    fn insufficient_funds_is_rejected() {
        let mut rng = StdRng::from_seed([13u8; 32]);
        let sender = AccountKey::random(&mut rng);
        // owned < amount + fee
        let req = make_request(&sender, 3_000_000_000, 5_000_000_000, MIN_TX_FEE, &mut rng);
        let err = build_and_sign_with_rng(&req, &mut rng).unwrap_err();
        assert!(err.contains("insufficient funds"), "got: {err}");
    }

    #[test]
    fn too_few_decoys_is_rejected() {
        let mut rng = StdRng::from_seed([17u8; 32]);
        let sender = AccountKey::random(&mut rng);
        let mut req = make_request(&sender, 10_000_000_000, 5_000_000_000, MIN_TX_FEE, &mut rng);
        req.inputs[0].decoys.truncate(2); // far fewer than ring size - 1
        let err = build_and_sign_with_rng(&req, &mut rng).unwrap_err();
        assert!(err.contains("not enough decoys"), "got: {err}");
    }

    #[test]
    fn fee_below_minimum_is_rejected() {
        let mut rng = StdRng::from_seed([19u8; 32]);
        let sender = AccountKey::random(&mut rng);
        let req = make_request(
            &sender,
            10_000_000_000,
            5_000_000_000,
            MIN_TX_FEE - 1,
            &mut rng,
        );
        let err = build_and_sign_with_rng(&req, &mut rng).unwrap_err();
        assert!(err.contains("below minimum"), "got: {err}");
    }

    #[test]
    fn scan_identifies_owned_outputs_only() {
        let mut rng = StdRng::from_seed([29u8; 32]);
        let me = AccountKey::random(&mut rng);
        let stranger = AccountKey::random(&mut rng);

        // Two outputs to me (default subaddress) and one to a stranger.
        let mine_a = TxOutput::new(1_000, &me.default_subaddress());
        let mine_b = TxOutput::new(2_000, &me.change_subaddress());
        let theirs = TxOutput::new(3_000, &stranger.default_subaddress());

        let to_chain = |o: &TxOutput| ChainOutput {
            target_key: hex::encode(o.target_key),
            public_key: hex::encode(o.public_key),
            amount: o.amount,
        };

        let req = ScanRequest {
            spend_private_key: hex::encode(me.spend_private_key().to_bytes()),
            view_private_key: hex::encode(me.view_private_key().to_bytes()),
            outputs: vec![to_chain(&mine_a), to_chain(&theirs), to_chain(&mine_b)],
        };

        let owned = scan_owned_outputs_inner(&req).expect("scan should succeed");
        // Exactly the two outputs paid to me, with correct subaddress indices.
        assert_eq!(owned.len(), 2);
        let by_amount: std::collections::BTreeMap<u64, u64> = owned
            .iter()
            .map(|o| (o.amount, o.subaddress_index))
            .collect();
        assert_eq!(by_amount.get(&1_000), Some(&0)); // default subaddress
        assert_eq!(by_amount.get(&2_000), Some(&1)); // change subaddress
        assert!(!by_amount.contains_key(&3_000)); // stranger's output excluded
    }

    /// Regression test for #383: an output built TO a recipient's wallet
    /// address (the keys the TS `deriveAddress`/`formatAddress` now packs) MUST
    /// be detected by the recipient's own scan (`belongs_to`), and an output
    /// built to the recipient's ACCOUNT-ROOT keys (what the buggy address
    /// previously packed) must NOT be detected.
    ///
    /// The wallet address packs the recipient's DEFAULT-SUBADDRESS public keys
    /// (proven byte-identical to the node's `Account::subaddress(0)` by
    /// `derivation-parity.test.ts`). This test confirms the receiving end:
    /// building a stealth output to the default-subaddress address and scanning
    /// as the recipient detects it at subaddress index 0. The account-root case
    /// returning `None` is the exact gap that left funds unspendable before the
    /// fix.
    #[test]
    fn output_to_wallet_address_is_detected_by_recipient() {
        use bth_account_keys::PublicAddress;
        use bth_crypto_keys::RistrettoPublic;

        let mut rng = StdRng::from_seed([31u8; 32]);
        let recipient = AccountKey::random(&mut rng);

        // The address the wallet displays now packs the DEFAULT-SUBADDRESS keys.
        // (TS `deriveAddress` -> these exact bytes, see derivation-parity test.)
        let wallet_address = recipient.default_subaddress();
        let to_subaddress = TxOutput::new(7_000, &wallet_address);

        // The buggy pre-#383 address packed the ACCOUNT-ROOT keys instead.
        let root_spend_public = RistrettoPublic::from(recipient.spend_private_key());
        let root_view_public = RistrettoPublic::from(recipient.view_private_key());
        let account_root_address = PublicAddress::new(&root_spend_public, &root_view_public);
        let to_account_root = TxOutput::new(7_000, &account_root_address);

        let to_chain = |o: &TxOutput| ChainOutput {
            target_key: hex::encode(o.target_key),
            public_key: hex::encode(o.public_key),
            amount: o.amount,
        };

        let req = ScanRequest {
            spend_private_key: hex::encode(recipient.spend_private_key().to_bytes()),
            view_private_key: hex::encode(recipient.view_private_key().to_bytes()),
            outputs: vec![to_chain(&to_subaddress), to_chain(&to_account_root)],
        };

        let owned = scan_owned_outputs_inner(&req).expect("scan should succeed");

        // The output to the wallet (default-subaddress) address is detected at
        // index 0; the account-root output is invisible to the scan.
        assert_eq!(
            owned.len(),
            1,
            "exactly the default-subaddress output must be detected"
        );
        assert_eq!(owned[0].amount, 7_000);
        assert_eq!(owned[0].subaddress_index, 0);

        // Sanity: building to the account-root keys really is undetectable,
        // which is precisely the bug #383 fixes by changing the address
        // encoding to the default subaddress.
        assert!(
            to_account_root.belongs_to(&recipient).is_none(),
            "account-root output must NOT be detectable (the original bug)"
        );
    }

    /// #965: the browser's v2 address derivation must be byte-identical to the
    /// node's. This uses the canonical BIP39 test vector (the 12-word
    /// "abandon…about" mnemonic → the well-known Trezor 64-byte seed) plus the
    /// account's default-subaddress classical public keys (the exact bytes the
    /// node derives for that mnemonic, pinned by the web wallet's
    /// `derivation-parity.test.ts`), and asserts:
    ///
    ///   * the produced string is a `botho://2/…` / `tbotho://2/…` v2 address;
    ///   * it decodes back through the shared codec to the same view/spend keys;
    ///   * the embedded PQ keys equal `derive_pq_keys_from_seed(seed)`, i.e. the
    ///     node's own derivation.
    ///
    /// The printed address is the golden vector the TS `deriveV2Address` test
    /// asserts against, closing the loop web ⇄ node.
    #[test]
    fn v2_address_from_seed_matches_node_derivation() {
        // Canonical BIP39 seed for
        // "abandon abandon abandon abandon abandon abandon abandon abandon
        //  abandon abandon abandon about" (empty passphrase).
        let seed_hex = "5eb00bbddcf069084889a8ab9155568165f5c453ccb85e70811aaed6f6da5fc19a5ac40b389cd370d086206dec8aa6c43daea6690f20ad3d8d48b2d2ce9e38e4";
        // Default-subaddress (index 0) public keys the node derives for that
        // mnemonic (see web/packages/core/src/wallet/derivation-parity.test.ts).
        let view_hex = "60eeebc23d5d4fa3b90621292da88f39c6df05114bd405319cf9adc905905773";
        let spend_hex = "8e2cf7239559d62c6ca0c0718eac345da1fa9348aa741a94d6489025a05a917c";

        let addr_str = derive_address_from_seed(seed_hex, view_hex, spend_hex, true)
            .expect("derive v2 address");
        assert!(
            addr_str.starts_with("tbotho://2/"),
            "expected a testnet v2 address, got: {addr_str}"
        );
        println!("GOLDEN tbotho v2 address (abandon…about): {addr_str}");

        // Decodes back to the same classical keys via the shared codec.
        let decoded = decode_address_string(&addr_str).expect("decode produced address");
        assert_eq!(hex::encode(decoded.view_public_key().to_bytes()), view_hex);
        assert_eq!(hex::encode(decoded.spend_public_key().to_bytes()), spend_hex);

        // The embedded PQ keys are exactly the node's seed derivation.
        let mut seed = [0u8; bth_crypto_pq::BIP39_SEED_SIZE];
        hex::decode_to_slice(seed_hex, &mut seed).unwrap();
        let pq = bth_crypto_pq::derive_pq_keys_from_seed(&seed);
        assert_eq!(decoded.kem_public_key(), pq.kem_keypair.public_key().as_bytes());
        assert_eq!(decoded.dsa_public_key(), pq.sig_keypair.public_key().as_bytes());

        // Mainnet uses the same body under a different prefix.
        let main = derive_address_from_seed(seed_hex, view_hex, spend_hex, false).unwrap();
        assert!(main.starts_with("botho://2/"));
        assert_eq!(
            main.strip_prefix("botho://2/").unwrap(),
            addr_str.strip_prefix("tbotho://2/").unwrap()
        );
    }

    #[test]
    fn derive_pq_public_keys_from_seed_has_correct_lengths() {
        let seed_hex = "5eb00bbddcf069084889a8ab9155568165f5c453ccb85e70811aaed6f6da5fc19a5ac40b389cd370d086206dec8aa6c43daea6690f20ad3d8d48b2d2ce9e38e4";
        let pq = derive_pq_public_keys_from_seed(seed_hex).expect("derive pq");
        // ML-KEM-768 public key = 1184 bytes (2368 hex chars); ML-DSA-65 = 1952
        // bytes (3904 hex chars).
        assert_eq!(pq.kem_public_key.len(), 1184 * 2);
        assert_eq!(pq.dsa_public_key.len(), 1952 * 2);
        // Deterministic.
        let pq2 = derive_pq_public_keys_from_seed(seed_hex).unwrap();
        assert_eq!(pq.kem_public_key, pq2.kem_public_key);
        assert_eq!(pq.dsa_public_key, pq2.dsa_public_key);
    }

    #[test]
    fn shuffle_preserves_membership() {
        let mut rng = StdRng::from_seed([23u8; 32]);
        let mut items: Vec<u32> = (0..50).collect();
        let original: Vec<u32> = items.clone();
        super::shuffle(&mut items, &mut rng);
        let mut sorted = items.clone();
        sorted.sort();
        assert_eq!(sorted, original, "shuffle must be a permutation");
    }
}
