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
        PICOCREDITS_PER_CREDIT,
    },
};
use botho_wallet::WalletKeys;

/// Helper to calculate fee for simple (non-private) transactions
fn calculate_fee(mempool: &Mempool, amount: u64) -> u64 {
    mempool.estimate_fee(false, amount, 0)
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

/// Create a signed transaction
fn create_signed_transaction(
    sender_wallet: &WalletKeys,
    sender_utxo: &Utxo,
    subaddress_index: u64,
    recipient: &PublicAddress,
    amount: u64,
    fee: u64,
    current_height: u64,
) -> Transaction {
    let sender_utxo_amount = sender_utxo.output.amount;
    assert!(
        sender_utxo_amount >= amount + fee,
        "Insufficient funds: have {}, need {} + {} fee",
        sender_utxo_amount,
        amount,
        fee
    );

    // Create outputs
    let mut outputs = vec![TxOutput::new(amount, recipient)];

    // Change output if needed
    let change = sender_utxo_amount.saturating_sub(amount + fee);
    if change > 0 {
        let change_addr = sender_wallet.public_address();
        outputs.push(TxOutput::new(change, &change_addr));
    }

    // Create unsigned input
    let input = TxInput {
        tx_hash: sender_utxo.id.tx_hash,
        output_index: sender_utxo.id.output_index,
        signature: vec![0u8; 64], // Placeholder
    };

    // Create transaction to get signing hash
    let mut tx = Transaction::new_simple(vec![input], outputs, fee, current_height);
    let signing_hash = tx.signing_hash();

    // Recover the one-time private key for this output
    let onetime_private = sender_utxo
        .output
        .recover_spend_key(sender_wallet.account_key(), subaddress_index)
        .expect("Failed to recover spend key");

    // Sign with the one-time private key
    let signature: RistrettoSignature = onetime_private.sign_schnorrkel(b"botho-tx-v1", &signing_hash);

    // Update the input with the real signature
    if let TxInputs::Simple(ref mut inputs) = tx.inputs {
        let sig_bytes: &[u8] = signature.as_ref();
        inputs[0].signature = sig_bytes.to_vec();
    }

    tx
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

    // Mine a block to generate coins
    let miner_address = miner_wallet.public_address();
    let block1 = mine_block(&ledger, &miner_address, vec![]);
    ledger.add_block(&block1).expect("Failed to add block 1");

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
    );

    // Verify input UTXO exists before block
    assert!(
        ledger.utxo_exists(&sender_utxo.id).unwrap(),
        "Input UTXO should exist before block"
    );

    // Mine block with transaction
    let block2 = mine_block(&ledger, &miner_address, vec![tx.clone()]);
    ledger.add_block(&block2).expect("Failed to add block 2");

    // Verify input UTXO is consumed
    assert!(
        !ledger.utxo_exists(&sender_utxo.id).unwrap(),
        "Input UTXO should be consumed after block"
    );

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

    // Mine a block
    let miner_address = miner_wallet.public_address();
    let block1 = mine_block(&ledger, &miner_address, vec![]);
    ledger.add_block(&block1).expect("Failed to add block 1");

    // Create transaction with fee
    let utxos = scan_wallet_utxos(&ledger, &miner_wallet);
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
    );

    // Mine block with transaction
    let block2 = mine_block(&ledger, &miner_address, vec![tx]);
    ledger.add_block(&block2).expect("Failed to add block 2");

    // Verify fee was burned
    let state = ledger.get_chain_state().unwrap();
    assert_eq!(
        state.total_fees_burned, fee,
        "Transaction fee should be burned"
    );

    // Create another transaction
    let utxos = scan_wallet_utxos(&ledger, &miner_wallet);
    let (sender_utxo, subaddr_idx) = &utxos[0]; // Use first UTXO

    let tx2 = create_signed_transaction(
        &miner_wallet,
        sender_utxo,
        *subaddr_idx,
        &recipient_wallet.public_address(),
        5 * PICOCREDITS_PER_CREDIT,
        fee,
        state.height,
    );

    let block3 = mine_block(&ledger, &miner_address, vec![tx2]);
    ledger.add_block(&block3).expect("Failed to add block 3");

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

    // Mine a block
    let miner_address = miner_wallet.public_address();
    let block1 = mine_block(&ledger, &miner_address, vec![]);
    ledger.add_block(&block1).expect("Failed to add block 1");

    // Create transaction with proper fee
    let utxos = scan_wallet_utxos(&ledger, &miner_wallet);
    let (sender_utxo, subaddr_idx) = &utxos[0];

    let send_amount = 10 * PICOCREDITS_PER_CREDIT;
    let fee = calculate_fee(&mempool, send_amount);
    let state = ledger.get_chain_state().unwrap();

    let tx = create_signed_transaction(
        &miner_wallet,
        sender_utxo,
        *subaddr_idx,
        &recipient_wallet.public_address(),
        send_amount,
        fee,
        state.height,
    );

    // Add to mempool
    let tx_hash = mempool.add_tx(tx.clone(), &ledger).expect("Failed to add tx to mempool");
    assert!(mempool.contains(&tx_hash), "Mempool should contain transaction");
    assert_eq!(mempool.len(), 1, "Mempool should have 1 transaction");

    // Get transactions for block
    let txs_for_block = mempool.get_transactions(10);
    assert_eq!(txs_for_block.len(), 1, "Should get 1 transaction for block");

    // Mine block with transaction
    let block2 = mine_block(&ledger, &miner_address, vec![tx.clone()]);
    ledger.add_block(&block2).expect("Failed to add block 2");

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

    // Mine a block
    let miner_address = miner_wallet.public_address();
    let block1 = mine_block(&ledger, &miner_address, vec![]);
    ledger.add_block(&block1).expect("Failed to add block 1");

    // Get the UTXO
    let utxos = scan_wallet_utxos(&ledger, &miner_wallet);
    let (sender_utxo, subaddr_idx) = &utxos[0];
    let state = ledger.get_chain_state().unwrap();

    // Create first transaction with proper fee
    let send_amount1 = 10 * PICOCREDITS_PER_CREDIT;
    let fee1 = calculate_fee(&mempool, send_amount1);
    let tx1 = create_signed_transaction(
        &miner_wallet,
        sender_utxo,
        *subaddr_idx,
        &recipient1.public_address(),
        send_amount1,
        fee1,
        state.height,
    );

    // Create second transaction spending the same UTXO with proper fee
    let send_amount2 = 15 * PICOCREDITS_PER_CREDIT;
    let fee2 = calculate_fee(&mempool, send_amount2);
    let tx2 = create_signed_transaction(
        &miner_wallet,
        sender_utxo,
        *subaddr_idx,
        &recipient2.public_address(),
        send_amount2,
        fee2,
        state.height,
    );

    // Add first transaction - should succeed
    mempool.add_tx(tx1, &ledger).expect("First tx should succeed");

    // Add second transaction - should fail (double spend)
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

    // Mine a block
    let miner_address = miner_wallet.public_address();
    let block1 = mine_block(&ledger, &miner_address, vec![]);
    ledger.add_block(&block1).expect("Failed to add block 1");

    // Get the UTXO
    let utxos = scan_wallet_utxos(&ledger, &miner_wallet);
    let (sender_utxo, subaddr_idx) = &utxos[0];
    let state = ledger.get_chain_state().unwrap();

    // Create transaction with zero fee
    let tx = create_signed_transaction(
        &miner_wallet,
        sender_utxo,
        *subaddr_idx,
        &recipient.public_address(),
        10 * PICOCREDITS_PER_CREDIT,
        0, // Zero fee
        state.height,
    );

    // Should be rejected for insufficient fee
    let result = mempool.add_tx(tx, &ledger);
    assert!(
        matches!(result, Err(MempoolError::FeeTooLow { .. })),
        "Transaction with zero fee should be rejected: {:?}",
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

    // Mine two blocks (two UTXOs)
    let miner_address = miner_wallet.public_address();
    let block1 = mine_block(&ledger, &miner_address, vec![]);
    ledger.add_block(&block1).expect("Failed to add block 1");
    let block2 = mine_block(&ledger, &miner_address, vec![]);
    ledger.add_block(&block2).expect("Failed to add block 2");

    // Get both UTXOs
    let utxos = scan_wallet_utxos(&ledger, &miner_wallet);
    assert_eq!(utxos.len(), 2, "Should have 2 UTXOs");

    let (utxo1, subaddr1) = &utxos[0];
    let (utxo2, subaddr2) = &utxos[1];
    let state = ledger.get_chain_state().unwrap();

    // Create two transactions, each spending different UTXOs
    let tx1 = create_signed_transaction(
        &miner_wallet,
        utxo1,
        *subaddr1,
        &recipient1.public_address(),
        10 * PICOCREDITS_PER_CREDIT,
        MIN_TX_FEE,
        state.height,
    );

    let tx2 = create_signed_transaction(
        &miner_wallet,
        utxo2,
        *subaddr2,
        &recipient2.public_address(),
        10 * PICOCREDITS_PER_CREDIT,
        MIN_TX_FEE,
        state.height,
    );

    // Add both to mempool
    mempool.add_tx(tx1.clone(), &ledger).expect("Failed to add tx1");
    mempool.add_tx(tx2.clone(), &ledger).expect("Failed to add tx2");
    assert_eq!(mempool.len(), 2, "Mempool should have 2 transactions");

    // Mine block with only tx1
    let block3 = mine_block(&ledger, &miner_address, vec![tx1.clone()]);
    ledger.add_block(&block3).expect("Failed to add block 3");

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

    // Mine initial block to wallet A
    let addr_a = wallet_a.public_address();
    let block1 = mine_block(&ledger, &addr_a, vec![]);
    ledger.add_block(&block1).expect("Failed to add block 1");

    // A -> B: Send 20 BTH
    let utxos = scan_wallet_utxos(&ledger, &wallet_a);
    let (utxo, subaddr) = &utxos[0];
    let state = ledger.get_chain_state().unwrap();

    let tx1 = create_signed_transaction(
        &wallet_a,
        utxo,
        *subaddr,
        &wallet_b.public_address(),
        20 * PICOCREDITS_PER_CREDIT,
        MIN_TX_FEE,
        state.height,
    );

    let block2 = mine_block(&ledger, &addr_a, vec![tx1]);
    ledger.add_block(&block2).expect("Failed to add block 2");

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
    );

    let block3 = mine_block(&ledger, &addr_a, vec![tx2]);
    ledger.add_block(&block3).expect("Failed to add block 3");

    // Verify final balances
    let balance_c = get_wallet_balance(&ledger, &wallet_c);
    assert_eq!(balance_c, 10 * PICOCREDITS_PER_CREDIT, "Wallet C should have 10 BTH");

    let balance_b_final = get_wallet_balance(&ledger, &wallet_b);
    let expected_b = 20 * PICOCREDITS_PER_CREDIT - 10 * PICOCREDITS_PER_CREDIT - MIN_TX_FEE;
    assert_eq!(balance_b_final, expected_b, "Wallet B should have change");
}

