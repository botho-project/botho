// Copyright (c) 2024 Botho Foundation

//! Transaction types for value transfers with CryptoNote-style stealth addresses
//! and ring signatures for sender privacy.
//!
//! # Privacy Model
//!
//! Botho provides two layers of privacy:
//!
//! ## Recipient Privacy (Stealth Addresses)
//! - Each output has a unique one-time public key (unlinkable)
//! - Only the recipient can detect outputs sent to them (using view key)
//! - Only the recipient can spend outputs (using spend key)
//!
//! ## Sender Privacy (Ring Signatures)
//! - Inputs are signed using MLSAG ring signatures
//! - The real input is hidden among decoy outputs (ring members)
//! - Key images prevent double-spending without revealing which output was spent
//!
//! # Transaction Versions
//!
//! - **Version 1**: Simple Schnorr signatures (no sender privacy)
//! - **Version 2**: Ring signatures with decoys (full sender privacy)
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
//! # Ring Signature Protocol (MLSAG)
//!
//! When spending an output:
//! 1. Select N-1 decoy outputs from the UTXO set (ring members)
//! 2. Include the real output at a random position in the ring
//! 3. Compute key image: `I = x * Hp(P)` where x is one-time private key
//! 4. Sign using MLSAG over all ring members
//!
//! Verification:
//! - Verify the MLSAG signature against all ring members
//! - Check key image hasn't been used before (prevents double-spend)
//! - Cannot determine which ring member is the real input

use bth_account_keys::{AccountKey, PublicAddress};
use bth_crypto_keys::{CompressedRistrettoPublic, RistrettoPrivate, RistrettoPublic, RistrettoSignature};
use bth_crypto_ring_signature::{
    onetime_keys::{
        create_tx_out_public_key, create_tx_out_target_key, recover_onetime_private_key,
        recover_public_subaddress_spend_key,
    },
    generators, CompressedCommitment, CurveScalar, KeyImage, ReducedTxOut, RingMLSAG, Scalar,
};
use bth_util_from_random::FromRandom;
use rand_core::{CryptoRng, OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Minimum transaction fee in picocredits (0.0001 credits = 100_000_000 picocredits)
pub const MIN_TX_FEE: u64 = 100_000_000;

/// Picocredits per credit (10^12)
pub const PICOCREDITS_PER_CREDIT: u64 = 1_000_000_000_000;

/// A transaction output (UTXO) with stealth addressing.
///
/// Uses CryptoNote-style one-time keys for recipient privacy:
/// - `target_key`: One-time public key that only the recipient can identify and spend
/// - `public_key`: Ephemeral DH public key for recipient to derive shared secret
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
}

impl TxOutput {
    /// Create a new stealth output for a recipient.
    ///
    /// Generates a random ephemeral key and computes:
    /// - `target_key = Hs(r * C) * G + D` (one-time spend key)
    /// - `public_key = r * D` (ephemeral DH key)
    ///
    /// Only the recipient with view key `a` (where `C = a * D`) can detect
    /// this output belongs to them by checking if `P - Hs(a * R) * G == D`.
    pub fn new(amount: u64, recipient: &PublicAddress) -> Self {
        // Generate random ephemeral private key
        let tx_private_key = RistrettoPrivate::from_random(&mut OsRng);

        // Create stealth output keys
        let target_key = create_tx_out_target_key(&tx_private_key, recipient);
        let public_key = create_tx_out_public_key(&tx_private_key, recipient.spend_public_key());

        Self {
            amount,
            target_key: target_key.to_bytes(),
            public_key: public_key.to_bytes(),
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
        }
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

/// Default ring size for transactions (real input + decoys)
pub const DEFAULT_RING_SIZE: usize = 11;

/// Minimum ring size (must have at least some privacy)
pub const MIN_RING_SIZE: usize = 3;

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

/// A ring signature transaction input (Version 2).
///
/// Uses MLSAG ring signatures to hide which output is actually being spent.
/// The key image prevents double-spending without revealing the real input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RingTxInput {
    /// Ring of potential inputs (one real, rest are decoys).
    /// The real input's position is hidden by the ring signature.
    pub ring: Vec<RingMember>,

    /// Key image: `I = x * Hp(P)` where x is one-time private key.
    /// Unique per output, used to prevent double-spending.
    /// If this key image was seen before, the input is a double-spend.
    pub key_image: [u8; 32],

    /// MLSAG ring signature proving ownership of one ring member.
    /// Serialized as: c_zero (32) || responses (32 * 2 * ring_size) || key_image (32)
    pub ring_signature: Vec<u8>,
}

impl RingTxInput {
    /// Create a new ring signature input.
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

