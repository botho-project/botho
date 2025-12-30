#![no_main]

use libfuzzer_sys::fuzz_target;

use bth_crypto_pq::{
    MlKem768Ciphertext, MlKem768PublicKey, MlDsa65PublicKey, MlDsa65Signature,
    ML_KEM_768_CIPHERTEXT_BYTES, ML_KEM_768_PUBLIC_KEY_BYTES,
    ML_DSA_65_PUBLIC_KEY_BYTES, ML_DSA_65_SIGNATURE_BYTES,
};

// Fuzz target for PQ cryptographic type parsing.
//
// Tests that malformed PQ key material cannot cause panics or undefined behavior.
// These types are parsed from untrusted network data in transaction outputs and inputs.
//
// Security rationale:
// - ML-KEM public keys (1184 bytes) are in every PQ address
// - ML-KEM ciphertexts (1088 bytes) are in every PQ output
// - ML-DSA public keys (1952 bytes) are in every PQ output
// - ML-DSA signatures (3309 bytes) are in every PQ input
//
// An attacker could craft malicious data to exploit parsing vulnerabilities.
fuzz_target!(|data: &[u8]| {
    // Test ML-KEM public key parsing
    // Should gracefully fail for wrong sizes or invalid points
    let _ = MlKem768PublicKey::from_bytes(data);

    // Test with exact expected size (might trigger different code paths)
    if data.len() >= ML_KEM_768_PUBLIC_KEY_BYTES {
        let _ = MlKem768PublicKey::from_bytes(&data[..ML_KEM_768_PUBLIC_KEY_BYTES]);
    }

    // Test ML-KEM ciphertext parsing
    let _ = MlKem768Ciphertext::from_bytes(data);

    if data.len() >= ML_KEM_768_CIPHERTEXT_BYTES {
        let _ = MlKem768Ciphertext::from_bytes(&data[..ML_KEM_768_CIPHERTEXT_BYTES]);
    }

    // Test ML-DSA public key parsing
    let _ = MlDsa65PublicKey::from_bytes(data);

    if data.len() >= ML_DSA_65_PUBLIC_KEY_BYTES {
        let _ = MlDsa65PublicKey::from_bytes(&data[..ML_DSA_65_PUBLIC_KEY_BYTES]);
    }

    // Test ML-DSA signature parsing
    let _ = MlDsa65Signature::from_bytes(data);

    if data.len() >= ML_DSA_65_SIGNATURE_BYTES {
        let _ = MlDsa65Signature::from_bytes(&data[..ML_DSA_65_SIGNATURE_BYTES]);
    }

    // If we successfully parse a public key and signature, test verification
    // (verification with random data should fail gracefully, not panic)
    if data.len() >= ML_DSA_65_PUBLIC_KEY_BYTES + ML_DSA_65_SIGNATURE_BYTES {
        let pk_bytes = &data[..ML_DSA_65_PUBLIC_KEY_BYTES];
        let sig_bytes = &data[ML_DSA_65_PUBLIC_KEY_BYTES..ML_DSA_65_PUBLIC_KEY_BYTES + ML_DSA_65_SIGNATURE_BYTES];

        if let (Ok(pk), Ok(sig)) = (
            MlDsa65PublicKey::from_bytes(pk_bytes),
            MlDsa65Signature::from_bytes(sig_bytes),
        ) {
            // Verification should return Err, not panic
            let _ = pk.verify(b"test message", &sig);
        }
    }
});
