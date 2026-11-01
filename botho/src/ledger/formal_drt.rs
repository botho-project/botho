//! TEST/DRT ONLY: the canonical LotteryV2 [`drawing`](super::lottery_v2) over
//! an in-memory [`DerivationReads`] double, for the Lean differential tests
//! in `formal-drt/` (#1706, #1707).
//!
//! Compiled only with the non-default `test-fixtures` feature (like the
//! ledger fixture API), so no node build contains it. It changes no
//! consensus code: [`drawing`] calls the production `lottery_v2::drawing`
//! unchanged; only the pinned pre-state it reads is in memory.
//!
//! The double's candidate read is the production window rule over an
//! in-memory key-ordered table: rotate the key order to start at the
//! seed-derived offset, keep the eligible rows, stop at
//! `MAX_LOTTERY_CANDIDATES`. That reimplements the LMDB reader
//! (`store::validation::lottery_candidates`) in miniature, so the DRT checks
//! the production reader separately, against the same Lean model, over a real
//! LMDB ledger (`formal-drt` leg `lottery_candidates`).

use super::{
    lottery_v2::{self, DerivationReads, StoredContext},
    LedgerError,
};
use crate::{
    block::{Block, BlockHeader},
    consensus::lottery::{BlockLotteryResult, LotteryPoolAccounting},
    transaction::{Utxo, UtxoId},
};
use bth_cluster_tax::{LotteryCandidate, LotteryDrawConfig};
use sha2::{Digest, Sha256};
use std::{cell::RefCell, collections::BTreeMap};

/// `MAX_LOTTERY_CANDIDATES` of the production reader
/// (`store::validation::lottery_candidates`).
pub const MAX_LOTTERY_CANDIDATES: usize = 10_000;

/// One row of the in-memory UTXO table: what the candidate read uses.
#[derive(Clone, Debug)]
pub struct Row {
    /// UTXO amount (`utxo.output.amount`).
    pub amount: u64,
    /// Creation height (`utxo.created_at`).
    pub created_at: u64,
    /// Cluster factor the production reader would derive from the row's
    /// tags (an input here; the fee curve is not part of the lottery model).
    pub cluster_factor: u64,
}

/// The pinned pre-state `drawing` reads.
#[derive(Clone, Debug)]
pub struct Reads {
    /// `DerivationReads::lottery_pool`.
    pub lottery_pool: u128,
    /// `DerivationReads::tip_header`: the parent of the drawn block.
    pub tip: BlockHeader,
    /// The UTXO table, keyed by the 36-byte UTXO id (LMDB key order).
    pub rows: BTreeMap<[u8; 36], Row>,
}

/// The double, plus the window seeds `drawing` asked for.
struct Double<'a> {
    reads: &'a Reads,
    windows: RefCell<Vec<[u8; 32]>>,
}

/// `Ledger::lottery_candidate_offset_key` (private to the store). Copied, not
/// shared: the `lottery_candidates` DRT leg checks the production copy.
fn offset_key(seed: &[u8; 32], block_height: u64) -> [u8; 36] {
    let mut head = Sha256::new();
    head.update(b"LOTTERY_CANDIDATE_OFFSET_V1");
    head.update(seed);
    head.update(block_height.to_le_bytes());
    let head: [u8; 32] = head.finalize().into();
    let mut tail = Sha256::new();
    tail.update(b"LOTTERY_CANDIDATE_OFFSET_V1_TAIL");
    tail.update(head);
    let tail: [u8; 32] = tail.finalize().into();
    let mut key = [0u8; 36];
    key[..32].copy_from_slice(&head);
    key[32..].copy_from_slice(&tail[..4]);
    key
}

impl DerivationReads for Double<'_> {
    fn genesis_hash(&self) -> Result<[u8; 32], LedgerError> {
        Ok([0; 32])
    }
    fn lottery_pool(&self) -> Result<u128, LedgerError> {
        Ok(self.reads.lottery_pool)
    }
    fn output_context(&self, _: &UtxoId) -> Result<(Utxo, StoredContext), LedgerError> {
        Err(LedgerError::InconsistentRecord(
            "formal_drt double has no output contexts".into(),
        ))
    }
    fn tip_header(&self) -> Result<BlockHeader, LedgerError> {
        Ok(self.reads.tip.clone())
    }
    fn lottery_candidates(
        &self,
        block: &Block,
        seed: &[u8; 32],
        config: &LotteryDrawConfig,
    ) -> Result<Vec<LotteryCandidate>, LedgerError> {
        self.windows.borrow_mut().push(*seed);
        let height = block.height();
        let offset = offset_key(seed, height);
        let upper = self.reads.rows.range(offset..);
        let lower = self.reads.rows.range(..offset);
        let mut candidates = Vec::new();
        for (id, row) in upper.chain(lower) {
            if candidates.len() >= MAX_LOTTERY_CANDIDATES {
                break;
            }
            let age = height.saturating_sub(row.created_at);
            if age >= config.min_utxo_age
                && age <= config.circulation_window
                && row.amount >= config.min_utxo_value
            {
                candidates.push(LotteryCandidate::new(
                    *id,
                    row.amount,
                    row.cluster_factor,
                    &bth_cluster_tax::TagVector::new(),
                    row.created_at,
                ));
            }
        }
        Ok(candidates)
    }
}

/// The production `lottery_v2::drawing` of `block` over `reads`, and the
/// window seeds it requested (one per call: the draw seed).
pub fn drawing(
    reads: &Reads,
    block: &Block,
) -> (
    Result<(BlockLotteryResult, LotteryPoolAccounting), LedgerError>,
    Vec<[u8; 32]>,
) {
    let double = Double {
        reads,
        windows: RefCell::new(Vec::new()),
    };
    let result = lottery_v2::drawing(&double, block);
    (result, double.windows.into_inner())
}
