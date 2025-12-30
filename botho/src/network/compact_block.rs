// Copyright (c) 2024 Botho Foundation

//! Compact block relay for bandwidth-efficient block propagation.
//!
//! This module implements BIP 152-style compact blocks, reducing block propagation
//! bandwidth by 99%+ by sending short transaction IDs instead of full transactions.
//! Receiving nodes reconstruct blocks from their mempool.
//!
//! # Protocol Flow
//!
//! 1. Miner creates block → broadcasts `CompactBlock` (header + 6-byte short IDs)
//! 2. Receiver attempts reconstruction from mempool using short ID mapping
//! 3. If transactions are missing → sends `GetBlockTxn` request
//! 4. Original node responds with `BlockTxn` containing missing transactions
//! 5. Receiver completes reconstruction and validates block
//!
//! # Size Comparison
//!
//! | Block Type | Full Size | Compact Size | Savings |
//! |------------|-----------|--------------|---------|
//! | 1000 simple txs | ~500 KB | ~6.5 KB | 99% |
//! | 1000 PQ ring txs | ~26 MB | ~6.5 KB | 99.97% |

use serde::{Deserialize, Serialize};
use siphasher::sip::SipHasher24;
use std::collections::HashMap;
use std::hash::Hasher;

use crate::block::{Block, BlockHeader, MintingTx};
use crate::mempool::Mempool;
use crate::transaction::Transaction;

/// A 6-byte short transaction ID derived via SipHash.
///
/// The probability of collision with 1000 transactions is approximately 10^-9.
pub type ShortId = [u8; 6];

/// A compact block containing transaction short IDs instead of full transactions.
///
/// Receivers reconstruct the full block by mapping short IDs to transactions
/// in their mempool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactBlock {
    /// Block header (contains merkle root for validation)
    pub header: BlockHeader,
    /// Minting transaction (always included - not in mempool)
    pub minting_tx: MintingTx,
    /// Nonce derived from block hash for SipHash computation
    pub nonce: u64,
    /// Short IDs for each transaction in block order
    pub short_ids: Vec<ShortId>,
    /// Pre-filled transactions (for txs unlikely to be in mempool)
    pub prefilled_txs: Vec<PrefilledTx>,
}

/// A pre-filled transaction included directly in the compact block.
///
/// Used for transactions that are unlikely to be in the receiver's mempool,
/// such as the miner's own transactions or very recent broadcasts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrefilledTx {
    /// Index in the transaction list
    pub index: u16,
    /// Full transaction data
    pub tx: Transaction,
}

/// Request for missing transactions during compact block reconstruction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetBlockTxn {
    /// Hash of the block being reconstructed
    pub block_hash: [u8; 32],
    /// Indices of missing transactions
    pub indices: Vec<u16>,
}

/// Response containing requested transactions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockTxn {
    /// Hash of the block these transactions belong to
    pub block_hash: [u8; 32],
    /// Requested transactions in order of requested indices
    pub txs: Vec<Transaction>,
}

/// Result of attempting to reconstruct a block from a compact block.
#[derive(Debug)]
pub enum ReconstructionResult {
    /// Block was fully reconstructed
    Complete(Block),
    /// Some transactions are missing
    Incomplete {
        /// Indices of missing transactions
        missing_indices: Vec<u16>,
    },
}

/// Compute a 6-byte short ID for a transaction.
///
/// Uses SipHash-2-4 with keys derived from the transaction hash and block nonce.
/// This provides collision resistance while keeping IDs small.
pub fn compute_short_id(tx_hash: &[u8; 32], nonce: u64) -> ShortId {
    // Use first 8 bytes of tx_hash as key0, nonce as key1
    let k0 = u64::from_le_bytes(tx_hash[0..8].try_into().unwrap());
    let mut hasher = SipHasher24::new_with_keys(k0, nonce);
    hasher.write(tx_hash);
    let hash = hasher.finish();

    // Take first 6 bytes of the hash
    let mut short_id = [0u8; 6];
    short_id.copy_from_slice(&hash.to_le_bytes()[0..6]);
    short_id
}

/// Derive the nonce from a block hash.
///
/// The nonce is used in short ID computation to prevent pre-computation attacks.
pub fn derive_nonce(block_hash: &[u8; 32]) -> u64 {
    u64::from_le_bytes(block_hash[0..8].try_into().unwrap())
}

impl CompactBlock {
    /// Create a compact block from a full block.
    pub fn from_block(block: &Block) -> Self {
        let block_hash = block.hash();
        let nonce = derive_nonce(&block_hash);

        let short_ids: Vec<ShortId> = block
            .transactions
            .iter()
            .map(|tx| compute_short_id(&tx.hash(), nonce))
            .collect();

        Self {
            header: block.header.clone(),
            minting_tx: block.minting_tx.clone(),
            nonce,
            short_ids,
            prefilled_txs: Vec::new(),
        }
    }

