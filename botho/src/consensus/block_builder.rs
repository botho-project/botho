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
        lottery::{draw_lottery_winners, BlockLotteryResult, LotteryFeeConfig},
        ConsensusValue,
    },
    transaction::{Transaction, Utxo},
};
use bth_cluster_tax::{LotteryCandidate, TagVector};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use tracing::{debug, info, warn};

/// Maximum age (in blocks) of a transfer transaction relative to the height of
/// the block that includes it.
///
/// A transfer tx whose `created_at_height + MAX_TX_AGE < block_height` is
/// considered stale and is excluded at block-build (and rejected at block-apply
/// as a deterministic backstop).
///
/// This staleness rule used to live in the SCP transfer-validity function
/// (`validate_transfer_tx`) and was evaluated against each node's *local
/// current tip*. Because two honest nodes can be at different tips while
/// nominating the same value, that made transfer-tx validity tip-dependent — a
/// value valid for one honest node could be silently dropped as
/// `StaleTransaction` by another, the #417-class asymmetric-validity fork
/// condition (issue #451).
///
/// The fix keeps block-height as the staleness metric (NOT wall-clock — no
/// timezone/leap-second/clock-skew dependence) but evaluates it against the
/// height of the block being built/applied. Every honest node assembles and
/// applies block N at the same height N, so the check is deterministic and
/// cannot fork. Defining it once here (used by both build and apply) guarantees
/// the two paths agree exactly.
pub const MAX_TX_AGE: u64 = 100;

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

        // The block's own height. Staleness is evaluated against THIS height
        // (deterministic across all honest nodes), never against any node's
        // local current tip — that tip-dependence was the #417-class fork risk
        // removed from the SCP validity gate in issue #451.
        let block_height = minting_tx.block_height;

        // Collect transfer transactions with key image deduplication
        // This is a defense-in-depth measure to prevent double-spend attacks where
        // two transactions with the same key image somehow both made it to consensus
        let mut transactions = Vec::new();
        let mut transfer_tx_hashes = Vec::new();
        let mut seen_key_images: HashSet<[u8; 32]> = HashSet::new();

        for value in &transfer_values {
            match get_transfer_tx(&value.tx_hash) {
                Some(tx) => {
                    // Height-based staleness filter (issue #451). Exclude any
                    // transfer tx that is too old relative to THIS block's
                    // height. Filtering here (rather than rejecting after
                    // externalize) guarantees the built block always applies
                    // cleanly: a block we build never contains a stale tx, so
                    // the apply-time backstop never fires for our own blocks
                    // (no externalize-then-reject halt — the #449/#421 failure
                    // mode).
                    //
                    // `created_at_height` is attacker-set and is NOT bounded
                    // anywhere (mempool admission, the SCP intrinsic validity
                    // gate, and gossip all leave it unconstrained), so the age
                    // bound must saturate. With `overflow-checks = true` on the
                    // release profile (#663) an unchecked `+` here would let a
                    // crafted `created_at_height` near `u64::MAX` panic — and
                    // because `build_from_externalized` runs deterministically
                    // on EVERY node after externalization (run.rs), that is a
                    // network-wide halt, not just a proposer-local one.
                    // Saturation preserves the rule's semantics and matches the
                    // apply-side store.rs `first_stale_transfer_tx` exactly: a
                    // saturated sum is `u64::MAX`, which is never `<
                    // block_height`, so a tx claiming a far-future
                    // `created_at_height` is treated as "not stale" here (it is
                    // instead rejected by the later validity gates — e.g. C3
                    // ring resolution — rather than by this staleness filter).
                    // Both paths run the same binary on the same block height,
                    // so the build and apply verdicts cannot diverge across
                    // nodes.
                    if tx.created_at_height.saturating_add(MAX_TX_AGE) < block_height {
                        warn!(
                            tx_hash = hex::encode(&value.tx_hash[0..8]),
                            created_at_height = tx.created_at_height,
                            block_height,
                            "Skipping stale transfer tx in block (created_at_height + \
                             MAX_TX_AGE < block_height)"
                        );
                        continue;
                    }

                    // Check for duplicate key images (defense in depth)
                    let mut has_duplicate = false;
                    for input in tx.inputs.clsag() {
                        if seen_key_images.contains(&input.key_image) {
                            warn!(
                                tx_hash = hex::encode(&value.tx_hash[0..8]),
                                key_image = hex::encode(&input.key_image[0..8]),
                                "Skipping transaction with duplicate key image in block"
                            );
                            has_duplicate = true;
                            break;
                        }
                    }

                    if has_duplicate {
                        continue;
                    }

                    // Track all key images from this transaction
                    for input in tx.inputs.clsag() {
                        seen_key_images.insert(input.key_image);
                    }

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
    /// * `candidates` - Lottery candidates with real cluster factors, from
    ///   `Ledger::get_lottery_validation_candidates` — the SAME function block
    ///   validation uses. Verification re-runs the draw, so proposer and
    ///   validator candidate sets (values, factors, order) must match.
    /// * `stored_pool` - Carryover lottery pool balance from the ledger
    /// * `utxo_lookup` - Function to look up UTXO details by ID for key
    ///   recovery
    /// * `lottery_config` - Configuration for fee splitting and lottery drawing
    ///
    /// # Returns
    /// Updated block with lottery outputs and summary
    pub fn apply_lottery<F>(
        mut block: Block,
        candidates: &[LotteryCandidate],
        stored_pool: u128,
        utxo_lookup: F,
        lottery_config: &LotteryFeeConfig,
    ) -> Block
    where
        F: Fn(&[u8; 36]) -> Option<Utxo>,
    {
        // Calculate total fees from transactions. Saturating
        // (block.rs::total_fees) — a plain sum() panics under release
        // overflow-checks on this every-node externalize path before
        // add_block's checked_block_fees can reject the block gracefully.
        let total_fees: u64 = block.total_fees();

        // Pool accounting: carryover + emission share + fee pool share,
        // payouts capped at one block reward (anti-grinding bound). Must
        // match validation exactly.
        let emission_share = block.minting_tx.lottery_emission_share();
        let accounting = crate::consensus::lottery::compute_pool_accounting(
            total_fees,
            emission_share,
            stored_pool,
            block.minting_tx.reward,
            lottery_config,
        );

        if accounting.payout == 0 && accounting.fee_burn == 0 {
            debug!("Nothing to distribute or burn, skipping lottery");
            return block;
        }

        if candidates.is_empty() {
            debug!("No lottery candidates available; pool carries over");
            // Fee burn share is burned; the pool share carries over.
            block.lottery_summary = BlockLotterySummary {
                total_fees,
                pool_distributed: 0,
                amount_burned: accounting.fee_burn,
                lottery_seed: [0u8; 32],
            };
            return block;
        }

        info!(
            candidates = candidates.len(),
            total_fees = total_fees,
            payout = accounting.payout,
            emission_share = emission_share,
            "Drawing lottery winners"
        );

        // Draw lottery winners
        let lottery_result = draw_lottery_winners(
            &candidates,
            total_fees,
            &accounting,
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

    /// Convert ClusterTagVector (on-chain format) to TagVector (cluster-tax
    /// format).
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
        let values = vec![ConsensusValue::from_transaction([1u8; 32], 0)];

        let result = BlockBuilder::build_from_externalized(&values, |_| None, |_| None);

        assert!(matches!(result, Err(BlockBuildError::NoMintingTx)));
    }

    /// Issue #451 (Test B, build side): a block built at height H must EXCLUDE
    /// any transfer tx whose `created_at_height + MAX_TX_AGE < H`. Filtering at
    /// build time (against the block's own height, deterministic across nodes)
    /// keeps staleness enforced without the tip-dependence that caused the
    /// #417-class fork, and guarantees the built block always applies cleanly
    /// (no externalize-then-reject halt).
    #[test]
    fn test_build_excludes_stale_transfer_tx() {
        use crate::transaction::TxInputs;

        let block_height = 200u64;

        // Fresh tx: created_at_height + MAX_TX_AGE >= block_height (kept).
        let fresh = Transaction {
            inputs: TxInputs::new(vec![]),
            outputs: vec![],
            fee: 0,
            created_at_height: block_height - MAX_TX_AGE, // exactly on the boundary, kept
        };
        // Stale tx: created_at_height + MAX_TX_AGE < block_height (filtered).
        let stale = Transaction {
            inputs: TxInputs::new(vec![]),
            outputs: vec![],
            fee: 0,
            created_at_height: block_height - MAX_TX_AGE - 1,
        };
        let fresh_hash = fresh.hash();
        let stale_hash = stale.hash();

        let values = vec![
            ConsensusValue::from_minting_tx([9u8; 32], 0),
            ConsensusValue::from_transaction(fresh_hash, 0),
            ConsensusValue::from_transaction(stale_hash, 0),
        ];

        let minting_tx = mock_minting_tx(block_height);
        let get_minting = |_: &[u8; 32]| Some(minting_tx.clone());
        let get_transfer = move |h: &[u8; 32]| {
            if *h == fresh_hash {
                Some(fresh.clone())
            } else if *h == stale_hash {
                Some(stale.clone())
            } else {
                None
            }
        };

        let built = BlockBuilder::build_from_externalized(&values, get_minting, get_transfer)
            .expect("build should succeed");

        assert_eq!(
            built.block.transactions.len(),
            1,
            "stale transfer tx must be filtered out at build"
        );
        assert_eq!(
            built.block.transactions[0].created_at_height,
            block_height - MAX_TX_AGE,
            "only the fresh tx (on the boundary) should remain"
        );
        assert!(
            !built.transfer_tx_hashes.contains(&stale_hash),
            "stale tx hash must not be recorded in the built block"
        );
    }

    /// Issue #663 (overflow-checks hardening): `created_at_height` is
    /// attacker-set and unbounded, so the staleness age bound
    /// (`created_at_height + MAX_TX_AGE`) must not panic under
    /// `overflow-checks = true` (the release profile). A tx with
    /// `created_at_height == u64::MAX` must build cleanly — never panic with
    /// "attempt to add with overflow" — because `build_from_externalized` runs
    /// deterministically on every validating node after externalization, so a
    /// panic here is a network-wide halt vector (not just a proposer-local
    /// crash).
    ///
    /// Semantics: with `saturating_add`, `u64::MAX + MAX_TX_AGE` saturates to
    /// `u64::MAX`, which is never `< block_height`, so the far-future tx is
    /// classified "not stale" here and simply retained by this filter (it is
    /// rejected downstream by the ordinary validity gates, e.g. C3 ring
    /// resolution, on real crafted inputs). This deterministically matches the
    /// apply-side `first_stale_transfer_tx` in store.rs, so build and apply
    /// verdicts cannot diverge across nodes.
    #[test]
    fn test_build_from_externalized_no_overflow_on_max_created_at_height() {
        use crate::transaction::TxInputs;

        let block_height = 200u64;

        // Attacker-crafted tx: created_at_height == u64::MAX. Under the old
        // unchecked `+ MAX_TX_AGE` this panicked "attempt to add with overflow"
        // with overflow-checks enabled.
        let crafted = Transaction {
            inputs: TxInputs::new(vec![]),
            outputs: vec![],
            fee: 0,
            created_at_height: u64::MAX,
        };
        let crafted_hash = crafted.hash();

        let values = vec![
            ConsensusValue::from_minting_tx([9u8; 32], 0),
            ConsensusValue::from_transaction(crafted_hash, 0),
        ];

        let minting_tx = mock_minting_tx(block_height);
        let get_minting = |_: &[u8; 32]| Some(minting_tx.clone());
        let get_transfer = move |h: &[u8; 32]| {
            if *h == crafted_hash {
                Some(crafted.clone())
            } else {
                None
            }
        };

        // Must not panic under overflow-checks. Build twice to pin determinism.
        let built_a =
            BlockBuilder::build_from_externalized(&values, get_minting, get_transfer.clone())
                .expect("build must not panic on u64::MAX created_at_height");
        let built_b = BlockBuilder::build_from_externalized(&values, get_minting, get_transfer)
            .expect("second build must not panic either");

        // Deterministic inclusion: saturated sum (u64::MAX) is never
        // < block_height, so the far-future tx is "not stale" and retained here.
        assert_eq!(
            built_a.block.transactions.len(),
            1,
            "u64::MAX-created tx is not stale (saturated sum >= block_height) and is retained"
        );
        assert_eq!(
            built_a.transfer_tx_hashes, built_b.transfer_tx_hashes,
            "inclusion/exclusion of the crafted tx must be deterministic across builds"
        );
        assert!(
            built_a.transfer_tx_hashes.contains(&crafted_hash),
            "crafted tx must be deterministically recorded in the built block"
        );
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
                kem_ciphertext: None,
            },
            created_at,
        }
    }

    fn mock_candidate(
        utxo_id: [u8; 36],
        amount: u64,
        created_at: u64,
    ) -> bth_cluster_tax::LotteryCandidate {
        let utxo = mock_utxo(utxo_id, amount, created_at);
        let tag_vector = BlockBuilder::cluster_tags_to_tag_vector(&utxo.output.cluster_tags);
        crate::consensus::lottery::utxo_to_candidate(
            utxo_id,
            amount,
            1000, // factor 1.0
            &tag_vector,
            created_at,
        )
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
        let candidates = vec![mock_candidate([1u8; 36], 1_000_000, 50)];
        let utxo_lookup = |id: &[u8; 36]| {
            if id == &[1u8; 36] {
                Some(mock_utxo([1u8; 36], 1_000_000, 50))
            } else {
                None
            }
        };

        let lottery_config = LotteryFeeConfig::default();
        let result = BlockBuilder::apply_lottery(
            block.clone(),
            &candidates,
            0,
            utxo_lookup,
            &lottery_config,
        );

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
        let candidates: Vec<bth_cluster_tax::LotteryCandidate> = vec![];
        let utxo_lookup = |_: &[u8; 36]| None;

        let lottery_config = LotteryFeeConfig::default();
        let result = BlockBuilder::apply_lottery(
            block.clone(),
            &candidates,
            0,
            utxo_lookup,
            &lottery_config,
        );

        // No candidates: the fee burn share (20%) is burned; the pool share
        // carries over via the persistent lottery pool
        assert!(result.lottery_outputs.is_empty());
        assert_eq!(result.lottery_summary.total_fees, 1_000_000);
        assert_eq!(result.lottery_summary.pool_distributed, 0);
        assert_eq!(result.lottery_summary.amount_burned, 200_000);
    }

    #[test]
    fn test_apply_lottery_with_fees_and_candidates() {
        let minting_tx = mock_minting_tx(100);
        let tx = mock_transaction_with_fee(10_000_000); // 10M picocredits fee
        let block = BlockBuilder::build_direct(minting_tx, vec![tx]);

        // Create some candidate UTXOs
        let candidates = vec![
            mock_candidate([1u8; 36], 100_000_000, 50), // 100M value, 50 blocks old
            mock_candidate([2u8; 36], 200_000_000, 40), // 200M value, 60 blocks old
            mock_candidate([3u8; 36], 50_000_000, 30),  // 50M value, 70 blocks old
        ];

        let utxo_lookup = |id: &[u8; 36]| match id[0] {
            1 => Some(mock_utxo([1u8; 36], 100_000_000, 50)),
            2 => Some(mock_utxo([2u8; 36], 200_000_000, 40)),
            3 => Some(mock_utxo([3u8; 36], 50_000_000, 30)),
            _ => None,
        };

        let lottery_config = LotteryFeeConfig::default();
        let result = BlockBuilder::apply_lottery(
            block.clone(),
            &candidates,
            0,
            utxo_lookup,
            &lottery_config,
        );

        // Should have applied lottery - total_fees should be recorded
        assert_eq!(result.lottery_summary.total_fees, 10_000_000);

        // The fee burn share (20%) is always burned; the pool share either
        // pays out (capped at one block reward) or carries over
        assert_eq!(result.lottery_summary.amount_burned, 2_000_000);

        if result.lottery_outputs.is_empty() {
            // No winners drawn (candidates too young for default min age):
            // nothing distributed, pool share carries over
            assert_eq!(result.lottery_summary.pool_distributed, 0);
        } else {
            // Winners drawn: payout = min(fee pool share, block reward cap)
            let cap = result.minting_tx.reward;
            assert_eq!(result.lottery_summary.pool_distributed, 8_000_000.min(cap));
        }
    }

    #[test]
    fn test_cluster_tags_to_tag_vector_conversion() {
        use bth_cluster_tax::ClusterId as TaxClusterId;
        use bth_transaction_types::{
            ClusterId as TypesClusterId, ClusterTagEntry, ClusterTagVector,
        };

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
