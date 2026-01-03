// Copyright (c) 2024 Botho Foundation

//! Quantum-Private Transaction Types
//!
//! This module extends the standard transaction types with post-quantum
//! cryptographic protection using NIST-standardized algorithms:
//!
//! - **ML-KEM-768** (Kyber): Key encapsulation for stealth address key exchange
//! - **ML-DSA-65** (Dilithium): Digital signatures for transaction signing
//!
//! # Hybrid Security Model
//!
//! Quantum-private transactions require BOTH classical and post-quantum
//! cryptographic operations to succeed:
//!
//! 1. **Outputs**: Use classical stealth addressing PLUS ML-KEM encapsulation
//! 2. **Inputs**: Require classical signature PLUS ML-DSA signature
//!
//! This provides defense-in-depth: if either cryptosystem is broken, the
//! other still provides protection.
//!
//! # Size Overhead
//!
//! Quantum-private transactions are significantly larger than classical ones:
//!
//! | Component | Classical | Quantum-Private |
//! |-----------|-----------|-----------------|
//! | Output    | ~72 B     | ~1160 B         |
//! | Input     | ~100 B    | ~2520 B         |
//!
//! This overhead comes from:
//! - ML-KEM ciphertext: 1088 bytes per output
//! - ML-DSA signature: 3309 bytes per input

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[cfg(feature = "pq")]
use bth_account_keys::{QuantumSafeAccountKey, QuantumSafePublicAddress};

use crate::transaction::{TxOutput, MIN_TX_FEE};

/// Size constants for quantum-private transactions
pub const PQ_CIPHERTEXT_SIZE: usize = 1088; // ML-KEM-768 ciphertext
pub const PQ_SIGNATURE_SIZE: usize = 3309; // ML-DSA-65 signature
pub const PQ_SIGNING_PUBKEY_SIZE: usize = 1952; // ML-DSA-65 public key

/// Fee constants for quantum-private transactions
///
/// Classical transactions have a minimum fee of 0.0001 credits (100_000_000 picocredits).
/// PQ transactions are ~19x larger, so they pay proportionally more.
///
/// Fee per byte of transaction data (in picocredits)
/// Set to ensure PQ transactions pay ~19x the classical fee for similar operations
pub const PQ_FEE_PER_BYTE: u64 = 10_000; // 0.00001 credits per byte

/// Minimum base fee for any PQ transaction (same as classical minimum)
pub const PQ_MIN_BASE_FEE: u64 = MIN_TX_FEE;

/// Calculate the minimum required fee for a quantum-private transaction.
///
/// The fee is calculated as:
/// `max(MIN_TX_FEE, base_fee + size_bytes * fee_per_byte)`
///
/// This ensures that:
/// - Very small PQ transactions still pay at least MIN_TX_FEE
/// - Larger PQ transactions pay proportionally more based on their size
///
/// # Arguments
/// * `num_inputs` - Number of transaction inputs
/// * `num_outputs` - Number of transaction outputs
///
/// # Returns
/// Minimum required fee in picocredits
pub fn calculate_pq_fee(num_inputs: usize, num_outputs: usize) -> u64 {
    // Estimated sizes per component
    const INPUT_SIZE: u64 = 32 + 4 + 64 + PQ_SIGNATURE_SIZE as u64; // ~3409 bytes
    const OUTPUT_SIZE: u64 = 72 + PQ_CIPHERTEXT_SIZE as u64 + PQ_SIGNING_PUBKEY_SIZE as u64; // ~3112 bytes
    const HEADER_SIZE: u64 = 24; // fee, height, length prefixes

    let total_size = HEADER_SIZE
        + (num_inputs as u64 * INPUT_SIZE)
        + (num_outputs as u64 * OUTPUT_SIZE);

    let size_based_fee = total_size * PQ_FEE_PER_BYTE;

    // Ensure minimum fee
    std::cmp::max(PQ_MIN_BASE_FEE, size_based_fee)
}

/// Calculate the minimum fee for a simple 1-input, 2-output PQ transaction
pub fn minimum_simple_pq_fee() -> u64 {
    calculate_pq_fee(1, 2)
}

