// Copyright (c) 2024 Botho Foundation
//
//! Transaction Lifecycle Integration Tests
//!
//! Tests the complete transaction flow from creation to block inclusion:
//! 1. Mine blocks to generate UTXOs
//! 2. Create transactions spending those UTXOs
//! 3. Submit to mempool with validation
//! 4. Build blocks containing transactions
//! 5. Apply blocks to ledger
//! 6. Verify UTXO consumption and creation
//! 7. Verify fee burning
//! 8. Verify mempool clearing
//!
//! These tests exercise the core transaction lifecycle without the complexity
//! of multi-node consensus, focusing on ledger state correctness.
//!
//! NOTE: All tests are currently ignored because they use the removed Simple
//! transaction type. They need to be rewritten to use CLSAG ring signatures
//! with proper decoy selection from the UTXO set.

use std::time::SystemTime;

use tempfile::TempDir;

use bth_account_keys::PublicAddress;
use bth_crypto_keys::RistrettoSignature;
use botho::{
    block::{Block, BlockHeader, MintingTx},
    ledger::Ledger,
    mempool::{Mempool, MempoolError},
    transaction::{
        Transaction, TxInput, TxInputs, TxOutput, Utxo, UtxoId,
        MIN_TX_FEE, PICOCREDITS_PER_CREDIT,
    },
};
use botho_wallet::WalletKeys;

/// Helper to calculate fee for transactions
///
/// NOTE: The mempool validates fees based on output_sum (total of all outputs),
/// not just the send amount. Since output_sum = input_sum - fee, and for small
/// fees this is approximately input_sum, we pass the input amount (UTXO value)
/// to ensure the calculated fee covers the validation requirement.
///
/// All transactions are now private (Standard-Private with CLSAG ring signatures).
fn calculate_fee_for_outputs(mempool: &Mempool, output_sum: u64) -> u64 {
    use bth_cluster_tax::TransactionType;
    mempool.estimate_fee(TransactionType::Hidden, output_sum, 0)
}

// ============================================================================
// Constants
// ============================================================================

/// Block reward for testing (50 BTH)
const TEST_BLOCK_REWARD: u64 = 50 * PICOCREDITS_PER_CREDIT;

/// Trivial PoW difficulty for instant mining
const TRIVIAL_DIFFICULTY: u64 = u64::MAX - 1;

// ============================================================================
// Test Helpers
// ============================================================================

/// Create a test ledger in a temporary directory
fn create_test_ledger() -> (TempDir, Ledger) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let ledger_path = temp_dir.path().join("ledger");
    let ledger = Ledger::open(&ledger_path).expect("Failed to open ledger");
    (temp_dir, ledger)
}

/// Create a deterministic wallet from a seed
/// Uses predefined 24-word mnemonics for reproducible tests
fn create_wallet(seed: u8) -> WalletKeys {
    // Use different predefined 24-word mnemonics for each wallet
    let mnemonics = [
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art",
        "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo vote",
        "legal winner thank year wave sausage worth useful legal winner thank year wave sausage worth useful legal winner thank year wave sausage worth title",
        "letter advice cage absurd amount doctor acoustic avoid letter advice cage absurd amount doctor acoustic avoid letter advice cage absurd amount doctor acoustic bless",
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art",
    ];

    let mnemonic = mnemonics[(seed as usize) % mnemonics.len()];
    WalletKeys::from_mnemonic(mnemonic).expect("Failed to create wallet from mnemonic")
}

/// Create a minting transaction for testing (trivial PoW)
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

    // Find a valid nonce (instant with trivial difficulty)
    for nonce in 0..1000 {
        minting_tx.nonce = nonce;
        if minting_tx.verify_pow() {
            break;
        }
    }

    minting_tx
}

/// Mine a block with optional transactions
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

    // Compute transaction root
    let tx_root = {
        use sha2::{Digest, Sha256};
        if transactions.is_empty() {
            [0u8; 32]
        } else {
            let mut hasher = Sha256::new();
            for tx in &transactions {
                hasher.update(tx.hash());
            }
            hasher.finalize().into()
        }
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
    }
}

