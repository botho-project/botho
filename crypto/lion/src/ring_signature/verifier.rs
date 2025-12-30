//! Lion ring signature verification implementation.
//!
//! Uses a sequential ring structure where each challenge depends on
//! the previous commitment. Verification recomputes all challenges
//! and checks that the ring "closes" (final challenge equals c0).

use crate::{
    error::{LionError, Result},
    lattice::{commitment::Commitment, LionKeyImage, LionPublicKey},
    params::*,
    polynomial::{Poly, PolyVecK},
    ring_signature::{LionRingSignature, Ring},
};
use sha3::{Shake256, digest::{ExtendableOutput, Update}};

/// Verify a Lion ring signature.
///
/// Uses a sequential ring structure:
/// 1. Start with c0 from the signature
/// 2. For each position, compute commitment and derive next challenge
/// 3. Verify that the ring closes: c'_0 == c_0
///
/// # Arguments
/// * `message` - The message that was signed
/// * `ring` - The ring of public keys
/// * `signature` - The signature to verify
///
/// # Returns
/// `Ok(())` if the signature is valid, or an error describing why it's invalid.
pub fn verify(
    message: &[u8],
    ring: impl Ring,
    signature: &LionRingSignature,
) -> Result<()> {
    // Validate ring
    ring.validate()?;

    let ring_size = ring.size();

    // Check signature has correct number of responses
    if signature.responses.len() != ring_size {
        return Err(LionError::InvalidSignature);
    }

    // Start with c0 from the signature
    let mut current_challenge = signature.c0.clone();

    // Go around the ring: for each position, compute commitment and next challenge
    for i in 0..ring_size {
        let pk = ring.get(i)?;
        let z = &signature.responses[i].z;

        // Check response norm (rejection sampling bound)
        if z.infinity_norm() >= GAMMA1 - BETA {
            return Err(LionError::VerificationFailed);
        }

        // Compute commitment w_i = A_i * z_i - c_i * t_i
        let w_i = compute_commitment_from_response(pk, z, &current_challenge)?;

        // Compute challenge for next position
        let next_challenge = compute_next_challenge(
            message,
            &ring,
            &signature.key_image,
            i,
            &w_i,
        )?;

        current_challenge = next_challenge;
    }

    // Verify the ring closes: after going around, we should get back c0
    if current_challenge != signature.c0 {
        return Err(LionError::VerificationFailed);
    }

    Ok(())
}

/// Compute commitment from response and challenge.
///
/// Computes w_i = A*z - c*t
fn compute_commitment_from_response(
    pk: &LionPublicKey,
    z: &crate::polynomial::PolyVecL,
    c: &Poly,
) -> Result<Commitment> {
    // Expand the matrix A
    let a = pk.expand_a();

    // Compute A*z
    let az = a.mul_vec(z);

    // Compute c*t
    let ct = poly_vec_mul_challenge(&pk.t, c);

    // Compute w = A*z - c*t
    let mut w = az;
    w.sub_assign(&ct);

    Ok(Commitment { w })
}

/// Multiply a polynomial vector by a challenge polynomial.
fn poly_vec_mul_challenge(v: &PolyVecK, c: &Poly) -> PolyVecK {
    let mut result = PolyVecK::zero();

    for (result_poly, v_poly) in result.polys.iter_mut().zip(v.polys.iter()) {
        *result_poly = c.ntt_mul(v_poly);
    }

    result
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
pub(crate) mod tests {
    use super::*;
    use crate::lattice::LionKeyPair;
    use crate::ring_signature::sign;
    use rand::SeedableRng;
    use rand_chacha::ChaCha20Rng;

    #[test]
    fn test_verify_valid_signature() {
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

        // Verification should pass
        assert!(verify(b"test message", ring.as_slice(), &signature).is_ok());
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
            ring.as_slice(),
            0,
            &keypairs[0].secret_key,
            &mut rng,
        ).expect("signing should succeed");

        // Verification with wrong message should fail
        assert!(verify(b"wrong message", ring.as_slice(), &signature).is_err());
    }

    #[test]
    fn test_verify_rejects_wrong_ring() {
        let mut rng = ChaCha20Rng::seed_from_u64(456);

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

        // Create a different ring
        let other_keypairs: Vec<LionKeyPair> = (0..RING_SIZE)
            .map(|i| LionKeyPair::from_seed(&[i as u8 + 100; 32]))
            .collect();

        let other_ring: Vec<LionPublicKey> = other_keypairs.iter()
            .map(|kp| kp.public_key.clone())
            .collect();

        // Verification with wrong ring should fail
        assert!(verify(b"test", other_ring.as_slice(), &signature).is_err());
    }

    #[test]
    fn test_verify_rejects_modified_response() {
        let mut rng = ChaCha20Rng::seed_from_u64(789);

        let keypairs: Vec<LionKeyPair> = (0..RING_SIZE)
            .map(|i| LionKeyPair::from_seed(&[i as u8; 32]))
            .collect();

        let ring: Vec<LionPublicKey> = keypairs.iter()
            .map(|kp| kp.public_key.clone())
            .collect();

        let mut signature = sign(
            b"test",
            ring.as_slice(),
            1,
            &keypairs[1].secret_key,
            &mut rng,
        ).expect("signing should succeed");

        // Modify a response
        signature.responses[0].z.polys[0].coeffs[0] =
            (signature.responses[0].z.polys[0].coeffs[0] + 1) % Q;

        // Verification should fail
        assert!(verify(b"test", ring.as_slice(), &signature).is_err());
    }

    #[test]
    fn test_verify_all_positions() {
        let mut rng = ChaCha20Rng::seed_from_u64(999);

        let keypairs: Vec<LionKeyPair> = (0..RING_SIZE)
            .map(|i| LionKeyPair::from_seed(&[i as u8; 32]))
            .collect();

        let ring: Vec<LionPublicKey> = keypairs.iter()
            .map(|kp| kp.public_key.clone())
            .collect();

        // Test signing from each position
        for real_index in 0..RING_SIZE {
            let signature = sign(
                b"test all positions",
                ring.as_slice(),
                real_index,
                &keypairs[real_index].secret_key,
                &mut rng,
            ).expect("signing should succeed");

            let result = verify(b"test all positions", ring.as_slice(), &signature);
            assert!(result.is_ok(), "verification failed for real_index={}: {:?}", real_index, result);
        }
    }

    #[test]
    fn test_poly_vec_mul_challenge() {
        // Basic sanity test for the multiplication
        let v = PolyVecK::zero();
        let c = Poly::zero();

        let result = poly_vec_mul_challenge(&v, &c);

        // Multiplying by zero should give zero
        for poly in result.polys.iter() {
            for &coeff in poly.coeffs.iter() {
                assert_eq!(coeff, 0);
            }
        }
    }
}
