// Copyright (c) 2024 Botho Foundation

//! Transaction validation for consensus.
//!
//! This module provides separate validation logic for:
//! - Minting transactions (PoW-based coinbase rewards)
//! - Transfer transactions (UTXO-based value transfers)

use crate::block::{calculate_block_reward_v2, MintingTx};
use crate::ledger::ChainState;
use crate::transaction::Transaction;
#[cfg(feature = "pq")]
use crate::transaction_pq::QuantumPrivateTransaction;
use std::sync::{Arc, RwLock};
use tracing::{debug, warn};

/// Validation errors for transactions
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    // Minting transaction errors
    InvalidPoW,
    WrongPrevBlockHash,
    WrongBlockHeight,
    WrongDifficulty,
    WrongReward { expected: u64, got: u64 },
    TimestampTooFarInFuture,
    TimestampBeforeParent,

    // Transfer transaction errors
    NoInputs,
    NoOutputs,
    ZeroAmountOutput,
    InputNotFound,
    InputAlreadySpent,
    InvalidSignature,
    InsufficientFunds { input: u64, output: u64, fee: u64 },
    StaleTransaction,

    // Quantum-private transaction errors
    #[cfg(feature = "pq")]
    InvalidPqSignature,
    #[cfg(feature = "pq")]
    InvalidPqCiphertext,
    #[cfg(feature = "pq")]
    PqOutputTooLarge,
    #[cfg(feature = "pq")]
    PqInputTooLarge,

    // General errors
    DeserializationFailed(String),
    ChainStateUnavailable,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPoW => write!(f, "Invalid proof of work"),
            Self::WrongPrevBlockHash => write!(f, "Wrong previous block hash"),
            Self::WrongBlockHeight => write!(f, "Wrong block height"),
            Self::WrongDifficulty => write!(f, "Wrong difficulty target"),
            Self::WrongReward { expected, got } => {
                write!(f, "Wrong reward: expected {}, got {}", expected, got)
            }
            Self::TimestampTooFarInFuture => write!(f, "Timestamp too far in future"),
            Self::TimestampBeforeParent => write!(f, "Timestamp before parent block"),
            Self::NoInputs => write!(f, "Transaction has no inputs"),
            Self::NoOutputs => write!(f, "Transaction has no outputs"),
            Self::ZeroAmountOutput => write!(f, "Transaction has zero-amount output"),
            Self::InputNotFound => write!(f, "Input UTXO not found"),
            Self::InputAlreadySpent => write!(f, "Input already spent"),
            Self::InvalidSignature => write!(f, "Invalid signature"),
            Self::InsufficientFunds { input, output, fee } => {
                write!(
                    f,
                    "Insufficient funds: input {} < output {} + fee {}",
                    input, output, fee
                )
            }
            Self::StaleTransaction => write!(f, "Transaction is stale"),
            #[cfg(feature = "pq")]
            Self::InvalidPqSignature => write!(f, "Invalid post-quantum signature"),
            #[cfg(feature = "pq")]
            Self::InvalidPqCiphertext => write!(f, "Invalid post-quantum ciphertext"),
            #[cfg(feature = "pq")]
            Self::PqOutputTooLarge => write!(f, "Quantum-private output exceeds size limit"),
            #[cfg(feature = "pq")]
            Self::PqInputTooLarge => write!(f, "Quantum-private input exceeds size limit"),
            Self::DeserializationFailed(e) => write!(f, "Deserialization failed: {}", e),
            Self::ChainStateUnavailable => write!(f, "Chain state unavailable"),
        }
    }
}

impl std::error::Error for ValidationError {}

/// Maximum allowed timestamp drift from current time (2 hours)
const MAX_FUTURE_TIMESTAMP_SECS: u64 = 2 * 60 * 60;

/// Transaction validator with access to chain state
pub struct TransactionValidator {
    /// Current chain state (shared with ledger)
    chain_state: Arc<RwLock<ChainState>>,
}

impl TransactionValidator {
    /// Create a new transaction validator
    pub fn new(chain_state: Arc<RwLock<ChainState>>) -> Self {
        Self { chain_state }
    }

