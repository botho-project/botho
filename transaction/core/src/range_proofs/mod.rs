// Copyright (c) 2018-2022 The Botho Foundation

//! Range proofs are used to prove that a set of committed values are all
//! in a well-defined range, without revealing the values.
//!
//! A range proof is relative to a Pedersen Generator. If a prover can construct
//! a range proof relative to one generator, they cannot construct a range proof
//! relative to another generator, if those generators are orthogonal.

extern crate alloc;
use alloc::vec::Vec;
use bth_crypto_ring_signature::PedersenGens;
use bulletproofs_og::{BulletproofGens, PedersenGens as BPPedersenGens, RangeProof};
use curve25519_dalek::{ristretto::CompressedRistretto, scalar::Scalar};
// bulletproofs-og is still built against curve25519-dalek 4, so scalars,
// compressed points, and generators must be converted across the dalek 4/5
// boundary. All conversions round-trip the canonical 32-byte encodings, so
// they are bit-exact (proofs and commitments are unchanged).
use curve25519_dalek_v4::{
    ristretto::{CompressedRistretto as CompressedRistrettoV4, RistrettoPoint as RistrettoPointV4},
    scalar::Scalar as ScalarV4,
};
use merlin::Transcript;
use rand_core::{CryptoRng, RngCore};

pub mod error;
use crate::domain_separators::BULLETPROOF_DOMAIN_TAG;
use error::Error;

lazy_static! {
    /// Generators (base points) for Bulletproofs.
    /// The `party_capacity` is the maximum number of values in one proof. It should
    /// be at least 2 * MAX_INPUTS + MAX_OUTPUTS, which allows for inputs, pseudo outputs, and outputs.
    pub static ref BP_GENERATORS: BulletproofGens =
        BulletproofGens::new(64, 64);
}

/// Create an aggregated 64-bit rangeproof for a set of values.
///
/// Creates a proof that each secret value is in the range [0,2^64).
///
/// # Arguments
/// `values` - Secret values that we want to prove are in [0,2^64).
/// `blindings` - Pedersen commitment blinding for each value.
/// `pedersen_generators` - Generators on which the commitments are based
/// `rng` - randomness
///
/// # Returns
/// The proof and the Pedersen commitments from `values` and `blindings` (padded
/// to a power of 2).
pub fn generate_range_proofs<T: RngCore + CryptoRng>(
    values: &[u64],
    blindings: &[Scalar],
    pedersen_generators: &PedersenGens,
    rng: &mut T,
) -> Result<(RangeProof, Vec<CompressedRistretto>), Error> {
    // Most of this comes directly from the example at
    // https://doc-internal.dalek.rs/bulletproofs/struct.RangeProof.html#example-1

    // Aggregated rangeproofs operate on sets of `m` values, where `m` must be a
    // power of 2. If the number of inputs is not a power of 2, pad them.
    let values_padded: Vec<u64> = resize_slice_to_pow2::<u64>(values)?;
    let blindings_padded: Vec<ScalarV4> = resize_slice_to_pow2::<Scalar>(blindings)?
        .iter()
        .map(scalar_to_v4)
        .collect();

    // Create a 64-bit RangeProof and corresponding commitments.
    let (proof, commitments) = RangeProof::prove_multiple_with_rng(
        &BP_GENERATORS,
        &convert_gens(pedersen_generators),
        &mut Transcript::new(BULLETPROOF_DOMAIN_TAG.as_ref()),
        &values_padded,
        &blindings_padded,
        64,
        rng,
    )?;
    let commitments = commitments
        .into_iter()
        .map(|c| CompressedRistretto(c.to_bytes()))
        .collect();
    Ok((proof, commitments))
}

/// Verifies an aggregated 64-bit RangeProof for the given value commitments.
///
/// Proves that the corresponding values lie in the range [0,2^64).
///
/// # Arguments
/// `range_proof` - A RangeProof.
/// `commitments` - Commitments to secret values that lie in the range [0,2^64).
/// `pedersen_generators` - Pedersen generators on which the commitments are
/// based `rng` - Randomness.
pub fn check_range_proofs<T: RngCore + CryptoRng>(
    range_proof: &RangeProof,
    commitments: &[CompressedRistretto],
    pedersen_generators: &PedersenGens,
    rng: &mut T,
) -> Result<(), Error> {
    // The length of `commitments` must be a power of 2. If not, resize it.
    let resized_commitments: Vec<CompressedRistrettoV4> =
        resize_slice_to_pow2::<CompressedRistretto>(commitments)?
            .iter()
            .map(|c| CompressedRistrettoV4(c.to_bytes()))
            .collect();
    range_proof
        .verify_multiple_with_rng(
            &BP_GENERATORS,
            &convert_gens(pedersen_generators),
            &mut Transcript::new(BULLETPROOF_DOMAIN_TAG.as_ref()),
            &resized_commitments,
            64,
            rng,
        )
        .map_err(Error::from)
}

