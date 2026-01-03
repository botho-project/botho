//! Post-Quantum Stealth Addresses using ML-KEM-768
//!
//! This module implements quantum-safe stealth addresses by replacing the
//! classical ECDH key exchange with ML-KEM-768 key encapsulation.
//!
//! # Protocol
//!
//! **Sender (creating output):**
//! 1. Sender encapsulates shared secret: `(ciphertext, shared_secret) = ML-KEM.Encapsulate(recipient_kem_pk)`
//! 2. Sender derives scalar: `Hs = H(shared_secret || output_index)`
//! 3. Sender computes one-time destination: `target_key = Hs * G + D` (recipient's spend public key)
//! 4. Output contains: `(target_key, ciphertext)`
//!
//! **Recipient (scanning):**
//! 1. Decapsulate: `shared_secret = ML-KEM.Decapsulate(ciphertext, kem_secret_key)`
//! 2. Derive scalar: `Hs = H(shared_secret || output_index)`
//! 3. Compute expected target: `target' = Hs * G + D`
//! 4. If `target' == target_key`, output belongs to recipient
//! 5. Spending key: `x = Hs + spend_secret_key`
//!
//! # Security
//!
//! This is a hybrid approach:
//! - **Post-quantum**: The shared secret derivation uses ML-KEM-768, which is
//!   secure against quantum computers
//! - **Classical**: The one-time keys are still Ristretto points, which provides
//!   proven classical security and compatibility with existing infrastructure
//!
//! This protects against "harvest now, decrypt later" attacks where adversaries
//! archive ciphertexts today for future quantum cryptanalysis.

use bth_crypto_hashes::{Blake2b512, Digest};
use bth_crypto_keys::{RistrettoPrivate, RistrettoPublic};
use bth_crypto_pq::{
    MlKem768Ciphertext, MlKem768KeyPair, MlKem768PublicKey, MlKem768SharedSecret,
};
use curve25519_dalek::{constants::RISTRETTO_BASEPOINT_POINT, ristretto::RistrettoPoint, scalar::Scalar};

use crate::domain_separators::HASH_TO_SCALAR_DOMAIN_TAG;

const G: RistrettoPoint = RISTRETTO_BASEPOINT_POINT;

/// Domain separator for PQ stealth address derivation
const PQ_STEALTH_DOMAIN_TAG: &[u8] = b"botho-pq-stealth-v1";

/// Hashes a shared secret and output index to a Scalar.
///
/// This replaces the ECDH-based `hash_to_scalar(r * C)` from classical stealth addresses.
fn hash_shared_secret_to_scalar(shared_secret: &MlKem768SharedSecret, output_index: u32) -> Scalar {
    let mut hasher = Blake2b512::new();
    hasher.update(HASH_TO_SCALAR_DOMAIN_TAG);
    hasher.update(PQ_STEALTH_DOMAIN_TAG);
    hasher.update(shared_secret.as_bytes());
    hasher.update(output_index.to_le_bytes());
    Scalar::from_hash(hasher)
}

/// Result of encapsulating a shared secret for a PQ stealth address output.
pub struct PqStealthOutput {
    /// The one-time target key (Ristretto point) for the output
    pub target_key: RistrettoPublic,
    /// The ML-KEM ciphertext to include in the output
    pub ciphertext: MlKem768Ciphertext,
    /// The shared secret (for deriving amount blinding, etc.)
    pub shared_secret: MlKem768SharedSecret,
}

/// Creates a PQ stealth address output for a recipient.
///
/// This function encapsulates a shared secret to the recipient's ML-KEM public key
/// and derives a one-time target key for the output.
///
/// # Arguments
/// * `recipient_kem_pk` - The recipient's ML-KEM-768 public key
/// * `recipient_spend_pk` - The recipient's classical spend public key (D)
/// * `output_index` - The index of this output in the transaction
///
/// # Returns
/// A `PqStealthOutput` containing the target key, ciphertext, and shared secret.
pub fn create_pq_stealth_output(
    recipient_kem_pk: &MlKem768PublicKey,
    recipient_spend_pk: &RistrettoPublic,
    output_index: u32,
) -> PqStealthOutput {
    // Encapsulate shared secret to recipient's KEM public key
    let (ciphertext, shared_secret) = recipient_kem_pk.encapsulate();

    // Derive scalar from shared secret
    let hs = hash_shared_secret_to_scalar(&shared_secret, output_index);

    // Compute one-time target key: Hs * G + D
    let d: &RistrettoPoint = recipient_spend_pk.as_ref();
    let target_key = RistrettoPublic::from(hs * G + d);

    PqStealthOutput {
        target_key,
        ciphertext,
        shared_secret,
    }
}

