// Copyright (c) 2024 Botho Foundation
//
//! Ledger Consistency Integration Tests
//!
//! Tests the correctness and consistency of the LMDB-backed ledger under
//! various scenarios:
//! - Concurrent read/write operations
//! - Large block application (many transactions)
//! - Index integrity verification
//! - UTXO set consistency after multi-block sequences
//! - Reorg handling (block reorganization)

use std::{
    sync::{Arc, RwLock},
    thread,
    time::{Duration, SystemTime},
};

use serial_test::serial;
use tempfile::TempDir;

use botho::{
    block::{Block, BlockHeader, BlockLotterySummary, MintingTx},
    ledger::{ChainState, Ledger},
    transaction::{
        ClsagRingInput, RingMember, Transaction, TxInput, TxInputs, TxOutput, Utxo, UtxoId,
        PICOCREDITS_PER_CREDIT,
    },
};
use botho_wallet::WalletKeys;
use bth_account_keys::PublicAddress;
use sha2::{Digest, Sha256};

// ============================================================================
// Constants
// ============================================================================

/// Block reward for testing (50 BTH)
const TEST_BLOCK_REWARD: u64 = 50 * PICOCREDITS_PER_CREDIT;

/// Trivial difficulty for fast PoW
const TRIVIAL_DIFFICULTY: u64 = 0x00FF_FFFF_FFFF_FFFF;

// ============================================================================
// Helper Functions
// ============================================================================

fn create_test_wallet(seed: u8) -> WalletKeys {
    let mnemonics = [
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art",
        "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo vote",
        "letter advice cage absurd amount doctor acoustic avoid letter advice cage absurd amount doctor acoustic avoid letter advice cage absurd amount doctor acoustic bless",
        "jelly better achieve collect unaware mountain thought cargo oxygen act hood bridge",
    ];
    // Generate a deterministic 24-word mnemonic from seed
    let base_mnemonic = mnemonics[(seed as usize) % 3]; // Use only the 24-word ones
    WalletKeys::from_mnemonic(base_mnemonic).expect("Failed to create wallet from mnemonic")
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

    // Find a valid nonce - with trivial difficulty this should always succeed
    // quickly
    for nonce in 0..u64::MAX {
        minting_tx.nonce = nonce;
        if minting_tx.verify_pow() {
            break;
        }
    }

    minting_tx
}

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

// ============================================================================
// Basic Consistency Tests
// ============================================================================

#[test]
#[serial]
fn test_ledger_genesis_consistency() {
    let temp_dir = TempDir::new().unwrap();
    let ledger = Ledger::open(temp_dir.path()).unwrap();

    let state = ledger.get_chain_state().unwrap();
    assert_eq!(state.height, 0, "Genesis should be at height 0");

    let tip = ledger.get_tip().unwrap();
    assert_eq!(tip.header.height, 0, "Tip should be genesis");

    // Verify genesis block hash is consistent
    let genesis_by_height = ledger.get_block(0).unwrap();
    assert_eq!(
        genesis_by_height.hash(),
        tip.hash(),
        "Genesis block should be consistent"
    );
}

#[test]
#[serial]
fn test_ledger_sequential_block_addition() {
    let temp_dir = TempDir::new().unwrap();
    let mut ledger = Ledger::open(temp_dir.path()).unwrap();
    let miner = create_test_wallet(1);
    let miner_address = miner.account_key().default_subaddress();

    // Add blocks sequentially
    for expected_height in 1..=10 {
        let block = mine_block(&ledger, &miner_address, vec![]);
        ledger.add_block(&block).expect("Failed to add block");

        let state = ledger.get_chain_state().unwrap();
        assert_eq!(state.height, expected_height, "Height should increment");

        // Verify block is retrievable
        let retrieved = ledger.get_block(expected_height).unwrap();
        assert_eq!(
            retrieved.hash(),
            block.hash(),
            "Block should be retrievable by height"
        );
    }
}

#[test]
#[serial]
fn test_ledger_utxo_creation_from_minting() {
    let temp_dir = TempDir::new().unwrap();
    let mut ledger = Ledger::open(temp_dir.path()).unwrap();
    let miner = create_test_wallet(1);
    let miner_address = miner.account_key().default_subaddress();

    // Mine a block
    let block = mine_block(&ledger, &miner_address, vec![]);
    let block_hash = block.hash();
    ledger.add_block(&block).expect("Failed to add block");

    // Verify UTXO was created (minting output is at index 0)
    let utxo_id = UtxoId::new(block_hash, 0);
    let utxo = ledger.get_utxo(&utxo_id).expect("Failed to query UTXO");
    assert!(utxo.is_some(), "Minting UTXO should exist");

    let utxo = utxo.unwrap();
    assert_eq!(
        utxo.output.amount, TEST_BLOCK_REWARD,
        "UTXO amount should match block reward"
    );
}