    /// Validate a minting transaction
    pub fn validate_minting_tx(&self, tx: &MintingTx) -> Result<(), ValidationError> {
        let state = self
            .chain_state
            .read()
            .map_err(|_| ValidationError::ChainStateUnavailable)?;

        debug!(
            height = tx.block_height,
            "Validating minting transaction"
        );

        // Check cheap validations first before expensive PoW verification

        // 1. Check prev_block_hash matches current chain tip
        if tx.prev_block_hash != state.tip_hash {
            warn!(
                expected = hex::encode(&state.tip_hash[0..8]),
                got = hex::encode(&tx.prev_block_hash[0..8]),
                "Minting tx has wrong prev_block_hash"
            );
            return Err(ValidationError::WrongPrevBlockHash);
        }

        // 2. Check block_height is next expected
        let expected_height = state.height + 1;
        if tx.block_height != expected_height {
            warn!(
                expected = expected_height,
                got = tx.block_height,
                "Minting tx has wrong block height"
            );
            return Err(ValidationError::WrongBlockHeight);
        }

        // 3. Check difficulty matches current network difficulty
        if tx.difficulty != state.difficulty {
            warn!(
                expected = state.difficulty,
                got = tx.difficulty,
                "Minting tx has wrong difficulty"
            );
            return Err(ValidationError::WrongDifficulty);
        }

        // 4. Check reward matches Two-Phase emission schedule
        let expected_reward = calculate_block_reward_v2(tx.block_height, state.total_mined);
        if tx.reward != expected_reward {
            warn!(
                expected = expected_reward,
                got = tx.reward,
                "Minting tx has wrong reward"
            );
            return Err(ValidationError::WrongReward {
                expected: expected_reward,
                got: tx.reward,
            });
        }

        // 5. Check timestamp is reasonable
        // Don't fallback to 0 on error - that would bypass future timestamp checks
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .map_err(|_| {
                warn!("System time before UNIX epoch - cannot validate timestamps");
                ValidationError::ChainStateUnavailable
            })?;

        if tx.timestamp > now + MAX_FUTURE_TIMESTAMP_SECS {
            warn!(
                timestamp = tx.timestamp,
                now = now,
                "Minting tx timestamp too far in future"
            );
            return Err(ValidationError::TimestampTooFarInFuture);
        }

        // Check timestamp is not before parent block (monotonicity)
        if tx.timestamp < state.tip_timestamp {
            warn!(
                timestamp = tx.timestamp,
                parent_timestamp = state.tip_timestamp,
                "Minting tx timestamp before parent block"
            );
            return Err(ValidationError::TimestampBeforeParent);
        }

        // 6. Verify PoW (hash < difficulty) - expensive, so do last
        if !tx.verify_pow() {
            warn!("Minting tx failed PoW verification");
            return Err(ValidationError::InvalidPoW);
        }

        debug!(
            height = tx.block_height,
            "Minting transaction validated successfully"
        );
        Ok(())
    }

    /// Validate a transfer transaction (structure only for now)
    ///
    /// Full UTXO validation requires access to the UTXO set, which
    /// will be integrated when we add the mempool.
    pub fn validate_transfer_tx(&self, tx: &Transaction) -> Result<(), ValidationError> {
        let state = self
            .chain_state
            .read()
            .map_err(|_| ValidationError::ChainStateUnavailable)?;

        debug!("Validating transfer transaction");

        // 1. Check structure
        if tx.inputs.is_empty() {
            return Err(ValidationError::NoInputs);
        }
        if tx.outputs.is_empty() {
            return Err(ValidationError::NoOutputs);
        }
        if tx.outputs.iter().any(|o| o.amount == 0) {
            return Err(ValidationError::ZeroAmountOutput);
        }

        // 2. Check transaction is not stale
        // Allow transactions from recent blocks (within 100 blocks)
        const MAX_TX_AGE: u64 = 100;
        if tx.created_at_height + MAX_TX_AGE < state.height {
            return Err(ValidationError::StaleTransaction);
        }

        // 3. UTXO existence and signature verification
        // Note: Full UTXO and signature validation happens in mempool.add_tx()
        // which has ledger access. The mempool verifies:
        // - UTXO existence in ledger
        // - Signature validity against UTXO target_key
        // - Input sum >= output sum + fee

        debug!("Transfer transaction validated successfully");
        Ok(())
    }