/// Return a vector which is the slice plus enough of the final element such
/// that the length of the vector is a power of two.
///
/// If the next power of two is greater than the type's maximum value, an Error
/// is returned.
///
/// # Arguments
/// `slice` - (in) the slice with the data to use
fn resize_slice_to_pow2<T: Clone>(slice: &[T]) -> Result<Vec<T>, Error> {
    let len: usize = slice.len();
    if let Some(next_power_of_two) = len.checked_next_power_of_two() {
        let diff = next_power_of_two - len;
        let mut pow2_slice: Vec<T> = Vec::with_capacity(next_power_of_two);
        pow2_slice.extend_from_slice(slice);
        pow2_slice.resize(slice.len() + diff, slice[slice.len() - 1].clone());
        Ok(pow2_slice)
    } else {
        // The next power of two would exceed the maximum value of usize.
        Err(Error::ResizeError)
    }
}

/// Convert from the bth_crypto_ring_signature::PedersenGens to BPPedersenGens.
/// These types are identical, but we need a version of it in the lower-level
/// crate to break dependency on the bulletproofs crate. The base points cross
/// the dalek 4/5 boundary through their canonical compressed encodings.
fn convert_gens(src: &PedersenGens) -> BPPedersenGens {
    BPPedersenGens {
        B: point_to_v4(&src.B),
        B_blinding: point_to_v4(&src.B_blinding),
    }
}

/// Convert a (dalek 5) `RistrettoPoint` into the dalek 4 equivalent that
/// bulletproofs-og expects, via the canonical compressed encoding.
fn point_to_v4(src: &curve25519_dalek::ristretto::RistrettoPoint) -> RistrettoPointV4 {
    CompressedRistrettoV4(src.compress().to_bytes())
        .decompress()
        .expect("compressing a valid point always yields a decompressible encoding")
}

/// Convert a (dalek 5) `Scalar` into the dalek 4 equivalent that
/// bulletproofs-og expects. The encoding of a `Scalar` is always canonical,
/// so this conversion is infallible and bit-exact.
fn scalar_to_v4(src: &Scalar) -> ScalarV4 {
    Option::from(ScalarV4::from_canonical_bytes(src.to_bytes()))
        .expect("Scalar encodings are always canonical")
}

/// Tests for the range_proofs module.
#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::ring_signature::generators;
    use bth_util_test_helper::get_seeded_rng;
    use curve25519_dalek::ristretto::RistrettoPoint;

    fn generate_and_check(values: Vec<u64>, blindings: Vec<Scalar>) {
        let mut rng = get_seeded_rng();
        let (proof, commitments) =
            generate_range_proofs(&values, &blindings, &generators(0), &mut rng).unwrap();

        match check_range_proofs(&proof, &commitments, &generators(0), &mut rng) {
            Ok(_) => {} // This is expected.
            Err(e) => panic!("{e:?}"),
        }
    }

    #[test]
    fn test_pow2_number_of_inputs() {
        let mut rng = get_seeded_rng();
        let vals: Vec<u64> = (0..2).map(|_| rng.next_u64()).collect();
        let blindings: Vec<Scalar> = vals
            .iter()
            .map(|_| bth_crypto_ring_signature::compat::random_scalar(&mut rng))
            .collect();
        generate_and_check(vals, blindings);
    }

    #[test]
    fn test_not_pow2_number_of_inputs() {
        let mut rng = get_seeded_rng();
        let vals: Vec<u64> = (0..9).map(|_| rng.next_u64()).collect();
        let blindings: Vec<Scalar> = vals
            .iter()
            .map(|_| bth_crypto_ring_signature::compat::random_scalar(&mut rng))
            .collect();
        generate_and_check(vals, blindings);
    }

    #[test]
    // `check_range_proofs` should return an error if the commitments do not agree
    // with the proof.
    fn test_check_range_proofs_rejects_wrong_commitments() {
        let mut rng = get_seeded_rng();

        let num_values: usize = 4;
        let values: Vec<u64> = (0..num_values).map(|_| rng.next_u64()).collect();
        let blindings: Vec<Scalar> = (0..num_values)
            .map(|_| bth_crypto_ring_signature::compat::random_scalar(&mut rng))
            .collect();
        let (proof, commitments) =
            generate_range_proofs(&values, &blindings, &generators(0), &mut rng).unwrap();

        // Modify a commitment.
        let mut wrong_commitments = commitments;
        wrong_commitments[0] = bth_crypto_ring_signature::compat::random_point(&mut rng).compress();

        match check_range_proofs(&proof, &wrong_commitments, &generators(0), &mut rng) {
            Ok(_) => panic!(),
            Err(_e) => {} // This is expected.
        }
    }
}
