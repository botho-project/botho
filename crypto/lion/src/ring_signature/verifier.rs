//! Lion ring signature verification implementation.

use crate::{
    error::{LionError, Result},
    lattice::{commitment::Commitment, LionPublicKey},
    params::*,
    polynomial::{Poly, PolyVecK, PolyVecL},
    ring_signature::{LionRingSignature, Ring},
};
use sha3::{Shake256, digest::{ExtendableOutput, Update}};

/// Verify a Lion ring signature.
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

    // Check signature has correct number of responses
    if signature.responses.len() != ring.size() {
        return Err(LionError::InvalidSignature);
    }

    // Expand per-member challenges
    let challenges = expand_challenges(&signature.challenge_seed, ring.size());

    // Recompute commitments from responses and challenges
    let mut commitments = Vec::with_capacity(ring.size());

    for i in 0..ring.size() {
        let pk = ring.get(i)?;
        let z = &signature.responses[i].z;
        let c = &challenges[i];

        // Check response norm
        if z.infinity_norm() >= GAMMA1 - BETA {
            return Err(LionError::VerificationFailed);
        }

        // Compute w' = A*z - c*t
        // This should equal the original commitment w if signature is valid
        let w_prime = compute_commitment_from_response(pk, z, c)?;
        commitments.push(w_prime);
    }

    // Recompute challenge seed
    let recomputed_seed = compute_challenge_seed(
        message,
        &ring,
        &signature.key_image,
        &commitments,
    )?;

    // Verify challenge matches
    if recomputed_seed != signature.challenge_seed {
        return Err(LionError::VerificationFailed);
    }

    // Verify key image is well-formed
    // In a full implementation, we would also verify the key image
    // is correctly derived from one of the ring members

    Ok(())
}

/// Compute commitment from response and challenge.
///
/// Computes w' = A*z - c*t, which should equal the original w
/// for valid signatures.
fn compute_commitment_from_response(
    pk: &LionPublicKey,
    z: &PolyVecL,
    c: &Poly,
) -> Result<Commitment> {
    // Expand and NTT transform the matrix A
    let mut a_ntt = pk.expand_a();
    a_ntt.ntt();

    // NTT transform z
    let mut z_ntt = z.clone();
    z_ntt.ntt();

    // Compute A*z in NTT domain
    let mut az = a_ntt.mul_vec(&z_ntt);
    az.inv_ntt();

    // Compute c*t
    let ct = poly_vec_mul_challenge(&pk.t, c);

    // Compute w' = A*z - c*t
    let mut w = az;
    w.sub_assign(&ct);

    Ok(Commitment { w })
}

/// Multiply a polynomial vector by a challenge polynomial.
fn poly_vec_mul_challenge(v: &PolyVecK, c: &Poly) -> PolyVecK {
    let mut result = PolyVecK::zero();

    // Transform challenge to NTT domain
    let mut c_ntt = c.clone();
    c_ntt.ntt();

    for (result_poly, v_poly) in result.polys.iter_mut().zip(v.polys.iter()) {
        let mut v_ntt = v_poly.clone();
        v_ntt.ntt();

        *result_poly = c_ntt.pointwise_mul(&v_ntt);
        result_poly.inv_ntt();
    }

    result
}

/// Compute the challenge seed from commitments.
fn compute_challenge_seed(
    message: &[u8],
    ring: &impl Ring,
    key_image: &crate::lattice::LionKeyImage,
    commitments: &[Commitment],
) -> Result<[u8; 32]> {
    let mut hasher = Shake256::default();

    // Domain separator
    hasher.update(DOMAIN_CHALLENGE);

    // Message
    hasher.update(&(message.len() as u64).to_le_bytes());
    hasher.update(message);

    // Note: sig_seed is incorporated in signing but we recompute from commitments
    // For verification, we use a zero seed since it's deterministic from commitments
    hasher.update(&[0u8; 32]);

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

        let mut challenge_seed = [0u8; 32];
        sha3::digest::XofReader::read(&mut reader, &mut challenge_seed);

        challenges.push(Poly::sample_challenge(&challenge_seed, TAU));
    }

    challenges
}

#[cfg(test)]
mod tests {
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
