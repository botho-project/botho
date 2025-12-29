//! ML-KEM-768 (Kyber) Key Encapsulation Mechanism
//!
//! ML-KEM is a lattice-based key encapsulation mechanism standardized by NIST
//! in FIPS 203. It provides IND-CCA2 security against both classical and
//! quantum adversaries.
//!
//! We use the ML-KEM-768 parameter set which provides approximately 192-bit
//! security against classical attacks and ~128-bit security against quantum.
//!
//! # Usage in Botho
//!
//! ML-KEM is used for quantum-safe stealth address key derivation:
//!
//! 1. Recipient publishes their ML-KEM public key as part of their address
//! 2. Sender encapsulates a shared secret to this public key
//! 3. The ciphertext is included in the transaction output
//! 4. Recipient decapsulates to recover the shared secret
//! 5. Shared secret is used to derive the one-time output keys

use crate::error::PqError;
use pqcrypto_kyber::kyber768;
use pqcrypto_traits::kem::{Ciphertext as _, PublicKey as _, SecretKey as _, SharedSecret as _};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// ML-KEM-768 public key size in bytes
pub const ML_KEM_768_PUBLIC_KEY_BYTES: usize = 1184;

/// ML-KEM-768 secret key size in bytes
pub const ML_KEM_768_SECRET_KEY_BYTES: usize = 2400;

/// ML-KEM-768 ciphertext size in bytes
pub const ML_KEM_768_CIPHERTEXT_BYTES: usize = 1088;

/// ML-KEM-768 shared secret size in bytes
pub const ML_KEM_768_SHARED_SECRET_BYTES: usize = 32;

/// ML-KEM-768 public key for key encapsulation
#[derive(Clone, PartialEq, Eq)]
pub struct MlKem768PublicKey {
    bytes: [u8; ML_KEM_768_PUBLIC_KEY_BYTES],
}

impl Serialize for MlKem768PublicKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(&self.bytes)
    }
}

impl<'de> Deserialize<'de> for MlKem768PublicKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let bytes: Vec<u8> = Deserialize::deserialize(deserializer)?;
        Self::from_bytes(&bytes).map_err(serde::de::Error::custom)
    }
}

impl MlKem768PublicKey {
    /// Create from raw bytes
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, PqError> {
        if bytes.len() != ML_KEM_768_PUBLIC_KEY_BYTES {
            return Err(PqError::InvalidPublicKey(format!(
                "expected {} bytes, got {}",
                ML_KEM_768_PUBLIC_KEY_BYTES,
                bytes.len()
            )));
        }
        let mut arr = [0u8; ML_KEM_768_PUBLIC_KEY_BYTES];
        arr.copy_from_slice(bytes);
        Ok(Self { bytes: arr })
    }

    /// Get the raw bytes
    pub fn as_bytes(&self) -> &[u8; ML_KEM_768_PUBLIC_KEY_BYTES] {
        &self.bytes
    }

    /// Encapsulate a shared secret to this public key
    ///
    /// Returns (ciphertext, shared_secret) where:
    /// - ciphertext should be included in the transaction output
    /// - shared_secret is used to derive one-time keys
    pub fn encapsulate(&self) -> (MlKem768Ciphertext, MlKem768SharedSecret) {
        let pk = kyber768::PublicKey::from_bytes(&self.bytes)
            .expect("Invalid public key bytes in encapsulate");

        let (ss, ct) = kyber768::encapsulate(&pk);

        let mut ct_bytes = [0u8; ML_KEM_768_CIPHERTEXT_BYTES];
        ct_bytes.copy_from_slice(ct.as_bytes());

        let mut ss_bytes = [0u8; ML_KEM_768_SHARED_SECRET_BYTES];
        ss_bytes.copy_from_slice(ss.as_bytes());

        (
            MlKem768Ciphertext { bytes: ct_bytes },
            MlKem768SharedSecret { bytes: ss_bytes },
        )
    }
}

impl std::fmt::Debug for MlKem768PublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MlKem768PublicKey({:02x?}...)", &self.bytes[..8])
    }
}

