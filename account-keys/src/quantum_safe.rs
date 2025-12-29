//! Quantum-Safe Account Keys
//!
//! This module provides post-quantum cryptographic extensions to the standard
//! account key types. It combines classical (Ristretto/Schnorr) keys with
//! NIST-standardized post-quantum algorithms:
//!
//! - **ML-KEM-768** (Kyber): Key Encapsulation for stealth address key exchange
//! - **ML-DSA-65** (Dilithium): Digital signatures for transaction signing
//!
//! # Hybrid Security Model
//!
//! Quantum-safe transactions require BOTH classical and post-quantum signatures
//! to verify. This provides:
//!
//! 1. Immediate protection against "harvest now, decrypt later" attacks
//! 2. Fallback security if either cryptosystem is broken
//! 3. Backward compatibility with existing infrastructure
//!
//! # Usage
//!
//! ```ignore
//! use bth_account_keys::{QuantumSafeAccountKey, QuantumSafePublicAddress};
//!
//! // Create from mnemonic (PQ keys derived deterministically)
//! let account = QuantumSafeAccountKey::from_mnemonic("word1 word2 ... word24");
//!
//! // Get quantum-safe public address for receiving
//! let address = account.default_subaddress();
//!
//! // Address includes both classical and PQ public keys
//! println!("Classical view key: {:?}", address.view_public_key());
//! println!("PQ KEM key: {:?}", address.pq_view_public_key());
//! ```

use alloc::{string::String, vec::Vec};
use core::fmt;

use bth_crypto_pq::{
    derive_pq_keys, MlDsa65KeyPair, MlDsa65PublicKey, MlKem768KeyPair, MlKem768PublicKey,
    PqKeyMaterial, ML_DSA_65_PUBLIC_KEY_BYTES, ML_KEM_768_PUBLIC_KEY_BYTES,
};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::{AccountKey, PublicAddress};

/// Quantum-safe account key combining classical and post-quantum keys
///
/// This structure contains:
/// - A classical `AccountKey` (Ristretto view/spend keys)
/// - An ML-KEM-768 keypair for quantum-safe stealth address key exchange
/// - An ML-DSA-65 keypair for quantum-safe transaction signing
///
/// All keys are derived from the same mnemonic, so users only need to
/// backup a single seed phrase.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct QuantumSafeAccountKey {
    /// Classical account key (Ristretto-based)
    #[zeroize(skip)] // AccountKey handles its own zeroization
    classical: AccountKey,

    /// ML-KEM-768 keypair for PQ key encapsulation (view key equivalent)
    #[zeroize(skip)] // MlKem768KeyPair handles its own zeroization
    pq_kem_keypair: MlKem768KeyPair,

    /// ML-DSA-65 keypair for PQ signatures (spend key equivalent)
    #[zeroize(skip)] // MlDsa65KeyPair handles its own zeroization
    pq_sig_keypair: MlDsa65KeyPair,
}

impl QuantumSafeAccountKey {
    /// Create a quantum-safe account key from a mnemonic phrase
    ///
    /// This derives both classical (Ristretto) and post-quantum (ML-KEM, ML-DSA)
    /// keypairs from the same mnemonic. The classical keys use the standard
    /// SLIP-0010 derivation path, while PQ keys use HKDF with domain separation.
    ///
    /// # Arguments
    ///
    /// * `mnemonic` - A BIP39 mnemonic phrase (typically 24 words)
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    /// let account = QuantumSafeAccountKey::from_mnemonic(mnemonic);
    /// ```
    pub fn from_mnemonic(mnemonic: &str) -> Self {
        // Derive classical keys using existing infrastructure
        let classical = Self::derive_classical_from_mnemonic(mnemonic);

        // Derive PQ keys from the same mnemonic
        let pq_keys = derive_pq_keys(mnemonic.as_bytes());

        Self {
            classical,
            pq_kem_keypair: pq_keys.kem_keypair,
            pq_sig_keypair: pq_keys.sig_keypair,
        }
    }

    /// Create from existing classical AccountKey and PQ key material
    ///
    /// Use this when you have already derived the classical key through
    /// other means and want to add PQ support.
    pub fn from_parts(classical: AccountKey, pq_keys: PqKeyMaterial) -> Self {
        Self {
            classical,
            pq_kem_keypair: pq_keys.kem_keypair,
            pq_sig_keypair: pq_keys.sig_keypair,
        }
    }

    /// Get the classical account key
    pub fn classical(&self) -> &AccountKey {
        &self.classical
    }

