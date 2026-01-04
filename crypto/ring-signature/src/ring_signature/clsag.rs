// Copyright (c) 2018-2025 The Botho Foundation

//! CLSAG (Concise Linkable Spontaneous Anonymous Group) signatures.
//!
//! CLSAG is an improvement over MLSAG that reduces signature size by ~50%
//! by aggregating the key and commitment components into a single response
//! per ring member.
//!
//! Reference: "Concise Linkable Ring Signatures and Forgery Against Adversarial
//! Keys" https://eprint.iacr.org/2019/654

use alloc::vec::Vec;
use curve25519_dalek::ristretto::RistrettoPoint;
use rand_core::CryptoRngCore;
use zeroize::Zeroize;

use bth_crypto_digestible::Digestible;
use bth_crypto_hashes::{Blake2b512, Digest};
use bth_crypto_keys::{RistrettoPrivate, RistrettoPublic};

#[cfg(feature = "prost")]
use prost::Message;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::{
    domain_separators::{
        CLSAG_AGG_COEFF_C_DOMAIN_TAG, CLSAG_AGG_COEFF_P_DOMAIN_TAG, CLSAG_ROUND_HASH_DOMAIN_TAG,
    },
    ring_signature::{
        hash_to_point, CurveScalar, Error, KeyImage, PedersenGens, Scalar, B_BLINDING,
    },
    Commitment, CompressedCommitment, ReducedTxOut,
};

/// CLSAG signature for a ring of public keys and amount commitments.
///
/// CLSAG uses a single response scalar per ring member (vs 2 for MLSAG),
/// achieved by aggregating the key and commitment proofs.
#[derive(Clone, Digestible, PartialEq, Eq, Zeroize)]
#[cfg_attr(feature = "prost", derive(Message))]
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
pub struct Clsag {
    /// The initial challenge `c[0]`.
    #[cfg_attr(feature = "prost", prost(message, required, tag = "1"))]
    pub c_zero: CurveScalar,

    /// Responses `s[0], s[1], ..., s[ring_size-1]`.
    /// One response per ring member (half the size of MLSAG).
    #[cfg_attr(feature = "prost", prost(message, repeated, tag = "2"))]
    pub responses: Vec<CurveScalar>,

    /// Key image "spent" by this signature.
    #[cfg_attr(feature = "prost", prost(message, required, tag = "3"))]
    pub key_image: KeyImage,

    /// Auxiliary key image D = z * Hp(P) for the commitment component.
    /// This is needed for verification of the commitment balance.
    #[cfg_attr(feature = "prost", prost(message, required, tag = "4"))]
    pub commitment_key_image: KeyImage,
}

impl Clsag {
    /// Sign a ring of input addresses and amount commitments using CLSAG.
    ///
    /// # Arguments
    /// * `message` - Message to be signed.
    /// * `ring` - A ring of reduced TxOuts.
    /// * `real_index` - The index in the ring of the real input.
    /// * `onetime_private_key` - The real input's private key.
    /// * `value` - Value of the real input.
    /// * `blinding` - Blinding of the real input's commitment.
    /// * `output_blinding` - The output amount's blinding factor.
    /// * `generator` - The Pedersen generator for commitments.
    /// * `rng` - Cryptographic RNG.
    pub fn sign(
        message: &[u8],
        ring: &[ReducedTxOut],
        real_index: usize,
        onetime_private_key: &RistrettoPrivate,
        value: u64,
        blinding: &Scalar,
        output_blinding: &Scalar,
        generator: &PedersenGens,
        rng: &mut dyn CryptoRngCore,
    ) -> Result<Self, Error> {
        Self::sign_with_balance_check(
            message,
            ring,
            real_index,
            onetime_private_key,
            value,
            blinding,
            output_blinding,
            generator,
            true,
            rng,
        )
    }