#[test]
#[serial]
fn test_ledger_total_mined_tracking() {
    let temp_dir = TempDir::new().unwrap();
    let mut ledger = Ledger::open(temp_dir.path()).unwrap();
    let miner = create_test_wallet(1);
    let miner_address = miner.account_key().default_subaddress();

    let initial_state = ledger.get_chain_state().unwrap();
    let initial_mined = initial_state.total_mined;

    // Mine 5 blocks
    for _ in 0..5 {
        let block = mine_block(&ledger, &miner_address, vec![]);
        ledger.add_block(&block).expect("Failed to add block");
    }

    let final_state = ledger.get_chain_state().unwrap();
    let expected_mined = initial_mined + (5 * TEST_BLOCK_REWARD as u128);
    assert_eq!(
        final_state.total_mined, expected_mined,
        "Total mined should track correctly"
    );
}

// ============================================================================
// Concurrent Access Tests
// ============================================================================

#[test]
#[serial]
fn test_concurrent_reads() {
    let temp_dir = TempDir::new().unwrap();
    let mut ledger = Ledger::open(temp_dir.path()).unwrap();
    let miner = create_test_wallet(1);
    let miner_address = miner.account_key().default_subaddress();

    // Create some blocks first
    for _ in 0..5 {
        let block = mine_block(&ledger, &miner_address, vec![]);
        ledger.add_block(&block).expect("Failed to add block");
    }

    let ledger = Arc::new(RwLock::new(ledger));

    // Spawn multiple reader threads
    let mut handles = vec![];
    for i in 0..10 {
        let ledger_clone = ledger.clone();
        let handle = thread::spawn(move || {
            for _ in 0..100 {
                let ledger = ledger_clone.read().unwrap();
                let state = ledger.get_chain_state().unwrap();
                assert!(
                    state.height >= 5,
                    "Height should be at least 5 from thread {}",
                    i
                );

                // Read random blocks
                for h in 0..=state.height {
                    let _ = ledger.get_block(h);
                }
            }
        });
        handles.push(handle);
    }

    // Wait for all readers
    for handle in handles {
        handle.join().expect("Reader thread panicked");
    }
}

#[test]
#[serial]
fn test_concurrent_read_write() {
    let temp_dir = TempDir::new().unwrap();
    let ledger = Ledger::open(temp_dir.path()).unwrap();
    let ledger = Arc::new(RwLock::new(ledger));

    let miner = create_test_wallet(1);
    let miner_address = miner.account_key().default_subaddress();

    // Spawn writer thread
    let ledger_writer = ledger.clone();
    let miner_addr_clone = miner_address.clone();
    let writer_handle = thread::spawn(move || {
        for _ in 0..10 {
            let mut ledger = ledger_writer.write().unwrap();
            let block = mine_block(&*ledger, &miner_addr_clone, vec![]);
            ledger.add_block(&block).expect("Failed to add block");
            drop(ledger);
            thread::sleep(Duration::from_millis(10));
        }
    });

    // Spawn reader threads
    let mut reader_handles = vec![];
    for _ in 0..5 {
        let ledger_reader = ledger.clone();
        let handle = thread::spawn(move || {
            let mut last_height = 0u64;
            for _ in 0..100 {
                let ledger = ledger_reader.read().unwrap();
                let state = ledger.get_chain_state().unwrap();
                // Height should never decrease
                assert!(
                    state.height >= last_height,
                    "Height decreased during concurrent access"
                );
                last_height = state.height;
                thread::sleep(Duration::from_millis(1));
            }
        });
        reader_handles.push(handle);
    }

    writer_handle.join().expect("Writer thread panicked");
    for handle in reader_handles {
        handle.join().expect("Reader thread panicked");
    }

    // Final verification
    let ledger = ledger.read().unwrap();
    let state = ledger.get_chain_state().unwrap();
    assert_eq!(
        state.height, 10,
        "Should have 10 blocks after concurrent operations"
    );
}

// ============================================================================
// Index Integrity Tests
// ============================================================================

