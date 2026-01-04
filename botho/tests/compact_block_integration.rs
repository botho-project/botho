// Copyright (c) 2024 Botho Foundation
//
//! Compact Block Integration Tests
//!
//! Tests the compact block relay protocol for bandwidth-efficient block
//! propagation:
//! - Compact block creation from full blocks
//! - Block reconstruction from mempool transactions
//! - GetBlockTxn/BlockTxn request/response flow for missing transactions
//! - Size comparison between full and compact blocks

use std::time::SystemTime;

use sha2::{Digest, Sha256};

use botho::{
    block::{Block, BlockHeader, MintingTx},
    mempool::Mempool,
    network::{BlockTxn, CompactBlock, GetBlockTxn, ReconstructionResult},
    transaction::{ClsagRingInput, RingMember, Transaction, TxOutput, PICOCREDITS_PER_CREDIT},
};
use botho_wallet::WalletKeys;
use bth_account_keys::PublicAddress;
use bth_transaction_types::ClusterTagVector;

// ============================================================================
// Constants
// ============================================================================

/// Block reward for testing (50 BTH)
const TEST_BLOCK_REWARD: u64 = 50 * PICOCREDITS_PER_CREDIT;

/// Trivial difficulty for fast PoW
const TRIVIAL_DIFFICULTY: u64 = 0x00FF_FFFF_FFFF_FFFF;

/// Minimum ring size for CLSAG signatures
const MIN_RING_SIZE: usize = 11;

// ============================================================================
// Helper Functions
// ============================================================================

fn create_test_wallet(seed: u8) -> WalletKeys {
    let mnemonics = [
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art",
        "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo vote",
    ];
    let mnemonic = mnemonics[(seed as usize) % mnemonics.len()];
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

    // Find a valid nonce
    for nonce in 0..1000 {
        minting_tx.nonce = nonce;
        if minting_tx.verify_pow() {
            break;
        }
    }

    minting_tx
}

/// Create a mock transaction with deterministic hash based on seed.
///
/// This creates a minimal valid-looking transaction structure for testing
/// compact block reconstruction. The transaction is not cryptographically
/// valid but has a unique, deterministic hash.
fn create_mock_transaction(seed: u64) -> Transaction {
    // Create deterministic key image from seed
    let mut key_image = [0u8; 32];
    key_image[0..8].copy_from_slice(&seed.to_le_bytes());
    key_image[8..16].copy_from_slice(&(seed.wrapping_mul(0x12345678)).to_le_bytes());

    // Create minimal ring with deterministic data
    let ring: Vec<RingMember> = (0..MIN_RING_SIZE)
        .map(|i| {
            let mut target_key = [0u8; 32];
            let mut public_key = [0u8; 32];
            let mut commitment = [0u8; 32];

            let ring_seed = seed.wrapping_add(i as u64);
            target_key[0..8].copy_from_slice(&ring_seed.to_le_bytes());
            public_key[0..8].copy_from_slice(&ring_seed.wrapping_mul(2).to_le_bytes());
            commitment[0..8].copy_from_slice(&ring_seed.wrapping_mul(3).to_le_bytes());

            RingMember {
                target_key,
                public_key,
                commitment,
            }
        })
        .collect();

    // Create mock commitment key image
    let mut commitment_key_image = [0u8; 32];
    commitment_key_image[0..8].copy_from_slice(&seed.wrapping_mul(5).to_le_bytes());

    let input = ClsagRingInput {
        ring,
        key_image,
        commitment_key_image,
        clsag_signature: vec![0u8; 64], // Mock signature
    };

    // Create deterministic output
    let mut target_key = [0u8; 32];
    let mut public_key = [0u8; 32];
    target_key[0..8].copy_from_slice(&seed.wrapping_mul(10).to_le_bytes());
    public_key[0..8].copy_from_slice(&seed.wrapping_mul(11).to_le_bytes());

    let output = TxOutput {
        amount: 1_000_000 + seed,
        target_key,
        public_key,
        e_memo: None,
        cluster_tags: ClusterTagVector::default(),
    };

    Transaction::new_clsag(
        vec![input],
        vec![output],
        10_000, // fee
        0,      // created_at_height
    )
}

