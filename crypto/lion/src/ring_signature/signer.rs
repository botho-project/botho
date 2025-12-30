//! Lion ring signature signing implementation.
//!
//! Uses a sequential ring structure where each challenge depends on
//! the previous commitment, breaking the circular dependency between
//! challenges and commitments.

use crate::{
    error::{LionError, Result},
    lattice::{
        commitment::{sample_y, Commitment},
        LionKeyImage, LionSecretKey,
    },
    params::*,
    polynomial::{Poly, PolyVecK, PolyVecL},
    ring_signature::{LionResponse, LionRingSignature, Ring},
};
use rand_core::CryptoRngCore;
use sha3::{Shake256, digest::{ExtendableOutput, Update}};

/// Sign a message with a ring signature.
///
/// Uses a sequential ring structure:
/// 1. Start at real signer position, compute commitment w_r = A*y
/// 2. Go around the ring computing challenges and commitments
/// 3. When returning to real position, compute response z_r = y + c_r*s1
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

    let ring_size = ring.size();
    if real_index >= ring_size {
        return Err(LionError::IndexOutOfBounds {
            index: real_index,
            ring_size,
        });
    }

    // Compute key image
    let key_image = LionKeyImage::from_secret_key(secret_key);

    // Rejection sampling loop for valid signature
    for _attempt in 0..MAX_REJECTION_ITERATIONS {
        // Step 1: Generate commitment randomness y for real signer
        let y = sample_y(rng);

        // Step 2: Compute real commitment w_r = A_r * y
        let real_pk = ring.get(real_index)?;
        let a_real = real_pk.expand_a();
        let w_real = Commitment::compute(&a_real, &y);

        // Step 3: Initialize storage for responses (we'll fill them in as we go around)
        let mut responses: Vec<Option<PolyVecL>> = vec![None; ring_size];
        let mut challenges: Vec<Poly> = vec![Poly::zero(); ring_size];

        // Step 4: Compute challenge for position (real_index + 1) mod ring_size
        let next_index = (real_index + 1) % ring_size;
        challenges[next_index] = compute_next_challenge(
            message,
            &ring,
            &key_image,
            real_index,
            &w_real,
        )?;

        // Step 5: Go around the ring from (real_index + 1) back to real_index
        let mut current = next_index;
        while current != real_index {
            // Sample random response for this decoy (must satisfy norm bound)
            let z_i = sample_bounded_response(rng);

            // Compute commitment w_i = A_i * z_i - c_i * t_i
            let pk_i = ring.get(current)?;
            let a_i = pk_i.expand_a();
            let c_i = &challenges[current];

            let mut az = a_i.mul_vec(&z_i);
            let ct = poly_vec_mul_challenge(&pk_i.t, c_i);
            az.sub_assign(&ct);
            let w_i = Commitment { w: az };

            // Store response
            responses[current] = Some(z_i);

            // Compute challenge for next position
            let next = (current + 1) % ring_size;
            challenges[next] = compute_next_challenge(
                message,
                &ring,
                &key_image,
                current,
                &w_i,
            )?;

            current = next;
        }

        // Step 6: Now we have c_real. Compute z_real = y + c_real * s1
        let c_real = &challenges[real_index];
        let mut z_real = y.clone();

        // z = y + c * s1
        for (z_poly, s1_poly) in z_real.polys.iter_mut().zip(secret_key.s1.polys.iter()) {
            let cs_poly = c_real.ntt_mul(s1_poly);
            z_poly.add_assign(&cs_poly);
        }

        // Step 7: Check if response is valid (rejection sampling)
        let z_norm = z_real.infinity_norm();
        if z_norm >= GAMMA1 - BETA {
            // Rejection: response would leak information about secret key
            continue;
        }

        responses[real_index] = Some(z_real);

        // Step 8: Build final signature with c0 as the starting challenge
        let final_responses: Vec<LionResponse> = responses
            .into_iter()
            .map(|r| LionResponse { z: r.expect("all responses should be filled") })
            .collect();

        return Ok(LionRingSignature {
            c0: challenges[0].clone(),
            key_image,
            responses: final_responses,
        });
    }

    Err(LionError::RejectionSamplingFailed)
}

/// Multiply polynomial vector by challenge polynomial.
fn poly_vec_mul_challenge(v: &PolyVecK, c: &Poly) -> PolyVecK {
    let mut result = PolyVecK::zero();
    for (result_poly, v_poly) in result.polys.iter_mut().zip(v.polys.iter()) {
        *result_poly = c.ntt_mul(v_poly);
    }
    result
}

/// Sample a response vector with bounded infinity norm.
///
/// For decoy responses, we need the norm to be < GAMMA1 - BETA so that
/// verification doesn't reject them. We sample from a smaller range.
fn sample_bounded_response<R: CryptoRngCore>(rng: &mut R) -> PolyVecL {
    let mut z = PolyVecL::zero();
    // Sample from range that guarantees norm < GAMMA1 - BETA
    // Use a margin to ensure we're safely under the bound
    let bound = GAMMA1 - BETA - 100; // Safe margin
    let range = 2 * bound;

    for poly in z.polys.iter_mut() {
        for c in poly.coeffs.iter_mut() {
            // Sample in [0, 2*bound)
            let r = rng.next_u32() % range;
            // Map to [-bound+1, bound]
            *c = if r < bound {
                r
            } else {
                Q - (r - bound + 1)
            };
        }
    }

    z
}

/// Compute the challenge for the next ring position.
///
/// c_{i+1} = H(message, ring, key_image, i, w_i)
fn compute_next_challenge(
    message: &[u8],
    ring: &impl Ring,
    key_image: &LionKeyImage,
    current_index: usize,
    commitment: &Commitment,
) -> Result<Poly> {
    let mut hasher = Shake256::default();

    // Domain separator
    hasher.update(DOMAIN_CHALLENGE);

    // Message
    hasher.update(&(message.len() as u64).to_le_bytes());
    hasher.update(message);

    // Ring public keys (for binding)
    for i in 0..ring.size() {
        let pk = ring.get(i)?;
        hasher.update(&pk.to_bytes());
    }

    // Key image
    hasher.update(&key_image.to_bytes());

    // Current index (which position's commitment this is)
    hasher.update(&(current_index as u16).to_le_bytes());

    // Commitment at current position
    hasher.update(&commitment.to_bytes());

    // Extract challenge seed and sample challenge polynomial
    let mut reader = hasher.finalize_xof();
    let mut challenge_seed = [0u8; 32];
    sha3::digest::XofReader::read(&mut reader, &mut challenge_seed);

    Ok(Poly::sample_challenge(&challenge_seed, TAU))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lattice::{LionKeyPair, LionPublicKey};
    use rand_core::SeedableRng;
    use rand_chacha::ChaCha20Rng;

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

    #[test]
    fn test_sign_different_positions() {
        let mut rng = ChaCha20Rng::seed_from_u64(456);

        let keypairs: Vec<LionKeyPair> = (0..RING_SIZE)
            .map(|i| LionKeyPair::from_seed(&[i as u8; 32]))
            .collect();

        let ring: Vec<LionPublicKey> = keypairs.iter()
            .map(|kp| kp.public_key.clone())
            .collect();

        // Sign from different positions
        for real_index in 0..RING_SIZE {
            let signature = sign(
                b"test",
                ring.as_slice(),
                real_index,
                &keypairs[real_index].secret_key,
                &mut rng,
            ).expect("signing should succeed");

            assert_eq!(signature.responses.len(), RING_SIZE);
            assert_eq!(signature.key_image, keypairs[real_index].key_image());
        }
    }
}