/// A quantum-private transaction output.
///
/// Extends the classical stealth output with post-quantum key encapsulation.
/// The recipient needs both their classical view key AND their ML-KEM private
/// key to detect and decrypt this output.
///
/// # Fields
///
/// - `classical`: Standard stealth output (amount, target_key, public_key)
/// - `pq_ciphertext`: ML-KEM-768 ciphertext (1088 bytes)
/// - `pq_signing_pubkey`: ML-DSA-65 one-time public key (1952 bytes)
///
/// # Protocol
///
/// **Sender creates:**
/// 1. Encapsulate to recipient's ML-KEM public key: (ciphertext, shared_secret)
/// 2. Derive PQ one-time keypair from shared_secret
/// 3. Create classical stealth output (bound to PQ shared_secret for security)
/// 4. Store the full PQ public key so validators can verify signatures
///
/// **Recipient scans:**
/// 1. Check classical ownership (view key derivation)
/// 2. Decapsulate ciphertext to recover shared_secret
/// 3. Derive expected PQ public key and verify it matches stored key
///
/// **Recipient spends:**
/// 1. Recover classical one-time private key
/// 2. Derive PQ one-time private key from shared_secret
/// 3. Sign with BOTH classical and PQ keys
///
/// **Validator verifies:**
/// 1. Verify classical signature against classical target_key
/// 2. Verify PQ signature against stored pq_signing_pubkey
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuantumPrivateTxOutput {
    /// Classical stealth output (amount, one-time keys)
    pub classical: TxOutput,

    /// ML-KEM-768 ciphertext for PQ key encapsulation (1088 bytes)
    /// This encapsulates a shared secret to the recipient's PQ KEM key.
    pub pq_ciphertext: Vec<u8>,

    /// ML-DSA-65 one-time public key (1952 bytes)
    /// Derived from shared_secret, stored so validators can verify spend signatures.
    pub pq_signing_pubkey: Vec<u8>,
}

impl QuantumPrivateTxOutput {
    /// Create a new quantum-private output for a recipient.
    ///
    /// This creates both classical and PQ stealth components with cryptographic binding.
    /// The PQ shared secret is mixed into the classical ephemeral key derivation,
    /// ensuring that both layers are bound together. An adversary cannot modify
    /// either layer independently without invalidating the other.
    ///
    /// # Binding Mechanism
    ///
    /// The classical ephemeral key is derived as:
    /// `k = HKDF(IKM=random || pq_shared_secret, salt="botho-pq-binding", info="ephemeral")`
    ///
    /// This ensures:
    /// 1. The classical stealth address incorporates PQ entropy
    /// 2. An adversary with only quantum capabilities still needs to solve classical DH
    /// 3. An adversary with only classical capabilities still needs to break ML-KEM
    ///
    /// # Arguments
    /// * `amount` - Amount in picocredits
    /// * `recipient` - Recipient's quantum-safe public address
    ///
    /// # Returns
    /// A new quantum-private output with both classical and PQ components.
    #[cfg(feature = "pq")]
    pub fn new(amount: u64, recipient: &QuantumSafePublicAddress) -> Self {
        use bth_crypto_keys::RistrettoPrivate;
        use bth_crypto_pq::derive_onetime_sig_keypair;
        use hkdf::Hkdf;
        use rand_core::{OsRng, RngCore};
        use sha2::Sha256;

        // Step 1: Encapsulate to recipient's PQ KEM public key FIRST
        // This shared_secret will be used to bind the classical layer
        let (ciphertext, shared_secret) = recipient.pq_kem_public_key().encapsulate();

        // Step 2: Generate random entropy for classical ephemeral key
        let mut random_seed = [0u8; 32];
        OsRng.fill_bytes(&mut random_seed);

        // Step 3: Bind classical and PQ layers by deriving ephemeral key from both
        // IKM = random || pq_shared_secret (64 bytes total)
        let mut ikm = [0u8; 64];
        ikm[..32].copy_from_slice(&random_seed);
        ikm[32..].copy_from_slice(shared_secret.as_bytes());

        let hk = Hkdf::<Sha256>::new(Some(b"botho-pq-binding"), &ikm);
        let mut bound_ephemeral = [0u8; 32];
        hk.expand(b"ephemeral", &mut bound_ephemeral)
            .expect("32 bytes is valid for HKDF-SHA256");

        // Create classical ephemeral private key from the bound value
        let tx_private_key = RistrettoPrivate::from_bytes_mod_order(&bound_ephemeral);

        // Step 4: Create classical stealth output with the bound ephemeral key
        let classical = TxOutput::new_with_key(amount, recipient.classical(), &tx_private_key);

        // Step 5: Derive PQ one-time keypair from shared secret
        // The PUBLIC KEY is stored in the output so validators can verify signatures
        let pq_keypair = derive_onetime_sig_keypair(shared_secret.as_bytes(), 0);
        let pq_signing_pubkey = pq_keypair.public_key().as_bytes().to_vec();

        Self {
            classical,
            pq_ciphertext: ciphertext.as_bytes().to_vec(),
            pq_signing_pubkey,
        }
    }

