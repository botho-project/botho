// Copyright (c) 2024 Botho Foundation

//! Core types for the privacy/onion gossip module.
//!
//! This module defines foundational types used throughout the privacy layer:
//! - [`CircuitId`]: Unique identifier for relay circuits
//! - [`SymmetricKey`]: Secure symmetric key for hop encryption
//!
//! # Security
//!
//! All key material uses `zeroize` for secure memory handling to prevent
//! sensitive data from persisting in memory after use.

use rand::{CryptoRng, RngCore};
use serde::{Deserialize, Serialize};
use std::fmt;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Length of circuit identifiers in bytes.
pub const CIRCUIT_ID_LEN: usize = 16;

/// Length of symmetric keys in bytes (256-bit for ChaCha20-Poly1305).
pub const SYMMETRIC_KEY_LEN: usize = 32;

/// A unique identifier for a relay circuit.
///
/// Circuit IDs are 16-byte random values that identify a specific circuit
/// through the relay network. Each circuit has a unique ID that is used
/// to route messages through the correct hops.
///
/// # Example
///
/// ```
/// use botho::network::privacy::CircuitId;
///
/// // Generate a random circuit ID
/// let mut rng = rand::thread_rng();
/// let circuit_id = CircuitId::random(&mut rng);
///
/// // Convert to bytes for transmission
/// let bytes = circuit_id.as_bytes();
/// assert_eq!(bytes.len(), 16);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CircuitId([u8; CIRCUIT_ID_LEN]);

impl CircuitId {
    /// Generate a new random circuit ID.
    ///
    /// Uses the provided cryptographically secure random number generator
    /// to create a unique circuit identifier.
    pub fn random<R: RngCore + CryptoRng>(rng: &mut R) -> Self {
        let mut bytes = [0u8; CIRCUIT_ID_LEN];
        rng.fill_bytes(&mut bytes);
        Self(bytes)
    }

    /// Create a circuit ID from raw bytes.
    ///
    /// # Errors
    ///
    /// Returns `None` if the slice length is not exactly [`CIRCUIT_ID_LEN`].
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != CIRCUIT_ID_LEN {
            return None;
        }
        let mut arr = [0u8; CIRCUIT_ID_LEN];
        arr.copy_from_slice(bytes);
        Some(Self(arr))
    }

    /// Get the raw bytes of this circuit ID.
    #[inline]
    pub fn as_bytes(&self) -> &[u8; CIRCUIT_ID_LEN] {
        &self.0
    }
}

impl AsRef<[u8]> for CircuitId {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for CircuitId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CircuitId({})", hex::encode(&self.0[..4]))
    }
}

impl fmt::Display for CircuitId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", hex::encode(&self.0[..8]))
    }
}

/// A symmetric key for encrypting circuit hop data.
///
/// Uses ChaCha20-Poly1305 (256-bit key) for authenticated encryption.
/// The key is automatically zeroed from memory when dropped.
///
/// # Security
///
/// - Implements [`Zeroize`] and [`ZeroizeOnDrop`] for secure memory handling
/// - Debug output shows only a hash of the key, not the key itself
/// - Clone is intentionally not derived to prevent accidental key duplication
///
/// # Example
///
/// ```
/// use botho::network::privacy::SymmetricKey;
///
/// // Generate a random key
/// let mut rng = rand::thread_rng();
/// let key = SymmetricKey::random(&mut rng);
///
/// // Key is zeroed when dropped
/// drop(key);
/// ```
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct SymmetricKey([u8; SYMMETRIC_KEY_LEN]);

impl SymmetricKey {
    /// Generate a new random symmetric key.
    ///
    /// Uses the provided cryptographically secure random number generator.
    pub fn random<R: RngCore + CryptoRng>(rng: &mut R) -> Self {
        let mut bytes = [0u8; SYMMETRIC_KEY_LEN];
        rng.fill_bytes(&mut bytes);
        Self(bytes)
    }

