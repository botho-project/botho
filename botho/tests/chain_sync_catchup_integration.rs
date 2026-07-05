// Copyright (c) 2024 Botho Foundation
//
//! Chain-sync (initial block download / catch-up) integration tests.
//!
//! Regression coverage for #376: a node joining an existing chain (peer tip at
//! height N) could not sync the historical blocks. It only received the current
//! tip via gossip and rejected it ("Expected height 1, got N"), staying at
//! height 0 forever, because the `ChainSyncManager` state machine — although
//! implemented — was never driven and no node ever requested the missing block
//! range.
//!
//! These tests exercise the *production* `ChainSyncManager` against real
//! LMDB-backed ledgers. They simulate the request/response loop that the node
//! event loop now runs (see `commands/run.rs`): a fresh node detects it is
//! behind a peer, requests the missing range, and applies the blocks
//! sequentially via the ledger's `add_block` — the exact path where the
//! "Expected height" error originated. Driving the real state machine + real
//! ledger (rather than libp2p networking, which is non-deterministic in a unit
//! test) gives a fast, deterministic proof that the catch-up path works.

use std::time::SystemTime;

use libp2p::PeerId;
use serial_test::serial;
use tempfile::TempDir;

use botho::{
    block::{Block, BlockHeader, BlockLotterySummary, MintingTx},
    ledger::Ledger,
    network::{ChainSyncManager, SyncAction, SyncRequest, SyncResponse, SyncState},
    transaction::{Transaction, PICOCREDITS_PER_CREDIT},
};
use botho_wallet::WalletKeys;
use bth_account_keys::PublicAddress;
use sha2::{Digest, Sha256};

// ============================================================================
// Constants
// ============================================================================

const TEST_BLOCK_REWARD: u64 = 50 * PICOCREDITS_PER_CREDIT;
const TRIVIAL_DIFFICULTY: u64 = u64::MAX;

/// Ring size floor: the existing all-at-genesis tests never push a node past a
/// few blocks; #376 specifically requires syncing a chain "of height N > ring
/// size", so we build well past this.
const RING_SIZE: u64 = 20;

// ============================================================================
// Block-building helpers (mirror ledger_consistency_integration.rs)
// ============================================================================

fn create_test_wallet() -> WalletKeys {
    let mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";
    WalletKeys::from_mnemonic(mnemonic).expect("Failed to create wallet from mnemonic")
}

fn create_mock_minting_tx(
    height: u64,
    reward: u64,
    minter_address: &PublicAddress,
    prev_block_hash: [u8; 32],
) -> MintingTx {
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let mut minting_tx = MintingTx::new(
        height,
        reward,
        minter_address,
        prev_block_hash,
        TRIVIAL_DIFFICULTY,
        timestamp,
    );

    for nonce in 0..u64::MAX {
        minting_tx.nonce = nonce;
        if minting_tx.verify_pow() {
            break;
        }
    }

    minting_tx
}

