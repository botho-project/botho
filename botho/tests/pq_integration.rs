// Copyright (c) 2024 Botho Foundation

//! Post-Quantum Cryptography Integration Tests
//!
//! These tests verify the complete PQ transaction flow:
//! 1. Key derivation from mnemonic
//! 2. Address generation and parsing
//! 3. Output creation with ML-KEM encapsulation
//! 4. Output scanning and ownership detection
//! 5. Transaction signing with dual signatures
//! 6. Transaction validation

#![cfg(feature = "pq")]

use bth_account_keys::QuantumSafeAccountKey;
use botho::transaction::TxOutput;
use botho::transaction_pq::{
    calculate_pq_fee, QuantumPrivateTransaction, QuantumPrivateTxInput, QuantumPrivateTxOutput,
    PQ_CIPHERTEXT_SIZE, PQ_SIGNATURE_SIZE, PQ_SIGNING_PUBKEY_SIZE,
};
use bincode;

const TEST_MNEMONIC_ALICE: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const TEST_MNEMONIC_BOB: &str = "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong";

// ============================================================================
// Key Derivation Tests
// ============================================================================

#[test]
fn test_different_mnemonics_produce_different_keys() {
    let alice = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC_ALICE);
    let bob = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC_BOB);

    // Classical keys should differ (compare via subaddresses which expose public keys)
    let alice_addr = alice.default_subaddress();
    let bob_addr = bob.default_subaddress();
    assert_ne!(
        alice_addr.view_public_key().as_ref(),
        bob_addr.view_public_key().as_ref()
    );

    // PQ keys should also differ
    assert_ne!(
        alice.pq_kem_keypair().public_key().as_bytes(),
        bob.pq_kem_keypair().public_key().as_bytes()
    );
}

#[test]
fn test_same_mnemonic_produces_same_classical_keys() {
    let alice1 = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC_ALICE);
    let alice2 = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC_ALICE);

    // Classical keys should be identical (compare via subaddresses)
    let addr1 = alice1.default_subaddress();
    let addr2 = alice2.default_subaddress();
    assert_eq!(
        addr1.view_public_key().as_ref(),
        addr2.view_public_key().as_ref()
    );
    assert_eq!(
        addr1.spend_public_key().as_ref(),
        addr2.spend_public_key().as_ref()
    );
}

// ============================================================================
// Address Tests
// ============================================================================

#[test]
fn test_pq_address_roundtrip() {
    let alice = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC_ALICE);
    let address = alice.default_subaddress();

    // Encode to string
    let address_string = address.to_address_string();
    assert!(address_string.starts_with("botho-pq://1/"));

    // Decode from string
    let parsed = bth_account_keys::QuantumSafePublicAddress::from_address_string(&address_string)
        .expect("Should parse valid address");

    // Classical components should match
    assert_eq!(
        address.classical().view_public_key().as_ref(),
        parsed.classical().view_public_key().as_ref()
    );
    assert_eq!(
        address.classical().spend_public_key().as_ref(),
        parsed.classical().spend_public_key().as_ref()
    );
}

#[test]
fn test_pq_address_size() {
    let alice = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC_ALICE);
    let address_string = alice.default_subaddress().to_address_string();

    // PQ address is ~4.3KB due to large public keys
    // ML-KEM-768: 1184 bytes, ML-DSA-65: 1952 bytes, classical: 64 bytes
    // Base58 encoding adds ~37% overhead
    assert!(address_string.len() > 4000);
    assert!(address_string.len() < 5000);
}

// ============================================================================
// Output Creation and Ownership Tests
// ============================================================================

#[test]
fn test_pq_output_creation() {
    let alice = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC_ALICE);
    let address = alice.default_subaddress();

    let output = QuantumPrivateTxOutput::new(1_000_000_000_000, &address);

    // Verify sizes
    assert_eq!(output.pq_ciphertext.len(), PQ_CIPHERTEXT_SIZE);
    assert_eq!(output.pq_signing_pubkey.len(), PQ_SIGNING_PUBKEY_SIZE);
    assert_eq!(output.amount(), 1_000_000_000_000);
}

