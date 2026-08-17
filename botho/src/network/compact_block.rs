// Copyright (c) 2024 Botho Foundation

//! Compact block relay for bandwidth-efficient block propagation.
//!
//! This module implements BIP 152-style compact blocks, reducing block
//! propagation bandwidth by 99%+ by sending short transaction IDs instead of
//! full transactions. Receiving nodes reconstruct blocks from their mempool.
//!
//! # Protocol Flow
//!
//! 1. Miner creates block → broadcasts `CompactBlock` (header + 6-byte short
//!    IDs)
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
use std::{collections::HashMap, hash::Hasher};

use crate::{
    block::{Block, BlockHeader, BlockLotterySummary, LotteryOutput, MintingTx},
    mempool::Mempool,
    transaction::Transaction,
};

/// A 6-byte short transaction ID derived via SipHash.
///
/// The probability of collision with 1000 transactions is approximately 10^-9.
pub type ShortId = [u8; 6];

/// A compact block containing transaction short IDs instead of full
/// transactions.
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
    /// Lottery payout outputs (always included - deterministic from block
    /// state)
    #[serde(default)]
    pub lottery_outputs: Vec<LotteryOutput>,
    /// Lottery summary for validation
    #[serde(default)]
    pub lottery_summary: BlockLotterySummary,
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

/// Measure the bincode-serialized size of a value without allocating a buffer.
///
/// Returns `fallback` if the value cannot be measured (not possible for the
/// plain data structs used here, but avoids panicking in a size heuristic).
fn serialized_size_or<T: Serialize>(value: &T, fallback: usize) -> usize {
    bincode::serialized_size(value)
        .map(|size| size as usize)
        .unwrap_or(fallback)
}

/// Compute a 6-byte short ID for a transaction.
///
/// Uses SipHash-2-4 with keys derived from the transaction hash and block
/// nonce. This provides collision resistance while keeping IDs small.
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
/// The nonce is used in short ID computation to prevent pre-computation
/// attacks.
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
            lottery_outputs: block.lottery_outputs.clone(),
            lottery_summary: block.lottery_summary.clone(),
        }
    }

    /// Create a compact block with pre-filled transactions.
    ///
    /// Use this when you know certain transactions won't be in receivers'
    /// mempools.
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
            lottery_outputs: block.lottery_outputs.clone(),
            lottery_summary: block.lottery_summary.clone(),
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
                lottery_outputs: self.lottery_outputs.clone(),
                lottery_summary: self.lottery_summary.clone(),
            })
        } else {
            ReconstructionResult::Incomplete { missing_indices }
        }
    }

    /// Add transactions received from a `BlockTxn` response and retry
    /// reconstruction.
    ///
    /// The `received_txs` should be in the same order as the
    /// `requested_indices`.
    pub fn add_transactions(&mut self, requested_indices: &[u16], received_txs: Vec<Transaction>) {
        for (idx, tx) in requested_indices.iter().zip(received_txs.into_iter()) {
            self.prefilled_txs.push(PrefilledTx { index: *idx, tx });
        }
    }

    /// Estimate the serialized size of this compact block in bytes.
    ///
    /// The estimate is used for logging and relay heuristics, not as a
    /// wire-protocol contract. It is derived from the *current* bincode
    /// encoding of each component rather than from hand-tuned constants, so it
    /// stays accurate as `BlockHeader` / `MintingTx` / `LotteryOutput` gain
    /// fields.
    ///
    /// This matters: the previous flat "`MintingTx` is ~300 bytes" budget
    /// predated the ML-KEM-768 hybrid stealth envelope (issue #958), whose
    /// 1,088-byte `kem_ciphertext` makes a real coinbase ~1,300 bytes. The
    /// estimate also omitted `lottery_outputs` / `lottery_summary` entirely and
    /// the 8-byte bincode length prefix each `Vec` carries, so a 10-tx compact
    /// block was estimated at 568 bytes against an actual 1,609 (issue #1187).
    ///
    /// The `*_FALLBACK` constants below are only reached if bincode fails to
    /// measure a component (which cannot happen for these plain data structs);
    /// they are order-of-magnitude backstops, not the primary estimate.
    pub fn estimated_size(&self) -> usize {
        /// bincode encodes every `Vec` length as a fixed-width u64.
        const VEC_LEN_PREFIX: usize = 8;
        /// `nonce: u64`
        const NONCE_SIZE: usize = 8;
        /// `BlockHeader`: 9 fixed-width fields.
        const HEADER_FALLBACK: usize = 164;
        /// `MintingTx`: fixed fields plus a 1,088-byte ML-KEM ciphertext.
        const MINTING_TX_FALLBACK: usize = 1_300;
        /// `BlockLotterySummary`: 3 × u64 + a 32-byte seed.
        const LOTTERY_SUMMARY_FALLBACK: usize = 56;
        /// `LotteryOutput`: fixed fields plus an optional ML-KEM ciphertext.
        const LOTTERY_OUTPUT_FALLBACK: usize = 1_216;
        /// A prefilled `Transaction` (highly variable: simple vs. ring spend).
        const PREFILLED_TX_FALLBACK: usize = 500;

        let header_size = serialized_size_or(&self.header, HEADER_FALLBACK);
        let minting_tx_size = serialized_size_or(&self.minting_tx, MINTING_TX_FALLBACK);
        let lottery_summary_size =
            serialized_size_or(&self.lottery_summary, LOTTERY_SUMMARY_FALLBACK);

        // Vec fields: bincode charges a length prefix even when empty.
        let short_ids_size = VEC_LEN_PREFIX + self.short_ids.len() * std::mem::size_of::<ShortId>();
        let lottery_outputs_size = serialized_size_or(
            &self.lottery_outputs,
            VEC_LEN_PREFIX + self.lottery_outputs.len() * LOTTERY_OUTPUT_FALLBACK,
        );
        let prefilled_size = serialized_size_or(
            &self.prefilled_txs,
            VEC_LEN_PREFIX
                + self.prefilled_txs.len() * (std::mem::size_of::<u16>() + PREFILLED_TX_FALLBACK),
        );

        header_size
            + minting_tx_size
            + NONCE_SIZE
            + short_ids_size
            + prefilled_size
            + lottery_outputs_size
            + lottery_summary_size
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
        let mut block_hash = [0u8; 32];
        block_hash[0] = 0x01;
        block_hash[1] = 0x02;
        block_hash[2] = 0x03;
        block_hash[3] = 0x04;
        block_hash[4] = 0x05;
        block_hash[5] = 0x06;
        block_hash[6] = 0x07;
        block_hash[7] = 0x08;

        let nonce = derive_nonce(&block_hash);
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
        assert_eq!(
            collisions, 0,
            "Expected no collisions with 1000 unique tx hashes"
        );
    }
}
