//! Protocol 8.0.0 median-time-past timestamp rules on the production ledger
//! (#1848).
//!
//! The lower bound for block `h + 1` is `timestamp > MTP(h)` (the median of
//! the last 11 non-genesis header timestamps, zero-padded), and it REPLACES
//! the 7.x `timestamp >= parent` rule. The window is persisted in the block
//! write txn, read back by `get_chain_state`, carried and verified by V2
//! snapshots, and a store past genesis without it is refused at open.
use super::*;
use crate::{
    block::calculate_block_reward,
    ledger::{median_time_past_of, MTP_WINDOW},
    wallet::Wallet,
};
use tempfile::TempDir;

// Public deterministic test mnemonic, used only for temporary fixtures.
const WORDS: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn open(dir: &Path) -> Ledger {
    let ledger = Ledger::open(dir).unwrap();
    ledger.set_difficulty(u64::MAX).unwrap();
    ledger
}

/// The next block on `ledger`'s tip, stamped exactly `timestamp` (header and
/// minting tx agree), sealed by the V2 producer.
fn block_at(ledger: &Ledger, wallet: &Wallet, timestamp: u64) -> Block {
    let state = ledger.get_chain_state().unwrap();
    let tip = ledger.get_tip().unwrap();
    let mut block = Block::new_template_with_txs(
        &tip,
        &wallet.default_address(),
        state.difficulty,
        calculate_block_reward(state.height + 1, state.total_mined),
        vec![],
    );
    block.header.timestamp = timestamp;
    block.minting_tx.timestamp = timestamp;
    ledger.seal_block(block).unwrap()
}

fn mine_at(ledger: &Ledger, wallet: &Wallet, timestamp: u64) -> Result<Block, LedgerError> {
    let block = block_at(ledger, wallet, timestamp);
    ledger.add_block(&block).map(|()| block)
}

fn assert_rejected(result: Result<Block, LedgerError>, needle: &str) {
    match result {
        Err(LedgerError::InvalidBlock(msg)) => {
            assert!(msg.contains(needle), "expected {needle:?}, got {msg:?}")
        }
        other => panic!("expected InvalidBlock({needle:?}), got {other:?}"),
    }
}

// ---- The pure median ------------------------------------------------------

#[test]
fn median_of_a_short_window_is_zero_padded() {
    // Fewer than 6 entries: at least 6 zero slots, so the median is 0 and only
    // the future bound applies (no early-chain lockout by a forward block 1).
    assert_eq!(median_time_past_of(&[]), 0);
    assert_eq!(median_time_past_of(&[u64::MAX]), 0);
    assert_eq!(median_time_past_of(&[5, 4, 3, 2, 1]), 0);
    // Six entries: the median is their minimum (5 zero pads below it).
    assert_eq!(median_time_past_of(&[60, 10, 50, 20, 40, 30]), 10);
    // Ten entries: one pad, the median is the 5th smallest real entry.
    assert_eq!(
        median_time_past_of(&[10, 20, 30, 40, 50, 60, 70, 80, 90, 100]),
        50
    );
}

#[test]
fn median_of_exactly_eleven_ignores_one_outlier() {
    let honest: Vec<u64> = (0..10).map(|i| 1_000 + 5 * i).collect();
    let mut window = honest.clone();
    window.push(1_000 + 7_000);
    assert_eq!(window.len(), MTP_WINDOW);
    // The outlier lands at the top of the sort; the median is the 6th honest
    // stamp, exactly as without it.
    assert_eq!(median_time_past_of(&window), 1_025);
    let mut backdated = honest.clone();
    backdated.push(0);
    assert_eq!(median_time_past_of(&backdated), 1_020);
    // Order does not matter.
    window.reverse();
    assert_eq!(median_time_past_of(&window), 1_025);
}

