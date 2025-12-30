// Copyright (c) 2024 The Botho Foundation

#![deny(unsafe_code)]

//! Secp256k1 key support for Ethereum compatibility.
//!
//! This crate provides Ethereum-compatible key derivation and signing using
//! the secp256k1 elliptic curve, following BIP-32/BIP-39/BIP-44 standards.
//!
//! # Examples
//!
//! ```
//! use bth_crypto_secp256k1::Secp256k1Keypair;
//!
//! // Derive from a BIP-39 mnemonic
//! let mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
//! let keypair = Secp256k1Keypair::from_mnemonic(mnemonic, "", 0).unwrap();
//!
//! // Get Ethereum address
//! let address = keypair.eth_address();
//! assert!(address.starts_with("0x"));
//!
//! // Sign a message (EIP-191 personal sign)
//! let signature = keypair.sign_message(b"Hello, Ethereum!");
//! assert_eq!(signature.len(), 65); // r (32) + s (32) + v (1)
//! ```

use hmac::{Hmac, Mac};
use k256::{
    ecdsa::{RecoveryId, Signature as K256Signature, SigningKey},
    elliptic_curve::bigint::{Encoding, Limb},
    SecretKey, U256,
};
use sha2::Sha512;
use sha3::{Digest, Keccak256};
use bip39::{Language, Mnemonic, Seed};
use zeroize::{Zeroize, ZeroizeOnDrop};

type HmacSha512 = Hmac<Sha512>;

/// Errors that can occur during key operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Invalid mnemonic phrase")]
    InvalidMnemonic,

    #[error("Key derivation failed: {0}")]
    DerivationError(String),

    #[error("Invalid private key")]
    InvalidPrivateKey,

    #[error("Signing failed: {0}")]
    SigningError(String),
}

/// BIP-44 path components for Ethereum
const ETH_PURPOSE: u32 = 44;
const ETH_COIN_TYPE: u32 = 60;

/// Hardened key offset
const HARDENED: u32 = 0x80000000;

/// A secp256k1 keypair for Ethereum-compatible operations.
#[derive(Clone, ZeroizeOnDrop)]
pub struct Secp256k1Keypair {
    #[zeroize(skip)] // SigningKey implements its own zeroization
    signing_key: SigningKey,
}

impl core::fmt::Debug for Secp256k1Keypair {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Secp256k1Keypair {{ address: {} }}", self.eth_address())
    }
}

impl Secp256k1Keypair {
    /// Create a keypair from a BIP-39 mnemonic phrase.
    ///
    /// Uses the standard Ethereum derivation path: m/44'/60'/0'/0/{index}
    pub fn from_mnemonic(mnemonic: &str, password: &str, index: u32) -> Result<Self, Error> {
        let mnemonic =
            Mnemonic::from_phrase(mnemonic, Language::English).map_err(|_| Error::InvalidMnemonic)?;

        let seed = Seed::new(&mnemonic, password);
        Self::from_seed(seed.as_bytes(), index)
    }

    /// Create a keypair from a 64-byte BIP-39 seed.
    pub fn from_seed(seed: &[u8], index: u32) -> Result<Self, Error> {
        // Derive master key from seed
        let mut mac =
            HmacSha512::new_from_slice(b"Bitcoin seed").expect("HMAC can take any size key");
        mac.update(seed);
        let result = mac.finalize().into_bytes();

        let mut key = [0u8; 32];
        let mut chain_code = [0u8; 32];
        key.copy_from_slice(&result[..32]);
        chain_code.copy_from_slice(&result[32..]);

        // Derive path: m/44'/60'/0'/0/{index}
        // Note: ' means hardened (add 0x80000000)
        let path = [
            ETH_PURPOSE | HARDENED,    // 44'
            ETH_COIN_TYPE | HARDENED,  // 60'
            HARDENED,                  // 0'
            0,                         // 0 (not hardened)
            index,                     // index (not hardened)
        ];

        for &child_index in &path {
            let (new_key, new_chain) =
                derive_child(&key, &chain_code, child_index).map_err(Error::DerivationError)?;
            key = new_key;
            chain_code = new_chain;
        }

        let secret_key =
            SecretKey::from_bytes((&key).into()).map_err(|_| Error::InvalidPrivateKey)?;

        key.zeroize();
        chain_code.zeroize();

        Ok(Self {
            signing_key: SigningKey::from(secret_key),
        })
    }

    /// Create a keypair from raw 32-byte private key bytes.
    pub fn from_bytes(bytes: &[u8; 32]) -> Result<Self, Error> {
        let secret_key =
            SecretKey::from_bytes(bytes.into()).map_err(|_| Error::InvalidPrivateKey)?;

        Ok(Self {
            signing_key: SigningKey::from(secret_key),
        })
    }