#[test]
fn test_pq_output_ownership_detection() {
    let alice = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC_ALICE);
    let address = alice.default_subaddress();

    let output = QuantumPrivateTxOutput::new(1_000_000_000_000, &address);

    // Alice should be able to detect ownership
    let result = output.belongs_to(&alice);
    assert!(result.is_some(), "Owner should detect their output");

    let (subaddress_index, shared_secret) = result.unwrap();
    assert_eq!(subaddress_index, 0);
    assert_ne!(shared_secret, [0u8; 32], "Shared secret should not be zero");
}

#[test]
fn test_pq_output_non_owner_cannot_detect() {
    let alice = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC_ALICE);
    let bob = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC_BOB);

    let alice_address = alice.default_subaddress();
    let output = QuantumPrivateTxOutput::new(1_000_000_000_000, &alice_address);

    // Bob should NOT be able to detect ownership of Alice's output
    let result = output.belongs_to(&bob);
    assert!(result.is_none(), "Non-owner should not detect ownership");
}

#[test]
fn test_pq_output_unique_ciphertexts() {
    let alice = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC_ALICE);
    let address = alice.default_subaddress();

    // Create multiple outputs to the same address
    let output1 = QuantumPrivateTxOutput::new(1_000_000, &address);
    let output2 = QuantumPrivateTxOutput::new(1_000_000, &address);

    // Ciphertexts should be different (randomized encapsulation)
    assert_ne!(
        output1.pq_ciphertext, output2.pq_ciphertext,
        "Each output should have unique ciphertext"
    );

    // Both should still be detectable by Alice
    assert!(output1.belongs_to(&alice).is_some());
    assert!(output2.belongs_to(&alice).is_some());
}

// ============================================================================
// Transaction Structure Tests
// ============================================================================

#[test]
fn test_pq_transaction_structure_valid() {
    let tx = create_mock_pq_transaction(1, 2, 100_000_000);

    assert!(tx.is_valid_structure().is_ok());
}

#[test]
fn test_pq_transaction_no_inputs_invalid() {
    let tx = QuantumPrivateTransaction::new(
        vec![], // No inputs
        vec![create_mock_pq_output(1_000_000)],
        100_000_000,
        100,
    );

    let result = tx.is_valid_structure();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("no inputs"));
}

#[test]
fn test_pq_transaction_no_outputs_invalid() {
    let tx = QuantumPrivateTransaction::new(
        vec![create_mock_pq_input()],
        vec![], // No outputs
        100_000_000,
        100,
    );

    let result = tx.is_valid_structure();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("no outputs"));
}

#[test]
fn test_pq_transaction_zero_amount_invalid() {
    let mut output = create_mock_pq_output(0);
    output.classical.amount = 0;

    let tx = QuantumPrivateTransaction::new(
        vec![create_mock_pq_input()],
        vec![output],
        100_000_000,
        100,
    );

    let result = tx.is_valid_structure();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("zero-amount"));
}

#[test]
fn test_pq_transaction_hash_determinism() {
    let tx1 = create_mock_pq_transaction(1, 2, 100_000_000);
    let tx2 = create_mock_pq_transaction(1, 2, 100_000_000);

    // Same structure should produce same hash
    assert_eq!(tx1.hash(), tx2.hash());

    // Different fee should produce different hash
    let tx3 = create_mock_pq_transaction(1, 2, 200_000_000);
    assert_ne!(tx1.hash(), tx3.hash());
}

#[test]
fn test_pq_transaction_signing_hash_excludes_signatures() {
    let tx1 = create_mock_pq_transaction(1, 2, 100_000_000);

    // Modify input signature
    let mut tx2 = tx1.clone();
    tx2.inputs[0].classical_signature = vec![0xFF; 64];
    tx2.inputs[0].pq_signature = vec![0xFF; PQ_SIGNATURE_SIZE];

    // Signing hash should be the same (signatures excluded)
    assert_eq!(tx1.signing_hash(), tx2.signing_hash());
}

// ============================================================================
// Fee Calculation Tests
// ============================================================================

#[test]
fn test_pq_fee_minimum() {
    // Simple transaction should use minimum fee
    let simple_fee = calculate_pq_fee(1, 2);
    assert!(simple_fee >= 100_000_000); // MIN_TX_FEE
}

#[test]
fn test_pq_fee_scales_with_inputs() {
    let fee_1 = calculate_pq_fee(1, 2);
    let fee_5 = calculate_pq_fee(5, 2);
    let fee_10 = calculate_pq_fee(10, 2);

    assert!(fee_5 > fee_1);
    assert!(fee_10 > fee_5);
}