/// Scan wallet for unspent UTXOs
fn scan_wallet_utxos(ledger: &Ledger, wallet: &WalletKeys) -> Vec<(Utxo, u64)> {
    let mut owned_utxos = Vec::new();
    let state = ledger.get_chain_state().unwrap();

    for height in 0..=state.height {
        if let Ok(block) = ledger.get_block(height) {
            // Check coinbase output
            let coinbase_output = block.minting_tx.to_tx_output();
            if let Some(subaddr_idx) = coinbase_output.belongs_to(wallet.account_key()) {
                let block_hash = block.hash();
                let utxo_id = UtxoId::new(block_hash, 0);
                if let Ok(Some(utxo)) = ledger.get_utxo(&utxo_id) {
                    owned_utxos.push((utxo, subaddr_idx));
                }
            }

            // Check transaction outputs
            for tx in &block.transactions {
                let tx_hash = tx.hash();
                for (idx, output) in tx.outputs.iter().enumerate() {
                    if let Some(subaddr_idx) = output.belongs_to(wallet.account_key()) {
                        let utxo_id = UtxoId::new(tx_hash, idx as u32);
                        if let Ok(Some(utxo)) = ledger.get_utxo(&utxo_id) {
                            owned_utxos.push((utxo, subaddr_idx));
                        }
                    }
                }
            }
        }
    }

    owned_utxos
}

/// Get total balance for a wallet
fn get_wallet_balance(ledger: &Ledger, wallet: &WalletKeys) -> u64 {
    scan_wallet_utxos(ledger, wallet)
        .iter()
        .map(|(utxo, _)| utxo.output.amount)
        .sum()
}

/// Minimum ring size for testing (matches production)
const MIN_RING_SIZE: usize = 20;

/// Mine enough blocks to have decoys available for ring signatures.
/// Creates UTXOs to different wallets so they can be used as decoys.
fn mine_decoy_blocks(ledger: &Ledger, primary_wallet: &WalletKeys) {
    let primary_address = primary_wallet.public_address();

    // Mine first block to primary wallet
    let block1 = mine_block(ledger, &primary_address, vec![]);
    ledger.add_block(&block1).expect("Failed to add block 1");

    // Mine 19 more blocks to other wallets for decoys
    for i in 0..19 {
        let other_wallet = create_wallet(100 + i);
        let other_address = other_wallet.public_address();
        let block = mine_block(ledger, &other_address, vec![]);
        ledger.add_block(&block).expect(&format!("Failed to add decoy block {}", i + 2));
    }
}

/// Create a signed CLSAG ring signature transaction.
///
/// This creates a transaction with ring signatures for sender privacy.
/// The real input is hidden among decoys selected from the ledger.
///
/// # Requirements
/// - The ledger must have at least MIN_RING_SIZE (20) unspent outputs for decoy selection
/// - The sender_utxo must belong to the sender_wallet
fn create_signed_transaction(
    sender_wallet: &WalletKeys,
    sender_utxo: &Utxo,
    subaddress_index: u64,
    recipient: &PublicAddress,
    amount: u64,
    fee: u64,
    current_height: u64,
    ledger: &Ledger,
) -> Transaction {
    use botho::transaction::{ClsagRingInput, RingMember};
    use rand::seq::SliceRandom;
    use rand::rngs::OsRng;

    let mut rng = OsRng;

    // Create outputs: recipient + change
    let change = sender_utxo.output.amount - amount - fee;
    let mut outputs = vec![TxOutput::new(amount, recipient)];
    if change > 0 {
        outputs.push(TxOutput::new(change, &sender_wallet.public_address()));
    }

    // Build preliminary transaction to get signing hash
    let preliminary_tx = Transaction::new_clsag(Vec::new(), outputs.clone(), fee, current_height);
    let signing_hash = preliminary_tx.signing_hash();

    // Recover the one-time private key for the real input
    let onetime_private = sender_utxo
        .output
        .recover_spend_key(sender_wallet.account_key(), subaddress_index)
        .expect("Failed to recover spend key - UTXO doesn't belong to wallet");

    // Get decoys from the ledger
    let exclude_keys = vec![sender_utxo.output.target_key];
    let decoys_needed = MIN_RING_SIZE - 1;

    let decoys = ledger
        .get_decoy_outputs(decoys_needed, &exclude_keys, 0) // 0 confirmations for tests
        .expect("Failed to get decoy outputs - need at least 20 UTXOs in ledger");

    assert!(
        decoys.len() >= decoys_needed,
        "Not enough decoys: need {}, got {}. Mine more blocks first.",
        decoys_needed,
        decoys.len()
    );

    // Build ring: real output + decoys
    let mut ring: Vec<RingMember> = Vec::with_capacity(MIN_RING_SIZE);
    ring.push(RingMember::from_output(&sender_utxo.output));
    for decoy in &decoys {
        ring.push(RingMember::from_output(decoy));
    }

    // Shuffle ring and find real input position
    let real_target_key = sender_utxo.output.target_key;
    let mut indices: Vec<usize> = (0..ring.len()).collect();
    indices.shuffle(&mut rng);
    let shuffled_ring: Vec<RingMember> = indices.iter().map(|&i| ring[i].clone()).collect();
    let real_index = shuffled_ring
        .iter()
        .position(|m| m.target_key == real_target_key)
        .expect("Real input not found in ring after shuffle");

    // Create CLSAG ring input
    let total_output = outputs.iter().map(|o| o.amount).sum::<u64>() + fee;
    let ring_input = ClsagRingInput::new(
        shuffled_ring,
        real_index,
        &onetime_private,
        sender_utxo.output.amount,
        total_output,
        &signing_hash,
        &mut rng,
    )
    .expect("Failed to create CLSAG ring signature");

    // Create final transaction
    Transaction::new_clsag(vec![ring_input], outputs, fee, current_height)
}