    /// Get the public key as uncompressed bytes (65 bytes: 0x04 || x || y).
    pub fn public_key_uncompressed(&self) -> [u8; 65] {
        let verifying_key = self.signing_key.verifying_key();
        let point = verifying_key.to_encoded_point(false);
        let mut result = [0u8; 65];
        result.copy_from_slice(point.as_bytes());
        result
    }

    /// Get the public key as compressed bytes (33 bytes: 0x02/0x03 || x).
    pub fn public_key_compressed(&self) -> [u8; 33] {
        let verifying_key = self.signing_key.verifying_key();
        let point = verifying_key.to_encoded_point(true);
        let mut result = [0u8; 33];
        result.copy_from_slice(point.as_bytes());
        result
    }

    /// Get the Ethereum address derived from this keypair.
    ///
    /// The address is computed as the last 20 bytes of keccak256(public_key),
    /// where public_key is the 64-byte uncompressed public key (without the 0x04 prefix).
    ///
    /// Returns a checksummed address string prefixed with "0x".
    pub fn eth_address(&self) -> String {
        let pubkey = self.public_key_uncompressed();
        // Skip the 0x04 prefix, hash the 64 bytes of x || y
        let hash = Keccak256::digest(&pubkey[1..]);
        // Take last 20 bytes
        let address_bytes: [u8; 20] = hash[12..32].try_into().unwrap();

        // EIP-55 checksum encoding
        Self::checksum_encode(&address_bytes)
    }

    /// Get the raw 20-byte Ethereum address.
    pub fn eth_address_bytes(&self) -> [u8; 20] {
        let pubkey = self.public_key_uncompressed();
        let hash = Keccak256::digest(&pubkey[1..]);
        hash[12..32].try_into().unwrap()
    }

    /// Sign a message using EIP-191 personal sign format.
    ///
    /// The message is prefixed with "\x19Ethereum Signed Message:\n{length}"
    /// before hashing and signing.
    ///
    /// Returns a 65-byte signature: r (32) || s (32) || v (1)
    /// where v is the recovery ID + 27.
    pub fn sign_message(&self, message: &[u8]) -> [u8; 65] {
        let prefixed = Self::eip191_hash(message);
        self.sign_hash(&prefixed)
    }

    /// Sign a raw 32-byte hash.
    ///
    /// Returns a 65-byte signature: r (32) || s (32) || v (1)
    /// where v is the recovery ID + 27.
    pub fn sign_hash(&self, hash: &[u8; 32]) -> [u8; 65] {
        let (signature, recovery_id) = self
            .signing_key
            .sign_prehash_recoverable(hash)
            .expect("signing should not fail with valid key");

        let mut result = [0u8; 65];
        result[..64].copy_from_slice(&signature.to_bytes());
        result[64] = recovery_id.to_byte() + 27;
        result
    }

    /// Sign a transaction hash for Ethereum.
    pub fn sign_transaction_hash(&self, tx_hash: &[u8; 32]) -> [u8; 65] {
        self.sign_hash(tx_hash)
    }

    /// Compute the EIP-191 personal sign hash for a message.
    fn eip191_hash(message: &[u8]) -> [u8; 32] {
        let prefix = format!("\x19Ethereum Signed Message:\n{}", message.len());
        let mut hasher = Keccak256::new();
        hasher.update(prefix.as_bytes());
        hasher.update(message);
        hasher.finalize().into()
    }

    /// EIP-55 checksum encode an address.
    fn checksum_encode(address: &[u8; 20]) -> String {
        let hex_addr = hex::encode(address);
        let hash = Keccak256::digest(hex_addr.as_bytes());

        let mut result = String::with_capacity(42);
        result.push_str("0x");

        for (i, c) in hex_addr.chars().enumerate() {
            if c.is_ascii_digit() {
                result.push(c);
            } else {
                // Get the corresponding nibble from the hash
                let hash_byte = hash[i / 2];
                let hash_nibble = if i % 2 == 0 {
                    hash_byte >> 4
                } else {
                    hash_byte & 0x0f
                };

                if hash_nibble >= 8 {
                    result.push(c.to_ascii_uppercase());
                } else {
                    result.push(c);
                }
            }
        }

        result
    }
}