#[test]
fn test_pq_fee_scales_with_outputs() {
    // Use larger counts to exceed the minimum fee floor
    let fee_5 = calculate_pq_fee(2, 5);
    let fee_10 = calculate_pq_fee(2, 10);
    let fee_15 = calculate_pq_fee(2, 15);

    assert!(fee_10 > fee_5, "fee_10={} should exceed fee_5={}", fee_10, fee_5);
    assert!(fee_15 > fee_10, "fee_15={} should exceed fee_10={}", fee_15, fee_10);
}

#[test]
fn test_pq_transaction_fee_check() {
    // Transaction with sufficient fee
    let tx = create_mock_pq_transaction(1, 2, 100_000_000);
    assert!(tx.has_sufficient_fee());

    // Large transaction with insufficient fee
    let large_tx = QuantumPrivateTransaction::new(
        (0..10).map(|_| create_mock_pq_input()).collect(),
        (0..10).map(|_| create_mock_pq_output(1_000_000)).collect(),
        100_000_000, // Too low for large tx
        100,
    );
    assert!(!large_tx.has_sufficient_fee());
}

// ============================================================================
// Validation Tests
// ============================================================================

#[test]
fn test_pq_validation_ciphertext_size() {
    use botho::consensus::{TransactionValidator, ValidationError};
    use botho::ledger::ChainState;
    use std::sync::{Arc, RwLock};

    let chain_state = Arc::new(RwLock::new(ChainState::default()));

    let validator = TransactionValidator::new(chain_state);

    // Valid transaction
    let valid_tx = create_mock_pq_transaction(1, 1, 100_000_000);
    assert!(validator.validate_quantum_private_tx(&valid_tx).is_ok());

    // Invalid ciphertext size
    let mut invalid_tx = create_mock_pq_transaction(1, 1, 100_000_000);
    invalid_tx.outputs[0].pq_ciphertext = vec![0u8; 100]; // Wrong size

    let result = validator.validate_quantum_private_tx(&invalid_tx);
    assert!(matches!(result, Err(ValidationError::InvalidPqCiphertext)));
}

#[test]
fn test_pq_validation_signature_size() {
    use botho::consensus::{TransactionValidator, ValidationError};
    use botho::ledger::ChainState;
    use std::sync::{Arc, RwLock};

    let chain_state = Arc::new(RwLock::new(ChainState::default()));

    let validator = TransactionValidator::new(chain_state);

    // Invalid PQ signature size
    let mut invalid_tx = create_mock_pq_transaction(1, 1, 100_000_000);
    invalid_tx.inputs[0].pq_signature = vec![0u8; 100]; // Wrong size

    let result = validator.validate_quantum_private_tx(&invalid_tx);
    assert!(matches!(result, Err(ValidationError::InvalidPqSignature)));
}

#[test]
fn test_pq_validation_input_limit() {
    use botho::consensus::{TransactionValidator, ValidationError};
    use botho::ledger::ChainState;
    use std::sync::{Arc, RwLock};

    let chain_state = Arc::new(RwLock::new(ChainState::default()));

    let validator = TransactionValidator::new(chain_state);

    // Too many inputs (limit is 16)
    let tx = QuantumPrivateTransaction::new(
        (0..17).map(|_| create_mock_pq_input()).collect(),
        vec![create_mock_pq_output(1_000_000)],
        1_000_000_000,
        100,
    );

    let result = validator.validate_quantum_private_tx(&tx);
    assert!(matches!(result, Err(ValidationError::PqInputTooLarge)));
}

// ============================================================================
// Size and Performance Tests
// ============================================================================

#[test]
fn test_pq_transaction_size_estimation() {
    let tx = create_mock_pq_transaction(1, 2, 100_000_000);
    let estimated = tx.estimated_size();

    // Should be in expected range
    // 1 input (~3409 bytes) + 2 outputs (~3112 bytes each) + header
    // Total: ~9633+ bytes (increased due to storing full ML-DSA pubkey)
    assert!(estimated > 9000);
    assert!(estimated < 12000);
}