// ============================================================================
// Basic Transaction Lifecycle Tests
// ============================================================================

#[test]
fn test_basic_tx_lifecycle_utxo_creation_and_consumption() {
    let (_temp_dir, ledger) = create_test_ledger();
    let miner_wallet = create_wallet(1);
    let recipient_wallet = create_wallet(2);

    // Verify genesis state
    let state = ledger.get_chain_state().unwrap();
    assert_eq!(state.height, 0, "Should start at genesis");
    assert_eq!(state.total_mined, 0, "No coins mined yet in genesis");

    // Mine enough blocks to have decoys for ring signatures (MIN_RING_SIZE = 20)
    // We mine to different wallets to create diverse UTXOs
    let miner_address = miner_wallet.public_address();
    let other_wallets: Vec<WalletKeys> = (10..30).map(|i| create_wallet(i)).collect();

    // First block to miner
    let block1 = mine_block(&ledger, &miner_address, vec![]);
    ledger.add_block(&block1).expect("Failed to add block 1");

    // Mine 19 more blocks to other wallets (for decoys)
    for (i, other_wallet) in other_wallets.iter().enumerate() {
        let other_address = other_wallet.public_address();
        let block = mine_block(&ledger, &other_address, vec![]);
        ledger.add_block(&block).expect(&format!("Failed to add block {}", i + 2));
    }

    // Verify miner has coins
    let miner_balance = get_wallet_balance(&ledger, &miner_wallet);
    assert_eq!(miner_balance, TEST_BLOCK_REWARD, "Miner should have block reward");

    // Create a transaction
    let utxos = scan_wallet_utxos(&ledger, &miner_wallet);
    assert_eq!(utxos.len(), 1, "Miner should have 1 UTXO");
    let (sender_utxo, subaddr_idx) = &utxos[0];

    let send_amount = 10 * PICOCREDITS_PER_CREDIT;
    let fee = MIN_TX_FEE;
    let state = ledger.get_chain_state().unwrap();

    let tx = create_signed_transaction(
        &miner_wallet,
        sender_utxo,
        *subaddr_idx,
        &recipient_wallet.public_address(),
        send_amount,
        fee,
        state.height,
        &ledger,
    );

    // Verify the transaction has valid ring signatures
    assert!(tx.is_clsag(), "Transaction should use CLSAG signatures");
    assert!(tx.verify_ring_signatures().is_ok(), "Ring signatures should be valid");

    // Mine block with transaction
    let block = mine_block(&ledger, &miner_address, vec![tx.clone()]);
    ledger.add_block(&block).expect("Failed to add block with tx");

    // Verify recipient received funds
    let recipient_balance = get_wallet_balance(&ledger, &recipient_wallet);
    assert_eq!(recipient_balance, send_amount, "Recipient should have received funds");

    // Verify miner has change + new block reward
    let miner_balance = get_wallet_balance(&ledger, &miner_wallet);
    let expected_change = TEST_BLOCK_REWARD - send_amount - fee;
    let expected_total = expected_change + TEST_BLOCK_REWARD; // change + new block reward
    assert_eq!(miner_balance, expected_total, "Miner should have change + new block reward");
}