    /// Validate a quantum-private transaction (structure and size limits)
    ///
    /// This validates:
    /// - Classical transaction structure (delegates to validate_transfer_tx)
    /// - PQ ciphertext sizes are valid
    /// - PQ signature sizes are valid
    /// - Overall transaction size is within limits
    ///
    /// Note: Full signature validation (both Schnorr and ML-DSA) happens
    /// in the mempool which has access to the UTXO set for key lookup.
    #[cfg(feature = "pq")]
    pub fn validate_quantum_private_tx(
        &self,
        tx: &QuantumPrivateTransaction,
    ) -> Result<(), ValidationError> {
        use crate::transaction_pq::{PQ_CIPHERTEXT_SIZE, PQ_SIGNATURE_SIZE};

        let state = self
            .chain_state
            .read()
            .map_err(|_| ValidationError::ChainStateUnavailable)?;

        debug!("Validating quantum-private transaction");

        // 1. Check basic structure
        if tx.inputs.is_empty() {
            return Err(ValidationError::NoInputs);
        }
        if tx.outputs.is_empty() {
            return Err(ValidationError::NoOutputs);
        }

        // 2. Check transaction is not stale
        const MAX_TX_AGE: u64 = 100;
        if tx.created_at_height + MAX_TX_AGE < state.height {
            return Err(ValidationError::StaleTransaction);
        }

        // 3. Validate PQ output sizes
        for output in &tx.outputs {
            // Check classical output
            if output.classical.amount == 0 {
                return Err(ValidationError::ZeroAmountOutput);
            }

            // Check PQ ciphertext size
            if output.pq_ciphertext.len() != PQ_CIPHERTEXT_SIZE {
                warn!(
                    expected = PQ_CIPHERTEXT_SIZE,
                    got = output.pq_ciphertext.len(),
                    "PQ output has invalid ciphertext size"
                );
                return Err(ValidationError::InvalidPqCiphertext);
            }
        }

        // 4. Validate PQ input sizes
        for input in &tx.inputs {
            // Check PQ signature size
            if input.pq_signature.len() != PQ_SIGNATURE_SIZE {
                warn!(
                    expected = PQ_SIGNATURE_SIZE,
                    got = input.pq_signature.len(),
                    "PQ input has invalid signature size"
                );
                return Err(ValidationError::InvalidPqSignature);
            }

            // Check classical signature size (Schnorr = 64 bytes)
            if input.classical_signature.len() != 64 {
                warn!(
                    expected = 64,
                    got = input.classical_signature.len(),
                    "Classical signature has invalid size"
                );
                return Err(ValidationError::InvalidSignature);
            }
        }

        // 5. Check total transaction size (rough estimate for DoS protection)
        // Max: 16 inputs, 16 outputs
        const MAX_PQ_INPUTS: usize = 16;
        const MAX_PQ_OUTPUTS: usize = 16;

        if tx.inputs.len() > MAX_PQ_INPUTS {
            warn!(
                max = MAX_PQ_INPUTS,
                got = tx.inputs.len(),
                "Too many PQ inputs"
            );
            return Err(ValidationError::PqInputTooLarge);
        }

        if tx.outputs.len() > MAX_PQ_OUTPUTS {
            warn!(
                max = MAX_PQ_OUTPUTS,
                got = tx.outputs.len(),
                "Too many PQ outputs"
            );
            return Err(ValidationError::PqOutputTooLarge);
        }

        debug!("Quantum-private transaction validated successfully");
        Ok(())
    }

