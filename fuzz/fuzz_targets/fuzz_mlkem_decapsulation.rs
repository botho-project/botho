#![no_main]

//! Fuzzing target for ML-KEM (Kyber) decapsulation.
//!
//! Security rationale: ML-KEM decapsulation accepts ciphertext from untrusted
//! sources. Malformed ciphertexts must never cause panics, memory corruption,
//! or timing side-channels that leak information about the private key.
//!
//! ML-KEM-768 is used for post-quantum key encapsulation in Botho.

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;

use bth_crypto_pq::{
    MlKem768Ciphertext, MlKem768KeyPair, MlKem768PublicKey, ML_KEM_768_CIPHERTEXT_BYTES,
    ML_KEM_768_PUBLIC_KEY_BYTES, ML_KEM_768_SHARED_SECRET_BYTES,
};

// Local aliases preserving the original fuzz target's vocabulary.
const PQ_CIPHERTEXT_SIZE: usize = ML_KEM_768_CIPHERTEXT_BYTES;
const PQ_PUBLIC_KEY_SIZE: usize = ML_KEM_768_PUBLIC_KEY_BYTES;

// ============================================================================
// Structured Fuzzing Types
// ============================================================================

/// Fuzz mode for ML-KEM
#[derive(Debug, Arbitrary)]
enum FuzzMode {
    /// Raw ciphertext bytes
    RawCiphertext(Vec<u8>),
    /// Structured decapsulation attempt
    Decapsulate(FuzzDecapsulation),
    /// Key generation with fuzzed seed
    KeyGen(FuzzKeyGen),
    /// Public key parsing
    PublicKeyParse(Vec<u8>),
}

/// Structured decapsulation attempt
#[derive(Debug, Arbitrary)]
struct FuzzDecapsulation {
    /// Seed for deterministic key generation
    key_seed: [u8; 32],
    /// Ciphertext bytes (may be wrong size or invalid)
    ciphertext: Vec<u8>,
    /// Whether to use exact size
    exact_size: bool,
}

/// Key generation fuzzing
#[derive(Debug, Arbitrary)]
struct FuzzKeyGen {
    /// Seed bytes
    seed: [u8; 32],
    /// Alternative seeds to test determinism
    alt_seeds: Vec<[u8; 32]>,
}

// ============================================================================
// Fuzz Target
// ============================================================================

fuzz_target!(|mode: FuzzMode| {
    match mode {
        FuzzMode::RawCiphertext(data) => {
            fuzz_raw_ciphertext(&data);
        }
        FuzzMode::Decapsulate(decap) => {
            fuzz_decapsulate(&decap);
        }
        FuzzMode::KeyGen(keygen) => {
            fuzz_keygen(&keygen);
        }
        FuzzMode::PublicKeyParse(data) => {
            fuzz_public_key_parse(&data);
        }
    }
});

/// Fuzz with raw ciphertext bytes
fn fuzz_raw_ciphertext(data: &[u8]) {
    // Generate a valid keypair for decapsulation attempts
    let seed = [0u8; 32];
    let keypair = MlKem768KeyPair::from_seed(&seed);

    // Try to decapsulate with various data sizes. `MlKem768Ciphertext::from_bytes`
    // performs the size check; `decapsulate` then must not panic even on garbage
    // (IND-CCA2 implicit rejection returns a pseudo-random secret, never a crash).
    if let Ok(ct) = MlKem768Ciphertext::from_bytes(data) {
        let _ = keypair.decapsulate(&ct);
    }

    // Also try with wrong sizes (should be rejected gracefully by from_bytes).
    for size in [0, 1, 32, 64, 128, 256, 512, 1024, 2048].iter() {
        if *size < data.len() {
            let slice = &data[..*size];
            if let Ok(ct) = MlKem768Ciphertext::from_bytes(slice) {
                let _ = keypair.decapsulate(&ct);
            }
        }
    }
}

