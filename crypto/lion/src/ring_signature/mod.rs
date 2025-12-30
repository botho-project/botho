//! Lion linkable ring signature scheme.
//!
//! This module implements the core ring signature operations:
//! signing and verification.

mod signer;
mod verifier;

pub use signer::sign;
pub use verifier::verify;

#[cfg(feature = "parallel")]
use rayon::prelude::*;

/// Batch verification result for a single signature.
#[derive(Debug, Clone)]
pub struct BatchVerifyResult {
    /// Index of the signature in the input batch.
    pub index: usize,
    /// Verification result.
    pub result: Result<()>,
}

/// Verify multiple signatures in parallel.
///
/// This function verifies a batch of signatures, optionally using parallel
/// execution when the `parallel` feature is enabled. Each signature is
/// verified independently against its corresponding message and ring.
///
/// # Arguments
/// * `items` - Iterator of (message, ring, signature) tuples
///
/// # Returns
/// A vector of `BatchVerifyResult` containing the index and result for each signature.
///
/// # Example
/// ```rust,no_run
/// use bth_crypto_lion::{verify_batch, LionRingSignature, LionPublicKey};
///
/// let items: Vec<(&[u8], &[LionPublicKey], &LionRingSignature)> = vec![
///     // (message, ring, signature) tuples
/// ];
///
/// let results = verify_batch(items);
/// for result in results {
///     if result.result.is_err() {
///         println!("Signature {} failed verification", result.index);
///     }
/// }
/// ```
#[cfg(feature = "parallel")]
pub fn verify_batch<'a>(
    items: impl IntoIterator<Item = (&'a [u8], &'a [LionPublicKey], &'a LionRingSignature)>,
) -> Vec<BatchVerifyResult> {
    let items: Vec<_> = items.into_iter().collect();

    items
        .par_iter()
        .enumerate()
        .map(|(index, (message, ring, signature))| BatchVerifyResult {
            index,
            result: verify(*message, *ring, *signature),
        })
        .collect()
}

/// Verify multiple signatures (serial version when parallel feature is disabled).
#[cfg(not(feature = "parallel"))]
pub fn verify_batch<'a>(
    items: impl IntoIterator<Item = (&'a [u8], &'a [LionPublicKey], &'a LionRingSignature)>,
) -> Vec<BatchVerifyResult> {
    items
        .into_iter()
        .enumerate()
        .map(|(index, (message, ring, signature))| BatchVerifyResult {
            index,
            result: verify(message, ring, signature),
        })
        .collect()
}

/// Check if all signatures in a batch are valid.
///
/// Returns `Ok(())` if all signatures verify, or the first error encountered.
/// Uses parallel verification when the `parallel` feature is enabled.
///
/// # Arguments
/// * `items` - Iterator of (message, ring, signature) tuples
///
/// # Returns
/// `Ok(())` if all signatures are valid, or the first verification error.
#[cfg(feature = "parallel")]
pub fn verify_batch_all<'a>(
    items: impl IntoIterator<Item = (&'a [u8], &'a [LionPublicKey], &'a LionRingSignature)>,
) -> Result<()> {
    let items: Vec<_> = items.into_iter().collect();

    items
        .par_iter()
        .try_for_each(|(message, ring, signature)| verify(*message, *ring, *signature))
}

/// Check if all signatures in a batch are valid (serial version).
#[cfg(not(feature = "parallel"))]
pub fn verify_batch_all<'a>(
    items: impl IntoIterator<Item = (&'a [u8], &'a [LionPublicKey], &'a LionRingSignature)>,
) -> Result<()> {
    for (message, ring, signature) in items {
        verify(message, ring, signature)?;
    }
    Ok(())
}

use crate::{
    error::{LionError, Result},
    lattice::{LionKeyImage, LionPublicKey},
    params::*,
    polynomial::{Poly, PolyVecL},
};

