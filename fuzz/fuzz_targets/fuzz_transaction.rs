#![no_main]

use libfuzzer_sys::fuzz_target;

use botho::transaction::{Transaction, TxInput, TxOutput, Utxo};

// Fuzz target for Transaction deserialization.
//
// This tests that malformed transaction data cannot cause panics or undefined behavior
// during deserialization. The deserializer should gracefully return an error for any
// invalid input.
//
// Security rationale: Transactions are received from untrusted peers over the network.
// A malformed transaction must never cause a crash, memory corruption, or infinite loop.
fuzz_target!(|data: &[u8]| {
    // Test Transaction deserialization - most common attack vector
    let _ = bincode::deserialize::<Transaction>(data);

    // Test individual components that might be parsed separately
    let _ = bincode::deserialize::<TxInput>(data);
    let _ = bincode::deserialize::<TxOutput>(data);
    let _ = bincode::deserialize::<Utxo>(data);

    // If deserialization succeeds, verify the transaction doesn't panic on validation
    if let Ok(tx) = bincode::deserialize::<Transaction>(data) {
        // These should not panic even on malformed but deserializable transactions
        let _ = tx.hash();
        let _ = tx.signing_hash();
        let _ = tx.total_output();
        let _ = tx.is_valid_structure();
    }
});
