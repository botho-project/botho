#![no_main]

//! Fuzzing target for CLSAG ring signature sign/verify.
//!
//! Security rationale: CLSAG signatures are cryptographically complex and provide
//! unlinkability for transaction inputs. Invalid signatures must NEVER verify,
//! and the implementation must handle malformed data gracefully.
//!
//! CLSAG is an improvement over MLSAG that reduces signature size by ~50%.

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use rand::{RngCore, SeedableRng};
use rand_chacha::ChaCha20Rng;

use bth_crypto_keys::{CompressedRistrettoPublic, RistrettoPrivate, RistrettoPublic};
use bth_crypto_ring_signature::{
    generators, Clsag, CompressedCommitment, CurveScalar, KeyImage, ReducedTxOut, Scalar,
};
use bth_util_from_random::FromRandom;
use subtle::CtOption;

// ============================================================================
// Structured Fuzzing Types
// ============================================================================

/// Fuzz mode for CLSAG signatures
#[derive(Debug, Arbitrary)]
enum FuzzMode {
    /// Generate valid ring and test signing/verification
    ValidSign(ValidSignFuzz),
    /// Test verification with malformed signature
    MalformedSig(MalformedSigFuzz),
    /// Test key image consistency
    KeyImageConsistency(KeyImageFuzz),
    /// Test with modified ring
    ModifiedRing(ModifiedRingFuzz),
    /// Raw bytes for signature components
    RawBytes(RawBytesFuzz),
}

/// Valid signing test
#[derive(Debug, Arbitrary)]
struct ValidSignFuzz {
    /// Seed for RNG
    seed: [u8; 32],
    /// Number of mixins (0-15)
    num_mixins: u8,
    /// Value to transact
    value: u64,
    /// Generator seed
    generator_seed: u64,
}

/// Malformed signature test
#[derive(Debug, Arbitrary)]
struct MalformedSigFuzz {
    /// Seed for RNG
    seed: [u8; 32],
    /// Which component to corrupt
    corruption: SigCorruption,
}

#[derive(Debug, Arbitrary)]
enum SigCorruption {
    /// Corrupt c_zero challenge
    CorruptCZero([u8; 32]),
    /// Corrupt a response scalar
    CorruptResponse { index: u8, value: [u8; 32] },
    /// Corrupt key image
    CorruptKeyImage([u8; 32]),
    /// Corrupt commitment key image
    CorruptCommitmentKeyImage([u8; 32]),
    /// Wrong number of responses
    WrongResponseCount(u8),
}

/// Key image consistency test
#[derive(Debug, Arbitrary)]
struct KeyImageFuzz {
    /// Private key seed
    private_key_seed: [u8; 32],
    /// Multiple messages to sign
    messages: Vec<Vec<u8>>,
}

/// Modified ring test
#[derive(Debug, Arbitrary)]
struct ModifiedRingFuzz {
    /// Seed for RNG
    seed: [u8; 32],
    /// Type of modification
    modification: RingModification,
}

#[derive(Debug, Arbitrary)]
enum RingModification {
    /// Replace a ring member
    ReplaceMember { index: u8, new_seed: [u8; 32] },
    /// Use all same members
    DuplicateAll,
    /// Empty ring (should fail gracefully)
    EmptyRing,
}

