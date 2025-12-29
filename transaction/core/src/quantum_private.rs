//! Quantum-Private Transaction Types
//!
//! This module provides transaction types that are protected against quantum
//! adversaries using a hybrid classical + post-quantum cryptographic approach.
//!
//! # Security Model
//!
//! Quantum-private transactions require BOTH layers to verify:
//! - Classical layer: Schnorr signatures on Ristretto (current security)
//! - Post-quantum layer: ML-DSA-65 (Dilithium) signatures (future security)
//!
//! This hybrid approach provides:
//! 1. Immediate protection against "harvest now, decrypt later" attacks
//! 2. Fallback security if either cryptosystem is compromised
//! 3. Privacy that persists even after quantum computers become practical
//!
//! # Transaction Structure
//!
//! ```text
//! QuantumPrivateTxOut (1160 bytes):
//!   - Classical: amount, target_key, public_key (72 B)
//!   - PQ: ML-KEM-768 ciphertext (1088 B)
//!
//! QuantumPrivateTxIn (2520+ bytes):
//!   - Reference: tx_hash, output_index (36 B)
//!   - Classical signature: Schnorr (64 B)
//!   - PQ signature: ML-DSA-65 (3309 B)
//! ```

use alloc::string::ToString;
use alloc::vec::Vec;

#[allow(unused_imports)]
use core::fmt; // Keep for Display impl

use bth_crypto_digestible::Digestible;
use bth_crypto_keys::CompressedRistrettoPublic;
use bth_crypto_pq::{
    MlDsa65PublicKey, MlDsa65Signature, MlKem768Ciphertext,
    ML_DSA_65_PUBLIC_KEY_BYTES, ML_DSA_65_SIGNATURE_BYTES, ML_KEM_768_CIPHERTEXT_BYTES,
};
use prost::Message;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::{
    memo::EncryptedMemo,
    tx::TxHash,
    ClusterTagVector, MaskedAmount,
};

/// Transaction type discriminator
///
/// Used to identify whether a transaction uses classical or quantum-safe
/// cryptography. This is encoded in the transaction format to enable
/// validators to apply the correct verification rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum TransactionType {
    /// Classical stealth addresses with Schnorr signatures.
    /// Quantum-vulnerable but can be upgraded via hard fork.
    Standard = 0,

    /// Hybrid classical + post-quantum.
    /// Both crypto layers must verify.
    /// Privacy protected against quantum adversaries.
    QuantumPrivate = 1,
}

impl Default for TransactionType {
    fn default() -> Self {
        TransactionType::Standard
    }
}

impl From<u8> for TransactionType {
    fn from(value: u8) -> Self {
        match value {
            1 => TransactionType::QuantumPrivate,
            _ => TransactionType::Standard,
        }
    }
}

impl From<TransactionType> for u8 {
    fn from(value: TransactionType) -> Self {
        value as u8
    }
}

/// A quantum-private transaction output.
///
/// This extends the classical TxOut with an ML-KEM-768 ciphertext that
/// encapsulates a shared secret to the recipient's post-quantum key.
///
/// # Size
///
/// - Classical components: ~72 bytes (amount, target_key, public_key)
/// - PQ ciphertext: 1088 bytes
/// - Total: ~1160 bytes per output (vs ~72 for classical)
#[derive(Clone, Deserialize, Digestible, Eq, Hash, Message, PartialEq, Serialize, Zeroize)]
pub struct QuantumPrivateTxOut {
    // === Classical Layer ===

    /// The masked amount being sent.
    #[prost(oneof = "MaskedAmount", tags = "1, 6")]
    #[digestible(name = "amount")]
    #[zeroize(skip)]
    pub masked_amount: Option<MaskedAmount>,

    /// The one-time public address of this output (classical).
    #[prost(message, required, tag = "2")]
    pub target_key: CompressedRistrettoPublic,

    /// The per-output tx public key for ECDH (R = r*G).
    #[prost(message, required, tag = "3")]
    pub public_key: CompressedRistrettoPublic,

    // Field 4 was `e_fog_hint` - removed as part of fog removal

    /// The encrypted memo.
    #[prost(message, tag = "5")]
    #[zeroize(skip)]
    pub e_memo: Option<EncryptedMemo>,

    /// Cluster tag vector for progressive transaction fees.
    #[prost(message, tag = "7")]
    #[zeroize(skip)]
    pub cluster_tags: Option<ClusterTagVector>,

    // === Post-Quantum Layer ===

    /// ML-KEM-768 ciphertext encapsulating a shared secret to the recipient's
    /// PQ view key. The recipient decapsulates this to derive the PQ one-time
    /// signing key for spending.
    #[prost(bytes, tag = "10")]
    pub pq_ciphertext: Vec<u8>,