    fn sign_with_balance_check(
        message: &[u8],
        ring: &[ReducedTxOut],
        real_index: usize,
        onetime_private_key: &RistrettoPrivate,
        value: u64,
        blinding: &Scalar,
        output_blinding: &Scalar,
        generator: &PedersenGens,
        check_value_is_preserved: bool,
        rng: &mut dyn CryptoRngCore,
    ) -> Result<Self, Error> {
        let ring_size = ring.len();

        if real_index >= ring_size {
            return Err(Error::IndexOutOfBounds);
        }

        if ring_size == 0 {
            return Err(Error::IndexOutOfBounds);
        }

        let G = B_BLINDING;

        // Pre-decompress ring
        let mut decompressed_ring =
            alloc::vec![(RistrettoPublic::default(), Commitment::default()); ring_size];
        for (i, r) in ring.iter().enumerate() {
            decompressed_ring[i] = r.try_into()?;
        }

        // Compute the output commitment
        let output_commitment = Commitment::new(value, *output_blinding, generator);

        // Compute commitment to zero differences: Z[i] = output_commitment -
        // input_commitment[i]
        let mut z_points: Vec<RistrettoPoint> = Vec::with_capacity(ring_size);
        for (_, input_commitment) in &decompressed_ring {
            z_points.push(output_commitment.point - input_commitment.point);
        }

        // The secret commitment difference for the real input
        let z: Scalar = *output_blinding - *blinding;

        // Check value is preserved
        if check_value_is_preserved {
            let (_, real_input_commitment) = &decompressed_ring[real_index];
            let difference = output_commitment.point - real_input_commitment.point;
            if difference != (z * G) {
                return Err(Error::ValueNotConserved);
            }
        }

        // Secret key for signing
        let x: Scalar = *onetime_private_key.as_ref();
        let real_pubkey = &decompressed_ring[real_index].0;

        // Compute key image: I = x * Hp(P)
        let key_image = KeyImage::from(onetime_private_key);
        let I = key_image.point.decompress().ok_or(Error::InvalidKeyImage)?;

        // Compute commitment key image (auxiliary): D = z * Hp(P)
        let Hp_real = hash_to_point(real_pubkey);
        let D = z * Hp_real;
        let commitment_key_image = KeyImage {
            point: D.compress(),
        };

        // Compute aggregation coefficients mu_P and mu_C
        let (mu_P, mu_C) =
            compute_aggregation_coefficients(ring, &key_image, &commitment_key_image);

        // Initialize responses
        let mut responses: Vec<CurveScalar> =
            alloc::vec![CurveScalar::from(Scalar::ZERO); ring_size];
        for i in 0..ring_size {
            if i != real_index {
                responses[i] = CurveScalar::from(Scalar::random(rng));
            }
        }

        // Random nonce for the real signer
        let alpha = Scalar::random(rng);

        // Compute initial L and R at real_index
        // L = alpha * G
        // R = alpha * Hp(P_real)
        let L_init = alpha * G;
        let R_init = alpha * Hp_real;

        // Compute c[real_index + 1]
        let mut challenges: Vec<Scalar> = alloc::vec![Scalar::ZERO; ring_size];
        challenges[(real_index + 1) % ring_size] =
            compute_round_hash(message, &key_image, &commitment_key_image, &L_init, &R_init);

        // Go around the ring from real_index + 1 back to real_index
        for n in 1..ring_size {
            let i = (real_index + n) % ring_size;
            let next_i = (i + 1) % ring_size;

            let (P_i, _) = &decompressed_ring[i];
            let Hp_i = hash_to_point(P_i);

            let c_i = challenges[i];
            let s_i = responses[i].scalar;

            // Aggregated public key: W = mu_P * P + mu_C * Z
            let W_i = mu_P * P_i.as_ref() + mu_C * z_points[i];

            // L = s * G + c * W
            let L_i = s_i * G + c_i * W_i;

            // R = s * Hp(P) + c * (mu_P * I + mu_C * D)
            let R_i = s_i * Hp_i + c_i * (mu_P * I + mu_C * D);

            // c[next] = H(...)
            challenges[next_i] =
                compute_round_hash(message, &key_image, &commitment_key_image, &L_i, &R_i);
        }

        // Close the loop: compute s[real_index]
        // s = alpha - c * (mu_P * x + mu_C * z)
        let c_real = challenges[real_index];
        let s_real = alpha - c_real * (mu_P * x + mu_C * z);
        responses[real_index] = CurveScalar::from(s_real);

        Ok(Clsag {
            c_zero: CurveScalar::from(challenges[0]),
            responses,
            key_image,
            commitment_key_image,
        })
    }

