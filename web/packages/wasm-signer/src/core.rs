// Copyright (c) 2024 Botho Foundation

//! Pure-Rust transaction build + CLSAG sign core.
//!
//! This module has no `wasm-bindgen` dependency so it can be unit-tested
//! natively with `cargo test`. The wasm layer in `lib.rs` is a thin serde shim
//! over [`build_and_sign_inner`].

use bth_account_keys::{AccountKey, PublicAddress};
use bth_crypto_keys::{RistrettoPrivate, RistrettoPublic};
use bth_transaction_clsag::{
    ClsagRingInput, MemoPayload, RingMember, Transaction, TxOutput, DEFAULT_RING_SIZE,
    DUST_THRESHOLD, MIN_TX_FEE,
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
    /// The output's position within its creating transaction. Under protocol
    /// 6.0.0 this index is bound into the HYBRID one-time key, so it must be
    /// supplied to spend a hybrid (ciphertext-bearing) owned output (#988).
    /// Defaults to 0 for classical/legacy callers.
    #[serde(default)]
    pub output_index: u32,
    /// Hex-encoded ML-KEM-768 ciphertext of the owned output, or `None` for a
    /// classical/legacy KEM-less output. When present (and the request carries
    /// a `seed`), the signer decapsulates it to recover the HYBRID one-time
    /// private key — without it a 6.0.0 output's spend key cannot be derived
    /// and the produced ring signature would be invalid (#988).
    #[serde(default)]
    pub kem_ciphertext: Option<String>,
    /// Decoys for this input's ring. Must contain at least `ringSize - 1`
    /// distinct outputs (the signer uses exactly `ringSize - 1`).
    pub decoys: Vec<DecoyOutput>,
}

/// A recipient address: the two 32-byte Ristretto stealth keys PLUS the
/// recipient's raw ML-KEM-768 public key.
///
/// The browser wallet decodes the recipient's `botho://2/` (v2) address into
/// these raw components before calling the signer (`parseAddress` in
/// `@botho/core`, or the wasm `decodeAddress`, both of which expose
/// `kemPublic`).
///
/// Under protocol 6.0.0 every send output is a hybrid post-quantum stealth
/// output: the signer encapsulates a shared secret against `kem_public_key` and
/// attaches the resulting 1,088-byte ML-KEM ciphertext (issue #978). A missing
/// or malformed key is a hard error — a KEM-less output is rejected by
/// consensus (`validate_transfer_tx`, #974) — so this field is required, not
/// optional.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecipientAddress {
    /// Hex-encoded 32-byte spend public key (`D`).
    pub spend_public_key: String,
    /// Hex-encoded 32-byte view public key (`C`).
    pub view_public_key: String,
    /// Hex-encoded raw ML-KEM-768 public key (1184 bytes) published in the
    /// recipient's v2 address. The sender encapsulates against this to build
    /// the hybrid one-time key and attach the ciphertext.
    pub kem_public_key: String,
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

/// A wallet's post-quantum public keys, derived from its BIP39 seed,
/// hex-encoded for the JS boundary.
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
///      seed via the node-identical
///      [`bth_crypto_pq::derive_pq_keys_from_seed`];
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
    encode_address_from_hex(
        view_hex,
        spend_hex,
        &pq.kem_public_key,
        &pq.dsa_public_key,
        testnet,
    )
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
    /// Hex-encoded 64-byte BIP39 seed of the wallet. **Stays client-side.**
    /// Used to derive the wallet's ML-KEM-768 secret (node-identical
    /// `derive_pq_keys_from_seed`) so a HYBRID owned output's one-time private
    /// key can be recovered at SPEND time — the same unified recovery the
    /// key-image path uses (#988). Empty means classical-only recovery, which
    /// cannot spend 6.0.0 hybrid outputs.
    #[serde(default)]
    pub seed: String,
    /// Owned outputs being spent (one ring per input).
    pub inputs: Vec<SpendInput>,
    /// Recipient of the transfer.
    pub recipient: RecipientAddress,
    /// Hex-encoded raw ML-KEM-768 public key (1184 bytes) of the SENDER's own
    /// v2 address. The change output is a self-send whose ciphertext is
    /// encapsulated against this key, so the sender can later scan and recover
    /// its change under the hybrid scheme (issue #978). Derived from the
    /// wallet's BIP39 seed (`derivePqPublicKeysFromSeed`), it is the same key
    /// published in the wallet's own `botho://2/` address.
    pub sender_kem_public_key: String,
    /// Amount to send to the recipient, in picocredits.
    pub amount: u64,
    /// Transaction fee in picocredits.
    pub fee: u64,
    /// Chain height to stamp the transaction with (replay protection).
    pub created_at_height: u64,
    /// Optional BRIDGE DEPOSIT memo (hex-encoded 64 bytes) to embed on the
    /// RECIPIENT output (index 0).
    ///
    /// This is a DEDICATED, TYPED channel for the bridge deposit hook (#1037,
    /// epic #1029) — deliberately distinct from any human free-text "note" a
    /// wallet UI might collect. A BTH→wBTH mint deposit must carry the order
    /// memo — a 64-byte value whose first 16 bytes are the mint-order UUID
    /// (`BridgeOrder::generate_memo`, returned by the public order API as a
    /// 128-char hex string) — so the bridge watcher can view-key-match the
    /// deposit to its order (`bridge/service/src/bth_scan.rs`
    /// `scan_deposit_output`). When present the signer encrypts it into the
    /// recipient output's `e_memo` via [`MemoPayload::destination_bytes`], the
    /// exact format the watcher decrypts + reads.
    ///
    /// Because this channel carries a binary order-id (NOT a UTF-8 note), a
    /// present value MUST be exactly 64 bytes of hex — anything else is a hard
    /// error (see [`parse_memo`]). Free-text UI notes must NOT be routed here;
    /// they belong to a separate cosmetic channel that never reaches the
    /// signer.
    ///
    /// When absent or empty the recipient output carries NO memo —
    /// byte-identical to an ordinary (non-bridge) send, preserving privacy.
    /// Defaults to `None` for back-compat with callers that never set it.
    #[serde(default)]
    pub bridge_deposit_memo: Option<String>,
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

/// Parse a hex-encoded raw ML-KEM-768 public key, validating its length up
/// front so the caller gets a clear error before the encapsulation step.
///
/// A missing (empty) or wrong-length key means the pasted address is not a v2
/// post-quantum address; on 6.0.0 that is a hard error, since a KEM-less output
/// would be rejected by consensus.
fn parse_kem_public_key(field: &str, s: &str) -> Result<Vec<u8>, String> {
    let bytes = hex::decode(s.trim()).map_err(|e| format!("{field}: invalid hex: {e}"))?;
    if bytes.len() != bth_crypto_pq::ML_KEM_768_PUBLIC_KEY_BYTES {
        return Err(format!(
            "{field}: expected a {}-byte ML-KEM-768 public key (v2 address), got {} bytes",
            bth_crypto_pq::ML_KEM_768_PUBLIC_KEY_BYTES,
            bytes.len()
        ));
    }
    Ok(bytes)
}