        // Compute key image from private key
        let key_image = KeyImage::from(onetime_private_key);

        // Use trivial blinding (zero) since amounts are public
        let blinding = Scalar::ZERO;
        let output_blinding = Scalar::ZERO;

        // Sign with MLSAG
        let generator = generators(0);
        let mlsag = RingMLSAG::sign(
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
        .map_err(|e| format!("MLSAG signing failed: {:?}", e))?;

        // Serialize the MLSAG signature
        let ring_signature = Self::serialize_mlsag(&mlsag);

        Ok(Self {
            ring,
            key_image: *key_image.as_bytes(),
            ring_signature,
        })
    }

    /// Verify this ring signature input.
    ///
    /// # Arguments
    /// * `message` - The message that was signed (transaction signing hash)
    /// * `total_output_amount` - Total output amount for balance verification
    ///
    /// # Returns
    /// `true` if the signature is valid, `false` otherwise.
    pub fn verify(&self, message: &[u8; 32], total_output_amount: u64) -> bool {
        // Parse key image
        let key_image = match KeyImage::try_from(&self.key_image[..]) {
            Ok(ki) => ki,
            Err(_) => return false,
        };

        // Deserialize the MLSAG and set the key image
        let mlsag = match Self::deserialize_mlsag(&self.ring_signature, self.ring.len(), key_image)
        {
            Some(m) => m,
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

        // Verify the MLSAG
        mlsag
            .verify(message, &reduced_ring, &output_commitment.compress().into())
            .is_ok()
    }

    /// Get the key image bytes.
    pub fn key_image(&self) -> &[u8; 32] {
        &self.key_image
    }

    /// Serialize MLSAG to bytes.
    fn serialize_mlsag(mlsag: &RingMLSAG) -> Vec<u8> {
        let mut bytes = Vec::new();

        // c_zero (32 bytes)
        bytes.extend_from_slice(mlsag.c_zero.as_ref());

        // responses (32 bytes each)
        for response in &mlsag.responses {
            bytes.extend_from_slice(response.as_ref());
        }

        // key_image is stored separately, not in signature bytes

        bytes
    }

    /// Deserialize MLSAG from bytes.
    fn deserialize_mlsag(bytes: &[u8], ring_size: usize, key_image: KeyImage) -> Option<RingMLSAG> {
        // Expected size: 32 (c_zero) + 32 * 2 * ring_size (responses)
        let expected_size = 32 + 32 * 2 * ring_size;
        if bytes.len() != expected_size {
            return None;
        }

        // Parse c_zero
        let c_zero_bytes: [u8; 32] = bytes[0..32].try_into().ok()?;
        let c_zero = CurveScalar::try_from(&c_zero_bytes[..]).ok()?;

        // Parse responses
        let mut responses = Vec::with_capacity(2 * ring_size);
        for i in 0..(2 * ring_size) {
            let start = 32 + i * 32;
            let end = start + 32;
            let resp_bytes: [u8; 32] = bytes[start..end].try_into().ok()?;
            let resp = CurveScalar::try_from(&resp_bytes[..]).ok()?;
            responses.push(resp);
        }

        Some(RingMLSAG {
            c_zero,
            responses,
            key_image,
        })
    }
}

// ============================================================================
// Transaction Inputs (enum-based for type safety)
// ============================================================================

/// How transaction inputs are specified.
///
/// This enum allows for clean separation between simple (visible) and
/// ring signature (private) transactions without version numbers or
/// optional fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TxInputs {
    /// Simple Schnorr signatures - sender is visible on chain.
    /// Used for bootstrap period and testing. Will be deprecated
    /// once the UTXO set is large enough for good ring decoy selection.
    Simple(Vec<TxInput>),

    /// Ring signatures with decoys - sender is hidden.
    /// Each input includes a ring of potential inputs (one real, rest decoys)
    /// and an MLSAG signature proving ownership without revealing which.
    Ring(Vec<RingTxInput>),
}

impl TxInputs {
    /// Check if these are ring signature inputs
    pub fn is_ring(&self) -> bool {
        matches!(self, TxInputs::Ring(_))
    }

    /// Get simple inputs (if this is a Simple variant)
    pub fn simple(&self) -> Option<&[TxInput]> {
        match self {
            TxInputs::Simple(inputs) => Some(inputs),
            TxInputs::Ring(_) => None,
        }
    }

    /// Get ring inputs (if this is a Ring variant)
    pub fn ring(&self) -> Option<&[RingTxInput]> {
        match self {
            TxInputs::Simple(_) => None,
            TxInputs::Ring(inputs) => Some(inputs),
        }
    }

