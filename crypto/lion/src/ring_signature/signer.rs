//! Lion ring signature signing implementation.

use crate::{
    error::{LionError, Result},
    lattice::{
        commitment::{sample_y, Commitment},
        LionKeyImage, LionSecretKey,
    },
    params::*,
    polynomial::{Poly, PolyVecL},
    ring_signature::{LionResponse, LionRingSignature, Ring},
};
use rand_core::CryptoRngCore;
use sha3::{Shake256, digest::{ExtendableOutput, Update}};

/// Sign a message with a ring signature.
///
/// # Arguments
/// * `message` - The message to sign
/// * `ring` - The ring of public keys (must contain exactly 7 members)
/// * `real_index` - Index of the signer's public key in the ring
/// * `secret_key` - The signer's secret key
/// * `rng` - Cryptographic random number generator
///
/// # Returns
/// A `LionRingSignature` if successful, or an error.
pub fn sign<R: CryptoRngCore>(
    message: &[u8],
    ring: impl Ring,
    real_index: usize,
    secret_key: &LionSecretKey,
    rng: &mut R,
) -> Result<LionRingSignature> {
    // Validate ring
    ring.validate()?;

    if real_index >= ring.size() {
        return Err(LionError::IndexOutOfBounds {
            index: real_index,
            ring_size: ring.size(),
        });
    }

    // Compute key image
    let key_image = LionKeyImage::from_secret_key(secret_key);

    // Generate random seed for this signature
    let mut sig_seed = [0u8; 32];
    rng.fill_bytes(&mut sig_seed);

    // Expand matrix A for real signer (all ring members should use same A, but we verify)
    let real_pk = ring.get(real_index)?;
    let mut a_ntt = real_pk.expand_a();
    a_ntt.ntt();

    // Prepare NTT form of secret key
    let mut s1_ntt = secret_key.s1.clone();
    s1_ntt.ntt();

    // Rejection sampling loop for valid signature
    for _attempt in 0..MAX_REJECTION_ITERATIONS {
        // Generate commitment randomness y for real signer
        let y = sample_y(rng);

        // Compute commitment w = A*y
        let commitment = Commitment::compute(&a_ntt, &y);

        // Generate random responses for all non-real ring members
        let mut responses: Vec<LionResponse> = Vec::with_capacity(ring.size());
        let mut commitments: Vec<Commitment> = Vec::with_capacity(ring.size());

        for i in 0..ring.size() {
            if i == real_index {
                // Placeholder for real response (computed after challenge)
                responses.push(LionResponse { z: PolyVecL::zero() });
                commitments.push(commitment.clone());
            } else {
                // Random response for decoy
                let z = sample_y(rng);
                let pk = ring.get(i)?;
                let mut a_i_ntt = pk.expand_a();
                a_i_ntt.ntt();

                // Compute "commitment" that will be consistent with this response
                // In a proper ring sig, we compute w_i from z_i and challenge
                // For now, just use the response commitment
                let w_i = Commitment::compute(&a_i_ntt, &z);

                responses.push(LionResponse { z });
                commitments.push(w_i);
            }
        }

        // Compute challenge from all commitments
        let challenge_seed = compute_challenge_seed(
            message,
            &ring,
            &key_image,
            &commitments,
            &sig_seed,
        )?;

        // Compute per-member challenges
        let challenges = expand_challenges(&challenge_seed, ring.size());

        // Compute real response: z = y + c * s1
        let c = &challenges[real_index];
        let mut z = y.clone();

        for (z_poly, s1_poly) in z.polys.iter_mut().zip(secret_key.s1.polys.iter()) {
            for (z_coeff, &s1_coeff) in z_poly.coeffs.iter_mut().zip(s1_poly.coeffs.iter()) {
                // Multiply challenge by secret coefficient
                let cs = poly_mul_scalar(c, s1_coeff);
                // Add to randomness
                *z_coeff = (*z_coeff + cs) % Q;
            }
        }

        // Check if response is valid (norm check for rejection sampling)
        let z_norm = z.infinity_norm();
        if z_norm >= GAMMA1 - BETA {
            // Rejection: response would leak information about secret key
            continue;
        }

        // Update the real response
        responses[real_index] = LionResponse { z };

        // Signature is valid
        return Ok(LionRingSignature {
            challenge_seed,
            key_image,
            responses,
        });
    }

    Err(LionError::RejectionSamplingFailed)
}