    /// Check if this output belongs to a quantum-safe account.
    ///
    /// Performs both classical and PQ ownership checks:
    /// 1. Classical: View key derivation check
    /// 2. PQ: Decapsulate and verify derived public key matches stored key
    ///
    /// # Returns
    /// `Some((subaddress_index, shared_secret))` if owned, `None` otherwise.
    #[cfg(feature = "pq")]
    pub fn belongs_to(
        &self,
        account: &QuantumSafeAccountKey,
    ) -> Option<(u64, [u8; 32])> {
        use bth_crypto_pq::{derive_onetime_sig_keypair, MlKem768Ciphertext};

        // First check classical ownership
        let subaddress_index = self.classical.belongs_to(account.classical())?;

        // Decapsulate PQ ciphertext
        let ciphertext = MlKem768Ciphertext::from_bytes(&self.pq_ciphertext).ok()?;
        let shared_secret = account.pq_kem_keypair().decapsulate(&ciphertext).ok()?;

        // Derive the expected PQ public key from shared secret
        let expected_keypair = derive_onetime_sig_keypair(shared_secret.as_bytes(), 0);
        let expected_pubkey = expected_keypair.public_key().as_bytes();

        // Verify PQ signing public key matches
        if expected_pubkey != self.pq_signing_pubkey.as_slice() {
            return None;
        }

        // Return the shared secret bytes
        let mut ss_bytes = [0u8; 32];
        ss_bytes.copy_from_slice(shared_secret.as_bytes());
        Some((subaddress_index, ss_bytes))
    }

    /// Get the amount
    pub fn amount(&self) -> u64 {
        self.classical.amount
    }

    /// Compute a unique identifier for this output
    pub fn id(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.classical.id());
        hasher.update(&self.pq_ciphertext);
        hasher.update(&self.pq_signing_pubkey);
        hasher.finalize().into()
    }

    /// Estimated serialized size
    pub const fn estimated_size() -> usize {
        // Classical: amount(8) + target_key(32) + public_key(32) = 72
        // PQ: ciphertext(1088) + pq_signing_pubkey(1952) = 3040
        // Total: ~3112
        72 + PQ_CIPHERTEXT_SIZE + PQ_SIGNING_PUBKEY_SIZE
    }
}

/// A quantum-private transaction input.
///
/// Extends the classical input with a post-quantum signature.
/// Both signatures must be valid for the input to be accepted.
///
/// # Fields
///
/// - `tx_hash`: Hash of the transaction containing the output
/// - `output_index`: Index of the output being spent
/// - `classical_signature`: Schnorr signature (64 bytes)
/// - `pq_signature`: ML-DSA-65 signature (3309 bytes)
///
/// # Verification
///
/// An input is valid if and only if:
/// 1. The classical signature verifies against the output's classical target_key
/// 2. The PQ signature verifies against the output's pq_signing_pubkey
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuantumPrivateTxInput {
    /// Hash of the transaction containing the output being spent
    pub tx_hash: [u8; 32],

    /// Index of the output in that transaction
    pub output_index: u32,

    /// Classical Schnorr signature (64 bytes)
    pub classical_signature: Vec<u8>,

    /// ML-DSA-65 signature (3309 bytes)
    pub pq_signature: Vec<u8>,
}