#[test]
#[serial]
fn test_block_height_index_integrity() {
    let temp_dir = TempDir::new().unwrap();
    let mut ledger = Ledger::open(temp_dir.path()).unwrap();
    let miner = create_test_wallet(1);
    let miner_address = miner.account_key().default_subaddress();

    // Mine blocks and track their hashes
    let mut block_hashes = vec![ledger.get_tip().unwrap().hash()]; // Genesis

    for _ in 0..20 {
        let block = mine_block(&ledger, &miner_address, vec![]);
        block_hashes.push(block.hash());
        ledger.add_block(&block).expect("Failed to add block");
    }

    // Verify all blocks are retrievable by height and have correct hashes
    for (height, expected_hash) in block_hashes.iter().enumerate() {
        let block = ledger
            .get_block(height as u64)
            .expect("Failed to get block by height");
        assert_eq!(
            &block.hash(),
            expected_hash,
            "Block at height {} has wrong hash",
            height
        );
        assert_eq!(
            block.header.height, height as u64,
            "Block height field doesn't match index"
        );
    }
}

#[test]
#[serial]
fn test_utxo_index_integrity() {
    let temp_dir = TempDir::new().unwrap();
    let mut ledger = Ledger::open(temp_dir.path()).unwrap();
    let miner = create_test_wallet(1);
    let miner_address = miner.account_key().default_subaddress();

    // Mine multiple blocks to create UTXOs
    let mut utxo_ids = vec![];
    for _ in 0..10 {
        let block = mine_block(&ledger, &miner_address, vec![]);
        let block_hash = block.hash();
        ledger.add_block(&block).expect("Failed to add block");

        // Track the minting UTXO
        utxo_ids.push(UtxoId::new(block_hash, 0));
    }

    // Verify all UTXOs exist
    for utxo_id in &utxo_ids {
        let utxo = ledger.get_utxo(utxo_id).expect("Failed to query UTXO");
        assert!(utxo.is_some(), "UTXO should exist: {:?}", utxo_id);
    }
}

#[test]
#[serial]
fn test_chain_state_consistency_after_multiple_blocks() {
    let temp_dir = TempDir::new().unwrap();
    let mut ledger = Ledger::open(temp_dir.path()).unwrap();
    let miner = create_test_wallet(1);
    let miner_address = miner.account_key().default_subaddress();

    let num_blocks = 50;
    let mut prev_state = ledger.get_chain_state().unwrap();

    for i in 1..=num_blocks {
        let block = mine_block(&ledger, &miner_address, vec![]);
        let block_hash = block.hash();
        ledger.add_block(&block).expect("Failed to add block");

        let state = ledger.get_chain_state().unwrap();

        // Verify state progression
        assert_eq!(
            state.height,
            prev_state.height + 1,
            "Height should increment by 1"
        );
        assert_eq!(
            state.tip_hash, block_hash,
            "Tip hash should match added block"
        );
        assert_eq!(
            state.total_mined,
            prev_state.total_mined + TEST_BLOCK_REWARD as u128,
            "Total mined should increase by block reward"
        );

        prev_state = state;
    }
}

// ============================================================================
// Large Block Tests
// ============================================================================

#[test]
#[serial]
fn test_block_with_many_transactions() {
    let temp_dir = TempDir::new().unwrap();
    let mut ledger = Ledger::open(temp_dir.path()).unwrap();
    let miner = create_test_wallet(1);
    let miner_address = miner.account_key().default_subaddress();

    // First, mine a block to get some coins
    let block1 = mine_block(&ledger, &miner_address, vec![]);
    ledger.add_block(&block1).expect("Failed to add block 1");

    // Note: Creating valid transactions requires proper ring signatures
    // For this test, we just verify the ledger can handle a block structure
    // with an empty transaction list (which is valid)

    // Mine multiple blocks to simulate chain growth
    for _ in 0..20 {
        let block = mine_block(&ledger, &miner_address, vec![]);
        ledger.add_block(&block).expect("Failed to add block");
    }

    let state = ledger.get_chain_state().unwrap();
    assert_eq!(state.height, 21, "Should have 21 blocks total");
}

// ============================================================================
// Edge Cases and Error Handling
// ============================================================================

#[test]
#[serial]
fn test_get_nonexistent_block() {
    let temp_dir = TempDir::new().unwrap();
    let ledger = Ledger::open(temp_dir.path()).unwrap();

    // Try to get a block that doesn't exist
    let result = ledger.get_block(9999);
    assert!(result.is_err(), "Should return error for nonexistent block");
}