/// Build (but do not apply) the next block on top of the ledger's tip.
fn mine_block(
    ledger: &Ledger,
    minter_address: &PublicAddress,
    transactions: Vec<Transaction>,
) -> Block {
    let state = ledger.get_chain_state().expect("Failed to get chain state");
    let prev_block = ledger.get_tip().expect("Failed to get tip");
    let prev_hash = prev_block.hash();
    let height = state.height + 1;

    let minting_tx = create_mock_minting_tx(height, TEST_BLOCK_REWARD, minter_address, prev_hash);

    let tx_root = if transactions.is_empty() {
        [0u8; 32]
    } else {
        let mut hasher = Sha256::new();
        for tx in &transactions {
            hasher.update(tx.hash());
        }
        hasher.finalize().into()
    };

    Block {
        header: BlockHeader {
            version: 1,
            prev_block_hash: prev_hash,
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
        lottery_summary: BlockLotterySummary::default(),
    }
}

/// Build a chain in `ledger` up to `target_height` (empty blocks).
fn build_chain_to_height(ledger: &Ledger, minter: &PublicAddress, target_height: u64) {
    while ledger.get_chain_state().unwrap().height < target_height {
        let block = mine_block(ledger, minter, vec![]);
        ledger.add_block(&block).expect("Failed to add block");
    }
}

/// Serve a `GetBlocks` request from a source ledger, exactly as the node event
/// loop does in `commands/run.rs`.
fn serve_get_blocks(source: &Ledger, start_height: u64, count: u32) -> SyncResponse {
    let mut blocks = Vec::new();
    let end_height = start_height.saturating_add(count as u64).saturating_sub(1);
    for height in start_height..=end_height.min(start_height + 99) {
        if let Ok(block) = source.get_block(height) {
            blocks.push(block);
        } else {
            break;
        }
    }
    let has_more = blocks.len() == count as usize;
    SyncResponse::Blocks { blocks, has_more }
}

/// Run the chain-sync state machine for a fresh `node` ledger against a
/// `source` ledger that is already at some height, mirroring the production
/// request/response dispatch in `commands/run.rs`. Returns the number of sync
/// iterations performed (for sanity assertions).
///
/// This is the in-process analogue of: node B connects to node A, discovers A
/// is ahead, requests the missing block range, and applies it sequentially.
fn run_catchup(
    sync_manager: &mut ChainSyncManager,
    node: &Ledger,
    source: &Ledger,
    peer: PeerId,
) -> usize {
    let source_state = source.get_chain_state().unwrap();
    let connected = [peer];

    let mut iterations = 0;
    // Generous cap: each iteration makes forward progress (status, or a batch
    // of up to 100 blocks). A runaway loop indicates a real bug, not a slow
    // sync, so the cap doubles as a liveness assertion.
    let max_iterations = 10_000;

    loop {
        iterations += 1;
        assert!(
            iterations < max_iterations,
            "chain sync did not converge within {} iterations",
            max_iterations
        );

        // Keep the manager's view of our height current, as the node does on
        // every sync tick.
        sync_manager.set_local_height(node.get_chain_state().unwrap().height);

        let Some(action) = sync_manager.tick(&connected) else {
            // No action this tick.
            if node.get_chain_state().unwrap().height >= source_state.height {
                break;
            }
            // We are still behind but the manager produced no action (e.g. it
            // is `Synced` against a stale status because the peer advanced after
            // our last poll). In production this is resolved either by the 30s
            // `STATUS_REFRESH_INTERVAL` re-poll or, promptly, by a gossiped tip
            // block that the node cannot apply across the gap. We model the
            // latter — the real production trigger (issue #423 RC2) — rather
            // than injecting `on_status` out of band, so this test exercises
            // only triggers the production loop actually has.
            sync_manager.note_gossiped_tip(&connected, source_state.height, source_state.tip_hash);
            continue;
        };

        match action {
            SyncAction::RequestStatus(p) => {
                assert_eq!(p, peer);
                sync_manager.on_request_sent(p);
                // Source answers with its current status.
                sync_manager.on_status(p, source_state.height, source_state.tip_hash);
            }
            SyncAction::RequestBlocks {
                peer: p,
                start_height,
                count,
            } => {
                assert_eq!(p, peer);
                sync_manager.on_request_sent(p);
                // Source answers with the requested block range.
                let SyncResponse::Blocks { blocks, has_more } =
                    serve_get_blocks(source, start_height, count)
                else {
                    panic!("expected Blocks response");
                };

                if let Some(SyncAction::AddBlocks(blocks)) =
                    sync_manager.on_blocks(&p, blocks, has_more)
                {
                    for block in &blocks {
                        node.add_block(block).unwrap_or_else(|e| {
                            panic!("failed to apply synced block {}: {}", block.height(), e)
                        });
                    }
                    let new_height = node.get_chain_state().unwrap().height;
                    sync_manager.on_blocks_added(new_height);
                }
            }
            SyncAction::Synced => break,
            SyncAction::Wait(_) | SyncAction::AddBlocks(_) => {}
        }

        if sync_manager.is_synced() && node.get_chain_state().unwrap().height >= source_state.height
        {
            break;
        }
    }

    iterations
}

// ============================================================================
// Tests
// ============================================================================

/// Core regression test for #376.
///
/// Node B starts with an empty ledger (genesis only, height 0) and joins a peer
/// (node A) whose chain is already at height N (> ring size). B must sync all
/// the way to A's tip via the chain-sync state machine. This is distinct from
/// the existing `e2e_consensus_integration` tests, where every node starts at
/// genesis and grows in lockstep — so no node ever has to *catch up*.
#[test]
#[serial]
fn test_fresh_node_syncs_existing_chain_to_tip() {
    let target_height = RING_SIZE + 75; // 95 — well past ring size, mirrors the betanet repro

    // --- Source node A: build a chain to height N ---
    let source_dir = TempDir::new().unwrap();
    let source = Ledger::open(source_dir.path()).unwrap();
    // RandomX genesis difficulty is real-hashrate sized; pin the chain to
    // the trivial target so test PoW solves in one hash and the C1
    // block-apply difficulty check accepts it.
    source.set_difficulty(TRIVIAL_DIFFICULTY).unwrap();
    let minter = create_test_wallet().public_address();
    build_chain_to_height(&source, &minter, target_height);

    let source_state = source.get_chain_state().unwrap();
    assert_eq!(source_state.height, target_height, "source built to target");

    // --- Fresh node B: empty ledger at genesis ---
    let node_dir = TempDir::new().unwrap();
    let node = Ledger::open(node_dir.path()).unwrap();
    // RandomX genesis difficulty is real-hashrate sized; pin the chain to
    // the trivial target so test PoW solves in one hash and the C1
    // block-apply difficulty check accepts it.
    node.set_difficulty(TRIVIAL_DIFFICULTY).unwrap();
    assert_eq!(
        node.get_chain_state().unwrap().height,
        0,
        "fresh node starts at genesis"
    );

    // Sanity: confirm the bug's symptom — gossiping the tip block alone to a
    // fresh node is rejected, because the intermediate blocks are missing.
    let tip_block = source.get_block(target_height).unwrap();
    let rejected = node.add_block(&tip_block);
    assert!(
        rejected.is_err(),
        "tip block should be rejected by a genesis node (the #376 symptom)"
    );

    // --- Drive the catch-up state machine ---
    let mut sync_manager = ChainSyncManager::new(0);
    let peer = PeerId::random();
    let iterations = run_catchup(&mut sync_manager, &node, &source, peer);

    // --- Verify B reached the tip and matches A exactly ---
    let node_state = node.get_chain_state().unwrap();
    assert_eq!(
        node_state.height, target_height,
        "fresh node should have synced to the source tip height"
    );
    assert_eq!(
        node_state.tip_hash, source_state.tip_hash,
        "fresh node tip hash must match the source chain tip"
    );
    assert!(
        sync_manager.is_synced(),
        "sync manager should report Synced after catching up"
    );

    // The sync should have completed in a reasonable number of round trips:
    // 1 status + ceil(95/100) = 1 block batch, plus a couple of housekeeping
    // ticks. This guards against a regression that re-requests the same range.
    assert!(
        iterations < 50,
        "catch-up took an unexpected number of iterations: {}",
        iterations
    );

    // Every historical block must be present and walkable.
    for h in 1..=target_height {
        let block = node
            .get_block(h)
            .unwrap_or_else(|e| panic!("synced node missing block {}: {}", h, e));
        assert_eq!(block.height(), h);
    }
}

/// Drive catch-up using ONLY the production Discovery round-trip: the node
/// requests status from its peer, gets a single status response, and must enter
/// `Downloading` and page the block range to completion. There is NO
/// out-of-band `on_status` injection on the idle path and NO gossiped-tip hint
/// — this is the minimal trigger a fresh joiner actually has on first connect.
///
/// Pre-#423 this would stall for any small gap (peer tip <= 10): the Discovery
/// arm sent one `RequestStatus`, received the status, evaluated
/// `tip > local + SYNC_BEHIND_THRESHOLD` = FALSE, jumped straight to `Synced`,
/// and never downloaded a single block.
fn run_catchup_discovery_only(
    sync_manager: &mut ChainSyncManager,
    node: &Ledger,
    source: &Ledger,
    peer: PeerId,
) {
    let source_state = source.get_chain_state().unwrap();
    let connected = [peer];

    for _ in 0..10_000 {
        sync_manager.set_local_height(node.get_chain_state().unwrap().height);

        let Some(action) = sync_manager.tick(&connected) else {
            // Strictly no fallback injection: if the production triggers we
            // model here are insufficient, the test must fail (stall) rather
            // than be papered over.
            if node.get_chain_state().unwrap().height >= source_state.height {
                return;
            }
            continue;
        };

        match action {
            SyncAction::RequestStatus(p) => {
                sync_manager.on_request_sent(p);
                sync_manager.on_status(p, source_state.height, source_state.tip_hash);
            }
            SyncAction::RequestBlocks {
                peer: p,
                start_height,
                count,
            } => {
                sync_manager.on_request_sent(p);
                let SyncResponse::Blocks { blocks, has_more } =
                    serve_get_blocks(source, start_height, count)
                else {
                    panic!("expected Blocks response");
                };
                if let Some(SyncAction::AddBlocks(blocks)) =
                    sync_manager.on_blocks(&p, blocks, has_more)
                {
                    for block in &blocks {
                        node.add_block(block).unwrap();
                    }
                    sync_manager.on_blocks_added(node.get_chain_state().unwrap().height);
                }
            }
            SyncAction::Synced => {
                if node.get_chain_state().unwrap().height >= source_state.height {
                    return;
                }
            }
            SyncAction::Wait(_) | SyncAction::AddBlocks(_) => {}
        }
    }
    panic!("discovery-only catch-up did not converge");
}

/// REGRESSION (#423): a fresh joiner at height 0 against a SMALL tip (height 9
/// — well under the old `SYNC_BEHIND_THRESHOLD = 10`) must trigger the
/// historical catch-up download and sync 0->9 using only the Discovery status
/// round-trip.
///
/// This is the regime the original
/// `test_fresh_node_syncs_existing_chain_to_tip` missed: it used tip = 95 (so
/// `95 > 0 + 10` was TRUE) and `run_catchup` injected `on_status` out of band
/// on the idle path. This test uses a small tip and no injection, so it FAILS
/// pre-fix (the Discovery arm jumps to `Synced` at height 0) and PASSES
/// post-fix (the gap-1 trigger enters `Downloading`).
#[test]
#[serial]
fn test_fresh_node_syncs_small_gap_chain_discovery_only() {
    let target_height = 9; // < old SYNC_BEHIND_THRESHOLD (10): the exact repro regime

    let source_dir = TempDir::new().unwrap();
    let source = Ledger::open(source_dir.path()).unwrap();
    // RandomX genesis difficulty is real-hashrate sized; pin the chain to
    // the trivial target so test PoW solves in one hash and the C1
    // block-apply difficulty check accepts it.
    source.set_difficulty(TRIVIAL_DIFFICULTY).unwrap();
    let minter = create_test_wallet().public_address();
    build_chain_to_height(&source, &minter, target_height);
    let source_state = source.get_chain_state().unwrap();
    assert_eq!(source_state.height, target_height);

    let node_dir = TempDir::new().unwrap();
    let node = Ledger::open(node_dir.path()).unwrap();
    // RandomX genesis difficulty is real-hashrate sized; pin the chain to
    // the trivial target so test PoW solves in one hash and the C1
    // block-apply difficulty check accepts it.
    node.set_difficulty(TRIVIAL_DIFFICULTY).unwrap();
    assert_eq!(node.get_chain_state().unwrap().height, 0);

    let mut sync_manager = ChainSyncManager::new(0);
    let peer = PeerId::random();
    run_catchup_discovery_only(&mut sync_manager, &node, &source, peer);

    let node_state = node.get_chain_state().unwrap();
    assert_eq!(
        node_state.height, target_height,
        "fresh node must catch up to a small tip (9) via the Discovery trigger"
    );
    assert_eq!(node_state.tip_hash, source_state.tip_hash);
    assert!(sync_manager.is_synced());

    for h in 1..=target_height {
        let block = node.get_block(h).unwrap();
        assert_eq!(block.height(), h);
    }
}

/// REGRESSION (#423) unit-level: the `on_status` gate must enter `Downloading`
/// for a small gap (>= 2) and stay `Synced` for a 1-block lag (gossip closes
/// that). Pre-fix, `on_status(peer, 9, ..)` from height 0 went to `Synced`.
#[test]
#[serial]
fn test_on_status_gap_triggers_download_boundary() {
    // gap = 9 (>= 2): must enter Downloading even though 9 < old threshold 10.
    let mut sm = ChainSyncManager::new(0);
    sm.on_status(PeerId::random(), 9, [1u8; 32]);
    assert!(
        !sm.is_synced(),
        "gap of 9 (>= 2) must trigger Downloading, not Synced (the #423 bug)"
    );

    // gap = 2: must enter Downloading.
    let mut sm = ChainSyncManager::new(5);
    sm.on_status(PeerId::random(), 7, [1u8; 32]);
    assert!(!sm.is_synced(), "gap of 2 must trigger Downloading");

    // gap = 1: must NOT trigger a download (gossip delivers the next block).
    let mut sm = ChainSyncManager::new(5);
    sm.on_status(PeerId::random(), 6, [1u8; 32]);
    assert!(
        sm.is_synced(),
        "a 1-block lag must not thrash into Downloading; gossip closes it"
    );

    // gap = 0 (equal heights): synced.
    let mut sm = ChainSyncManager::new(5);
    sm.on_status(PeerId::random(), 5, [1u8; 32]);
    assert!(sm.is_synced(), "equal heights are synced");
}

/// REGRESSION (#423): the gossiped-tip fallback must (re)enter catch-up when a
/// node receives a far-ahead tip it cannot apply, instead of waiting for a
/// status refresh. A 1-block-ahead gossip must not trigger a download.
#[test]
#[serial]
fn test_gossiped_tip_fallback_triggers_catchup() {
    let peer = PeerId::random();

    // Far-ahead gossiped tip (gap 9): must enter Downloading even from a
    // node that would otherwise be Synced.
    let mut sm = ChainSyncManager::new(0);
    sm.on_status(peer, 0, [0u8; 32]); // reach Synced (equal height)
    assert!(sm.is_synced());
    sm.note_gossiped_tip(&[peer], 9, [2u8; 32]);
    assert!(
        matches!(
            sm.state(),
            SyncState::Downloading {
                target_height: 9,
                ..
            }
        ),
        "a gossiped far-ahead tip (gap 9) must trigger catch-up from Synced, got {:?}",
        sm.state()
    );

    // 1-block-ahead gossip (gap 1): gossip itself delivers it; no download.
    let mut sm = ChainSyncManager::new(5);
    sm.on_status(peer, 5, [0u8; 32]); // reach Synced
    assert!(sm.is_synced());
    sm.note_gossiped_tip(&[peer], 6, [2u8; 32]);
    assert!(
        !matches!(sm.state(), SyncState::Downloading { .. }),
        "a 1-block-ahead gossiped tip must not trigger a redundant download, got {:?}",
        sm.state()
    );
}

/// A node that is only slightly behind (within the sync-behind threshold) while
/// already `Synced` does not thrash into a redundant initial block download via
/// the Synced-arm hysteresis path. This guards the legitimate purpose of
/// `SYNC_BEHIND_THRESHOLD`.
#[test]
#[serial]
fn test_node_close_to_tip_does_not_trigger_ibd() {
    // Already Synced at height 100, peer drifts to 105 — within
    // SYNC_BEHIND_THRESHOLD (10). The Synced-arm hysteresis must not re-enter
    // Downloading for this small near-tip lag (gossip closes it).
    let mut sync_manager = ChainSyncManager::new(100);
    let peer = PeerId::random();
    // Reach Synced first (equal-height status), then observe a small drift.
    sync_manager.on_status(peer, 100, [7u8; 32]);
    assert!(sync_manager.is_synced(), "equal-height start is synced");

    // A synced node re-polling and seeing a 5-block drift uses the hysteresis
    // threshold, not the gap-1 rule: it must stay Synced. Drive the Synced arm.
    sync_manager.on_status(peer, 105, [7u8; 32]);
    let _ = sync_manager.tick(&[peer]);
    assert!(
        sync_manager.is_synced(),
        "an already-synced node within SYNC_BEHIND_THRESHOLD should not start IBD"
    );
}

/// A node that catches up, then sees the peer advance further, must re-enter
/// download and sync the new blocks too. This models a node that joins, syncs
/// to the (then-current) tip, and keeps up as the chain grows.
#[test]
#[serial]
fn test_node_resyncs_when_peer_advances() {
    let first_target = RING_SIZE + 40; // 60
    let second_target = first_target + 60; // 120

    let source_dir = TempDir::new().unwrap();
    let source = Ledger::open(source_dir.path()).unwrap();
    // RandomX genesis difficulty is real-hashrate sized; pin the chain to
    // the trivial target so test PoW solves in one hash and the C1
    // block-apply difficulty check accepts it.
    source.set_difficulty(TRIVIAL_DIFFICULTY).unwrap();
    let minter = create_test_wallet().public_address();
    build_chain_to_height(&source, &minter, first_target);

    let node_dir = TempDir::new().unwrap();
    let node = Ledger::open(node_dir.path()).unwrap();
    // RandomX genesis difficulty is real-hashrate sized; pin the chain to
    // the trivial target so test PoW solves in one hash and the C1
    // block-apply difficulty check accepts it.
    node.set_difficulty(TRIVIAL_DIFFICULTY).unwrap();

    let mut sync_manager = ChainSyncManager::new(0);
    let peer = PeerId::random();

    // First catch-up to the initial tip.
    run_catchup(&mut sync_manager, &node, &source, peer);
    assert_eq!(node.get_chain_state().unwrap().height, first_target);

    // Peer advances; node must notice and resync.
    build_chain_to_height(&source, &minter, second_target);
    run_catchup(&mut sync_manager, &node, &source, peer);

    let node_state = node.get_chain_state().unwrap();
    let source_state = source.get_chain_state().unwrap();
    assert_eq!(node_state.height, second_target);
    assert_eq!(node_state.tip_hash, source_state.tip_hash);
}

/// The block-serving logic (mirrored from the node's `GetBlocks` handler)
/// returns exactly the requested contiguous range and signals `has_more`
/// correctly so the requester keeps paging until caught up.
#[test]
#[serial]
fn test_get_blocks_serves_requested_range() {
    let source_dir = TempDir::new().unwrap();
    let source = Ledger::open(source_dir.path()).unwrap();
    // RandomX genesis difficulty is real-hashrate sized; pin the chain to
    // the trivial target so test PoW solves in one hash and the C1
    // block-apply difficulty check accepts it.
    source.set_difficulty(TRIVIAL_DIFFICULTY).unwrap();
    let minter = create_test_wallet().public_address();
    build_chain_to_height(&source, &minter, 30);

    // Request a full batch of 100 starting at 1: only 30 blocks exist, so
    // fewer than `count` come back and `has_more` is false.
    let resp = serve_get_blocks(&source, 1, 100);
    let SyncResponse::Blocks { blocks, has_more } = resp else {
        panic!("expected Blocks");
    };
    assert_eq!(blocks.len(), 30);
    assert!(
        !has_more,
        "fewer blocks than requested means no more to page"
    );
    assert_eq!(blocks.first().unwrap().height(), 1);
    assert_eq!(blocks.last().unwrap().height(), 30);

    // Request a smaller window that is fully satisfied: has_more is true.
    let resp = serve_get_blocks(&source, 5, 10);
    let SyncResponse::Blocks { blocks, has_more } = resp else {
        panic!("expected Blocks");
    };
    assert_eq!(blocks.len(), 10);
    assert!(has_more, "a fully satisfied window signals more may follow");
    assert_eq!(blocks.first().unwrap().height(), 5);
    assert_eq!(blocks.last().unwrap().height(), 14);
}

/// `SyncRequest`/`SyncResponse` round-trip via bincode, the wire codec used by
/// the sync protocol. A serialization regression would silently break IBD.
#[test]
fn test_sync_messages_roundtrip() {
    let req = SyncRequest::GetBlocks {
        start_height: 1,
        count: 100,
    };
    let bytes = bincode::serialize(&req).unwrap();
    let decoded: SyncRequest = bincode::deserialize(&bytes).unwrap();
    assert!(matches!(
        decoded,
        SyncRequest::GetBlocks {
            start_height: 1,
            count: 100
        }
    ));

    let resp = SyncResponse::Status {
        height: 95,
        tip_hash: [9u8; 32],
    };
    let bytes = bincode::serialize(&resp).unwrap();
    let decoded: SyncResponse = bincode::deserialize(&bytes).unwrap();
    assert!(matches!(
        decoded,
        SyncResponse::Status {
            height: 95,
            tip_hash
        } if tip_hash == [9u8; 32]
    ));
}

/// Apply a served `GetBlocks` batch exactly as the node event loop does in
/// `commands/run.rs` after the #641 fix: snapshot the committed height, skip
/// blocks at or below it (overlap tolerance), and apply only the novel tail.
///
/// Returns the number of blocks actually applied. A pure-overlap batch (every
/// block already committed) applies zero and must NOT be reported as a failure
/// — pre-#641 the requester hard-failed on the first already-known block
/// ("Expected height N, got N-k") and entered a retry loop.
fn apply_batch_with_overlap_skip(
    sync_manager: &mut ChainSyncManager,
    node: &Ledger,
    peer: PeerId,
    response: SyncResponse,
) -> usize {
    let SyncResponse::Blocks { blocks, has_more } = response else {
        panic!("expected Blocks response");
    };
    let Some(SyncAction::AddBlocks(blocks)) = sync_manager.on_blocks(&peer, blocks, has_more)
    else {
        return 0;
    };

    let local_before = node.get_chain_state().unwrap().height;
    let mut applied = 0;
    for block in &blocks {
        // Overlap: already committed. Skip rather than fail.
        if block.height() <= local_before {
            continue;
        }
        node.add_block(block).unwrap_or_else(|e| {
            panic!(
                "unexpected apply failure for novel block {}: {}",
                block.height(),
                e
            )
        });
        applied += 1;
    }
    if applied > 0 {
        sync_manager.on_blocks_added(node.get_chain_state().unwrap().height);
    }
    applied
}

/// Regression test for #641: a fresh node bootstrapping near a batch boundary
/// must tolerate a peer re-serving an *overlapping* batch (blocks it has
/// already committed) instead of hard-failing the whole batch and locking into
/// a retry loop.
///
/// This reproduces the exact live-testnet failure sequence: after batch
/// `[1..100]` is applied, a duplicate/stale response for a range the node
/// already has arrives. Pre-fix, the first already-known block failed the
/// ledger's sequential-height check ("Expected height 101, got 1"), the batch
/// aborted, and the node entered `Failed` and looped. Post-fix, the overlap is
/// skipped, no failure is raised, and the node completes sync to tip.
#[test]
#[serial]
fn test_duplicate_batch_response_tolerated() {
    // 145 mirrors the live-testnet repro: tip just past a 100-block batch
    // boundary, so the final batch overlaps the boundary.
    let target_height = 145;

    // --- Source node: chain to 145 ---
    let source_dir = TempDir::new().unwrap();
    let source = Ledger::open(source_dir.path()).unwrap();
    source.set_difficulty(TRIVIAL_DIFFICULTY).unwrap();
    let minter = create_test_wallet().public_address();
    build_chain_to_height(&source, &minter, target_height);
    let source_state = source.get_chain_state().unwrap();
    assert_eq!(source_state.height, target_height);

    // --- Fresh node at genesis ---
    let node_dir = TempDir::new().unwrap();
    let node = Ledger::open(node_dir.path()).unwrap();
    node.set_difficulty(TRIVIAL_DIFFICULTY).unwrap();

    let mut sync_manager = ChainSyncManager::new(0);
    let peer = PeerId::random();

    // Discovery -> Downloading (peer far ahead).
    sync_manager.on_status(peer, source_state.height, source_state.tip_hash);
    assert!(matches!(
        sync_manager.state(),
        SyncState::Downloading { .. }
    ));

    // --- Batch 1 [1..100] applied normally ---
    let action = sync_manager
        .tick(&[peer])
        .expect("should request first batch");
    let SyncAction::RequestBlocks {
        start_height,
        count,
        ..
    } = action
    else {
        panic!("expected RequestBlocks, got {action:?}");
    };
    assert_eq!(start_height, 1, "first request anchored at genesis + 1");
    sync_manager.on_request_sent(peer);
    let applied = apply_batch_with_overlap_skip(
        &mut sync_manager,
        &node,
        peer,
        serve_get_blocks(&source, start_height, count),
    );
    assert_eq!(applied, 100, "first batch applies 100 novel blocks");
    assert_eq!(node.get_chain_state().unwrap().height, 100);

    // --- The #641 trigger: a DUPLICATE stale response for [1..100] arrives ---
    // (In production this is the second, redundant request the pre-fix
    // fixed-interval tick issued while the first response was still in flight.)
    sync_manager.set_local_height(node.get_chain_state().unwrap().height);
    let dup_applied = apply_batch_with_overlap_skip(
        &mut sync_manager,
        &node,
        peer,
        serve_get_blocks(&source, 1, 100),
    );
    assert_eq!(
        dup_applied, 0,
        "a fully-overlapping batch must apply zero blocks"
    );
    assert!(
        !matches!(sync_manager.state(), SyncState::Failed { .. }),
        "an overlapping batch must NOT push the sync manager into Failed (the #641 bug)"
    );
    assert_eq!(
        node.get_chain_state().unwrap().height,
        100,
        "the overlap must not corrupt or regress committed height"
    );

    // --- Finish sync to tip; the state machine must still make progress ---
    let iterations = run_catchup(&mut sync_manager, &node, &source, peer);

    let node_state = node.get_chain_state().unwrap();
    assert_eq!(
        node_state.height, target_height,
        "node must reach the source tip despite the overlapping batch"
    );
    assert_eq!(
        node_state.tip_hash, source_state.tip_hash,
        "node tip hash must match the source after tolerating the overlap"
    );
    assert!(
        sync_manager.is_synced(),
        "sync manager must report Synced, not loop in Failed"
    );

    // --- Final overlap: a late duplicate of the tail [101..145] after we are ---
    // already at the tip (the curator's step-5 "Response B" scenario). It must
    // be tolerated with zero applied and no failure.
    let late_dup = apply_batch_with_overlap_skip(
        &mut sync_manager,
        &node,
        peer,
        serve_get_blocks(&source, 101, 100),
    );
    assert_eq!(late_dup, 0, "late duplicate tail applies nothing");
    assert!(
        sync_manager.is_synced(),
        "a late duplicate response must not knock a synced node out of Synced"
    );

    // The whole exchange must have converged quickly — a regression that
    // re-requests the same range would blow past this bound.
    assert!(
        iterations < 50,
        "catch-up after overlap took too many iterations: {iterations}"
    );
}
