//! PoW-bound lottery seed (#1676).
//!
//! Block N's draw and #573 candidate window are seeded from the RandomX PoW
//! output of N−1 (`consensus::lottery_seed_v2`), not from `hash(N−1)`, which
//! also covers the timestamp and (under V2) the freely regenerable coinbase
//! stealth output. These tests pin the property the fix exists for:
//! - two parents with identical PoW fields (nonce, prev, minter keys) but a
//!   different timestamp and coinbase output — so a different header hash —
//!   produce the identical next-block seed, candidate window and draw;
//! - a different nonce produces a different seed (and window);
//! - producer (`seal_block`) and validator (`add_block`) agree on real sealed
//!   blocks, including height 1 (parent = genesis), and the committed summary
//!   seed is the V2 seed.
use super::*;
use crate::{
    block::BlockHeader,
    consensus::lottery_seed_v2,
    ledger::lottery_v2::{self, DerivationReads, StoredContext},
    transaction::{Transaction, Utxo, UtxoId},
    wallet::Wallet,
};
use bth_cluster_tax::{LotteryCandidate, LotteryDrawConfig, TagVector};
use std::cell::RefCell;
use tempfile::TempDir;

// Public deterministic test mnemonic, used only for temporary fixtures.
const WORDS: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";

// ---------------------------------------------------------------------------
// Rule-level tests over a mock pinned view (non-empty, windowed draws without
// a 720-block real-maturity chain).
// ---------------------------------------------------------------------------

const CHILD_HEIGHT: u64 = 1_000;
/// Window cap of the mock: a strict subset of the candidates, so the
/// seed-derived offset actually decides membership.
const MOCK_WINDOW: usize = 6;

struct Mock {
    /// The pinned view's committed tip (the parent of the drawn block).
    tip: BlockHeader,
    candidates: Vec<LotteryCandidate>,
    /// Every window seed `drawing` asked for.
    windows: RefCell<Vec<[u8; 32]>>,
}

impl Mock {
    fn new(parent: &BlockHeader) -> Self {
        // 16 eligible candidates (age 730: mature, inside the circulation
        // window) with distinct, spread-out ids.
        let candidates = (0u8..16)
            .map(|i| {
                let mut id = [0u8; 36];
                id[0] = i.wrapping_mul(16);
                id[1] = i;
                LotteryCandidate::new(
                    id,
                    5_000_000_000,
                    1_000,
                    &TagVector::new(),
                    CHILD_HEIGHT - 730,
                )
            })
            .collect();
        Self {
            tip: parent.clone(),
            candidates,
            windows: RefCell::new(Vec::new()),
        }
    }
}

impl DerivationReads for Mock {
    fn genesis_hash(&self) -> Result<[u8; 32], LedgerError> {
        Ok([7; 32])
    }
    fn lottery_pool(&self) -> Result<u128, LedgerError> {
        Ok(1_000_000_000_000_000)
    }
    fn output_context(&self, _: &UtxoId) -> Result<(Utxo, StoredContext), LedgerError> {
        Err(LedgerError::InconsistentRecord(
            "mock has no outputs".into(),
        ))
    }
    fn tip_header(&self) -> Result<BlockHeader, LedgerError> {
        Ok(self.tip.clone())
    }
    /// The production window rule in miniature: key order rotated to start
    /// at `Ledger::lottery_candidate_offset_key(seed, height)`, capped.
    fn lottery_candidates(
        &self,
        block: &Block,
        seed: &[u8; 32],
        _: &LotteryDrawConfig,
    ) -> Result<Vec<LotteryCandidate>, LedgerError> {
        self.windows.borrow_mut().push(*seed);
        let offset = Ledger::lottery_candidate_offset_key(seed, block.height());
        let mut sorted = self.candidates.clone();
        sorted.sort_by_key(|c| c.id);
        let (lo, hi): (Vec<_>, Vec<_>) = sorted.into_iter().partition(|c| c.id < offset);
        Ok(hi.into_iter().chain(lo).take(MOCK_WINDOW).collect())
    }
}

fn parent_header(nonce: u64, timestamp: u64, tx_root: [u8; 32]) -> BlockHeader {
    BlockHeader {
        version: 1,
        prev_block_hash: [0x11; 32],
        tx_root,
        timestamp,
        height: CHILD_HEIGHT - 1,
        difficulty: u64::MAX,
        nonce,
        minter_view_key: [0x22; 32],
        minter_spend_key: [0x33; 32],
    }
}

fn child_of(parent: &BlockHeader) -> Block {
    let mut block = Block::genesis();
    block.header.height = CHILD_HEIGHT;
    block.header.prev_block_hash = parent.hash();
    block.header.timestamp = parent.timestamp + 5;
    block.minting_tx.block_height = CHILD_HEIGHT;
    block.minting_tx.reward = 50_000_000_000_000;
    block
}

