// Copyright (c) 2024 Botho Foundation

//! Transaction types for value transfers with CryptoNote-style stealth addresses
//! and ring signatures for sender privacy.
//!
//! # Privacy Model
//!
//! Botho provides privacy by default with two tiers:
//!
//! ## Recipient Privacy (All Transactions)
//! - Each output has a unique one-time public key (unlinkable)
//! - Only the recipient can detect outputs sent to them (using view key)
//! - Only the recipient can spend outputs (using spend key)
//! - PQ-safe key derivation using ML-KEM-768
//!
//! ## Sender Privacy (All Transactions)
//! - All transactions use ring signatures with 20 decoys
//! - Key images prevent double-spending without revealing which output was spent
//!
//! ## Privacy Tiers
//!
//! - **Standard-Private (CLSAG)**: Classical ring signatures (~700 bytes/input)
//!   - Strong anonymity against today's adversaries
//!   - ~100x smaller than PQ-Private
//!
//! - **PQ-Private (LION)**: Post-quantum ring signatures (~63 KB/input)
//!   - Quantum-resistant sender privacy
//!   - Protects against "harvest now, decrypt later" attacks
//!   - Higher fees due to signature size
//!
//! # Stealth Address Protocol
//!
//! For a recipient with subaddress (C, D) where C is the view public key and
//! D is the spend public key:
//!
//! **Sender creates:**
//! - Random ephemeral key `r`
//! - Target key: `P = Hs(r * C) * G + D` (one-time spend public key)
//! - Public key: `R = r * D` (ephemeral DH public key)
//!
//! **Recipient scans:**
//! - Computes `D' = P - Hs(a * R) * G` where `a` is view private key
//! - If `D' == D` (their spend public key), they own the output
//!
//! **Recipient spends:**
//! - Recovers private key: `x = Hs(a * R) + d` where `d` is spend private key
//!
//! # Ring Signature Protocol (CLSAG)
//!
//! When spending an output:
//! 1. Select N-1 decoy outputs from the UTXO set (ring members)
//! 2. Include the real output at a random position in the ring
//! 3. Compute key image: `I = x * Hp(P)` where x is one-time private key
//! 4. Sign using CLSAG over all ring members
//!
//! Verification:
//! - Verify the CLSAG signature against all ring members
//! - Check key image hasn't been used before (prevents double-spend)
//! - Cannot determine which ring member is the real input

use aes::cipher::{KeyIvInit, StreamCipher};
use aes::Aes256;
use bth_account_keys::{AccountKey, PublicAddress};
use bth_transaction_types::ClusterTagVector;
use bth_crypto_keys::{CompressedRistrettoPublic, RistrettoPrivate, RistrettoPublic, RistrettoSignature};
use bth_crypto_ring_signature::{
    onetime_keys::{
        create_shared_secret, create_tx_out_public_key, create_tx_out_target_key,
        recover_onetime_private_key, recover_public_subaddress_spend_key,
    },
    generators, Clsag, CompressedCommitment, CurveScalar, KeyImage, ReducedTxOut, Scalar,
};
use bth_crypto_lion::{
    self as lion,
    LionKeyImage, LionKeyPair, LionPublicKey, LionSecretKey, LionRingSignature,
};
use bth_util_from_random::FromRandom;
use ctr::Ctr64BE;
use hkdf::Hkdf;
use rand_core::{CryptoRng, OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256, Sha512};
use std::collections::HashSet;

/// Minimum transaction fee in picocredits (0.0001 credits = 100_000_000 picocredits)
pub const MIN_TX_FEE: u64 = 100_000_000;

/// Picocredits per credit (10^12)
pub const PICOCREDITS_PER_CREDIT: u64 = 1_000_000_000_000;

/// Dust threshold - minimum output amount in picocredits.
/// Outputs below this value are rejected to prevent dust attacks.
/// Set to 1 microcredit (0.000001 credits = 1_000_000 picocredits).
pub const DUST_THRESHOLD: u64 = 1_000_000;

/// Size of an encrypted memo in bytes (2-byte type + 64-byte payload)
pub const ENCRYPTED_MEMO_SIZE: usize = 66;

// ============================================================================
// Encrypted Memo Types
// ============================================================================

/// An encrypted memo attached to a transaction output.
///
/// Memos are 66 bytes: 2-byte type identifier + 64-byte encrypted payload.
/// They are encrypted using AES-256-CTR with a key derived from the TxOut's
/// shared secret (HKDF-SHA512).
///
/// Only the recipient (who has the view private key) can decrypt the memo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncryptedMemo {
    /// The encrypted memo bytes (always 66 bytes)
    pub ciphertext: Vec<u8>,
}

impl EncryptedMemo {
    /// Create an encrypted memo from raw bytes
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != ENCRYPTED_MEMO_SIZE {
            return None;
        }
        Some(Self { ciphertext: bytes.to_vec() })
    }

    /// Get the raw bytes
    pub fn as_bytes(&self) -> &[u8] {
        &self.ciphertext
    }

    /// Decrypt this memo using the TxOut's shared secret.
    ///
    /// The shared_secret is computed as: tx_private_key * view_public_key
    /// which equals view_private_key * tx_public_key (by DH symmetry).
    pub fn decrypt(&self, shared_secret: &RistrettoPublic) -> Option<MemoPayload> {
        if self.ciphertext.len() != ENCRYPTED_MEMO_SIZE {
            return None;
        }
        let mut plaintext = [0u8; ENCRYPTED_MEMO_SIZE];
        plaintext.copy_from_slice(&self.ciphertext);
        apply_memo_keystream(&mut plaintext, shared_secret);
        Some(MemoPayload { data: plaintext })
    }
}

/// A plaintext memo payload (66 bytes: 2-byte type + 64-byte data).
///
/// Known memo types:
/// - `[0x00, 0x00]`: Unused/empty memo
/// - `[0x01, 0x00]`: Authenticated sender memo (includes sender address hash)
/// - `[0x02, 0x00]`: Destination memo (payment ID or reference)
/// - `[0x03, 0x00]`: Gift code memo
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoPayload {
    /// The plaintext memo data (66 bytes)
    pub data: [u8; ENCRYPTED_MEMO_SIZE],
}

impl MemoPayload {
    /// Create an empty/unused memo
    pub fn unused() -> Self {
        Self { data: [0u8; ENCRYPTED_MEMO_SIZE] }
    }

