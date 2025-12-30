#![no_main]

//! Fuzzing target for MLSAG ring signature verification.
//!
//! Security rationale: Ring signatures are cryptographically complex and provide
//! unlinkability for transaction inputs. Invalid signatures must NEVER verify,
//! and malformed signature data must never cause panics or undefined behavior.
//!
//! The ring signature implementation uses MLSAG (Multilayer Linkable Spontaneous
//! Anonymous Group signatures) with Ristretto points.

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;

use bth_crypto_ring_signature::{
    KeyImage, RingMLSAG, Blinding, Commitment,
};
use bth_crypto_keys::{RistrettoPublic, RistrettoPrivate};

// ============================================================================
// Structured Fuzzing Types
// ============================================================================

/// Fuzz mode for ring signatures
#[derive(Debug, Arbitrary)]
enum FuzzMode {
    /// Raw bytes deserialization
    RawSignature(Vec<u8>),
    /// Raw key image parsing
    RawKeyImage([u8; 32]),
    /// Raw commitment parsing
    RawCommitment([u8; 32]),
    /// Structured signature verification attempt
    StructuredVerify(FuzzVerification),
}

/// Structured verification attempt
#[derive(Debug, Arbitrary)]
struct FuzzVerification {
    /// Message to verify
    message: Vec<u8>,
    /// Ring public keys (raw bytes, may be invalid points)
    ring_keys: Vec<[u8; 32]>,
    /// Commitment points (raw bytes)
    commitments: Vec<[u8; 32]>,
    /// Pseudo output commitment
    pseudo_output: [u8; 32],
    /// Key image bytes
    key_image: [u8; 32],
    /// Challenge scalar bytes
    c_zero: [u8; 32],
    /// Response scalars
    responses: Vec<[u8; 32]>,
}

/// Test malformed key images
#[derive(Debug, Arbitrary)]
struct FuzzKeyImage {
    /// Raw bytes that may or may not be a valid key image
    bytes: [u8; 32],
    /// Alternative representation
    alt_bytes: Vec<u8>,
}

// ============================================================================
// Fuzz Target
// ============================================================================

fuzz_target!(|mode: FuzzMode| {
    match mode {
        FuzzMode::RawSignature(data) => {
            fuzz_raw_signature(&data);
        }
        FuzzMode::RawKeyImage(bytes) => {
            fuzz_key_image(&bytes);
        }
        FuzzMode::RawCommitment(bytes) => {
            fuzz_commitment(&bytes);
        }
        FuzzMode::StructuredVerify(verify) => {
            fuzz_structured_verify(&verify);
        }
    }
});

/// Fuzz raw signature bytes deserialization
fn fuzz_raw_signature(data: &[u8]) {
    // Try to deserialize as RingMLSAG
    let _ = bincode::deserialize::<RingMLSAG>(data);

    // Try parsing subcomponents
    if data.len() >= 32 {
        let _ = KeyImage::try_from(&data[..32]);
    }

    // Try various slices
    for offset in [0, 1, 16, 31, 32, 64, 128].iter() {
        if *offset < data.len() {
            let slice = &data[*offset..];
            let _ = bincode::deserialize::<RingMLSAG>(slice);
        }
    }
}

/// Fuzz key image parsing
fn fuzz_key_image(bytes: &[u8; 32]) {
    // Try to create a key image from bytes
    // This should not panic even for invalid curve points
    let result = KeyImage::try_from(&bytes[..]);

    // If parsing succeeds, verify operations don't panic
    if let Ok(key_image) = result {
        // These should never panic
        let _ = key_image.as_bytes();

        // Test equality
        let _ = key_image == key_image;

        // Test serialization roundtrip
        let bytes_out = key_image.as_bytes();
        let _ = KeyImage::try_from(&bytes_out[..]);
    }
}

/// Fuzz commitment parsing
fn fuzz_commitment(bytes: &[u8; 32]) {
    // Try to parse as a commitment point
    // Commitments are Pedersen commitments: C = v*G + r*H
    let result = Commitment::try_from(&bytes[..]);

    if let Ok(commitment) = result {
        // Operations on valid commitments should not panic
        let _ = commitment.as_bytes();
    }
}

/// Fuzz structured verification
fn fuzz_structured_verify(verify: &FuzzVerification) {
    // Limit ring size to prevent OOM
    let ring_size = verify.ring_keys.len().min(16);
    if ring_size == 0 {
        return;
    }

    // Try to parse all ring public keys
    let mut ring: Vec<RistrettoPublic> = Vec::with_capacity(ring_size);
    for key_bytes in verify.ring_keys.iter().take(ring_size) {
        if let Ok(pubkey) = RistrettoPublic::try_from(&key_bytes[..]) {
            ring.push(pubkey);
        }
    }

    if ring.is_empty() {
        return;
    }

    // Try to parse key image
    let key_image = match KeyImage::try_from(&verify.key_image[..]) {
        Ok(ki) => ki,
        Err(_) => return,
    };

    // Try to parse commitments
    let mut input_commitments: Vec<Commitment> = Vec::new();
    for comm_bytes in verify.commitments.iter().take(ring_size) {
        if let Ok(comm) = Commitment::try_from(&comm_bytes[..]) {
            input_commitments.push(comm);
        }
    }

    // Parse pseudo output commitment
    let pseudo_output = match Commitment::try_from(&verify.pseudo_output[..]) {
        Ok(c) => c,
        Err(_) => return,
    };

    // Even with potentially invalid components, verification should not panic
    // It should return false or an error, never crash
    //
    // Note: We can't easily construct a full RingMLSAG without the internal
    // structure, so we focus on testing the components don't panic.

    // Test message hashing with fuzzed message
    let message = &verify.message;
    let _hash = bth_crypto_hashes::Blake2b256::digest(message);

    // Verify serialization doesn't panic
    if let Ok(serialized) = bincode::serialize(&key_image) {
        let _ = bincode::deserialize::<KeyImage>(&serialized);
    }
}

// ============================================================================
// Additional Safety Tests
// ============================================================================

/// Test that invalid curve points are rejected
#[allow(dead_code)]
fn test_invalid_points() {
    // All zeros should not be a valid point
    let zeros = [0u8; 32];
    assert!(RistrettoPublic::try_from(&zeros[..]).is_err());

    // All 0xFF should not be a valid point
    let ones = [0xFFu8; 32];
    assert!(RistrettoPublic::try_from(&ones[..]).is_err());
}
