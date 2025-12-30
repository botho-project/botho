//! Cross-implementation compatibility tests for Post-Quantum types.
//!
//! These tests verify that PQ types in `botho` and `transaction/core` are compatible
//! and can interoperate correctly. This is critical for ensuring that:
//!
//! 1. Transactions created by wallet code validate correctly in consensus
//! 2. Size calculations match between implementations
//! 3. Cryptographic operations produce consistent results

use bth_account_keys::QuantumSafeAccountKey;
use bth_crypto_pq::{
    derive_onetime_sig_keypair, MlDsa65KeyPair, MlKem768KeyPair,
    ML_DSA_65_PUBLIC_KEY_BYTES, ML_DSA_65_SIGNATURE_BYTES,
    ML_KEM_768_CIPHERTEXT_BYTES, ML_KEM_768_PUBLIC_KEY_BYTES,
};
use bth_transaction_core::{
    QuantumPrivateTxIn, QuantumPrivateTxOut, TransactionType,
    quantum_private::size_comparison,
};

const TEST_MNEMONIC: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

// ============================================================================
// Size Constant Compatibility Tests
// ============================================================================

/// Verify that size constants match between implementations
#[test]
fn test_size_constants_match() {
    // These constants must match for fee calculations to be consistent
    assert_eq!(ML_KEM_768_CIPHERTEXT_BYTES, 1088, "ML-KEM ciphertext size");
    assert_eq!(ML_KEM_768_PUBLIC_KEY_BYTES, 1184, "ML-KEM public key size");
    assert_eq!(ML_DSA_65_SIGNATURE_BYTES, 3309, "ML-DSA signature size");
    assert_eq!(ML_DSA_65_PUBLIC_KEY_BYTES, 1952, "ML-DSA public key size");

    // Verify transaction/core sizes
    assert_eq!(
        QuantumPrivateTxOut::APPROX_SIZE,
        3112,
        "transaction/core output size"
    );
    assert_eq!(
        QuantumPrivateTxIn::APPROX_SIZE,
        3409,
        "transaction/core input size"
    );
}

/// Verify size comparison constants are accurate
#[test]
fn test_size_comparison_constants() {
    // Classical sizes
    assert_eq!(size_comparison::CLASSICAL_TX_OUT, 72);
    assert_eq!(size_comparison::CLASSICAL_TX_IN, 100);

    // PQ sizes
    assert_eq!(size_comparison::QUANTUM_PRIVATE_TX_OUT, 3112);
    assert_eq!(size_comparison::QUANTUM_PRIVATE_TX_IN, 3409);
}

// ============================================================================
// Key Generation Compatibility Tests
// ============================================================================

/// Verify that account keys generate valid PQ key material
#[test]
fn test_account_key_generates_valid_pq_keys() {
    let account = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC);

    // KEM key should be correct size
    assert_eq!(
        account.pq_kem_keypair().public_key().as_bytes().len(),
        ML_KEM_768_PUBLIC_KEY_BYTES,
        "KEM public key size"
    );

    // Signature key should be correct size
    assert_eq!(
        account.pq_sig_keypair().public_key().as_bytes().len(),
        ML_DSA_65_PUBLIC_KEY_BYTES,
        "Signature public key size"
    );
}

/// Verify KEM encapsulation produces correct-sized ciphertext
#[test]
fn test_kem_ciphertext_size() {
    let keypair = MlKem768KeyPair::from_seed(&[42u8; 32]);
    let (ciphertext, _shared_secret) = keypair.public_key().encapsulate();

    assert_eq!(
        ciphertext.as_bytes().len(),
        ML_KEM_768_CIPHERTEXT_BYTES,
        "encapsulated ciphertext size"
    );
}

/// Verify signature produces correct-sized output
#[test]
fn test_signature_size() {
    let keypair = MlDsa65KeyPair::from_seed(&[42u8; 32]);
    let signature = keypair.sign(b"test message");

    assert_eq!(
        signature.as_bytes().len(),
        ML_DSA_65_SIGNATURE_BYTES,
        "signature size"
    );
}

// ============================================================================
// One-Time Key Derivation Compatibility Tests
// ============================================================================

