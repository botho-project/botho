//! #1535: a full LMDB map surfaces from the production commit path as a
//! typed, classifiable local storage fault, not an opaque string.

use super::*;
use crate::{block::Block, ledger::LedgerError};

/// 1 MiB map: fills within a few hundred blocks (as `ledger::store`'s
/// `map_full_tests`).
const SMALL_MAP: usize = 1024 * 1024;

fn controller_for(ledger: &Ledger) -> EmissionController {
    let state = ledger.get_chain_state().unwrap();
    EmissionController::from_chain_state(
        state.difficulty,
        state.total_mined,
        state.total_fees_burned,
        state.total_tx,
        state.epoch_tx,
        state.epoch_emission,
        state.epoch_burns,
        state.current_reward,
    )
}

/// Next empty trivial-PoW, V2-sealed block on the current tip.
fn next_block(ledger: &SharedLedger, minter: &bth_account_keys::PublicAddress) -> Block {
    let state = ledger.read().unwrap().get_chain_state().unwrap();
    let mut block = Block::new_template_with_txs(
        &Block::genesis(),
        minter,
        state.difficulty,
        calculate_block_reward(state.height + 1, state.total_mined),
        vec![],
    );
    block.header.height = state.height + 1;
    block.header.prev_block_hash = state.tip_hash;
    block.header.timestamp = block.header.timestamp.max(state.tip_timestamp);
    block.minting_tx.block_height = block.header.height;
    block.minting_tx.prev_block_hash = state.tip_hash;
    block.minting_tx.timestamp = block.header.timestamp;
    while !block.header.is_valid_pow() {
        block.header.nonce += 1;
    }
    block.minting_tx.nonce = block.header.nonce;
    ledger.read().unwrap().seal_block(block).unwrap()
}

/// The `LedgerError` survives `commit_block_locked`'s conversion to
/// `anyhow` (#1535), so the node loop can classify a full map as a local
/// storage fault, while the logged message is unchanged.
#[test]
fn map_full_error_type_survives_commit_block_locked() {
    let dir = tempfile::tempdir().unwrap();
    let ledger = Ledger::open_with_map_size(dir.path(), SMALL_MAP).unwrap();
    ledger.set_difficulty(u64::MAX).unwrap();
    let ledger: SharedLedger = Arc::new(RwLock::new(ledger));
    let wallet = Wallet::from_mnemonic(
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
    )
    .unwrap();
    let minter = wallet.default_address();

    // An invalid block (wrong height) is a rejection, never a storage fault.
    let mut controller = controller_for(&ledger.read().unwrap());
    let mut bad = next_block(&ledger, &minter);
    bad.header.height += 5;
    let rejected = commit_block_locked(&ledger, &mut controller, &bad).unwrap_err();
    assert!(!is_ledger_map_full(&rejected), "{rejected:#}");
    assert!(rejected.downcast_ref::<LedgerError>().is_some());

    // Fill the map through the plain ledger path: it keeps the trivial
    // difficulty, whereas the emission controller in `commit_block_locked`
    // would retarget it and make each further block slow to mine.
    let refused = loop {
        let height = ledger.read().unwrap().get_chain_state().unwrap().height;
        assert!(height < 100_000, "map never filled");
        let block = next_block(&ledger, &minter);
        if let Err(error) = ledger.read().unwrap().add_block(&block) {
            assert!(error.is_map_full(), "{error:?}");
            break block;
        }
    };

    // The refused block, retried through the node's commit path, fails the
    // same way, and the anyhow error still carries the typed LedgerError.
    let mut controller = controller_for(&ledger.read().unwrap());
    let error = commit_block_locked(&ledger, &mut controller, &refused).unwrap_err();

    assert!(is_ledger_map_full(&error), "{error:#}");
    match error.downcast_ref::<LedgerError>() {
        Some(e @ LedgerError::Database(message)) => {
            assert!(e.is_map_full() && message.contains("MDB_MAP_FULL"))
        }
        other => panic!("expected LedgerError::Database, got {other:?}"),
    }
    // `{}` keeps the pre-#1535 message, details included.
    let shown = error.to_string();
    assert!(
        shown.starts_with("Failed to add network block: Database error:")
            && shown.contains("MDB_MAP_FULL"),
        "{shown}"
    );
    assert!(ledger.read().unwrap().storage_usage().map_full);
}