/// A Lion ring signature.
///
/// Uses a sequential ring structure where each challenge depends on
/// the previous commitment, breaking the circular dependency.
///
/// Contains all the data needed to verify the signature:
/// - Starting challenge c0 (the first challenge in the ring)
/// - Key image (for linkability/double-spend detection)
/// - Response vectors for each ring member
#[derive(Clone, Debug)]
pub struct LionRingSignature {
    /// Starting challenge (at index 0).
    /// Other challenges are computed sequentially during verification.
    pub c0: Poly,

    /// Key image for linkability.
    pub key_image: LionKeyImage,

    /// Response vector z for each ring member.
    /// z_i = y_i + c_i * s for real signer, random for others.
    pub responses: Vec<LionResponse>,
}

/// Response for a single ring member.
#[derive(Clone, Debug)]
pub struct LionResponse {
    /// Response vector z.
    pub z: PolyVecL,
}

impl LionResponse {
    /// Serialize to bytes.
    pub fn to_bytes(&self) -> [u8; RESPONSE_BYTES] {
        let mut bytes = [0u8; RESPONSE_BYTES];

        // Serialize each polynomial (3 bytes per coefficient)
        let mut offset = 0;
        for poly in self.z.polys.iter() {
            let poly_bytes = poly.to_bytes();
            bytes[offset..offset + 768].copy_from_slice(&poly_bytes);
            offset += 768;
        }

        bytes
    }

    /// Deserialize from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != RESPONSE_BYTES {
            return Err(LionError::InvalidSignature);
        }

        let mut z = PolyVecL::zero();

        for (i, poly) in z.polys.iter_mut().enumerate() {
            let offset = i * 768;
            let poly_bytes: [u8; 768] = bytes[offset..offset + 768]
                .try_into()
                .map_err(|_| LionError::DeserializationError("invalid response bytes"))?;

            *poly = Poly::from_bytes(&poly_bytes)
                .ok_or(LionError::DeserializationError("invalid polynomial in response"))?;
        }

        Ok(Self { z })
    }
}

impl LionRingSignature {
    /// Serialize the signature to bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(signature_size(self.responses.len()));

        // Starting challenge c0 (768 bytes for a polynomial)
        bytes.extend_from_slice(&self.c0.to_bytes());

        // Key image
        bytes.extend_from_slice(&self.key_image.to_bytes());

        // Responses
        for response in &self.responses {
            bytes.extend_from_slice(&response.to_bytes());
        }

        bytes
    }

    /// Deserialize from bytes.
    pub fn from_bytes(bytes: &[u8], ring_size: usize) -> Result<Self> {
        let expected_size = signature_size(ring_size);
        if bytes.len() != expected_size {
            return Err(LionError::InvalidSignature);
        }

        // Starting challenge c0
        let c0_bytes: [u8; POLY_BYTES] = bytes[..POLY_BYTES]
            .try_into()
            .map_err(|_| LionError::DeserializationError("invalid c0 bytes"))?;
        let c0 = Poly::from_bytes(&c0_bytes)
            .ok_or(LionError::DeserializationError("invalid c0 polynomial"))?;

        // Key image
        let key_image = LionKeyImage::from_bytes(&bytes[POLY_BYTES..POLY_BYTES + KEY_IMAGE_BYTES])?;

        // Responses
        let mut offset = POLY_BYTES + KEY_IMAGE_BYTES;
        let mut responses = Vec::with_capacity(ring_size);
        for _ in 0..ring_size {
            let response_bytes = &bytes[offset..offset + RESPONSE_BYTES];
            responses.push(LionResponse::from_bytes(response_bytes)?);
            offset += RESPONSE_BYTES;
        }

        Ok(Self {
            c0,
            key_image,
            responses,
        })
    }

    /// Get the size of this signature in bytes.
    pub fn size(&self) -> usize {
        signature_size(self.responses.len())
    }
}

/// Trait for ring operations.
pub trait Ring {
    /// Get the number of members in the ring.
    fn size(&self) -> usize;

    /// Get the public key at the given index.
    fn get(&self, index: usize) -> Result<&LionPublicKey>;

    /// Validate the ring.
    fn validate(&self) -> Result<()> {
        if self.size() != RING_SIZE {
            return Err(LionError::InvalidRingSize {
                expected: RING_SIZE,
                got: self.size(),
            });
        }
        Ok(())
    }
}

