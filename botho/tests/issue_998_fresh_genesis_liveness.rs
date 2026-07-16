// Copyright (c) 2024 Botho Foundation
//
//! Issue #998 — fresh 6.0.0 genesis liveness.
//!
//! A fresh protocol-6.0.0 genesis was reported to externalize a few blocks and
//! then wedge permanently (the minter re-submits the next height's coinbase
//! forever). This test drives the single-node BLOCK-APPLY path — the one the
//! 6.0.0 batch changed for every block: a hybrid ML-KEM coinbase output
//! (#968/#969), its fold into `MintingTx::hash()`, the coinbase UTXO write, and
//! the per-block lottery/cluster-wealth accounting — across MANY successive
//! blocks on a fresh genesis and asserts the height advances each time.
//!
//! No previous test exercised this: the e2e consensus harness uses a stub SCP
//! validity callback and a classical (ciphertext-less) coinbase, so it never
//! ran the 6.0.0 coinbase through `add_block` repeatedly on a fresh chain.

use std::time::SystemTime;

use serial_test::serial;
use tempfile::TempDir;

use botho::{
    block::{Block, BlockHeader, BlockLotterySummary, MintingTx},
    ledger::Ledger,
    transaction::PICOCREDITS_PER_CREDIT,
};
use bth_account_keys::PublicAddress;
use botho_wallet::WalletKeys;

/// Trivial PoW difficulty for instant mining; must equal the chain's pinned
/// difficulty because block acceptance enforces `header.difficulty ==
/// chain.difficulty` (audit cycle 6, C1).
const TRIVIAL_DIFFICULTY: u64 = u64::MAX;

/// Block reward for testing (50 BTH), matching the fresh-genesis emission.
const TEST_BLOCK_REWARD: u64 = 50 * PICOCREDITS_PER_CREDIT;

fn create_test_ledger() -> (TempDir, Ledger) {
    let temp_dir = TempDir::new().expect("temp dir");
    let ledger = Ledger::open(&temp_dir.path().join("ledger")).expect("open ledger");
    ledger.set_difficulty(TRIVIAL_DIFFICULTY).unwrap();
    (temp_dir, ledger)
}

/// A deterministic **post-quantum** (hybrid ML-KEM) address, so the coinbase
/// carries a real 1,088-byte ML-KEM ciphertext exactly like the live 6.0.0
/// minter (`coinbase_stealth_fields`), not a classical ciphertext-less output.
fn pq_minter_address() -> PublicAddress {
    let mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon \
                    abandon abandon abandon abandon abandon abandon abandon abandon \
                    abandon abandon abandon abandon abandon abandon abandon art";
    WalletKeys::from_mnemonic(mnemonic)
        .expect("wallet")
        .pq_public_address()
}

/// Build a valid minting-only block for `height` on top of `prev_hash`, mining a
/// trivial-difficulty PoW. The coinbase is a hybrid ML-KEM output.
fn mine_minting_block(height: u64, prev_hash: [u8; 32], minter: &PublicAddress) -> Block {
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let mut minting_tx = MintingTx::new(
        height,
        TEST_BLOCK_REWARD,
        minter,
        prev_hash,
        TRIVIAL_DIFFICULTY,
        timestamp,
    );
    // Sanity: the 6.0.0 coinbase must carry the hybrid ML-KEM ciphertext.
    assert!(
        minting_tx.kem_ciphertext.is_some(),
        "pq coinbase must attach an ML-KEM ciphertext (6.0.0)"
    );

    for nonce in 0..200_000u64 {
        minting_tx.nonce = nonce;
        if minting_tx.verify_pow() {
            break;
        }
    }
    assert!(minting_tx.verify_pow(), "failed to find trivial PoW");

    Block {
        header: BlockHeader {
            version: 1,
            prev_block_hash: prev_hash,
            tx_root: [0u8; 32],
            timestamp: minting_tx.timestamp,
            height: minting_tx.block_height,
            difficulty: minting_tx.difficulty,
            nonce: minting_tx.nonce,
            minter_view_key: minting_tx.minter_view_key,
            minter_spend_key: minting_tx.minter_spend_key,
        },
        minting_tx,
        transactions: vec![],
        lottery_outputs: vec![],
        lottery_summary: BlockLotterySummary::default(),
    }
}

/// REGRESSION (issue #998): a fresh 6.0.0 genesis must apply many successive
/// hybrid-coinbase blocks with the height advancing every block — never wedge.
#[test]
#[serial]
fn fresh_6_0_0_genesis_advances_past_height_6() {
    let (_tmp, ledger) = create_test_ledger();
    let minter = pq_minter_address();

    // Mint well past the reported wedge point (height ~5).
    const N: u64 = 12;
    for height in 1..=N {
        let prev_hash = ledger.get_tip().expect("tip").hash();
        let block = mine_minting_block(height, prev_hash, &minter);
        ledger
            .add_block(&block)
            .unwrap_or_else(|e| panic!("WEDGE at height {height}: add_block failed: {e}"));

        let state = ledger.get_chain_state().expect("chain state");
        assert_eq!(
            state.height, height,
            "chain height must advance to {height} after applying block {height}"
        );
    }

    let final_state = ledger.get_chain_state().expect("chain state");
    assert_eq!(final_state.height, N, "final height must be {N}");
    assert!(
        final_state.height > 6,
        "chain must advance past the reported wedge height"
    );
}