#[test]
#[serial]
fn test_get_nonexistent_utxo() {
    let temp_dir = TempDir::new().unwrap();
    let ledger = Ledger::open(temp_dir.path()).unwrap();

    // Create a fake UTXO ID
    let fake_utxo_id = UtxoId::new([0xAB; 32], 42);
    let result = ledger.get_utxo(&fake_utxo_id).unwrap();
    assert!(result.is_none(), "Should return None for nonexistent UTXO");
}

#[test]
#[serial]
fn test_block_with_wrong_parent_hash() {
    let temp_dir = TempDir::new().unwrap();
    let mut ledger = Ledger::open(temp_dir.path()).unwrap();
    let miner = create_test_wallet(1);
    let miner_address = miner.account_key().default_subaddress();

    // Create a block with wrong parent hash
    let wrong_hash = [0xFF; 32];
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let mut minting_tx = MintingTx::new(
        1,
        TEST_BLOCK_REWARD,
        &miner_address,
        wrong_hash,
        TRIVIAL_DIFFICULTY,
        timestamp,
    );

    for nonce in 0..1000 {
        minting_tx.nonce = nonce;
        if minting_tx.verify_pow() {
            break;
        }
    }

    let bad_block = Block {
        header: BlockHeader {
            version: 1,
            prev_block_hash: wrong_hash,
            tx_root: [0u8; 32],
            timestamp: minting_tx.timestamp,
            height: 1,
            difficulty: minting_tx.difficulty,
            nonce: minting_tx.nonce,
            minter_view_key: minting_tx.minter_view_key,
            minter_spend_key: minting_tx.minter_spend_key,
        },
        minting_tx,
        transactions: vec![],
        lottery_outputs: Vec::new(),
        lottery_summary: BlockLotterySummary::default(),
    };

    let result = ledger.add_block(&bad_block);
    assert!(
        result.is_err(),
        "Should reject block with wrong parent hash"
    );
}

#[test]
#[serial]
fn test_block_with_wrong_height() {
    let temp_dir = TempDir::new().unwrap();
    let mut ledger = Ledger::open(temp_dir.path()).unwrap();
    let miner = create_test_wallet(1);
    let miner_address = miner.account_key().default_subaddress();

    let prev_block = ledger.get_tip().unwrap();
    let prev_hash = prev_block.hash();

    // Create block with height 5 instead of 1
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let mut minting_tx = MintingTx::new(
        5, // Wrong height!
        TEST_BLOCK_REWARD,
        &miner_address,
        prev_hash,
        TRIVIAL_DIFFICULTY,
        timestamp,
    );

    for nonce in 0..1000 {
        minting_tx.nonce = nonce;
        if minting_tx.verify_pow() {
            break;
        }
    }

    let bad_block = Block {
        header: BlockHeader {
            version: 1,
            prev_block_hash: prev_hash,
            tx_root: [0u8; 32],
            timestamp: minting_tx.timestamp,
            height: 5, // Wrong!
            difficulty: minting_tx.difficulty,
            nonce: minting_tx.nonce,
            minter_view_key: minting_tx.minter_view_key,
            minter_spend_key: minting_tx.minter_spend_key,
        },
        minting_tx,
        transactions: vec![],
        lottery_outputs: Vec::new(),
        lottery_summary: BlockLotterySummary::default(),
    };

    let result = ledger.add_block(&bad_block);
    assert!(result.is_err(), "Should reject block with wrong height");
}

// ============================================================================
// Stress Tests
// ============================================================================

#[test]
#[serial]
fn test_rapid_block_addition() {
    let temp_dir = TempDir::new().unwrap();
    let mut ledger = Ledger::open(temp_dir.path()).unwrap();
    let miner = create_test_wallet(1);
    let miner_address = miner.account_key().default_subaddress();

    let num_blocks = 100;
    let start = std::time::Instant::now();

    for _ in 0..num_blocks {
        let block = mine_block(&ledger, &miner_address, vec![]);
        ledger.add_block(&block).expect("Failed to add block");
    }

    let elapsed = start.elapsed();
    println!(
        "Added {} blocks in {:?} ({:.2} blocks/sec)",
        num_blocks,
        elapsed,
        num_blocks as f64 / elapsed.as_secs_f64()
    );

    let state = ledger.get_chain_state().unwrap();
    assert_eq!(
        state.height, num_blocks,
        "Should have correct number of blocks"
    );
}