impl Ring for &[LionPublicKey] {
    fn size(&self) -> usize {
        self.len()
    }

    fn get(&self, index: usize) -> Result<&LionPublicKey> {
        <[LionPublicKey]>::get(self, index).ok_or(LionError::IndexOutOfBounds {
            index,
            ring_size: self.len(),
        })
    }
}

impl Ring for Vec<LionPublicKey> {
    fn size(&self) -> usize {
        self.len()
    }

    fn get(&self, index: usize) -> Result<&LionPublicKey> {
        <[LionPublicKey]>::get(self.as_slice(), index).ok_or(LionError::IndexOutOfBounds {
            index,
            ring_size: self.len(),
        })
    }
}

impl Ring for &Vec<LionPublicKey> {
    fn size(&self) -> usize {
        self.len()
    }

    fn get(&self, index: usize) -> Result<&LionPublicKey> {
        <[LionPublicKey]>::get(self.as_slice(), index).ok_or(LionError::IndexOutOfBounds {
            index,
            ring_size: self.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lattice::LionKeyPair;
    use rand::SeedableRng;
    use rand_chacha::ChaCha20Rng;

    #[test]
    fn test_sign_verify_roundtrip() {
        let mut rng = ChaCha20Rng::seed_from_u64(42);
        let message = b"test message for ring signature";

        // Generate ring of 7 keypairs
        let keypairs: Vec<LionKeyPair> = (0..RING_SIZE)
            .map(|i| LionKeyPair::from_seed(&[i as u8; 32]))
            .collect();

        let ring: Vec<LionPublicKey> = keypairs.iter()
            .map(|kp| kp.public_key.clone())
            .collect();

        // Real signer is at index 3
        let real_index = 3;
        let secret_key = &keypairs[real_index].secret_key;

        // Sign
        let signature = sign(message, &ring, real_index, secret_key, &mut rng)
            .expect("signing should succeed");

        // Verify
        let result = verify(message, &ring, &signature);
        if let Err(ref e) = result {
            eprintln!("Verification failed: {:?}", e);
        }
        assert!(result.is_ok());
    }

    #[test]
    fn test_sign_verify_at_position_zero() {
        // Test signing at position 0 (edge case for sequential ring)
        let mut rng = ChaCha20Rng::seed_from_u64(999);
        let message = b"test position zero";

        let keypairs: Vec<LionKeyPair> = (0..RING_SIZE)
            .map(|i| LionKeyPair::from_seed(&[i as u8; 32]))
            .collect();

        let ring: Vec<LionPublicKey> = keypairs.iter()
            .map(|kp| kp.public_key.clone())
            .collect();

        let signature = sign(message, &ring, 0, &keypairs[0].secret_key, &mut rng)
            .expect("signing should succeed");

        assert!(verify(message, &ring, &signature).is_ok());
    }

    #[test]
    fn test_verify_rejects_wrong_message() {
        let mut rng = ChaCha20Rng::seed_from_u64(123);

        let keypairs: Vec<LionKeyPair> = (0..RING_SIZE)
            .map(|i| LionKeyPair::from_seed(&[i as u8; 32]))
            .collect();

        let ring: Vec<LionPublicKey> = keypairs.iter()
            .map(|kp| kp.public_key.clone())
            .collect();

        let signature = sign(
            b"correct message",
            &ring,
            0,
            &keypairs[0].secret_key,
            &mut rng,
        ).expect("signing should succeed");

        // Verification with wrong message should fail
        assert!(verify(b"wrong message", &ring, &signature).is_err());
    }

    #[test]
    fn test_signature_serialization() {
        let mut rng = ChaCha20Rng::seed_from_u64(456);

        let keypairs: Vec<LionKeyPair> = (0..RING_SIZE)
            .map(|i| LionKeyPair::from_seed(&[i as u8; 32]))
            .collect();

        let ring: Vec<LionPublicKey> = keypairs.iter()
            .map(|kp| kp.public_key.clone())
            .collect();

        let signature = sign(
            b"test",
            &ring,
            2,
            &keypairs[2].secret_key,
            &mut rng,
        ).expect("signing should succeed");

        let bytes = signature.to_bytes();
        let recovered = LionRingSignature::from_bytes(&bytes, RING_SIZE)
            .expect("deserialization should succeed");

        // Recovered signature should still verify
        assert!(verify(b"test", &ring, &recovered).is_ok());
    }

    #[test]
    fn test_key_image_linkability() {
        let mut rng = ChaCha20Rng::seed_from_u64(789);

        let keypairs: Vec<LionKeyPair> = (0..RING_SIZE)
            .map(|i| LionKeyPair::from_seed(&[i as u8; 32]))
            .collect();

        let ring: Vec<LionPublicKey> = keypairs.iter()
            .map(|kp| kp.public_key.clone())
            .collect();

        // Sign twice with the same key
        let sig1 = sign(b"message1", &ring, 0, &keypairs[0].secret_key, &mut rng)
            .expect("signing should succeed");
        let sig2 = sign(b"message2", &ring, 0, &keypairs[0].secret_key, &mut rng)
            .expect("signing should succeed");

        // Key images should be the same (linkable)
        assert_eq!(sig1.key_image, sig2.key_image);

        // But with a different key, key image should be different
        let sig3 = sign(b"message3", &ring, 1, &keypairs[1].secret_key, &mut rng)
            .expect("signing should succeed");
        assert_ne!(sig1.key_image, sig3.key_image);
    }

    #[test]
    fn test_verify_batch() {
        let mut rng = ChaCha20Rng::seed_from_u64(1234);

        let keypairs: Vec<LionKeyPair> = (0..RING_SIZE)
            .map(|i| LionKeyPair::from_seed(&[i as u8; 32]))
            .collect();

        let ring: Vec<LionPublicKey> = keypairs.iter()
            .map(|kp| kp.public_key.clone())
            .collect();

        // Create multiple signatures
        let messages: Vec<&[u8]> = vec![b"msg1", b"msg2", b"msg3"];
        let signatures: Vec<LionRingSignature> = messages.iter()
            .enumerate()
            .map(|(i, msg)| {
                sign(*msg, ring.as_slice(), i % RING_SIZE, &keypairs[i % RING_SIZE].secret_key, &mut rng)
                    .expect("signing should succeed")
            })
            .collect();

        // Batch verify - all should pass
        let items: Vec<_> = messages.iter()
            .zip(signatures.iter())
            .map(|(msg, sig)| (*msg, ring.as_slice(), sig))
            .collect();

        let results = verify_batch(items);
        assert_eq!(results.len(), 3);
        for result in &results {
            assert!(result.result.is_ok(), "signature {} failed", result.index);
        }
    }

    #[test]
    fn test_verify_batch_all() {
        let mut rng = ChaCha20Rng::seed_from_u64(5678);

        let keypairs: Vec<LionKeyPair> = (0..RING_SIZE)
            .map(|i| LionKeyPair::from_seed(&[i as u8; 32]))
            .collect();

        let ring: Vec<LionPublicKey> = keypairs.iter()
            .map(|kp| kp.public_key.clone())
            .collect();

        // Create multiple valid signatures
        let messages: Vec<&[u8]> = vec![b"batch1", b"batch2"];
        let signatures: Vec<LionRingSignature> = messages.iter()
            .enumerate()
            .map(|(i, msg)| {
                sign(*msg, ring.as_slice(), i, &keypairs[i].secret_key, &mut rng)
                    .expect("signing should succeed")
            })
            .collect();

        // All valid - should return Ok
        let items: Vec<_> = messages.iter()
            .zip(signatures.iter())
            .map(|(msg, sig)| (*msg, ring.as_slice(), sig))
            .collect();

        assert!(verify_batch_all(items).is_ok());

        // Include one invalid - should return error
        let items_with_invalid: Vec<_> = vec![
            (messages[0], ring.as_slice(), &signatures[0]),
            (b"wrong message".as_slice(), ring.as_slice(), &signatures[1]),  // wrong message
        ];

        assert!(verify_batch_all(items_with_invalid).is_err());
    }
}