    /// Get the number of inputs
    pub fn len(&self) -> usize {
        match self {
            TxInputs::Simple(inputs) => inputs.len(),
            TxInputs::Ring(inputs) => inputs.len(),
        }
    }

    /// Check if there are no inputs
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get all key images (only for Ring variant)
    pub fn key_images(&self) -> Vec<[u8; 32]> {
        match self {
            TxInputs::Simple(_) => Vec::new(),
            TxInputs::Ring(inputs) => inputs.iter().map(|ri| ri.key_image).collect(),
        }
    }
}

// ============================================================================
// Transaction
// ============================================================================

/// A transfer transaction (user-initiated, spending existing UTXOs).
///
/// This is the main transaction type for value transfers. Mining/coinbase
/// transactions are handled separately by `MiningTx` in block.rs.
///
/// # Privacy Model
///
/// - **Outputs**: Always use stealth addresses (recipient privacy)
/// - **Inputs**: Either simple (visible sender) or ring (hidden sender)
///
/// Ring signatures are the standard for privacy. Simple signatures are
/// supported for the bootstrap period when the UTXO set is too small
/// for effective decoy selection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    /// Inputs being spent (simple or ring signature)
    pub inputs: TxInputs,

    /// Outputs being created (always stealth-addressed)
    pub outputs: Vec<TxOutput>,

    /// Transaction fee in picocredits
    pub fee: u64,

    /// Block height when this tx was created (for replay protection)
    pub created_at_height: u64,
}

impl Transaction {
    /// Create a new simple transaction (visible sender - for bootstrap/testing)
    pub fn new_simple(
        inputs: Vec<TxInput>,
        outputs: Vec<TxOutput>,
        fee: u64,
        created_at_height: u64,
    ) -> Self {
        Self {
            inputs: TxInputs::Simple(inputs),
            outputs,
            fee,
            created_at_height,
        }
    }

    /// Create a new private transaction (hidden sender - standard)
    pub fn new_private(
        ring_inputs: Vec<RingTxInput>,
        outputs: Vec<TxOutput>,
        fee: u64,
        created_at_height: u64,
    ) -> Self {
        Self {
            inputs: TxInputs::Ring(ring_inputs),
            outputs,
            fee,
            created_at_height,
        }
    }

    /// Backward compatibility: create simple transaction
    #[deprecated(note = "Use new_simple() or new_private() instead")]
    pub fn new(
        inputs: Vec<TxInput>,
        outputs: Vec<TxOutput>,
        fee: u64,
        created_at_height: u64,
    ) -> Self {
        Self::new_simple(inputs, outputs, fee, created_at_height)
    }

    /// Check if this is a ring signature (private) transaction
    pub fn is_private(&self) -> bool {
        self.inputs.is_ring()
    }

    /// Get all key images from ring inputs (for double-spend checking)
    pub fn key_images(&self) -> Vec<[u8; 32]> {
        self.inputs.key_images()
    }

    /// Compute the transaction hash (includes signatures)
    pub fn hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();