    /// Create a destination memo with a text message (up to 64 bytes).
    ///
    /// The message is stored in the 64-byte data portion.
    /// Longer messages are truncated, shorter ones are zero-padded.
    pub fn destination(message: &str) -> Self {
        let mut data = [0u8; ENCRYPTED_MEMO_SIZE];
        // Type: 0x0200 = Destination memo
        data[0] = 0x02;
        data[1] = 0x00;
        // Copy message bytes (up to 64 bytes)
        let msg_bytes = message.as_bytes();
        let copy_len = msg_bytes.len().min(64);
        data[2..2 + copy_len].copy_from_slice(&msg_bytes[..copy_len]);
        Self { data }
    }

    /// Get the memo type bytes (first 2 bytes)
    pub fn memo_type(&self) -> [u8; 2] {
        [self.data[0], self.data[1]]
    }

    /// Get the memo data (remaining 64 bytes)
    pub fn memo_data(&self) -> &[u8; 64] {
        self.data[2..66].try_into().expect("slice is exactly 64 bytes")
    }

    /// Check if this is an unused/empty memo
    pub fn is_unused(&self) -> bool {
        self.memo_type() == [0x00, 0x00]
    }

    /// Try to interpret the memo data as a UTF-8 string.
    /// Returns None if the data is not valid UTF-8 or is empty.
    pub fn as_text(&self) -> Option<&str> {
        let data = self.memo_data();
        // Find the first null byte or end of data
        let end = data.iter().position(|&b| b == 0).unwrap_or(64);
        if end == 0 {
            return None;
        }
        std::str::from_utf8(&data[..end]).ok()
    }

    /// Encrypt this memo using the TxOut's shared secret.
    pub fn encrypt(&self, shared_secret: &RistrettoPublic) -> EncryptedMemo {
        let mut ciphertext = self.data;
        apply_memo_keystream(&mut ciphertext, shared_secret);
        EncryptedMemo { ciphertext: ciphertext.to_vec() }
    }
}

/// Apply AES-256-CTR keystream to memo data for encryption/decryption.
///
/// Uses HKDF-SHA512 to derive the AES key and nonce from the shared secret.
fn apply_memo_keystream(data: &mut [u8; ENCRYPTED_MEMO_SIZE], shared_secret: &RistrettoPublic) {
    // Derive key material using HKDF
    let shared_secret_compressed = CompressedRistrettoPublic::from(shared_secret);
    let hkdf = Hkdf::<Sha512>::new(Some(b"mc-memo-okm"), shared_secret_compressed.as_ref());

    // Get 48 bytes: 32 for AES key + 16 for nonce
    let mut okm = [0u8; 48];
    hkdf.expand(b"", &mut okm).expect("48 bytes is valid for SHA512");

    let key: [u8; 32] = okm[0..32].try_into().unwrap();
    let nonce: [u8; 16] = okm[32..48].try_into().unwrap();

    // Apply AES-256-CTR keystream
    type Aes256Ctr = Ctr64BE<Aes256>;
    let mut cipher = Aes256Ctr::new((&key).into(), (&nonce).into());
    cipher.apply_keystream(data);
}

// ============================================================================
// Transaction Output
// ============================================================================

/// A transaction output (UTXO) with stealth addressing.
///
/// Uses CryptoNote-style one-time keys for recipient privacy:
/// - `target_key`: One-time public key that only the recipient can identify and spend
/// - `public_key`: Ephemeral DH public key for recipient to derive shared secret
/// - `e_memo`: Optional encrypted memo (66 bytes) readable only by recipient
/// - `cluster_tags`: Cluster ancestry for progressive fee tracking
///
/// The recipient's actual address is not stored in the output, making outputs
/// unlinkable even for the same recipient.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TxOutput {
    /// Amount in picocredits
    pub amount: u64,

    /// One-time target key: `Hs(r * C) * G + D`
    /// This is the stealth spend public key that only the recipient can identify.
    pub target_key: [u8; 32],

    /// Ephemeral public key: `r * D`
    /// Used by recipient to derive the shared secret for detecting ownership.
    pub public_key: [u8; 32],

    /// Optional encrypted memo (66 bytes).
    /// Contains payment notes, reference IDs, or sender information.
    /// Only the recipient can decrypt this using their view key.
    #[serde(default)]
    pub e_memo: Option<EncryptedMemo>,

    /// Cluster ancestry tags for progressive fee computation.
    /// Tracks what fraction of this output's value traces to each cluster origin.
    /// Used by the cluster-tax system to apply higher fees to concentrated wealth.
    #[serde(default)]
    pub cluster_tags: ClusterTagVector,
}

impl TxOutput {
    /// Create a new stealth output for a recipient (no memo).
    ///
    /// Generates a random ephemeral key and computes:
    /// - `target_key = Hs(r * C) * G + D` (one-time spend key)
    /// - `public_key = r * D` (ephemeral DH key)
    ///
    /// Only the recipient with view key `a` (where `C = a * D`) can detect
    /// this output belongs to them by checking if `P - Hs(a * R) * G == D`.
    pub fn new(amount: u64, recipient: &PublicAddress) -> Self {
        Self::new_with_memo(amount, recipient, None)
    }

    /// Create a new stealth output with an optional memo.
    ///
    /// The memo is encrypted using a shared secret derived from the ephemeral
    /// key, so only the recipient can read it.
    pub fn new_with_memo(amount: u64, recipient: &PublicAddress, memo: Option<MemoPayload>) -> Self {
        // Generate random ephemeral private key
        let tx_private_key = RistrettoPrivate::from_random(&mut OsRng);

        // Create stealth output keys
        let target_key = create_tx_out_target_key(&tx_private_key, recipient);
        let public_key = create_tx_out_public_key(&tx_private_key, recipient.spend_public_key());

        // Encrypt memo if provided
        let e_memo = memo.map(|m| {
            // Compute shared secret: tx_private_key * view_public_key
            // The recipient can compute the same secret as: view_private_key * public_key
            let shared_secret = create_shared_secret(recipient.view_public_key(), &tx_private_key);
            m.encrypt(&shared_secret)
        });

        Self {
            amount,
            target_key: target_key.to_bytes(),
            public_key: public_key.to_bytes(),
            e_memo,
            cluster_tags: ClusterTagVector::empty(),
        }
    }