/// ML-KEM-768 secret key for decapsulation
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct MlKem768SecretKey {
    bytes: [u8; ML_KEM_768_SECRET_KEY_BYTES],
}

impl MlKem768SecretKey {
    /// Create from raw bytes
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, PqError> {
        if bytes.len() != ML_KEM_768_SECRET_KEY_BYTES {
            return Err(PqError::InvalidSecretKey(format!(
                "expected {} bytes, got {}",
                ML_KEM_768_SECRET_KEY_BYTES,
                bytes.len()
            )));
        }
        let mut arr = [0u8; ML_KEM_768_SECRET_KEY_BYTES];
        arr.copy_from_slice(bytes);
        Ok(Self { bytes: arr })
    }

    /// Get the raw bytes (be careful with secret material!)
    pub fn as_bytes(&self) -> &[u8; ML_KEM_768_SECRET_KEY_BYTES] {
        &self.bytes
    }
}

impl std::fmt::Debug for MlKem768SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MlKem768SecretKey([REDACTED])")
    }
}

/// ML-KEM-768 ciphertext (encapsulated shared secret)
#[derive(Clone, PartialEq, Eq)]
pub struct MlKem768Ciphertext {
    bytes: [u8; ML_KEM_768_CIPHERTEXT_BYTES],
}

impl Serialize for MlKem768Ciphertext {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(&self.bytes)
    }
}

impl<'de> Deserialize<'de> for MlKem768Ciphertext {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let bytes: Vec<u8> = Deserialize::deserialize(deserializer)?;
        Self::from_bytes(&bytes).map_err(serde::de::Error::custom)
    }
}

impl MlKem768Ciphertext {
    /// Create from raw bytes
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, PqError> {
        if bytes.len() != ML_KEM_768_CIPHERTEXT_BYTES {
            return Err(PqError::InvalidCiphertext(format!(
                "expected {} bytes, got {}",
                ML_KEM_768_CIPHERTEXT_BYTES,
                bytes.len()
            )));
        }
        let mut arr = [0u8; ML_KEM_768_CIPHERTEXT_BYTES];
        arr.copy_from_slice(bytes);
        Ok(Self { bytes: arr })
    }

    /// Get the raw bytes
    pub fn as_bytes(&self) -> &[u8; ML_KEM_768_CIPHERTEXT_BYTES] {
        &self.bytes
    }
}

impl std::fmt::Debug for MlKem768Ciphertext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MlKem768Ciphertext({:02x?}...)", &self.bytes[..8])
    }
}

/// ML-KEM-768 shared secret (result of encapsulation/decapsulation)
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct MlKem768SharedSecret {
    bytes: [u8; ML_KEM_768_SHARED_SECRET_BYTES],
}

impl MlKem768SharedSecret {
    /// Get the raw bytes (be careful with secret material!)
    pub fn as_bytes(&self) -> &[u8; ML_KEM_768_SHARED_SECRET_BYTES] {
        &self.bytes
    }
}

impl std::fmt::Debug for MlKem768SharedSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MlKem768SharedSecret([REDACTED])")
    }
}

/// ML-KEM-768 keypair for key encapsulation/decapsulation
#[derive(Clone)]
pub struct MlKem768KeyPair {
    public_key: MlKem768PublicKey,
    secret_key: MlKem768SecretKey,
}

impl MlKem768KeyPair {
    /// Generate a new random keypair
    pub fn generate() -> Self {
        let (pk, sk) = kyber768::keypair();

        let mut pk_bytes = [0u8; ML_KEM_768_PUBLIC_KEY_BYTES];
        pk_bytes.copy_from_slice(pk.as_bytes());

        let mut sk_bytes = [0u8; ML_KEM_768_SECRET_KEY_BYTES];
        sk_bytes.copy_from_slice(sk.as_bytes());

        Self {
            public_key: MlKem768PublicKey { bytes: pk_bytes },
            secret_key: MlKem768SecretKey { bytes: sk_bytes },
        }
    }

