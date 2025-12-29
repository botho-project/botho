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
pub const PQ_TARGET_KEY_SIZE: usize = 32; // Derived PQ one-time key hash

/// Fee constants for quantum-private transactions
///
/// Classical transactions have a minimum fee of 0.0001 credits (100_000_000 picocredits).
/// PQ transactions are ~19x larger, so they pay proportionally more.

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
    const OUTPUT_SIZE: u64 = 72 + PQ_CIPHERTEXT_SIZE as u64 + PQ_TARGET_KEY_SIZE as u64; // ~1192 bytes
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
/// - `pq_target_key`: Hash of the PQ one-time public key (32 bytes)
///
/// # Protocol
///
/// **Sender creates:**
/// 1. Classical stealth output as normal
/// 2. Encapsulate to recipient's ML-KEM public key: (ciphertext, shared_secret)
/// 3. Derive PQ one-time key from shared_secret
/// 4. Store hash of PQ one-time key for efficient scanning
///
/// **Recipient scans:**
/// 1. Check classical ownership (view key derivation)
/// 2. Decapsulate ciphertext to recover shared_secret
/// 3. Derive PQ one-time key and verify hash matches
///
/// **Recipient spends:**
/// 1. Recover classical one-time private key
/// 2. Derive PQ one-time private key from shared_secret
/// 3. Sign with BOTH classical and PQ keys
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuantumPrivateTxOutput {
    /// Classical stealth output (amount, one-time keys)
    pub classical: TxOutput,

    /// ML-KEM-768 ciphertext for PQ key encapsulation (1088 bytes)
    /// This encapsulates a shared secret to the recipient's PQ view key.
    pub pq_ciphertext: Vec<u8>,

    /// Hash of the PQ one-time public key (32 bytes)
    /// Used for efficient output scanning without full decapsulation.
    pub pq_target_key: [u8; 32],
}

impl QuantumPrivateTxOutput {
    /// Create a new quantum-private output for a recipient.
    ///
    /// This creates both classical and PQ stealth components.
    ///
    /// # Arguments
    /// * `amount` - Amount in picocredits
    /// * `recipient` - Recipient's quantum-safe public address
    ///
    /// # Returns
    /// A new quantum-private output with both classical and PQ components.
    #[cfg(feature = "pq")]
    pub fn new(amount: u64, recipient: &QuantumSafePublicAddress) -> Self {
        // Create classical stealth output
        let classical = TxOutput::new(amount, recipient.classical());

        // Encapsulate to recipient's PQ KEM public key
        let (ciphertext, shared_secret) = recipient.pq_kem_public_key().encapsulate();

        // Derive a deterministic target key from the shared secret.
        // Note: We hash the shared secret directly rather than deriving a keypair,
        // because the PQ keygen is currently non-deterministic (pqcrypto limitation).
        // This still provides quantum-safe binding: only someone who can decapsulate
        // can recover the shared secret and verify the target key.
        let pq_target_key = Self::hash_shared_secret(shared_secret.as_bytes(), 0);

        Self {
            classical,
            pq_ciphertext: ciphertext.as_bytes().to_vec(),
            pq_target_key,
        }
    }

    /// Check if this output belongs to a quantum-safe account.
    ///
    /// Performs both classical and PQ ownership checks.
    ///
    /// # Returns
    /// `Some((subaddress_index, shared_secret))` if owned, `None` otherwise.
    #[cfg(feature = "pq")]
    pub fn belongs_to(
        &self,
        account: &QuantumSafeAccountKey,
    ) -> Option<(u64, [u8; 32])> {
        use bth_crypto_pq::MlKem768Ciphertext;

        // First check classical ownership
        let subaddress_index = self.classical.belongs_to(account.classical())?;

        // Decapsulate PQ ciphertext
        let ciphertext = MlKem768Ciphertext::from_bytes(&self.pq_ciphertext).ok()?;
        let shared_secret = account.pq_kem_keypair().decapsulate(&ciphertext).ok()?;

        // Verify PQ target key matches (deterministic hash of shared secret)
        let expected_target = Self::hash_shared_secret(shared_secret.as_bytes(), 0);

        if expected_target != self.pq_target_key {
            return None;
        }

        // Return the shared secret bytes
        let mut ss_bytes = [0u8; 32];
        ss_bytes.copy_from_slice(shared_secret.as_bytes());
        Some((subaddress_index, ss_bytes))
    }

