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
use ml_kem::{kem::Encapsulate, Encoded, EncodedSizeUser, KemCore, MlKem768};
// Use rand_core 0.6 which is compatible with ml-kem
use rand_chacha::ChaCha20Rng;
use rand_core::{OsRng, SeedableRng};
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
        // Type alias for the encoded encapsulation key
        type EkEncoded = Encoded<<MlKem768 as KemCore>::EncapsulationKey>;

        // Parse the public key bytes into Encoded, then into EncapsulationKey
        let ek_encoded: EkEncoded = EkEncoded::try_from(&self.bytes[..])
            .expect("invalid encapsulation key size");
        let ek = <MlKem768 as KemCore>::EncapsulationKey::from_bytes(&ek_encoded);

        // Encapsulate with random
        let mut rng = OsRng;
        let (ct, ss) = ek.encapsulate(&mut rng).expect("encapsulation failed");

        // Extract bytes
        let ct_bytes: [u8; ML_KEM_768_CIPHERTEXT_BYTES] = ct.as_slice().try_into().unwrap();
        let ss_bytes: [u8; ML_KEM_768_SHARED_SECRET_BYTES] = ss.as_slice().try_into().unwrap();

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
        let mut rng = OsRng;
        let (dk, ek) = MlKem768::generate(&mut rng);

        let mut pk_bytes = [0u8; ML_KEM_768_PUBLIC_KEY_BYTES];
        pk_bytes.copy_from_slice(ek.as_bytes().as_slice());

        let mut sk_bytes = [0u8; ML_KEM_768_SECRET_KEY_BYTES];
        sk_bytes.copy_from_slice(dk.as_bytes().as_slice());

        Self {
            public_key: MlKem768PublicKey { bytes: pk_bytes },
            secret_key: MlKem768SecretKey { bytes: sk_bytes },
        }
    }

    /// Generate a keypair deterministically from a 32-byte seed
    ///
    /// This uses ChaCha20Rng seeded with the input to generate
    /// deterministic keys. The same seed always produces the same keypair.
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        // ChaCha20Rng from rand_chacha 0.3 is compatible with ml-kem's rand_core 0.6
        let mut rng = ChaCha20Rng::from_seed(*seed);
        let (dk, ek) = MlKem768::generate(&mut rng);

        let mut pk_bytes = [0u8; ML_KEM_768_PUBLIC_KEY_BYTES];
        pk_bytes.copy_from_slice(ek.as_bytes().as_slice());

        let mut sk_bytes = [0u8; ML_KEM_768_SECRET_KEY_BYTES];
        sk_bytes.copy_from_slice(dk.as_bytes().as_slice());

        Self {
            public_key: MlKem768PublicKey { bytes: pk_bytes },
            secret_key: MlKem768SecretKey { bytes: sk_bytes },
        }
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
        use ml_kem::kem::Decapsulate;

        // Type aliases for encoded types
        type DkEncoded = Encoded<<MlKem768 as KemCore>::DecapsulationKey>;
        type CtEncoded = ml_kem::Ciphertext<MlKem768>;

        // Parse the secret key (decapsulation key) bytes into Encoded, then into DecapsulationKey
        let dk_encoded: DkEncoded = DkEncoded::try_from(&self.secret_key.bytes[..])
            .map_err(|_| PqError::InvalidSecretKey("invalid decapsulation key size".into()))?;
        let dk = <MlKem768 as KemCore>::DecapsulationKey::from_bytes(&dk_encoded);

        // Parse the ciphertext - Ciphertext is just an Array, use TryFrom
        let ct: CtEncoded = CtEncoded::try_from(&ciphertext.bytes[..])
            .map_err(|_| PqError::InvalidCiphertext("invalid ciphertext size".into()))?;

        // Decapsulate - returns Result<SharedSecret, ()> in newer ml-kem
        let ss = dk.decapsulate(&ct)
            .map_err(|_| PqError::DecapsulationFailed)?;

        let mut ss_bytes = [0u8; ML_KEM_768_SHARED_SECRET_BYTES];
        ss_bytes.copy_from_slice(ss.as_slice());

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
    fn test_deterministic_keygen() {
        let seed = [42u8; 32];

        let keypair1 = MlKem768KeyPair::from_seed(&seed);
        let keypair2 = MlKem768KeyPair::from_seed(&seed);

        // Same seed should produce same keys
        assert_eq!(keypair1.public_key().as_bytes(), keypair2.public_key().as_bytes());
        assert_eq!(keypair1.secret_key().as_bytes(), keypair2.secret_key().as_bytes());

        // Different seed should produce different keys
        let keypair3 = MlKem768KeyPair::from_seed(&[43u8; 32]);
        assert_ne!(keypair1.public_key().as_bytes(), keypair3.public_key().as_bytes());
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