    /// Generate a keypair deterministically from a 32-byte seed
    ///
    /// This uses HKDF to expand the seed into the required randomness
    /// for key generation, ensuring the same seed always produces
    /// the same keypair.
    ///
    /// Note: Currently uses random keygen as pqcrypto doesn't expose seeded keygen.
    /// TODO: Implement proper deterministic keygen when pqcrypto supports it.
    pub fn from_seed(_seed: &[u8; 32]) -> Self {
        // pqcrypto doesn't expose seeded keygen directly.
        // For now, generate random keys. This will be fixed when we add
        // a proper seeded keygen implementation.
        Self::generate()
    }

    /// Get the public key
    pub fn public_key(&self) -> &MlKem768PublicKey {
        &self.public_key
    }

    /// Get the secret key
    pub fn secret_key(&self) -> &MlKem768SecretKey {
        &self.secret_key
    }

    /// Decapsulate a ciphertext to recover the shared secret
    pub fn decapsulate(&self, ciphertext: &MlKem768Ciphertext) -> Result<MlKem768SharedSecret, PqError> {
        let sk = kyber768::SecretKey::from_bytes(&self.secret_key.bytes)
            .map_err(|_| PqError::InvalidSecretKey("failed to parse secret key".into()))?;

        let ct = kyber768::Ciphertext::from_bytes(&ciphertext.bytes)
            .map_err(|_| PqError::InvalidCiphertext("failed to parse ciphertext".into()))?;

        let ss = kyber768::decapsulate(&ct, &sk);

        let mut ss_bytes = [0u8; ML_KEM_768_SHARED_SECRET_BYTES];
        ss_bytes.copy_from_slice(ss.as_bytes());

        Ok(MlKem768SharedSecret { bytes: ss_bytes })
    }
}

impl std::fmt::Debug for MlKem768KeyPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MlKem768KeyPair")
            .field("public_key", &self.public_key)
            .field("secret_key", &self.secret_key)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_sizes() {
        let keypair = MlKem768KeyPair::generate();
        assert_eq!(keypair.public_key().as_bytes().len(), ML_KEM_768_PUBLIC_KEY_BYTES);
        assert_eq!(keypair.secret_key().as_bytes().len(), ML_KEM_768_SECRET_KEY_BYTES);
    }

    #[test]
    fn test_encapsulate_decapsulate() {
        let keypair = MlKem768KeyPair::generate();

        let (ciphertext, shared_secret) = keypair.public_key().encapsulate();
        assert_eq!(ciphertext.as_bytes().len(), ML_KEM_768_CIPHERTEXT_BYTES);
        assert_eq!(shared_secret.as_bytes().len(), ML_KEM_768_SHARED_SECRET_BYTES);

        let decapsulated = keypair.decapsulate(&ciphertext).unwrap();
        assert_eq!(shared_secret.as_bytes(), decapsulated.as_bytes());
    }

    #[test]
    fn test_wrong_keypair_decapsulation() {
        let keypair1 = MlKem768KeyPair::generate();
        let keypair2 = MlKem768KeyPair::generate();

        let (ciphertext, shared_secret) = keypair1.public_key().encapsulate();

        // Decapsulating with wrong key should produce different shared secret
        // (ML-KEM is IND-CCA2, so decapsulation always "succeeds" but with wrong value)
        let wrong_decap = keypair2.decapsulate(&ciphertext).unwrap();
        assert_ne!(shared_secret.as_bytes(), wrong_decap.as_bytes());
    }

    #[test]
    fn test_public_key_serialization() {
        let keypair = MlKem768KeyPair::generate();
        let bytes = keypair.public_key().as_bytes();
        let restored = MlKem768PublicKey::from_bytes(bytes).unwrap();
        assert_eq!(keypair.public_key().as_bytes(), restored.as_bytes());
    }

    #[test]
    fn test_invalid_public_key_length() {
        let result = MlKem768PublicKey::from_bytes(&[0u8; 100]);
        assert!(matches!(result, Err(PqError::InvalidPublicKey(_))));
    }

    #[test]
    fn test_invalid_ciphertext_length() {
        let result = MlKem768Ciphertext::from_bytes(&[0u8; 100]);
        assert!(matches!(result, Err(PqError::InvalidCiphertext(_))));
    }
}