    /// Verify a CLSAG signature.
    ///
    /// # Arguments
    /// * `message` - Message that was signed.
    /// * `ring` - The ring of input addresses and commitments.
    /// * `output_commitment` - The output commitment.
    pub fn verify(
        &self,
        message: &[u8],
        ring: &[ReducedTxOut],
        output_commitment: &CompressedCommitment,
    ) -> Result<(), Error> {
        let ring_size = ring.len();

        if self.responses.len() != ring_size {
            return Err(Error::LengthMismatch(ring_size, self.responses.len()));
        }

        if ring_size == 0 {
            return Err(Error::IndexOutOfBounds);
        }

        let G = B_BLINDING;

        // Decompress key images
        let I = self
            .key_image
            .point
            .decompress()
            .ok_or(Error::InvalidKeyImage)?;
        let D = self
            .commitment_key_image
            .point
            .decompress()
            .ok_or(Error::InvalidKeyImage)?;

        // Decompress output commitment
        let output_commitment: Commitment = Commitment::try_from(output_commitment)?;

        // Pre-decompress ring
        let mut decompressed_ring =
            alloc::vec![(RistrettoPublic::default(), Commitment::default()); ring_size];
        for (i, r) in ring.iter().enumerate() {
            decompressed_ring[i] = r.try_into()?;
        }

        // Compute commitment to zero differences
        let mut z_points: Vec<RistrettoPoint> = Vec::with_capacity(ring_size);
        for (_, input_commitment) in &decompressed_ring {
            z_points.push(output_commitment.point - input_commitment.point);
        }

        // Compute aggregation coefficients
        let (mu_P, mu_C) =
            compute_aggregation_coefficients(ring, &self.key_image, &self.commitment_key_image);

        // Verify the ring
        let mut c = self.c_zero.scalar;

        for i in 0..ring_size {
            let (P_i, _) = &decompressed_ring[i];
            let Hp_i = hash_to_point(P_i);
            let s_i = self.responses[i].scalar;

            // Aggregated public key
            let W_i = mu_P * P_i.as_ref() + mu_C * z_points[i];

            // L = s * G + c * W
            let L_i = s_i * G + c * W_i;

            // R = s * Hp(P) + c * (mu_P * I + mu_C * D)
            let R_i = s_i * Hp_i + c * (mu_P * I + mu_C * D);

            // c[next] = H(...)
            c = compute_round_hash(
                message,
                &self.key_image,
                &self.commitment_key_image,
                &L_i,
                &R_i,
            );
        }

        // Check that we closed the loop
        if c == self.c_zero.scalar {
            Ok(())
        } else {
            Err(Error::InvalidSignature)
        }
    }
}

/// Compute the aggregation coefficients mu_P and mu_C.
///
/// These coefficients are derived from a hash of the ring and key images,
/// ensuring the aggregation is binding.
fn compute_aggregation_coefficients(
    ring: &[ReducedTxOut],
    key_image: &KeyImage,
    commitment_key_image: &KeyImage,
) -> (Scalar, Scalar) {
    // mu_P = H("bth_clsag_agg_p" || ring || I || D)
    let mut hasher_p = Blake2b512::new();
    hasher_p.update(CLSAG_AGG_COEFF_P_DOMAIN_TAG);
    for r in ring {
        hasher_p.update(AsRef::<[u8; 32]>::as_ref(&r.target_key));
        hasher_p.update(AsRef::<[u8; 32]>::as_ref(&r.commitment));
    }
    hasher_p.update(key_image.as_bytes());
    hasher_p.update(commitment_key_image.as_bytes());
    let mu_P = Scalar::from_hash(hasher_p);

    // mu_C = H("bth_clsag_agg_c" || ring || I || D)
    let mut hasher_c = Blake2b512::new();
    hasher_c.update(CLSAG_AGG_COEFF_C_DOMAIN_TAG);
    for r in ring {
        hasher_c.update(AsRef::<[u8; 32]>::as_ref(&r.target_key));
        hasher_c.update(AsRef::<[u8; 32]>::as_ref(&r.commitment));
    }
    hasher_c.update(key_image.as_bytes());
    hasher_c.update(commitment_key_image.as_bytes());
    let mu_C = Scalar::from_hash(hasher_c);

    (mu_P, mu_C)
}

/// Compute the round hash for challenge derivation.
fn compute_round_hash(
    message: &[u8],
    key_image: &KeyImage,
    commitment_key_image: &KeyImage,
    L: &RistrettoPoint,
    R: &RistrettoPoint,
) -> Scalar {
    let mut hasher = Blake2b512::new();
    hasher.update(CLSAG_ROUND_HASH_DOMAIN_TAG);
    hasher.update(message);
    hasher.update(key_image.as_bytes());
    hasher.update(commitment_key_image.as_bytes());
    hasher.update(L.compress().as_bytes());
    hasher.update(R.compress().as_bytes());
    Scalar::from_hash(hasher)
}

#[cfg(test)]
mod clsag_tests {
    use super::*;
    use crate::generators;
    use bth_crypto_keys::CompressedRistrettoPublic;
    use bth_util_from_random::FromRandom;
    use bth_util_test_helper::{RngType, SeedableRng};
    use proptest::prelude::*;
    use rand_core::RngCore;