#[test]
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
fn test_exact_amount_spend_no_change() {
    let (_temp_dir, ledger) = create_test_ledger();
    let miner = create_wallet(1);
    let recipient = create_wallet(2);

    // Mine a block
    let miner_addr = miner.public_address();
    let block1 = mine_block(&ledger, &miner_addr, vec![]);
    ledger.add_block(&block1).expect("Failed to add block 1");

    let utxos = scan_wallet_utxos(&ledger, &miner);
    let (utxo, subaddr) = &utxos[0];
    let state = ledger.get_chain_state().unwrap();

    // Spend entire UTXO (amount = balance - fee)
    let amount = TEST_BLOCK_REWARD - MIN_TX_FEE;

    // Create unsigned input
    let input = TxInput {
        tx_hash: utxo.id.tx_hash,
        output_index: utxo.id.output_index,
        signature: vec![0u8; 64],
    };

    // Create with only one output (no change)
    let output = TxOutput::new(amount, &recipient.public_address());
    let mut tx = Transaction::new_simple(vec![input], vec![output], MIN_TX_FEE, state.height);
    let signing_hash = tx.signing_hash();

    let onetime_private = utxo
        .output
        .recover_spend_key(miner.account_key(), *subaddr)
        .unwrap();
    let signature: RistrettoSignature = onetime_private.sign_schnorrkel(b"botho-tx-v1", &signing_hash);

    if let TxInputs::Simple(ref mut inputs) = tx.inputs {
        let sig_bytes: &[u8] = signature.as_ref();
        inputs[0].signature = sig_bytes.to_vec();
    }

    let tx_hash = tx.hash();
    let block2 = mine_block(&ledger, &miner_addr, vec![tx]);
    ledger.add_block(&block2).expect("Failed to add block 2");

    // Verify only one output was created (no change)
    let recipient_utxo_id = UtxoId::new(tx_hash, 0);
    assert!(ledger.utxo_exists(&recipient_utxo_id).unwrap());

    let change_utxo_id = UtxoId::new(tx_hash, 1);
    assert!(!ledger.utxo_exists(&change_utxo_id).unwrap(), "No change UTXO should exist");

    // Recipient should have the full amount
    let balance = get_wallet_balance(&ledger, &recipient);
    assert_eq!(balance, amount);
}

#[test]
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

    let tx = create_signed_transaction(
        &miner,
        utxo,
        *subaddr,
        &recipient.public_address(),
        10 * PICOCREDITS_PER_CREDIT,
        MIN_TX_FEE,
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

    // Create transactions with different fees
    let fees = [MIN_TX_FEE, MIN_TX_FEE * 2, MIN_TX_FEE * 3];
    let mut tx_hashes = vec![];

    for (i, fee) in fees.iter().enumerate() {
        let (utxo, subaddr) = &utxos[i];
        let tx = create_signed_transaction(
            &miner,
            utxo,
            *subaddr,
            &recipient.public_address(),
            5 * PICOCREDITS_PER_CREDIT,
            *fee,
            state.height,
        );
        tx_hashes.push((tx.hash(), *fee));
        mempool.add_tx(tx, &ledger).expect("Failed to add tx");
    }

    // Get transactions (should be sorted by fee, highest first)
    let sorted_txs = mempool.get_transactions(10);
    assert_eq!(sorted_txs.len(), 3);

    // Highest fee should be first
    assert_eq!(sorted_txs[0].fee, MIN_TX_FEE * 3);
    // Lowest fee should be last
    assert_eq!(sorted_txs[2].fee, MIN_TX_FEE);
}
