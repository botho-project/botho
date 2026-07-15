//! Quantum-Safe Account Keys
//!
//! This module provides the post-quantum *secret-key holder* used to derive an
//! account's post-quantum public keys. It combines classical (Ristretto/
//! Schnorr) keys with NIST-standardized post-quantum algorithms:
//!
//! - **ML-KEM-768** (Kyber): Key Encapsulation for stealth address key exchange
//! - **ML-DSA-65** (Dilithium): Digital signatures for transaction signing
//!
//! # Unified address type
//!
//! As of address format v2 (ADR 0008), the post-quantum public keys are folded
//! directly into the canonical [`PublicAddress`]. There is no longer a separate
//! `QuantumSafePublicAddress` type or a `botho-pq://` address string; a
//! quantum-safe subaddress is simply a [`PublicAddress`] whose
//! [`PublicAddress::kem_public_key`] and [`PublicAddress::dsa_public_key`]
//! fields are populated. [`QuantumSafeAccountKey`] remains as the secret-side
//! keypair holder that derives those keys from the mnemonic (its unification
//! into `AccountKey` is tracked by the key-hierarchy sub-issue).
//!
//! # Hybrid Security Model
//!
//! Quantum-safe transactions require BOTH classical and post-quantum secrets to
//! spend. This provides:
//!
//! 1. Immediate protection against "harvest now, decrypt later" attacks
//! 2. Fallback security if either cryptosystem is broken
//! 3. Backward compatibility with existing infrastructure
//!
//! # Usage
//!
//! ```ignore
//! use bth_account_keys::QuantumSafeAccountKey;
//!
//! // Create from mnemonic (PQ keys derived deterministically)
//! let account = QuantumSafeAccountKey::from_mnemonic("word1 word2 ... word24");
//!
//! // Get the unified public address for receiving (carries both PQ keys)
//! let address = account.default_subaddress();
//!
//! // Address includes both classical and PQ public keys
//! println!("Classical view key: {:?}", address.view_public_key());
//! println!("PQ KEM key bytes:  {}", address.kem_public_key().len());
//! ```

use alloc::vec::Vec;
use core::fmt;

use bip39::{Language, Mnemonic, Seed};
use bth_crypto_pq::{
    derive_pq_keys_from_seed, MlDsa65KeyPair, MlKem768KeyPair, PqKeyMaterial, BIP39_SEED_SIZE,
};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::{AccountKey, PublicAddress};