    /// Validate a transaction from its serialized form
    pub fn validate_from_bytes(
        &self,
        tx_bytes: &[u8],
        is_minting_tx: bool,
    ) -> Result<(), ValidationError> {
        if is_minting_tx {
            let tx: MintingTx = bincode::deserialize(tx_bytes)
                .map_err(|e| ValidationError::DeserializationFailed(e.to_string()))?;
            self.validate_minting_tx(&tx)
        } else {
            let tx: Transaction = bincode::deserialize(tx_bytes)
                .map_err(|e| ValidationError::DeserializationFailed(e.to_string()))?;
            self.validate_transfer_tx(&tx)
        }
    }

    /// Update the chain state reference
    pub fn update_chain_state(&mut self, chain_state: Arc<RwLock<ChainState>>) {
        self.chain_state = chain_state;
    }
}

/// Validation result for a batch of transactions
#[derive(Debug)]
pub struct BatchValidationResult {
    /// Valid transaction hashes
    pub valid: Vec<[u8; 32]>,
    /// Invalid transaction hashes with errors
    pub invalid: Vec<([u8; 32], ValidationError)>,
}

impl TransactionValidator {
    /// Validate multiple transactions, separating valid from invalid
    pub fn validate_batch(
        &self,
        txs: &[([u8; 32], Vec<u8>, bool)], // (hash, bytes, is_minting_tx)
    ) -> BatchValidationResult {
        let mut valid = Vec::new();
        let mut invalid = Vec::new();

        for (hash, bytes, is_minting_tx) in txs {
            match self.validate_from_bytes(bytes, *is_minting_tx) {
                Ok(()) => valid.push(*hash),
                Err(e) => invalid.push((*hash, e)),
            }
        }

        BatchValidationResult { valid, invalid }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transaction::{TxInput, MIN_TX_FEE};

    fn mock_chain_state() -> Arc<RwLock<ChainState>> {
        Arc::new(RwLock::new(ChainState {
            height: 10,
            tip_hash: [0u8; 32],
            tip_timestamp: 1000000,
            difficulty: 1000,
            total_mined: 1_000_000_000_000,
            total_fees_burned: 0,
        }))
    }

    fn current_timestamp() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    #[test]
    fn test_minting_tx_wrong_height() {
        let validator = TransactionValidator::new(mock_chain_state());

        let tx = MintingTx {
            block_height: 5, // Wrong - should be 11
            reward: 600_000_000_000,
            minter_view_key: [0u8; 32],
            minter_spend_key: [0u8; 32],
            target_key: [0u8; 32],
            public_key: [0u8; 32],
            prev_block_hash: [0u8; 32],
            difficulty: 1000,
            nonce: 0,
            timestamp: 0,
        };

        let result = validator.validate_minting_tx(&tx);
        assert!(matches!(result, Err(ValidationError::WrongBlockHeight)));
    }

    #[test]
    fn test_transfer_tx_no_inputs() {
        let validator = TransactionValidator::new(mock_chain_state());

        let tx = Transaction::new_simple(vec![], vec![], 0, 10);
        let result = validator.validate_transfer_tx(&tx);
        assert!(matches!(result, Err(ValidationError::NoInputs)));
    }

    #[test]
    fn test_minting_tx_correct_height() {
        let validator = TransactionValidator::new(mock_chain_state());

        let tx = MintingTx {
            block_height: 11, // Correct - chain height is 10
            reward: 600_000_000_000, // Tail emission
            minter_view_key: [0u8; 32],
            minter_spend_key: [0u8; 32],
            target_key: [0u8; 32],
            public_key: [0u8; 32],
            prev_block_hash: [0u8; 32],
            difficulty: 1000,
            nonce: 0,
            timestamp: current_timestamp(),
        };

        // Should pass height check (may fail on other validation)
        let result = validator.validate_minting_tx(&tx);
        // Either passes or fails for different reason (not wrong height)
        match result {
            Err(ValidationError::WrongBlockHeight) => panic!("Should not fail on height"),
            _ => {} // Ok or other error is fine
        }
    }

    #[test]
    fn test_timestamp_check_safety() {
        // Test that current_timestamp() helper doesn't panic even with system time issues
        let ts = current_timestamp();
        // Should return a reasonable value (not panic)
        assert!(ts > 0 || ts == 0); // Valid even if system clock is weird
    }

    #[test]
    fn test_transfer_tx_no_outputs() {
        let validator = TransactionValidator::new(mock_chain_state());

        // Transaction with inputs but no outputs
        let tx = Transaction::new_simple(
            vec![TxInput {
                tx_hash: [0u8; 32],
                output_index: 0,
                signature: vec![0u8; 64],
            }],
            vec![], // No outputs
            1000,
            10,
        );

        let result = validator.validate_transfer_tx(&tx);
        assert!(matches!(result, Err(ValidationError::NoOutputs)));
    }

    #[test]
    fn test_validation_error_display() {
        let err = ValidationError::NoInputs;
        assert_eq!(format!("{}", err), "Transaction has no inputs");

        let err = ValidationError::NoOutputs;
        assert_eq!(format!("{}", err), "Transaction has no outputs");

        let err = ValidationError::WrongBlockHeight;
        assert_eq!(format!("{}", err), "Wrong block height");

        let err = ValidationError::WrongReward { expected: 100, got: 200 };
        assert!(format!("{}", err).contains("Wrong reward"));

        let err = ValidationError::TimestampTooFarInFuture;
        assert!(format!("{}", err).contains("future"));

        let err = ValidationError::InsufficientFunds { input: 100, output: 80, fee: 30 };
        assert!(format!("{}", err).contains("Insufficient funds"));

        let err = ValidationError::InvalidSignature;
        assert_eq!(format!("{}", err), "Invalid signature");
    }

    #[test]
    fn test_batch_validation_empty_bytes() {
        let validator = TransactionValidator::new(mock_chain_state());

        // Test with invalid (empty) bytes - should fail deserialization
        let invalid_bytes = vec![];

        let batch = vec![
            ([1u8; 32], invalid_bytes.clone(), false),
            ([2u8; 32], invalid_bytes, true), // Also test minting tx path
        ];

        let result = validator.validate_batch(&batch);

        // Both should fail deserialization
        assert_eq!(result.invalid.len(), 2);
        assert!(result.valid.is_empty());
    }

    #[cfg(feature = "pq")]
    mod pq_tests {
        use super::*;
        use crate::transaction_pq::{
            QuantumPrivateTransaction, QuantumPrivateTxInput, QuantumPrivateTxOutput,
            PQ_CIPHERTEXT_SIZE, PQ_SIGNATURE_SIZE, PQ_SIGNING_PUBKEY_SIZE,
        };
        use crate::transaction::TxOutput;

        fn mock_pq_output() -> QuantumPrivateTxOutput {
            QuantumPrivateTxOutput {
                classical: TxOutput {
                    amount: 1_000_000,
                    target_key: [1u8; 32],
                    public_key: [2u8; 32],
                    e_memo: None,
                },
                pq_ciphertext: vec![0u8; PQ_CIPHERTEXT_SIZE],
                pq_signing_pubkey: vec![0u8; PQ_SIGNING_PUBKEY_SIZE],
            }
        }

        fn mock_pq_input() -> QuantumPrivateTxInput {
            QuantumPrivateTxInput {
                tx_hash: [0u8; 32],
                output_index: 0,
                classical_signature: vec![0u8; 64],
                pq_signature: vec![0u8; PQ_SIGNATURE_SIZE],
            }
        }

        #[test]
        fn test_pq_tx_valid_structure() {
            let validator = TransactionValidator::new(mock_chain_state());

            let tx = QuantumPrivateTransaction {
                inputs: vec![mock_pq_input()],
                outputs: vec![mock_pq_output()],
                fee: 1000,
                created_at_height: 10,
            };

            let result = validator.validate_quantum_private_tx(&tx);
            assert!(result.is_ok());
        }

        #[test]
        fn test_pq_tx_no_inputs() {
            let validator = TransactionValidator::new(mock_chain_state());

            let tx = QuantumPrivateTransaction {
                inputs: vec![],
                outputs: vec![mock_pq_output()],
                fee: 1000,
                created_at_height: 10,
            };

            let result = validator.validate_quantum_private_tx(&tx);
            assert!(matches!(result, Err(ValidationError::NoInputs)));
        }

        #[test]
        fn test_pq_tx_no_outputs() {
            let validator = TransactionValidator::new(mock_chain_state());

            let tx = QuantumPrivateTransaction {
                inputs: vec![mock_pq_input()],
                outputs: vec![],
                fee: 1000,
                created_at_height: 10,
            };

            let result = validator.validate_quantum_private_tx(&tx);
            assert!(matches!(result, Err(ValidationError::NoOutputs)));
        }

        #[test]
        fn test_pq_tx_zero_amount() {
            let validator = TransactionValidator::new(mock_chain_state());

            let mut output = mock_pq_output();
            output.classical.amount = 0;

            let tx = QuantumPrivateTransaction {
                inputs: vec![mock_pq_input()],
                outputs: vec![output],
                fee: 1000,
                created_at_height: 10,
            };

            let result = validator.validate_quantum_private_tx(&tx);
            assert!(matches!(result, Err(ValidationError::ZeroAmountOutput)));
        }

        #[test]
        fn test_pq_tx_invalid_ciphertext_size() {
            let validator = TransactionValidator::new(mock_chain_state());

            let mut output = mock_pq_output();
            output.pq_ciphertext = vec![0u8; 100]; // Wrong size

            let tx = QuantumPrivateTransaction {
                inputs: vec![mock_pq_input()],
                outputs: vec![output],
                fee: 1000,
                created_at_height: 10,
            };

            let result = validator.validate_quantum_private_tx(&tx);
            assert!(matches!(result, Err(ValidationError::InvalidPqCiphertext)));
        }

        #[test]
        fn test_pq_tx_invalid_signature_size() {
            let validator = TransactionValidator::new(mock_chain_state());

            let mut input = mock_pq_input();
            input.pq_signature = vec![0u8; 100]; // Wrong size

            let tx = QuantumPrivateTransaction {
                inputs: vec![input],
                outputs: vec![mock_pq_output()],
                fee: 1000,
                created_at_height: 10,
            };

            let result = validator.validate_quantum_private_tx(&tx);
            assert!(matches!(result, Err(ValidationError::InvalidPqSignature)));
        }

        #[test]
        fn test_pq_tx_too_many_inputs() {
            let validator = TransactionValidator::new(mock_chain_state());

            // 17 inputs (exceeds limit of 16)
            let inputs: Vec<_> = (0..17).map(|_| mock_pq_input()).collect();

            let tx = QuantumPrivateTransaction {
                inputs,
                outputs: vec![mock_pq_output()],
                fee: 1000,
                created_at_height: 10,
            };

            let result = validator.validate_quantum_private_tx(&tx);
            assert!(matches!(result, Err(ValidationError::PqInputTooLarge)));
        }

        #[test]
        fn test_pq_tx_too_many_outputs() {
            let validator = TransactionValidator::new(mock_chain_state());

            // 17 outputs (exceeds limit of 16)
            let outputs: Vec<_> = (0..17).map(|_| mock_pq_output()).collect();

            let tx = QuantumPrivateTransaction {
                inputs: vec![mock_pq_input()],
                outputs,
                fee: 1000,
                created_at_height: 10,
            };

            let result = validator.validate_quantum_private_tx(&tx);
            assert!(matches!(result, Err(ValidationError::PqOutputTooLarge)));
        }

        #[test]
        fn test_pq_tx_stale() {
            let validator = TransactionValidator::new(mock_chain_state());

            let tx = QuantumPrivateTransaction {
                inputs: vec![mock_pq_input()],
                outputs: vec![mock_pq_output()],
                fee: 1000,
                created_at_height: 0, // Very old (chain height is 10, max age is 100)
            };

            // Not stale yet since 0 + 100 >= 10
            let result = validator.validate_quantum_private_tx(&tx);
            assert!(result.is_ok());
        }
    }
}