    /// Create a compact block with pre-filled transactions.
    ///
    /// Use this when you know certain transactions won't be in receivers' mempools.
    pub fn from_block_with_prefilled(block: &Block, prefill_indices: &[usize]) -> Self {
        let block_hash = block.hash();
        let nonce = derive_nonce(&block_hash);

        let short_ids: Vec<ShortId> = block
            .transactions
            .iter()
            .map(|tx| compute_short_id(&tx.hash(), nonce))
            .collect();

        let prefilled_txs: Vec<PrefilledTx> = prefill_indices
            .iter()
            .filter_map(|&idx| {
                block.transactions.get(idx).map(|tx| PrefilledTx {
                    index: idx as u16,
                    tx: tx.clone(),
                })
            })
            .collect();

        Self {
            header: block.header.clone(),
            minting_tx: block.minting_tx.clone(),
            nonce,
            short_ids,
            prefilled_txs,
        }
    }

    /// Get the block hash (header hash).
    pub fn hash(&self) -> [u8; 32] {
        self.header.hash()
    }

    /// Get the block height.
    pub fn height(&self) -> u64 {
        self.header.height
    }

    /// Attempt to reconstruct the full block from mempool transactions.
    ///
    /// Returns `Complete` with the full block if all transactions are found,
    /// or `Incomplete` with the indices of missing transactions.
    pub fn reconstruct(&self, mempool: &Mempool) -> ReconstructionResult {
        // Build short_id → transaction map from mempool
        let mut id_map: HashMap<ShortId, Transaction> = HashMap::new();

        for (hash, tx) in mempool.iter_with_hashes() {
            let short_id = compute_short_id(&hash, self.nonce);
            id_map.insert(short_id, tx.clone());
        }

        // Add prefilled transactions to the map
        for prefilled in &self.prefilled_txs {
            let hash = prefilled.tx.hash();
            let short_id = compute_short_id(&hash, self.nonce);
            id_map.insert(short_id, prefilled.tx.clone());
        }

        // Reconstruct transaction list in order
        let mut transactions = Vec::with_capacity(self.short_ids.len());
        let mut missing_indices = Vec::new();

        for (idx, short_id) in self.short_ids.iter().enumerate() {
            if let Some(tx) = id_map.get(short_id) {
                transactions.push(tx.clone());
            } else {
                missing_indices.push(idx as u16);
            }
        }

        if missing_indices.is_empty() {
            ReconstructionResult::Complete(Block {
                header: self.header.clone(),
                minting_tx: self.minting_tx.clone(),
                transactions,
            })
        } else {
            ReconstructionResult::Incomplete { missing_indices }
        }
    }

    /// Add transactions received from a `BlockTxn` response and retry reconstruction.
    ///
    /// The `received_txs` should be in the same order as the `requested_indices`.
    pub fn add_transactions(&mut self, requested_indices: &[u16], received_txs: Vec<Transaction>) {
        for (idx, tx) in requested_indices.iter().zip(received_txs.into_iter()) {
            self.prefilled_txs.push(PrefilledTx {
                index: *idx,
                tx,
            });
        }
    }

    /// Estimate the serialized size of this compact block in bytes.
    pub fn estimated_size(&self) -> usize {
        // Header: ~200 bytes
        // MintingTx: ~300 bytes
        // Nonce: 8 bytes
        // Short IDs: 6 bytes each
        // Prefilled: variable
        let base_size = 200 + 300 + 8;
        let short_ids_size = self.short_ids.len() * 6;
        let prefilled_size: usize = self.prefilled_txs.iter().map(|p| 2 + 500).sum(); // estimate

        base_size + short_ids_size + prefilled_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_short_id_determinism() {
        let tx_hash = [0x42u8; 32];
        let nonce = 12345u64;

        let id1 = compute_short_id(&tx_hash, nonce);
        let id2 = compute_short_id(&tx_hash, nonce);

        assert_eq!(id1, id2, "Short ID should be deterministic");
    }

    #[test]
    fn test_short_id_different_nonces() {
        let tx_hash = [0x42u8; 32];

        let id1 = compute_short_id(&tx_hash, 1);
        let id2 = compute_short_id(&tx_hash, 2);

        assert_ne!(id1, id2, "Different nonces should produce different IDs");
    }

    #[test]
    fn test_short_id_different_hashes() {
        let nonce = 12345u64;

        let id1 = compute_short_id(&[0x01u8; 32], nonce);
        let id2 = compute_short_id(&[0x02u8; 32], nonce);

        assert_ne!(id1, id2, "Different hashes should produce different IDs");
    }

    #[test]
    fn test_derive_nonce() {
        let block_hash = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x00; 4].concat();
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&block_hash[..32]);

        let nonce = derive_nonce(&arr);
        assert_eq!(nonce, 0x0807060504030201, "Nonce should be little-endian");
    }

    #[test]
    fn test_short_id_uniqueness_monte_carlo() {
        // Test collision probability with 1000 random transactions
        use std::collections::HashSet;

        let nonce = 0xDEADBEEFu64;
        let mut short_ids = HashSet::new();
        let mut collisions = 0;

        for i in 0u32..1000 {
            let mut tx_hash = [0u8; 32];
            tx_hash[0..4].copy_from_slice(&i.to_le_bytes());
            tx_hash[4..8].copy_from_slice(&(i.wrapping_mul(0x12345678)).to_le_bytes());

            let short_id = compute_short_id(&tx_hash, nonce);
            if !short_ids.insert(short_id) {
                collisions += 1;
            }
        }

        // With 6 bytes (48 bits), collision probability for 1000 items is ~10^-9
        // We should never see collisions in practice
        assert_eq!(collisions, 0, "Expected no collisions with 1000 unique tx hashes");
    }
}