    /// ML-DSA-65 one-time public key derived from the encapsulated secret.
    /// Used to verify the PQ signature when this output is spent.
    #[prost(bytes, tag = "11")]
    pub pq_target_key: Vec<u8>,
}

impl QuantumPrivateTxOut {
    /// Create a new quantum-private transaction output.
    ///
    /// # Arguments
    ///
    /// * `masked_amount` - The encrypted amount
    /// * `target_key` - Classical one-time spend public key
    /// * `public_key` - Classical ephemeral ECDH public key
    /// * `pq_ciphertext` - ML-KEM-768 ciphertext
    /// * `pq_target_key` - ML-DSA-65 one-time public key
    pub fn new(
        masked_amount: MaskedAmount,
        target_key: CompressedRistrettoPublic,
        public_key: CompressedRistrettoPublic,
        pq_ciphertext: MlKem768Ciphertext,
        pq_target_key: MlDsa65PublicKey,
    ) -> Self {
        Self {
            masked_amount: Some(masked_amount),
            target_key,
            public_key,
            e_memo: None,
            cluster_tags: None,
            pq_ciphertext: pq_ciphertext.as_bytes().to_vec(),
            pq_target_key: pq_target_key.as_bytes().to_vec(),
        }
    }

    /// Get the ML-KEM ciphertext.
    pub fn get_pq_ciphertext(&self) -> Result<MlKem768Ciphertext, QuantumPrivateError> {
        MlKem768Ciphertext::from_bytes(&self.pq_ciphertext)
            .map_err(|e| QuantumPrivateError::InvalidCiphertext(e.to_string()))
    }

    /// Get the ML-DSA one-time public key.
    pub fn get_pq_target_key(&self) -> Result<MlDsa65PublicKey, QuantumPrivateError> {
        MlDsa65PublicKey::from_bytes(&self.pq_target_key)
            .map_err(|e| QuantumPrivateError::InvalidPublicKey(e.to_string()))
    }

    /// Total serialized size of a quantum-private output.
    pub const APPROX_SIZE: usize = 72 + ML_KEM_768_CIPHERTEXT_BYTES + ML_DSA_65_PUBLIC_KEY_BYTES;
}


/// A quantum-private transaction input.
///
/// This contains both classical (Schnorr) and post-quantum (ML-DSA-65)
/// signatures. BOTH signatures must verify for the input to be valid.
///
/// # Size
///
/// - Reference: 36 bytes (tx_hash + output_index)
/// - Classical signature: 64 bytes
/// - PQ signature: 3309 bytes
/// - Total: ~3409 bytes per input (vs ~100 for classical)
#[derive(Clone, Deserialize, Digestible, Eq, PartialEq, Message, Serialize, Zeroize)]
pub struct QuantumPrivateTxIn {
    /// Hash of the transaction containing the output being spent.
    #[prost(bytes, tag = "1")]
    pub tx_hash: Vec<u8>,

    /// Index of the output within the transaction.
    #[prost(uint32, tag = "2")]
    pub output_index: u32,

    /// Classical Schnorr signature using the one-time private key.
    /// Signs the transaction prefix hash.
    #[prost(bytes, tag = "3")]
    pub schnorr_signature: Vec<u8>,

    /// ML-DSA-65 signature using the PQ one-time private key.
    /// Signs the same transaction prefix hash.
    #[prost(bytes, tag = "4")]
    pub dilithium_signature: Vec<u8>,
}

impl QuantumPrivateTxIn {
    /// Create a new quantum-private transaction input.
    ///
    /// # Arguments
    ///
    /// * `tx_hash` - Hash of the transaction being spent from
    /// * `output_index` - Index of the output being spent
    /// * `schnorr_signature` - Classical signature
    /// * `dilithium_signature` - Post-quantum signature
    pub fn new(
        tx_hash: TxHash,
        output_index: u32,
        schnorr_signature: [u8; 64],
        dilithium_signature: MlDsa65Signature,
    ) -> Self {
        Self {
            tx_hash: tx_hash.to_vec(),
            output_index,
            schnorr_signature: schnorr_signature.to_vec(),
            dilithium_signature: dilithium_signature.as_bytes().to_vec(),
        }
    }

    /// Get the transaction hash.
    pub fn get_tx_hash(&self) -> Result<TxHash, QuantumPrivateError> {
        TxHash::try_from(self.tx_hash.as_slice())
            .map_err(|_| QuantumPrivateError::InvalidTxHash)
    }

    /// Get the classical signature.
    pub fn get_schnorr_signature(&self) -> Result<[u8; 64], QuantumPrivateError> {
        self.schnorr_signature
            .as_slice()
            .try_into()
            .map_err(|_| QuantumPrivateError::InvalidSignature("Schnorr signature wrong length".into()))
    }