#[test]
fn median_only_counts_the_last_eleven() {
    let mut window: Vec<u64> = vec![u64::MAX; 5];
    window.extend((0..11).map(|i| 100 + i));
    assert_eq!(median_time_past_of(&window), 105);
}

#[test]
fn rolled_window_keeps_the_last_eleven_oldest_first() {
    let mut state = ChainState::default();
    for ts in 1..=13u64 {
        state.recent_timestamps = state.rolled_timestamps(ts);
    }
    assert_eq!(state.recent_timestamps, (3..=13).collect::<Vec<_>>());
    assert_eq!(state.median_time_past(), 8);
    assert_eq!(state.next_min_timestamp(), 9);
}

// ---- add_block ------------------------------------------------------------

/// Heights 1-6 accept any positive stamps, in any order: their windows hold
/// at most 5 entries, so the zero-padded median is 0.
#[test]
fn chain_start_accepts_arbitrary_past_stamps() {
    let dir = TempDir::new().unwrap();
    let ledger = open(dir.path());
    let wallet = Wallet::from_mnemonic(WORDS).unwrap();
    for ts in [6_000, 1_000, 5_000, 2_000, 4_000, 3_000] {
        assert_eq!(ledger.get_chain_state().unwrap().median_time_past(), 0);
        mine_at(&ledger, &wallet, ts).unwrap();
    }
    // Block 7: the window is six real stamps, so the median is their minimum.
    let state = ledger.get_chain_state().unwrap();
    assert_eq!(state.recent_timestamps.len(), 6);
    assert_eq!(state.median_time_past(), 1_000);
    assert_rejected(mine_at(&ledger, &wallet, 1_000), "median time past");
    mine_at(&ledger, &wallet, 1_001).unwrap();
}

/// The bound is strict and the parent rule is gone: `ts == MTP` is rejected,
/// `parent > ts > MTP` is accepted, `ts > now + 60` is rejected.
#[test]
fn add_block_enforces_median_time_past_not_parent() {
    let dir = TempDir::new().unwrap();
    let ledger = open(dir.path());
    let wallet = Wallet::from_mnemonic(WORDS).unwrap();
    let base = now() - 1_000;
    for i in 0..11 {
        mine_at(&ledger, &wallet, base + 10 * i).unwrap();
    }
    let state = ledger.get_chain_state().unwrap();
    let mtp = state.median_time_past();
    assert_eq!(mtp, base + 50);
    assert_eq!(state.tip_timestamp, base + 100);

    assert_rejected(mine_at(&ledger, &wallet, mtp), "not after median time past");
    assert_rejected(
        mine_at(&ledger, &wallet, now() + MAX_FUTURE_TIMESTAMP_SECS + 5),
        "too far in the future",
    );
    assert_eq!(ledger.get_chain_state().unwrap().height, 11);

    // Before the parent (base + 100) but after the median: valid under 8.0.0.
    let backward = mine_at(&ledger, &wallet, mtp + 1).unwrap();
    assert!(backward.header.timestamp < state.tip_timestamp);
    assert_eq!(ledger.get_chain_state().unwrap().height, 12);
}

/// The ledger-level #1848 regression: a tip forward-dated to the edge of the
/// future bound no longer locks out an honest block stamped `now` (under the
/// 7.x parent rule `now < tip` was rejected).
#[test]
fn forward_dated_tip_does_not_lock_out_honest_block() {
    let dir = TempDir::new().unwrap();
    let ledger = open(dir.path());
    let wallet = Wallet::from_mnemonic(WORDS).unwrap();
    let t = now();
    for i in 0..10 {
        mine_at(&ledger, &wallet, t - 100 + 5 * i).unwrap();
    }
    let forward = mine_at(&ledger, &wallet, t + 55).unwrap();
    let honest_now = now();
    assert!(honest_now < forward.header.timestamp);
    mine_at(&ledger, &wallet, honest_now).expect("honest `now` stamp after a forward-dated tip");
}