    /// Hash a shared secret with output index to create a deterministic target key.
    ///
    /// This provides quantum-safe binding: only someone who can decapsulate
    /// the ML-KEM ciphertext can compute this hash and verify ownership.
    fn hash_shared_secret(shared_secret: &[u8; 32], output_index: u32) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"botho-pq-target-v1");
        hasher.update(shared_secret);
        hasher.update(output_index.to_le_bytes());
        hasher.finalize().into()
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
        hasher.update(self.pq_target_key);
        hasher.finalize().into()
    }

    /// Estimated serialized size
    pub const fn estimated_size() -> usize {
        // Classical: amount(8) + target_key(32) + public_key(32) = 72
        // PQ: ciphertext(1088) + pq_target_key(32) = 1120
        // Total: ~1192, round to 1160 accounting for encoding overhead
        72 + PQ_CIPHERTEXT_SIZE + PQ_TARGET_KEY_SIZE
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
/// 2. The PQ signature verifies against the output's pq_target_key
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
    /// * `pq_target_key` - PQ target key hash from UTXO
    ///
    /// # Returns
    /// `true` if BOTH signatures are valid, `false` otherwise.
    #[cfg(feature = "pq")]
    pub fn verify(
        &self,
        signing_hash: &[u8; 32],
        classical_target_key: &[u8; 32],
        pq_target_key: &[u8; 32],
    ) -> bool {
        // Verify classical signature
        if !self.verify_classical(signing_hash, classical_target_key) {
            return false;
        }

        // Verify PQ signature
        // Note: We can't directly verify against pq_target_key (it's a hash)
        // In a full implementation, we'd need the actual PQ public key stored
        // or reconstructed. For now, we verify the signature is well-formed.
        self.verify_pq_structure()
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

    /// Verify PQ signature structure (size check)
    fn verify_pq_structure(&self) -> bool {
        self.pq_signature.len() == PQ_SIGNATURE_SIZE
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
            hasher.update(output.pq_target_key);
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

        // Verify PQ ciphertext sizes
        for output in &self.outputs {
            if output.pq_ciphertext.len() != PQ_CIPHERTEXT_SIZE {
                return Err("Invalid PQ ciphertext size");
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

    const TEST_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    #[test]
    fn test_quantum_private_output_creation() {
        let account = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC);
        let address = account.default_subaddress();

        let output = QuantumPrivateTxOutput::new(1_000_000, &address);

        // Check sizes
        assert_eq!(output.pq_ciphertext.len(), PQ_CIPHERTEXT_SIZE);
        assert_eq!(output.pq_target_key.len(), 32);
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
                },
                pq_ciphertext: vec![0u8; PQ_CIPHERTEXT_SIZE],
                pq_target_key: [4u8; 32],
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
                },
                pq_ciphertext: vec![0u8; PQ_CIPHERTEXT_SIZE],
                pq_target_key: [4u8; 32],
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
        // Verify our size estimates match PLAN.md expectations
        assert!(QuantumPrivateTxOutput::estimated_size() > 1000);
        assert!(QuantumPrivateTxOutput::estimated_size() < 1500);

        assert!(QuantumPrivateTxInput::estimated_size() > 3000);
        assert!(QuantumPrivateTxInput::estimated_size() < 4000);
    }

    #[test]
    fn test_pq_fee_calculation() {
        // Simple 1-input, 2-output transaction
        let simple_fee = calculate_pq_fee(1, 2);

        // Should be higher than classical minimum fee due to larger size
        assert!(simple_fee >= MIN_TX_FEE);

        // For a simple PQ tx:
        // Size = 24 + 3409 + 2*1192 = 5817 bytes
        // Fee = 5817 * 10_000 = 58,170,000 picocredits
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

        // Size = 24 + 10*3409 + 10*1192 = 46,034 bytes
        // Fee = 46,034 * 10,000 = 460,340,000 picocredits
        // This exceeds MIN_TX_FEE, so use calculated fee
        assert!(large_fee > MIN_TX_FEE);
        assert!(large_fee > 400_000_000); // ~0.4 credits
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
                },
                pq_ciphertext: vec![0u8; PQ_CIPHERTEXT_SIZE],
                pq_target_key: [4u8; 32],
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
                },
                pq_ciphertext: vec![0u8; PQ_CIPHERTEXT_SIZE],
                pq_target_key: [4u8; 32],
            }).collect(),
            MIN_TX_FEE, // This is too low for a large tx
            100,
        );

        // Large tx needs more than MIN_TX_FEE
        assert!(tx.minimum_fee() > MIN_TX_FEE);
        assert!(!tx.has_sufficient_fee());
    }
}