    /// Get the post-quantum signature.
    pub fn get_dilithium_signature(&self) -> Result<MlDsa65Signature, QuantumPrivateError> {
        MlDsa65Signature::from_bytes(&self.dilithium_signature)
            .map_err(|e| QuantumPrivateError::InvalidSignature(e.to_string()))
    }

    /// Total serialized size of a quantum-private input.
    pub const APPROX_SIZE: usize = 36 + 64 + ML_DSA_65_SIGNATURE_BYTES;
}

/// Errors that can occur with quantum-private transactions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuantumPrivateError {
    /// Invalid ML-KEM ciphertext
    InvalidCiphertext(alloc::string::String),
    /// Invalid ML-DSA public key
    InvalidPublicKey(alloc::string::String),
    /// Invalid signature
    InvalidSignature(alloc::string::String),
    /// Invalid transaction hash
    InvalidTxHash,
    /// Schnorr signature verification failed
    SchnorrVerificationFailed,
    /// Dilithium signature verification failed
    DilithiumVerificationFailed,
    /// Both signatures must be present
    MissingSignature,
}

impl fmt::Display for QuantumPrivateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCiphertext(s) => write!(f, "Invalid PQ ciphertext: {}", s),
            Self::InvalidPublicKey(s) => write!(f, "Invalid PQ public key: {}", s),
            Self::InvalidSignature(s) => write!(f, "Invalid signature: {}", s),
            Self::InvalidTxHash => write!(f, "Invalid transaction hash"),
            Self::SchnorrVerificationFailed => write!(f, "Schnorr signature verification failed"),
            Self::DilithiumVerificationFailed => write!(f, "Dilithium signature verification failed"),
            Self::MissingSignature => write!(f, "Both classical and PQ signatures required"),
        }
    }
}

/// Size comparison between classical and quantum-private transactions.
///
/// This provides constants for understanding the overhead of quantum resistance.
pub mod size_comparison {
    use super::*;

    /// Classical TxOut approximate size (bytes)
    pub const CLASSICAL_TX_OUT: usize = 72;

    /// Quantum-private TxOut approximate size (bytes)
    pub const QUANTUM_PRIVATE_TX_OUT: usize = QuantumPrivateTxOut::APPROX_SIZE;

    /// Classical TxIn approximate size (bytes)
    pub const CLASSICAL_TX_IN: usize = 100;

    /// Quantum-private TxIn approximate size (bytes)
    pub const QUANTUM_PRIVATE_TX_IN: usize = QuantumPrivateTxIn::APPROX_SIZE;

    /// Output overhead multiplier (quantum / classical)
    pub const OUTPUT_OVERHEAD: f64 = QUANTUM_PRIVATE_TX_OUT as f64 / CLASSICAL_TX_OUT as f64;

    /// Input overhead multiplier (quantum / classical)
    pub const INPUT_OVERHEAD: f64 = QUANTUM_PRIVATE_TX_IN as f64 / CLASSICAL_TX_IN as f64;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transaction_type_default() {
        assert_eq!(TransactionType::default(), TransactionType::Standard);
    }

    #[test]
    fn test_transaction_type_from_u8() {
        assert_eq!(TransactionType::from(0), TransactionType::Standard);
        assert_eq!(TransactionType::from(1), TransactionType::QuantumPrivate);
        assert_eq!(TransactionType::from(255), TransactionType::Standard); // Unknown defaults to Standard
    }

    #[test]
    fn test_transaction_type_to_u8() {
        assert_eq!(u8::from(TransactionType::Standard), 0);
        assert_eq!(u8::from(TransactionType::QuantumPrivate), 1);
    }

    #[test]
    fn test_size_constants() {
        // Verify our size expectations
        assert_eq!(ML_KEM_768_CIPHERTEXT_BYTES, 1088);
        assert_eq!(ML_DSA_65_SIGNATURE_BYTES, 3309);
        assert_eq!(ML_DSA_65_PUBLIC_KEY_BYTES, 1952);

        // Check approximate sizes
        assert!(QuantumPrivateTxOut::APPROX_SIZE > 1000);
        assert!(QuantumPrivateTxIn::APPROX_SIZE > 3000);
    }

    #[test]
    fn test_size_comparison() {
        use size_comparison::*;

        // Quantum-private outputs are ~43x larger
        // (72 classical + 1088 ML-KEM ciphertext + 1952 ML-DSA public key = 3112 bytes)
        // vs 72 bytes for classical
        assert!(OUTPUT_OVERHEAD > 40.0);
        assert!(OUTPUT_OVERHEAD < 50.0);

        // Quantum-private inputs are ~34x larger
        // (36 reference + 64 Schnorr + 3309 Dilithium = 3409 bytes)
        // vs 100 bytes for classical
        assert!(INPUT_OVERHEAD > 30.0);
        assert!(INPUT_OVERHEAD < 40.0);
    }
}