#[test]
fn test_tx_lifecycle_fee_burning() {
    let (_temp_dir, ledger) = create_test_ledger();
    let miner_wallet = create_wallet(1);
    let recipient_wallet = create_wallet(2);

    // Verify initial fee state
    let state = ledger.get_chain_state().unwrap();
    assert_eq!(state.total_fees_burned, 0, "No fees burned initially");

    // Mine blocks to create enough decoys
    mine_decoy_blocks(&ledger, &miner_wallet);

    // Create transaction with fee
    let utxos = scan_wallet_utxos(&ledger, &miner_wallet);
    let (sender_utxo, subaddr_idx) = &utxos[0];

    let send_amount = 10 * PICOCREDITS_PER_CREDIT;
    let fee = MIN_TX_FEE;
    let state = ledger.get_chain_state().unwrap();
    let miner_address = miner_wallet.public_address();

    let tx = create_signed_transaction(
        &miner_wallet,
        sender_utxo,
        *subaddr_idx,
        &recipient_wallet.public_address(),
        send_amount,
        fee,
        state.height,
        &ledger,
    );

    // Mine block with transaction
    let block = mine_block(&ledger, &miner_address, vec![tx]);
    ledger.add_block(&block).expect("Failed to add block with tx");

    // Verify fee was burned
    let state = ledger.get_chain_state().unwrap();
    assert_eq!(
        state.total_fees_burned, fee,
        "Transaction fee should be burned"
    );

    // Create another transaction
    let utxos = scan_wallet_utxos(&ledger, &miner_wallet);
    let (sender_utxo, subaddr_idx) = &utxos[0]; // Use first UTXO
    let state = ledger.get_chain_state().unwrap();

    let tx2 = create_signed_transaction(
        &miner_wallet,
        sender_utxo,
        *subaddr_idx,
        &recipient_wallet.public_address(),
        5 * PICOCREDITS_PER_CREDIT,
        fee,
        state.height,
        &ledger,
    );

    let block = mine_block(&ledger, &miner_address, vec![tx2]);
    ledger.add_block(&block).expect("Failed to add block with tx2");

    // Verify cumulative fees burned
    let state = ledger.get_chain_state().unwrap();
    assert_eq!(
        state.total_fees_burned,
        fee * 2,
        "Both transaction fees should be burned"
    );
}

// ============================================================================
// Mempool Integration Tests
// ============================================================================

#[test]
fn test_mempool_add_and_clear_on_block() {
    let (_temp_dir, ledger) = create_test_ledger();
    let mut mempool = Mempool::new();
    let miner_wallet = create_wallet(1);
    let recipient_wallet = create_wallet(2);

    // Mine blocks to create enough decoys
    mine_decoy_blocks(&ledger, &miner_wallet);

    // Create transaction with proper fee
    let utxos = scan_wallet_utxos(&ledger, &miner_wallet);
    let (sender_utxo, subaddr_idx) = &utxos[0];

    let send_amount = 10 * PICOCREDITS_PER_CREDIT;
    let fee = calculate_fee_for_outputs(&mempool, sender_utxo.output.amount);
    let state = ledger.get_chain_state().unwrap();
    let miner_address = miner_wallet.public_address();

    let tx = create_signed_transaction(
        &miner_wallet,
        sender_utxo,
        *subaddr_idx,
        &recipient_wallet.public_address(),
        send_amount,
        fee,
        state.height,
        &ledger,
    );

    // Add to mempool
    let tx_hash = mempool.add_tx(tx.clone(), &ledger).expect("Failed to add tx to mempool");
    assert!(mempool.contains(&tx_hash), "Mempool should contain transaction");
    assert_eq!(mempool.len(), 1, "Mempool should have 1 transaction");

    // Get transactions for block
    let txs_for_block = mempool.get_transactions(10);
    assert_eq!(txs_for_block.len(), 1, "Should get 1 transaction for block");

    // Mine block with transaction
    let block = mine_block(&ledger, &miner_address, vec![tx.clone()]);
    ledger.add_block(&block).expect("Failed to add block with tx");

    // Clear confirmed transactions from mempool
    mempool.remove_confirmed(&[tx]);
    assert!(mempool.is_empty(), "Mempool should be empty after block");
    assert!(!mempool.contains(&tx_hash), "Transaction should be removed from mempool");
}

