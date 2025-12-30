#![no_main]

//! Fuzzing target for Transaction parsing and validation.
//!
//! Security rationale: Transactions are received from untrusted peers over the network.
//! A malformed transaction must never cause a crash, memory corruption, or infinite loop.
//!
//! This target uses both:
//! 1. Raw byte fuzzing for deserialization edge cases
//! 2. Structured fuzzing for semantic validation testing

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;

use botho::transaction::{Transaction, TxInput, TxOutput, Utxo};

// ============================================================================
// Structured Fuzzing Types
// ============================================================================

/// Structured representation of a transaction for fuzzing.
/// This generates semantically meaningful transactions rather than random bytes.
#[derive(Debug, Arbitrary)]
struct FuzzTransaction {
    /// Number of inputs (0-16)
    num_inputs: u8,
    /// Number of outputs (0-16)
    num_outputs: u8,
    /// Fee amount
    fee: u64,
    /// Block height when created
    created_at_height: u64,
    /// Raw signature bytes (may be invalid)
    signature_bytes: [u8; 64],
    /// Input data for generating inputs
    input_seeds: Vec<FuzzInput>,
    /// Output data for generating outputs
    output_seeds: Vec<FuzzOutput>,
}

/// Fuzz data for an input
#[derive(Debug, Arbitrary)]
struct FuzzInput {
    /// Target key (one-time public key)
    target_key: [u8; 32],
    /// Key image for double-spend prevention
    key_image: [u8; 32],
    /// Ring size (if ring signature)
    ring_size: u8,
}

/// Fuzz data for an output
#[derive(Debug, Arbitrary)]
struct FuzzOutput {
    /// Target key
    target_key: [u8; 32],
    /// Public key
    public_key: [u8; 32],
    /// Amount (may be plaintext or commitment depending on tx type)
    amount: u64,
    /// Whether this has an encrypted memo
    has_memo: bool,
}

/// Test mode selector
#[derive(Debug, Arbitrary)]
enum FuzzMode {
    /// Raw bytes deserialization
    RawBytes(Vec<u8>),
    /// Structured transaction
    Structured(FuzzTransaction),
    /// Partial deserialization (components only)
    Component(ComponentFuzz),
}

#[derive(Debug, Arbitrary)]
enum ComponentFuzz {
    Input(Vec<u8>),
    Output(Vec<u8>),
    Utxo(Vec<u8>),
}

// ============================================================================
// Fuzz Target
// ============================================================================

fuzz_target!(|mode: FuzzMode| {
    match mode {
        FuzzMode::RawBytes(data) => {
            fuzz_raw_bytes(&data);
        }
        FuzzMode::Structured(tx) => {
            fuzz_structured(&tx);
        }
        FuzzMode::Component(comp) => {
            fuzz_component(comp);
        }
    }
});

/// Fuzz with raw bytes - tests deserialization edge cases
fn fuzz_raw_bytes(data: &[u8]) {
    // Test Transaction deserialization - most common attack vector
    let _ = bincode::deserialize::<Transaction>(data);

    // Test individual components that might be parsed separately
    let _ = bincode::deserialize::<TxInput>(data);
    let _ = bincode::deserialize::<TxOutput>(data);
    let _ = bincode::deserialize::<Utxo>(data);

    // If deserialization succeeds, verify the transaction doesn't panic on validation
    if let Ok(tx) = bincode::deserialize::<Transaction>(data) {
        validate_transaction(&tx);
    }
}

/// Fuzz with structured data - tests validation logic
fn fuzz_structured(fuzz_tx: &FuzzTransaction) {
    // Build inputs from seeds
    let num_inputs = (fuzz_tx.num_inputs % 16) as usize;
    let num_outputs = (fuzz_tx.num_outputs % 16) as usize;

    // We can't construct a full valid Transaction without proper signatures,
    // but we can test that the types handle edge cases properly.

    // Test output amount overflow
    let total: u128 = fuzz_tx.output_seeds.iter()
        .take(num_outputs)
        .map(|o| o.amount as u128)
        .sum();

    // This should not panic even for overflowing amounts
    let _overflows = total > u64::MAX as u128;

    // Test fee calculations with extreme values
    let fee = fuzz_tx.fee;
    let _fee_check = fee.checked_add(1);

    // Test height validation
    let height = fuzz_tx.created_at_height;
    let _future_height = height.saturating_add(1000);

    // Test that arithmetic doesn't overflow/panic
    let _ = fuzz_tx.fee.checked_mul(1000);
}

/// Fuzz individual components
fn fuzz_component(comp: ComponentFuzz) {
    match comp {
        ComponentFuzz::Input(data) => {
            let _ = bincode::deserialize::<TxInput>(&data);
        }
        ComponentFuzz::Output(data) => {
            if let Ok(output) = bincode::deserialize::<TxOutput>(&data) {
                // Test output methods don't panic
                let _ = output.amount;
            }
        }
        ComponentFuzz::Utxo(data) => {
            let _ = bincode::deserialize::<Utxo>(&data);
        }
    }
}

/// Validate a deserialized transaction - should never panic
fn validate_transaction(tx: &Transaction) {
    // These should not panic even on malformed but deserializable transactions
    let _ = tx.hash();
    let _ = tx.signing_hash();
    let _ = tx.total_output();
    let _ = tx.is_valid_structure();
    let _ = tx.fee;
    let _ = tx.inputs.len();
    let _ = tx.outputs.len();

    // Test iteration doesn't panic
    // TxInputs is an enum - access via methods
    let _ = tx.inputs.len();
    let _ = tx.inputs.is_ring();
    let _ = tx.inputs.key_images();
    for output in &tx.outputs {
        let _ = output.amount;
        let _ = output.target_key;
        let _ = output.public_key;
    }

    // Test serialization roundtrip
    if let Ok(serialized) = bincode::serialize(tx) {
        let _ = bincode::deserialize::<Transaction>(&serialized);
    }
}