fn create_block_with_transactions(
    minter_address: &PublicAddress,
    prev_block_hash: [u8; 32],
    height: u64,
    transactions: Vec<Transaction>,
) -> Block {
    let minting_tx =
        create_mock_minting_tx(height, TEST_BLOCK_REWARD, minter_address, prev_block_hash);

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
            prev_block_hash,
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
    }
}

// ============================================================================
// Compact Block Creation Tests
// ============================================================================

#[test]
fn test_compact_block_from_empty_block() {
    let wallet = create_test_wallet(1);
    let address = wallet.account_key().default_subaddress();

    let block = create_block_with_transactions(&address, [0u8; 32], 1, vec![]);
    let compact = CompactBlock::from_block(&block);

    assert_eq!(compact.height(), 1);
    assert_eq!(compact.short_ids.len(), 0);
    assert_eq!(compact.header.height, block.header.height);
    assert_eq!(compact.hash(), block.hash());
}

#[test]
fn test_compact_block_from_block_with_transactions() {
    let wallet = create_test_wallet(1);
    let address = wallet.account_key().default_subaddress();

    // Create block with 10 transactions
    let transactions: Vec<Transaction> = (0..10).map(create_mock_transaction).collect();
    let block = create_block_with_transactions(&address, [0u8; 32], 1, transactions);

    let compact = CompactBlock::from_block(&block);

    assert_eq!(compact.height(), 1);
    assert_eq!(compact.short_ids.len(), 10);
    assert_eq!(compact.hash(), block.hash());
}

#[test]
fn test_compact_block_short_ids_are_unique() {
    let wallet = create_test_wallet(1);
    let address = wallet.account_key().default_subaddress();

    // Create block with 100 transactions
    let transactions: Vec<Transaction> = (0..100).map(create_mock_transaction).collect();
    let block = create_block_with_transactions(&address, [0u8; 32], 1, transactions);

    let compact = CompactBlock::from_block(&block);

    // All short IDs should be unique
    let mut seen = std::collections::HashSet::new();
    for short_id in &compact.short_ids {
        assert!(
            seen.insert(*short_id),
            "Short IDs should be unique within a block"
        );
    }
}

// ============================================================================
// Block Reconstruction Tests
// ============================================================================

#[test]
fn test_reconstruction_with_all_transactions_in_mempool() {
    let wallet = create_test_wallet(1);
    let address = wallet.account_key().default_subaddress();

    // Create transactions
    let transactions: Vec<Transaction> = (0..5).map(create_mock_transaction).collect();
    let block = create_block_with_transactions(&address, [0u8; 32], 1, transactions.clone());

    // For this test, we'll use prefilled transactions to simulate having
    // all transactions available (since adding to mempool requires a ledger)
    let compact_with_prefilled =
        CompactBlock::from_block_with_prefilled(&block, &(0..5).collect::<Vec<_>>());

    // Reconstruction should succeed with prefilled transactions
    let empty_mempool = Mempool::new();
    let result = compact_with_prefilled.reconstruct(&empty_mempool);

    match result {
        ReconstructionResult::Complete(reconstructed) => {
            assert_eq!(reconstructed.height(), block.height());
            assert_eq!(reconstructed.transactions.len(), block.transactions.len());
            for (original, reconstructed) in block
                .transactions
                .iter()
                .zip(reconstructed.transactions.iter())
            {
                assert_eq!(original.hash(), reconstructed.hash());
            }
        }
        ReconstructionResult::Incomplete { .. } => {
            panic!("Reconstruction should succeed with all prefilled transactions");
        }
    }
}

#[test]
fn test_reconstruction_with_missing_transactions() {
    let wallet = create_test_wallet(1);
    let address = wallet.account_key().default_subaddress();

    // Create block with 5 transactions
    let transactions: Vec<Transaction> = (0..5).map(create_mock_transaction).collect();
    let block = create_block_with_transactions(&address, [0u8; 32], 1, transactions.clone());

    // Create compact block with only first 2 transactions prefilled
    let compact = CompactBlock::from_block_with_prefilled(&block, &[0, 1]);

    // Reconstruction should fail (missing transactions 2, 3, 4)
    let empty_mempool = Mempool::new();
    let result = compact.reconstruct(&empty_mempool);

    match result {
        ReconstructionResult::Complete(_) => {
            panic!("Reconstruction should fail with missing transactions");
        }
        ReconstructionResult::Incomplete { missing_indices } => {
            assert_eq!(missing_indices.len(), 3);
            assert!(missing_indices.contains(&2));
            assert!(missing_indices.contains(&3));
            assert!(missing_indices.contains(&4));
        }
    }
}