#[test]
fn test_mempool_rejects_double_spend() {
    let (_temp_dir, ledger) = create_test_ledger();
    let mut mempool = Mempool::new();
    let miner_wallet = create_wallet(1);
    let recipient1 = create_wallet(2);
    let recipient2 = create_wallet(3);

    // Mine blocks to create enough decoys
    mine_decoy_blocks(&ledger, &miner_wallet);

    // Get the UTXO
    let utxos = scan_wallet_utxos(&ledger, &miner_wallet);
    let (sender_utxo, subaddr_idx) = &utxos[0];
    let state = ledger.get_chain_state().unwrap();

    // Create first transaction with proper fee
    let send_amount1 = 10 * PICOCREDITS_PER_CREDIT;
    let fee1 = calculate_fee_for_outputs(&mempool, sender_utxo.output.amount);
    let tx1 = create_signed_transaction(
        &miner_wallet,
        sender_utxo,
        *subaddr_idx,
        &recipient1.public_address(),
        send_amount1,
        fee1,
        state.height,
        &ledger,
    );

    // Create second transaction spending the same UTXO with proper fee
    let send_amount2 = 15 * PICOCREDITS_PER_CREDIT;
    let fee2 = calculate_fee_for_outputs(&mempool, sender_utxo.output.amount);
    let tx2 = create_signed_transaction(
        &miner_wallet,
        sender_utxo,
        *subaddr_idx,
        &recipient2.public_address(),
        send_amount2,
        fee2,
        state.height,
        &ledger,
    );

    // Add first transaction - should succeed
    mempool.add_tx(tx1, &ledger).expect("First tx should succeed");

    // Add second transaction - should fail (double spend via key image)
    let result = mempool.add_tx(tx2, &ledger);
    assert!(
        matches!(result, Err(MempoolError::DoubleSpend)),
        "Second tx should be rejected as double spend: {:?}",
        result
    );
}

#[test]
fn test_mempool_rejects_insufficient_fee() {
    let (_temp_dir, ledger) = create_test_ledger();
    let mut mempool = Mempool::new();
    let miner_wallet = create_wallet(1);
    let recipient = create_wallet(2);

    // Mine blocks to create enough decoys
    mine_decoy_blocks(&ledger, &miner_wallet);

    // Get the UTXO
    let utxos = scan_wallet_utxos(&ledger, &miner_wallet);
    let (sender_utxo, subaddr_idx) = &utxos[0];
    let state = ledger.get_chain_state().unwrap();

    let send_amount = 10 * PICOCREDITS_PER_CREDIT;
    let required_fee = calculate_fee_for_outputs(&mempool, sender_utxo.output.amount);

    // Create transaction with fee that's too low (half of required)
    let insufficient_fee = required_fee / 2;
    let tx = create_signed_transaction(
        &miner_wallet,
        sender_utxo,
        *subaddr_idx,
        &recipient.public_address(),
        send_amount,
        insufficient_fee,
        state.height,
        &ledger,
    );

    // Should be rejected for insufficient fee
    let result = mempool.add_tx(tx, &ledger);
    assert!(
        matches!(result, Err(MempoolError::FeeTooLow { .. })),
        "Transaction with insufficient fee should be rejected: {:?}",
        result
    );
}

#[test]
fn test_mempool_remove_invalid_after_block() {
    let (_temp_dir, ledger) = create_test_ledger();
    let mut mempool = Mempool::new();
    let miner_wallet = create_wallet(1);
    let recipient1 = create_wallet(2);
    let recipient2 = create_wallet(3);

    // Mine two blocks to miner (two UTXOs) + 18 more for decoys
    let miner_address = miner_wallet.public_address();
    let block1 = mine_block(&ledger, &miner_address, vec![]);
    ledger.add_block(&block1).expect("Failed to add block 1");
    let block2 = mine_block(&ledger, &miner_address, vec![]);
    ledger.add_block(&block2).expect("Failed to add block 2");

    // Mine 18 more blocks to other wallets for decoys
    for i in 0..18 {
        let other_wallet = create_wallet(100 + i);
        let block = mine_block(&ledger, &other_wallet.public_address(), vec![]);
        ledger.add_block(&block).expect(&format!("Failed to add decoy block {}", i + 3));
    }

    // Get both UTXOs belonging to miner
    let utxos = scan_wallet_utxos(&ledger, &miner_wallet);
    assert_eq!(utxos.len(), 2, "Should have 2 UTXOs");

    let (utxo1, subaddr1) = &utxos[0];
    let (utxo2, subaddr2) = &utxos[1];
    let state = ledger.get_chain_state().unwrap();

    // Create two transactions, each spending different UTXOs with proper fees
    let send_amount = 10 * PICOCREDITS_PER_CREDIT;
    let fee = calculate_fee_for_outputs(&mempool, utxo1.output.amount);

    let tx1 = create_signed_transaction(
        &miner_wallet,
        utxo1,
        *subaddr1,
        &recipient1.public_address(),
        send_amount,
        fee,
        state.height,
        &ledger,
    );

    let tx2 = create_signed_transaction(
        &miner_wallet,
        utxo2,
        *subaddr2,
        &recipient2.public_address(),
        send_amount,
        fee,
        state.height,
        &ledger,
    );

    // Add both to mempool
    mempool.add_tx(tx1.clone(), &ledger).expect("Failed to add tx1");
    mempool.add_tx(tx2.clone(), &ledger).expect("Failed to add tx2");
    assert_eq!(mempool.len(), 2, "Mempool should have 2 transactions");

    // Mine block with only tx1
    let block = mine_block(&ledger, &miner_address, vec![tx1.clone()]);
    ledger.add_block(&block).expect("Failed to add block with tx1");

    // Remove confirmed tx1
    mempool.remove_confirmed(&[tx1]);
    assert_eq!(mempool.len(), 1, "Mempool should have 1 transaction after removing confirmed");

    // tx2 should still be valid since it spent a different UTXO
    mempool.remove_invalid(&ledger);
    assert_eq!(mempool.len(), 1, "tx2 should still be valid");

    // Verify tx2 is still there
    assert!(mempool.contains(&tx2.hash()), "tx2 should still be in mempool");
}