/// (seed, window, draw winners) of the child of `parent`.
fn draw_after(parent: &BlockHeader) -> ([u8; 32], Vec<[u8; 36]>, Vec<([u8; 36], u64)>) {
    let mock = Mock::new(parent);
    let child = child_of(parent);
    let seed = lottery_v2::seed(&mock, &child).unwrap();
    let (draw, _) = lottery_v2::drawing(&mock, &child).unwrap();
    assert_eq!(
        *mock.windows.borrow(),
        vec![seed],
        "the window is seeded by the draw seed"
    );
    let window = mock
        .lottery_candidates(&child, &seed, &LotteryDrawConfig::default())
        .unwrap()
        .iter()
        .map(|c| c.id)
        .collect();
    assert!(!draw.winners.is_empty(), "fixture must draw");
    assert_eq!(draw.seed, seed, "BlockLotteryResult.seed is the V2 seed");
    let winners = draw.winners.iter().map(|w| (w.utxo_id, w.payout)).collect();
    (seed, window, winners)
}

#[test]
fn reheaded_parent_with_same_pow_fields_draws_identically() {
    // Same PoW preimage (nonce, prev, minter keys) and height; different
    // timestamp and tx_root (a regenerated coinbase stealth output changes the
    // V2 body root), hence a different header hash.
    let a = parent_header(42, 1_700_000_000, [0x01; 32]);
    let b = parent_header(42, 1_700_000_017, [0x02; 32]);
    assert_ne!(a.hash(), b.hash(), "the free levers change hash(N-1)");
    assert_eq!(a.pow_hash(), b.pow_hash(), "but not the PoW output");

    let (seed_a, window_a, winners_a) = draw_after(&a);
    let (seed_b, window_b, winners_b) = draw_after(&b);
    assert_eq!(seed_a, seed_b, "identical seed");
    assert_eq!(
        Ledger::lottery_candidate_offset_key(&seed_a, CHILD_HEIGHT),
        Ledger::lottery_candidate_offset_key(&seed_b, CHILD_HEIGHT),
        "identical window offset"
    );
    assert_eq!(window_a, window_b, "identical candidate window");
    assert_eq!(winners_a, winners_b, "identical draw");
}

#[test]
fn different_nonce_changes_seed_and_window() {
    let a = parent_header(42, 1_700_000_000, [0x01; 32]);
    let c = parent_header(43, 1_700_000_000, [0x01; 32]);
    assert_ne!(a.pow_hash(), c.pow_hash());
    let (seed_a, _, _) = draw_after(&a);
    let (seed_c, _, _) = draw_after(&c);
    assert_ne!(seed_a, seed_c, "a new PoW solution re-rolls the seed");
    assert_ne!(
        Ledger::lottery_candidate_offset_key(&seed_a, CHILD_HEIGHT),
        Ledger::lottery_candidate_offset_key(&seed_c, CHILD_HEIGHT),
        "and the window offset"
    );
    // Across many nonces the windows/draws are not stuck: the seed is live.
    let distinct: std::collections::BTreeSet<_> = (0..8)
        .map(|n| draw_after(&parent_header(n, 1_700_000_000, [0x01; 32])).2)
        .collect();
    assert!(distinct.len() > 1, "nonce must move the draw");
}

#[test]
fn seed_is_the_documented_construction() {
    let a = parent_header(42, 1_700_000_000, [0x01; 32]);
    let mock = Mock::new(&a);
    let child = child_of(&a);
    let expected: [u8; 32] = {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(b"BOTHO_LOTTERY_SEED_V2");
        h.update(a.pow_hash());
        h.update(a.prev_block_hash);
        h.update(CHILD_HEIGHT.to_le_bytes());
        h.finalize().into()
    };
    assert_eq!(lottery_v2::seed(&mock, &child).unwrap(), expected);
    // Height is bound.
    assert_ne!(
        lottery_seed_v2(&a.pow_hash(), &a.prev_block_hash, CHILD_HEIGHT + 1),
        expected
    );
}

#[test]
fn seed_requires_the_linked_parent() {
    let a = parent_header(42, 1_700_000_000, [0x01; 32]);
    let mock = Mock::new(&a);
    // Child naming a different parent hash: invalid block.
    let mut child = child_of(&a);
    child.header.prev_block_hash[0] ^= 1;
    assert!(matches!(
        lottery_v2::seed(&mock, &child),
        Err(LedgerError::InvalidBlock(_))
    ));
    assert!(matches!(
        lottery_v2::drawing(&mock, &child),
        Err(LedgerError::InvalidBlock(_))
    ));
    // Wrong height (not tip + 1): invalid block, classified before any
    // seed is derived.
    let mut orphan = child_of(&a);
    orphan.header.height += 1;
    assert!(matches!(
        lottery_v2::seed(&mock, &orphan),
        Err(LedgerError::InvalidBlock(_))
    ));
    // Genesis has no drawing.
    assert!(matches!(
        lottery_v2::seed(&mock, &Block::genesis()),
        Err(LedgerError::InvalidBlock(_))
    ));
}

// ---------------------------------------------------------------------------
// Real ledgers: seal_block (producer) / add_block (validator).
// ---------------------------------------------------------------------------

