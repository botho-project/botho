// Copyright (c) 2024 Botho Foundation

#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

//! Lion: Lattice-based Linkable Ring Signatures for Botho
//!
//! This crate implements the Lion post-quantum ring signature scheme,
//! providing sender privacy with linkability for double-spend detection.
//!
//! # Overview
//!
//! Lion is a lattice-based linkable ring signature scheme designed for
//! use in privacy-preserving cryptocurrency transactions. It provides:
//!
//! - **Sender Privacy**: The actual signer is hidden among a ring of
//!   possible signers (ring size = 7)
//! - **Linkability**: Signatures from the same key can be linked via
//!   the key image, enabling double-spend detection
//! - **Post-Quantum Security**: Based on Module-LWE/SIS assumptions,
//!   resistant to quantum attacks
//!
//! # Example
//!
//! ```rust,no_run
//! use bth_crypto_lion::{
//!     lattice::{LionKeyPair, LionPublicKey},
//!     ring_signature::{sign, verify},
//!     params::RING_SIZE,
//! };
//! use rand::rngs::OsRng;
//!
//! // Generate keypairs for the ring
//! let keypairs: Vec<LionKeyPair> = (0..RING_SIZE)
//!     .map(|_| LionKeyPair::generate(&mut OsRng))
//!     .collect();
//!
//! // Extract public keys for the ring
//! let ring: Vec<LionPublicKey> = keypairs.iter()
//!     .map(|kp| kp.public_key.clone())
//!     .collect();
//!
//! // Sign a message (real signer at index 3)
//! let message = b"Transfer 100 credits";
//! let real_index = 3;
//! let signature = sign(
//!     message,
//!     ring.as_slice(),
//!     real_index,
//!     &keypairs[real_index].secret_key,
//!     &mut OsRng,
//! ).expect("signing failed");
//!
//! // Anyone can verify the signature
//! assert!(verify(message, ring.as_slice(), &signature).is_ok());
//!
//! // Key image is deterministic - same key always produces same image
//! let key_image = keypairs[real_index].key_image();
//! assert_eq!(signature.key_image, key_image);
//! ```
//!
//! # Security Parameters
//!
//! The scheme uses parameters similar to ML-DSA (Dilithium) for
//! approximately 128-bit post-quantum security:
//!
//! - Ring dimension N = 256
//! - Modulus Q = 8380417
//! - Module dimensions K = L = 4
//! - Ring size = 7 members
//!
//! # Signature Sizes
//!
//! | Component | Size |
//! |-----------|------|
//! | Public Key | 1,312 bytes |
//! | Secret Key | 800 bytes |
//! | Key Image | 1,312 bytes |
//! | Signature (7 members) | ~17.5 KB |

#[cfg(feature = "std")]
extern crate std;

extern crate alloc;

pub mod error;
pub mod lattice;
pub mod params;
pub mod polynomial;
pub mod ring_signature;

// Re-export commonly used types
pub use error::{LionError, Result};
pub use lattice::{LionKeyImage, LionKeyPair, LionPublicKey, LionSecretKey};
pub use params::{KEY_IMAGE_BYTES, PUBLIC_KEY_BYTES, RING_SIZE, SECRET_KEY_BYTES, SIGNATURE_BYTES};
pub use ring_signature::{sign, verify, verify_batch, verify_batch_all, BatchVerifyResult, LionRingSignature, Ring};

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand_chacha::ChaCha20Rng;

    #[test]
    fn test_full_workflow() {
        let mut rng = ChaCha20Rng::seed_from_u64(12345);

        // Generate ring of keypairs
        let keypairs: Vec<LionKeyPair> = (0..RING_SIZE)
            .map(|i| LionKeyPair::from_seed(&[i as u8; 32]))
            .collect();

        let ring: Vec<LionPublicKey> = keypairs
            .iter()
            .map(|kp| kp.public_key.clone())
            .collect();

        // Sign and verify
        let message = b"Hello, post-quantum world!";
        let real_index = 4;

        let signature = ring_signature::sign(
            message,
            ring.as_slice(),
            real_index,
            &keypairs[real_index].secret_key,
            &mut rng,
        )
        .expect("signing should succeed");

        // Verify passes
        assert!(ring_signature::verify(message, ring.as_slice(), &signature).is_ok());

        // Key image matches
        assert_eq!(signature.key_image, keypairs[real_index].key_image());

        // Serialization roundtrip
        let sig_bytes = signature.to_bytes();
        let recovered =
            LionRingSignature::from_bytes(&sig_bytes, RING_SIZE).expect("deserialization failed");
        assert!(ring_signature::verify(message, ring.as_slice(), &recovered).is_ok());
    }

    #[test]
    fn test_signature_size() {
        let mut rng = ChaCha20Rng::seed_from_u64(42);

        let keypairs: Vec<LionKeyPair> = (0..RING_SIZE)
            .map(|i| LionKeyPair::from_seed(&[i as u8; 32]))
            .collect();

        let ring: Vec<LionPublicKey> = keypairs
            .iter()
            .map(|kp| kp.public_key.clone())
            .collect();

        let signature = ring_signature::sign(
            b"test",
            ring.as_slice(),
            0,
            &keypairs[0].secret_key,
            &mut rng,
        )
        .expect("signing should succeed");

        // Verify signature size matches expected
        assert_eq!(signature.to_bytes().len(), SIGNATURE_BYTES);
    }

    #[test]
    fn test_double_spend_detection() {
        let mut rng = ChaCha20Rng::seed_from_u64(999);

        let keypairs: Vec<LionKeyPair> = (0..RING_SIZE)
            .map(|i| LionKeyPair::from_seed(&[i as u8; 32]))
            .collect();

        let ring: Vec<LionPublicKey> = keypairs
            .iter()
            .map(|kp| kp.public_key.clone())
            .collect();

        // Same signer signs two different messages
        let sig1 = ring_signature::sign(
            b"spend output 1",
            ring.as_slice(),
            2,
            &keypairs[2].secret_key,
            &mut rng,
        )
        .expect("signing should succeed");

        let sig2 = ring_signature::sign(
            b"spend output 1 again",
            ring.as_slice(),
            2,
            &keypairs[2].secret_key,
            &mut rng,
        )
        .expect("signing should succeed");

        // Both signatures are valid
        assert!(ring_signature::verify(b"spend output 1", ring.as_slice(), &sig1).is_ok());
        assert!(ring_signature::verify(b"spend output 1 again", ring.as_slice(), &sig2).is_ok());

        // But they have the same key image - double spend detected!
        assert_eq!(sig1.key_image, sig2.key_image);

        // Different signer has different key image
        let sig3 = ring_signature::sign(
            b"different output",
            ring.as_slice(),
            3,
            &keypairs[3].secret_key,
            &mut rng,
        )
        .expect("signing should succeed");

        assert_ne!(sig1.key_image, sig3.key_image);
    }
}