// ============================================================================
// Chain Transaction Tests
// ============================================================================

#[test]
fn test_chained_transactions_in_sequence() {
    let (_temp_dir, ledger) = create_test_ledger();
    let wallet_a = create_wallet(1);
    let wallet_b = create_wallet(2);
    let wallet_c = create_wallet(3);

    // Mine blocks to create enough decoys
    mine_decoy_blocks(&ledger, &wallet_a);

    // A -> B: Send 20 BTH
    let utxos = scan_wallet_utxos(&ledger, &wallet_a);
    let (utxo, subaddr) = &utxos[0];
    let state = ledger.get_chain_state().unwrap();
    let addr_a = wallet_a.public_address();

    let tx1 = create_signed_transaction(
        &wallet_a,
        utxo,
        *subaddr,
        &wallet_b.public_address(),
        20 * PICOCREDITS_PER_CREDIT,
        MIN_TX_FEE,
        state.height,
        &ledger,
    );

    let block = mine_block(&ledger, &addr_a, vec![tx1]);
    ledger.add_block(&block).expect("Failed to add block with tx1");

    // Verify B has 20 BTH
    let balance_b = get_wallet_balance(&ledger, &wallet_b);
    assert_eq!(balance_b, 20 * PICOCREDITS_PER_CREDIT, "Wallet B should have 20 BTH");

    // B -> C: Send 10 BTH (spending the funds B received)
    let utxos = scan_wallet_utxos(&ledger, &wallet_b);
    assert!(!utxos.is_empty(), "Wallet B should have UTXOs");
    let (utxo, subaddr) = &utxos[0];
    let state = ledger.get_chain_state().unwrap();

    let tx2 = create_signed_transaction(
        &wallet_b,
        utxo,
        *subaddr,
        &wallet_c.public_address(),
        10 * PICOCREDITS_PER_CREDIT,
        MIN_TX_FEE,
        state.height,
        &ledger,
    );

    let block = mine_block(&ledger, &addr_a, vec![tx2]);
    ledger.add_block(&block).expect("Failed to add block with tx2");

    // Verify final balances
    let balance_c = get_wallet_balance(&ledger, &wallet_c);
    assert_eq!(balance_c, 10 * PICOCREDITS_PER_CREDIT, "Wallet C should have 10 BTH");

    let balance_b_final = get_wallet_balance(&ledger, &wallet_b);
    let expected_b = 20 * PICOCREDITS_PER_CREDIT - 10 * PICOCREDITS_PER_CREDIT - MIN_TX_FEE;
    assert_eq!(balance_b_final, expected_b, "Wallet B should have change");
}

#[test]
#[ignore = "Needs update for ring signature transactions (Simple tx removed)"]
fn test_multiple_transactions_in_single_block() {
    let (_temp_dir, ledger) = create_test_ledger();
    let miner = create_wallet(1);
    let recipient1 = create_wallet(2);
    let recipient2 = create_wallet(3);

    // Mine two blocks to get two UTXOs
    let miner_addr = miner.public_address();
    let block1 = mine_block(&ledger, &miner_addr, vec![]);
    ledger.add_block(&block1).expect("Failed to add block 1");
    let block2 = mine_block(&ledger, &miner_addr, vec![]);
    ledger.add_block(&block2).expect("Failed to add block 2");

    // Get both UTXOs
    let utxos = scan_wallet_utxos(&ledger, &miner);
    assert_eq!(utxos.len(), 2, "Should have 2 UTXOs");

    let (utxo1, subaddr1) = &utxos[0];
    let (utxo2, subaddr2) = &utxos[1];
    let state = ledger.get_chain_state().unwrap();

    // Create two transactions
    let tx1 = create_signed_transaction(
        &miner,
        utxo1,
        *subaddr1,
        &recipient1.public_address(),
        10 * PICOCREDITS_PER_CREDIT,
        MIN_TX_FEE,
        state.height,
    );

    let tx2 = create_signed_transaction(
        &miner,
        utxo2,
        *subaddr2,
        &recipient2.public_address(),
        15 * PICOCREDITS_PER_CREDIT,
        MIN_TX_FEE,
        state.height,
    );

    // Mine block with both transactions
    let block3 = mine_block(&ledger, &miner_addr, vec![tx1, tx2]);
    ledger.add_block(&block3).expect("Failed to add block 3");

    // Verify both recipients received funds
    let balance1 = get_wallet_balance(&ledger, &recipient1);
    let balance2 = get_wallet_balance(&ledger, &recipient2);

    assert_eq!(balance1, 10 * PICOCREDITS_PER_CREDIT, "Recipient 1 should have 10 BTH");
    assert_eq!(balance2, 15 * PICOCREDITS_PER_CREDIT, "Recipient 2 should have 15 BTH");

    // Verify fees burned (2 transactions)
    let state = ledger.get_chain_state().unwrap();
    assert_eq!(state.total_fees_burned, MIN_TX_FEE * 2, "Both fees should be burned");
}

