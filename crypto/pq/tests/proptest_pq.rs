//! Property-based tests for post-quantum cryptographic primitives.
//!
//! These tests verify that the PQ crypto implementations satisfy essential
//! mathematical and security properties for all inputs, not just fixed test
//! vectors.

use bth_crypto_pq::{
    derive_onetime_sig_keypair, derive_pq_keys_from_seed, MlDsa65KeyPair, MlKem768KeyPair,
};
use proptest::prelude::*;

// ============================================================================
// ML-KEM Property Tests
// ============================================================================

proptest! {
    /// Property: KEM encapsulation/decapsulation is always correct.
    /// For any keypair, encapsulating to the public key and decapsulating with
    /// the secret key always recovers the same shared secret.
    #[test]
    fn prop_kem_roundtrip(seed in prop::array::uniform32(any::<u8>())) {
        let keypair = MlKem768KeyPair::from_seed(&seed);
        let (ciphertext, shared_secret) = keypair.public_key().encapsulate();
        let decapsulated = keypair.decapsulate(&ciphertext)
            .expect("decapsulation should succeed for matching keypair");

        prop_assert_eq!(
            shared_secret.as_bytes(),
            decapsulated.as_bytes(),
            "shared secrets must match after roundtrip"
        );
    }

    /// Property: Different seeds produce different keypairs.
    /// This tests that the key derivation has sufficient entropy.
    #[test]
    fn prop_kem_different_seeds_different_keys(
        seed1 in prop::array::uniform32(any::<u8>()),
        seed2 in prop::array::uniform32(any::<u8>()),
    ) {
        prop_assume!(seed1 != seed2);

        let keypair1 = MlKem768KeyPair::from_seed(&seed1);
        let keypair2 = MlKem768KeyPair::from_seed(&seed2);

        prop_assert_ne!(
            keypair1.public_key().as_bytes(),
            keypair2.public_key().as_bytes(),
            "different seeds must produce different public keys"
        );
    }

    /// Property: Same seed always produces identical keypairs.
    /// This is essential for wallet recovery from mnemonic.
    #[test]
    fn prop_kem_deterministic(seed in prop::array::uniform32(any::<u8>())) {
        let keypair1 = MlKem768KeyPair::from_seed(&seed);
        let keypair2 = MlKem768KeyPair::from_seed(&seed);

        prop_assert_eq!(
            keypair1.public_key().as_bytes(),
            keypair2.public_key().as_bytes(),
            "same seed must produce identical public keys"
        );
    }

    /// Property: Wrong keypair cannot decapsulate.
    /// This tests that the cryptographic binding is correct.
    #[test]
    fn prop_kem_wrong_key_fails(
        seed1 in prop::array::uniform32(any::<u8>()),
        seed2 in prop::array::uniform32(any::<u8>()),
    ) {
        prop_assume!(seed1 != seed2);

        let keypair1 = MlKem768KeyPair::from_seed(&seed1);
        let keypair2 = MlKem768KeyPair::from_seed(&seed2);

        // Encapsulate to keypair1's public key
        let (ciphertext, shared_secret1) = keypair1.public_key().encapsulate();

        // Decapsulate with keypair2 (wrong key)
        let result = keypair2.decapsulate(&ciphertext);

        // ML-KEM uses implicit rejection - returns a random value instead of error
        // The shared secrets should be different
        if let Ok(shared_secret2) = result {
            prop_assert_ne!(
                shared_secret1.as_bytes(),
                shared_secret2.as_bytes(),
                "wrong key must produce different shared secret"
            );
        }
    }
}

// ============================================================================
// ML-DSA Property Tests
// ============================================================================

proptest! {
    /// Property: Signatures verify correctly for any message.
    #[test]
    fn prop_signature_roundtrip(
        seed in prop::array::uniform32(any::<u8>()),
        message in prop::collection::vec(any::<u8>(), 0..1024),
    ) {
        let keypair = MlDsa65KeyPair::from_seed(&seed);
        let signature = keypair.sign(&message);

        prop_assert!(
            keypair.public_key().verify(&message, &signature).is_ok(),
            "valid signature must verify"
        );
    }

    /// Property: Signatures are deterministic for the same message.
    /// This is critical for reproducibility and testing.
    #[test]
    fn prop_signature_deterministic(
        seed in prop::array::uniform32(any::<u8>()),
        message in prop::collection::vec(any::<u8>(), 0..256),
    ) {
        let keypair = MlDsa65KeyPair::from_seed(&seed);
        let sig1 = keypair.sign(&message);
        let sig2 = keypair.sign(&message);

        prop_assert_eq!(
            sig1.as_bytes(),
            sig2.as_bytes(),
            "same message must produce identical signature"
        );
    }

    /// Property: Wrong message fails verification.
    #[test]
    fn prop_signature_wrong_message_fails(
        seed in prop::array::uniform32(any::<u8>()),
        message1 in prop::collection::vec(any::<u8>(), 1..256),
        message2 in prop::collection::vec(any::<u8>(), 1..256),
    ) {
        prop_assume!(message1 != message2);

        let keypair = MlDsa65KeyPair::from_seed(&seed);
        let signature = keypair.sign(&message1);

        prop_assert!(
            keypair.public_key().verify(&message2, &signature).is_err(),
            "signature for different message must fail"
        );
    }

    /// Property: Wrong key fails verification.
    #[test]
    fn prop_signature_wrong_key_fails(
        seed1 in prop::array::uniform32(any::<u8>()),
        seed2 in prop::array::uniform32(any::<u8>()),
        message in prop::collection::vec(any::<u8>(), 0..256),
    ) {
        prop_assume!(seed1 != seed2);

        let keypair1 = MlDsa65KeyPair::from_seed(&seed1);
        let keypair2 = MlDsa65KeyPair::from_seed(&seed2);

        let signature = keypair1.sign(&message);

        prop_assert!(
            keypair2.public_key().verify(&message, &signature).is_err(),
            "signature with different key must fail"
        );
    }

    /// Property: Different seeds produce different signature keypairs.
    #[test]
    fn prop_sig_different_seeds_different_keys(
        seed1 in prop::array::uniform32(any::<u8>()),
        seed2 in prop::array::uniform32(any::<u8>()),
    ) {
        prop_assume!(seed1 != seed2);

        let keypair1 = MlDsa65KeyPair::from_seed(&seed1);
        let keypair2 = MlDsa65KeyPair::from_seed(&seed2);

        prop_assert_ne!(
            keypair1.public_key().as_bytes(),
            keypair2.public_key().as_bytes(),
            "different seeds must produce different public keys"
        );
    }
}

