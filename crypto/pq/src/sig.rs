//! ML-DSA-65 (Dilithium) Digital Signatures
//!
//! ML-DSA is a lattice-based digital signature scheme standardized by NIST
//! in FIPS 204. It provides EUF-CMA security against both classical and
//! quantum adversaries.
//!
//! We use the ML-DSA-65 parameter set which provides approximately 192-bit
//! security against classical attacks and ~128-bit security against quantum.
//!
//! # Usage in Botho
//!
//! ML-DSA is used for quantum-safe transaction signing:
//!
//! 1. Each output has an associated one-time ML-DSA public key
//! 2. To spend, the owner derives the one-time private key and signs
//! 3. Both classical (Schnorr) and PQ (ML-DSA) signatures must verify
//! 4. This hybrid approach ensures security against both classical and quantum attacks

use crate::error::PqError;
use ml_dsa::{KeyGen, MlDsa65, SigningKey, VerifyingKey};
use rand_core::RngCore;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// ML-DSA-65 public key size in bytes
pub const ML_DSA_65_PUBLIC_KEY_BYTES: usize = 1952;

/// ML-DSA-65 secret key size in bytes
pub const ML_DSA_65_SECRET_KEY_BYTES: usize = 4032;

/// ML-DSA-65 signature size in bytes
pub const ML_DSA_65_SIGNATURE_BYTES: usize = 3309;

/// ML-DSA-65 public key for signature verification
#[derive(Clone, PartialEq, Eq)]
pub struct MlDsa65PublicKey {
    bytes: [u8; ML_DSA_65_PUBLIC_KEY_BYTES],
}

impl Serialize for MlDsa65PublicKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(&self.bytes)
    }
}

impl<'de> Deserialize<'de> for MlDsa65PublicKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let bytes: Vec<u8> = Deserialize::deserialize(deserializer)?;
        Self::from_bytes(&bytes).map_err(serde::de::Error::custom)
    }
}

impl MlDsa65PublicKey {
    /// Create from raw bytes
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, PqError> {
        if bytes.len() != ML_DSA_65_PUBLIC_KEY_BYTES {
            return Err(PqError::InvalidPublicKey(format!(
                "expected {} bytes, got {}",
                ML_DSA_65_PUBLIC_KEY_BYTES,
                bytes.len()
            )));
        }
        let mut arr = [0u8; ML_DSA_65_PUBLIC_KEY_BYTES];
        arr.copy_from_slice(bytes);
        Ok(Self { bytes: arr })
    }

    /// Get the raw bytes
    pub fn as_bytes(&self) -> &[u8; ML_DSA_65_PUBLIC_KEY_BYTES] {
        &self.bytes
    }

    /// Verify a signature on a message
    pub fn verify(&self, message: &[u8], signature: &MlDsa65Signature) -> Result<(), PqError> {
        use ml_dsa::signature::Verifier;

        // Parse the public key using decode()
        let vk_encoded = hybrid_array::Array::try_from(&self.bytes[..])
            .expect("public key has correct size");
        let vk = VerifyingKey::<MlDsa65>::decode(&vk_encoded);

        // Parse the signature using decode()
        let sig_encoded = hybrid_array::Array::try_from(&signature.bytes[..])
            .expect("signature has correct size");
        let sig = ml_dsa::Signature::<MlDsa65>::decode(&sig_encoded)
            .ok_or_else(|| PqError::InvalidSignature("failed to decode signature".into()))?;

        // Verify
        vk.verify(message, &sig)
            .map_err(|_| PqError::VerificationFailed)
    }
}

impl std::fmt::Debug for MlDsa65PublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MlDsa65PublicKey({:02x?}...)", &self.bytes[..8])
    }
}

/// ML-DSA-65 secret key for signing
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct MlDsa65SecretKey {
    bytes: [u8; ML_DSA_65_SECRET_KEY_BYTES],
}

impl MlDsa65SecretKey {
    /// Create from raw bytes
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, PqError> {
        if bytes.len() != ML_DSA_65_SECRET_KEY_BYTES {
            return Err(PqError::InvalidSecretKey(format!(
                "expected {} bytes, got {}",
                ML_DSA_65_SECRET_KEY_BYTES,
                bytes.len()
            )));
        }
        let mut arr = [0u8; ML_DSA_65_SECRET_KEY_BYTES];
        arr.copy_from_slice(bytes);
        Ok(Self { bytes: arr })
    }