/// Verify that one-time keys derived from shared secret can sign and verify
#[test]
fn test_onetime_key_sign_verify_roundtrip() {
    // Simulate the flow: sender encapsulates, receiver decapsulates
    let kem_keypair = MlKem768KeyPair::from_seed(&[42u8; 32]);
    let (ciphertext, sender_secret) = kem_keypair.public_key().encapsulate();
    let receiver_secret = kem_keypair.decapsulate(&ciphertext).unwrap();

    assert_eq!(
        sender_secret.as_bytes(),
        receiver_secret.as_bytes(),
        "shared secrets must match"
    );

    // Both parties derive the same one-time keypair
    let sender_onetime = derive_onetime_sig_keypair(sender_secret.as_bytes(), 0);
    let receiver_onetime = derive_onetime_sig_keypair(receiver_secret.as_bytes(), 0);

    assert_eq!(
        sender_onetime.public_key().as_bytes(),
        receiver_onetime.public_key().as_bytes(),
        "one-time public keys must match"
    );

    // Signature created by sender can be verified by receiver
    let message = b"transaction signing hash";
    let signature = sender_onetime.sign(message);

    assert!(
        receiver_onetime.public_key().verify(message, &signature).is_ok(),
        "receiver must be able to verify sender's signature"
    );
}

/// Verify one-time key public key has correct size for storage in outputs
#[test]
fn test_onetime_key_public_key_size() {
    let shared_secret = [0u8; 32];
    let onetime = derive_onetime_sig_keypair(&shared_secret, 0);

    assert_eq!(
        onetime.public_key().as_bytes().len(),
        ML_DSA_65_PUBLIC_KEY_BYTES,
        "one-time public key size"
    );
}

// ============================================================================
// Transaction Type Compatibility Tests
// ============================================================================

/// Verify transaction type enum values
#[test]
fn test_transaction_type_values() {
    assert_eq!(TransactionType::Standard as u8, 0);
    assert_eq!(TransactionType::QuantumPrivate as u8, 1);

    // Verify roundtrip
    assert_eq!(TransactionType::from(0u8), TransactionType::Standard);
    assert_eq!(TransactionType::from(1u8), TransactionType::QuantumPrivate);
}

/// Verify default transaction type
#[test]
fn test_transaction_type_default() {
    assert_eq!(
        TransactionType::default(),
        TransactionType::Standard,
        "default should be classical"
    );
}

// ============================================================================
// Input/Output Structure Compatibility Tests
// ============================================================================

/// Verify QuantumPrivateTxIn raw data fields work correctly
#[test]
fn test_quantum_private_tx_in_raw_fields() {
    // Create a valid signature
    let keypair = MlDsa65KeyPair::from_seed(&[42u8; 32]);
    let signature = keypair.sign(b"test");

    // Build input using raw fields (the protobuf way)
    let input = QuantumPrivateTxIn {
        tx_hash: vec![0u8; 32],
        output_index: 0,
        schnorr_signature: vec![0u8; 64],
        dilithium_signature: signature.as_bytes().to_vec(),
    };

    assert_eq!(input.tx_hash.len(), 32);
    assert_eq!(input.output_index, 0);
    assert_eq!(input.schnorr_signature.len(), 64);
    assert_eq!(input.dilithium_signature.len(), ML_DSA_65_SIGNATURE_BYTES);

    // Verify typed getter works
    assert_eq!(
        input.get_dilithium_signature().unwrap().as_bytes(),
        signature.as_bytes()
    );
}

/// Verify QuantumPrivateTxOut raw data fields work correctly
#[test]
fn test_quantum_private_tx_out_raw_fields() {
    // Create valid PQ material
    let kem_keypair = MlKem768KeyPair::from_seed(&[42u8; 32]);
    let (ciphertext, _) = kem_keypair.public_key().encapsulate();
    let sig_keypair = MlDsa65KeyPair::from_seed(&[42u8; 32]);
    let pq_target_key = sig_keypair.public_key();

    // Build output using raw fields
    let output = QuantumPrivateTxOut {
        masked_amount: None,
        target_key: Default::default(),
        public_key: Default::default(),
        e_memo: None,
        cluster_tags: None,
        pq_ciphertext: ciphertext.as_bytes().to_vec(),
        pq_target_key: pq_target_key.as_bytes().to_vec(),
    };

    assert_eq!(output.pq_ciphertext.len(), ML_KEM_768_CIPHERTEXT_BYTES);
    assert_eq!(output.pq_target_key.len(), ML_DSA_65_PUBLIC_KEY_BYTES);

    // Verify typed getters work
    assert_eq!(
        output.get_pq_ciphertext().unwrap().as_bytes(),
        ciphertext.as_bytes()
    );
    assert_eq!(
        output.get_pq_target_key().unwrap().as_bytes(),
        pq_target_key.as_bytes()
    );
}

