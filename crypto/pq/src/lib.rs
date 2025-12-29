//! Post-Quantum Cryptographic Primitives for Botho
//!
//! This crate provides wrappers around NIST-standardized post-quantum algorithms:
//!
//! - **ML-KEM-768** (Kyber): Key Encapsulation Mechanism for stealth address key exchange
//! - **ML-DSA-65** (Dilithium): Digital signatures for transaction signing
//!
//! # Quantum Resistance Strategy
//!
//! Botho uses a hybrid classical + post-quantum approach for private transactions:
//!
//! 1. Classical layer (Schnorr/Ristretto) provides current security guarantees
//! 2. Post-quantum layer (ML-KEM/ML-DSA) protects against future quantum computers
//! 3. Both layers must verify for a transaction to be valid
//!
//! This protects privacy against "harvest now, decrypt later" attacks where
//! adversaries archive blockchain data today for future quantum cryptanalysis.
//!
//! # Example
//!
//! ```rust
//! use bth_crypto_pq::{MlKem768KeyPair, MlDsa65KeyPair};
//!
//! // Generate keypairs from seed (deterministic)
//! let seed = [0u8; 32];
//! let kem_keypair = MlKem768KeyPair::from_seed(&seed);
//! let sig_keypair = MlDsa65KeyPair::from_seed(&seed);
//!
//! // Key encapsulation (for stealth addresses)
//! let (ciphertext, shared_secret) = kem_keypair.public_key().encapsulate();
//! let decapsulated = kem_keypair.decapsulate(&ciphertext).unwrap();
//! assert_eq!(shared_secret.as_bytes(), decapsulated.as_bytes());
//!
//! // Digital signatures (for transaction signing)
//! let message = b"transaction data";
//! let signature = sig_keypair.sign(message);
//! assert!(sig_keypair.public_key().verify(message, &signature).is_ok());
//! ```

mod derive;
mod error;
mod kem;
mod sig;

pub use derive::{derive_onetime_sig_keypair, derive_pq_keys, PqKeyMaterial};
pub use error::PqError;
pub use kem::{
    MlKem768Ciphertext, MlKem768KeyPair, MlKem768PublicKey, MlKem768SecretKey, MlKem768SharedSecret,
    ML_KEM_768_CIPHERTEXT_BYTES, ML_KEM_768_PUBLIC_KEY_BYTES, ML_KEM_768_SECRET_KEY_BYTES,
    ML_KEM_768_SHARED_SECRET_BYTES,
};
pub use sig::{
    MlDsa65KeyPair, MlDsa65PublicKey, MlDsa65SecretKey, MlDsa65Signature,
    ML_DSA_65_PUBLIC_KEY_BYTES, ML_DSA_65_SECRET_KEY_BYTES, ML_DSA_65_SIGNATURE_BYTES,
};

/// Domain separator for Botho PQ key derivation
pub const BOTHO_PQ_DOMAIN: &[u8] = b"botho-pq-v1";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kem_roundtrip() {
        let seed = [42u8; 32];
        let keypair = MlKem768KeyPair::from_seed(&seed);

        let (ciphertext, shared_secret) = keypair.public_key().encapsulate();
        let decapsulated = keypair.decapsulate(&ciphertext).unwrap();

        assert_eq!(shared_secret.as_bytes(), decapsulated.as_bytes());
    }

    #[test]
    fn test_signature_roundtrip() {
        let seed = [42u8; 32];
        let keypair = MlDsa65KeyPair::from_seed(&seed);

        let message = b"test message for signing";
        let signature = keypair.sign(message);

        assert!(keypair.public_key().verify(message, &signature).is_ok());
    }

    #[test]
    fn test_signature_wrong_message_fails() {
        let seed = [42u8; 32];
        let keypair = MlDsa65KeyPair::from_seed(&seed);

        let signature = keypair.sign(b"correct message");
        assert!(keypair.public_key().verify(b"wrong message", &signature).is_err());
    }

    #[test]
    fn test_keygen_deterministic() {
        let seed = [123u8; 32];
        let keypair1 = MlKem768KeyPair::from_seed(&seed);
        let keypair2 = MlKem768KeyPair::from_seed(&seed);

        // Same seed produces same keys
        assert_eq!(keypair1.public_key().as_bytes(), keypair2.public_key().as_bytes());

        // Verify the keypair works for encapsulation
        let (ct, ss) = keypair1.public_key().encapsulate();
        let decap = keypair1.decapsulate(&ct).unwrap();
        assert_eq!(ss.as_bytes(), decap.as_bytes());
    }

    #[test]
    fn test_derive_pq_keys_deterministic() {
        let mnemonic = b"abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let keys1 = derive_pq_keys(mnemonic);
        let keys2 = derive_pq_keys(mnemonic);

        // Same mnemonic produces same keys
        assert_eq!(
            keys1.kem_keypair.public_key().as_bytes(),
            keys2.kem_keypair.public_key().as_bytes()
        );
        assert_eq!(
            keys1.sig_keypair.public_key().as_bytes(),
            keys2.sig_keypair.public_key().as_bytes()
        );

        // Verify the KEM keypair works
        let (ct, ss) = keys1.kem_keypair.public_key().encapsulate();
        let decap = keys1.kem_keypair.decapsulate(&ct).unwrap();
        assert_eq!(ss.as_bytes(), decap.as_bytes());

        // Verify the signature keypair works
        let msg = b"test message";
        let sig = keys1.sig_keypair.sign(msg);
        assert!(keys1.sig_keypair.public_key().verify(msg, &sig).is_ok());
    }
}
