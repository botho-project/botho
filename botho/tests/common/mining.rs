// Copyright (c) 2024 Botho Foundation
//
//! Mining utilities for test networks.

use std::{thread, time::Duration};

use bth_account_keys::PublicAddress;

use botho::{block::MintingTx, transaction::Transaction};

use crate::common::{TestNetwork, INITIAL_BLOCK_REWARD, TEST_RING_SIZE, TRIVIAL_DIFFICULTY};

/// Create a mock minting transaction with trivial PoW for fast testing.
///
/// Uses a very high difficulty target so that valid nonces are found quickly.
pub fn create_mock_minting_tx(
    height: u64,
    reward: u64,
    minter_address: &PublicAddress,
    prev_block_hash: [u8; 32],
) -> MintingTx {
    let mut minting_tx = MintingTx::new(
        height,
        reward,
        minter_address,
        prev_block_hash,
        TRIVIAL_DIFFICULTY,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    );

    // Find a valid nonce (should be fast with trivial difficulty).
    //
    // At `TRIVIAL_DIFFICULTY = 0x00FF_FFFF_FFFF_FFFF`, 1/256 nonces solve,
    // so 100k attempts have effectively zero false-failure risk while
    // still completing in milliseconds.
    for nonce in 0..100_000 {
        minting_tx.nonce = nonce;
        if minting_tx.verify_pow() {
            break;
        }
    }

    minting_tx
}

/// Mine a single block with the specified miner receiving the reward.
///
/// This broadcasts a minting transaction and waits for consensus to include it
/// in a block. The miner_idx specifies which node's wallet receives the
/// coinbase.
pub fn mine_block(network: &TestNetwork, miner_idx: usize) {
    let miner_wallet = &network.wallets[miner_idx];
    let miner_address = miner_wallet.default_address();

    let node = network.get_node(0);
    let state = node.chain_state();
    let prev_block = node.get_tip();
    let prev_hash = prev_block.hash();
    let height = state.height + 1;
    drop(node);

    // Clear any previous pending minting txs to avoid conflicts
    network.pending_minting_txs.lock().unwrap().clear();

    let minting_tx =
        create_mock_minting_tx(height, INITIAL_BLOCK_REWARD, &miner_address, prev_hash);
    network.broadcast_minting_tx(minting_tx);

    if !network.wait_for_height(height, Duration::from_secs(30)) {
        panic!("Timeout waiting for block {}", height);
    }

    // Small delay for state settlement across nodes
    thread::sleep(Duration::from_millis(150));
}

/// Mine a block with a custom reward amount.
pub fn mine_block_with_reward(network: &TestNetwork, miner_idx: usize, reward: u64) {
    let miner_wallet = &network.wallets[miner_idx];
    let miner_address = miner_wallet.default_address();

    let node = network.get_node(0);
    let state = node.chain_state();
    let prev_block = node.get_tip();
    let prev_hash = prev_block.hash();
    let height = state.height + 1;
    drop(node);

    network.pending_minting_txs.lock().unwrap().clear();

    let minting_tx = create_mock_minting_tx(height, reward, &miner_address, prev_hash);
    network.broadcast_minting_tx(minting_tx);

    if !network.wait_for_height(height, Duration::from_secs(30)) {
        panic!("Timeout waiting for block {}", height);
    }

    thread::sleep(Duration::from_millis(150));
}

/// Broadcast a transaction and mine until it is confirmed on-chain.
///
/// Confirmation is detected by the transaction's first key image becoming
/// spent on node 0 (recorded only when the tx is included in an applied
/// block). Block production is retried a few times to absorb consensus and
/// propagation races — without this, a freshly broadcast tx can miss the
/// block that `mine_block` produces, leaving the test non-deterministic.
///
/// Returns true if the transaction was confirmed within the retry budget.
pub fn confirm_transaction(network: &TestNetwork, tx: &Transaction, miner_idx: usize) -> bool {
    let key_image = tx.inputs.clsag()[0].key_image;

    let is_confirmed = |network: &TestNetwork| -> bool {
        let node = network.get_node(0);
        let spent = node
            .ledger
            .read()
            .unwrap()
            .is_key_image_spent(&key_image)
            .ok()
            .flatten()
            .is_some();
        drop(node);
        spent
    };

    if is_confirmed(network) {
        return true;
    }

    // Re-broadcast the tx each round, immediately before mining. The
    // simulated SCP loop can externalize a slot containing only the regular
    // tx (no minting tx); such a slot does not apply, and the node keeps the
    // tx pending — but re-broadcasting guarantees it is present alongside the
    // fresh minting tx that `mine_block` injects, so they share a block.
    for _ in 0..8 {
        network.broadcast_transaction(tx.clone());
        mine_block(network, miner_idx);
        if is_confirmed(network) {
            return true;
        }
    }
    false
}

/// Pre-mine blocks to ensure enough UTXOs exist for decoy selection.
///
/// CLSAG ring signatures need at least TEST_RING_SIZE members per input.
/// For multi-input transactions, we need extra UTXOs since the real inputs
/// are excluded from the decoy pool.
///
/// # Arguments
///
/// * `network` - The test network
/// * `extra_inputs` - Number of additional inputs that will be spent (excluded
///   from decoys)
pub fn ensure_decoy_availability(network: &TestNetwork, extra_inputs: usize) {
    let needed_blocks = TEST_RING_SIZE + extra_inputs;
    let node = network.get_node(0);
    let current_height = node.chain_state().height;
    drop(node);

    if current_height < needed_blocks as u64 {
        let blocks_to_mine = needed_blocks - current_height as usize;
        println!(
            "  Pre-mining {} blocks for decoy availability...",
            blocks_to_mine
        );
        for i in 0..blocks_to_mine {
            mine_block(network, i % network.config.num_nodes);
        }
    }
}