    /// Create a new stealth output with cluster tags for progressive fees.
    ///
    /// This is the primary constructor for transactions that properly inherit
    /// cluster ancestry from their inputs.
    pub fn new_with_cluster_tags(
        amount: u64,
        recipient: &PublicAddress,
        memo: Option<MemoPayload>,
        cluster_tags: ClusterTagVector,
    ) -> Self {
        // Generate random ephemeral private key
        let tx_private_key = RistrettoPrivate::from_random(&mut OsRng);

        // Create stealth output keys
        let target_key = create_tx_out_target_key(&tx_private_key, recipient);
        let public_key = create_tx_out_public_key(&tx_private_key, recipient.spend_public_key());

        // Encrypt memo if provided
        let e_memo = memo.map(|m| {
            let shared_secret = create_shared_secret(recipient.view_public_key(), &tx_private_key);
            m.encrypt(&shared_secret)
        });

        Self {
            amount,
            target_key: target_key.to_bytes(),
            public_key: public_key.to_bytes(),
            e_memo,
            cluster_tags,
        }
    }

    /// Create a stealth output with a specific ephemeral key (for testing).
    pub fn new_with_key(
        amount: u64,
        recipient: &PublicAddress,
        tx_private_key: &RistrettoPrivate,
    ) -> Self {
        let target_key = create_tx_out_target_key(tx_private_key, recipient);
        let public_key = create_tx_out_public_key(tx_private_key, recipient.spend_public_key());

        Self {
            amount,
            target_key: target_key.to_bytes(),
            public_key: public_key.to_bytes(),
            e_memo: None,
            cluster_tags: ClusterTagVector::empty(),
        }
    }

    /// Check if this output has an encrypted memo.
    pub fn has_memo(&self) -> bool {
        self.e_memo.is_some()
    }

    /// Decrypt the memo using the recipient's view private key.
    ///
    /// Returns None if there's no memo or decryption fails.
    pub fn decrypt_memo(&self, account: &AccountKey) -> Option<MemoPayload> {
        let e_memo = self.e_memo.as_ref()?;

        // Parse the public key (ephemeral DH key from sender)
        let public_key = RistrettoPublic::try_from(&self.public_key[..]).ok()?;

        // Compute shared secret: view_private_key * public_key
        // This equals: tx_private_key * view_public_key (by DH symmetry)
        let shared_secret = create_shared_secret(&public_key, account.view_private_key());

        e_memo.decrypt(&shared_secret)
    }

    /// Compute a unique identifier for this output.
    pub fn id(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.amount.to_le_bytes());
        hasher.update(self.target_key);
        hasher.update(self.public_key);
        hasher.finalize().into()
    }

    /// Check if this stealth output belongs to an account.
    ///
    /// Uses the view private key to compute the expected spend public key
    /// and compares with the account's known subaddresses.
    ///
    /// Returns `Some(subaddress_index)` if the output belongs to this account,
    /// or `None` if it doesn't.
    pub fn belongs_to(&self, account: &AccountKey) -> Option<u64> {
        let view_private = account.view_private_key();
        let public_key = match RistrettoPublic::try_from(&self.public_key[..]) {
            Ok(pk) => pk,
            Err(_) => return None,
        };
        let target_key = match RistrettoPublic::try_from(&self.target_key[..]) {
            Ok(tk) => tk,
            Err(_) => return None,
        };

        // Recover what the spend public key would be if this output belongs to us
        let recovered_spend_key =
            recover_public_subaddress_spend_key(view_private, &target_key, &public_key);

        // Check against default subaddress (index 0)
        let default_subaddr = account.default_subaddress();
        let default_spend = default_subaddr.spend_public_key();
        if recovered_spend_key.to_bytes() == default_spend.to_bytes() {
            return Some(0);
        }

        // Check against change subaddress (index 1)
        let change_subaddr = account.change_subaddress();
        let change_spend = change_subaddr.spend_public_key();
        if recovered_spend_key.to_bytes() == change_spend.to_bytes() {
            return Some(1);
        }

        // Could extend to check more subaddresses if needed
        None
    }

    /// Recover the one-time private key needed to spend this output.
    ///
    /// This should only be called after verifying `belongs_to` returns Some.
    ///
    /// # Arguments
    /// * `account` - The account that owns this output
    /// * `subaddress_index` - The subaddress index (from `belongs_to`)
    pub fn recover_spend_key(
        &self,
        account: &AccountKey,
        subaddress_index: u64,
    ) -> Option<RistrettoPrivate> {
        let public_key = RistrettoPublic::try_from(&self.public_key[..]).ok()?;
        let view_private = account.view_private_key();
        let subaddress_spend_private = account.subaddress_spend_private(subaddress_index);

        Some(recover_onetime_private_key(
            &public_key,
            view_private,
            &subaddress_spend_private,
        ))
    }
}

/// A reference to a previous output being spent
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TxInput {
    /// Hash of the transaction containing the output
    pub tx_hash: [u8; 32],

    /// Index of the output in that transaction
    pub output_index: u32,

    /// Signature proving ownership (64 bytes for Schnorrkel)
    pub signature: Vec<u8>,
}

impl TxInput {
    /// Verify the signature for this input.
    ///
    /// # Arguments
    /// * `signing_hash` - The transaction's signing hash (from `Transaction::signing_hash()`)
    /// * `target_key` - The one-time public key from the UTXO being spent
    ///
    /// # Returns
    /// `true` if the signature is valid, `false` otherwise.
    pub fn verify_signature(&self, signing_hash: &[u8; 32], target_key: &[u8; 32]) -> bool {
        // Signature must be exactly 64 bytes
        if self.signature.len() != 64 {
            return false;
        }

        // Parse the target key as a RistrettoPublic
        let public_key = match RistrettoPublic::try_from(&target_key[..]) {
            Ok(pk) => pk,
            Err(_) => return false,
        };

        // Parse the signature
        let signature = match RistrettoSignature::try_from(self.signature.as_slice()) {
            Ok(sig) => sig,
            Err(_) => return false,
        };

        // Verify using Schnorrkel with the same domain separator used for signing
        public_key
            .verify_schnorrkel(b"botho-tx-v1", signing_hash, &signature)
            .is_ok()
    }
}

// ============================================================================
// Ring Signature Types (Version 2 Transactions)
// ============================================================================

/// Default ring size for CLSAG (Standard-Private) ring signatures.
/// Ring size 20 provides strong anonymity (larger than Monero's 16).
pub const DEFAULT_RING_SIZE: usize = 20;