    #[derive(Clone)]
    struct ClsagTestParams {
        message: [u8; 32],
        ring: Vec<ReducedTxOut>,
        real_index: usize,
        onetime_private_key: RistrettoPrivate,
        value: u64,
        blinding: Scalar,
        pseudo_output_blinding: Scalar,
        generator: PedersenGens,
    }

    impl ClsagTestParams {
        fn random<RNG: CryptoRngCore>(
            num_mixins: usize,
            pseudo_output_blinding: Scalar,
            rng: &mut RNG,
        ) -> Self {
            let mut message = [0u8; 32];
            rng.fill_bytes(&mut message);

            let generator = generators(rng.next_u64());

            let mut ring: Vec<ReducedTxOut> = Vec::new();
            for _ in 0..num_mixins {
                let public_key = CompressedRistrettoPublic::from_random(rng);
                let target_key = CompressedRistrettoPublic::from_random(rng);
                let commitment = {
                    let value = rng.next_u64();
                    let blinding = Scalar::random(rng);
                    CompressedCommitment::new(value, blinding, &generator)
                };
                ring.push(ReducedTxOut {
                    public_key,
                    target_key,
                    commitment,
                });
            }

            // The real input
            let onetime_private_key = RistrettoPrivate::from_random(rng);
            let value = rng.next_u64();
            let blinding = Scalar::random(rng);
            let commitment = CompressedCommitment::new(value, blinding, &generator);

            let reduced_tx_out = ReducedTxOut {
                target_key: CompressedRistrettoPublic::from(RistrettoPublic::from(
                    &onetime_private_key,
                )),
                public_key: CompressedRistrettoPublic::from_random(rng),
                commitment,
            };

            let real_index = rng.next_u64() as usize % (num_mixins + 1);
            ring.insert(real_index, reduced_tx_out);
            assert_eq!(ring.len(), num_mixins + 1);

            Self {
                message,
                ring,
                real_index,
                onetime_private_key,
                value,
                blinding,
                pseudo_output_blinding,
                generator,
            }
        }

        fn sign<RNG: CryptoRngCore>(&self, rng: &mut RNG) -> Result<Clsag, Error> {
            Clsag::sign(
                &self.message,
                &self.ring,
                self.real_index,
                &self.onetime_private_key,
                self.value,
                &self.blinding,
                &self.pseudo_output_blinding,
                &self.generator,
                rng,
            )
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(6))]

        #[test]
        fn test_clsag_signature_has_correct_length(
            num_mixins in 1..17usize,
            seed in any::<[u8; 32]>(),
        ) {
            let mut rng: RngType = SeedableRng::from_seed(seed);
            let pseudo_output_blinding = Scalar::random(&mut rng);
            let params = ClsagTestParams::random(num_mixins, pseudo_output_blinding, &mut rng);

            let signature = params.sign(&mut rng).unwrap();

            // CLSAG has 1 response per ring member (vs 2 for MLSAG)
            let ring_size = num_mixins + 1;
            assert_eq!(signature.responses.len(), ring_size);

            // All responses should be non-zero
            for r in &signature.responses {
                assert_ne!(r.scalar, Scalar::ZERO);
            }
        }

        #[test]
        fn test_clsag_correct_key_image(
            num_mixins in 1..17usize,
            seed in any::<[u8; 32]>(),
        ) {
            let mut rng: RngType = SeedableRng::from_seed(seed);
            let pseudo_output_blinding = Scalar::random(&mut rng);
            let params = ClsagTestParams::random(num_mixins, pseudo_output_blinding, &mut rng);

            let signature = params.sign(&mut rng).unwrap();
            let expected_key_image = KeyImage::from(&params.onetime_private_key);
            assert_eq!(signature.key_image, expected_key_image);
        }

        #[test]
        fn test_clsag_verify_accepts_valid(
            num_mixins in 1..17usize,
            seed in any::<[u8; 32]>(),
        ) {
            let mut rng: RngType = SeedableRng::from_seed(seed);
            let pseudo_output_blinding = Scalar::random(&mut rng);
            let params = ClsagTestParams::random(num_mixins, pseudo_output_blinding, &mut rng);

            let signature = params.sign(&mut rng).unwrap();

            let output_commitment = CompressedCommitment::new(
                params.value,
                params.pseudo_output_blinding,
                &params.generator,
            );

            assert!(signature.verify(&params.message, &params.ring, &output_commitment).is_ok());
        }