// ============================================================================
// Key Derivation Property Tests
// ============================================================================

// Strategy to generate 64-byte seeds (BIP39 seed size)
fn bip39_seed() -> impl Strategy<Value = [u8; 64]> {
    prop::collection::vec(any::<u8>(), 64).prop_map(|v| {
        let mut arr = [0u8; 64];
        arr.copy_from_slice(&v);
        arr
    })
}

proptest! {
    /// Property: PQ key derivation from BIP39 seed is deterministic.
    #[test]
    fn prop_derive_pq_keys_deterministic(seed in bip39_seed()) {
        let keys1 = derive_pq_keys_from_seed(&seed);
        let keys2 = derive_pq_keys_from_seed(&seed);

        prop_assert_eq!(
            keys1.kem_keypair.public_key().as_bytes(),
            keys2.kem_keypair.public_key().as_bytes(),
            "same seed must produce identical KEM public keys"
        );
        prop_assert_eq!(
            keys1.sig_keypair.public_key().as_bytes(),
            keys2.sig_keypair.public_key().as_bytes(),
            "same seed must produce identical signature public keys"
        );
    }

    /// Property: Different seeds produce different derived keys.
    #[test]
    fn prop_derive_pq_keys_different_seeds(
        seed1 in bip39_seed(),
        seed2 in bip39_seed(),
    ) {
        prop_assume!(seed1 != seed2);

        let keys1 = derive_pq_keys_from_seed(&seed1);
        let keys2 = derive_pq_keys_from_seed(&seed2);

        prop_assert_ne!(
            keys1.kem_keypair.public_key().as_bytes(),
            keys2.kem_keypair.public_key().as_bytes(),
            "different seeds must produce different keys"
        );
    }

    /// Property: One-time keypair derivation is deterministic.
    #[test]
    fn prop_onetime_keypair_deterministic(
        shared_secret in prop::array::uniform32(any::<u8>()),
        output_index in any::<u32>(),
    ) {
        let kp1 = derive_onetime_sig_keypair(&shared_secret, output_index);
        let kp2 = derive_onetime_sig_keypair(&shared_secret, output_index);

        prop_assert_eq!(
            kp1.public_key().as_bytes(),
            kp2.public_key().as_bytes(),
            "same inputs must produce identical one-time keypair"
        );
    }

    /// Property: Different output indices produce different one-time keys.
    /// This is critical for unlinkability between outputs.
    #[test]
    fn prop_onetime_keypair_different_indices(
        shared_secret in prop::array::uniform32(any::<u8>()),
        index1 in any::<u32>(),
        index2 in any::<u32>(),
    ) {
        prop_assume!(index1 != index2);

        let kp1 = derive_onetime_sig_keypair(&shared_secret, index1);
        let kp2 = derive_onetime_sig_keypair(&shared_secret, index2);

        prop_assert_ne!(
            kp1.public_key().as_bytes(),
            kp2.public_key().as_bytes(),
            "different output indices must produce different keys"
        );
    }

    /// Property: Different shared secrets produce different one-time keys.
    #[test]
    fn prop_onetime_keypair_different_secrets(
        secret1 in prop::array::uniform32(any::<u8>()),
        secret2 in prop::array::uniform32(any::<u8>()),
        output_index in any::<u32>(),
    ) {
        prop_assume!(secret1 != secret2);

        let kp1 = derive_onetime_sig_keypair(&secret1, output_index);
        let kp2 = derive_onetime_sig_keypair(&secret2, output_index);

        prop_assert_ne!(
            kp1.public_key().as_bytes(),
            kp2.public_key().as_bytes(),
            "different shared secrets must produce different keys"
        );
    }

    /// Property: One-time keypairs can sign and verify.
    #[test]
    fn prop_onetime_keypair_sign_verify(
        shared_secret in prop::array::uniform32(any::<u8>()),
        output_index in any::<u32>(),
        message in prop::collection::vec(any::<u8>(), 0..512),
    ) {
        let keypair = derive_onetime_sig_keypair(&shared_secret, output_index);
        let signature = keypair.sign(&message);

        prop_assert!(
            keypair.public_key().verify(&message, &signature).is_ok(),
            "one-time keypair signature must verify"
        );
    }
}