// ============================================================================
// Cross-Passphrase Compatibility Tests
// ============================================================================

/// Verify that passphrase produces different keys
#[test]
fn test_passphrase_produces_different_keys() {
    let account_no_pass = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC);
    let account_with_pass =
        QuantumSafeAccountKey::from_mnemonic_with_passphrase(TEST_MNEMONIC, "secret");

    // PQ keys should differ
    assert_ne!(
        account_no_pass.pq_kem_keypair().public_key().as_bytes(),
        account_with_pass.pq_kem_keypair().public_key().as_bytes(),
        "passphrase must produce different KEM keys"
    );

    assert_ne!(
        account_no_pass.pq_sig_keypair().public_key().as_bytes(),
        account_with_pass.pq_sig_keypair().public_key().as_bytes(),
        "passphrase must produce different signature keys"
    );
}

/// Verify from_mnemonic equals empty passphrase
#[test]
fn test_from_mnemonic_equals_empty_passphrase() {
    let account1 = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC);
    let account2 = QuantumSafeAccountKey::from_mnemonic_with_passphrase(TEST_MNEMONIC, "");

    assert_eq!(
        account1.pq_kem_keypair().public_key().as_bytes(),
        account2.pq_kem_keypair().public_key().as_bytes(),
        "from_mnemonic should equal empty passphrase for KEM"
    );

    assert_eq!(
        account1.pq_sig_keypair().public_key().as_bytes(),
        account2.pq_sig_keypair().public_key().as_bytes(),
        "from_mnemonic should equal empty passphrase for signature"
    );
}

// ============================================================================
// End-to-End Transaction Flow Test
// ============================================================================

/// Simulate a complete transaction flow to verify all components work together
#[test]
fn test_end_to_end_transaction_flow() {
    // 1. Create recipient account
    let recipient = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC);
    let recipient_kem_pk = recipient.pq_kem_keypair().public_key();

    // 2. Sender encapsulates to recipient's KEM public key
    let (ciphertext, sender_shared_secret) = recipient_kem_pk.encapsulate();

    // Verify ciphertext size
    assert_eq!(ciphertext.as_bytes().len(), ML_KEM_768_CIPHERTEXT_BYTES);

    // 3. Sender derives one-time signing keypair
    let output_index = 0u32;
    let sender_onetime = derive_onetime_sig_keypair(sender_shared_secret.as_bytes(), output_index);

    // 4. Recipient decapsulates to recover shared secret
    let recipient_shared_secret = recipient.pq_kem_keypair().decapsulate(&ciphertext).unwrap();
    assert_eq!(
        sender_shared_secret.as_bytes(),
        recipient_shared_secret.as_bytes()
    );

    // 5. Recipient re-derives the same one-time keypair
    let recipient_onetime =
        derive_onetime_sig_keypair(recipient_shared_secret.as_bytes(), output_index);
    assert_eq!(
        sender_onetime.public_key().as_bytes(),
        recipient_onetime.public_key().as_bytes()
    );

    // 6. Create a transaction input (spending the output)
    let tx_hash = [0u8; 32]; // Dummy for test
    let signing_hash = [1u8; 32]; // Dummy signing hash

    // Sign with the one-time key
    let pq_signature = recipient_onetime.sign(&signing_hash);
    assert_eq!(pq_signature.as_bytes().len(), ML_DSA_65_SIGNATURE_BYTES);

    // 7. Verify the signature (simulating consensus validation)
    assert!(
        sender_onetime
            .public_key()
            .verify(&signing_hash, &pq_signature)
            .is_ok(),
        "signature must verify for spending"
    );

    // 8. Create transaction/core input structure using raw fields
    let input = QuantumPrivateTxIn {
        tx_hash: tx_hash.to_vec(),
        output_index,
        schnorr_signature: vec![0u8; 64], // Dummy classical signature
        dilithium_signature: pq_signature.as_bytes().to_vec(),
    };

    // Verify structure
    assert_eq!(input.output_index, output_index);
    assert_eq!(
        input.get_dilithium_signature().unwrap().as_bytes().len(),
        ML_DSA_65_SIGNATURE_BYTES
    );
}
