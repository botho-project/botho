//! Cross-implementation compatibility tests for Post-Quantum types.
//!
//! These tests verify that PQ types in `botho` and `transaction/core` are
//! compatible and can interoperate correctly. This is critical for ensuring
//! that:
//!
//! 1. Transactions created by wallet code validate correctly in consensus
//! 2. Size calculations match between implementations
//! 3. Cryptographic operations produce consistent results

use botho::transaction_pq::{
    QuantumPrivateTxInput, QuantumPrivateTxOutput, PQ_CIPHERTEXT_SIZE, PQ_SIGNATURE_SIZE,
    PQ_SIGNING_PUBKEY_SIZE,
};
use bth_account_keys::QuantumSafeAccountKey;
use bth_crypto_pq::{
    derive_onetime_sig_keypair, MlDsa65KeyPair, MlKem768KeyPair, ML_DSA_65_PUBLIC_KEY_BYTES,
    ML_DSA_65_SIGNATURE_BYTES, ML_KEM_768_CIPHERTEXT_BYTES, ML_KEM_768_PUBLIC_KEY_BYTES,
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

    // Verify transaction_pq constants match crypto constants
    assert_eq!(
        PQ_CIPHERTEXT_SIZE, ML_KEM_768_CIPHERTEXT_BYTES,
        "ciphertext size constant"
    );
    assert_eq!(
        PQ_SIGNATURE_SIZE, ML_DSA_65_SIGNATURE_BYTES,
        "signature size constant"
    );
    assert_eq!(
        PQ_SIGNING_PUBKEY_SIZE, ML_DSA_65_PUBLIC_KEY_BYTES,
        "signing pubkey size constant"
    );
}

/// Verify estimated sizes are reasonable
#[test]
fn test_estimated_sizes() {
    // Output: classical(72) + ciphertext(1088) + signing_pubkey(1952) = 3112
    let output_size = QuantumPrivateTxOutput::estimated_size();
    assert!(
        output_size > 3000 && output_size < 3500,
        "output size: {}",
        output_size
    );

    // Input: tx_hash(32) + output_index(4) + classical_sig(64) + pq_sig(3309) =
    // 3409
    let input_size = QuantumPrivateTxInput::estimated_size();
    assert!(
        input_size > 3000 && input_size < 4000,
        "input size: {}",
        input_size
    );
}

/// Verify size comparison: PQ vs classical
#[test]
fn test_size_comparison() {
    // Classical sizes (approximate)
    const CLASSICAL_TX_OUT: usize = 72; // amount + target_key + public_key
    const CLASSICAL_TX_IN: usize = 100; // reference + signature

    // PQ sizes
    let pq_output_size = QuantumPrivateTxOutput::estimated_size();
    let pq_input_size = QuantumPrivateTxInput::estimated_size();

    // PQ transactions should be significantly larger
    assert!(
        pq_output_size > CLASSICAL_TX_OUT * 40,
        "PQ output should be ~43x larger than classical"
    );
    assert!(
        pq_input_size > CLASSICAL_TX_IN * 30,
        "PQ input should be ~34x larger than classical"
    );
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
        receiver_onetime
            .public_key()
            .verify(message, &signature)
            .is_ok(),
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
// Input/Output Structure Compatibility Tests
// ============================================================================

/// Verify QuantumPrivateTxInput field sizes
#[test]
fn test_quantum_private_tx_input_fields() {
    // Create a valid signature
    let keypair = MlDsa65KeyPair::from_seed(&[42u8; 32]);
    let signature = keypair.sign(b"test");

    // Build input with correct field sizes
    let input = QuantumPrivateTxInput {
        tx_hash: [0u8; 32],
        output_index: 0,
        classical_signature: vec![0u8; 64],
        pq_signature: signature.as_bytes().to_vec(),
    };

    assert_eq!(input.tx_hash.len(), 32);
    assert_eq!(input.output_index, 0);
    assert_eq!(input.classical_signature.len(), 64);
    assert_eq!(input.pq_signature.len(), ML_DSA_65_SIGNATURE_BYTES);
}

/// Verify QuantumPrivateTxOutput field sizes
#[test]
fn test_quantum_private_tx_output_fields() {
    // Create valid PQ material
    let kem_keypair = MlKem768KeyPair::from_seed(&[42u8; 32]);
    let (ciphertext, _) = kem_keypair.public_key().encapsulate();
    let sig_keypair = MlDsa65KeyPair::from_seed(&[42u8; 32]);
    let pq_signing_pubkey = sig_keypair.public_key();

    // Build output - requires a classical TxOutput
    use botho::transaction::TxOutput;
    use bth_transaction_types::ClusterTagVector;

    let classical = TxOutput {
        amount: 1_000_000,
        target_key: [1u8; 32],
        public_key: [2u8; 32],
        e_memo: None,
        cluster_tags: ClusterTagVector::empty(),
    };

    let output = QuantumPrivateTxOutput {
        classical,
        pq_ciphertext: ciphertext.as_bytes().to_vec(),
        pq_signing_pubkey: pq_signing_pubkey.as_bytes().to_vec(),
    };

    assert_eq!(output.pq_ciphertext.len(), ML_KEM_768_CIPHERTEXT_BYTES);
    assert_eq!(output.pq_signing_pubkey.len(), ML_DSA_65_PUBLIC_KEY_BYTES);
    assert_eq!(output.amount(), 1_000_000);
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

    // 8. Create transaction input structure
    let input = QuantumPrivateTxInput {
        tx_hash,
        output_index,
        classical_signature: vec![0u8; 64], // Dummy classical signature
        pq_signature: pq_signature.as_bytes().to_vec(),
    };

    // Verify structure
    assert_eq!(input.output_index, output_index);
    assert_eq!(input.pq_signature.len(), ML_DSA_65_SIGNATURE_BYTES);
}