/// Minimum ring size for CLSAG (Standard-Private) transactions.
/// Ring size 20 provides strong anonymity set with efficient ~700B signatures.
pub const MIN_RING_SIZE: usize = 20;

/// Ring size for LION (PQ-Private) ring signatures.
/// Ring size 11 balances privacy with manageable ~36KB signature sizes.
/// Still provides 3.30 bits of privacy (95% efficiency).
pub const PQ_RING_SIZE: usize = 11;

/// Minimum ring size for LION (PQ-Private) transactions.
/// Matches PQ_RING_SIZE for validation.
pub const MIN_PQ_RING_SIZE: usize = 11;

/// A member of a ring (either the real input or a decoy).
///
/// Ring members are indistinguishable from each other - verifiers cannot
/// determine which member is the real input being spent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RingMember {
    /// One-time target key from the output
    pub target_key: [u8; 32],

    /// Ephemeral public key from the output (for DH)
    pub public_key: [u8; 32],

    /// Amount commitment (for RingCT - trivial commitment if amounts are public)
    pub commitment: [u8; 32],
}

impl RingMember {
    /// Create a ring member from a TxOutput.
    ///
    /// Uses a trivial commitment (zero blinding) since amounts are public.
    pub fn from_output(output: &TxOutput) -> Self {
        // Create trivial Pedersen commitment: C = amount * H + 0 * G
        // This is transparent (anyone can verify amount) but compatible with MLSAG
        let generator = generators(0); // token_id = 0
        let commitment = generator.commit(Scalar::from(output.amount), Scalar::ZERO);

        Self {
            target_key: output.target_key,
            public_key: output.public_key,
            commitment: commitment.compress().to_bytes(),
        }
    }

    /// Convert to ReducedTxOut for MLSAG operations.
    pub fn to_reduced_tx_out(&self) -> Result<ReducedTxOut, &'static str> {
        let target_key = CompressedRistrettoPublic::try_from(&self.target_key[..])
            .map_err(|_| "invalid target_key")?;
        let public_key = CompressedRistrettoPublic::try_from(&self.public_key[..])
            .map_err(|_| "invalid public_key")?;
        let commitment = CompressedCommitment::try_from(&self.commitment[..])
            .map_err(|_| "invalid commitment")?;

        Ok(ReducedTxOut {
            target_key,
            public_key,
            commitment,
        })
    }
}

/// A CLSAG ring signature input (Standard-Private).
///
/// Uses CLSAG ring signatures to hide which output is actually being spent.
/// The key image prevents double-spending without revealing the real input.
/// CLSAG is more compact than MLSAG (~50% smaller) and is the standard tier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClsagRingInput {
    /// Ring of potential inputs (one real, rest are decoys).
    /// The real input's position is hidden by the ring signature.
    pub ring: Vec<RingMember>,

    /// Key image: `I = x * Hp(P)` where x is one-time private key.
    /// Unique per output, used to prevent double-spending.
    pub key_image: [u8; 32],

    /// Commitment key image (auxiliary): `D = z * Hp(P)` for balance proof.
    pub commitment_key_image: [u8; 32],

    /// CLSAG ring signature proving ownership of one ring member.
    /// Serialized as: c_zero (32) || responses (32 * ring_size) || key_image (32) || commitment_key_image (32)
    pub clsag_signature: Vec<u8>,
}

/// A LION ring signature input (PQ-Private).
///
/// Uses LION post-quantum ring signatures to hide which output is being spent.
/// Provides protection against quantum computers that could break classical
/// ring signatures. Significantly larger than CLSAG (~63KB vs ~700B).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LionRingInput {
    /// Ring of potential inputs (one real, rest are decoys).
    /// Uses LION public keys instead of Ristretto points.
    pub ring: Vec<LionRingMember>,

    /// LION key image for linkability and double-spend prevention.
    /// Much larger than classical key images (1312 bytes vs 32 bytes).
    pub key_image: Vec<u8>,

    /// LION ring signature proving ownership of one ring member.
    /// Approximately 63KB for ring size 20.
    pub lion_signature: Vec<u8>,
}

/// A ring member for LION PQ-private transactions.
///
/// Uses LION lattice-based public keys instead of Ristretto points.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LionRingMember {
    /// LION public key (1312 bytes serialized)
    pub lion_public_key: Vec<u8>,

    /// Classical target key (for output identification)
    pub target_key: [u8; 32],

    /// Ephemeral public key (for DH)
    pub public_key: [u8; 32],

    /// Amount commitment
    pub commitment: [u8; 32],
}


impl ClsagRingInput {
    /// Create a new CLSAG ring signature input.
    ///
    /// # Arguments
    /// * `ring` - Ring of outputs (real + decoys), with real at `real_index`
    /// * `real_index` - Position of the real input in the ring
    /// * `onetime_private_key` - Private key for the real input
    /// * `amount` - Amount of the real input
    /// * `output_amount` - Total output amount (for balance proof)
    /// * `message` - Message to sign (typically transaction signing hash)
    /// * `rng` - Random number generator
    pub fn new<R: RngCore + CryptoRng>(
        ring: Vec<RingMember>,
        real_index: usize,
        onetime_private_key: &RistrettoPrivate,
        amount: u64,
        _output_amount: u64, // Reserved for future RingCT balance proofs
        message: &[u8; 32],
        rng: &mut R,
    ) -> Result<Self, String> {
        if ring.len() < MIN_RING_SIZE {
            return Err(format!(
                "ring size {} is less than minimum {}",
                ring.len(),
                MIN_RING_SIZE
            ));
        }

        if real_index >= ring.len() {
            return Err("real_index out of bounds".to_string());
        }

        // Convert ring members to ReducedTxOut
        let reduced_ring: Result<Vec<ReducedTxOut>, _> =
            ring.iter().map(|m| m.to_reduced_tx_out()).collect();
        let reduced_ring = reduced_ring.map_err(|e| e.to_string())?;

        // Use trivial blinding (zero) since amounts are public
        let blinding = Scalar::ZERO;
        let output_blinding = Scalar::ZERO;

        // Sign with CLSAG
        let generator = generators(0);
        let clsag = Clsag::sign(
            message,
            &reduced_ring,
            real_index,
            onetime_private_key,
            amount,
            &blinding,
            &output_blinding,
            &generator,
            rng,
        )
        .map_err(|e| format!("CLSAG signing failed: {:?}", e))?;

        // Extract key images
        let key_image = *clsag.key_image.as_bytes();
        let commitment_key_image = *clsag.commitment_key_image.as_bytes();

        // Serialize the CLSAG signature (excluding key images, stored separately)
        let clsag_signature = Self::serialize_clsag(&clsag);

        Ok(Self {
            ring,
            key_image,
            commitment_key_image,
            clsag_signature,
        })
    }