/// Parse an optional hex-encoded 64-byte BRIDGE DEPOSIT memo into a
/// [`MemoPayload`].
///
/// This is the strict validator for the dedicated bridge channel ONLY (never a
/// free-text UI note — those never reach here). Returns `Ok(None)` when the
/// memo is absent or an empty string (an ordinary send: no memo, byte-identical
/// to today). A present memo MUST hex-decode to exactly 64 bytes — the fixed
/// order-memo width the bridge watcher reads (`bth_scan.rs`) and the public
/// order API emits (`generate_memo`, 128 hex chars). Any other length is a hard
/// error rather than a silently truncated / zero-padded memo that would fail to
/// match the order UUID.
fn parse_memo(field: &str, memo: &Option<String>) -> Result<Option<MemoPayload>, String> {
    match memo {
        None => Ok(None),
        Some(s) if s.trim().is_empty() => Ok(None),
        Some(s) => {
            let bytes = hex::decode(s.trim()).map_err(|e| format!("{field}: invalid hex: {e}"))?;
            if bytes.len() != 64 {
                return Err(format!(
                    "{field}: expected a 64-byte memo (128 hex chars), got {} bytes",
                    bytes.len()
                ));
            }
            let mut arr = [0u8; 64];
            arr.copy_from_slice(&bytes);
            Ok(Some(MemoPayload::destination_bytes(&arr)))
        }
    }
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

    // Reconstruct the recipient's v2 (post-quantum) address: the classical
    // stealth keys plus the recipient's published ML-KEM-768 key. The DSA key is
    // not consulted on the send path (only the recipient verifies signatures),
    // so it is left empty.
    let recipient = PublicAddress::new_with_pq(
        &parse_public("recipient.spendPublicKey", &req.recipient.spend_public_key)?,
        &parse_public("recipient.viewPublicKey", &req.recipient.view_public_key)?,
        parse_kem_public_key("recipient.kemPublicKey", &req.recipient.kem_public_key)?,
        Vec::new(),
    );

    // Reconstruct the sender's OWN v2 address for change: the account's
    // default-subaddress stealth keys plus the sender's own published ML-KEM-768
    // key (derived from the wallet seed). The change output encapsulates against
    // this so the sender can later recover it under the hybrid scheme.
    let default_subaddress = account.default_subaddress();
    let sender_address = PublicAddress::new_with_pq(
        default_subaddress.spend_public_key(),
        default_subaddress.view_public_key(),
        parse_kem_public_key("senderKemPublicKey", &req.sender_kem_public_key)?,
        Vec::new(),
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

    // Build outputs: recipient (index 0) + (optional) change (index 1) back to
    // our own address. Sub-dust change is folded into the fee so we never create
    // an unspendable output and the balance equation still holds exactly.
    //
    // Every output is a HYBRID post-quantum stealth output (protocol 6.0.0,
    // issue #978): the signer encapsulates a shared secret against the
    // recipient's (resp. sender's) published ML-KEM-768 key, attaches the
    // 1,088-byte ciphertext, and folds the secret into the one-time key —
    // byte-identical to the node send path (`new_hybrid_to_address`, #966). The
    // `output_index` is bound into the one-time-key derivation, so it MUST match
    // the output's position in the tx (recipient=0, change=1). A recipient (or
    // sender) address that lacks a well-formed ML-KEM key is a hard error, never
    // a silent KEM-less output that 6.0.0 consensus would reject.
    // Optional bridge-deposit memo on the RECIPIENT output (index 0): the bridge
    // deposit hook (#1037), a dedicated channel separate from any free-text UI
    // note. Encrypted to the recipient's view key like any memo, so only the
    // deposit account (the bridge reserve) can read it. `None` for an ordinary
    // send => no memo, unchanged privacy.
    let recipient_memo = parse_memo("bridgeDepositMemo", &req.bridge_deposit_memo)?;
    let recipient_output = TxOutput::new_hybrid_to_address(
        req.amount,
        &recipient,
        0,
        recipient_memo,
        Default::default(),
    )
    .map_err(|e| format!("recipient address is not post-quantum (v2): {e}"))?;
    let mut outputs = vec![recipient_output];
    let actual_fee = if change >= DUST_THRESHOLD {
        let change_output =
            TxOutput::new_hybrid_to_address(change, &sender_address, 1, None, Default::default())
                .map_err(|e| format!("sender address is not post-quantum (v2): {e}"))?;
        outputs.push(change_output);
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

    // The wallet's ML-KEM secret (from the BIP39 seed), for recovering the
    // one-time private key of HYBRID owned inputs (#988). `None` (empty seed)
    // falls back to classical recovery.
    let kem_keypair = derive_kem_keypair(&req.seed)?;

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

        // Recover the one-time private key for this owned output, on the same
        // unified path the key-image derivation uses (#988): a hybrid
        // (ciphertext-bearing) output needs the ML-KEM secret plus the
        // `output_index` bound into its one-time key; a KEM-less output uses
        // classical recovery. Signing a hybrid input WITHOUT its ciphertext
        // would silently derive the wrong one-time key and produce an invalid
        // CLSAG signature.
        let kem_ciphertext = parse_kem_ciphertext("input.kem_ciphertext", &input.kem_ciphertext)?;
        let real_output = TxOutput {
            amount: input.amount,
            target_key: parse_hex_32("input.target_key", &input.target_key)?,
            public_key: parse_hex_32("input.public_key", &input.public_key)?,
            e_memo: None,
            cluster_tags: Default::default(),
            kem_ciphertext,
        };
        let onetime_private = match &kem_keypair {
            Some(kp) => real_output.recover_spend_key_for(
                &account,
                kp,
                input.subaddress_index,
                input.output_index,
            ),
            None => real_output.recover_spend_key(&account, input.subaddress_index),
        }
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
/// (`targetKey`, `publicKey`, the transparent `amount` recovered from
/// `amountCommitment`, the output's `outputIndex` within its transaction, and
/// its optional ML-KEM `kemCiphertext`).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChainOutput {
    /// Hex-encoded 32-byte one-time target key of the output.
    pub target_key: String,
    /// Hex-encoded 32-byte ephemeral public key of the output.
    pub public_key: String,
    /// Amount in picocredits (recovered from the transparent commitment).
    pub amount: u64,
    /// The output's position within its creating transaction (`outputIndex`
    /// from `chain_getOutputs`). Under protocol 6.0.0 this index is bound into
    /// the hybrid one-time key, so it MUST match the value the producer used
    /// (recipient=0, change=1, coinbase=0). Defaults to 0 so legacy callers
    /// that only scanned classical (index-independent) outputs still
    /// deserialize.
    #[serde(default)]
    pub output_index: u32,
    /// Hex-encoded ML-KEM-768 ciphertext (`kemCiphertext` from
    /// `chain_getOutputs`), or `None` for a classical/legacy KEM-less output.
    /// When present, the scan decapsulates it with the wallet's seed-derived
    /// ML-KEM secret to detect the hybrid one-time key (issue #970/#988).
    #[serde(default)]
    pub kem_ciphertext: Option<String>,
}

/// A scan request: the account's private keys plus candidate chain outputs.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanRequest {
    /// Hex-encoded 32-byte account spend private key. **Stays client-side.**
    pub spend_private_key: String,
    /// Hex-encoded 32-byte account view private key. **Stays client-side.**
    pub view_private_key: String,
    /// Hex-encoded 64-byte BIP39 seed of the wallet. **Stays client-side.**
    ///
    /// Used to derive the wallet's ML-KEM-768 secret keypair
    /// ([`bth_crypto_pq::derive_pq_keys_from_seed`], node-identical) so the
    /// scan can decapsulate each hybrid output's ciphertext and detect the
    /// 6.0.0 hybrid one-time key. This is the feature-independent
    /// derivation the send side already uses (`derivePqPublicKeysFromSeed`)
    /// — NOT `WalletKeys::public_address()`, which requires the `pq`
    /// feature and is unavailable in the mobile crate's classical
    /// `botho-wallet` build (#984). Empty means "classical-only scan" (no
    /// ML-KEM secret available): outputs are matched with the legacy
    /// [`TxOutput::belongs_to`] check, so hybrid outputs are not detected.
    /// Defaults to empty for back-compat.
    #[serde(default)]
    pub seed: String,
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
    /// The output's position within its creating transaction. Carried through
    /// from the scan so the hybrid one-time-key recovery
    /// ([`TxOutput::recover_spend_key_for`]) can rebind it when deriving the
    /// key image / spend key. Defaults to 0 for back-compat.
    #[serde(default)]
    pub output_index: u32,
    /// The owned output's ML-KEM-768 ciphertext (hex), or `None` for a
    /// classical/legacy output. Preserved from the scan so key-image
    /// derivation dispatches down the same hybrid-when-present path (#988).
    #[serde(default)]
    pub kem_ciphertext: Option<String>,
}

/// Derive the wallet's ML-KEM-768 secret keypair from its hex BIP39 seed, using
/// the **node-identical**, feature-independent
/// [`bth_crypto_pq::derive_pq_keys_from_seed`].
///
/// Returns `Ok(None)` when `seed_hex` is empty (a classical-only scan: no
/// ML-KEM secret available, so hybrid outputs cannot be decapsulated). This is
/// the same derivation the send side uses for the public half
/// (`derive_pq_public_keys_from_seed`), so the secret matches the ML-KEM key
/// published in the wallet's own v2 address. It does NOT route through
/// `WalletKeys::public_address()` (which needs the `pq` feature and is absent
/// from the mobile crate's classical build — the trap documented in #984).
fn derive_kem_keypair(seed_hex: &str) -> Result<Option<bth_crypto_pq::MlKem768KeyPair>, String> {
    if seed_hex.trim().is_empty() {
        return Ok(None);
    }
    let seed = parse_bip39_seed(seed_hex)?;
    Ok(Some(
        bth_crypto_pq::derive_pq_keys_from_seed(&seed).kem_keypair,
    ))
}

/// Parse an optional hex-encoded ML-KEM-768 ciphertext into raw bytes.
fn parse_kem_ciphertext(field: &str, ct: &Option<String>) -> Result<Option<Vec<u8>>, String> {
    match ct {
        None => Ok(None),
        Some(s) if s.trim().is_empty() => Ok(None),
        Some(s) => {
            let bytes = hex::decode(s.trim()).map_err(|e| format!("{field}: invalid hex: {e}"))?;
            Ok(Some(bytes))
        }
    }
}

/// Identify which of `outputs` belong to the account, using the
/// **node-identical** unified hybrid scan ([`TxOutput::belongs_to_account`],
/// issue #970).
///
/// This is the single RECEIVE-scan path shared by both wallet frontends (the
/// browser sync and the mobile `wallet_ops::sync`). Under protocol 6.0.0 every
/// producer emits **hybrid** outputs whose one-time key folds in an ML-KEM
/// shared secret, so a purely-classical `belongs_to` check (the pre-#988
/// behaviour) could not detect incoming payments or the wallet's own change.
///
/// For each candidate it reconstructs the on-chain [`TxOutput`] **with its real
/// `kem_ciphertext`** and dispatches via `belongs_to_account`:
/// * ciphertext present + seed supplied → decapsulate with the wallet's
///   seed-derived ML-KEM secret and check the hybrid one-time key at the
///   output's `output_index`;
/// * ciphertext absent → classical [`TxOutput::belongs_to`] (back-compat).
///
/// When no seed is supplied (`req.seed` empty) the scan stays classical-only
/// and hybrid outputs are skipped, exactly as before. The keys/seed never leave
/// the client.
pub fn scan_owned_outputs_inner(req: &ScanRequest) -> Result<Vec<OwnedOutput>, String> {
    let spend_private = parse_private("spendPrivateKey", &req.spend_private_key)?;
    let view_private = parse_private("viewPrivateKey", &req.view_private_key)?;
    let account = AccountKey::new(&spend_private, &view_private);
    let kem_keypair = derive_kem_keypair(&req.seed)?;

    let mut owned = Vec::new();
    for out in &req.outputs {
        let kem_ciphertext = parse_kem_ciphertext("output.kem_ciphertext", &out.kem_ciphertext)?;
        let tx_out = TxOutput {
            amount: out.amount,
            target_key: parse_hex_32("output.target_key", &out.target_key)?,
            public_key: parse_hex_32("output.public_key", &out.public_key)?,
            e_memo: None,
            cluster_tags: Default::default(),
            kem_ciphertext,
        };
        // Unified dispatch (mirrors the node's `Wallet::scan_output`): with the
        // ML-KEM secret available, `belongs_to_account` decapsulates a
        // ciphertext-bearing output and folds the secret into the classical DH
        // stealth check; a KEM-less output falls through to the classical path.
        // Without a seed we can only run the classical check.
        let detected = match &kem_keypair {
            Some(kp) => tx_out.belongs_to_account(&account, kp, out.output_index),
            None => tx_out.belongs_to(&account),
        };
        if let Some(subaddress_index) = detected {
            owned.push(OwnedOutput {
                target_key: out.target_key.clone(),
                public_key: out.public_key.clone(),
                amount: out.amount,
                subaddress_index,
                output_index: out.output_index,
                kem_ciphertext: out.kem_ciphertext.clone(),
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
    /// Hex-encoded 64-byte BIP39 seed. **Stays client-side.** Used to derive
    /// the wallet's ML-KEM-768 secret so a hybrid owned output's one-time
    /// spend key (hence its key image) can be recovered via
    /// [`TxOutput::recover_spend_key_for`]. Empty falls back to classical
    /// recovery; defaults to empty for back-compat (#988).
    #[serde(default)]
    pub seed: String,
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
    /// The output's position within its creating transaction, preserved from
    /// the scan so a subsequent spend recovers the hybrid one-time key on
    /// the unified path (issue #988). Defaults to 0 for back-compat.
    #[serde(default)]
    pub output_index: u32,
    /// The owned output's ML-KEM-768 ciphertext (hex), or `None` for a
    /// classical/legacy output, preserved from the scan (issue #988).
    #[serde(default)]
    pub kem_ciphertext: Option<String>,
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
    let kem_keypair = derive_kem_keypair(&req.seed)?;

    let mut result = Vec::with_capacity(req.outputs.len());
    for out in &req.outputs {
        let kem_ciphertext = parse_kem_ciphertext("output.kem_ciphertext", &out.kem_ciphertext)?;
        let tx_out = TxOutput {
            amount: out.amount,
            target_key: parse_hex_32("output.target_key", &out.target_key)?,
            public_key: parse_hex_32("output.public_key", &out.public_key)?,
            e_memo: None,
            cluster_tags: Default::default(),
            kem_ciphertext,
        };
        // Recover the one-time spend key on the same unified path as the scan:
        // a hybrid (ciphertext-bearing) output needs the ML-KEM secret + its
        // bound `output_index`; a KEM-less output uses classical recovery.
        let onetime_private = match &kem_keypair {
            Some(kp) => {
                tx_out.recover_spend_key_for(&account, kp, out.subaddress_index, out.output_index)
            }
            None => tx_out.recover_spend_key(&account, out.subaddress_index),
        }
        .ok_or("failed to recover one-time private key for owned output")?;
        let key_image = KeyImage::from(&onetime_private);
        result.push(OwnedOutputKeyImage {
            target_key: out.target_key.clone(),
            public_key: out.public_key.clone(),
            amount: out.amount,
            subaddress_index: out.subaddress_index,
            output_index: out.output_index,
            kem_ciphertext: out.kem_ciphertext.clone(),
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
    use bth_crypto_pq::PqKeyMaterial;
    use bth_transaction_clsag::TxOutput;
    use rand::{rngs::StdRng, SeedableRng};

    /// Derive a deterministic ML-KEM-768 / ML-DSA-65 keypair for tests from a
    /// single seed byte (the node-identical `derive_pq_keys_from_seed`).
    fn pq_keys(seed_byte: u8) -> PqKeyMaterial {
        let seed = [seed_byte; bth_crypto_pq::BIP39_SEED_SIZE];
        bth_crypto_pq::derive_pq_keys_from_seed(&seed)
    }

    /// The hex-encoded raw ML-KEM-768 public key for a test PQ keypair.
    fn kem_public_hex(pq: &PqKeyMaterial) -> String {
        hex::encode(pq.kem_keypair.public_key().as_bytes())
    }

    /// Create a recipient address request fragment from an account, publishing
    /// the ML-KEM-768 public key of `pq` (so the send path can encapsulate a
    /// ciphertext against a v2 address).
    fn recipient_of_with_pq(account: &AccountKey, pq: &PqKeyMaterial) -> RecipientAddress {
        let addr = account.default_subaddress();
        RecipientAddress {
            spend_public_key: hex::encode(addr.spend_public_key().to_bytes()),
            view_public_key: hex::encode(addr.view_public_key().to_bytes()),
            kem_public_key: kem_public_hex(pq),
        }
    }

    /// Convenience: a recipient with a throwaway (deterministic) ML-KEM key for
    /// tests that only care that outputs are well-formed, not receivability.
    fn recipient_of(account: &AccountKey) -> RecipientAddress {
        recipient_of_with_pq(account, &pq_keys(0xAB))
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
            seed: String::new(),
            spend_private_key: hex::encode(sender.spend_private_key().to_bytes()),
            view_private_key: hex::encode(sender.view_private_key().to_bytes()),
            inputs: vec![SpendInput {
                output_index: 0,
                kem_ciphertext: None,
                target_key: hex::encode(owned.target_key),
                public_key: hex::encode(owned.public_key),
                amount: owned_amount,
                subaddress_index: 0,
                decoys,
            }],
            recipient: recipient_of(&recipient_account),
            sender_kem_public_key: kem_public_hex(&pq_keys(0xCD)),
            amount: send_amount,
            fee,
            created_at_height: 1000,
            bridge_deposit_memo: None,
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
            seed: String::new(),
            outputs: vec![OwnedOutput {
                target_key: hex::encode(owned.target_key),
                public_key: hex::encode(owned.public_key),
                amount: owned_amount,
                subaddress_index: 0,
                output_index: 0,
                kem_ciphertext: None,
            }],
        };
        let computed = compute_owned_output_key_images_inner(&ki_req).unwrap();
        assert_eq!(computed.len(), 1);

        // Build + sign a tx spending the same output, then read the key image
        // the CLSAG signature actually used.
        let decoys = make_decoys(DEFAULT_RING_SIZE - 1, owned_amount, &mut rng);
        let recipient_account = AccountKey::random(&mut rng);
        let req = SignRequest {
            seed: String::new(),
            spend_private_key: hex::encode(sender.spend_private_key().to_bytes()),
            view_private_key: hex::encode(sender.view_private_key().to_bytes()),
            inputs: vec![SpendInput {
                output_index: 0,
                kem_ciphertext: None,
                target_key: hex::encode(owned.target_key),
                public_key: hex::encode(owned.public_key),
                amount: owned_amount,
                subaddress_index: 0,
                decoys,
            }],
            recipient: recipient_of(&recipient_account),
            sender_kem_public_key: kem_public_hex(&pq_keys(0xCD)),
            amount: 5_000_000_000,
            fee: MIN_TX_FEE,
            created_at_height: 1000,
            bridge_deposit_memo: None,
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
            seed: String::new(),
            outputs: vec![
                ChainOutput {
                    target_key: hex::encode(mine.target_key),
                    public_key: hex::encode(mine.public_key),
                    amount: 1_000_000_000,
                    output_index: 0,
                    kem_ciphertext: None,
                },
                ChainOutput {
                    target_key: hex::encode(theirs.target_key),
                    public_key: hex::encode(theirs.public_key),
                    amount: 2_000_000_000,
                    output_index: 0,
                    kem_ciphertext: None,
                },
            ],
        };
        let owned = scan_owned_outputs_inner(&scan).unwrap();
        assert_eq!(owned.len(), 1, "only my output should be owned");

        let ki_req = KeyImageRequest {
            spend_private_key: scan.spend_private_key.clone(),
            view_private_key: scan.view_private_key.clone(),
            seed: scan.seed.clone(),
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
            output_index: 0,
            kem_ciphertext: o.kem_ciphertext.as_ref().map(hex::encode),
        };

        let req = ScanRequest {
            spend_private_key: hex::encode(me.spend_private_key().to_bytes()),
            view_private_key: hex::encode(me.view_private_key().to_bytes()),
            seed: String::new(),
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
            output_index: 0,
            kem_ciphertext: o.kem_ciphertext.as_ref().map(hex::encode),
        };

        let req = ScanRequest {
            spend_private_key: hex::encode(recipient.spend_private_key().to_bytes()),
            view_private_key: hex::encode(recipient.view_private_key().to_bytes()),
            seed: String::new(),
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
    ///   * it decodes back through the shared codec to the same view/spend
    ///     keys;
    ///   * the embedded PQ keys equal `derive_pq_keys_from_seed(seed)`, i.e.
    ///     the node's own derivation.
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
        assert_eq!(
            hex::encode(decoded.spend_public_key().to_bytes()),
            spend_hex
        );

        // The embedded PQ keys are exactly the node's seed derivation.
        let mut seed = [0u8; bth_crypto_pq::BIP39_SEED_SIZE];
        hex::decode_to_slice(seed_hex, &mut seed).unwrap();
        let pq = bth_crypto_pq::derive_pq_keys_from_seed(&seed);
        assert_eq!(
            decoded.kem_public_key(),
            pq.kem_keypair.public_key().as_bytes()
        );
        assert_eq!(
            decoded.dsa_public_key(),
            pq.sig_keypair.public_key().as_bytes()
        );

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

    /// #978: every output a browser SEND builds (recipient + change) MUST carry
    /// a valid 1,088-byte ML-KEM-768 ciphertext, encapsulated against the
    /// respective published address. Without this, the output is rejected by
    /// 6.0.0 consensus enforcement (`validate_transfer_tx`, #974).
    #[test]
    fn browser_send_outputs_carry_ml_kem_ciphertexts() {
        let mut rng = StdRng::from_seed([41u8; 32]);
        let sender = AccountKey::random(&mut rng);

        // Ensure a change output exists: owned - amount - fee well above dust.
        let owned_amount = 10_000_000_000u64;
        let send_amount = 4_000_000_000u64;
        let req = make_request(&sender, owned_amount, send_amount, MIN_TX_FEE, &mut rng);

        let tx = build_and_sign_with_rng(&req, &mut rng).expect("build+sign should succeed");

        // Recipient (index 0) + change (index 1).
        assert_eq!(tx.outputs.len(), 2, "expected recipient + change outputs");
        for (i, out) in tx.outputs.iter().enumerate() {
            let ct = out
                .kem_ciphertext
                .as_ref()
                .unwrap_or_else(|| panic!("output {i} is missing its ML-KEM ciphertext"));
            assert_eq!(
                ct.len(),
                bth_crypto_pq::ML_KEM_768_CIPHERTEXT_BYTES,
                "output {i} ciphertext must be exactly 1088 bytes"
            );
        }
    }

    /// #978: the hybrid one-time key a browser SEND derives must match the node
    /// construction — proven end-to-end by having a NODE scanner
    /// (`belongs_to_hybrid`, classical DH ⊕ ML-KEM decapsulation) detect the
    /// browser-built outputs. The recipient detects its output at index 0; the
    /// sender detects its own change at index 1. If the browser derived the
    /// one-time key even slightly differently from the node, neither scan would
    /// match and the funds would be unspendable.
    #[test]
    fn browser_send_outputs_are_receivable_by_node_scanner() {
        let mut rng = StdRng::from_seed([43u8; 32]);

        // Sender: classical account + its own ML-KEM keypair (published in the
        // sender's v2 address, used to encapsulate change).
        let sender = AccountKey::random(&mut rng);
        let sender_pq = pq_keys(0x11);

        // Recipient: classical account + its own ML-KEM keypair (published in
        // the recipient's v2 address).
        let recipient_account = AccountKey::random(&mut rng);
        let recipient_pq = pq_keys(0x22);

        let owned_amount = 10_000_000_000u64;
        let send_amount = 4_000_000_000u64;
        let owned = TxOutput::new(owned_amount, &sender.default_subaddress());
        let decoys = make_decoys(DEFAULT_RING_SIZE - 1, owned_amount, &mut rng);

        let req = SignRequest {
            seed: String::new(),
            spend_private_key: hex::encode(sender.spend_private_key().to_bytes()),
            view_private_key: hex::encode(sender.view_private_key().to_bytes()),
            inputs: vec![SpendInput {
                output_index: 0,
                kem_ciphertext: None,
                target_key: hex::encode(owned.target_key),
                public_key: hex::encode(owned.public_key),
                amount: owned_amount,
                subaddress_index: 0,
                decoys,
            }],
            recipient: recipient_of_with_pq(&recipient_account, &recipient_pq),
            sender_kem_public_key: kem_public_hex(&sender_pq),
            amount: send_amount,
            fee: MIN_TX_FEE,
            created_at_height: 1000,
            bridge_deposit_memo: None,
        };

        let tx = build_and_sign_with_rng(&req, &mut rng).expect("build+sign should succeed");
        assert_eq!(tx.outputs.len(), 2);

        // The recipient's node-identical hybrid scan detects the recipient
        // output (index 0) at its default subaddress.
        let recipient_index =
            tx.outputs[0].belongs_to_hybrid(&recipient_account, &recipient_pq.kem_keypair, 0);
        assert_eq!(
            recipient_index,
            Some(0),
            "recipient must detect the browser-built output at subaddress 0"
        );

        // The sender's own hybrid scan detects the change output (index 1).
        let change_index = tx.outputs[1].belongs_to_hybrid(&sender, &sender_pq.kem_keypair, 1);
        assert_eq!(
            change_index,
            Some(0),
            "sender must detect its own change at default subaddress"
        );

        // Cross-check: the recipient must NOT detect the sender's change, and
        // vice versa (the ciphertexts are bound to distinct ML-KEM keys).
        assert_eq!(
            tx.outputs[1].belongs_to_hybrid(&recipient_account, &recipient_pq.kem_keypair, 1),
            None,
            "recipient must not detect the sender's change"
        );
    }

    /// #988 spend leg (found by the #815 snap spike): a HYBRID owned output —
    /// which is what EVERY received output is under 6.0.0, including solo
    /// coinbases — must be spendable through `build_and_sign`. The signer must
    /// recover the hybrid one-time key from the wallet seed + the output's
    /// ciphertext + its bound `output_index` (the same unified recovery the
    /// key-image path uses). Before this fix the sign path hardcoded classical
    /// recovery (`kem_ciphertext: None`), silently derived the WRONG one-time
    /// key for hybrid inputs, and failed its own CLSAG self-verification.
    #[test]
    fn hybrid_owned_output_is_spendable_with_seed_and_ciphertext() {
        let mut rng = StdRng::from_seed([61u8; 32]);

        // The wallet under test: classical account + PQ keys derived from a
        // known 64-byte seed (exactly how a real wallet derives them).
        let seed = [0x33u8; bth_crypto_pq::BIP39_SEED_SIZE];
        let seed_hex = hex::encode(seed);
        let wallet = AccountKey::random(&mut rng);
        let wallet_pq = bth_crypto_pq::derive_pq_keys_from_seed(&seed);

        // A funder pays the wallet: tx1's recipient output (index 0) is a
        // HYBRID output to the wallet's v2 address.
        let funder = AccountKey::random(&mut rng);
        let funder_pq = pq_keys(0x44);
        let funder_amount = 20_000_000_000u64;
        let funder_owned = TxOutput::new(funder_amount, &funder.default_subaddress());
        let received_amount = 10_000_000_000u64;
        let fund_req = SignRequest {
            seed: String::new(),
            spend_private_key: hex::encode(funder.spend_private_key().to_bytes()),
            view_private_key: hex::encode(funder.view_private_key().to_bytes()),
            inputs: vec![SpendInput {
                output_index: 0,
                kem_ciphertext: None,
                target_key: hex::encode(funder_owned.target_key),
                public_key: hex::encode(funder_owned.public_key),
                amount: funder_amount,
                subaddress_index: 0,
                decoys: make_decoys(DEFAULT_RING_SIZE - 1, funder_amount, &mut rng),
            }],
            recipient: recipient_of_with_pq(&wallet, &wallet_pq),
            sender_kem_public_key: kem_public_hex(&funder_pq),
            amount: received_amount,
            fee: MIN_TX_FEE,
            created_at_height: 1000,
            bridge_deposit_memo: None,
        };
        let tx1 = build_and_sign_with_rng(&fund_req, &mut rng).expect("funding tx should build");
        let received = &tx1.outputs[0];
        assert!(
            received.kem_ciphertext.is_some(),
            "6.0.0 recipient output must be hybrid"
        );
        assert_eq!(
            received.belongs_to_hybrid(&wallet, &wallet_pq.kem_keypair, 0),
            Some(0),
            "wallet must detect the hybrid output"
        );

        // Now SPEND that hybrid output. With the seed + ciphertext +
        // output_index supplied, the signer recovers the hybrid one-time key
        // and the produced tx passes its own node-identical verification.
        let spend_req = SignRequest {
            seed: seed_hex,
            spend_private_key: hex::encode(wallet.spend_private_key().to_bytes()),
            view_private_key: hex::encode(wallet.view_private_key().to_bytes()),
            inputs: vec![SpendInput {
                output_index: 0,
                kem_ciphertext: Some(hex::encode(
                    received.kem_ciphertext.as_ref().expect("hybrid ciphertext"),
                )),
                target_key: hex::encode(received.target_key),
                public_key: hex::encode(received.public_key),
                amount: received_amount,
                subaddress_index: 0,
                decoys: make_decoys(DEFAULT_RING_SIZE - 1, received_amount, &mut rng),
            }],
            recipient: recipient_of(&funder),
            sender_kem_public_key: kem_public_hex(&wallet_pq),
            amount: 4_000_000_000,
            fee: MIN_TX_FEE,
            created_at_height: 1001,
            bridge_deposit_memo: None,
        };
        let tx2 = build_and_sign_with_rng(&spend_req, &mut rng)
            .expect("hybrid owned output must be spendable with seed + ciphertext");
        tx2.verify_ring_signatures()
            .expect("spend of hybrid output must verify");

        // Regression guard: the SAME spend WITHOUT the seed (classical-only
        // recovery) must fail — never silently produce an invalid signature.
        let mut classical_req = spend_req.clone();
        classical_req.seed = String::new();
        assert!(
            build_and_sign_with_rng(&classical_req, &mut rng).is_err(),
            "classical recovery of a hybrid input must fail loudly"
        );
    }

    /// #978 acceptance #4: a recipient whose address publishes no ML-KEM key
    /// (empty `kem_public_key`, i.e. a retired v1 / classical-only address) is
    /// a HARD ERROR on 6.0.0 — the signer must never emit a KEM-less output
    /// that consensus would reject. Fail loudly instead.
    #[test]
    fn send_to_kemless_address_is_hard_error() {
        let mut rng = StdRng::from_seed([47u8; 32]);
        let sender = AccountKey::random(&mut rng);
        let recipient_account = AccountKey::random(&mut rng);

        let owned_amount = 10_000_000_000u64;
        let owned = TxOutput::new(owned_amount, &sender.default_subaddress());
        let decoys = make_decoys(DEFAULT_RING_SIZE - 1, owned_amount, &mut rng);

        let mut recipient = recipient_of(&recipient_account);
        recipient.kem_public_key = String::new(); // v1 / classical-only address

        let req = SignRequest {
            seed: String::new(),
            spend_private_key: hex::encode(sender.spend_private_key().to_bytes()),
            view_private_key: hex::encode(sender.view_private_key().to_bytes()),
            inputs: vec![SpendInput {
                output_index: 0,
                kem_ciphertext: None,
                target_key: hex::encode(owned.target_key),
                public_key: hex::encode(owned.public_key),
                amount: owned_amount,
                subaddress_index: 0,
                decoys,
            }],
            recipient,
            sender_kem_public_key: kem_public_hex(&pq_keys(0xCD)),
            amount: 4_000_000_000,
            fee: MIN_TX_FEE,
            created_at_height: 1000,
            bridge_deposit_memo: None,
        };

        let err = build_and_sign_with_rng(&req, &mut rng)
            .expect_err("a KEM-less recipient address must be rejected");
        assert!(
            err.contains("ML-KEM") || err.contains("post-quantum"),
            "error should explain the address is not post-quantum, got: {err}"
        );
    }

    /// The hex-encoded 64-byte BIP39 seed whose `derive_pq_keys_from_seed`
    /// ML-KEM keypair equals `pq_keys(seed_byte)` — so a wallet built from this
    /// seed can decapsulate outputs encapsulated against `pq_keys(seed_byte)`.
    fn seed_hex(seed_byte: u8) -> String {
        hex::encode([seed_byte; bth_crypto_pq::BIP39_SEED_SIZE])
    }

    /// Turn a signed transaction output into a `ChainOutput` exactly as the
    /// node RPC (`chain_getOutputs`) would present it: stealth keys,
    /// transparent amount, its position within the tx, and its hybrid
    /// ML-KEM ciphertext.
    fn chain_output_of(out: &TxOutput, output_index: u32) -> ChainOutput {
        ChainOutput {
            target_key: hex::encode(out.target_key),
            public_key: hex::encode(out.public_key),
            amount: out.amount,
            output_index,
            kem_ciphertext: out.kem_ciphertext.as_ref().map(hex::encode),
        }
    }

    /// #988: a hybrid output SENT (via the #978 send path) to the wallet's
    /// address MUST be DETECTED by the shared `scan_owned_outputs_inner` — the
    /// exact receive path both the browser sync and the mobile
    /// `wallet_ops::sync` consume — by decapsulating its ML-KEM ciphertext
    /// with the wallet's seed-derived secret, AND its one-time spend key
    /// must recover so the funds are spendable (`x·G == target_key`). This
    /// is the RECEIVE-side counterpart to #978: before this fix the scan
    /// built every candidate with `kem_ciphertext: None` and used the
    /// classical `belongs_to`, so hybrid incoming payments were invisible.
    #[test]
    fn hybrid_received_output_is_detected_and_spendable_by_scan() {
        let mut rng = StdRng::from_seed([53u8; 32]);

        // Sender pays a recipient whose ML-KEM keypair is derived from a known
        // seed (so the recipient can scan with that seed).
        let sender = AccountKey::random(&mut rng);
        let recipient_account = AccountKey::random(&mut rng);
        let recipient_pq = pq_keys(0x22);

        let owned_amount = 10_000_000_000u64;
        let send_amount = 4_000_000_000u64;
        let owned = TxOutput::new(owned_amount, &sender.default_subaddress());
        let decoys = make_decoys(DEFAULT_RING_SIZE - 1, owned_amount, &mut rng);

        let req = SignRequest {
            seed: String::new(),
            spend_private_key: hex::encode(sender.spend_private_key().to_bytes()),
            view_private_key: hex::encode(sender.view_private_key().to_bytes()),
            inputs: vec![SpendInput {
                output_index: 0,
                kem_ciphertext: None,
                target_key: hex::encode(owned.target_key),
                public_key: hex::encode(owned.public_key),
                amount: owned_amount,
                subaddress_index: 0,
                decoys,
            }],
            recipient: recipient_of_with_pq(&recipient_account, &recipient_pq),
            sender_kem_public_key: kem_public_hex(&pq_keys(0x11)),
            amount: send_amount,
            fee: MIN_TX_FEE,
            created_at_height: 1000,
            bridge_deposit_memo: None,
        };
        let tx = build_and_sign_with_rng(&req, &mut rng).expect("build+sign should succeed");
        assert_eq!(tx.outputs.len(), 2, "recipient (0) + change (1)");

        // The recipient scans the chain outputs through the SHARED path, passing
        // its seed so the scan derives its ML-KEM secret and decapsulates.
        let scan = ScanRequest {
            spend_private_key: hex::encode(recipient_account.spend_private_key().to_bytes()),
            view_private_key: hex::encode(recipient_account.view_private_key().to_bytes()),
            seed: seed_hex(0x22),
            outputs: vec![
                chain_output_of(&tx.outputs[0], 0),
                chain_output_of(&tx.outputs[1], 1),
            ],
        };
        let owned = scan_owned_outputs_inner(&scan).expect("scan should succeed");
        assert_eq!(
            owned.len(),
            1,
            "recipient must detect exactly its own hybrid output"
        );
        assert_eq!(owned[0].amount, send_amount);
        assert_eq!(owned[0].subaddress_index, 0);
        assert_eq!(owned[0].output_index, 0);
        assert!(
            owned[0].kem_ciphertext.is_some(),
            "detected hybrid output must carry its ciphertext for spend recovery"
        );

        // The recovered one-time spend key must satisfy x·G == target_key, i.e.
        // the recipient can actually SPEND what it received. Recovery runs on the
        // same unified path the key-image derivation uses.
        let recovered = tx.outputs[0]
            .recover_spend_key_for(&recipient_account, &recipient_pq.kem_keypair, 0, 0)
            .expect("hybrid spend key must recover");
        assert_eq!(
            RistrettoPublic::from(&recovered).to_bytes(),
            tx.outputs[0].target_key,
            "x·G must equal the output's one-time target key (spendable)"
        );

        // And the shared key-image path (seed-aware) succeeds end-to-end.
        let ki = compute_owned_output_key_images_inner(&KeyImageRequest {
            spend_private_key: scan.spend_private_key.clone(),
            view_private_key: scan.view_private_key.clone(),
            seed: scan.seed.clone(),
            outputs: owned,
        })
        .expect("key-image derivation must succeed for the detected hybrid output");
        assert_eq!(ki.len(), 1);
    }

    /// #988: the wallet's OWN change (a hybrid self-send, output index 1) must be
    /// detected by its own sync through the shared scan path. Without the fix,
    /// spending money made your change vanish from your balance until a full
    /// rescan by a hybrid-aware node.
    #[test]
    fn hybrid_self_send_change_is_detected_on_sync() {
        let mut rng = StdRng::from_seed([59u8; 32]);

        // The sender's change is encapsulated against its OWN ML-KEM key, which
        // is derived from `seed_hex(0x33)`.
        let sender = AccountKey::random(&mut rng);
        let sender_pq = pq_keys(0x33);
        let recipient_account = AccountKey::random(&mut rng);
        let recipient_pq = pq_keys(0x44);

        let owned_amount = 10_000_000_000u64;
        let send_amount = 4_000_000_000u64;
        let owned = TxOutput::new(owned_amount, &sender.default_subaddress());
        let decoys = make_decoys(DEFAULT_RING_SIZE - 1, owned_amount, &mut rng);

        let req = SignRequest {
            seed: String::new(),
            spend_private_key: hex::encode(sender.spend_private_key().to_bytes()),
            view_private_key: hex::encode(sender.view_private_key().to_bytes()),
            inputs: vec![SpendInput {
                output_index: 0,
                kem_ciphertext: None,
                target_key: hex::encode(owned.target_key),
                public_key: hex::encode(owned.public_key),
                amount: owned_amount,
                subaddress_index: 0,
                decoys,
            }],
            recipient: recipient_of_with_pq(&recipient_account, &recipient_pq),
            sender_kem_public_key: kem_public_hex(&sender_pq),
            amount: send_amount,
            fee: MIN_TX_FEE,
            created_at_height: 1000,
            bridge_deposit_memo: None,
        };
        let tx = build_and_sign_with_rng(&req, &mut rng).expect("build+sign should succeed");
        assert_eq!(tx.outputs.len(), 2);

        // The sender syncs and must find its own change (output index 1) at the
        // change subaddress (index 1).
        let owned = scan_owned_outputs_inner(&ScanRequest {
            spend_private_key: hex::encode(sender.spend_private_key().to_bytes()),
            view_private_key: hex::encode(sender.view_private_key().to_bytes()),
            seed: seed_hex(0x33),
            outputs: vec![
                chain_output_of(&tx.outputs[0], 0),
                chain_output_of(&tx.outputs[1], 1),
            ],
        })
        .expect("scan should succeed");

        assert_eq!(owned.len(), 1, "sender must detect its own change");
        let change = &owned[0];
        // The change output is at position 1 within the tx, but the browser send
        // path pays change back to the sender's DEFAULT subaddress (index 0) —
        // the same key its own address publishes (mirrors #978's node-scanner
        // test, which detects the change at subaddress 0).
        assert_eq!(change.output_index, 1);
        assert_eq!(change.subaddress_index, 0);
        assert_eq!(change.amount, owned_amount - send_amount - MIN_TX_FEE);
    }

    /// #988 back-compat: a classical (`kemCiphertext: None`) output must still be
    /// detected on the same shared scan path, even when a seed IS supplied. The
    /// unified `belongs_to_account` dispatch falls through to the classical
    /// `belongs_to` when an output carries no ciphertext.
    #[test]
    fn classical_none_ciphertext_output_still_scans_with_seed() {
        let mut rng = StdRng::from_seed([61u8; 32]);
        let me = AccountKey::random(&mut rng);

        // A legacy classical output (no ciphertext) paid to my default address.
        let mine = TxOutput::new(7_000, &me.default_subaddress());
        assert!(mine.kem_ciphertext.is_none());

        let owned = scan_owned_outputs_inner(&ScanRequest {
            spend_private_key: hex::encode(me.spend_private_key().to_bytes()),
            view_private_key: hex::encode(me.view_private_key().to_bytes()),
            // Seed present, but the output is KEM-less → classical branch.
            seed: seed_hex(0x77),
            outputs: vec![chain_output_of(&mine, 0)],
        })
        .expect("scan should succeed");

        assert_eq!(owned.len(), 1, "classical output must still be detected");
        assert_eq!(owned[0].amount, 7_000);
        assert_eq!(owned[0].subaddress_index, 0);
        assert!(owned[0].kem_ciphertext.is_none());
    }

    /// #988 negative: a hybrid output addressed to a DIFFERENT wallet must NOT be
    /// detected by this wallet's scan — decapsulating the ciphertext with the
    /// wrong ML-KEM secret yields a useless secret and the stealth check fails.
    #[test]
    fn hybrid_output_to_other_wallet_is_not_detected() {
        let mut rng = StdRng::from_seed([67u8; 32]);

        let sender = AccountKey::random(&mut rng);
        let recipient_account = AccountKey::random(&mut rng);
        let recipient_pq = pq_keys(0x22);
        // A DIFFERENT wallet doing the scanning.
        let stranger = AccountKey::random(&mut rng);

        let owned_amount = 10_000_000_000u64;
        let owned = TxOutput::new(owned_amount, &sender.default_subaddress());
        let decoys = make_decoys(DEFAULT_RING_SIZE - 1, owned_amount, &mut rng);

        let req = SignRequest {
            seed: String::new(),
            spend_private_key: hex::encode(sender.spend_private_key().to_bytes()),
            view_private_key: hex::encode(sender.view_private_key().to_bytes()),
            inputs: vec![SpendInput {
                output_index: 0,
                kem_ciphertext: None,
                target_key: hex::encode(owned.target_key),
                public_key: hex::encode(owned.public_key),
                amount: owned_amount,
                subaddress_index: 0,
                decoys,
            }],
            recipient: recipient_of_with_pq(&recipient_account, &recipient_pq),
            sender_kem_public_key: kem_public_hex(&pq_keys(0x11)),
            amount: 4_000_000_000,
            fee: MIN_TX_FEE,
            created_at_height: 1000,
            bridge_deposit_memo: None,
        };
        let tx = build_and_sign_with_rng(&req, &mut rng).expect("build+sign should succeed");

        // The stranger scans the recipient's output with its OWN (wrong) seed.
        let owned = scan_owned_outputs_inner(&ScanRequest {
            spend_private_key: hex::encode(stranger.spend_private_key().to_bytes()),
            view_private_key: hex::encode(stranger.view_private_key().to_bytes()),
            seed: seed_hex(0x99),
            outputs: vec![chain_output_of(&tx.outputs[0], 0)],
        })
        .expect("scan should succeed");
        assert!(
            owned.is_empty(),
            "an output addressed to another wallet must not be detected"
        );
    }

    /// #1037: a send carrying a bridge order memo embeds it on the RECIPIENT
    /// output (index 0) in the exact format the bridge watcher reads. The
    /// deposit account (bridge reserve) recovers the 64-byte memo via
    /// `decrypt_memo` — the same call `bth_scan::scan_deposit_output` makes —
    /// and the first 16 bytes are the order UUID
    /// (`BridgeOrder::order_id_from_memo`). The change output carries NO
    /// memo. This is the wallet half of the deposit-matching round trip.
    #[test]
    fn bridge_order_memo_is_embedded_on_recipient_output() {
        let mut rng = StdRng::from_seed([203u8; 32]);
        let sender = AccountKey::random(&mut rng);
        // The recipient is the bridge deposit account (the reserve).
        let deposit_account = AccountKey::random(&mut rng);
        let deposit_pq = pq_keys(0x5a);

        // A 64-byte order memo: first 16 bytes the order UUID, rest zero
        // (mirrors `BridgeOrder::generate_memo`; the public API hands this back
        // as a 128-char hex string).
        let mut order_memo = [0u8; 64];
        order_memo[..16].copy_from_slice(&[
            0xde, 0xad, 0xbe, 0xef, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99,
            0xaa, 0xbb,
        ]);
        let memo_hex = hex::encode(order_memo);

        let owned_amount = 10_000_000_000u64;
        let owned = TxOutput::new(owned_amount, &sender.default_subaddress());
        let decoys = make_decoys(DEFAULT_RING_SIZE - 1, owned_amount, &mut rng);

        let req = SignRequest {
            seed: String::new(),
            spend_private_key: hex::encode(sender.spend_private_key().to_bytes()),
            view_private_key: hex::encode(sender.view_private_key().to_bytes()),
            inputs: vec![SpendInput {
                output_index: 0,
                kem_ciphertext: None,
                target_key: hex::encode(owned.target_key),
                public_key: hex::encode(owned.public_key),
                amount: owned_amount,
                subaddress_index: 0,
                decoys,
            }],
            recipient: recipient_of_with_pq(&deposit_account, &deposit_pq),
            sender_kem_public_key: kem_public_hex(&pq_keys(0xcd)),
            amount: 5_000_000_000,
            fee: MIN_TX_FEE,
            created_at_height: 1000,
            bridge_deposit_memo: Some(memo_hex),
        };
        let tx = build_and_sign_with_rng(&req, &mut rng).expect("build+sign should succeed");

        // The recipient (deposit) output is index 0 and carries the memo.
        let recipient_out = &tx.outputs[0];
        assert!(
            recipient_out.e_memo.is_some(),
            "the deposit output must carry the order memo"
        );
        let decrypted = recipient_out
            .decrypt_memo(&deposit_account)
            .expect("deposit account decrypts the memo with its view key");
        assert!(!decrypted.is_unused(), "the memo must not read as 'unused'");
        assert_eq!(
            decrypted.memo_data(),
            &order_memo,
            "decrypted memo must equal the 64 order bytes byte-for-byte"
        );
        // The first 16 bytes are the order UUID the watcher binds to.
        assert_eq!(&decrypted.memo_data()[..16], &order_memo[..16]);

        // The change output (index 1) must NOT carry a memo — no privacy leak.
        assert_eq!(tx.outputs.len(), 2, "recipient + change");
        assert!(
            tx.outputs[1].e_memo.is_none(),
            "the change output must not carry a memo"
        );

        // The deposit output must NOT be detectable by the sender/stranger and the
        // tx must still verify under the node's verifier.
        tx.verify_ring_signatures().unwrap();
        tx.is_valid_structure().unwrap();
    }

    /// #1037 no-regression: an ordinary send (no memo) produces outputs with NO
    /// encrypted memo — byte-identical to the pre-#1037 behaviour. Both an
    /// absent memo and an empty-string memo take the no-memo path.
    #[test]
    fn absent_or_empty_memo_produces_no_output_memo() {
        let mut rng = StdRng::from_seed([204u8; 32]);
        let sender = AccountKey::random(&mut rng);

        // Absent memo (None) via the shared helper.
        let req_none = make_request(&sender, 10_000_000_000, 5_000_000_000, MIN_TX_FEE, &mut rng);
        assert!(req_none.bridge_deposit_memo.is_none());
        let tx_none = build_and_sign_with_rng(&req_none, &mut rng).unwrap();
        for o in &tx_none.outputs {
            assert!(
                o.e_memo.is_none(),
                "no output may carry a memo for a plain send"
            );
        }

        // Empty-string memo is treated as no memo, not a zero-length memo.
        let mut req_empty =
            make_request(&sender, 10_000_000_000, 5_000_000_000, MIN_TX_FEE, &mut rng);
        req_empty.bridge_deposit_memo = Some(String::new());
        let tx_empty = build_and_sign_with_rng(&req_empty, &mut rng).unwrap();
        for o in &tx_empty.outputs {
            assert!(
                o.e_memo.is_none(),
                "an empty memo must not attach an e_memo"
            );
        }
    }

    /// #1037: a memo that does not hex-decode to exactly 64 bytes is a hard
    /// error rather than a silently truncated/padded memo that would fail to
    /// match the order UUID.
    #[test]
    fn malformed_memo_length_is_rejected() {
        let mut rng = StdRng::from_seed([205u8; 32]);
        let sender = AccountKey::random(&mut rng);
        let mut req = make_request(&sender, 10_000_000_000, 5_000_000_000, MIN_TX_FEE, &mut rng);
        // 16 bytes, not 64.
        req.bridge_deposit_memo = Some(hex::encode([7u8; 16]));
        let err = build_and_sign_with_rng(&req, &mut rng).unwrap_err();
        assert!(err.contains("expected a 64-byte memo"), "got: {err}");
    }
}