    /// Get the ML-KEM-768 keypair (for stealth address key exchange)
    pub fn pq_kem_keypair(&self) -> &MlKem768KeyPair {
        &self.pq_kem_keypair
    }

    /// Get the ML-DSA-65 keypair (for transaction signing)
    pub fn pq_sig_keypair(&self) -> &MlDsa65KeyPair {
        &self.pq_sig_keypair
    }

    /// Get the default quantum-safe subaddress
    pub fn default_subaddress(&self) -> QuantumSafePublicAddress {
        QuantumSafePublicAddress::from_parts(
            self.classical.default_subaddress(),
            self.pq_kem_keypair.public_key().clone(),
            self.pq_sig_keypair.public_key().clone(),
        )
    }

    /// Get the change quantum-safe subaddress
    pub fn change_subaddress(&self) -> QuantumSafePublicAddress {
        QuantumSafePublicAddress::from_parts(
            self.classical.change_subaddress(),
            self.pq_kem_keypair.public_key().clone(),
            self.pq_sig_keypair.public_key().clone(),
        )
    }

    /// Get the i^th quantum-safe subaddress
    ///
    /// Note: The PQ public keys are the same across all subaddresses.
    /// Subaddress derivation only affects the classical keys.
    /// One-time PQ keys are derived per-output using the encapsulated secret.
    pub fn subaddress(&self, index: u64) -> QuantumSafePublicAddress {
        QuantumSafePublicAddress::from_parts(
            self.classical.subaddress(index),
            self.pq_kem_keypair.public_key().clone(),
            self.pq_sig_keypair.public_key().clone(),
        )
    }

    /// Derive classical AccountKey from mnemonic
    ///
    /// This uses HKDF to derive keys from the mnemonic bytes, similar to
    /// the RootIdentity approach. For proper BIP39 compliance, use
    /// `from_parts` with an AccountKey derived through the bip39 feature.
    fn derive_classical_from_mnemonic(mnemonic: &str) -> AccountKey {
        use hkdf::Hkdf;
        use sha2::Sha256;

        // Hash the mnemonic to get 32 bytes of entropy
        let hk = Hkdf::<Sha256>::new(Some(b"botho-classical-v1"), mnemonic.as_bytes());

        let mut entropy = [0u8; 32];
        hk.expand(b"root-entropy", &mut entropy)
            .expect("32 bytes is valid for HKDF-SHA256");

        // Use RootIdentity to derive the AccountKey
        let root_id = crate::RootIdentity::from(&entropy);
        AccountKey::from(&root_id)
    }
}

impl fmt::Debug for QuantumSafeAccountKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QuantumSafeAccountKey")
            .field("classical", &"[AccountKey]")
            .field("pq_kem", &"[MlKem768KeyPair]")
            .field("pq_sig", &"[MlDsa65KeyPair]")
            .finish()
    }
}

/// Quantum-safe public address for receiving funds
///
/// This extended address format includes both classical and post-quantum
/// public keys, enabling senders to create outputs that are protected
/// against quantum adversaries.
///
/// # Size
///
/// - Classical keys: 64 bytes (view + spend)
/// - PQ KEM key: 1184 bytes (ML-KEM-768)
/// - PQ Signature key: 1952 bytes (ML-DSA-65)
/// - Total: ~3200 bytes
///
/// # Encoding
///
/// Quantum-safe addresses use a distinct prefix for identification:
/// - Standard: `botho://1/<base58(view||spend)>`
/// - Quantum-safe: `botho-pq://1/<base58(view||spend||pq_kem||pq_sig)>`
#[derive(Clone)]
pub struct QuantumSafePublicAddress {
    /// Classical public address (view + spend public keys)
    classical: PublicAddress,

    /// ML-KEM-768 public key for quantum-safe key encapsulation
    pq_kem_public_key: MlKem768PublicKey,

    /// ML-DSA-65 public key for quantum-safe signature verification
    pq_sig_public_key: MlDsa65PublicKey,
}

impl QuantumSafePublicAddress {
    /// Create a quantum-safe address from its component parts
    pub fn from_parts(
        classical: PublicAddress,
        pq_kem_public_key: MlKem768PublicKey,
        pq_sig_public_key: MlDsa65PublicKey,
    ) -> Self {
        Self {
            classical,
            pq_kem_public_key,
            pq_sig_public_key,
        }
    }

    /// Get the classical public address
    pub fn classical(&self) -> &PublicAddress {
        &self.classical
    }