#[test]
fn test_pq_vs_classical_size_overhead() {
    // Classical output: ~72 bytes
    // PQ output: ~3112 bytes (43x) - includes 1952-byte ML-DSA public key
    // This is the cost of verifiable PQ signatures
    let pq_output = QuantumPrivateTxOutput::estimated_size();
    let classical_output = 72;
    let output_overhead = pq_output as f64 / classical_output as f64;
    assert!(output_overhead > 40.0);
    assert!(output_overhead < 50.0);

    // Classical input: ~100 bytes
    // PQ input: ~3409 bytes (34x)
    let pq_input = QuantumPrivateTxInput::estimated_size();
    let classical_input = 100;
    let input_overhead = pq_input as f64 / classical_input as f64;
    assert!(input_overhead > 30.0);
    assert!(input_overhead < 40.0);
}

// ============================================================================
// Performance Tests
// ============================================================================

#[test]
fn test_pq_key_derivation_performance() {
    use std::time::Instant;

    let start = Instant::now();
    let iterations: u32 = 10;

    for _ in 0..iterations {
        let _ = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC_ALICE);
    }

    let elapsed = start.elapsed();
    let per_op = elapsed / iterations;

    // Key derivation should complete in reasonable time (< 500ms per key)
    assert!(
        per_op.as_millis() < 500,
        "Key derivation too slow: {:?} per operation",
        per_op
    );
}

#[test]
fn test_pq_address_generation_performance() {
    use std::time::Instant;

    let key = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC_ALICE);

    let start = Instant::now();
    let iterations: u32 = 100;

    for i in 0..iterations as u64 {
        let _ = key.subaddress(i);
    }

    let elapsed = start.elapsed();
    let per_op = elapsed / iterations;

    // Address generation should be fast (< 10ms per address)
    assert!(
        per_op.as_millis() < 10,
        "Address generation too slow: {:?} per operation",
        per_op
    );
}

#[test]
fn test_pq_address_encoding_performance() {
    use std::time::Instant;

    let key = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC_ALICE);
    let address = key.default_subaddress();

    let start = Instant::now();
    let iterations: u32 = 100;

    for _ in 0..iterations {
        let _ = address.to_address_string();
    }

    let elapsed = start.elapsed();
    let per_op = elapsed / iterations;

    // Address encoding for PQ addresses (4KB+) is slower due to Base58 encoding
    // Allow up to 200ms per encode in debug mode
    assert!(
        per_op.as_millis() < 200,
        "Address encoding too slow: {:?} per operation",
        per_op
    );
}

#[test]
fn test_pq_transaction_hashing_performance() {
    use std::time::Instant;

    let tx = create_mock_pq_transaction(4, 4, 200_000_000);

    let start = Instant::now();
    let iterations: u32 = 1000;

    for _ in 0..iterations {
        let _ = tx.hash();
    }

    let elapsed = start.elapsed();
    let per_op = elapsed / iterations;

    // Transaction hashing should be fast
    // Debug mode is ~10x slower, so use different thresholds
    #[cfg(debug_assertions)]
    let threshold_micros = 10_000; // 10ms in debug mode
    #[cfg(not(debug_assertions))]
    let threshold_micros = 1_000; // 1ms in release mode

    assert!(
        per_op.as_micros() < threshold_micros,
        "Transaction hashing too slow: {:?} per operation (threshold: {}µs)",
        per_op,
        threshold_micros
    );
}

#[test]
fn test_pq_serialization_performance() {
    use std::time::Instant;

    let tx = create_mock_pq_transaction(4, 4, 200_000_000);

    // Debug mode is ~10x slower, so use different thresholds
    #[cfg(debug_assertions)]
    let threshold_ms = 50; // 50ms in debug mode
    #[cfg(not(debug_assertions))]
    let threshold_ms = 5; // 5ms in release mode

    // Serialize performance
    let start = Instant::now();
    let iterations: u32 = 100;

    let mut serialized = vec![];
    for _ in 0..iterations {
        serialized = bincode::serialize(&tx).unwrap();
    }

    let serialize_elapsed = start.elapsed();
    let serialize_per_op = serialize_elapsed / iterations;

    assert!(
        serialize_per_op.as_millis() < threshold_ms,
        "Serialization too slow: {:?} per operation (threshold: {}ms)",
        serialize_per_op,
        threshold_ms
    );

    // Deserialize performance
    let start = Instant::now();
    for _ in 0..iterations {
        let _: QuantumPrivateTransaction = bincode::deserialize(&serialized).unwrap();
    }

    let deserialize_elapsed = start.elapsed();
    let deserialize_per_op = deserialize_elapsed / iterations;

    assert!(
        deserialize_per_op.as_millis() < threshold_ms,
        "Deserialization too slow: {:?} per operation (threshold: {}ms)",
        deserialize_per_op,
        threshold_ms
    );
}