    /// Verify this CLSAG ring signature input.
    ///
    /// # Arguments
    /// * `message` - The message that was signed (transaction signing hash)
    /// * `total_output_amount` - Total output amount for balance verification
    ///
    /// # Returns
    /// `true` if the signature is valid, `false` otherwise.
    pub fn verify(&self, message: &[u8; 32], total_output_amount: u64) -> bool {
        // Parse key images
        let key_image = match KeyImage::try_from(&self.key_image[..]) {
            Ok(ki) => ki,
            Err(_) => return false,
        };
        let commitment_key_image = match KeyImage::try_from(&self.commitment_key_image[..]) {
            Ok(ki) => ki,
            Err(_) => return false,
        };

        // Deserialize the CLSAG signature
        let clsag = match Self::deserialize_clsag(
            &self.clsag_signature,
            self.ring.len(),
            key_image,
            commitment_key_image,
        ) {
            Some(c) => c,
            None => return false,
        };

        // Convert ring to ReducedTxOut
        let reduced_ring: Result<Vec<ReducedTxOut>, _> =
            self.ring.iter().map(|m| m.to_reduced_tx_out()).collect();
        let reduced_ring = match reduced_ring {
            Ok(r) => r,
            Err(_) => return false,
        };

        // Create output commitment (trivial - zero blinding)
        let generator = generators(0);
        let output_commitment =
            generator.commit(Scalar::from(total_output_amount), Scalar::ZERO);

        // Verify the CLSAG
        clsag
            .verify(message, &reduced_ring, &output_commitment.compress().into())
            .is_ok()
    }

    /// Get the key image bytes.
    pub fn key_image(&self) -> &[u8; 32] {
        &self.key_image
    }

    /// Get the commitment key image bytes.
    pub fn commitment_key_image(&self) -> &[u8; 32] {
        &self.commitment_key_image
    }

    /// Serialize CLSAG to bytes (excluding key images, stored separately).
    ///
    /// Format: c_zero (32 bytes) || responses (32 bytes * ring_size)
    fn serialize_clsag(clsag: &Clsag) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(32 + 32 * clsag.responses.len());

        // c_zero (32 bytes)
        bytes.extend_from_slice(clsag.c_zero.as_ref());

        // responses (32 bytes each) - CLSAG has 1 response per member (vs 2 for MLSAG)
        for response in &clsag.responses {
            bytes.extend_from_slice(response.as_ref());
        }

        bytes
    }

    /// Deserialize CLSAG from bytes.
    fn deserialize_clsag(
        bytes: &[u8],
        ring_size: usize,
        key_image: KeyImage,
        commitment_key_image: KeyImage,
    ) -> Option<Clsag> {
        // Expected size: 32 (c_zero) + 32 * ring_size (responses)
        // CLSAG has 1 response per member (vs 2 for MLSAG)
        let expected_size = 32 + 32 * ring_size;
        if bytes.len() != expected_size {
            return None;
        }

        // Parse c_zero
        let c_zero_bytes: [u8; 32] = bytes[0..32].try_into().ok()?;
        let c_zero = CurveScalar::try_from(&c_zero_bytes[..]).ok()?;

        // Parse responses
        let mut responses = Vec::with_capacity(ring_size);
        for i in 0..ring_size {
            let start = 32 + i * 32;
            let end = start + 32;
            let resp_bytes: [u8; 32] = bytes[start..end].try_into().ok()?;
            let resp = CurveScalar::try_from(&resp_bytes[..]).ok()?;
            responses.push(resp);
        }

        Some(Clsag {
            c_zero,
            responses,
            key_image,
            commitment_key_image,
        })
    }
}

// ============================================================================
// LionRingInput Implementation
// ============================================================================

impl LionRingInput {
    /// Create a new LION ring signature input.
    ///
    /// # Arguments
    /// * `ring` - Ring of outputs (real + decoys), with real at `real_index`
    /// * `real_index` - Position of the real input in the ring
    /// * `lion_secret_key` - LION secret key for the real input
    /// * `message` - Message to sign (typically transaction signing hash)
    /// * `rng` - Random number generator
    pub fn new<R: RngCore + CryptoRng>(
        ring: Vec<LionRingMember>,
        real_index: usize,
        lion_secret_key: &LionSecretKey,
        message: &[u8; 32],
        rng: &mut R,
    ) -> Result<Self, String> {
        if ring.len() < MIN_RING_SIZE {
            return Err(format!(
                "ring size {} is less than minimum {}",
                ring.len(),
                MIN_RING_SIZE
            ));
        }

        if real_index >= ring.len() {
            return Err("real_index out of bounds".to_string());
        }

        // Collect LION public keys from ring members
        let lion_public_keys: Result<Vec<LionPublicKey>, _> = ring
            .iter()
            .map(|m| {
                LionPublicKey::from_bytes(&m.lion_public_key)
                    .map_err(|e| format!("Invalid LION public key: {:?}", e))
            })
            .collect();
        let lion_public_keys = lion_public_keys?;

        // Sign with LION ring signature (uses module-level function)
        let lion_sig = lion::sign(
            message,
            &lion_public_keys,
            real_index,
            lion_secret_key,
            rng,
        )
        .map_err(|e| format!("LION signing failed: {:?}", e))?;

        // Key image is embedded in the signature
        let key_image = lion_sig.key_image.to_bytes().to_vec();

        Ok(Self {
            ring,
            key_image,
            lion_signature: lion_sig.to_bytes(),
        })
    }

    /// Verify this LION ring signature input.
    ///
    /// # Arguments
    /// * `message` - The message that was signed (transaction signing hash)
    ///
    /// # Returns
    /// `true` if the signature is valid, `false` otherwise.
    pub fn verify(&self, message: &[u8; 32]) -> bool {
        // Collect LION public keys from ring members
        let lion_public_keys: Result<Vec<LionPublicKey>, _> = self
            .ring
            .iter()
            .map(|m| LionPublicKey::from_bytes(&m.lion_public_key))
            .collect();
        let lion_public_keys = match lion_public_keys {
            Ok(keys) => keys,
            Err(_) => return false,
        };

        // Parse signature (includes key image)
        let lion_sig = match LionRingSignature::from_bytes(&self.lion_signature, self.ring.len()) {
            Ok(sig) => sig,
            Err(_) => return false,
        };

        // Verify the LION ring signature (uses module-level function)
        lion::verify(message, &lion_public_keys, &lion_sig).is_ok()
    }