/// Creates the one-time target key for a PQ stealth address output.
///
/// This is a convenience function when you already have the shared secret.
///
/// # Arguments
/// * `shared_secret` - The ML-KEM shared secret
/// * `recipient_spend_pk` - The recipient's classical spend public key (D)
/// * `output_index` - The index of this output in the transaction
pub fn create_pq_target_key(
    shared_secret: &MlKem768SharedSecret,
    recipient_spend_pk: &RistrettoPublic,
    output_index: u32,
) -> RistrettoPublic {
    let hs = hash_shared_secret_to_scalar(shared_secret, output_index);
    let d: &RistrettoPoint = recipient_spend_pk.as_ref();
    RistrettoPublic::from(hs * G + d)
}

/// Recovers the public subaddress spend key from a PQ stealth output.
///
/// This function decapsulates the shared secret and computes `P - Hs * G` to
/// recover the recipient's subaddress spend key. If the output was sent to
/// this recipient, the result equals their subaddress spend public key D_i.
///
/// # Arguments
/// * `kem_keypair` - The recipient's ML-KEM keypair
/// * `ciphertext` - The ML-KEM ciphertext from the output
/// * `target_key` - The output's target key (P)
/// * `output_index` - The index of this output in the transaction
///
/// # Returns
/// The recovered subaddress spend public key, or an error if decapsulation fails.
pub fn recover_pq_subaddress_spend_key(
    kem_keypair: &MlKem768KeyPair,
    ciphertext: &MlKem768Ciphertext,
    target_key: &RistrettoPublic,
    output_index: u32,
) -> Result<RistrettoPublic, bth_crypto_pq::PqError> {
    // Decapsulate to get shared secret
    let shared_secret = kem_keypair.decapsulate(ciphertext)?;

    // Derive scalar from shared secret
    let hs = hash_shared_secret_to_scalar(&shared_secret, output_index);

    // Compute D' = P - Hs * G
    let p: &RistrettoPoint = target_key.as_ref();
    Ok(RistrettoPublic::from(p - hs * G))
}

/// Checks if a PQ stealth output belongs to the given recipient.
///
/// # Arguments
/// * `kem_keypair` - The recipient's ML-KEM keypair
/// * `expected_spend_pk` - The expected subaddress spend public key
/// * `ciphertext` - The ML-KEM ciphertext from the output
/// * `target_key` - The output's target key
/// * `output_index` - The index of this output in the transaction
///
/// # Returns
/// `true` if the output belongs to this recipient, `false` otherwise.
pub fn check_pq_output_ownership(
    kem_keypair: &MlKem768KeyPair,
    expected_spend_pk: &RistrettoPublic,
    ciphertext: &MlKem768Ciphertext,
    target_key: &RistrettoPublic,
    output_index: u32,
) -> bool {
    match recover_pq_subaddress_spend_key(kem_keypair, ciphertext, target_key, output_index) {
        Ok(recovered) => &recovered == expected_spend_pk,
        Err(_) => false,
    }
}

/// Recovers the one-time private key for spending a PQ stealth output.
///
/// This computes `x = Hs + d` where:
/// - `Hs` is derived from the decapsulated shared secret
/// - `d` is the subaddress spend private key
///
/// # Arguments
/// * `kem_keypair` - The recipient's ML-KEM keypair
/// * `ciphertext` - The ML-KEM ciphertext from the output
/// * `subaddress_spend_private` - The subaddress spend private key (d)
/// * `output_index` - The index of this output in the transaction
///
/// # Returns
/// The one-time private key for spending this output.
pub fn recover_pq_onetime_private_key(
    kem_keypair: &MlKem768KeyPair,
    ciphertext: &MlKem768Ciphertext,
    subaddress_spend_private: &RistrettoPrivate,
    output_index: u32,
) -> Result<RistrettoPrivate, bth_crypto_pq::PqError> {
    // Decapsulate to get shared secret
    let shared_secret = kem_keypair.decapsulate(ciphertext)?;

    // Derive scalar from shared secret
    let hs = hash_shared_secret_to_scalar(&shared_secret, output_index);

    // Compute x = Hs + d
    let d: &Scalar = subaddress_spend_private.as_ref();
    Ok(RistrettoPrivate::from(hs + d))
}

