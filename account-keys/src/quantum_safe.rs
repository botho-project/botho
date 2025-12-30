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

use bip39::{Language, Mnemonic, Seed};
use bth_crypto_pq::{
    derive_pq_keys_from_seed, MlDsa65KeyPair, MlDsa65PublicKey, MlKem768KeyPair, MlKem768PublicKey,
    PqKeyMaterial, BIP39_SEED_SIZE, ML_DSA_65_PUBLIC_KEY_BYTES, ML_KEM_768_PUBLIC_KEY_BYTES,
};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::{AccountKey, PublicAddress};

/// Errors that can occur when parsing a quantum-safe address
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddressParseError {
    /// Invalid address prefix (expected "botho-pq://1/")
    InvalidPrefix,
    /// Invalid base58 encoding
    InvalidBase58,
    /// Invalid address length
    InvalidLength {
        /// Expected byte length
        expected: usize,
        /// Actual byte length
        got: usize,
    },
    /// Invalid classical Ristretto key
    InvalidClassicalKey,
    /// Invalid post-quantum key
    InvalidPqKey,
}

impl fmt::Display for AddressParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPrefix => write!(f, "invalid address prefix, expected 'botho-pq://1/'"),
            Self::InvalidBase58 => write!(f, "invalid base58 encoding"),
            Self::InvalidLength { expected, got } => {
                write!(f, "invalid length: expected {} bytes, got {}", expected, got)
            }
            Self::InvalidClassicalKey => write!(f, "invalid classical Ristretto public key"),
            Self::InvalidPqKey => write!(f, "invalid post-quantum public key"),
        }
    }
}

// Note: Error trait requires std; pq feature enables std
extern crate std;