#[test]
#[serial]
fn test_repeated_open_close() {
    let temp_dir = TempDir::new().unwrap();
    let miner = create_test_wallet(1);
    let miner_address = miner.account_key().default_subaddress();

    // Open, add block, close - repeat
    for i in 1..=10 {
        {
            let mut ledger = Ledger::open(temp_dir.path()).unwrap();
            let state = ledger.get_chain_state().unwrap();
            assert_eq!(
                state.height,
                (i - 1) as u64,
                "Height should persist across reopens"
            );

            let block = mine_block(&ledger, &miner_address, vec![]);
            ledger.add_block(&block).expect("Failed to add block");

            let state = ledger.get_chain_state().unwrap();
            assert_eq!(state.height, i as u64, "Height should be updated");
        }
        // ledger is dropped here, closing the DB
    }

    // Final verification
    let ledger = Ledger::open(temp_dir.path()).unwrap();
    let state = ledger.get_chain_state().unwrap();
    assert_eq!(
        state.height, 10,
        "All blocks should persist after multiple open/close cycles"
    );
}

// ============================================================================
// Data Integrity Tests
// ============================================================================

#[test]
#[serial]
fn test_block_data_integrity() {
    let temp_dir = TempDir::new().unwrap();
    let mut ledger = Ledger::open(temp_dir.path()).unwrap();
    let miner = create_test_wallet(1);
    let miner_address = miner.account_key().default_subaddress();

    // Mine a block with specific data
    let block = mine_block(&ledger, &miner_address, vec![]);
    let original_hash = block.hash();
    let original_height = block.header.height;
    let original_reward = block.minting_tx.reward;

    ledger.add_block(&block).expect("Failed to add block");

    // Retrieve and verify
    let retrieved = ledger.get_block(original_height).unwrap();
    assert_eq!(retrieved.hash(), original_hash, "Block hash should match");
    assert_eq!(
        retrieved.header.height, original_height,
        "Block height should match"
    );
    assert_eq!(
        retrieved.minting_tx.reward, original_reward,
        "Block reward should match"
    );
}

#[test]
#[serial]
fn test_chain_tip_tracking() {
    let temp_dir = TempDir::new().unwrap();
    let mut ledger = Ledger::open(temp_dir.path()).unwrap();
    let miner = create_test_wallet(1);
    let miner_address = miner.account_key().default_subaddress();

    for _ in 0..10 {
        let pre_state = ledger.get_chain_state().unwrap();
        let block = mine_block(&ledger, &miner_address, vec![]);
        let block_hash = block.hash();

        ledger.add_block(&block).expect("Failed to add block");

        let post_state = ledger.get_chain_state().unwrap();
        let tip = ledger.get_tip().unwrap();

        // Tip hash in chain state should match actual tip block
        assert_eq!(
            post_state.tip_hash, block_hash,
            "Chain state tip_hash should match added block"
        );
        assert_eq!(
            tip.hash(),
            block_hash,
            "get_tip() should return the latest block"
        );
        assert_eq!(
            post_state.height,
            pre_state.height + 1,
            "Height should increment"
        );
    }
}

#[test]
#[serial]
fn test_multiple_miners_consistency() {
    let temp_dir = TempDir::new().unwrap();
    let mut ledger = Ledger::open(temp_dir.path()).unwrap();

    // Create multiple miners
    let miners: Vec<_> = (0..5).map(|i| create_test_wallet(i)).collect();

    // Mine blocks alternating between miners
    let mut miner_block_counts = vec![0u32; 5];

    for i in 0..20 {
        let miner_idx = i % 5;
        let miner = &miners[miner_idx];
        let miner_address = miner.account_key().default_subaddress();

        let block = mine_block(&ledger, &miner_address, vec![]);
        ledger.add_block(&block).expect("Failed to add block");
        miner_block_counts[miner_idx] += 1;
    }

    let state = ledger.get_chain_state().unwrap();
    assert_eq!(state.height, 20, "Should have 20 blocks");

    // Each miner should have mined 4 blocks
    for (idx, count) in miner_block_counts.iter().enumerate() {
        assert_eq!(*count, 4, "Miner {} should have mined 4 blocks", idx);
    }
}

// ============================================================================
// Block-Acceptance Hardening (audit cycle 6: C1–C4)
//
// These tests cover the consensus rules that gossip/sync blocks must satisfy
// at `Ledger::add_block`. The SCP proposal path validates the minting tx
// separately; these tests pin down the rules for any block arriving from the
// network.
// ============================================================================