    /// Get the classical view public key
    pub fn view_public_key(&self) -> &bth_crypto_keys::RistrettoPublic {
        self.classical.view_public_key()
    }

    /// Get the classical spend public key
    pub fn spend_public_key(&self) -> &bth_crypto_keys::RistrettoPublic {
        self.classical.spend_public_key()
    }

    /// Get the ML-KEM-768 public key for quantum-safe key encapsulation
    pub fn pq_kem_public_key(&self) -> &MlKem768PublicKey {
        &self.pq_kem_public_key
    }

    /// Get the ML-DSA-65 public key for quantum-safe signature verification
    pub fn pq_sig_public_key(&self) -> &MlDsa65PublicKey {
        &self.pq_sig_public_key
    }

    /// Serialize to bytes
    ///
    /// Format: classical_view || classical_spend || pq_kem || pq_sig
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(
            32 + 32 + ML_KEM_768_PUBLIC_KEY_BYTES + ML_DSA_65_PUBLIC_KEY_BYTES,
        );

        bytes.extend_from_slice(&self.classical.view_public_key().to_bytes());
        bytes.extend_from_slice(&self.classical.spend_public_key().to_bytes());
        bytes.extend_from_slice(self.pq_kem_public_key.as_bytes());
        bytes.extend_from_slice(self.pq_sig_public_key.as_bytes());

        bytes
    }

    /// Total serialized size in bytes
    pub const SERIALIZED_SIZE: usize =
        32 + 32 + ML_KEM_768_PUBLIC_KEY_BYTES + ML_DSA_65_PUBLIC_KEY_BYTES;

    /// Encode as a quantum-safe address string
    ///
    /// Format: `botho-pq://1/<base58>`
    pub fn to_address_string(&self) -> String {
        use alloc::format;

        let bytes = self.to_bytes();
        let encoded = bs58::encode(&bytes).into_string();
        format!("botho-pq://1/{}", encoded)
    }
}

impl fmt::Debug for QuantumSafePublicAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QuantumSafePublicAddress")
            .field("classical", &self.classical)
            .field("pq_kem", &format_args!("MlKem768PublicKey(...)"))
            .field("pq_sig", &format_args!("MlDsa65PublicKey(...)"))
            .finish()
    }
}

impl fmt::Display for QuantumSafePublicAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_address_string())
    }
}

impl PartialEq for QuantumSafePublicAddress {
    fn eq(&self, other: &Self) -> bool {
        self.classical == other.classical
            && self.pq_kem_public_key.as_bytes() == other.pq_kem_public_key.as_bytes()
            && self.pq_sig_public_key.as_bytes() == other.pq_sig_public_key.as_bytes()
    }
}

impl Eq for QuantumSafePublicAddress {}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    #[test]
    fn test_quantum_safe_account_key_creation() {
        let account = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC);

        // Verify we have valid keys
        assert!(account.classical().view_private_key().as_ref().len() > 0);
    }

    #[test]
    fn test_quantum_safe_public_address() {
        let account = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC);
        let address = account.default_subaddress();

        // Check size constants
        assert_eq!(
            address.pq_kem_public_key().as_bytes().len(),
            ML_KEM_768_PUBLIC_KEY_BYTES
        );
        assert_eq!(
            address.pq_sig_public_key().as_bytes().len(),
            ML_DSA_65_PUBLIC_KEY_BYTES
        );
    }

    #[test]
    fn test_address_serialization() {
        let account = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC);
        let address = account.default_subaddress();

        let bytes = address.to_bytes();
        assert_eq!(bytes.len(), QuantumSafePublicAddress::SERIALIZED_SIZE);
    }

    #[test]
    fn test_address_string_format() {
        let account = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC);
        let address = account.default_subaddress();

        let addr_string = address.to_address_string();
        assert!(addr_string.starts_with("botho-pq://1/"));
    }

    #[test]
    fn test_subaddresses_have_same_pq_keys() {
        let account = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC);

        let addr0 = account.subaddress(0);
        let addr1 = account.subaddress(1);
        let addr_change = account.change_subaddress();

        // PQ keys should be the same across subaddresses
        assert_eq!(
            addr0.pq_kem_public_key().as_bytes(),
            addr1.pq_kem_public_key().as_bytes()
        );
        assert_eq!(
            addr0.pq_sig_public_key().as_bytes(),
            addr_change.pq_sig_public_key().as_bytes()
        );

        // Classical keys should differ between subaddresses
        assert_ne!(
            addr0.view_public_key().to_bytes(),
            addr1.view_public_key().to_bytes()
        );
    }
}