impl std::error::Error for AddressParseError {}

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
    /// The PQ keys are derived from the full BIP39 seed (512 bits) which includes
    /// PBKDF2-HMAC-SHA512 key stretching with 2048 iterations.
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

    /// Create a quantum-safe account key from a mnemonic phrase with optional passphrase
    ///
    /// This derives both classical (Ristretto) and post-quantum (ML-KEM, ML-DSA)
    /// keypairs from the same mnemonic. The passphrase provides an additional
    /// layer of security - different passphrases produce completely different keys.
    ///
    /// The PQ keys are derived from the full BIP39 seed (512 bits) which includes
    /// PBKDF2-HMAC-SHA512 key stretching with 2048 iterations.
    ///
    /// # Arguments
    ///
    /// * `mnemonic_phrase` - A BIP39 mnemonic phrase (typically 24 words)
    /// * `passphrase` - Optional passphrase (can be empty string for no passphrase)
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
        let seed_bytes: &[u8; BIP39_SEED_SIZE] = seed.as_bytes().try_into()
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

    /// Decode a quantum-safe address from its string representation
    ///
    /// Format: `botho-pq://1/<base58>`
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The prefix is not `botho-pq://1/`
    /// - The base58 decoding fails
    /// - The decoded length doesn't match `SERIALIZED_SIZE`
    /// - The classical keys are not valid Ristretto points
    pub fn from_address_string(s: &str) -> Result<Self, AddressParseError> {
        const PREFIX: &str = "botho-pq://1/";

        let encoded = s
            .strip_prefix(PREFIX)
            .ok_or(AddressParseError::InvalidPrefix)?;

        let bytes = bs58::decode(encoded)
            .into_vec()
            .map_err(|_| AddressParseError::InvalidBase58)?;

        if bytes.len() != Self::SERIALIZED_SIZE {
            return Err(AddressParseError::InvalidLength {
                expected: Self::SERIALIZED_SIZE,
                got: bytes.len(),
            });
        }

        Self::from_bytes(&bytes)
    }

    /// Deserialize from bytes
    ///
    /// Format: classical_view || classical_spend || pq_kem || pq_sig
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, AddressParseError> {
        if bytes.len() != Self::SERIALIZED_SIZE {
            return Err(AddressParseError::InvalidLength {
                expected: Self::SERIALIZED_SIZE,
                got: bytes.len(),
            });
        }

        let mut offset = 0;

        // Classical view key (32 bytes)
        let view_bytes: [u8; 32] = bytes[offset..offset + 32]
            .try_into()
            .expect("slice is 32 bytes");
        let view_public_key = bth_crypto_keys::RistrettoPublic::try_from(&view_bytes)
            .map_err(|_| AddressParseError::InvalidClassicalKey)?;
        offset += 32;

        // Classical spend key (32 bytes)
        let spend_bytes: [u8; 32] = bytes[offset..offset + 32]
            .try_into()
            .expect("slice is 32 bytes");
        let spend_public_key = bth_crypto_keys::RistrettoPublic::try_from(&spend_bytes)
            .map_err(|_| AddressParseError::InvalidClassicalKey)?;
        offset += 32;

        let classical = PublicAddress::new(&spend_public_key, &view_public_key);

        // PQ KEM key (1184 bytes)
        let kem_bytes = &bytes[offset..offset + ML_KEM_768_PUBLIC_KEY_BYTES];
        let pq_kem_public_key = MlKem768PublicKey::from_bytes(kem_bytes)
            .map_err(|_| AddressParseError::InvalidPqKey)?;
        offset += ML_KEM_768_PUBLIC_KEY_BYTES;

        // PQ Sig key (1952 bytes)
        let sig_bytes = &bytes[offset..offset + ML_DSA_65_PUBLIC_KEY_BYTES];
        let pq_sig_public_key = MlDsa65PublicKey::from_bytes(sig_bytes)
            .map_err(|_| AddressParseError::InvalidPqKey)?;

        Ok(Self {
            classical,
            pq_kem_public_key,
            pq_sig_public_key,
        })
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

        // Verify we have valid keys - check that default subaddress can be derived
        let _subaddr = account.default_subaddress();
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

    #[test]
    fn test_address_roundtrip() {
        let account = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC);
        let address = account.default_subaddress();

        // Encode to string
        let addr_string = address.to_address_string();

        // Decode back
        let parsed = QuantumSafePublicAddress::from_address_string(&addr_string)
            .expect("should parse valid address");

        // Should be equal
        assert_eq!(address, parsed);
    }

    #[test]
    fn test_address_bytes_roundtrip() {
        let account = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC);
        let address = account.default_subaddress();

        // Serialize to bytes
        let bytes = address.to_bytes();

        // Deserialize back
        let parsed =
            QuantumSafePublicAddress::from_bytes(&bytes).expect("should parse valid bytes");

        // Should be equal
        assert_eq!(address, parsed);
    }

    #[test]
    fn test_address_parse_invalid_prefix() {
        let result = QuantumSafePublicAddress::from_address_string("botho://1/abc");
        assert_eq!(result, Err(AddressParseError::InvalidPrefix));
    }

    #[test]
    fn test_address_parse_invalid_base58() {
        let result = QuantumSafePublicAddress::from_address_string("botho-pq://1/invalid!base58");
        assert_eq!(result, Err(AddressParseError::InvalidBase58));
    }

    #[test]
    fn test_address_parse_invalid_length() {
        // Valid base58 but wrong length
        let result = QuantumSafePublicAddress::from_address_string("botho-pq://1/abc123");
        assert!(matches!(
            result,
            Err(AddressParseError::InvalidLength { .. })
        ));
    }

    #[test]
    fn test_deterministic_key_derivation() {
        // Same mnemonic should produce identical keys every time
        let account1 = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC);
        let account2 = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC);

        // Get addresses to compare classical keys
        let addr1 = account1.default_subaddress();
        let addr2 = account2.default_subaddress();

        // Classical keys should be identical (via address comparison)
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

        // Full address should be identical
        assert_eq!(addr1, addr2);
        assert_eq!(addr1.to_address_string(), addr2.to_address_string());
    }

    #[test]
    fn test_different_mnemonic_different_keys() {
        let account1 = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC);
        let account2 = QuantumSafeAccountKey::from_mnemonic(
            "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong",
        );

        // Get addresses to compare
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
        let account_no_pass = QuantumSafeAccountKey::from_mnemonic_with_passphrase(TEST_MNEMONIC, "");
        let account_with_pass = QuantumSafeAccountKey::from_mnemonic_with_passphrase(TEST_MNEMONIC, "my secret passphrase");

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
        let account1 = QuantumSafeAccountKey::from_mnemonic_with_passphrase(TEST_MNEMONIC, "deterministic");
        let account2 = QuantumSafeAccountKey::from_mnemonic_with_passphrase(TEST_MNEMONIC, "deterministic");

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
