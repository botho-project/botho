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
#[cfg(test)]
use bth_cluster_tax::TagVector;
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

    /// Apply the V2 lottery to a built block (protocol 7.0.0, #1286 P3,
    /// #1640).
    ///
    /// The block's header, minting tx and ordinary transactions must be final.
    /// This delegates to the ledger's V2 producer [`Ledger::seal_block`],
    /// which draws over one pinned pre-state, derives every payout record
    /// through the shared `ledger::lottery_v2::derive` rules (a fresh,
    /// independently keyed target per award, recoverable by the winner from
    /// the committed derivation context), writes the payouts and the lottery
    /// summary, and sets `header.tx_root` to the V2 body root.
    ///
    /// The former V1 producer (`winners_to_outputs`) built each payout by
    /// COPYING the winning UTXO's one-time target, DH key and ML-KEM
    /// ciphertext, so source and payout shared one key image (#1286). It is
    /// retired with protocol 7.0.0 (see git history before #1640); V1-shaped
    /// payouts are invalid under the V2 validator.
    ///
    /// Errors are returned to the caller; there is deliberately no lottery-free
    /// fallback, because any other body would fail validation on every node.
    ///
    /// [`Ledger::seal_block`]: crate::ledger::Ledger::seal_block
    pub fn apply_lottery(
        block: Block,
        ledger: &crate::ledger::Ledger,
    ) -> Result<Block, crate::ledger::LedgerError> {
        ledger.seal_block(block)
    }

    /// Convert ClusterTagVector (on-chain format) to TagVector (cluster-tax
    /// format).
    ///
    /// Both represent the same concept but in different crate contexts.
    /// Only its unit test uses it since the V1 producer was retired (#1640).
    #[cfg(test)]
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
            kem_ciphertext: None,
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
            settlement: None,
        };
        // Stale tx: created_at_height + MAX_TX_AGE < block_height (filtered).
        let stale = Transaction {
            inputs: TxInputs::new(vec![]),
            outputs: vec![],
            fee: 0,
            created_at_height: block_height - MAX_TX_AGE - 1,
            settlement: None,
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
            settlement: None,
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

    // The V1 `apply_lottery` unit tests (mock candidates + a UTXO lookup)
    // and the #998 proposer/validator-gate reproduction exercised the retired
    // V1 envelope-copy producer. Their V2 equivalents run against a real
    // ledger, where proposer and validator share one pinned pre-state:
    // `ledger::store::v2_activation_tests` (winner path at the real 720-block
    // maturity, forged payouts, determinism) and the `lottery_v2_activation`
    // TestNetwork regression.

    #[test]
    fn apply_lottery_seals_a_v2_body_root_that_the_ledger_accepts() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = crate::ledger::Ledger::open(dir.path()).unwrap();
        let tip = ledger.get_tip().unwrap();
        let mut minting_tx = mock_minting_tx(1);
        minting_tx.prev_block_hash = tip.hash();
        let block = BlockBuilder::build_direct(minting_tx, vec![]);
        let legacy_root = block.header.tx_root;
        let sealed = BlockBuilder::apply_lottery(block.clone(), &ledger).unwrap();
        // No payouts below maturity, but the header now commits the V2 body.
        assert!(sealed.lottery_outputs.is_empty());
        assert_ne!(sealed.header.tx_root, legacy_root);
        assert_ne!(sealed.hash(), block.hash());
        // Sealing is a pure function of the pre-state and the block.
        assert_eq!(
            bincode::serialize(&sealed).unwrap(),
            bincode::serialize(&BlockBuilder::apply_lottery(block, &ledger).unwrap()).unwrap()
        );
    }

    #[test]
    fn apply_lottery_refuses_a_block_that_does_not_extend_the_tip() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = crate::ledger::Ledger::open(dir.path()).unwrap();
        // mock_minting_tx's parent is all-zero, not the genesis hash.
        let block = BlockBuilder::build_direct(mock_minting_tx(1), vec![]);
        let error = BlockBuilder::apply_lottery(block, &ledger).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Previous block hash mismatch (V2 producer pre-state"),
            "{error}"
        );
        let mut minting_tx = mock_minting_tx(2);
        minting_tx.prev_block_hash = ledger.get_tip().unwrap().hash();
        let skipped = BlockBuilder::build_direct(minting_tx, vec![]);
        let error = BlockBuilder::apply_lottery(skipped, &ledger).unwrap_err();
        assert!(
            error.to_string().contains("Expected height 1, got 2"),
            "{error}"
        );
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