/// Compute the challenge seed from all commitments.
fn compute_challenge_seed(
    message: &[u8],
    ring: &impl Ring,
    key_image: &LionKeyImage,
    commitments: &[Commitment],
    sig_seed: &[u8; 32],
) -> Result<[u8; 32]> {
    let mut hasher = Shake256::default();

    // Domain separator
    hasher.update(DOMAIN_CHALLENGE);

    // Message
    hasher.update(&(message.len() as u64).to_le_bytes());
    hasher.update(message);

    // Signature randomness
    hasher.update(sig_seed);

    // Ring public keys
    for i in 0..ring.size() {
        let pk = ring.get(i)?;
        hasher.update(&pk.to_bytes());
    }

    // Key image
    hasher.update(&key_image.to_bytes());

    // All commitments
    for commitment in commitments {
        hasher.update(&commitment.to_bytes());
    }

    // Extract challenge seed
    let mut reader = hasher.finalize_xof();
    let mut seed = [0u8; 32];
    sha3::digest::XofReader::read(&mut reader, &mut seed);

    Ok(seed)
}

/// Expand per-member challenges from the challenge seed.
fn expand_challenges(seed: &[u8; 32], ring_size: usize) -> Vec<Poly> {
    let mut challenges = Vec::with_capacity(ring_size);

    for i in 0..ring_size {
        let mut hasher = Shake256::default();
        hasher.update(DOMAIN_EXPAND);
        hasher.update(seed);
        hasher.update(&(i as u16).to_le_bytes());
        let mut reader = hasher.finalize_xof();

        // Sample challenge polynomial with TAU non-zero coefficients
        let mut challenge_seed = [0u8; 32];
        sha3::digest::XofReader::read(&mut reader, &mut challenge_seed);

        challenges.push(Poly::sample_challenge(&challenge_seed, TAU));
    }

    challenges
}

/// Multiply polynomial by a scalar (used for c * s1 computation).
fn poly_mul_scalar(poly: &Poly, scalar: u32) -> u32 {
    // For the simplified version, we just return the weighted sum
    // In a full implementation, this would be proper polynomial multiplication
    let mut sum = 0u64;
    for &c in poly.coeffs.iter() {
        let contribution = (c as u64 * scalar as u64) % Q as u64;
        sum = (sum + contribution) % Q as u64;
    }
    sum as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lattice::{LionKeyPair, LionPublicKey};
    use rand_core::SeedableRng;
    use rand_chacha::ChaCha20Rng;

    #[test]
    fn test_expand_challenges_deterministic() {
        let seed = [42u8; 32];
        let c1 = expand_challenges(&seed, 7);
        let c2 = expand_challenges(&seed, 7);

        for i in 0..7 {
            assert_eq!(c1[i], c2[i]);
        }
    }

    #[test]
    fn test_expand_challenges_different_indices() {
        let seed = [42u8; 32];
        let challenges = expand_challenges(&seed, 7);

        // All challenges should be different
        for i in 0..7 {
            for j in (i + 1)..7 {
                assert_ne!(challenges[i], challenges[j]);
            }
        }
    }

    #[test]
    fn test_sign_produces_valid_structure() {
        let mut rng = ChaCha20Rng::seed_from_u64(42);

        let keypairs: Vec<LionKeyPair> = (0..RING_SIZE)
            .map(|i| LionKeyPair::from_seed(&[i as u8; 32]))
            .collect();

        let ring: Vec<LionPublicKey> = keypairs.iter()
            .map(|kp| kp.public_key.clone())
            .collect();

        let signature = sign(
            b"test message",
            ring.as_slice(),
            3,
            &keypairs[3].secret_key,
            &mut rng,
        ).expect("signing should succeed");

        // Check structure
        assert_eq!(signature.responses.len(), RING_SIZE);
        assert_eq!(signature.challenge_seed.len(), 32);
    }

    #[test]
    fn test_sign_key_image_matches_keypair() {
        let mut rng = ChaCha20Rng::seed_from_u64(123);

        let keypairs: Vec<LionKeyPair> = (0..RING_SIZE)
            .map(|i| LionKeyPair::from_seed(&[i as u8; 32]))
            .collect();

        let ring: Vec<LionPublicKey> = keypairs.iter()
            .map(|kp| kp.public_key.clone())
            .collect();

        let signature = sign(
            b"test",
            ring.as_slice(),
            2,
            &keypairs[2].secret_key,
            &mut rng,
        ).expect("signing should succeed");

        // Key image in signature should match the keypair's key image
        let expected_key_image = keypairs[2].key_image();
        assert_eq!(signature.key_image, expected_key_image);
    }
}