    /// Get the raw bytes (be careful with secret material!)
    pub fn as_bytes(&self) -> &[u8; ML_DSA_65_SECRET_KEY_BYTES] {
        &self.bytes
    }
}

impl std::fmt::Debug for MlDsa65SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MlDsa65SecretKey([REDACTED])")
    }
}

/// ML-DSA-65 detached signature
#[derive(Clone, PartialEq, Eq)]
pub struct MlDsa65Signature {
    bytes: [u8; ML_DSA_65_SIGNATURE_BYTES],
}

impl Serialize for MlDsa65Signature {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(&self.bytes)
    }
}

impl<'de> Deserialize<'de> for MlDsa65Signature {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let bytes: Vec<u8> = Deserialize::deserialize(deserializer)?;
        Self::from_bytes(&bytes).map_err(serde::de::Error::custom)
    }
}

impl MlDsa65Signature {
    /// Create from raw bytes
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, PqError> {
        if bytes.len() != ML_DSA_65_SIGNATURE_BYTES {
            return Err(PqError::InvalidSignature(format!(
                "expected {} bytes, got {}",
                ML_DSA_65_SIGNATURE_BYTES,
                bytes.len()
            )));
        }
        let mut arr = [0u8; ML_DSA_65_SIGNATURE_BYTES];
        arr.copy_from_slice(bytes);
        Ok(Self { bytes: arr })
    }

    /// Get the raw bytes
    pub fn as_bytes(&self) -> &[u8; ML_DSA_65_SIGNATURE_BYTES] {
        &self.bytes
    }
}

impl std::fmt::Debug for MlDsa65Signature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MlDsa65Signature({:02x?}...)", &self.bytes[..8])
    }
}

/// ML-DSA-65 keypair for signing and verification
#[derive(Clone)]
pub struct MlDsa65KeyPair {
    public_key: MlDsa65PublicKey,
    secret_key: MlDsa65SecretKey,
}

impl MlDsa65KeyPair {
    /// Generate a new random keypair
    pub fn generate() -> Self {
        // Get 32 random bytes using rand_core 0.6's OsRng
        let mut seed = [0u8; 32];
        rand_core::OsRng.fill_bytes(&mut seed);

        // Use deterministic keygen from the seed
        Self::from_seed(&seed)
    }

    /// Generate a keypair deterministically from a 32-byte seed
    ///
    /// This uses ML-DSA's internal deterministic key generation (FIPS 204 KeyGen_internal).
    /// The same seed always produces the same keypair.
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        // Convert to ml-dsa's B32 type (Array<u8, U32>)
        let seed_arr = hybrid_array::Array::try_from(&seed[..])
            .expect("seed has correct size");

        // Use KeyGen trait's from_seed method for deterministic generation
        // This completely avoids any RNG version conflicts!
        let keypair = <MlDsa65 as KeyGen>::from_seed(&seed_arr);

        // Encode keys to bytes
        let pk_encoded = keypair.verifying_key().encode();
        let sk_encoded = keypair.signing_key().encode();

        let mut pk_bytes = [0u8; ML_DSA_65_PUBLIC_KEY_BYTES];
        pk_bytes.copy_from_slice(pk_encoded.as_slice());

        let mut sk_bytes = [0u8; ML_DSA_65_SECRET_KEY_BYTES];
        sk_bytes.copy_from_slice(sk_encoded.as_slice());

        Self {
            public_key: MlDsa65PublicKey { bytes: pk_bytes },
            secret_key: MlDsa65SecretKey { bytes: sk_bytes },
        }
    }

    /// Get the public key
    pub fn public_key(&self) -> &MlDsa65PublicKey {
        &self.public_key
    }

    /// Get the secret key
    pub fn secret_key(&self) -> &MlDsa65SecretKey {
        &self.secret_key
    }

    /// Sign a message
    pub fn sign(&self, message: &[u8]) -> MlDsa65Signature {
        use ml_dsa::signature::Signer;

        // Parse the signing key from encoded bytes using decode()
        let sk_encoded = hybrid_array::Array::try_from(&self.secret_key.bytes[..])
            .expect("secret key has correct size");
        let sk = SigningKey::<MlDsa65>::decode(&sk_encoded);

        // Sign the message (deterministic signing)
        let sig = sk.sign(message);

        let mut sig_bytes = [0u8; ML_DSA_65_SIGNATURE_BYTES];
        sig_bytes.copy_from_slice(sig.encode().as_slice());

        MlDsa65Signature { bytes: sig_bytes }
    }
}