/// Derive a child key from a parent key and chain code.
fn derive_child(
    parent_key: &[u8; 32],
    parent_chain: &[u8; 32],
    index: u32,
) -> Result<([u8; 32], [u8; 32]), String> {
    let mut mac =
        HmacSha512::new_from_slice(parent_chain).expect("HMAC can take any size key");

    if index >= HARDENED {
        // Hardened derivation: use 0x00 || parent_key || index
        mac.update(&[0x00]);
        mac.update(parent_key);
    } else {
        // Normal derivation: use compressed public key || index
        let secret = SecretKey::from_bytes(parent_key.into())
            .map_err(|_| "Invalid parent key".to_string())?;
        let signing = SigningKey::from(secret);
        let pubkey = signing.verifying_key().to_encoded_point(true);
        mac.update(pubkey.as_bytes());
    }

    mac.update(&index.to_be_bytes());
    let result = mac.finalize().into_bytes();

    // Parse the derived key
    let mut derived_key = [0u8; 32];
    derived_key.copy_from_slice(&result[..32]);

    // Add parent key to derived key (mod n) using U256 arithmetic
    let parent_u256 = U256::from_be_slice(parent_key);
    let derived_u256 = U256::from_be_slice(&derived_key);

    // secp256k1 curve order
    let n = U256::from_be_hex("FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141");

    // Add and reduce mod n
    let (sum, overflow) = parent_u256.adc(&derived_u256, Limb::ZERO);
    let new_key_u256 = if overflow.0 != 0 || sum >= n {
        sum.wrapping_sub(&n)
    } else {
        sum
    };

    let new_key: [u8; 32] = new_key_u256.to_be_bytes();

    let mut new_chain = [0u8; 32];
    new_chain.copy_from_slice(&result[32..]);

    Ok((new_key, new_chain))
}

/// Recover the public key from a signature and message hash.
pub fn recover_public_key(hash: &[u8; 32], signature: &[u8; 65]) -> Option<[u8; 65]> {
    use k256::ecdsa::VerifyingKey;

    let r_s: [u8; 64] = signature[..64].try_into().ok()?;
    let v = signature[64];

    // v should be 27 or 28 (or 0/1 for some implementations)
    let recovery_id = if v >= 27 {
        RecoveryId::try_from(v - 27).ok()?
    } else {
        RecoveryId::try_from(v).ok()?
    };

    let sig = K256Signature::from_slice(&r_s).ok()?;
    let verifying_key = VerifyingKey::recover_from_prehash(hash, &sig, recovery_id).ok()?;

    let point = verifying_key.to_encoded_point(false);
    let mut result = [0u8; 65];
    result.copy_from_slice(point.as_bytes());
    Some(result)
}

/// Recover the Ethereum address from a signature and message.
pub fn recover_address(message: &[u8], signature: &[u8; 65]) -> Option<[u8; 20]> {
    let hash = Secp256k1Keypair::eip191_hash(message);
    let pubkey = recover_public_key(&hash, signature)?;

    let addr_hash = Keccak256::digest(&pubkey[1..]);
    Some(addr_hash[12..32].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Standard test mnemonic (DO NOT USE IN PRODUCTION)
    const TEST_MNEMONIC: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    #[test]
    fn test_mnemonic_derivation() {
        let keypair = Secp256k1Keypair::from_mnemonic(TEST_MNEMONIC, "", 0).unwrap();
        let address = keypair.eth_address();

        // This is the known address for the test mnemonic at index 0
        // Verified against MetaMask and other wallets
        assert_eq!(
            address.to_lowercase(),
            "0x9858effd232b4033e47d90003d41ec34ecaeda94"
        );
    }

    #[test]
    fn test_different_indices() {
        let keypair0 = Secp256k1Keypair::from_mnemonic(TEST_MNEMONIC, "", 0).unwrap();
        let keypair1 = Secp256k1Keypair::from_mnemonic(TEST_MNEMONIC, "", 1).unwrap();

        // Different indices should produce different addresses
        assert_ne!(keypair0.eth_address(), keypair1.eth_address());
    }

    #[test]
    fn test_sign_and_recover() {
        let keypair = Secp256k1Keypair::from_mnemonic(TEST_MNEMONIC, "", 0).unwrap();
        let message = b"Hello, Ethereum!";

        let signature = keypair.sign_message(message);
        let recovered = recover_address(message, &signature).unwrap();

        assert_eq!(recovered, keypair.eth_address_bytes());
    }

    #[test]
    fn test_checksum_address() {
        let keypair = Secp256k1Keypair::from_mnemonic(TEST_MNEMONIC, "", 0).unwrap();
        let address = keypair.eth_address();

        // Should be properly checksummed
        assert!(address.starts_with("0x"));
        assert_eq!(address.len(), 42);
    }

    #[test]
    fn test_public_key_formats() {
        let keypair = Secp256k1Keypair::from_mnemonic(TEST_MNEMONIC, "", 0).unwrap();

        let uncompressed = keypair.public_key_uncompressed();
        assert_eq!(uncompressed[0], 0x04); // Uncompressed prefix
        assert_eq!(uncompressed.len(), 65);

        let compressed = keypair.public_key_compressed();
        assert!(compressed[0] == 0x02 || compressed[0] == 0x03); // Compressed prefix
        assert_eq!(compressed.len(), 33);
    }
}