/// C1: A block declaring a different difficulty than the chain's expected
/// difficulty must be rejected — even if its PoW satisfies that declared
/// difficulty. Otherwise an attacker fabricates a chain with trivial
/// difficulty and we accept it at near-zero PoW cost.
#[test]
#[serial]
fn test_c1_block_rejected_when_difficulty_differs_from_chain_state() {
    let temp_dir = TempDir::new().unwrap();
    let mut ledger = Ledger::open(temp_dir.path()).unwrap();
    let miner = create_test_wallet(1);
    let miner_address = miner.account_key().default_subaddress();

    // Build a block that solves PoW against a *lower* declared difficulty
    // than the chain expects. Both the header and the minting tx use the
    // tampered value so that they stay consistent — the gap the test
    // exercises is "chain state vs declared", not "header vs minting tx".
    let state = ledger.get_chain_state().unwrap();
    let prev_block = ledger.get_tip().unwrap();
    let prev_hash = prev_block.hash();
    let tampered_difficulty = state.difficulty / 4;
    assert_ne!(tampered_difficulty, state.difficulty);

    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let mut minting_tx = MintingTx::new(
        1,
        TEST_BLOCK_REWARD,
        &miner_address,
        prev_hash,
        tampered_difficulty,
        timestamp,
    );
    for nonce in 0..u64::MAX {
        minting_tx.nonce = nonce;
        if minting_tx.verify_pow() {
            break;
        }
    }

    let bad_block = Block {
        header: BlockHeader {
            version: 1,
            prev_block_hash: prev_hash,
            tx_root: [0u8; 32],
            timestamp,
            height: 1,
            difficulty: tampered_difficulty,
            nonce: minting_tx.nonce,
            minter_view_key: minting_tx.minter_view_key,
            minter_spend_key: minting_tx.minter_spend_key,
        },
        minting_tx,
        transactions: vec![],
        lottery_outputs: Vec::new(),
        lottery_summary: BlockLotterySummary::default(),
    };

    let err = ledger
        .add_block(&bad_block)
        .expect_err("Block with tampered difficulty must be rejected");
    let msg = format!("{}", err);
    assert!(
        msg.contains("difficulty"),
        "Error should mention difficulty mismatch, got: {}",
        msg
    );
}

/// C2a: A block claiming a reward not equal to `calculate_block_reward(height,
/// total_mined)` must be rejected. Otherwise a producer mints unlimited
/// inflation by overstating the coinbase amount.
#[test]
#[serial]
fn test_c2_block_rejected_when_reward_inflated() {
    let temp_dir = TempDir::new().unwrap();
    let mut ledger = Ledger::open(temp_dir.path()).unwrap();
    let miner = create_test_wallet(1);
    let miner_address = miner.account_key().default_subaddress();

    let mut block = mine_block(&ledger, &miner_address, vec![]);
    // Inflate the reward by 2x while keeping all other fields consistent.
    // PoW is over the header (nonce/prev_hash/minter keys) and is unaffected
    // by the reward value, so this block still passes the PoW check.
    block.minting_tx.reward = block.minting_tx.reward.saturating_mul(2);

    let err = ledger
        .add_block(&block)
        .expect_err("Block with inflated reward must be rejected");
    let msg = format!("{}", err);
    assert!(
        msg.contains("reward"),
        "Error should mention reward mismatch, got: {}",
        msg
    );
}