impl std::fmt::Debug for MlDsa65KeyPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MlDsa65KeyPair")
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
        let keypair = MlDsa65KeyPair::generate();
        assert_eq!(keypair.public_key().as_bytes().len(), ML_DSA_65_PUBLIC_KEY_BYTES);
        assert_eq!(keypair.secret_key().as_bytes().len(), ML_DSA_65_SECRET_KEY_BYTES);
    }

    #[test]
    fn test_sign_verify() {
        let keypair = MlDsa65KeyPair::generate();
        let message = b"test message for signing";

        let signature = keypair.sign(message);
        assert_eq!(signature.as_bytes().len(), ML_DSA_65_SIGNATURE_BYTES);

        // Verify should succeed
        assert!(keypair.public_key().verify(message, &signature).is_ok());
    }

    #[test]
    fn test_deterministic_keygen() {
        let seed = [42u8; 32];

        let keypair1 = MlDsa65KeyPair::from_seed(&seed);
        let keypair2 = MlDsa65KeyPair::from_seed(&seed);

        // Same seed should produce same keys
        assert_eq!(keypair1.public_key().as_bytes(), keypair2.public_key().as_bytes());
        assert_eq!(keypair1.secret_key().as_bytes(), keypair2.secret_key().as_bytes());

        // Different seed should produce different keys
        let keypair3 = MlDsa65KeyPair::from_seed(&[43u8; 32]);
        assert_ne!(keypair1.public_key().as_bytes(), keypair3.public_key().as_bytes());
    }

    #[test]
    fn test_deterministic_signing() {
        let seed = [42u8; 32];
        let keypair = MlDsa65KeyPair::from_seed(&seed);
        let message = b"deterministic test";

        // Both signatures should verify
        let sig1 = keypair.sign(message);
        let sig2 = keypair.sign(message);

        assert!(keypair.public_key().verify(message, &sig1).is_ok());
        assert!(keypair.public_key().verify(message, &sig2).is_ok());
    }

    #[test]
    fn test_wrong_message_fails() {
        let keypair = MlDsa65KeyPair::generate();

        let signature = keypair.sign(b"correct message");

        // Verification with wrong message should fail
        let result = keypair.public_key().verify(b"wrong message", &signature);
        assert!(matches!(result, Err(PqError::VerificationFailed)));
    }

    #[test]
    fn test_wrong_key_fails() {
        let keypair1 = MlDsa65KeyPair::generate();
        let keypair2 = MlDsa65KeyPair::generate();

        let message = b"test message";
        let signature = keypair1.sign(message);

        // Verification with wrong public key should fail
        let result = keypair2.public_key().verify(message, &signature);
        assert!(matches!(result, Err(PqError::VerificationFailed)));
    }

    #[test]
    fn test_public_key_serialization() {
        let keypair = MlDsa65KeyPair::generate();
        let bytes = keypair.public_key().as_bytes();
        let restored = MlDsa65PublicKey::from_bytes(bytes).unwrap();
        assert_eq!(keypair.public_key().as_bytes(), restored.as_bytes());
    }

    #[test]
    fn test_signature_serialization() {
        let keypair = MlDsa65KeyPair::generate();
        let signature = keypair.sign(b"test");
        let bytes = signature.as_bytes();
        let restored = MlDsa65Signature::from_bytes(bytes).unwrap();
        assert_eq!(signature.as_bytes(), restored.as_bytes());
    }

    #[test]
    fn test_invalid_public_key_length() {
        let result = MlDsa65PublicKey::from_bytes(&[0u8; 100]);
        assert!(matches!(result, Err(PqError::InvalidPublicKey(_))));
    }

    #[test]
    fn test_invalid_signature_length() {
        let result = MlDsa65Signature::from_bytes(&[0u8; 100]);
        assert!(matches!(result, Err(PqError::InvalidSignature(_))));
    }

    #[test]
    fn test_empty_message() {
        let keypair = MlDsa65KeyPair::generate();
        let signature = keypair.sign(b"");
        assert!(keypair.public_key().verify(b"", &signature).is_ok());
    }

    #[test]
    fn test_large_message() {
        let keypair = MlDsa65KeyPair::generate();
        let message = vec![0xab; 1_000_000]; // 1 MB message
        let signature = keypair.sign(&message);
        assert!(keypair.public_key().verify(&message, &signature).is_ok());
    }
}