    /// Get the key image bytes.
    pub fn key_image(&self) -> &[u8] {
        &self.key_image
    }
}

// ============================================================================
// Transaction Inputs (enum-based for type safety)
// ============================================================================

/// How transaction inputs are specified.
///
/// All transactions in Botho are private by default. Users choose between:
/// - **Clsag**: Standard-private with classical ring signatures (~700B/input)
/// - **Lion**: PQ-private with post-quantum ring signatures (~63KB/input)
///
/// Both tiers provide ring size 20 (larger than Monero's 16) for strong anonymity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TxInputs {
    /// CLSAG ring signatures - standard-private tier.
    /// Classical cryptography with compact signatures.
    /// Recommended for most transactions.
    Clsag(Vec<ClsagRingInput>),

    /// LION ring signatures - PQ-private tier.
    /// Post-quantum cryptography with larger signatures.
    /// Recommended for high-value or long-term security needs.
    Lion(Vec<LionRingInput>),
}

/// Privacy tier for transactions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivacyTier {
    /// Standard-private with CLSAG (classical)
    StandardPrivate,
    /// PQ-private with LION (post-quantum)
    PqPrivate,
}

impl TxInputs {
    /// Check if this is a CLSAG (standard-private) transaction
    pub fn is_clsag(&self) -> bool {
        matches!(self, TxInputs::Clsag(_))
    }

    /// Check if this is a LION (pq-private) transaction
    pub fn is_lion(&self) -> bool {
        matches!(self, TxInputs::Lion(_))
    }

    /// Get the privacy tier
    pub fn privacy_tier(&self) -> PrivacyTier {
        match self {
            TxInputs::Clsag(_) => PrivacyTier::StandardPrivate,
            TxInputs::Lion(_) => PrivacyTier::PqPrivate,
        }
    }

    /// Get CLSAG inputs (if this is a Clsag variant)
    pub fn clsag(&self) -> Option<&[ClsagRingInput]> {
        match self {
            TxInputs::Clsag(inputs) => Some(inputs),
            _ => None,
        }
    }

    /// Get LION inputs (if this is a Lion variant)
    pub fn lion(&self) -> Option<&[LionRingInput]> {
        match self {
            TxInputs::Lion(inputs) => Some(inputs),
            _ => None,
        }
    }

    /// Get the number of inputs
    pub fn len(&self) -> usize {
        match self {
            TxInputs::Clsag(inputs) => inputs.len(),
            TxInputs::Lion(inputs) => inputs.len(),
        }
    }

    /// Check if there are no inputs
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get all key images (32-byte classical key images).
    /// For LION, this extracts a 32-byte hash of the full key image.
    pub fn key_images(&self) -> Vec<[u8; 32]> {
        match self {
            TxInputs::Clsag(inputs) => inputs.iter().map(|ri| ri.key_image).collect(),
            TxInputs::Lion(inputs) => {
                // Hash LION key images to 32 bytes for storage/indexing
                inputs.iter().map(|ri| {
                    let mut hasher = Sha256::new();
                    hasher.update(&ri.key_image);
                    hasher.finalize().into()
                }).collect()
            }
        }
    }

    /// Get full LION key images (1312 bytes each) - only for LION transactions.
    pub fn lion_key_images(&self) -> Option<Vec<&[u8]>> {
        match self {
            TxInputs::Lion(inputs) => Some(inputs.iter().map(|ri| ri.key_image.as_slice()).collect()),
            _ => None,
        }
    }
}

// ============================================================================
// Transaction
// ============================================================================

/// A transfer transaction (user-initiated, spending existing UTXOs).
///
/// This is the main transaction type for value transfers. Minting/coinbase
/// transactions are handled separately by `MintingTx` in block.rs.
///
/// # Privacy Model
///
/// All transactions are private by default:
/// - **Outputs**: Always use stealth addresses (recipient privacy)
/// - **Inputs**: Always use ring signatures (sender privacy)
///
/// Users choose between two privacy tiers:
/// - **Standard-Private (CLSAG)**: Classical ring signatures (~700B/input)
/// - **PQ-Private (LION)**: Post-quantum ring signatures (~63KB/input)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    /// Inputs being spent (CLSAG or LION ring signatures)
    pub inputs: TxInputs,

    /// Outputs being created (always stealth-addressed)
    pub outputs: Vec<TxOutput>,

    /// Transaction fee in picocredits
    pub fee: u64,

    /// Block height when this tx was created (for replay protection)
    pub created_at_height: u64,
}

impl Transaction {
    /// Create a new CLSAG transaction (standard-private).
    ///
    /// Uses classical CLSAG ring signatures with ~700 bytes per input.
    /// This is the recommended default for most transactions.
    pub fn new_clsag(
        inputs: Vec<ClsagRingInput>,
        outputs: Vec<TxOutput>,
        fee: u64,
        created_at_height: u64,
    ) -> Self {
        Self {
            inputs: TxInputs::Clsag(inputs),
            outputs,
            fee,
            created_at_height,
        }
    }

    /// Create a new LION transaction (pq-private).
    ///
    /// Uses post-quantum LION ring signatures with ~63KB per input.
    /// Recommended for high-value or long-term security needs.
    pub fn new_lion(
        inputs: Vec<LionRingInput>,
        outputs: Vec<TxOutput>,
        fee: u64,
        created_at_height: u64,
    ) -> Self {
        Self {
            inputs: TxInputs::Lion(inputs),
            outputs,
            fee,
            created_at_height,
        }
    }

    /// Get the privacy tier of this transaction.
    pub fn privacy_tier(&self) -> PrivacyTier {
        self.inputs.privacy_tier()
    }

    /// Check if this is a CLSAG (standard-private) transaction.
    pub fn is_clsag(&self) -> bool {
        self.inputs.is_clsag()
    }

    /// Check if this is a LION (pq-private) transaction.
    pub fn is_lion(&self) -> bool {
        self.inputs.is_lion()
    }

