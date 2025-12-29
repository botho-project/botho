// Copyright (c) 2024 Cadence Foundation

//! Block builder for constructing blocks from externalized consensus transactions.
//!
//! This module handles:
//! - Building blocks from externalized MiningTx and Transaction values
//! - Computing merkle roots for transactions
//! - Ensuring only one MiningTx per block (the consensus winner)

use crate::block::{Block, BlockHeader, MiningTx};
use crate::consensus::ConsensusValue;
use crate::transaction::Transaction;
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

/// Result of building a block from externalized values
#[derive(Debug)]
pub struct BuiltBlock {
    pub block: Block,
    pub mining_tx_hash: [u8; 32],
    pub transfer_tx_hashes: Vec<[u8; 32]>,
}

/// Block builder that constructs blocks from externalized consensus values
pub struct BlockBuilder;

impl BlockBuilder {
    /// Build a block from externalized consensus values
    ///
    /// Arguments:
    /// - `values`: The externalized ConsensusValues (must include exactly one MiningTx)
    /// - `get_mining_tx`: Callback to retrieve MiningTx data by hash
    /// - `get_transfer_tx`: Callback to retrieve Transaction data by hash
    ///
    /// Returns the built block or an error
    pub fn build_from_externalized<F, G>(
        values: &[ConsensusValue],
        get_mining_tx: F,
        get_transfer_tx: G,
    ) -> Result<BuiltBlock, BlockBuildError>
    where
        F: Fn(&[u8; 32]) -> Option<MiningTx>,
        G: Fn(&[u8; 32]) -> Option<Transaction>,
    {
        // Separate mining transactions from transfer transactions
        let mining_values: Vec<_> = values.iter().filter(|v| v.is_mining_tx).collect();
        let transfer_values: Vec<_> = values.iter().filter(|v| !v.is_mining_tx).collect();

        // Must have exactly one mining transaction
        if mining_values.is_empty() {
            return Err(BlockBuildError::NoMiningTx);
        }
        if mining_values.len() > 1 {
            warn!(
                "Multiple mining txs externalized ({}), using first (highest priority)",
                mining_values.len()
            );
        }

        // Get the winning mining transaction (first one has highest priority from combine_fn)
        let winning_mining_value = mining_values[0];
        let mining_tx = get_mining_tx(&winning_mining_value.tx_hash)
            .ok_or(BlockBuildError::MiningTxNotFound(winning_mining_value.tx_hash))?;

        debug!(
            height = mining_tx.block_height,
            "Building block from externalized mining tx"
        );

        // Collect transfer transactions
        let mut transactions = Vec::new();
        let mut transfer_tx_hashes = Vec::new();

        for value in &transfer_values {
            match get_transfer_tx(&value.tx_hash) {
                Some(tx) => {
                    transfer_tx_hashes.push(value.tx_hash);
                    transactions.push(tx);
                }
                None => {
                    warn!(
                        hash = hex::encode(&value.tx_hash[0..8]),
                        "Transfer tx not found in cache, skipping"
                    );
                }
            }
        }

        info!(
            height = mining_tx.block_height,
            transfer_txs = transactions.len(),
            "Building block with {} transfer transactions",
            transactions.len()
        );

        // Compute transaction merkle root
        let tx_root = Self::compute_tx_root(&transactions);

        // Build the block
        let block = Block {
            header: BlockHeader {
                version: 1,
                prev_block_hash: mining_tx.prev_block_hash,
                tx_root,
                timestamp: mining_tx.timestamp,
                height: mining_tx.block_height,
                difficulty: mining_tx.difficulty,
                nonce: mining_tx.nonce,
                miner_view_key: mining_tx.recipient_view_key,
                miner_spend_key: mining_tx.recipient_spend_key,
            },
            mining_tx,
            transactions,
        };

        Ok(BuiltBlock {
            block,
            mining_tx_hash: winning_mining_value.tx_hash,
            transfer_tx_hashes,
        })
    }

    /// Compute merkle root of transactions
    fn compute_tx_root(transactions: &[Transaction]) -> [u8; 32] {
        if transactions.is_empty() {
            return [0u8; 32];
        }

        let mut hasher = Sha256::new();
        for tx in transactions {
            hasher.update(tx.hash());
        }
        hasher.finalize().into()
    }

    /// Build a block directly from a MiningTx and list of Transactions
    /// (Convenience method for non-consensus block building)
    pub fn build_direct(mining_tx: MiningTx, transactions: Vec<Transaction>) -> Block {
        let tx_root = Self::compute_tx_root(&transactions);

        Block {
            header: BlockHeader {
                version: 1,
                prev_block_hash: mining_tx.prev_block_hash,
                tx_root,
                timestamp: mining_tx.timestamp,
                height: mining_tx.block_height,
                difficulty: mining_tx.difficulty,
                nonce: mining_tx.nonce,
                miner_view_key: mining_tx.recipient_view_key,
                miner_spend_key: mining_tx.recipient_spend_key,
            },
            mining_tx,
            transactions,
        }
    }
}

/// Errors that can occur during block building
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockBuildError {
    /// No mining transaction in externalized values
    NoMiningTx,
    /// Mining transaction not found in cache
    MiningTxNotFound([u8; 32]),
    /// Invalid mining transaction
    InvalidMiningTx(String),
}

impl std::fmt::Display for BlockBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoMiningTx => write!(f, "No mining transaction in externalized values"),
            Self::MiningTxNotFound(hash) => {
                write!(f, "Mining tx not found: {}", hex::encode(&hash[0..8]))
            }
            Self::InvalidMiningTx(e) => write!(f, "Invalid mining tx: {}", e),
        }
    }
}

impl std::error::Error for BlockBuildError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_mining_tx(height: u64) -> MiningTx {
        MiningTx {
            block_height: height,
            reward: 1_000_000_000_000,
            recipient_view_key: [1u8; 32],
            recipient_spend_key: [2u8; 32],
            output_public_key: [3u8; 32],
            prev_block_hash: [0u8; 32],
            difficulty: 1000,
            nonce: 12345,
            timestamp: 1000000,
        }
    }

    #[test]
    fn test_build_direct() {
        let mining_tx = mock_mining_tx(1);
        let block = BlockBuilder::build_direct(mining_tx.clone(), vec![]);

        assert_eq!(block.height(), 1);
        assert_eq!(block.mining_tx, mining_tx);
        assert!(block.transactions.is_empty());
        assert_eq!(block.header.tx_root, [0u8; 32]);
    }

    #[test]
    fn test_build_from_externalized_no_mining_tx() {
        let values = vec![ConsensusValue::from_transaction([1u8; 32])];

        let result = BlockBuilder::build_from_externalized(
            &values,
            |_| None,
            |_| None,
        );

        assert!(matches!(result, Err(BlockBuildError::NoMiningTx)));
    }
}