// ============================================================================
// Serialization Roundtrip Tests
// ============================================================================

#[test]
fn test_pq_output_serialization_roundtrip() {
    let output = create_mock_pq_output(1_000_000_000);

    // Serialize to bytes
    let serialized = bincode::serialize(&output).expect("Serialize should succeed");

    // Deserialize back
    let deserialized: QuantumPrivateTxOutput =
        bincode::deserialize(&serialized).expect("Deserialize should succeed");

    // Compare all fields
    assert_eq!(output.classical.amount, deserialized.classical.amount);
    assert_eq!(output.classical.target_key, deserialized.classical.target_key);
    assert_eq!(output.classical.public_key, deserialized.classical.public_key);
    assert_eq!(output.pq_ciphertext, deserialized.pq_ciphertext);
    assert_eq!(output.pq_signing_pubkey, deserialized.pq_signing_pubkey);
}

#[test]
fn test_pq_input_serialization_roundtrip() {
    let input = create_mock_pq_input();

    // Serialize to bytes
    let serialized = bincode::serialize(&input).expect("Serialize should succeed");

    // Deserialize back
    let deserialized: QuantumPrivateTxInput =
        bincode::deserialize(&serialized).expect("Deserialize should succeed");

    // Compare all fields
    assert_eq!(input.tx_hash, deserialized.tx_hash);
    assert_eq!(input.output_index, deserialized.output_index);
    assert_eq!(input.classical_signature, deserialized.classical_signature);
    assert_eq!(input.pq_signature, deserialized.pq_signature);
}

#[test]
fn test_pq_transaction_serialization_roundtrip() {
    let tx = create_mock_pq_transaction(2, 3, 150_000_000);

    // Serialize to bytes
    let serialized = bincode::serialize(&tx).expect("Serialize should succeed");

    // Deserialize back
    let deserialized: QuantumPrivateTransaction =
        bincode::deserialize(&serialized).expect("Deserialize should succeed");

    // Hash should be identical
    assert_eq!(tx.hash(), deserialized.hash());
    assert_eq!(tx.signing_hash(), deserialized.signing_hash());

    // Check structure
    assert_eq!(tx.inputs.len(), deserialized.inputs.len());
    assert_eq!(tx.outputs.len(), deserialized.outputs.len());
    assert_eq!(tx.fee, deserialized.fee);
    assert_eq!(tx.created_at_height, deserialized.created_at_height);
}

#[test]
fn test_pq_transaction_large_serialization() {
    // Create a larger transaction (max inputs/outputs)
    let tx = QuantumPrivateTransaction::new(
        (0..16).map(|_| create_mock_pq_input()).collect(),
        (0..16).map(|i| create_mock_pq_output(1_000_000 * (i as u64 + 1))).collect(),
        1_000_000_000,
        100,
    );

    // Serialize
    let serialized = bincode::serialize(&tx).expect("Serialize large tx should succeed");

    // Should be substantial in size (~70KB for max tx)
    assert!(serialized.len() > 50_000, "Large tx should be substantial: {} bytes", serialized.len());

    // Deserialize
    let deserialized: QuantumPrivateTransaction =
        bincode::deserialize(&serialized).expect("Deserialize large tx should succeed");

    assert_eq!(tx.hash(), deserialized.hash());
}

#[test]
fn test_pq_address_bytes_roundtrip() {
    let alice = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC_ALICE);
    let address = alice.default_subaddress();

    // Convert to bytes
    let bytes = address.to_bytes();

    // Parse from bytes
    let parsed = bth_account_keys::QuantumSafePublicAddress::from_bytes(&bytes)
        .expect("Should parse from bytes");

    // Should be identical
    assert_eq!(address.to_address_string(), parsed.to_address_string());
}

#[test]
fn test_pq_output_with_varying_amounts() {
    // Test serialization with edge case amounts
    let amounts = [0u64, 1, 100, 1_000_000, u64::MAX / 2, u64::MAX];

    for amount in amounts {
        let output = create_mock_pq_output(amount);
        let serialized = bincode::serialize(&output).expect("Serialize should succeed");
        let deserialized: QuantumPrivateTxOutput =
            bincode::deserialize(&serialized).expect("Deserialize should succeed");

        assert_eq!(output.classical.amount, deserialized.classical.amount);
    }
}