    /// Get all key images from ring inputs (for double-spend checking)
    pub fn key_images(&self) -> Vec<[u8; 32]> {
        self.inputs.key_images()
    }

    /// Estimate the size of this transaction in bytes.
    ///
    /// Uses typical sizes for each component:
    /// - CLSAG input: ~700 bytes (ring of 20 × 32-byte keys + signature)
    /// - LION input: ~63 KB (lattice-based ring signature)
    /// - Output: ~120 bytes (amount, target_key, public_key, optional memo)
    /// - Header: ~50 bytes (fee, created_at_height, etc.)
    pub fn estimate_size(&self) -> usize {
        const CLSAG_INPUT_SIZE: usize = 700;
        const LION_INPUT_SIZE: usize = 63_000;
        const OUTPUT_SIZE: usize = 120;
        const OUTPUT_MEMO_SIZE: usize = 66;
        const HEADER_SIZE: usize = 50;

        let input_size = match &self.inputs {
            TxInputs::Clsag(inputs) => inputs.len() * CLSAG_INPUT_SIZE,
            TxInputs::Lion(inputs) => inputs.len() * LION_INPUT_SIZE,
        };

        let output_size: usize = self.outputs.iter()
            .map(|o| OUTPUT_SIZE + if o.has_memo() { OUTPUT_MEMO_SIZE } else { 0 })
            .sum();

        HEADER_SIZE + input_size + output_size
    }

    /// Compute the transaction hash (includes signatures)
    pub fn hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();

        // Include input type tag and key images
        match &self.inputs {
            TxInputs::Clsag(inputs) => {
                hasher.update(b"clsag");
                for input in inputs {
                    hasher.update(input.key_image);
                }
            }
            TxInputs::Lion(inputs) => {
                hasher.update(b"lion");
                for input in inputs {
                    hasher.update(&input.key_image);
                }
            }
        }

        for output in &self.outputs {
            hasher.update(output.amount.to_le_bytes());
            hasher.update(output.target_key);
            hasher.update(output.public_key);
        }

        hasher.update(self.fee.to_le_bytes());
        hasher.update(self.created_at_height.to_le_bytes());
        hasher.finalize().into()
    }

    /// Compute the signing hash (excludes signatures for deterministic signing)
    ///
    /// This hash is used as the message for signing/verifying transaction inputs.
    /// It includes all transaction data except the signatures themselves.
    pub fn signing_hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();

        // Domain separator based on input type
        // For ring signatures, we do NOT include ring members or key images
        // in the signing hash. This is because:
        // 1. Ring members aren't known until decoys are selected
        // 2. Key images are computed inside the signing
        // 3. The hash must be consistent between signing and verification
        match &self.inputs {
            TxInputs::Clsag(_) => hasher.update(b"botho-tx-clsag"),
            TxInputs::Lion(_) => hasher.update(b"botho-tx-lion"),
        }

        // Include all outputs (stealth keys, not recipient identity)
        for output in &self.outputs {
            hasher.update(output.amount.to_le_bytes());
            hasher.update(output.target_key);
            hasher.update(output.public_key);
        }

        hasher.update(self.fee.to_le_bytes());
        hasher.update(self.created_at_height.to_le_bytes());
        hasher.finalize().into()
    }

    /// Get total output amount (excluding fee)
    pub fn total_output(&self) -> u64 {
        self.outputs.iter().map(|o| o.amount).sum()
    }

    /// Check basic transaction validity (structure only, not signatures or UTXO existence)
    pub fn is_valid_structure(&self) -> Result<(), &'static str> {
        // Check inputs based on type
        match &self.inputs {
            TxInputs::Clsag(inputs) => {
                if inputs.is_empty() {
                    return Err("CLSAG transaction has no inputs");
                }
                for input in inputs {
                    if input.ring.len() < MIN_RING_SIZE {
                        return Err("CLSAG input has insufficient ring size");
                    }
                }
            }
            TxInputs::Lion(inputs) => {
                if inputs.is_empty() {
                    return Err("LION transaction has no inputs");
                }
                for input in inputs {
                    if input.ring.len() < MIN_RING_SIZE {
                        return Err("LION input has insufficient ring size");
                    }
                }
            }
        }

        // Check for duplicate key images within this transaction
        // (double-spend attempt within a single tx)
        let key_images = self.key_images();
        let unique_count = key_images.iter().collect::<HashSet<_>>().len();
        if unique_count != key_images.len() {
            return Err("Transaction has duplicate key images");
        }

        if self.outputs.is_empty() {
            return Err("Transaction has no outputs");
        }
        if self.outputs.iter().any(|o| o.amount == 0) {
            return Err("Transaction has zero-amount output");
        }
        // Check for dust outputs (below minimum threshold)
        if self.outputs.iter().any(|o| o.amount < DUST_THRESHOLD) {
            return Err("Transaction has output below dust threshold");
        }
        if self.fee < MIN_TX_FEE {
            return Err("Transaction fee below minimum");
        }
        Ok(())
    }

    /// Verify all ring signatures in this transaction.
    ///
    /// Verifies CLSAG or LION signatures depending on transaction type.
    pub fn verify_ring_signatures(&self) -> Result<(), &'static str> {
        let signing_hash = self.signing_hash();
        let total_output = self.total_output() + self.fee;

        match &self.inputs {
            TxInputs::Clsag(inputs) => {
                for input in inputs {
                    if !input.verify(&signing_hash, total_output) {
                        return Err("Invalid CLSAG signature");
                    }
                }
                Ok(())
            }
            TxInputs::Lion(inputs) => {
                for input in inputs {
                    if !input.verify(&signing_hash) {
                        return Err("Invalid LION signature");
                    }
                }
                Ok(())
            }
        }
    }
}

/// Identifier for a UTXO (transaction hash + output index)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UtxoId {
    pub tx_hash: [u8; 32],
    pub output_index: u32,
}

impl UtxoId {
    pub fn new(tx_hash: [u8; 32], output_index: u32) -> Self {
        Self {
            tx_hash,
            output_index,
        }
    }

    /// Convert to bytes for storage
    pub fn to_bytes(&self) -> [u8; 36] {
        let mut bytes = [0u8; 36];
        bytes[0..32].copy_from_slice(&self.tx_hash);
        bytes[32..36].copy_from_slice(&self.output_index.to_le_bytes());
        bytes
    }