        // Include input type tag
        match &self.inputs {
            TxInputs::Simple(inputs) => {
                hasher.update(b"simple");
                for input in inputs {
                    hasher.update(input.tx_hash);
                    hasher.update(input.output_index.to_le_bytes());
                }
            }
            TxInputs::Ring(ring_inputs) => {
                hasher.update(b"ring");
                // Only include key images (signatures are large)
                for ring_input in ring_inputs {
                    hasher.update(ring_input.key_image);
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
        match &self.inputs {
            TxInputs::Simple(inputs) => {
                hasher.update(b"botho-tx-simple");
                // Include input references but NOT signatures
                for input in inputs {
                    hasher.update(input.tx_hash);
                    hasher.update(input.output_index.to_le_bytes());
                }
            }
            TxInputs::Ring(ring_inputs) => {
                hasher.update(b"botho-tx-ring");
                // Include ring members and key images (but NOT ring signatures)
                for ring_input in ring_inputs {
                    // Include all ring members (decoys + real)
                    for member in &ring_input.ring {
                        hasher.update(member.target_key);
                        hasher.update(member.public_key);
                        hasher.update(member.commitment);
                    }
                    // Include key image (deterministic from private key)
                    hasher.update(ring_input.key_image);
                }
            }
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
            TxInputs::Simple(inputs) => {
                if inputs.is_empty() {
                    return Err("Transaction has no inputs");
                }
            }
            TxInputs::Ring(ring_inputs) => {
                if ring_inputs.is_empty() {
                    return Err("Private transaction has no ring inputs");
                }
                // Validate ring sizes
                for ring_input in ring_inputs {
                    if ring_input.ring.len() < MIN_RING_SIZE {
                        return Err("Ring input has insufficient ring size");
                    }
                }
            }
        }

        if self.outputs.is_empty() {
            return Err("Transaction has no outputs");
        }
        if self.outputs.iter().any(|o| o.amount == 0) {
            return Err("Transaction has zero-amount output");
        }
        if self.fee < MIN_TX_FEE {
            return Err("Transaction fee below minimum");
        }
        Ok(())
    }

    /// Verify all ring signatures in a private transaction
    ///
    /// For each ring input, verifies the MLSAG signature against the ring members.
    pub fn verify_ring_signatures(&self) -> Result<(), &'static str> {
        let ring_inputs = match &self.inputs {
            TxInputs::Ring(inputs) => inputs,
            TxInputs::Simple(_) => return Err("Not a ring signature transaction"),
        };

        let signing_hash = self.signing_hash();
        let total_output = self.total_output() + self.fee;

        for ring_input in ring_inputs {
            // For now, we can't verify the exact input amount without knowing which
            // ring member is real. With trivial commitments, we use the sum of all
            // ring member commitments as a proxy (this is a simplification).
            //
            // In a full RingCT implementation, the commitment would hide the amount
            // and the signature would prove balance without revealing amounts.

            if !ring_input.verify(&signing_hash, total_output) {
                return Err("Invalid ring signature");
            }
        }

        // Input amount validation happens at a higher level where we have
        // access to the ledger to verify the ring members are valid UTXOs.
        Ok(())
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
        let tx = Transaction::new_simple(
            vec![TxInput {
                tx_hash: [1u8; 32],
                output_index: 0,
                signature: vec![0u8; 64],
            }],
            vec![test_output(1000, [2u8; 32], [3u8; 32])],
            100,
            1,
        );
        let hash1 = tx.hash();
        let hash2 = tx.hash();
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_signing_hash_excludes_signatures() {
        // Create two transactions with different signatures but same content
        let tx1 = Transaction::new_simple(
            vec![TxInput {
                tx_hash: [1u8; 32],
                output_index: 0,
                signature: vec![0u8; 64], // zeros
            }],
            vec![test_output(1000, [2u8; 32], [3u8; 32])],
            100,
            1,
        );

        let tx2 = Transaction::new_simple(
            vec![TxInput {
                tx_hash: [1u8; 32],
                output_index: 0,
                signature: vec![0xff; 64], // ones
            }],
            vec![test_output(1000, [2u8; 32], [3u8; 32])],
            100,
            1,
        );

        // signing_hash should be the same (excludes signatures)
        assert_eq!(tx1.signing_hash(), tx2.signing_hash());
    }

    #[test]
    fn test_signing_hash_changes_with_content() {
        let tx1 = Transaction::new_simple(
            vec![TxInput {
                tx_hash: [1u8; 32],
                output_index: 0,
                signature: vec![],
            }],
            vec![test_output(1000, [2u8; 32], [3u8; 32])],
            100,
            1,
        );

        let tx2 = Transaction::new_simple(
            vec![TxInput {
                tx_hash: [1u8; 32],
                output_index: 0,
                signature: vec![],
            }],
            vec![test_output(2000, [2u8; 32], [3u8; 32])], // Different amount
            100,
            1,
        );

        // signing_hash should be different when content changes
        assert_ne!(tx1.signing_hash(), tx2.signing_hash());
    }

    #[test]
    fn test_transaction_is_valid_structure_no_inputs() {
        let tx = Transaction::new_simple(
            vec![],
            vec![test_output(1000, [2u8; 32], [3u8; 32])],
            100,
            1,
        );
        assert!(tx.is_valid_structure().is_err());
    }

    #[test]
    fn test_transaction_is_valid_structure_no_outputs() {
        let tx = Transaction::new_simple(
            vec![TxInput {
                tx_hash: [1u8; 32],
                output_index: 0,
                signature: vec![],
            }],
            vec![],
            100,
            1,
        );
        assert!(tx.is_valid_structure().is_err());
    }

    #[test]
    fn test_transaction_is_valid_structure_valid() {
        let tx = Transaction::new_simple(
            vec![TxInput {
                tx_hash: [1u8; 32],
                output_index: 0,
                signature: vec![],
            }],
            vec![test_output(1000, [2u8; 32], [3u8; 32])],
            MIN_TX_FEE,
            1,
        );
        assert!(tx.is_valid_structure().is_ok());
    }

    // Stealth address tests require actual crypto keys - see wallet tests
}