/// The window rolls in the block write txn across more than 11 blocks, and a
/// reopened ledger reads back the same window.
#[test]
fn window_rolls_and_survives_restart() {
    let dir = TempDir::new().unwrap();
    let wallet = Wallet::from_mnemonic(WORDS).unwrap();
    let base = now() - 10_000;
    let stamps: Vec<u64> = (0..14).map(|i| base + 7 * i).collect();
    {
        let ledger = open(dir.path());
        for &ts in &stamps {
            mine_at(&ledger, &wallet, ts).unwrap();
        }
        let state = ledger.get_chain_state().unwrap();
        assert_eq!(state.recent_timestamps, stamps[3..].to_vec());
        assert_eq!(state.tip_timestamp, *stamps.last().unwrap());
    }
    let reopened = Ledger::open(dir.path()).unwrap();
    let state = reopened.get_chain_state().unwrap();
    assert_eq!(state.recent_timestamps, stamps[3..].to_vec());
    assert_eq!(state.median_time_past(), stamps[8]);
}

/// A rejected block leaves the window untouched (it is written only inside
/// the committed block txn).
#[test]
fn rejected_block_does_not_roll_the_window() {
    let dir = TempDir::new().unwrap();
    let ledger = open(dir.path());
    let wallet = Wallet::from_mnemonic(WORDS).unwrap();
    let base = now() - 1_000;
    for i in 0..8 {
        mine_at(&ledger, &wallet, base + i).unwrap();
    }
    let before = ledger.get_chain_state().unwrap().recent_timestamps;
    assert_rejected(
        mine_at(&ledger, &wallet, now() + 3_600),
        "too far in the future",
    );
    assert_eq!(ledger.get_chain_state().unwrap().recent_timestamps, before);
}

// ---- Store schema ---------------------------------------------------------

/// A `botho.ledger.2` store past genesis without the persisted window was
/// written by a pre-8.0.0 binary and is refused at open, untouched.
#[test]
fn pre_protocol_8_store_is_refused_at_open() {
    let dir = TempDir::new().unwrap();
    {
        let ledger = open(dir.path());
        let wallet = Wallet::from_mnemonic(WORDS).unwrap();
        mine_at(&ledger, &wallet, now()).unwrap();
        let mut txn = ledger.env.write_txn().unwrap();
        assert!(ledger
            .meta_db
            .delete(&mut txn, META_RECENT_TIMESTAMPS)
            .unwrap());
        txn.commit().unwrap();
    }
    match Ledger::open(dir.path()) {
        Err(LedgerError::UnsupportedSchema(msg)) => {
            assert!(msg.contains("before protocol 8.0.0"), "{msg}")
        }
        Err(other) => panic!("expected UnsupportedSchema, got {other:?}"),
        Ok(_) => panic!("a pre-8.0.0 store must be refused"),
    }
}

/// A genesis-only store has an empty window and opens either way.
#[test]
fn genesis_only_store_opens_with_an_empty_window() {
    let dir = TempDir::new().unwrap();
    drop(open(dir.path()));
    let ledger = Ledger::open(dir.path()).unwrap();
    let state = ledger.get_chain_state().unwrap();
    assert_eq!(state.height, 0);
    assert!(state.recent_timestamps.is_empty());
    assert_eq!(state.median_time_past(), 0);
}

/// A malformed window row is a storage-encoding error, never a default.
#[test]
fn malformed_window_row_is_rejected() {
    let dir = TempDir::new().unwrap();
    let ledger = open(dir.path());
    for bad in [vec![0u8; 7], vec![0u8; 8 * (MTP_WINDOW + 1)]] {
        let mut txn = ledger.env.write_txn().unwrap();
        ledger
            .meta_db
            .put(&mut txn, META_RECENT_TIMESTAMPS, &bad)
            .unwrap();
        txn.commit().unwrap();
        assert!(matches!(
            ledger.get_chain_state(),
            Err(LedgerError::StorageEncoding(_))
        ));
    }
}