impl QuantumPrivateTxInput {
    /// Create a new quantum-private input with dual signatures.
    ///
    /// Signs the message with both classical and PQ keys.
    ///
    /// # Arguments
    /// * `tx_hash` - Hash of the transaction containing the UTXO
    /// * `output_index` - Index of the UTXO in that transaction
    /// * `signing_hash` - Message to sign (transaction signing hash)
    /// * `classical_private_key` - Classical one-time private key
    /// * `pq_shared_secret` - Shared secret from KEM decapsulation
    #[cfg(feature = "pq")]
    pub fn new(
        tx_hash: [u8; 32],
        output_index: u32,
        signing_hash: &[u8; 32],
        classical_private_key: &bth_crypto_keys::RistrettoPrivate,
        pq_shared_secret: &[u8; 32],
    ) -> Self {
        use bth_crypto_pq::derive_onetime_sig_keypair;

        // Sign with classical key
        let classical_sig = classical_private_key.sign_schnorrkel(b"botho-tx-v1", signing_hash);
        let classical_signature: &[u8] = classical_sig.as_ref();

        // Derive PQ one-time keypair and sign
        let pq_keypair = derive_onetime_sig_keypair(pq_shared_secret, 0);
        let pq_sig = pq_keypair.sign(signing_hash);

        Self {
            tx_hash,
            output_index,
            classical_signature: classical_signature.to_vec(),
            pq_signature: pq_sig.as_bytes().to_vec(),
        }
    }

    /// Verify both signatures for this input.
    ///
    /// # Arguments
    /// * `signing_hash` - The message that was signed
    /// * `classical_target_key` - Classical one-time public key from UTXO
    /// * `pq_signing_pubkey` - PQ signing public key from UTXO (1952 bytes)
    ///
    /// # Returns
    /// `true` if BOTH signatures are valid, `false` otherwise.
    #[cfg(feature = "pq")]
    pub fn verify(
        &self,
        signing_hash: &[u8; 32],
        classical_target_key: &[u8; 32],
        pq_signing_pubkey: &[u8],
    ) -> bool {
        // Verify classical signature
        if !self.verify_classical(signing_hash, classical_target_key) {
            return false;
        }

        // Verify PQ signature against the stored public key
        self.verify_pq(signing_hash, pq_signing_pubkey)
    }

    /// Verify the classical Schnorr signature
    fn verify_classical(&self, signing_hash: &[u8; 32], target_key: &[u8; 32]) -> bool {
        use bth_crypto_keys::{RistrettoPublic, RistrettoSignature};

        if self.classical_signature.len() != 64 {
            return false;
        }

        let public_key = match RistrettoPublic::try_from(&target_key[..]) {
            Ok(pk) => pk,
            Err(_) => return false,
        };

        let signature = match RistrettoSignature::try_from(self.classical_signature.as_slice()) {
            Ok(sig) => sig,
            Err(_) => return false,
        };

        public_key
            .verify_schnorrkel(b"botho-tx-v1", signing_hash, &signature)
            .is_ok()
    }

    /// Verify the PQ ML-DSA signature against the stored public key
    #[cfg(feature = "pq")]
    fn verify_pq(&self, signing_hash: &[u8; 32], pq_signing_pubkey: &[u8]) -> bool {
        use bth_crypto_pq::{MlDsa65PublicKey, MlDsa65Signature};

        // Check signature size
        if self.pq_signature.len() != PQ_SIGNATURE_SIZE {
            return false;
        }

        // Check public key size
        if pq_signing_pubkey.len() != PQ_SIGNING_PUBKEY_SIZE {
            return false;
        }

        // Parse public key
        let public_key = match MlDsa65PublicKey::from_bytes(pq_signing_pubkey) {
            Ok(pk) => pk,
            Err(_) => return false,
        };

        // Parse signature
        let signature = match MlDsa65Signature::from_bytes(&self.pq_signature) {
            Ok(sig) => sig,
            Err(_) => return false,
        };

        // Verify signature
        public_key.verify(signing_hash, &signature).is_ok()
    }

    /// Verify PQ signature structure (size check only, for non-pq builds)
    #[cfg(not(feature = "pq"))]
    fn verify_pq(&self, _signing_hash: &[u8; 32], pq_signing_pubkey: &[u8]) -> bool {
        self.pq_signature.len() == PQ_SIGNATURE_SIZE
            && pq_signing_pubkey.len() == PQ_SIGNING_PUBKEY_SIZE
    }