/// Decapsulates a shared secret from a PQ stealth output.
///
/// This is useful when you need the shared secret for other purposes
/// (e.g., deriving amount blinding factors).
///
/// # Arguments
/// * `kem_keypair` - The recipient's ML-KEM keypair
/// * `ciphertext` - The ML-KEM ciphertext from the output
pub fn decapsulate_pq_shared_secret(
    kem_keypair: &MlKem768KeyPair,
    ciphertext: &MlKem768Ciphertext,
) -> Result<MlKem768SharedSecret, bth_crypto_pq::PqError> {
    kem_keypair.decapsulate(ciphertext)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bth_crypto_pq::MlKem768KeyPair;
    use bth_util_from_random::FromRandom;
    use rand_core::OsRng;

    #[test]
    fn test_pq_stealth_roundtrip() {
        // Generate recipient keys
        let kem_keypair = MlKem768KeyPair::generate();
        let spend_private = RistrettoPrivate::from_random(&mut OsRng);
        let spend_public = RistrettoPublic::from(&spend_private);

        // Sender creates output
        let output = create_pq_stealth_output(
            kem_keypair.public_key(),
            &spend_public,
            0,
        );

        // Recipient recovers spend key
        let recovered_spend = recover_pq_subaddress_spend_key(
            &kem_keypair,
            &output.ciphertext,
            &output.target_key,
            0,
        ).expect("decapsulation should succeed");

        // Should match
        assert_eq!(recovered_spend, spend_public);
    }

    #[test]
    fn test_pq_ownership_check() {
        // Generate recipient keys
        let kem_keypair = MlKem768KeyPair::generate();
        let spend_private = RistrettoPrivate::from_random(&mut OsRng);
        let spend_public = RistrettoPublic::from(&spend_private);

        // Sender creates output
        let output = create_pq_stealth_output(
            kem_keypair.public_key(),
            &spend_public,
            0,
        );

        // Ownership check should pass for correct spend key
        assert!(check_pq_output_ownership(
            &kem_keypair,
            &spend_public,
            &output.ciphertext,
            &output.target_key,
            0,
        ));

        // Ownership check should fail for wrong spend key
        let wrong_spend = RistrettoPublic::from(&RistrettoPrivate::from_random(&mut OsRng));
        assert!(!check_pq_output_ownership(
            &kem_keypair,
            &wrong_spend,
            &output.ciphertext,
            &output.target_key,
            0,
        ));
    }

    #[test]
    fn test_pq_onetime_key_recovery() {
        // Generate recipient keys
        let kem_keypair = MlKem768KeyPair::generate();
        let spend_private = RistrettoPrivate::from_random(&mut OsRng);
        let spend_public = RistrettoPublic::from(&spend_private);

        // Sender creates output
        let output = create_pq_stealth_output(
            kem_keypair.public_key(),
            &spend_public,
            0,
        );

        // Recipient recovers one-time private key
        let onetime_private = recover_pq_onetime_private_key(
            &kem_keypair,
            &output.ciphertext,
            &spend_private,
            0,
        ).expect("should recover key");

        // The corresponding public key should equal the target key
        let onetime_public = RistrettoPublic::from(&onetime_private);
        assert_eq!(onetime_public, output.target_key);
    }

    #[test]
    fn test_different_output_indices_produce_different_keys() {
        let kem_keypair = MlKem768KeyPair::generate();
        let spend_public = RistrettoPublic::from(&RistrettoPrivate::from_random(&mut OsRng));

        // Create outputs at different indices
        let output0 = create_pq_stealth_output(
            kem_keypair.public_key(),
            &spend_public,
            0,
        );
        let output1 = create_pq_stealth_output(
            kem_keypair.public_key(),
            &spend_public,
            1,
        );

        // Target keys should be different (different ciphertexts = different shared secrets)
        assert_ne!(output0.target_key, output1.target_key);
    }

    #[test]
    fn test_wrong_kem_keypair_fails_ownership() {
        // Generate two different keypairs
        let kem_keypair1 = MlKem768KeyPair::generate();
        let kem_keypair2 = MlKem768KeyPair::generate();
        let spend_public = RistrettoPublic::from(&RistrettoPrivate::from_random(&mut OsRng));

        // Sender creates output for keypair1
        let output = create_pq_stealth_output(
            kem_keypair1.public_key(),
            &spend_public,
            0,
        );

        // Ownership check with keypair2 should fail
        // (ML-KEM is IND-CCA2, so decapsulation "succeeds" but returns wrong secret)
        assert!(!check_pq_output_ownership(
            &kem_keypair2,
            &spend_public,
            &output.ciphertext,
            &output.target_key,
            0,
        ));
    }

    #[test]
    fn test_create_pq_target_key_consistency() {
        let kem_keypair = MlKem768KeyPair::generate();
        let spend_public = RistrettoPublic::from(&RistrettoPrivate::from_random(&mut OsRng));

        // Create output using full function
        let output = create_pq_stealth_output(
            kem_keypair.public_key(),
            &spend_public,
            0,
        );

        // Recreate target key using shared secret
        let target_key = create_pq_target_key(
            &output.shared_secret,
            &spend_public,
            0,
        );

        // Should match
        assert_eq!(target_key, output.target_key);
    }
}