// ---- V2 snapshots ---------------------------------------------------------

/// A ledger restored from a snapshot at height H computes the same MTP as the
/// genesis-synced source and accepts and rejects the same next blocks.
#[test]
fn snapshot_restore_carries_the_window() {
    let wallet = Wallet::from_mnemonic(WORDS).unwrap();
    let src_dir = TempDir::new().unwrap();
    let src = open(src_dir.path());
    let base = now() - 1_000;
    // Out-of-order stamps so the median differs from the parent.
    for i in 0..13u64 {
        let ts = base + 10 * i + if i % 2 == 0 { 0 } else { 30 };
        mine_at(&src, &wallet, ts).unwrap();
    }
    let src_state = src.get_chain_state().unwrap();
    let snapshot = src.create_snapshot().unwrap();
    assert_eq!(
        snapshot.chain_state.recent_timestamps,
        src_state.recent_timestamps
    );

    let dst_dir = TempDir::new().unwrap();
    let dst = Ledger::open(dst_dir.path()).unwrap();
    dst.load_from_snapshot(&snapshot, None).unwrap();
    let dst_state = dst.get_chain_state().unwrap();
    assert_eq!(dst_state.recent_timestamps, src_state.recent_timestamps);
    assert_eq!(dst_state.median_time_past(), src_state.median_time_past());

    // The same block at MTP is rejected by both; at MTP + 1 accepted by both.
    let mtp = src_state.median_time_past();
    let at_mtp = block_at(&src, &wallet, mtp);
    assert!(src.add_block(&at_mtp).is_err());
    assert!(dst.add_block(&at_mtp).is_err());
    let next = block_at(&src, &wallet, mtp + 1);
    src.add_block(&next).unwrap();
    dst.add_block(&next).unwrap();
    assert_eq!(
        dst.get_chain_state().unwrap().recent_timestamps,
        src.get_chain_state().unwrap().recent_timestamps
    );
}

/// A consistently re-committed snapshot whose window disagrees with its tip
/// or height is refused before any write.
#[test]
fn snapshot_with_inconsistent_window_is_rejected() {
    let wallet = Wallet::from_mnemonic(WORDS).unwrap();
    let src_dir = TempDir::new().unwrap();
    let src = open(src_dir.path());
    let base = now() - 1_000;
    for i in 0..4u64 {
        mine_at(&src, &wallet, base + i).unwrap();
    }
    let snapshot = src.create_snapshot().unwrap();
    let recommit = |edit: &dyn Fn(&mut ChainState)| {
        let mut state = snapshot.chain_state.clone();
        edit(&mut state);
        crate::ledger::UtxoSnapshotV2::new(
            snapshot.genesis_hash,
            snapshot.height,
            snapshot.block_hash,
            state,
            snapshot.lottery_pool,
            &snapshot.tip_block_bytes().unwrap(),
            &snapshot.contents().unwrap(),
        )
        .unwrap()
    };
    let forgeries = [
        // Last entry is not the tip's timestamp (median pulled back).
        recommit(&|s| *s.recent_timestamps.last_mut().unwrap() -= 1),
        // Too short for the height.
        recommit(&|s| {
            s.recent_timestamps.remove(0);
        }),
        // Too long for the height.
        recommit(&|s| s.recent_timestamps.insert(0, 0)),
    ];
    let dst_dir = TempDir::new().unwrap();
    let dst = Ledger::open(dst_dir.path()).unwrap();
    for forged in forgeries {
        forged.verify().unwrap();
        let err = dst.load_from_snapshot(&forged, None).unwrap_err();
        assert!(err.to_string().contains("timestamp window"), "{err}");
        assert_eq!(dst.get_chain_state().unwrap().height, 0);
    }
    dst.load_from_snapshot(&snapshot, None).unwrap();
}