// ============================================================================
// Determinism Tests
// ============================================================================

#[test]
fn test_pq_key_derivation_determinism() {
    // Derive keys multiple times from the same mnemonic
    let key1 = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC_ALICE);
    let key2 = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC_ALICE);
    let key3 = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC_ALICE);

    // All PQ KEM keys should be identical
    assert_eq!(
        key1.pq_kem_keypair().public_key().as_bytes(),
        key2.pq_kem_keypair().public_key().as_bytes()
    );
    assert_eq!(
        key2.pq_kem_keypair().public_key().as_bytes(),
        key3.pq_kem_keypair().public_key().as_bytes()
    );

    // All PQ signature keys should be identical
    assert_eq!(
        key1.pq_sig_keypair().public_key().as_bytes(),
        key2.pq_sig_keypair().public_key().as_bytes()
    );
    assert_eq!(
        key2.pq_sig_keypair().public_key().as_bytes(),
        key3.pq_sig_keypair().public_key().as_bytes()
    );
}

#[test]
fn test_pq_address_string_determinism() {
    let key1 = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC_ALICE);
    let key2 = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC_ALICE);

    let addr1 = key1.default_subaddress().to_address_string();
    let addr2 = key2.default_subaddress().to_address_string();

    // Addresses should be byte-for-byte identical
    assert_eq!(addr1, addr2);
}

#[test]
fn test_pq_subaddress_determinism() {
    let key1 = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC_ALICE);
    let key2 = QuantumSafeAccountKey::from_mnemonic(TEST_MNEMONIC_ALICE);

    // Test multiple subaddress indices
    for index in [0, 1, 10, 100, 1000] {
        let addr1 = key1.subaddress(index);
        let addr2 = key2.subaddress(index);

        assert_eq!(
            addr1.to_address_string(),
            addr2.to_address_string(),
            "Subaddress {} should be deterministic",
            index
        );
    }
}

#[test]
fn test_pq_transaction_hash_determinism_extended() {
    // Create identical transactions
    let tx1 = create_mock_pq_transaction(2, 3, 150_000_000);
    let tx2 = create_mock_pq_transaction(2, 3, 150_000_000);
    let tx3 = create_mock_pq_transaction(2, 3, 150_000_000);

    // All hashes should be identical
    assert_eq!(tx1.hash(), tx2.hash());
    assert_eq!(tx2.hash(), tx3.hash());

    // Signing hashes should also be identical
    assert_eq!(tx1.signing_hash(), tx2.signing_hash());
    assert_eq!(tx2.signing_hash(), tx3.signing_hash());
}

#[test]
fn test_pq_output_id_determinism() {
    let output1 = create_mock_pq_output(1_000_000);
    let output2 = create_mock_pq_output(1_000_000);

    // Same construction -> same ID
    assert_eq!(output1.id(), output2.id());
}

// ============================================================================
// Helper Functions
// ============================================================================

fn create_mock_pq_input() -> QuantumPrivateTxInput {
    QuantumPrivateTxInput {
        tx_hash: [1u8; 32],
        output_index: 0,
        classical_signature: vec![0u8; 64],
        pq_signature: vec![0u8; PQ_SIGNATURE_SIZE],
    }
}

fn create_mock_pq_output(amount: u64) -> QuantumPrivateTxOutput {
    use bth_transaction_types::ClusterTagVector;
    QuantumPrivateTxOutput {
        classical: TxOutput {
            amount,
            target_key: [2u8; 32],
            public_key: [3u8; 32],
            e_memo: None,
            cluster_tags: ClusterTagVector::empty(),
        },
        pq_ciphertext: vec![0u8; PQ_CIPHERTEXT_SIZE],
        pq_signing_pubkey: vec![0u8; PQ_SIGNING_PUBKEY_SIZE],
    }
}

fn create_mock_pq_transaction(
    num_inputs: usize,
    num_outputs: usize,
    fee: u64,
) -> QuantumPrivateTransaction {
    QuantumPrivateTransaction::new(
        (0..num_inputs).map(|_| create_mock_pq_input()).collect(),
        (0..num_outputs)
            .map(|i| create_mock_pq_output(1_000_000 * (i as u64 + 1)))
            .collect(),
        fee,
        100,
    )
}
