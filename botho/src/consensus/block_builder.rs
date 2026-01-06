// Copyright (c) 2024 Botho Foundation

//! Block builder for constructing blocks from externalized consensus
//! transactions.
//!
//! This module handles:
//! - Building blocks from externalized MintingTx and Transaction values
//! - Computing merkle roots for transactions
//! - Ensuring only one MintingTx per block (the consensus winner)

use crate::{
    block::{Block, BlockHeader, MintingTx},
    consensus::ConsensusValue,
    transaction::Transaction,
};
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

/// Result of building a block from externalized values
#[derive(Debug)]
pub struct BuiltBlock {
    pub block: Block,
    pub minting_tx_hash: [u8; 32],
    pub transfer_tx_hashes: Vec<[u8; 32]>,
}

/// Block builder that constructs blocks from externalized consensus values
pub struct BlockBuilder;

impl BlockBuilder {
    /// Build a block from externalized consensus values
    ///
    /// Arguments:
    /// - `values`: The externalized ConsensusValues (must include exactly one
    ///   MintingTx)
    /// - `get_minting_tx`: Callback to retrieve MintingTx data by hash
    /// - `get_transfer_tx`: Callback to retrieve Transaction data by hash
    ///
    /// Returns the built block or an error
    pub fn build_from_externalized<F, G>(
        values: &[ConsensusValue],
        get_minting_tx: F,
        get_transfer_tx: G,
    ) -> Result<BuiltBlock, BlockBuildError>
    where
        F: Fn(&[u8; 32]) -> Option<MintingTx>,
        G: Fn(&[u8; 32]) -> Option<Transaction>,
    {
        // Separate minting transactions from transfer transactions
        let minting_values: Vec<_> = values.iter().filter(|v| v.is_minting_tx).collect();
        let transfer_values: Vec<_> = values.iter().filter(|v| !v.is_minting_tx).collect();

        // Must have exactly one minting transaction
        if minting_values.is_empty() {
            return Err(BlockBuildError::NoMintingTx);
        }
        if minting_values.len() > 1 {
            warn!(
                "Multiple minting txs externalized ({}), using first (highest priority)",
                minting_values.len()
            );
        }

        // Get the winning minting transaction (first one has highest priority from
        // combine_fn)
        let winning_minting_value = minting_values[0];
        let minting_tx = get_minting_tx(&winning_minting_value.tx_hash).ok_or(
            BlockBuildError::MintingTxNotFound(winning_minting_value.tx_hash),
        )?;

        debug!(
            height = minting_tx.block_height,
            "Building block from externalized minting tx"
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
            height = minting_tx.block_height,
            transfer_txs = transactions.len(),
            "Building block with {} transfer transactions",
            transactions.len()
        );

        // Compute transaction merkle root
        let tx_root = Self::compute_tx_root(&transactions);

        // Build the block
        // Note: Lottery outputs are added separately via set_lottery_result()
        // after the block is built, typically by the consensus/minting process
        let block = Block {
            header: BlockHeader {
                version: 1,
                prev_block_hash: minting_tx.prev_block_hash,
                tx_root,
                timestamp: minting_tx.timestamp,
                height: minting_tx.block_height,
                difficulty: minting_tx.difficulty,
                nonce: minting_tx.nonce,
                minter_view_key: minting_tx.minter_view_key,
                minter_spend_key: minting_tx.minter_spend_key,
            },
            minting_tx,
            transactions,
            lottery_outputs: Vec::new(),
            lottery_summary: crate::block::BlockLotterySummary::default(),
        };

        Ok(BuiltBlock {
            block,
            minting_tx_hash: winning_minting_value.tx_hash,
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

    /// Build a block directly from a MintingTx and list of Transactions
    /// (Convenience method for non-consensus block building)
    pub fn build_direct(minting_tx: MintingTx, transactions: Vec<Transaction>) -> Block {
        let tx_root = Self::compute_tx_root(&transactions);

        Block {
            header: BlockHeader {
                version: 1,
                prev_block_hash: minting_tx.prev_block_hash,
                tx_root,
                timestamp: minting_tx.timestamp,
                height: minting_tx.block_height,
                difficulty: minting_tx.difficulty,
                nonce: minting_tx.nonce,
                minter_view_key: minting_tx.minter_view_key,
                minter_spend_key: minting_tx.minter_spend_key,
            },
            minting_tx,
            transactions,
            lottery_outputs: Vec::new(),
            lottery_summary: crate::block::BlockLotterySummary::default(),
        }
    }
}

/// Errors that can occur during block building
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockBuildError {
    /// No minting transaction in externalized values
    NoMintingTx,
    /// Minting transaction not found in cache
    MintingTxNotFound([u8; 32]),
    /// Invalid minting transaction
    InvalidMintingTx(String),
}

impl std::fmt::Display for BlockBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoMintingTx => write!(f, "No minting transaction in externalized values"),
            Self::MintingTxNotFound(hash) => {
                write!(f, "Minting tx not found: {}", hex::encode(&hash[0..8]))
            }
            Self::InvalidMintingTx(e) => write!(f, "Invalid minting tx: {}", e),
        }
    }
}

impl std::error::Error for BlockBuildError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_minting_tx(height: u64) -> MintingTx {
        MintingTx {
            block_height: height,
            reward: 1_000_000_000_000,
            minter_view_key: [1u8; 32],
            minter_spend_key: [2u8; 32],
            target_key: [3u8; 32],
            public_key: [4u8; 32],
            prev_block_hash: [0u8; 32],
            difficulty: 1000,
            nonce: 12345,
            timestamp: 1000000,
        }
    }

    #[test]
    fn test_build_direct() {
        let minting_tx = mock_minting_tx(1);
        let block = BlockBuilder::build_direct(minting_tx.clone(), vec![]);

        assert_eq!(block.height(), 1);
        assert_eq!(block.minting_tx, minting_tx);
        assert!(block.transactions.is_empty());
        assert_eq!(block.header.tx_root, [0u8; 32]);
    }

    #[test]
    fn test_build_from_externalized_no_minting_tx() {
        let values = vec![ConsensusValue::from_transaction([1u8; 32])];

        let result = BlockBuilder::build_from_externalized(&values, |_| None, |_| None);

        assert!(matches!(result, Err(BlockBuildError::NoMintingTx)));
    }
}