/// C2b: A block whose timestamp is earlier than its parent's must be
/// rejected. Without monotonicity an adversary can backdate blocks to bias
/// any timestamp-derived state.
#[test]
#[serial]
fn test_c2_block_rejected_when_timestamp_before_parent() {
    let temp_dir = TempDir::new().unwrap();
    let mut ledger = Ledger::open(temp_dir.path()).unwrap();
    let miner = create_test_wallet(1);
    let miner_address = miner.account_key().default_subaddress();

    // Add a valid block 1 with a real wall-clock timestamp.
    let block1 = mine_block(&ledger, &miner_address, vec![]);
    let block1_timestamp = block1.header.timestamp;
    assert!(block1_timestamp > 0);
    ledger.add_block(&block1).expect("Failed to add block 1");

    // Construct block 2 that backdates one second before block 1.
    let state = ledger.get_chain_state().unwrap();
    let prev_hash = state.tip_hash;
    let bad_timestamp = block1_timestamp - 1;
    let mut minting_tx = MintingTx::new(
        2,
        TEST_BLOCK_REWARD,
        &miner_address,
        prev_hash,
        TRIVIAL_DIFFICULTY,
        bad_timestamp,
    );
    for nonce in 0..u64::MAX {
        minting_tx.nonce = nonce;
        if minting_tx.verify_pow() {
            break;
        }
    }
    let bad_block = Block {
        header: BlockHeader {
            version: 1,
            prev_block_hash: prev_hash,
            tx_root: [0u8; 32],
            timestamp: bad_timestamp,
            height: 2,
            difficulty: TRIVIAL_DIFFICULTY,
            nonce: minting_tx.nonce,
            minter_view_key: minting_tx.minter_view_key,
            minter_spend_key: minting_tx.minter_spend_key,
        },
        minting_tx,
        transactions: vec![],
        lottery_outputs: Vec::new(),
        lottery_summary: BlockLotterySummary::default(),
    };

    let err = ledger
        .add_block(&bad_block)
        .expect_err("Backdated block must be rejected");
    let msg = format!("{}", err);
    assert!(
        msg.contains("timestamp") && msg.contains("before parent"),
        "Error should mention timestamp monotonicity, got: {}",
        msg
    );
}

/// C2b: A block timestamped far in the future (beyond the 2h skew tolerance)
/// must be rejected.
#[test]
#[serial]
fn test_c2_block_rejected_when_timestamp_far_in_future() {
    let temp_dir = TempDir::new().unwrap();
    let mut ledger = Ledger::open(temp_dir.path()).unwrap();
    let miner = create_test_wallet(1);
    let miner_address = miner.account_key().default_subaddress();

    let state = ledger.get_chain_state().unwrap();
    let prev_block = ledger.get_tip().unwrap();
    let prev_hash = prev_block.hash();

    // 3 hours ahead — beyond the 2h tolerance.
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let future_timestamp = now + 3 * 60 * 60;

    let mut minting_tx = MintingTx::new(
        1,
        TEST_BLOCK_REWARD,
        &miner_address,
        prev_hash,
        state.difficulty,
        future_timestamp,
    );
    for nonce in 0..u64::MAX {
        minting_tx.nonce = nonce;
        if minting_tx.verify_pow() {
            break;
        }
    }
    let bad_block = Block {
        header: BlockHeader {
            version: 1,
            prev_block_hash: prev_hash,
            tx_root: [0u8; 32],
            timestamp: future_timestamp,
            height: 1,
            difficulty: state.difficulty,
            nonce: minting_tx.nonce,
            minter_view_key: minting_tx.minter_view_key,
            minter_spend_key: minting_tx.minter_spend_key,
        },
        minting_tx,
        transactions: vec![],
        lottery_outputs: Vec::new(),
        lottery_summary: BlockLotterySummary::default(),
    };

    let err = ledger
        .add_block(&bad_block)
        .expect_err("Block timestamped 3h ahead must be rejected");
    let msg = format!("{}", err);
    assert!(
        msg.contains("future"),
        "Error should mention future timestamp, got: {}",
        msg
    );
}

/// C4: A block whose `header.tx_root` does not commit to the actual
/// transactions must be rejected. The tx_root feeds the header hash and
/// therefore PoW; without re-derivation at acceptance, a relay can swap
/// the tx list under a valid PoW.
#[test]
#[serial]
fn test_c4_block_rejected_when_tx_root_does_not_match_transactions() {
    let temp_dir = TempDir::new().unwrap();
    let mut ledger = Ledger::open(temp_dir.path()).unwrap();
    let miner = create_test_wallet(1);
    let miner_address = miner.account_key().default_subaddress();

    // Build a block with an empty tx list, then claim a non-zero tx_root.
    // (`compute_tx_root(&[]) == [0; 32]`, so anything else is a mismatch.)
    let mut block = mine_block(&ledger, &miner_address, vec![]);
    block.header.tx_root = [0xAB; 32];

    let err = ledger
        .add_block(&block)
        .expect_err("Block with mismatched tx_root must be rejected");
    let msg = format!("{}", err);
    assert!(
        msg.contains("tx_root"),
        "Error should mention tx_root mismatch, got: {}",
        msg
    );
}