    /// Create a symmetric key from raw bytes.
    ///
    /// # Errors
    ///
    /// Returns `None` if the slice length is not exactly [`SYMMETRIC_KEY_LEN`].
    ///
    /// # Security
    ///
    /// The input bytes are copied; the caller is responsible for zeroing
    /// the original source if needed.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != SYMMETRIC_KEY_LEN {
            return None;
        }
        let mut arr = [0u8; SYMMETRIC_KEY_LEN];
        arr.copy_from_slice(bytes);
        Some(Self(arr))
    }

    /// Get the raw bytes of this key.
    ///
    /// # Security
    ///
    /// Be careful with the returned reference - avoid copying or logging it.
    #[inline]
    pub fn as_bytes(&self) -> &[u8; SYMMETRIC_KEY_LEN] {
        &self.0
    }

    /// Create an explicit copy of this key.
    ///
    /// This method exists to make key copying explicit and intentional,
    /// rather than allowing implicit cloning.
    ///
    /// # Security
    ///
    /// Only use this when you genuinely need a second copy of the key.
    /// Both copies will be independently zeroed on drop.
    pub fn duplicate(&self) -> Self {
        Self(self.0)
    }
}

impl AsRef<[u8]> for SymmetricKey {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for SymmetricKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never log the actual key - show a hash fingerprint instead
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(&self.0);
        let hash = hasher.finalize();
        write!(f, "SymmetricKey(sha256:{})", hex::encode(&hash[..4]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_circuit_id_random_uniqueness() {
        let mut rng = rand::thread_rng();
        let id1 = CircuitId::random(&mut rng);
        let id2 = CircuitId::random(&mut rng);
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_circuit_id_from_bytes() {
        let bytes = [0x42u8; CIRCUIT_ID_LEN];
        let id = CircuitId::from_bytes(&bytes).unwrap();
        assert_eq!(id.as_bytes(), &bytes);
    }

    #[test]
    fn test_circuit_id_from_bytes_wrong_length() {
        let bytes = [0x42u8; 8];
        assert!(CircuitId::from_bytes(&bytes).is_none());
    }

    #[test]
    fn test_circuit_id_debug_format() {
        let id = CircuitId([0xDE, 0xAD, 0xBE, 0xEF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let debug = format!("{:?}", id);
        assert!(debug.contains("deadbeef"));
    }

    #[test]
    fn test_symmetric_key_random_uniqueness() {
        let mut rng = rand::thread_rng();
        let key1 = SymmetricKey::random(&mut rng);
        let key2 = SymmetricKey::random(&mut rng);
        assert_ne!(key1.as_bytes(), key2.as_bytes());
    }

    #[test]
    fn test_symmetric_key_from_bytes() {
        let bytes = [0x42u8; SYMMETRIC_KEY_LEN];
        let key = SymmetricKey::from_bytes(&bytes).unwrap();
        assert_eq!(key.as_bytes(), &bytes);
    }

    #[test]
    fn test_symmetric_key_from_bytes_wrong_length() {
        let bytes = [0x42u8; 16];
        assert!(SymmetricKey::from_bytes(&bytes).is_none());
    }

    #[test]
    fn test_symmetric_key_debug_does_not_leak() {
        let key = SymmetricKey([0x42u8; SYMMETRIC_KEY_LEN]);
        let debug = format!("{:?}", key);
        // Should not contain the actual key bytes
        assert!(!debug.contains("42424242"));
        // Should contain a hash fingerprint
        assert!(debug.contains("sha256:"));
    }

    #[test]
    fn test_symmetric_key_duplicate() {
        let key1 = SymmetricKey([0x42u8; SYMMETRIC_KEY_LEN]);
        let key2 = key1.duplicate();
        assert_eq!(key1.as_bytes(), key2.as_bytes());
    }

    #[test]
    fn test_circuit_id_hash_impl() {
        use std::collections::HashSet;

        let mut rng = rand::thread_rng();
        let mut set = HashSet::new();

        for _ in 0..100 {
            let id = CircuitId::random(&mut rng);
            set.insert(id);
        }

        assert_eq!(set.len(), 100);
    }
}