// ============================================================================
// UTXO State Verification Tests
// ============================================================================

#[test]
#[ignore = "Needs update for ring signature transactions (Simple tx removed)"]
fn test_utxo_set_consistency_after_transactions() {
    let (_temp_dir, ledger) = create_test_ledger();
    let miner = create_wallet(1);
    let recipient = create_wallet(2);

    // Mine initial block
    let miner_addr = miner.public_address();
    let block1 = mine_block(&ledger, &miner_addr, vec![]);
    ledger.add_block(&block1).expect("Failed to add block 1");

    // Count initial UTXOs
    let initial_utxos = scan_wallet_utxos(&ledger, &miner);
    assert_eq!(initial_utxos.len(), 1, "Should have 1 initial UTXO");

    // Create and apply transaction
    let (utxo, subaddr) = &initial_utxos[0];
    let state = ledger.get_chain_state().unwrap();
    let input_utxo_id = utxo.id;

    let tx = create_signed_transaction(
        &miner,
        utxo,
        *subaddr,
        &recipient.public_address(),
        10 * PICOCREDITS_PER_CREDIT,
        MIN_TX_FEE,
        state.height,
    );

    let block2 = mine_block(&ledger, &miner_addr, vec![tx.clone()]);
    ledger.add_block(&block2).expect("Failed to add block 2");

    // Verify old UTXO is gone
    assert!(
        !ledger.utxo_exists(&input_utxo_id).unwrap(),
        "Input UTXO should be consumed"
    );

    // Verify new UTXOs exist (one for recipient, one for change, one for coinbase)
    let tx_hash = tx.hash();

    // Recipient output (index 0)
    let recipient_utxo_id = UtxoId::new(tx_hash, 0);
    assert!(
        ledger.utxo_exists(&recipient_utxo_id).unwrap(),
        "Recipient UTXO should exist"
    );

    // Change output (index 1)
    let change_utxo_id = UtxoId::new(tx_hash, 1);
    assert!(
        ledger.utxo_exists(&change_utxo_id).unwrap(),
        "Change UTXO should exist"
    );

    // Verify amounts
    let recipient_utxo = ledger.get_utxo(&recipient_utxo_id).unwrap().unwrap();
    assert_eq!(recipient_utxo.output.amount, 10 * PICOCREDITS_PER_CREDIT);

    let change_utxo = ledger.get_utxo(&change_utxo_id).unwrap().unwrap();
    let expected_change = TEST_BLOCK_REWARD - 10 * PICOCREDITS_PER_CREDIT - MIN_TX_FEE;
    assert_eq!(change_utxo.output.amount, expected_change);
}