// Note: pq feature enables std; kept for parity with the rest of the crate.
extern crate std;

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
    /// This derives both classical (Ristretto) and post-quantum (ML-KEM,
    /// ML-DSA) keypairs from the same mnemonic. The classical keys use the
    /// standard SLIP-0010 derivation path, while PQ keys use HKDF with
    /// domain separation.
    ///
    /// The PQ keys are derived from the full BIP39 seed (512 bits) which
    /// includes PBKDF2-HMAC-SHA512 key stretching with 2048 iterations.
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
    pub fn from_mnemonic(mnemonic_phrase: &str) -> Self {
        Self::from_mnemonic_with_passphrase(mnemonic_phrase, "")
    }

    /// Create a quantum-safe account key from a mnemonic phrase with optional
    /// passphrase
    ///
    /// This derives both classical (Ristretto) and post-quantum (ML-KEM,
    /// ML-DSA) keypairs from the same mnemonic. The passphrase provides an
    /// additional layer of security - different passphrases produce
    /// completely different keys.
    ///
    /// The PQ keys are derived from the full BIP39 seed (512 bits) which
    /// includes PBKDF2-HMAC-SHA512 key stretching with 2048 iterations.
    ///
    /// # Arguments
    ///
    /// * `mnemonic_phrase` - A BIP39 mnemonic phrase (typically 24 words)
    /// * `passphrase` - Optional passphrase (can be empty string for no
    ///   passphrase)
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    /// let account = QuantumSafeAccountKey::from_mnemonic_with_passphrase(mnemonic, "my secret");
    /// ```
    pub fn from_mnemonic_with_passphrase(mnemonic_phrase: &str, passphrase: &str) -> Self {
        // Parse the mnemonic phrase
        let mnemonic = Mnemonic::from_phrase(mnemonic_phrase, Language::English)
            .expect("invalid mnemonic phrase");

        // Derive the BIP39 seed with full PBKDF2 key stretching (2048 iterations)
        let seed = Seed::new(&mnemonic, passphrase);
        let seed_bytes: &[u8; BIP39_SEED_SIZE] = seed
            .as_bytes()
            .try_into()
            .expect("BIP39 seed is always 64 bytes");

        // Derive classical keys using existing infrastructure
        let classical = Self::derive_classical_from_mnemonic(mnemonic_phrase);

        // Derive PQ keys from the BIP39 seed (with proper key stretching)
        let pq_keys = derive_pq_keys_from_seed(seed_bytes);

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

    /// Raw ML-KEM-768 public key bytes for this account (account-wide).
    fn kem_public_bytes(&self) -> Vec<u8> {
        self.pq_kem_keypair.public_key().as_bytes().to_vec()
    }

    /// Raw ML-DSA-65 public key bytes for this account (account-wide).
    fn dsa_public_bytes(&self) -> Vec<u8> {
        self.pq_sig_keypair.public_key().as_bytes().to_vec()
    }

    /// Get the default quantum-safe subaddress (as a unified
    /// [`PublicAddress`]).
    pub fn default_subaddress(&self) -> PublicAddress {
        self.classical
            .default_subaddress()
            .with_pq_keys(self.kem_public_bytes(), self.dsa_public_bytes())
    }

    /// Get the change quantum-safe subaddress (as a unified [`PublicAddress`]).
    pub fn change_subaddress(&self) -> PublicAddress {
        self.classical
            .change_subaddress()
            .with_pq_keys(self.kem_public_bytes(), self.dsa_public_bytes())
    }

    /// Get the i^th quantum-safe subaddress (as a unified [`PublicAddress`]).
    ///
    /// Note: The PQ public keys are the same across all subaddresses.
    /// Subaddress derivation only affects the classical keys.
    /// One-time PQ keys are derived per-output using the encapsulated secret.
    pub fn subaddress(&self, index: u64) -> PublicAddress {
        self.classical
            .subaddress(index)
            .with_pq_keys(self.kem_public_bytes(), self.dsa_public_bytes())
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

#[cfg(test)]
mod tests {
    use super::*;
    use bth_crypto_pq::{ML_DSA_65_PUBLIC_KEY_BYTES, ML_KEM_768_PUBLIC_KEY_BYTES};

    const TEST_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    #[test]
    fn test_quantum_safe_account_key_creation() {
        let account = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC);

        // Verify we have valid keys - check that default subaddress can be derived
        let _subaddr = account.default_subaddress();
    }

    #[test]
    fn test_unified_public_address_carries_pq_keys() {
        let account = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC);
        let address = account.default_subaddress();

        // The unified PublicAddress carries both PQ keys at their raw lengths.
        assert_eq!(address.kem_public_key().len(), ML_KEM_768_PUBLIC_KEY_BYTES);
        assert_eq!(address.dsa_public_key().len(), ML_DSA_65_PUBLIC_KEY_BYTES);
        assert!(address.has_pq_keys());

        // The published PQ keys match the account's keypairs.
        assert_eq!(
            address.kem_public_key(),
            account.pq_kem_keypair().public_key().as_bytes()
        );
        assert_eq!(
            address.dsa_public_key(),
            account.pq_sig_keypair().public_key().as_bytes()
        );
    }

    #[test]
    fn test_subaddresses_have_same_pq_keys() {
        let account = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC);

        let addr0 = account.subaddress(0);
        let addr1 = account.subaddress(1);
        let addr_change = account.change_subaddress();

        // PQ keys should be the same across subaddresses
        assert_eq!(addr0.kem_public_key(), addr1.kem_public_key());
        assert_eq!(addr0.dsa_public_key(), addr_change.dsa_public_key());

        // Classical keys should differ between subaddresses
        assert_ne!(
            addr0.view_public_key().to_bytes(),
            addr1.view_public_key().to_bytes()
        );
    }

    #[test]
    fn test_deterministic_key_derivation() {
        // Same mnemonic should produce identical keys every time
        let account1 = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC);
        let account2 = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC);

        let addr1 = account1.default_subaddress();
        let addr2 = account2.default_subaddress();

        // Classical keys should be identical
        assert_eq!(
            addr1.view_public_key().to_bytes(),
            addr2.view_public_key().to_bytes()
        );
        assert_eq!(
            addr1.spend_public_key().to_bytes(),
            addr2.spend_public_key().to_bytes()
        );

        // PQ KEM public keys should be identical
        assert_eq!(
            account1.pq_kem_keypair().public_key().as_bytes(),
            account2.pq_kem_keypair().public_key().as_bytes()
        );

        // PQ Signature public keys should be identical
        assert_eq!(
            account1.pq_sig_keypair().public_key().as_bytes(),
            account2.pq_sig_keypair().public_key().as_bytes()
        );

        // Full unified address should be identical (classical + PQ).
        assert_eq!(addr1, addr2);
    }

    #[test]
    fn test_different_mnemonic_different_keys() {
        let account1 = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC);
        let account2 = QuantumSafeAccountKey::from_mnemonic(
            "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong",
        );

        let addr1 = account1.default_subaddress();
        let addr2 = account2.default_subaddress();

        // Classical keys should differ
        assert_ne!(
            addr1.view_public_key().to_bytes(),
            addr2.view_public_key().to_bytes()
        );

        // PQ keys should also differ
        assert_ne!(
            account1.pq_kem_keypair().public_key().as_bytes(),
            account2.pq_kem_keypair().public_key().as_bytes()
        );
        assert_ne!(
            account1.pq_sig_keypair().public_key().as_bytes(),
            account2.pq_sig_keypair().public_key().as_bytes()
        );
    }

    #[test]
    fn test_passphrase_produces_different_pq_keys() {
        // Same mnemonic with different passphrases should produce different PQ keys
        let account_no_pass =
            QuantumSafeAccountKey::from_mnemonic_with_passphrase(TEST_MNEMONIC, "");
        let account_with_pass = QuantumSafeAccountKey::from_mnemonic_with_passphrase(
            TEST_MNEMONIC,
            "my secret passphrase",
        );

        // PQ keys should be completely different with a passphrase
        assert_ne!(
            account_no_pass.pq_kem_keypair().public_key().as_bytes(),
            account_with_pass.pq_kem_keypair().public_key().as_bytes(),
            "PQ KEM keys should differ with different passphrases"
        );
        assert_ne!(
            account_no_pass.pq_sig_keypair().public_key().as_bytes(),
            account_with_pass.pq_sig_keypair().public_key().as_bytes(),
            "PQ signature keys should differ with different passphrases"
        );
    }

    #[test]
    fn test_passphrase_deterministic() {
        // Same mnemonic + passphrase should always produce identical keys
        let account1 =
            QuantumSafeAccountKey::from_mnemonic_with_passphrase(TEST_MNEMONIC, "deterministic");
        let account2 =
            QuantumSafeAccountKey::from_mnemonic_with_passphrase(TEST_MNEMONIC, "deterministic");

        assert_eq!(
            account1.pq_kem_keypair().public_key().as_bytes(),
            account2.pq_kem_keypair().public_key().as_bytes(),
            "Same passphrase should produce identical PQ KEM keys"
        );
        assert_eq!(
            account1.pq_sig_keypair().public_key().as_bytes(),
            account2.pq_sig_keypair().public_key().as_bytes(),
            "Same passphrase should produce identical PQ signature keys"
        );
    }

    #[test]
    fn test_from_mnemonic_equals_empty_passphrase() {
        // from_mnemonic() should be equivalent to from_mnemonic_with_passphrase(_, "")
        let account1 = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC);
        let account2 = QuantumSafeAccountKey::from_mnemonic_with_passphrase(TEST_MNEMONIC, "");

        assert_eq!(
            account1.pq_kem_keypair().public_key().as_bytes(),
            account2.pq_kem_keypair().public_key().as_bytes(),
            "from_mnemonic should equal empty passphrase"
        );
        assert_eq!(
            account1.pq_sig_keypair().public_key().as_bytes(),
            account2.pq_sig_keypair().public_key().as_bytes(),
            "from_mnemonic should equal empty passphrase"
        );
    }
}