        #[test]
        fn test_clsag_verify_rejects_wrong_message(
            num_mixins in 1..17usize,
            seed in any::<[u8; 32]>(),
        ) {
            let mut rng: RngType = SeedableRng::from_seed(seed);
            let pseudo_output_blinding = Scalar::random(&mut rng);
            let params = ClsagTestParams::random(num_mixins, pseudo_output_blinding, &mut rng);

            let signature = params.sign(&mut rng).unwrap();

            let mut wrong_message = [0u8; 32];
            rng.fill_bytes(&mut wrong_message);

            let output_commitment = CompressedCommitment::new(
                params.value,
                params.pseudo_output_blinding,
                &params.generator,
            );

            match signature.verify(&wrong_message, &params.ring, &output_commitment) {
                Err(Error::InvalidSignature) => {}
                _ => panic!("Should reject wrong message"),
            }
        }

        #[test]
        fn test_clsag_verify_rejects_modified_key_image(
            num_mixins in 1..17usize,
            seed in any::<[u8; 32]>(),
        ) {
            let mut rng: RngType = SeedableRng::from_seed(seed);
            let pseudo_output_blinding = Scalar::random(&mut rng);
            let params = ClsagTestParams::random(num_mixins, pseudo_output_blinding, &mut rng);

            let mut signature = params.sign(&mut rng).unwrap();
            signature.key_image = KeyImage::from(rng.next_u64());

            let output_commitment = CompressedCommitment::new(
                params.value,
                params.pseudo_output_blinding,
                &params.generator,
            );

            match signature.verify(&params.message, &params.ring, &output_commitment) {
                Err(Error::InvalidSignature) => {}
                _ => panic!("Should reject modified key image"),
            }
        }

        #[test]
        fn test_clsag_verify_rejects_wrong_output_commitment(
            num_mixins in 1..17usize,
            seed in any::<[u8; 32]>(),
        ) {
            let mut rng: RngType = SeedableRng::from_seed(seed);
            let pseudo_output_blinding = Scalar::random(&mut rng);
            let params = ClsagTestParams::random(num_mixins, pseudo_output_blinding, &mut rng);

            let signature = params.sign(&mut rng).unwrap();

            let wrong_output_commitment = CompressedCommitment::new(
                rng.next_u64(),
                params.pseudo_output_blinding,
                &params.generator,
            );

            match signature.verify(&params.message, &params.ring, &wrong_output_commitment) {
                Err(Error::InvalidSignature) => {}
                _ => panic!("Should reject wrong output commitment"),
            }
        }

        #[test]
        #[cfg(feature = "prost")]
        fn test_clsag_encode_decode(
            num_mixins in 1..17usize,
            seed in any::<[u8; 32]>(),
        ) {
            let mut rng: RngType = SeedableRng::from_seed(seed);
            let pseudo_output_blinding = Scalar::random(&mut rng);
            let params = ClsagTestParams::random(num_mixins, pseudo_output_blinding, &mut rng);

            let signature = params.sign(&mut rng).unwrap();

            use bth_util_serial::prost::Message;

            let bytes = bth_util_serial::encode(&signature);
            assert_eq!(bytes.len(), signature.encoded_len());

            let recovered: Clsag = bth_util_serial::decode(&bytes).unwrap();
            assert_eq!(signature, recovered);
        }
    }

    #[test]
    fn test_clsag_value_not_conserved() {
        let mut rng = rand_core::OsRng;
        let pseudo_output_blinding = Scalar::random(&mut rng);
        let mut params = ClsagTestParams::random(5, pseudo_output_blinding, &mut rng);

        // Change the value so it doesn't match
        params.value = params.value.wrapping_add(1);

        match params.sign(&mut rng) {
            Err(Error::ValueNotConserved) => {}
            _ => panic!("Should fail with ValueNotConserved"),
        }
    }

    #[test]
    fn test_clsag_index_out_of_bounds() {
        let mut rng = rand_core::OsRng;
        let pseudo_output_blinding = Scalar::random(&mut rng);
        let mut params = ClsagTestParams::random(5, pseudo_output_blinding, &mut rng);

        params.real_index = 100; // Out of bounds

        match params.sign(&mut rng) {
            Err(Error::IndexOutOfBounds) => {}
            _ => panic!("Should fail with IndexOutOfBounds"),
        }
    }

    #[test]
    fn test_clsag_smaller_than_mlsag() {
        // CLSAG should produce smaller signatures than MLSAG
        // MLSAG: 2 * ring_size responses
        // CLSAG: ring_size responses
        let ring_size = 11;
        let clsag_responses = ring_size;
        let mlsag_responses = 2 * ring_size;

        // Each response is 32 bytes
        let clsag_response_bytes = clsag_responses * 32;
        let mlsag_response_bytes = mlsag_responses * 32;

        // CLSAG has an extra 32-byte commitment key image
        let clsag_overhead = 32;

        assert!(clsag_response_bytes + clsag_overhead < mlsag_response_bytes);
    }
}