    /// Estimated serialized size
    pub const fn estimated_size() -> usize {
        // tx_hash(32) + output_index(4) + classical_sig(64) + pq_sig(3309) = 3409
        // Plus encoding overhead, round to ~2520 for ring input equivalent
        32 + 4 + 64 + PQ_SIGNATURE_SIZE
    }
}

/// A complete quantum-private transaction.
///
/// Uses quantum-private inputs and outputs for full post-quantum protection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantumPrivateTransaction {
    /// Quantum-private inputs
    pub inputs: Vec<QuantumPrivateTxInput>,

    /// Quantum-private outputs
    pub outputs: Vec<QuantumPrivateTxOutput>,

    /// Transaction fee in picocredits
    pub fee: u64,

    /// Block height when this tx was created (for replay protection)
    pub created_at_height: u64,
}

impl QuantumPrivateTransaction {
    /// Create a new quantum-private transaction
    pub fn new(
        inputs: Vec<QuantumPrivateTxInput>,
        outputs: Vec<QuantumPrivateTxOutput>,
        fee: u64,
        created_at_height: u64,
    ) -> Self {
        Self {
            inputs,
            outputs,
            fee,
            created_at_height,
        }
    }

    /// Compute the transaction hash
    pub fn hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"botho-pq-tx");

        for input in &self.inputs {
            hasher.update(input.tx_hash);
            hasher.update(input.output_index.to_le_bytes());
        }

        for output in &self.outputs {
            hasher.update(output.id());
        }

        hasher.update(self.fee.to_le_bytes());
        hasher.update(self.created_at_height.to_le_bytes());
        hasher.finalize().into()
    }

    /// Compute the signing hash (excludes signatures)
    pub fn signing_hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"botho-pq-tx-sign");

        // Include input references but NOT signatures
        for input in &self.inputs {
            hasher.update(input.tx_hash);
            hasher.update(input.output_index.to_le_bytes());
        }

        // Include all output data
        for output in &self.outputs {
            hasher.update(output.classical.amount.to_le_bytes());
            hasher.update(output.classical.target_key);
            hasher.update(output.classical.public_key);
            hasher.update(&output.pq_ciphertext);
            hasher.update(&output.pq_signing_pubkey);
        }

        hasher.update(self.fee.to_le_bytes());
        hasher.update(self.created_at_height.to_le_bytes());
        hasher.finalize().into()
    }

    /// Get total output amount
    pub fn total_output(&self) -> u64 {
        self.outputs.iter().map(|o| o.amount()).sum()
    }

    /// Check basic structure validity
    pub fn is_valid_structure(&self) -> Result<(), &'static str> {
        if self.inputs.is_empty() {
            return Err("Transaction has no inputs");
        }
        if self.outputs.is_empty() {
            return Err("Transaction has no outputs");
        }
        if self.outputs.iter().any(|o| o.amount() == 0) {
            return Err("Transaction has zero-amount output");
        }
        if self.fee < MIN_TX_FEE {
            return Err("Transaction fee below minimum");
        }

        // Verify PQ signature sizes
        for input in &self.inputs {
            if input.classical_signature.len() != 64 {
                return Err("Invalid classical signature size");
            }
            if input.pq_signature.len() != PQ_SIGNATURE_SIZE {
                return Err("Invalid PQ signature size");
            }
        }

        // Verify PQ output sizes
        for output in &self.outputs {
            if output.pq_ciphertext.len() != PQ_CIPHERTEXT_SIZE {
                return Err("Invalid PQ ciphertext size");
            }
            if output.pq_signing_pubkey.len() != PQ_SIGNING_PUBKEY_SIZE {
                return Err("Invalid PQ signing public key size");
            }
        }

        Ok(())
    }

    /// Estimated serialized size
    pub fn estimated_size(&self) -> usize {
        let inputs_size = self.inputs.len() * QuantumPrivateTxInput::estimated_size();
        let outputs_size = self.outputs.len() * QuantumPrivateTxOutput::estimated_size();
        // Header: fee(8) + created_at_height(8) + lengths(~8)
        24 + inputs_size + outputs_size
    }

    /// Calculate the minimum required fee for this transaction.
    ///
    /// PQ transactions are ~19x larger than classical transactions, so they
    /// pay proportionally higher fees based on their size.
    ///
    /// Fee formula: base_fee + (size_bytes * fee_per_byte)
    pub fn minimum_fee(&self) -> u64 {
        calculate_pq_fee(self.inputs.len(), self.outputs.len())
    }

    /// Check if the transaction fee meets the minimum requirement
    pub fn has_sufficient_fee(&self) -> bool {
        self.fee >= self.minimum_fee()
    }
}