#[test]
fn test_reconstruction_after_adding_missing_transactions() {
    let wallet = create_test_wallet(1);
    let address = wallet.account_key().default_subaddress();

    // Create block with 5 transactions
    let transactions: Vec<Transaction> = (0..5).map(create_mock_transaction).collect();
    let block = create_block_with_transactions(&address, [0u8; 32], 1, transactions.clone());

    // Create compact block with no prefilled transactions
    let mut compact = CompactBlock::from_block(&block);

    // First reconstruction attempt should fail
    let empty_mempool = Mempool::new();
    let result = compact.reconstruct(&empty_mempool);

    let missing_indices = match result {
        ReconstructionResult::Incomplete { missing_indices } => missing_indices,
        _ => panic!("Should have missing transactions"),
    };

    // Simulate receiving BlockTxn response
    let missing_txs: Vec<Transaction> = missing_indices
        .iter()
        .map(|&idx| transactions[idx as usize].clone())
        .collect();

    compact.add_transactions(&missing_indices, missing_txs);

    // Second reconstruction should succeed
    let result = compact.reconstruct(&empty_mempool);

    match result {
        ReconstructionResult::Complete(reconstructed) => {
            assert_eq!(reconstructed.height(), block.height());
            assert_eq!(reconstructed.transactions.len(), 5);
        }
        ReconstructionResult::Incomplete { .. } => {
            panic!("Reconstruction should succeed after adding missing transactions");
        }
    }
}

// ============================================================================
// GetBlockTxn / BlockTxn Protocol Tests
// ============================================================================

#[test]
fn test_get_block_txn_request_creation() {
    let block_hash = [42u8; 32];
    let missing_indices = vec![2, 4, 7];

    let request = GetBlockTxn {
        block_hash,
        indices: missing_indices.clone(),
    };

    assert_eq!(request.block_hash, block_hash);
    assert_eq!(request.indices, missing_indices);
}

#[test]
fn test_block_txn_response_creation() {
    let wallet = create_test_wallet(1);
    let address = wallet.account_key().default_subaddress();

    let transactions: Vec<Transaction> = (0..10).map(create_mock_transaction).collect();
    let block = create_block_with_transactions(&address, [0u8; 32], 1, transactions.clone());

    // Simulate responding to GetBlockTxn for indices 2, 5, 8
    let requested_indices = vec![2, 5, 8];
    let response_txs: Vec<Transaction> = requested_indices
        .iter()
        .map(|&idx| transactions[idx].clone())
        .collect();

    let response = BlockTxn {
        block_hash: block.hash(),
        txs: response_txs,
    };

    assert_eq!(response.block_hash, block.hash());
    assert_eq!(response.txs.len(), 3);
    assert_eq!(response.txs[0].hash(), transactions[2].hash());
    assert_eq!(response.txs[1].hash(), transactions[5].hash());
    assert_eq!(response.txs[2].hash(), transactions[8].hash());
}