    /// Parse from bytes
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 36 {
            return None;
        }
        let tx_hash: [u8; 32] = bytes[0..32].try_into().ok()?;
        let output_index = u32::from_le_bytes(bytes[32..36].try_into().ok()?);
        Some(Self {
            tx_hash,
            output_index,
        })
    }
}

/// A stored UTXO with its output data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Utxo {
    pub id: UtxoId,
    pub output: TxOutput,
    /// Block height where this UTXO was created
    pub created_at: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create a test output with raw bytes (for structure tests)
    fn test_output(amount: u64, target: [u8; 32], public: [u8; 32]) -> TxOutput {
        TxOutput {
            amount,
            target_key: target,
            public_key: public,
            e_memo: None,
            cluster_tags: ClusterTagVector::empty(),
        }
    }

    /// Default test amount that's above the dust threshold
    const TEST_AMOUNT: u64 = DUST_THRESHOLD * 10;

    /// Helper to create a minimal test ring member
    fn test_ring_member(id: u8) -> RingMember {
        RingMember {
            target_key: [id; 32],
            public_key: [id.wrapping_add(1); 32],
            commitment: [id.wrapping_add(2); 32],
        }
    }

    /// Helper to create a test CLSAG input with MIN_RING_SIZE members
    fn test_clsag_input(ring_id: u8) -> ClsagRingInput {
        let ring: Vec<RingMember> = (0..MIN_RING_SIZE)
            .map(|i| test_ring_member(ring_id.wrapping_add(i as u8)))
            .collect();
        ClsagRingInput {
            ring,
            key_image: [ring_id; 32],
            commitment_key_image: [ring_id.wrapping_add(100); 32],
            clsag_signature: vec![0u8; 32 + 32 * MIN_RING_SIZE], // Fake signature
        }
    }

    #[test]
    fn test_utxo_id_roundtrip() {
        let id = UtxoId::new([1u8; 32], 42);
        let bytes = id.to_bytes();
        let recovered = UtxoId::from_bytes(&bytes).unwrap();
        assert_eq!(id, recovered);
    }

    #[test]
    fn test_transaction_hash_deterministic() {
        let tx = Transaction::new_clsag(
            vec![test_clsag_input(1)],
            vec![test_output(TEST_AMOUNT, [2u8; 32], [3u8; 32])],
            MIN_TX_FEE,
            1,
        );
        let hash1 = tx.hash();
        let hash2 = tx.hash();
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_signing_hash_changes_with_content() {
        let tx1 = Transaction::new_clsag(
            vec![test_clsag_input(1)],
            vec![test_output(TEST_AMOUNT, [2u8; 32], [3u8; 32])],
            MIN_TX_FEE,
            1,
        );

        let tx2 = Transaction::new_clsag(
            vec![test_clsag_input(1)],
            vec![test_output(TEST_AMOUNT * 2, [2u8; 32], [3u8; 32])], // Different amount
            MIN_TX_FEE,
            1,
        );

        // signing_hash should be different when content changes
        assert_ne!(tx1.signing_hash(), tx2.signing_hash());
    }

    #[test]
    fn test_transaction_is_valid_structure_no_inputs() {
        let tx = Transaction::new_clsag(
            vec![],
            vec![test_output(TEST_AMOUNT, [2u8; 32], [3u8; 32])],
            MIN_TX_FEE,
            1,
        );
        assert!(tx.is_valid_structure().is_err());
    }

    #[test]
    fn test_transaction_is_valid_structure_no_outputs() {
        let tx = Transaction::new_clsag(
            vec![test_clsag_input(1)],
            vec![],
            MIN_TX_FEE,
            1,
        );
        assert!(tx.is_valid_structure().is_err());
    }

    #[test]
    fn test_transaction_is_valid_structure_valid() {
        let tx = Transaction::new_clsag(
            vec![test_clsag_input(1)],
            vec![test_output(TEST_AMOUNT, [2u8; 32], [3u8; 32])],
            MIN_TX_FEE,
            1,
        );
        assert!(tx.is_valid_structure().is_ok());
    }

    #[test]
    fn test_transaction_dust_output_rejected() {
        // Output below dust threshold should be rejected
        let tx = Transaction::new_clsag(
            vec![test_clsag_input(1)],
            vec![test_output(DUST_THRESHOLD - 1, [2u8; 32], [3u8; 32])],
            MIN_TX_FEE,
            1,
        );
        assert_eq!(
            tx.is_valid_structure().unwrap_err(),
            "Transaction has output below dust threshold"
        );
    }

    #[test]
    fn test_transaction_at_dust_threshold_accepted() {
        // Output exactly at dust threshold should be accepted
        let tx = Transaction::new_clsag(
            vec![test_clsag_input(1)],
            vec![test_output(DUST_THRESHOLD, [2u8; 32], [3u8; 32])],
            MIN_TX_FEE,
            1,
        );
        assert!(tx.is_valid_structure().is_ok());
    }

    #[test]
    fn test_transaction_duplicate_key_images_rejected() {
        // Create two inputs with the same key image (simulated double-spend)
        let mut input1 = test_clsag_input(1);
        let mut input2 = test_clsag_input(2);
        // Force same key image on both inputs
        input2.key_image = input1.key_image;

        let tx = Transaction::new_clsag(
            vec![input1, input2],
            vec![test_output(TEST_AMOUNT, [2u8; 32], [3u8; 32])],
            MIN_TX_FEE,
            1,
        );
        assert_eq!(
            tx.is_valid_structure().unwrap_err(),
            "Transaction has duplicate key images"
        );
    }

    #[test]
    fn test_transaction_unique_key_images_accepted() {
        // Two inputs with different key images should be valid
        let input1 = test_clsag_input(1);
        let input2 = test_clsag_input(2); // Different ring_id = different key image

        let tx = Transaction::new_clsag(
            vec![input1, input2],
            vec![test_output(TEST_AMOUNT, [2u8; 32], [3u8; 32])],
            MIN_TX_FEE,
            1,
        );
        assert!(tx.is_valid_structure().is_ok());
    }

    #[test]
    fn test_privacy_tier_clsag() {
        let tx = Transaction::new_clsag(
            vec![test_clsag_input(1)],
            vec![test_output(TEST_AMOUNT, [2u8; 32], [3u8; 32])],
            MIN_TX_FEE,
            1,
        );
        assert_eq!(tx.privacy_tier(), PrivacyTier::StandardPrivate);
    }

    // Ring signature verification tests require actual crypto keys - see wallet tests
}