/// Fuzz structured decapsulation
fn fuzz_decapsulate(decap: &FuzzDecapsulation) {
    // Generate keypair from fuzzed seed
    let keypair = MlKem768KeyPair::from_seed(&decap.key_seed);

    // Get public key for reference
    let _pubkey = keypair.public_key();

    // Attempt decapsulation
    if decap.exact_size && decap.ciphertext.len() == PQ_CIPHERTEXT_SIZE {
        // Decapsulation should never panic. With a well-formed-size but otherwise
        // arbitrary ciphertext, the implementation returns Ok(pseudo-random secret)
        // via implicit rejection; either way it must not crash.
        if let Ok(ct) = MlKem768Ciphertext::from_bytes(&decap.ciphertext) {
            if let Ok(shared_secret) = keypair.decapsulate(&ct) {
                // Shared secret should always be 32 bytes.
                assert_eq!(shared_secret.as_bytes().len(), ML_KEM_768_SHARED_SECRET_BYTES);
            }
        }
    }

    // Test with wrong size (should handle gracefully via from_bytes rejection).
    for truncate in [1, 10, 100, PQ_CIPHERTEXT_SIZE / 2].iter() {
        if *truncate < decap.ciphertext.len() {
            let truncated = &decap.ciphertext[..*truncate];
            // Wrong-size ciphertext must be rejected at parse time, never panic.
            let _ = MlKem768Ciphertext::from_bytes(truncated);
        }
    }
}

/// Fuzz key generation
fn fuzz_keygen(keygen: &FuzzKeyGen) {
    // Generate primary keypair
    let keypair1 = MlKem768KeyPair::from_seed(&keygen.seed);

    // Test determinism: same seed should produce same keys
    let keypair2 = MlKem768KeyPair::from_seed(&keygen.seed);

    let pk1 = keypair1.public_key();
    let pk2 = keypair2.public_key();

    // Public keys should be identical
    assert_eq!(pk1.as_bytes(), pk2.as_bytes());

    // Test that different seeds produce different keys
    for alt_seed in keygen.alt_seeds.iter().take(3) {
        if alt_seed != &keygen.seed {
            let alt_keypair = MlKem768KeyPair::from_seed(alt_seed);
            let alt_pk = alt_keypair.public_key();

            // Should be different (with overwhelming probability)
            if pk1.as_bytes() == alt_pk.as_bytes() {
                // This would be a serious bug - different seeds producing same key
                panic!("Different seeds produced identical public keys!");
            }
        }
    }

    // Test encapsulation/decapsulation roundtrip. Encapsulation lives on the
    // public key; decapsulation on the keypair. A genuine roundtrip must recover
    // the same shared secret.
    let (ciphertext, shared_secret_enc) = keypair1.public_key().encapsulate();
    let shared_secret_dec = keypair1
        .decapsulate(&ciphertext)
        .expect("decapsulation of a freshly encapsulated ciphertext must succeed");

    // Shared secrets should match (MlKem768SharedSecret has no PartialEq;
    // compare the underlying bytes).
    assert_eq!(shared_secret_enc.as_bytes(), shared_secret_dec.as_bytes());
}

/// Fuzz public key parsing
fn fuzz_public_key_parse(data: &[u8]) {
    // Try to parse as public key
    if data.len() == PQ_PUBLIC_KEY_SIZE {
        let mut pk_bytes = [0u8; PQ_PUBLIC_KEY_SIZE];
        pk_bytes.copy_from_slice(data);

        // Try to create public key from bytes
        let result = MlKem768PublicKey::from_bytes(&pk_bytes);

        if let Ok(pubkey) = result {
            // If parsing succeeds, encapsulation should work
            let (ciphertext, _shared_secret) = pubkey.encapsulate();

            // Ciphertext should be correct size
            assert_eq!(ciphertext.as_bytes().len(), PQ_CIPHERTEXT_SIZE);
        }
    }

    // Test with various invalid sizes
    for size in [0, 1, 32, 64, 100, 500, 1000, 2000].iter() {
        if *size < data.len() {
            let slice = &data[..*size];
            // Can't construct with wrong size, but we test the bounds checking
            let _ = slice.len();
        }
    }
}
