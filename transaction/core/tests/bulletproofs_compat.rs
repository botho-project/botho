//! Golden bytes captured using the unmodified upstream fork at
//! 9abfdc054d9ba65f1e185ea1e6eff3947ce879dc before the zeroize migration.
use bth_transaction_core::{
    range_proofs::{check_range_proofs, generate_range_proofs},
    ring_signature::generators,
};
use bth_util_test_helper::get_seeded_rng;
use bulletproofs_og::RangeProof;
use curve25519_dalek::scalar::Scalar;

#[test]
fn pinned_fork_range_proof_bytes() {
    for count in [1usize, 3, 8] {
        let values: Vec<u64> = (0..count).map(|i| [0, u64::MAX, 42][i % 3]).collect();
        let blindings: Vec<Scalar> = (0..count).map(|i| Scalar::from(i as u64 + 17)).collect();
        let mut rng = get_seeded_rng();
        let (proof, commitments) =
            generate_range_proofs(&values, &blindings, &generators(0), &mut rng).unwrap();
        let mut bytes = proof.to_bytes();
        for commitment in &commitments {
            bytes.extend_from_slice(commitment.as_bytes());
        }
        let path = format!(
            "{}/tests/fixtures/range-{count}.bin",
            env!("CARGO_MANIFEST_DIR")
        );
        if std::env::var_os("CAPTURE_PINNED_FIXTURES").is_some() {
            std::fs::write(&path, &bytes).unwrap();
        }
        let expected = std::fs::read(&path).unwrap();
        assert_eq!(bytes, expected, "proof and commitments for {count} values");
        let golden = RangeProof::from_bytes(&expected[..proof.to_bytes().len()]).unwrap();
        check_range_proofs(&golden, &commitments, &generators(0), &mut rng).unwrap();
        check_range_proofs(&proof, &commitments, &generators(0), &mut rng).unwrap();
        let mut invalid = proof.to_bytes();
        invalid[100] ^= 1;
        if let Ok(invalid) = RangeProof::from_bytes(&invalid) {
            assert!(check_range_proofs(&invalid, &commitments, &generators(0), &mut rng).is_err());
        }
        let mut wrong_commitments = commitments.clone();
        wrong_commitments[0] = (curve25519_dalek::constants::RISTRETTO_BASEPOINT_POINT
            * Scalar::from(5u64))
        .compress();
        assert!(check_range_proofs(&golden, &wrong_commitments, &generators(0), &mut rng).is_err());
    }
}