/// Raw bytes fuzzing
#[derive(Debug, Arbitrary)]
struct RawBytesFuzz {
    /// Key image bytes
    key_image: [u8; 32],
    /// Commitment bytes
    commitment: [u8; 32],
    /// Public key bytes
    public_key: [u8; 32],
    /// Scalar bytes
    scalar: [u8; 32],
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Create a valid ring for testing
fn create_test_ring(
    rng: &mut ChaCha20Rng,
    num_mixins: usize,
    generator_seed: u64,
) -> (
    Vec<ReducedTxOut>,
    usize,
    RistrettoPrivate,
    u64,
    Scalar,
    Scalar,
    bth_crypto_ring_signature::PedersenGens,
) {
    let generator = generators(generator_seed);
    let mut ring: Vec<ReducedTxOut> = Vec::new();

    // Add mixins
    for _ in 0..num_mixins {
        let public_key = CompressedRistrettoPublic::from_random(rng);
        let target_key = CompressedRistrettoPublic::from_random(rng);
        let mixin_value = rng.next_u64();
        let mixin_blinding = Scalar::random(rng);
        let commitment = CompressedCommitment::new(mixin_value, mixin_blinding, &generator);
        ring.push(ReducedTxOut {
            public_key,
            target_key,
            commitment,
        });
    }

    // Create the real input
    let onetime_private_key = RistrettoPrivate::from_random(rng);
    let value = rng.next_u64();
    let blinding = Scalar::random(rng);
    let commitment = CompressedCommitment::new(value, blinding, &generator);

    let reduced_tx_out = ReducedTxOut {
        target_key: CompressedRistrettoPublic::from(RistrettoPublic::from(&onetime_private_key)),
        public_key: CompressedRistrettoPublic::from_random(rng),
        commitment,
    };

    let real_index = rng.next_u64() as usize % (num_mixins + 1);
    ring.insert(real_index, reduced_tx_out);

    let pseudo_output_blinding = Scalar::random(rng);

    (
        ring,
        real_index,
        onetime_private_key,
        value,
        blinding,
        pseudo_output_blinding,
        generator,
    )
}


// ============================================================================
// Fuzz Target
// ============================================================================

fuzz_target!(|mode: FuzzMode| {
    match mode {
        FuzzMode::ValidSign(valid) => {
            fuzz_valid_sign(&valid);
        }
        FuzzMode::MalformedSig(malformed) => {
            fuzz_malformed_sig(&malformed);
        }
        FuzzMode::KeyImageConsistency(ki) => {
            fuzz_key_image_consistency(&ki);
        }
        FuzzMode::ModifiedRing(modified) => {
            fuzz_modified_ring(&modified);
        }
        FuzzMode::RawBytes(raw) => {
            fuzz_raw_bytes(&raw);
        }
    }
});

/// Test valid signing and verification
fn fuzz_valid_sign(valid: &ValidSignFuzz) {
    let mut rng = ChaCha20Rng::from_seed(valid.seed);
    let num_mixins = (valid.num_mixins as usize % 16).max(1);

    let (ring, real_index, onetime_private_key, value, blinding, pseudo_output_blinding, generator) =
        create_test_ring(&mut rng, num_mixins, valid.generator_seed);

    let message = [0u8; 32];

    // Sign
    let signature = match Clsag::sign(
        &message,
        &ring,
        real_index,
        &onetime_private_key,
        value,
        &blinding,
        &pseudo_output_blinding,
        &generator,
        &mut rng,
    ) {
        Ok(sig) => sig,
        Err(_) => return, // Some edge cases may fail
    };

    // Verify output commitment
    let output_commitment = CompressedCommitment::new(value, pseudo_output_blinding, &generator);

    // Valid signature must verify
    assert!(
        signature.verify(&message, &ring, &output_commitment).is_ok(),
        "Valid CLSAG signature must verify"
    );

    // Key image must match
    let expected_key_image = KeyImage::from(&onetime_private_key);
    assert_eq!(
        signature.key_image, expected_key_image,
        "Key image must be deterministic"
    );

    // Wrong message must fail
    let wrong_message = [0xFF; 32];
    assert!(
        signature
            .verify(&wrong_message, &ring, &output_commitment)
            .is_err(),
        "Wrong message must fail"
    );

    // Response count must match ring size
    assert_eq!(signature.responses.len(), ring.len());
}

/// Test malformed signature handling
fn fuzz_malformed_sig(malformed: &MalformedSigFuzz) {
    let mut rng = ChaCha20Rng::from_seed(malformed.seed);

    let (ring, real_index, onetime_private_key, value, blinding, pseudo_output_blinding, generator) =
        create_test_ring(&mut rng, 5, 12345);

    let message = [0u8; 32];

    let mut signature = match Clsag::sign(
        &message,
        &ring,
        real_index,
        &onetime_private_key,
        value,
        &blinding,
        &pseudo_output_blinding,
        &generator,
        &mut rng,
    ) {
        Ok(sig) => sig,
        Err(_) => return,
    };

    let output_commitment = CompressedCommitment::new(value, pseudo_output_blinding, &generator);

    // Apply corruption
    match &malformed.corruption {
        SigCorruption::CorruptCZero(bytes) => {
            let ct_opt: CtOption<Scalar> = Scalar::from_canonical_bytes(*bytes);
            if ct_opt.is_some().into() {
                signature.c_zero = CurveScalar::from(ct_opt.unwrap());
            }
        }
        SigCorruption::CorruptResponse { index, value } => {
            let idx = (*index as usize) % signature.responses.len().max(1);
            if idx < signature.responses.len() {
                let ct_opt: CtOption<Scalar> = Scalar::from_canonical_bytes(*value);
                if ct_opt.is_some().into() {
                    signature.responses[idx] = CurveScalar::from(ct_opt.unwrap());
                }
            }
        }
        SigCorruption::CorruptKeyImage(bytes) => {
            signature.key_image = KeyImage { point: curve25519_dalek::ristretto::CompressedRistretto(*bytes) };
        }
        SigCorruption::CorruptCommitmentKeyImage(bytes) => {
            signature.commitment_key_image = KeyImage { point: curve25519_dalek::ristretto::CompressedRistretto(*bytes) };
        }
        SigCorruption::WrongResponseCount(count) => {
            let new_len = (*count as usize).min(100);
            signature.responses.truncate(new_len);
        }
    }

    // Verification should not panic, but should likely fail
    let _ = signature.verify(&message, &ring, &output_commitment);
}

/// Test key image consistency across multiple signatures
fn fuzz_key_image_consistency(ki: &KeyImageFuzz) {
    let mut rng = ChaCha20Rng::from_seed(ki.private_key_seed);

    // Create a fixed private key
    let onetime_private_key = RistrettoPrivate::from_random(&mut rng);
    let expected_key_image = KeyImage::from(&onetime_private_key);

    // Sign multiple messages - key image should always be the same
    for (i, msg) in ki.messages.iter().take(5).enumerate() {
        let message = &msg[..msg.len().min(100)];
        let mut msg_rng = ChaCha20Rng::from_seed([i as u8; 32]);

        let (mut ring, _, _, value, blinding, pseudo_output_blinding, generator) =
            create_test_ring(&mut msg_rng, 5, 12345);

        // Replace one member with our key
        ring[0].target_key =
            CompressedRistrettoPublic::from(RistrettoPublic::from(&onetime_private_key));
        ring[0].commitment = CompressedCommitment::new(value, blinding, &generator);

        let signature = match Clsag::sign(
            message,
            &ring,
            0,
            &onetime_private_key,
            value,
            &blinding,
            &pseudo_output_blinding,
            &generator,
            &mut msg_rng,
        ) {
            Ok(sig) => sig,
            Err(_) => continue,
        };

        // Key image must always match
        assert_eq!(
            signature.key_image, expected_key_image,
            "Key image must be consistent across signatures"
        );
    }
}

/// Test modified ring handling
fn fuzz_modified_ring(modified: &ModifiedRingFuzz) {
    let mut rng = ChaCha20Rng::from_seed(modified.seed);

    let (ring, real_index, onetime_private_key, value, blinding, pseudo_output_blinding, generator) =
        create_test_ring(&mut rng, 5, 12345);

    let message = [0u8; 32];

    let signature = match Clsag::sign(
        &message,
        &ring,
        real_index,
        &onetime_private_key,
        value,
        &blinding,
        &pseudo_output_blinding,
        &generator,
        &mut rng,
    ) {
        Ok(sig) => sig,
        Err(_) => return,
    };

    let output_commitment = CompressedCommitment::new(value, pseudo_output_blinding, &generator);

    // Modify the ring
    let mut modified_ring = ring.clone();
    match &modified.modification {
        RingModification::ReplaceMember { index, new_seed } => {
            let idx = (*index as usize) % modified_ring.len();
            let mut new_rng = ChaCha20Rng::from_seed(*new_seed);
            modified_ring[idx].target_key = CompressedRistrettoPublic::from_random(&mut new_rng);
        }
        RingModification::DuplicateAll => {
            if !modified_ring.is_empty() {
                let first = modified_ring[0].clone();
                for member in modified_ring.iter_mut() {
                    *member = first.clone();
                }
            }
        }
        RingModification::EmptyRing => {
            modified_ring.clear();
        }
    }

    // Verification with modified ring should not panic
    let _ = signature.verify(&message, &modified_ring, &output_commitment);
}

/// Test raw bytes parsing
fn fuzz_raw_bytes(raw: &RawBytesFuzz) {
    // Try to parse key image - should not panic
    let _ki_result = curve25519_dalek::ristretto::CompressedRistretto(raw.key_image).decompress();

    // Try to parse commitment
    let _ = bth_crypto_ring_signature::Commitment::try_from(&raw.commitment[..]);

    // Try to parse public key
    let _ = RistrettoPublic::try_from(&raw.public_key[..]);

    // Try to parse scalar
    let ct_opt: CtOption<Scalar> = Scalar::from_canonical_bytes(raw.scalar);
    let _ = ct_opt.is_some();
}
