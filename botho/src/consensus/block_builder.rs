// Copyright (c) 2024 Botho Foundation

//! Block builder for constructing blocks from externalized consensus
//! transactions.
//!
//! This module handles:
//! - Building blocks from externalized MintingTx and Transaction values
//! - Computing merkle roots for transactions
//! - Ensuring only one MintingTx per block (the consensus winner)

use crate::{
    block::{Block, BlockHeader, BlockLotterySummary, LotteryOutput, MintingTx},
    consensus::{
        lottery::{draw_lottery_winners, utxo_to_candidate, BlockLotteryResult, LotteryFeeConfig},
        ConsensusValue,
    },
    transaction::{Transaction, Utxo},
};
use bth_cluster_tax::{LotteryCandidate, TagVector};
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

    // ========================================================================
    // Lottery Integration
    // ========================================================================

    /// Apply lottery results to a built block.
    ///
    /// This method draws lottery winners from the UTXO set and adds lottery
    /// outputs to the block. It's called after the basic block is built to
    /// integrate fee redistribution.
    ///
    /// # Arguments
    /// * `block` - The block to add lottery results to
    /// * `lottery_candidates` - UTXOs eligible for lottery participation
    /// * `utxo_lookup` - Function to look up UTXO details by ID for key recovery
    /// * `lottery_config` - Configuration for fee splitting and lottery drawing
    ///
    /// # Returns
    /// Updated block with lottery outputs and summary
    pub fn apply_lottery<F>(
        mut block: Block,
        lottery_candidates: &[Utxo],
        utxo_lookup: F,
        lottery_config: &LotteryFeeConfig,
    ) -> Block
    where
        F: Fn(&[u8; 36]) -> Option<Utxo>,
    {
        // Calculate total fees from transactions
        let total_fees: u64 = block.transactions.iter().map(|tx| tx.fee).sum();

        if total_fees == 0 {
            debug!("No fees to distribute, skipping lottery");
            return block;
        }

        // Convert UTXOs to lottery candidates
        let candidates: Vec<LotteryCandidate> = lottery_candidates
            .iter()
            .map(|utxo| {
                // Convert ClusterTagVector to TagVector for lottery entropy calculation
                let tag_vector = Self::cluster_tags_to_tag_vector(&utxo.output.cluster_tags);

                // Default cluster factor of 1.0 (1000 on the 1000-6000 scale)
                // In a full implementation, this would be calculated from cluster wealth
                let cluster_factor = 1000u64;

                utxo_to_candidate(
                    utxo.id.to_bytes(),
                    utxo.output.amount,
                    cluster_factor,
                    &tag_vector,
                    utxo.created_at,
                )
            })
            .collect();

        if candidates.is_empty() {
            debug!("No lottery candidates available, burning all fees");
            // When there are no lottery candidates, all fees are burned
            block.lottery_summary = BlockLotterySummary {
                total_fees,
                pool_distributed: 0,
                amount_burned: total_fees,
                lottery_seed: [0u8; 32],
            };
            return block;
        }

        info!(
            candidates = candidates.len(),
            total_fees = total_fees,
            "Drawing lottery winners"
        );

        // Draw lottery winners
        let lottery_result = draw_lottery_winners(
            &candidates,
            total_fees,
            block.height(),
            &block.header.prev_block_hash,
            lottery_config,
        );

        // Convert winners to lottery outputs
        let lottery_outputs =
            Self::winners_to_outputs(&lottery_result, &utxo_lookup, lottery_config);

        // Build lottery summary
        let lottery_summary = BlockLotterySummary {
            total_fees,
            pool_distributed: lottery_result.pool_amount,
            amount_burned: lottery_result.burn_amount,
            lottery_seed: lottery_result.seed,
        };

        info!(
            winners = lottery_outputs.len(),
            pool = lottery_result.pool_amount,
            burned = lottery_result.burn_amount,
            "Lottery drawing complete"
        );

        block.lottery_outputs = lottery_outputs;
        block.lottery_summary = lottery_summary;
        block
    }

    /// Convert lottery winners to lottery outputs.
    ///
    /// For each winner, looks up the original UTXO to get the stealth keys
    /// (target_key, public_key) so the payout goes to the same owner.
    fn winners_to_outputs<F>(
        result: &BlockLotteryResult,
        utxo_lookup: &F,
        _config: &LotteryFeeConfig,
    ) -> Vec<LotteryOutput>
    where
        F: Fn(&[u8; 36]) -> Option<Utxo>,
    {
        let mut outputs = Vec::new();

        for winner in &result.winners {
            // Look up the winning UTXO to get its stealth keys
            if let Some(utxo) = utxo_lookup(&winner.utxo_id) {
                outputs.push(LotteryOutput::from_utxo_id(
                    winner.utxo_id,
                    winner.payout,
                    utxo.output.target_key,
                    utxo.output.public_key,
                ));
            } else {
                warn!(
                    utxo_id = hex::encode(&winner.utxo_id[..8]),
                    "Could not find winning UTXO for lottery output, skipping"
                );
            }
        }

        outputs
    }

    /// Convert ClusterTagVector (on-chain format) to TagVector (cluster-tax format).
    ///
    /// Both represent the same concept but in different crate contexts.
    fn cluster_tags_to_tag_vector(
        cluster_tags: &bth_transaction_types::ClusterTagVector,
    ) -> TagVector {
        let mut tag_vector = TagVector::new();
        for entry in &cluster_tags.entries {
            tag_vector.set(
                bth_cluster_tax::ClusterId::new(entry.cluster_id.0),
                entry.weight,
            );
        }
        tag_vector
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

    // ========================================================================
    // Lottery Integration Tests
    // ========================================================================

    fn mock_utxo(utxo_id: [u8; 36], amount: u64, created_at: u64) -> Utxo {
        use crate::transaction::{TxOutput, UtxoId};
        use bth_transaction_types::ClusterTagVector;

        Utxo {
            id: UtxoId::from_bytes(&utxo_id).expect("valid utxo id"),
            output: TxOutput {
                amount,
                target_key: [10u8; 32],
                public_key: [20u8; 32],
                cluster_tags: ClusterTagVector::single(bth_transaction_types::ClusterId(1)),
                e_memo: None,
            },
            created_at,
        }
    }

    fn mock_transaction_with_fee(fee: u64) -> Transaction {
        use crate::transaction::TxInputs;

        // Minimal transaction with specified fee
        Transaction {
            inputs: TxInputs::new(vec![]),
            outputs: vec![],
            fee,
            created_at_height: 0,
        }
    }

    #[test]
    fn test_apply_lottery_no_fees() {
        let minting_tx = mock_minting_tx(100);
        let block = BlockBuilder::build_direct(minting_tx, vec![]);

        // Block with no transactions (0 fees)
        let candidates = vec![mock_utxo([1u8; 36], 1_000_000, 50)];
        let utxo_lookup = |id: &[u8; 36]| {
            if id == &[1u8; 36] {
                Some(mock_utxo([1u8; 36], 1_000_000, 50))
            } else {
                None
            }
        };

        let lottery_config = LotteryFeeConfig::default();
        let result = BlockBuilder::apply_lottery(block.clone(), &candidates, utxo_lookup, &lottery_config);

        // No fees means no lottery
        assert!(result.lottery_outputs.is_empty());
        assert_eq!(result.lottery_summary.total_fees, 0);
    }

    #[test]
    fn test_apply_lottery_no_candidates() {
        let minting_tx = mock_minting_tx(100);
        let tx = mock_transaction_with_fee(1_000_000);
        let block = BlockBuilder::build_direct(minting_tx, vec![tx]);

        // Empty candidates
        let candidates: Vec<Utxo> = vec![];
        let utxo_lookup = |_: &[u8; 36]| None;

        let lottery_config = LotteryFeeConfig::default();
        let result = BlockBuilder::apply_lottery(block.clone(), &candidates, utxo_lookup, &lottery_config);

        // No candidates means all fees are burned
        assert!(result.lottery_outputs.is_empty());
        assert_eq!(result.lottery_summary.total_fees, 1_000_000);
        assert_eq!(result.lottery_summary.pool_distributed, 0);
        assert_eq!(result.lottery_summary.amount_burned, 1_000_000);
    }

    #[test]
    fn test_apply_lottery_with_fees_and_candidates() {
        let minting_tx = mock_minting_tx(100);
        let tx = mock_transaction_with_fee(10_000_000); // 10M picocredits fee
        let block = BlockBuilder::build_direct(minting_tx, vec![tx]);

        // Create some candidate UTXOs
        let candidates = vec![
            mock_utxo([1u8; 36], 100_000_000, 50),  // 100M value, 50 blocks old
            mock_utxo([2u8; 36], 200_000_000, 40),  // 200M value, 60 blocks old
            mock_utxo([3u8; 36], 50_000_000, 30),   // 50M value, 70 blocks old
        ];

        let utxo_lookup = |id: &[u8; 36]| {
            match id[0] {
                1 => Some(mock_utxo([1u8; 36], 100_000_000, 50)),
                2 => Some(mock_utxo([2u8; 36], 200_000_000, 40)),
                3 => Some(mock_utxo([3u8; 36], 50_000_000, 30)),
                _ => None,
            }
        };

        let lottery_config = LotteryFeeConfig::default();
        let result = BlockBuilder::apply_lottery(block.clone(), &candidates, utxo_lookup, &lottery_config);

        // Should have applied lottery - total_fees should be recorded
        assert_eq!(result.lottery_summary.total_fees, 10_000_000);

        // Pool distributed is 80% of fees (default config)
        assert_eq!(result.lottery_summary.pool_distributed, 8_000_000);

        // Note: lottery_seed is only non-zero when winners are drawn
        // When no winners, seed is [0u8; 32]

        // The burn amount depends on whether winners were drawn:
        // - If winners: burn = 20% of fees (2M)
        // - If no winners: burn = 100% of fees (10M, pool is also burned)
        // Both cases are valid lottery outcomes
        assert!(
            result.lottery_summary.amount_burned == 2_000_000
                || result.lottery_summary.amount_burned == 10_000_000,
            "Expected burn of 2M (winners) or 10M (no winners), got {}",
            result.lottery_summary.amount_burned
        );

        // Lottery outputs should exist only if there were winners
        if result.lottery_summary.amount_burned == 2_000_000 {
            // Winners were drawn - should have lottery outputs
            assert!(!result.lottery_outputs.is_empty());
        } else {
            // No winners - no lottery outputs
            assert!(result.lottery_outputs.is_empty());
        }
    }

    #[test]
    fn test_cluster_tags_to_tag_vector_conversion() {
        use bth_transaction_types::{ClusterId as TypesClusterId, ClusterTagEntry, ClusterTagVector};
        use bth_cluster_tax::ClusterId as TaxClusterId;

        // Create a ClusterTagVector with multiple entries
        let mut tags = ClusterTagVector::empty();
        tags.entries.push(ClusterTagEntry {
            cluster_id: TypesClusterId(1),
            weight: 500_000, // 50%
        });
        tags.entries.push(ClusterTagEntry {
            cluster_id: TypesClusterId(2),
            weight: 300_000, // 30%
        });

        let tag_vector = BlockBuilder::cluster_tags_to_tag_vector(&tags);

        // Verify conversion
        assert_eq!(tag_vector.get(TaxClusterId::new(1)), 500_000);
        assert_eq!(tag_vector.get(TaxClusterId::new(2)), 300_000);
        assert_eq!(tag_vector.get(TaxClusterId::new(3)), 0); // Not present
    }
}