fn open(dir: &Path) -> Ledger {
    let ledger = Ledger::open(dir).unwrap();
    ledger.set_difficulty(u64::MAX).unwrap();
    ledger
}

fn template(ledger: &Ledger, wallet: &Wallet, txs: Vec<Transaction>) -> Block {
    let state = ledger.get_chain_state().unwrap();
    let tip = ledger.get_tip().unwrap();
    let mut block = Block::new_template_with_txs(
        &tip,
        &wallet.default_address(),
        state.difficulty,
        calculate_block_reward(state.height + 1, state.total_mined),
        txs,
    );
    block.header.timestamp = block.header.timestamp.max(state.tip_timestamp);
    block.minting_tx.timestamp = block.header.timestamp;
    while !block.header.is_valid_pow() {
        block.header.nonce += 1;
    }
    block.minting_tx.nonce = block.header.nonce;
    block
}

fn view_seed(ledger: &Ledger, block: &Block) -> [u8; 32] {
    let txn = ledger.env.read_txn().unwrap();
    lottery_v2::seed(&ledger.production_view(&txn), block).unwrap()
}

/// Height 1 seeds from the fixed genesis header; producer and validator agree.
#[test]
fn height_one_seeds_from_genesis_pow() {
    let dir = TempDir::new().unwrap();
    let ledger = open(dir.path());
    let wallet = Wallet::from_mnemonic(WORDS).unwrap();
    let genesis = ledger.get_tip().unwrap();
    let block = template(&ledger, &wallet, vec![]);
    assert_eq!(
        view_seed(&ledger, &block),
        lottery_seed_v2(
            &genesis.header.pow_hash(),
            &genesis.header.prev_block_hash,
            1
        )
    );
    let sealed = ledger.seal_block(block).unwrap();
    ledger
        .add_block(&sealed)
        .expect("validator accepts producer");
}

/// The real-ledger twin of
/// `reheaded_parent_with_same_pow_fields_draws_identically`: two ledgers share
/// history to height k, then commit sibling parents with the same PoW fields
/// but a different timestamp and a regenerated coinbase output. Both next
/// blocks see the identical seed (hence window and draw), and each is sealed by
/// the producer and accepted by the validator.
#[test]
fn sibling_parents_with_same_pow_fields_seed_identically_on_real_ledgers() {
    let wallet = Wallet::from_mnemonic(WORDS).unwrap();
    let (d1, d2) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let (l1, l2) = (open(d1.path()), open(d2.path()));
    for _ in 0..3 {
        let block = l1.seal_block(template(&l1, &wallet, vec![])).unwrap();
        l1.add_block(&block).unwrap();
        l2.add_block(&block).unwrap();
    }

    let parent_a = l1.seal_block(template(&l1, &wallet, vec![])).unwrap();
    // Fresh template: a new random coinbase stealth output.
    let mut b = template(&l2, &wallet, vec![]);
    b.header.timestamp = parent_a.header.timestamp + 1;
    b.minting_tx.timestamp = b.header.timestamp;
    b.header.nonce = parent_a.header.nonce;
    b.minting_tx.nonce = parent_a.header.nonce;
    let parent_b = l2.seal_block(b).unwrap();
    assert_ne!(
        parent_a.minting_tx.target_key,
        parent_b.minting_tx.target_key
    );
    assert_ne!(parent_a.header.tx_root, parent_b.header.tx_root);
    assert_ne!(parent_a.hash(), parent_b.hash());
    assert_eq!(parent_a.header.pow_hash(), parent_b.header.pow_hash());
    l1.add_block(&parent_a).unwrap();
    l2.add_block(&parent_b).unwrap();

    let child_1 = template(&l1, &wallet, vec![]);
    let child_2 = template(&l2, &wallet, vec![]);
    let (s1, s2) = (view_seed(&l1, &child_1), view_seed(&l2, &child_2));
    assert_eq!(s1, s2, "identical next-block seed");
    let height = child_1.height();
    let config = lottery_v2::canonical_config().draw_config;
    let window = |l: &Ledger, seed: &[u8; 32]| -> Vec<[u8; 36]> {
        l.get_lottery_validation_candidates(height, seed, &config)
            .unwrap()
            .iter()
            .map(|c| c.id)
            .collect()
    };
    assert_eq!(
        Ledger::lottery_candidate_offset_key(&s1, height),
        Ledger::lottery_candidate_offset_key(&s2, height),
        "identical window offset"
    );
    assert_eq!(window(&l1, &s1), window(&l2, &s2));
    for (l, child) in [(&l1, child_1), (&l2, child_2)] {
        let sealed = l.seal_block(child).unwrap();
        l.add_block(&sealed).expect("validator accepts producer");
    }

    // A different nonce on the parent re-rolls the seed.
    let mut other = parent_a.header.clone();
    other.nonce += 1;
    assert_ne!(
        lottery_seed_v2(&other.pow_hash(), &other.prev_block_hash, height),
        s1
    );
}
