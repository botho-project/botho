// Copyright (c) 2024 Cadence Foundation

//! Transaction validation for consensus.
//!
//! This module provides separate validation logic for:
//! - Mining transactions (PoW-based coinbase rewards)
//! - Transfer transactions (UTXO-based value transfers)

use crate::block::{calculate_block_reward, MiningTx};
use crate::ledger::ChainState;
use crate::transaction::Transaction;
use std::sync::{Arc, RwLock};
use tracing::{debug, warn};

/// Validation errors for transactions
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    // Mining transaction errors
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

    /// Validate a mining transaction
    pub fn validate_mining_tx(&self, tx: &MiningTx) -> Result<(), ValidationError> {
        let state = self
            .chain_state
            .read()
            .map_err(|_| ValidationError::ChainStateUnavailable)?;

        debug!(
            height = tx.block_height,
            "Validating mining transaction"
        );

        // Check cheap validations first before expensive PoW verification

        // 1. Check prev_block_hash matches current chain tip
        if tx.prev_block_hash != state.tip_hash {
            warn!(
                expected = hex::encode(&state.tip_hash[0..8]),
                got = hex::encode(&tx.prev_block_hash[0..8]),
                "Mining tx has wrong prev_block_hash"
            );
            return Err(ValidationError::WrongPrevBlockHash);
        }

        // 2. Check block_height is next expected
        let expected_height = state.height + 1;
        if tx.block_height != expected_height {
            warn!(
                expected = expected_height,
                got = tx.block_height,
                "Mining tx has wrong block height"
            );
            return Err(ValidationError::WrongBlockHeight);
        }

        // 3. Check difficulty matches current network difficulty
        if tx.difficulty != state.difficulty {
            warn!(
                expected = state.difficulty,
                got = tx.difficulty,
                "Mining tx has wrong difficulty"
            );
            return Err(ValidationError::WrongDifficulty);
        }

        // 4. Check reward matches emission schedule
        let expected_reward = calculate_block_reward(tx.block_height, state.total_mined);
        if tx.reward != expected_reward {
            warn!(
                expected = expected_reward,
                got = tx.reward,
                "Mining tx has wrong reward"
            );
            return Err(ValidationError::WrongReward {
                expected: expected_reward,
                got: tx.reward,
            });
        }

        // 5. Check timestamp is reasonable
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        if tx.timestamp > now + MAX_FUTURE_TIMESTAMP_SECS {
            warn!(
                timestamp = tx.timestamp,
                now = now,
                "Mining tx timestamp too far in future"
            );
            return Err(ValidationError::TimestampTooFarInFuture);
        }

        // Note: We don't check if timestamp is before parent here because
        // that requires looking up the parent block. The consensus layer
        // should handle this during block construction.

        // 6. Verify PoW (hash < difficulty) - expensive, so do last
        if !tx.verify_pow() {
            warn!("Mining tx failed PoW verification");
            return Err(ValidationError::InvalidPoW);
        }

        debug!(
            height = tx.block_height,
            "Mining transaction validated successfully"
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
        // TODO: These require access to the UTXO set and proper crypto
        // For now, we accept structurally valid transactions
        // Full validation happens when adding to the mempool

        debug!("Transfer transaction validated successfully");
        Ok(())
    }

    /// Validate a transaction from its serialized form
    pub fn validate_from_bytes(
        &self,
        tx_bytes: &[u8],
        is_mining_tx: bool,
    ) -> Result<(), ValidationError> {
        if is_mining_tx {
            let tx: MiningTx = bincode::deserialize(tx_bytes)
                .map_err(|e| ValidationError::DeserializationFailed(e.to_string()))?;
            self.validate_mining_tx(&tx)
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
        txs: &[([u8; 32], Vec<u8>, bool)], // (hash, bytes, is_mining_tx)
    ) -> BatchValidationResult {
        let mut valid = Vec::new();
        let mut invalid = Vec::new();

        for (hash, bytes, is_mining_tx) in txs {
            match self.validate_from_bytes(bytes, *is_mining_tx) {
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

    fn mock_chain_state() -> Arc<RwLock<ChainState>> {
        Arc::new(RwLock::new(ChainState {
            height: 10,
            tip_hash: [0u8; 32],
            difficulty: 1000,
            total_mined: 1_000_000_000_000,
        }))
    }

    #[test]
    fn test_mining_tx_wrong_height() {
        let validator = TransactionValidator::new(mock_chain_state());

        let tx = MiningTx {
            block_height: 5, // Wrong - should be 11
            reward: 600_000_000_000,
            miner_view_key: [0u8; 32],
            miner_spend_key: [0u8; 32],
            target_key: [0u8; 32],
            public_key: [0u8; 32],
            prev_block_hash: [0u8; 32],
            difficulty: 1000,
            nonce: 0,
            timestamp: 0,
        };

        let result = validator.validate_mining_tx(&tx);
        assert!(matches!(result, Err(ValidationError::WrongBlockHeight)));
    }

    #[test]
    fn test_transfer_tx_no_inputs() {
        let validator = TransactionValidator::new(mock_chain_state());

        let tx = Transaction::new(vec![], vec![], 0, 10);
        let result = validator.validate_transfer_tx(&tx);
        assert!(matches!(result, Err(ValidationError::NoInputs)));
    }
}