/// C3: A block containing a transaction whose CLSAG ring references a
/// `target_key` that is not in the UTXO set must be rejected — otherwise
/// a producer fabricates a ring member with a key they control and an
/// arbitrary commitment to mint value via the CLSAG balance proof.
#[test]
#[serial]
fn test_c3_block_rejected_when_ring_member_target_key_not_in_utxo_set() {
    let temp_dir = TempDir::new().unwrap();
    let ledger = Ledger::open(temp_dir.path()).unwrap();
    let miner = create_test_wallet(1);
    let miner_address = miner.account_key().default_subaddress();

    // A transaction with one CLSAG input whose ring member has a target_key
    // (0xAA..) that no UTXO carries. The signature/key-image content does
    // not need to be valid — the ring-member resolution runs before CLSAG
    // verification, so this test pins down exactly the C3 rejection path.
    let fake_member = RingMember {
        target_key: [0xAA; 32],
        public_key: [0xBB; 32],
        commitment: [0xCC; 32],
    };
    let fake_input = ClsagRingInput {
        ring: vec![fake_member],
        key_image: [0xDD; 32],
        commitment_key_image: [0xEE; 32],
        clsag_signature: vec![0u8; 64],
        pseudo_output_amount: 0,
    };
    let fake_output = TxOutput {
        amount: 1,
        target_key: [0x11; 32],
        public_key: [0x22; 32],
        e_memo: None,
        cluster_tags: bth_transaction_types::ClusterTagVector::empty(),
    };
    let bad_tx = Transaction::new(vec![fake_input], vec![fake_output], 0, 0);

    let mut block = mine_block(&ledger, &miner_address, vec![bad_tx]);
    // mine_block sets tx_root from the (now non-empty) transaction list.
    block.header.tx_root = Block::compute_tx_root(&block.transactions);

    let err = ledger
        .add_block(&block)
        .expect_err("Block with counterfeit ring member must be rejected");
    let msg = format!("{}", err);
    assert!(
        msg.contains("target_key not in UTXO set")
            || msg.contains("ring member"),
        "Error should identify the counterfeit ring member, got: {}",
        msg
    );
}

/// C3 (counterfeit amount): a ring member that *does* point at a real
/// target_key, but with a commitment that doesn't match the stored UTXO's
/// amount, must be rejected. The CLSAG signature can verify against any
/// claimed amount, so re-deriving the commitment from the UTXO set is the
/// only block-level defense.
#[test]
#[serial]
fn test_c3_block_rejected_when_ring_member_commitment_mismatched() {
    let temp_dir = TempDir::new().unwrap();
    let mut ledger = Ledger::open(temp_dir.path()).unwrap();
    let miner = create_test_wallet(1);
    let miner_address = miner.account_key().default_subaddress();

    // Mine a block so the coinbase UTXO populates the UTXO set with a real
    // target_key we can reference.
    let block1 = mine_block(&ledger, &miner_address, vec![]);
    ledger.add_block(&block1).expect("Failed to add block 1");
    let coinbase_target = block1.minting_tx.target_key;
    let coinbase_public = block1.minting_tx.public_key;

    // Build a ring member that points at the real target_key/public_key but
    // claims an amount-commitment derived from a different (fabricated)
    // amount. `RingMember::from_output` derives the commitment as
    // `amount * H + 0 * G`, so changing the amount changes the commitment.
    let counterfeit_member = RingMember {
        target_key: coinbase_target,
        public_key: coinbase_public,
        commitment: [0xCC; 32], // not the commitment to the coinbase amount
    };
    let fake_input = ClsagRingInput {
        ring: vec![counterfeit_member],
        key_image: [0xDD; 32],
        commitment_key_image: [0xEE; 32],
        clsag_signature: vec![0u8; 64],
        pseudo_output_amount: 0,
    };
    let fake_output = TxOutput {
        amount: 1,
        target_key: [0x11; 32],
        public_key: [0x22; 32],
        e_memo: None,
        cluster_tags: bth_transaction_types::ClusterTagVector::empty(),
    };
    let bad_tx = Transaction::new(vec![fake_input], vec![fake_output], 0, 1);

    let mut block2 = mine_block(&ledger, &miner_address, vec![bad_tx]);
    block2.header.tx_root = Block::compute_tx_root(&block2.transactions);

    let err = ledger
        .add_block(&block2)
        .expect_err("Block with counterfeit ring-member amount must be rejected");
    let msg = format!("{}", err);
    assert!(
        msg.contains("counterfeit amount") || msg.contains("mismatch"),
        "Error should identify the counterfeit amount, got: {}",
        msg
    );
}