#[test]
#[ignore = "Needs update for ring signature transactions (Simple tx removed)"]
fn test_transaction_index_lookup() {
    let (_temp_dir, ledger) = create_test_ledger();
    let miner = create_wallet(1);
    let recipient = create_wallet(2);

    // Mine and add transaction
    let miner_addr = miner.public_address();
    let block1 = mine_block(&ledger, &miner_addr, vec![]);
    ledger.add_block(&block1).expect("Failed to add block 1");

    let utxos = scan_wallet_utxos(&ledger, &miner);
    let (utxo, subaddr) = &utxos[0];
    let state = ledger.get_chain_state().unwrap();

    let tx = create_signed_transaction(
        &miner,
        utxo,
        *subaddr,
        &recipient.public_address(),
        10 * PICOCREDITS_PER_CREDIT,
        MIN_TX_FEE,
        state.height,
    );
    let tx_hash = tx.hash();

    let block2 = mine_block(&ledger, &miner_addr, vec![tx]);
    ledger.add_block(&block2).expect("Failed to add block 2");

    // Look up transaction by hash
    let tx_location = ledger.get_transaction_location(&tx_hash).expect("Failed to get tx location");
    assert!(tx_location.is_some(), "Transaction should be indexed");

    let location = tx_location.unwrap();
    assert_eq!(location.block_height, 2, "Transaction should be in block 2");
    assert_eq!(location.tx_index, 0, "Transaction should be at index 0");

    // Verify we can retrieve the block and find the transaction
    let block = ledger.get_block(location.block_height).unwrap();
    assert_eq!(block.transactions.len(), 1);
    assert_eq!(block.transactions[0].hash(), tx_hash);
}

// ============================================================================
// Edge Case Tests
// ============================================================================

#[test]
#[ignore = "Needs update for ring signature transactions (Simple tx removed)"]
fn test_exact_amount_spend_no_change() {
    // TODO: Rewrite to use CLSAG ring signatures
    todo!("Update to use CLSAG ring signatures instead of Simple transactions");
}

#[test]
#[ignore = "Needs update for ring signature transactions (Simple tx removed)"]
fn test_mempool_already_exists_rejection() {
    let (_temp_dir, ledger) = create_test_ledger();
    let mut mempool = Mempool::new();
    let miner = create_wallet(1);
    let recipient = create_wallet(2);

    // Mine a block
    let miner_addr = miner.public_address();
    let block1 = mine_block(&ledger, &miner_addr, vec![]);
    ledger.add_block(&block1).expect("Failed to add block 1");

    let utxos = scan_wallet_utxos(&ledger, &miner);
    let (utxo, subaddr) = &utxos[0];
    let state = ledger.get_chain_state().unwrap();

    let send_amount = 10 * PICOCREDITS_PER_CREDIT;
    let fee = calculate_fee_for_outputs(&mempool, utxo.output.amount);

    let tx = create_signed_transaction(
        &miner,
        utxo,
        *subaddr,
        &recipient.public_address(),
        send_amount,
        fee,
        state.height,
    );

    // Add transaction once
    mempool.add_tx(tx.clone(), &ledger).expect("First add should succeed");

    // Try to add again
    let result = mempool.add_tx(tx, &ledger);
    assert!(
        matches!(result, Err(MempoolError::AlreadyExists)),
        "Duplicate transaction should be rejected: {:?}",
        result
    );
}

#[test]
#[ignore = "Needs update for ring signature transactions (Simple tx removed)"]
fn test_mempool_transactions_sorted_by_fee() {
    let (_temp_dir, ledger) = create_test_ledger();
    let mut mempool = Mempool::new();
    let miner = create_wallet(1);
    let recipient = create_wallet(2);

    // Mine three blocks
    let miner_addr = miner.public_address();
    for _ in 0..3 {
        let block = mine_block(&ledger, &miner_addr, vec![]);
        ledger.add_block(&block).expect("Failed to add block");
    }

    // Get UTXOs
    let utxos = scan_wallet_utxos(&ledger, &miner);
    assert!(utxos.len() >= 3, "Should have at least 3 UTXOs");

    let state = ledger.get_chain_state().unwrap();

    // Create transactions with different fees (base fee + multiplier)
    let send_amount = 5 * PICOCREDITS_PER_CREDIT;
    // Use first UTXO's amount for base fee calculation
    let base_fee = calculate_fee_for_outputs(&mempool, utxos[0].0.output.amount);
    let fee_multipliers = [1u64, 2u64, 3u64];
    let mut actual_fees = vec![];

    for (i, multiplier) in fee_multipliers.iter().enumerate() {
        let (utxo, subaddr) = &utxos[i];
        let fee = base_fee * multiplier;
        let tx = create_signed_transaction(
            &miner,
            utxo,
            *subaddr,
            &recipient.public_address(),
            send_amount,
            fee,
            state.height,
        );
        actual_fees.push(fee);
        mempool.add_tx(tx, &ledger).expect("Failed to add tx");
    }

    // Get transactions (should be sorted by fee per byte, highest first)
    let sorted_txs = mempool.get_transactions(10);
    assert_eq!(sorted_txs.len(), 3);

    // Highest fee should be first (3x base)
    assert_eq!(sorted_txs[0].fee, base_fee * 3);
    // Lowest fee should be last (1x base)
    assert_eq!(sorted_txs[2].fee, base_fee);
}