#[cfg(all(test, feature = "pq"))]
mod tests {
    use super::*;
    use bth_transaction_types::ClusterTagVector;

    const TEST_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    #[test]
    fn test_quantum_private_output_creation() {
        let account = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC);
        let address = account.default_subaddress();

        let output = QuantumPrivateTxOutput::new(1_000_000, &address);

        // Check sizes
        assert_eq!(output.pq_ciphertext.len(), PQ_CIPHERTEXT_SIZE);
        assert_eq!(output.pq_signing_pubkey.len(), PQ_SIGNING_PUBKEY_SIZE);
        assert_eq!(output.amount(), 1_000_000);
    }

    #[test]
    fn test_quantum_private_output_ownership() {
        let account = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC);
        let address = account.default_subaddress();

        let output = QuantumPrivateTxOutput::new(1_000_000, &address);

        // Should detect ownership
        let result = output.belongs_to(&account);
        assert!(result.is_some());

        let (subaddress_index, _shared_secret) = result.unwrap();
        assert_eq!(subaddress_index, 0);
    }

    #[test]
    fn test_quantum_private_output_wrong_account() {
        let account1 = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC);
        let account2 = QuantumSafeAccountKey::from_mnemonic(
            "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong",
        );

        let address1 = account1.default_subaddress();
        let output = QuantumPrivateTxOutput::new(1_000_000, &address1);

        // Different account should not detect ownership
        // Note: This may pass due to random PQ keys, but classical check should fail
        let result = output.belongs_to(&account2);
        assert!(result.is_none());
    }

    #[test]
    fn test_quantum_private_transaction_structure() {
        let tx = QuantumPrivateTransaction::new(
            vec![QuantumPrivateTxInput {
                tx_hash: [1u8; 32],
                output_index: 0,
                classical_signature: vec![0u8; 64],
                pq_signature: vec![0u8; PQ_SIGNATURE_SIZE],
            }],
            vec![QuantumPrivateTxOutput {
                classical: TxOutput {
                    amount: 1_000_000,
                    target_key: [2u8; 32],
                    public_key: [3u8; 32],
                    e_memo: None,
                    cluster_tags: ClusterTagVector::empty(),
                },
                pq_ciphertext: vec![0u8; PQ_CIPHERTEXT_SIZE],
                pq_signing_pubkey: vec![0u8; PQ_SIGNING_PUBKEY_SIZE],
            }],
            MIN_TX_FEE,
            100,
        );

        assert!(tx.is_valid_structure().is_ok());
    }

    #[test]
    fn test_quantum_private_transaction_hash_deterministic() {
        let tx = QuantumPrivateTransaction::new(
            vec![QuantumPrivateTxInput {
                tx_hash: [1u8; 32],
                output_index: 0,
                classical_signature: vec![0u8; 64],
                pq_signature: vec![0u8; PQ_SIGNATURE_SIZE],
            }],
            vec![QuantumPrivateTxOutput {
                classical: TxOutput {
                    amount: 1_000_000,
                    target_key: [2u8; 32],
                    public_key: [3u8; 32],
                    e_memo: None,
                    cluster_tags: ClusterTagVector::empty(),
                },
                pq_ciphertext: vec![0u8; PQ_CIPHERTEXT_SIZE],
                pq_signing_pubkey: vec![0u8; PQ_SIGNING_PUBKEY_SIZE],
            }],
            MIN_TX_FEE,
            100,
        );

        let hash1 = tx.hash();
        let hash2 = tx.hash();
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_estimated_sizes() {
        // Verify our size estimates match updated PLAN.md expectations
        // Output: 72 (classical) + 1088 (ciphertext) + 1952 (pubkey) = 3112
        assert!(QuantumPrivateTxOutput::estimated_size() > 3000);
        assert!(QuantumPrivateTxOutput::estimated_size() < 3500);

        assert!(QuantumPrivateTxInput::estimated_size() > 3000);
        assert!(QuantumPrivateTxInput::estimated_size() < 4000);
    }

    #[test]
    fn test_pq_fee_calculation() {
        // Simple 1-input, 2-output transaction
        let simple_fee = calculate_pq_fee(1, 2);

        // Should be higher than classical minimum fee due to larger size
        assert!(simple_fee >= MIN_TX_FEE);

        // For a simple PQ tx with new sizes:
        // Size = 24 + 3409 + 2*3112 = 9657 bytes
        // Fee = 9657 * 10_000 = 96,570,000 picocredits
        // This is less than MIN_TX_FEE (100_000_000), so use MIN_TX_FEE
        assert_eq!(simple_fee, MIN_TX_FEE);
    }

    #[test]
    fn test_pq_fee_scales_with_size() {
        let fee_1_in = calculate_pq_fee(1, 2);
        let fee_10_in = calculate_pq_fee(10, 2);

        // More inputs = higher fee
        assert!(fee_10_in > fee_1_in);
    }

    #[test]
    fn test_pq_fee_large_transaction() {
        // Large transaction with 10 inputs, 10 outputs
        let large_fee = calculate_pq_fee(10, 10);

        // Size = 24 + 10*3409 + 10*3112 = 65,234 bytes
        // Fee = 65,234 * 10,000 = 652,340,000 picocredits
        // This exceeds MIN_TX_FEE, so use calculated fee
        assert!(large_fee > MIN_TX_FEE);
        assert!(large_fee > 600_000_000); // ~0.6 credits
    }

    #[test]
    fn test_transaction_minimum_fee_method() {
        let tx = QuantumPrivateTransaction::new(
            vec![QuantumPrivateTxInput {
                tx_hash: [1u8; 32],
                output_index: 0,
                classical_signature: vec![0u8; 64],
                pq_signature: vec![0u8; PQ_SIGNATURE_SIZE],
            }],
            vec![QuantumPrivateTxOutput {
                classical: TxOutput {
                    amount: 1_000_000,
                    target_key: [2u8; 32],
                    public_key: [3u8; 32],
                    e_memo: None,
                    cluster_tags: ClusterTagVector::empty(),
                },
                pq_ciphertext: vec![0u8; PQ_CIPHERTEXT_SIZE],
                pq_signing_pubkey: vec![0u8; PQ_SIGNING_PUBKEY_SIZE],
            }],
            MIN_TX_FEE,
            100,
        );

        // Transaction minimum_fee should match calculated fee
        assert_eq!(tx.minimum_fee(), calculate_pq_fee(1, 1));

        // With MIN_TX_FEE, should have sufficient fee
        assert!(tx.has_sufficient_fee());
    }

    #[test]
    fn test_transaction_insufficient_fee() {
        let tx = QuantumPrivateTransaction::new(
            (0..10).map(|_| QuantumPrivateTxInput {
                tx_hash: [1u8; 32],
                output_index: 0,
                classical_signature: vec![0u8; 64],
                pq_signature: vec![0u8; PQ_SIGNATURE_SIZE],
            }).collect(),
            (0..10).map(|_| QuantumPrivateTxOutput {
                classical: TxOutput {
                    amount: 1_000_000,
                    target_key: [2u8; 32],
                    public_key: [3u8; 32],
                    e_memo: None,
                    cluster_tags: ClusterTagVector::empty(),
                },
                pq_ciphertext: vec![0u8; PQ_CIPHERTEXT_SIZE],
                pq_signing_pubkey: vec![0u8; PQ_SIGNING_PUBKEY_SIZE],
            }).collect(),
            MIN_TX_FEE, // This is too low for a large tx
            100,
        );

        // Large tx needs more than MIN_TX_FEE
        assert!(tx.minimum_fee() > MIN_TX_FEE);
        assert!(!tx.has_sufficient_fee());
    }
}