#[test]
fn test_full_compact_block_protocol_flow() {
    let wallet = create_test_wallet(1);
    let address = wallet.account_key().default_subaddress();

    // Step 1: Miner creates block with 10 transactions
    let transactions: Vec<Transaction> = (0..10).map(create_mock_transaction).collect();
    let block = create_block_with_transactions(&address, [0u8; 32], 1, transactions.clone());

    // Step 2: Miner broadcasts compact block
    // Receiver receives it and starts with prefilled transactions representing
    // what they already have in their mempool (simulated as indices 0, 1, 6, 9)
    let mut receiver_compact = CompactBlock::from_block_with_prefilled(&block, &[0, 1, 6, 9]);
    let empty_mempool = Mempool::new();

    // Step 3: Receiver attempts reconstruction
    let result = receiver_compact.reconstruct(&empty_mempool);

    // Step 4: Receiver identifies missing transactions
    let missing_indices = match result {
        ReconstructionResult::Incomplete { missing_indices } => missing_indices,
        _ => panic!("Should have missing transactions"),
    };

    assert_eq!(missing_indices.len(), 6);

    // Step 5: Receiver sends GetBlockTxn request
    let request = GetBlockTxn {
        block_hash: block.hash(),
        indices: missing_indices.clone(),
    };

    // Step 6: Original node looks up requested transactions from their copy
    let response_txs: Vec<Transaction> = request
        .indices
        .iter()
        .filter_map(|&idx| block.transactions.get(idx as usize).cloned())
        .collect();

    let response = BlockTxn {
        block_hash: request.block_hash,
        txs: response_txs,
    };

    // Step 7: Receiver adds received transactions to their compact block
    receiver_compact.add_transactions(&missing_indices, response.txs);

    // Step 8: Receiver completes reconstruction
    let final_result = receiver_compact.reconstruct(&empty_mempool);

    match final_result {
        ReconstructionResult::Complete(reconstructed) => {
            assert_eq!(reconstructed.height(), block.height());
            assert_eq!(reconstructed.hash(), block.hash());
            assert_eq!(reconstructed.transactions.len(), 10);

            // Verify all transaction hashes match
            for (i, (original, reconstructed)) in block
                .transactions
                .iter()
                .zip(reconstructed.transactions.iter())
                .enumerate()
            {
                assert_eq!(
                    original.hash(),
                    reconstructed.hash(),
                    "Transaction {} hash mismatch",
                    i
                );
            }
        }
        ReconstructionResult::Incomplete { missing_indices } => {
            panic!(
                "Reconstruction should succeed, but missing {} transactions",
                missing_indices.len()
            );
        }
    }
}

// ============================================================================
// Size Comparison Tests (Bandwidth Reduction Verification)
// ============================================================================

#[test]
fn test_compact_block_size_reduction_simple_block() {
    let wallet = create_test_wallet(1);
    let address = wallet.account_key().default_subaddress();

    // Create block with 100 transactions
    let transactions: Vec<Transaction> = (0..100).map(create_mock_transaction).collect();
    let block = create_block_with_transactions(&address, [0u8; 32], 1, transactions);

    // Serialize full block
    let full_block_bytes = bincode::serialize(&block).unwrap();

    // Serialize compact block
    let compact = CompactBlock::from_block(&block);
    let compact_block_bytes = bincode::serialize(&compact).unwrap();

    // Compact block should be significantly smaller
    let reduction_ratio = 1.0 - (compact_block_bytes.len() as f64 / full_block_bytes.len() as f64);

    println!("Full block size: {} bytes", full_block_bytes.len());
    println!("Compact block size: {} bytes", compact_block_bytes.len());
    println!("Size reduction: {:.1}%", reduction_ratio * 100.0);

    // With 100 transactions, we expect at least 80% reduction
    // (transactions are ~100+ bytes each, short IDs are 6 bytes)
    assert!(
        reduction_ratio > 0.80,
        "Expected >80% size reduction, got {:.1}%",
        reduction_ratio * 100.0
    );
}

#[test]
fn test_compact_block_size_reduction_large_block() {
    let wallet = create_test_wallet(1);
    let address = wallet.account_key().default_subaddress();

    // Create block with 1000 transactions (simulating high-throughput scenario)
    let transactions: Vec<Transaction> = (0..1000).map(create_mock_transaction).collect();
    let block = create_block_with_transactions(&address, [0u8; 32], 1, transactions);

    // Serialize full block
    let full_block_bytes = bincode::serialize(&block).unwrap();

    // Serialize compact block
    let compact = CompactBlock::from_block(&block);
    let compact_block_bytes = bincode::serialize(&compact).unwrap();

    let reduction_ratio = 1.0 - (compact_block_bytes.len() as f64 / full_block_bytes.len() as f64);

    println!(
        "Large block - Full size: {} bytes ({:.2} KB)",
        full_block_bytes.len(),
        full_block_bytes.len() as f64 / 1024.0
    );
    println!(
        "Large block - Compact size: {} bytes ({:.2} KB)",
        compact_block_bytes.len(),
        compact_block_bytes.len() as f64 / 1024.0
    );
    println!("Size reduction: {:.1}%", reduction_ratio * 100.0);

    // With 1000 transactions, we expect at least 90% reduction
    assert!(
        reduction_ratio > 0.90,
        "Expected >90% size reduction for large block, got {:.1}%",
        reduction_ratio * 100.0
    );
}

