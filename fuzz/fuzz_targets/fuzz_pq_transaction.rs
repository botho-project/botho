#![no_main]

use libfuzzer_sys::fuzz_target;

use botho::transaction_pq::{QuantumPrivateTxInput, QuantumPrivateTxOutput, QuantumPrivateTransaction};

// Fuzz target for Quantum-Private Transaction deserialization.
//
// PQ transactions have larger attack surface due to:
// - ML-KEM ciphertexts (1088 bytes) that must be validated
// - ML-DSA signatures (3309 bytes) that must be validated
// - One-time PQ public keys (1952 bytes) that must be validated
//
// Security rationale: PQ transactions are received from untrusted peers.
// Malformed PQ data must never cause crashes or undefined behavior.
fuzz_target!(|data: &[u8]| {
    // Test full PQ transaction deserialization
    let _ = bincode::deserialize::<QuantumPrivateTransaction>(data);

    // Test individual PQ components
    let _ = bincode::deserialize::<QuantumPrivateTxInput>(data);
    let _ = bincode::deserialize::<QuantumPrivateTxOutput>(data);

    // If deserialization succeeds, verify methods don't panic
    if let Ok(tx) = bincode::deserialize::<QuantumPrivateTransaction>(data) {
        let _ = tx.hash();
        let _ = tx.signing_hash();
        let _ = tx.total_output();
        let _ = tx.is_valid_structure();
        let _ = tx.estimated_size();
        let _ = tx.minimum_fee();
        let _ = tx.has_sufficient_fee();
    }

    // Test PQ input verification with arbitrary keys (should not panic)
    if let Ok(input) = bincode::deserialize::<QuantumPrivateTxInput>(data) {
        let fake_hash = [0u8; 32];
        let fake_key = [0u8; 32];
        let fake_pq_key = vec![0u8; 1952];
        // verify() should return false, not panic
        let _ = input.verify(&fake_hash, &fake_key, &fake_pq_key);
    }
});