#[test]
fn test_compact_block_estimated_size_accuracy() {
    let wallet = create_test_wallet(1);
    let address = wallet.account_key().default_subaddress();

    // Create block with various transaction counts
    for tx_count in [10, 50, 100, 500] {
        let transactions: Vec<Transaction> = (0..tx_count).map(create_mock_transaction).collect();
        let block = create_block_with_transactions(&address, [0u8; 32], 1, transactions);

        let compact = CompactBlock::from_block(&block);
        let estimated = compact.estimated_size();
        let actual = bincode::serialize(&compact).unwrap().len();

        // Estimated size should be within 50% of actual (rough estimate)
        let ratio = estimated as f64 / actual as f64;
        assert!(
            ratio > 0.5 && ratio < 2.0,
            "Estimated size ({}) should be roughly accurate vs actual ({}) for {} txs",
            estimated,
            actual,
            tx_count
        );
    }
}

// ============================================================================
// Edge Cases
// ============================================================================

#[test]
fn test_compact_block_with_prefilled_transactions() {
    let wallet = create_test_wallet(1);
    let address = wallet.account_key().default_subaddress();

    let transactions: Vec<Transaction> = (0..5).map(create_mock_transaction).collect();
    let block = create_block_with_transactions(&address, [0u8; 32], 1, transactions.clone());

    // Prefill indices 1 and 3
    let compact = CompactBlock::from_block_with_prefilled(&block, &[1, 3]);

    assert_eq!(compact.prefilled_txs.len(), 2);
    assert_eq!(compact.prefilled_txs[0].index, 1);
    assert_eq!(compact.prefilled_txs[0].tx.hash(), transactions[1].hash());
    assert_eq!(compact.prefilled_txs[1].index, 3);
    assert_eq!(compact.prefilled_txs[1].tx.hash(), transactions[3].hash());
}

#[test]
fn test_compact_block_serialization_roundtrip() {
    let wallet = create_test_wallet(1);
    let address = wallet.account_key().default_subaddress();

    let transactions: Vec<Transaction> = (0..10).map(create_mock_transaction).collect();
    let block = create_block_with_transactions(&address, [0u8; 32], 1, transactions);

    let compact = CompactBlock::from_block(&block);

    // Serialize and deserialize
    let bytes = bincode::serialize(&compact).unwrap();
    let deserialized: CompactBlock = bincode::deserialize(&bytes).unwrap();

    assert_eq!(deserialized.height(), compact.height());
    assert_eq!(deserialized.hash(), compact.hash());
    assert_eq!(deserialized.short_ids.len(), compact.short_ids.len());
    assert_eq!(deserialized.short_ids, compact.short_ids);
}

#[test]
fn test_get_block_txn_serialization_roundtrip() {
    let request = GetBlockTxn {
        block_hash: [42u8; 32],
        indices: vec![1, 5, 7, 12, 99],
    };

    let bytes = bincode::serialize(&request).unwrap();
    let deserialized: GetBlockTxn = bincode::deserialize(&bytes).unwrap();

    assert_eq!(deserialized.block_hash, request.block_hash);
    assert_eq!(deserialized.indices, request.indices);
}

#[test]
fn test_block_txn_serialization_roundtrip() {
    let transactions: Vec<Transaction> = (0..3).map(create_mock_transaction).collect();

    let response = BlockTxn {
        block_hash: [42u8; 32],
        txs: transactions.clone(),
    };

    let bytes = bincode::serialize(&response).unwrap();
    let deserialized: BlockTxn = bincode::deserialize(&bytes).unwrap();

    assert_eq!(deserialized.block_hash, response.block_hash);
    assert_eq!(deserialized.txs.len(), response.txs.len());
    for (original, deserialized) in response.txs.iter().zip(deserialized.txs.iter()) {
        assert_eq!(original.hash(), deserialized.hash());
    }
}
