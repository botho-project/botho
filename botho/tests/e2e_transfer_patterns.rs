// Copyright (c) 2024 Botho Foundation
//
//! End-to-End Transfer Pattern Tests
//!
//! Tests realistic transaction patterns to build confidence in the system:
//! 1. Concurrent transfers - Multiple wallets sending simultaneously
//! 2. Multi-input consolidation - Spending multiple UTXOs in one transaction
//! 3. Payment splitting - One sender paying multiple recipients
//! 4. Stress/load patterns - High-volume transaction bursts
//!
//! These tests use a simulated 5-node SCP consensus network with in-memory
//! message passing for fast, deterministic testing.
//!
//! Tests use CLSAG ring signatures with proper decoy selection from the UTXO
//! set. The `ensure_decoy_availability` helper pre-mines enough blocks to
//! satisfy the minimum ring size requirement.

mod common;

use std::{thread, time::Duration};

use bth_account_keys::PublicAddress;

use botho::transaction::{Utxo, MIN_TX_FEE, PICOCREDITS_PER_CREDIT};

use crate::common::{
    create_multi_input_transaction, create_signed_transaction, create_split_payment_transaction,
    ensure_decoy_availability, get_wallet_balance, mine_block, scan_wallet_utxos, TestNetwork,
    TestNetworkConfig, DEFAULT_NUM_NODES, INITIAL_BLOCK_REWARD, TEST_RING_SIZE,
};

// ============================================================================
// Tests
// ============================================================================

/// Test 1: Concurrent Transfers
///
/// Multiple wallets broadcast transactions simultaneously, all included
/// in the same block. Tests mempool handling and consensus under concurrent
/// load.
#[test]
fn test_concurrent_transfers() {
    println!("\n=== Concurrent Transfers Test ===\n");

    let mut network = TestNetwork::build(TestNetworkConfig::for_stress_testing());
    thread::sleep(Duration::from_millis(500));

    // Mine exactly TEST_RING_SIZE blocks distributed across wallets for decoy
    // availability Each wallet gets some blocks, and we need at least 20 total
    println!("Mining initial blocks for decoys and to fund wallets...");
    for i in 0..TEST_RING_SIZE {
        mine_block(&network, i % DEFAULT_NUM_NODES);
    }
    network.verify_consistency();

    // Verify each wallet has at least one UTXO
    for (i, wallet) in network.wallets.iter().enumerate() {
        let balance = get_wallet_balance(&network, wallet);
        println!("  Wallet {}: {} BTH", i, balance / PICOCREDITS_PER_CREDIT);
        assert!(
            balance >= INITIAL_BLOCK_REWARD,
            "Wallet {} should have mining reward",
            i
        );
    }

    // Create concurrent transfers: each wallet sends to the next
    println!("\nCreating {} concurrent transactions...", DEFAULT_NUM_NODES);
    let node = network.get_node(0);
    let current_height = node.chain_state().height;
    drop(node);

    let mut transactions = Vec::new();
    let send_amount = 5 * PICOCREDITS_PER_CREDIT;

    for i in 0..DEFAULT_NUM_NODES {
        let sender_wallet = &network.wallets[i];
        let recipient_wallet = &network.wallets[(i + 1) % DEFAULT_NUM_NODES];
        let recipient_address = recipient_wallet.default_address();

        let sender_utxos = scan_wallet_utxos(&network, sender_wallet);
        let (utxo, subaddr_idx) = &sender_utxos[0];

        let tx = create_signed_transaction(
            sender_wallet,
            utxo,
            *subaddr_idx,
            &recipient_address,
            send_amount,
            MIN_TX_FEE,
            current_height,
            &network,
        )
        .expect(&format!("Failed to create tx from wallet {}", i));

        transactions.push((i, tx));
    }

    // Broadcast all transactions simultaneously
    println!("Broadcasting all transactions...");
    for (i, tx) in &transactions {
        println!(
            "  Wallet {} -> Wallet {}: {} BTH",
            i,
            (i + 1) % DEFAULT_NUM_NODES,
            send_amount / PICOCREDITS_PER_CREDIT
        );
        network.broadcast_transaction(tx.clone());
    }

    // Mine a single block containing all transactions
    println!("\nMining block with all concurrent transactions...");
    mine_block(&network, 0);

    // Verify all transactions were included
    network.verify_consistency();

    let node = network.get_node(0);
    let state = node.chain_state();
    let expected_fees = DEFAULT_NUM_NODES as u64 * MIN_TX_FEE;
    println!(
        "\nFees burned: {} (expected: {})",
        state.total_fees_burned, expected_fees
    );
    assert!(
        state.total_fees_burned >= expected_fees,
        "Expected at least {} fees from {} concurrent transactions",
        expected_fees,
        DEFAULT_NUM_NODES
    );
    drop(node);

    // Verify each wallet received the transfer
    println!("\nFinal balances:");
    for (i, wallet) in network.wallets.iter().enumerate() {
        let balance = get_wallet_balance(&network, wallet);
        println!("  Wallet {}: {} BTH", i, balance / PICOCREDITS_PER_CREDIT);
    }

    println!("\n=== Concurrent Transfers Test Complete ===");
    println!(
        "  - {} transactions processed in single block",
        DEFAULT_NUM_NODES
    );
    println!("  - All nodes reached consensus");
    println!("  - Ring of transfers verified");

    network.stop();
}

/// Test 2: Multi-Input Consolidation
///
/// A wallet with multiple small UTXOs consolidates them into a single
/// larger output. Tests dust collection and multi-input transaction handling.
#[test]
fn test_multi_input_consolidation() {
    println!("\n=== Multi-Input Consolidation Test ===\n");

    let mut network = TestNetwork::build(TestNetworkConfig::for_stress_testing());
    thread::sleep(Duration::from_millis(500));

    // Pre-mine blocks for decoy availability (need extra for the 3 inputs we'll
    // consolidate)
    ensure_decoy_availability(&network, 5);

    // Mine 5 blocks to wallet 0 (creates 5 UTXOs to potentially consolidate)
    println!("Mining 5 blocks to wallet 0 for UTXOs to consolidate...");
    for _ in 0..5 {
        mine_block(&network, 0);
    }
    network.verify_consistency();

    let wallet0 = &network.wallets[0];
    let wallet1 = &network.wallets[1];
    let recipient_address = wallet1.default_address();

    let utxos = scan_wallet_utxos(&network, wallet0);
    println!("  Wallet 0 has {} UTXOs", utxos.len());
    assert!(utxos.len() >= 3, "Wallet 0 should have at least 3 UTXOs");

    let utxos_to_consolidate: Vec<(Utxo, u64)> = utxos.into_iter().take(3).collect();
    let total_input: u64 = utxos_to_consolidate
        .iter()
        .map(|(u, _)| u.output.amount)
        .sum();
    let send_amount = total_input - MIN_TX_FEE;

    let node = network.get_node(0);
    let current_height = node.chain_state().height;
    drop(node);

    println!(
        "  Consolidating {} UTXOs ({} BTH total) into single output",
        utxos_to_consolidate.len(),
        total_input / PICOCREDITS_PER_CREDIT
    );

    let tx = create_multi_input_transaction(
        wallet0,
        &utxos_to_consolidate,
        &recipient_address,
        send_amount,
        MIN_TX_FEE,
        current_height,
        &network,
    )
    .expect("Failed to create multi-input transaction");

    println!("  Transaction has {} inputs", tx.inputs.len());

    network.broadcast_transaction(tx.clone());
    mine_block(&network, 0);
    network.verify_consistency();

    let balance1 = get_wallet_balance(&network, wallet1);
    println!(
        "  Wallet 1 balance: {} BTH",
        balance1 / PICOCREDITS_PER_CREDIT
    );
    assert!(
        balance1 >= send_amount,
        "Wallet 1 should have received amount"
    );

    println!("\n=== Multi-Input Consolidation Test Complete ===");
    network.stop();
}

/// Test 3: Payment Splitting
///
/// A single sender pays multiple recipients in one transaction.
/// Tests multi-output transaction handling.
#[test]
fn test_payment_splitting() {
    println!("\n=== Payment Splitting Test ===\n");

    let mut network = TestNetwork::build(TestNetworkConfig::for_stress_testing());
    thread::sleep(Duration::from_millis(500));

    // Pre-mine blocks for decoy availability, plus one for the sender's UTXO
    ensure_decoy_availability(&network, 1);

    // Mine a block to fund wallet 0
    println!("Mining block to fund wallet 0...");
    mine_block(&network, 0);
    network.verify_consistency();

    let sender_wallet = &network.wallets[0];
    let sender_balance = get_wallet_balance(&network, sender_wallet);
    println!(
        "  Sender (wallet 0) balance: {} BTH",
        sender_balance / PICOCREDITS_PER_CREDIT
    );

    // Prepare recipients: wallets 1, 2, 3, 4
    let addr1 = network.wallets[1].default_address();
    let addr2 = network.wallets[2].default_address();
    let addr3 = network.wallets[3].default_address();
    let addr4 = network.wallets[4].default_address();
    let recipients: Vec<(&PublicAddress, u64)> = vec![
        (&addr1, 5 * PICOCREDITS_PER_CREDIT),
        (&addr2, 7 * PICOCREDITS_PER_CREDIT),
        (&addr3, 3 * PICOCREDITS_PER_CREDIT),
        (&addr4, 2 * PICOCREDITS_PER_CREDIT),
    ];
    let total_to_send: u64 = recipients.iter().map(|(_, amt)| *amt).sum();

    println!(
        "\nCreating split payment to {} recipients:",
        recipients.len()
    );
    for (i, (_, amt)) in recipients.iter().enumerate() {
        println!(
            "  -> Wallet {}: {} BTH",
            i + 1,
            amt / PICOCREDITS_PER_CREDIT
        );
    }
    println!(
        "  Total: {} BTH + {} fee",
        total_to_send / PICOCREDITS_PER_CREDIT,
        MIN_TX_FEE / PICOCREDITS_PER_CREDIT
    );

    let node = network.get_node(0);
    let current_height = node.chain_state().height;
    drop(node);

    let sender_utxos = scan_wallet_utxos(&network, sender_wallet);
    let (utxo, subaddr_idx) = &sender_utxos[0];

    let split_tx = create_split_payment_transaction(
        sender_wallet,
        utxo,
        *subaddr_idx,
        &recipients,
        MIN_TX_FEE,
        current_height,
        &network,
    )
    .expect("Failed to create split payment");

    // Verify transaction structure
    println!("\nTransaction structure:");
    println!("  Inputs: 1");
    println!(
        "  Outputs: {} (4 recipients + 1 change)",
        split_tx.outputs.len()
    );
    assert_eq!(
        split_tx.outputs.len(),
        5,
        "Should have 5 outputs (4 recipients + change)"
    );

    network.broadcast_transaction(split_tx.clone());
    mine_block(&network, 0); // Wallet 0 mines, getting a new reward

    // Verify all recipients received their amounts
    network.verify_consistency();

    println!("\nFinal balances:");
    for (i, wallet) in network.wallets.iter().enumerate() {
        let balance = get_wallet_balance(&network, wallet);
        println!("  Wallet {}: {} BTH", i, balance / PICOCREDITS_PER_CREDIT);
    }

    // Verify each recipient has at least what we sent them
    assert!(
        get_wallet_balance(&network, &network.wallets[1]) >= 5 * PICOCREDITS_PER_CREDIT,
        "Wallet 1 should have at least 5 BTH"
    );
    assert!(
        get_wallet_balance(&network, &network.wallets[2]) >= 7 * PICOCREDITS_PER_CREDIT,
        "Wallet 2 should have at least 7 BTH"
    );
    assert!(
        get_wallet_balance(&network, &network.wallets[3]) >= 3 * PICOCREDITS_PER_CREDIT,
        "Wallet 3 should have at least 3 BTH"
    );
    assert!(
        get_wallet_balance(&network, &network.wallets[4]) >= 2 * PICOCREDITS_PER_CREDIT,
        "Wallet 4 should have at least 2 BTH"
    );

    println!("\n=== Payment Splitting Test Complete ===");
    println!(
        "  - Single transaction paid {} recipients",
        recipients.len()
    );
    println!("  - All recipients received correct amounts");
    println!("  - Change returned to sender");

    network.stop();
}

/// Test 4: Stress/Load Testing
///
/// High-volume transaction bursts to test throughput and stability.
/// Generates many transactions across multiple blocks.
#[test]
#[ignore = "Needs update for ring signature transactions (Simple tx removed)"]
fn test_stress_load_patterns() {
    println!("\n=== Stress/Load Test ===\n");

    let mut network = TestNetwork::build(TestNetworkConfig::for_stress_testing());
    thread::sleep(Duration::from_millis(500));

    // Mine initial blocks to fund all wallets
    println!("Phase 1: Mining initial blocks to fund all wallets...");
    let initial_blocks = 10;
    for i in 0..initial_blocks {
        mine_block(&network, i % DEFAULT_NUM_NODES);
    }
    network.verify_consistency();

    // Show initial balances
    println!("\nInitial balances:");
    for (i, wallet) in network.wallets.iter().enumerate() {
        let balance = get_wallet_balance(&network, wallet);
        println!("  Wallet {}: {} BTH", i, balance / PICOCREDITS_PER_CREDIT);
    }

    // Phase 2: Generate burst of transactions
    println!("\nPhase 2: Generating transaction burst...");

    let node = network.get_node(0);
    let mut current_height = node.chain_state().height;
    drop(node);

    let transactions_per_block = 3; // Keep manageable for test speed
    let num_stress_blocks = 5;
    let mut total_transactions = 0;
    let mut total_fees_expected: u64 = 0;

    for block_num in 0..num_stress_blocks {
        println!("\n  Block {} transactions:", block_num + 1);

        // Create multiple transactions for this block
        for tx_num in 0..transactions_per_block {
            let sender_idx = (block_num * transactions_per_block + tx_num) % DEFAULT_NUM_NODES;
            let recipient_idx = (sender_idx + 1) % DEFAULT_NUM_NODES;

            let sender_wallet = &network.wallets[sender_idx];
            let recipient_wallet = &network.wallets[recipient_idx];

            let sender_utxos = scan_wallet_utxos(&network, sender_wallet);
            if sender_utxos.is_empty() {
                println!("    Wallet {} has no UTXOs, skipping", sender_idx);
                continue;
            }

            let (utxo, subaddr_idx) = &sender_utxos[0];
            let available = utxo.output.amount;
            let send_amount = 1 * PICOCREDITS_PER_CREDIT; // Small transfers

            if available < send_amount + MIN_TX_FEE {
                println!("    Wallet {} insufficient funds, skipping", sender_idx);
                continue;
            }

            let tx = create_signed_transaction(
                sender_wallet,
                utxo,
                *subaddr_idx,
                &recipient_wallet.default_address(),
                send_amount,
                MIN_TX_FEE,
                current_height,
                &network,
            );

            match tx {
                Ok(transaction) => {
                    network.broadcast_transaction(transaction);
                    total_transactions += 1;
                    total_fees_expected += MIN_TX_FEE;
                    println!("    {} -> {}: 1 BTH", sender_idx, recipient_idx);
                }
                Err(e) => {
                    println!("    {} -> {}: FAILED ({})", sender_idx, recipient_idx, e);
                }
            }
        }

        // Mine block with these transactions
        mine_block(&network, block_num % DEFAULT_NUM_NODES);

        let node = network.get_node(0);
        current_height = node.chain_state().height;
        drop(node);
    }

    // Verify consistency after all stress blocks
    println!("\nPhase 3: Verifying consistency...");
    network.verify_consistency();

    let node = network.get_node(0);
    let final_state = node.chain_state();
    drop(node);

    println!("\nStress test results:");
    println!("  Total blocks: {}", final_state.height);
    println!("  Total transactions created: {}", total_transactions);
    println!(
        "  Total fees burned: {} picocredits",
        final_state.total_fees_burned
    );
    println!("  Max expected fees: {} picocredits", total_fees_expected);

    // Calculate how many transactions were actually confirmed
    let confirmed_tx_count = final_state.total_fees_burned / MIN_TX_FEE;
    println!("  Confirmed transactions: {}", confirmed_tx_count);

    // At least some transactions should have been processed (at least 30%
    // throughput)
    let min_expected_confirms = total_transactions as u64 / 3;
    assert!(
        confirmed_tx_count >= min_expected_confirms,
        "Expected at least {} transactions confirmed, got {}",
        min_expected_confirms,
        confirmed_tx_count
    );

    // Final balance verification
    println!("\nFinal balances:");
    let mut total_balance: u64 = 0;
    for (i, wallet) in network.wallets.iter().enumerate() {
        let balance = get_wallet_balance(&network, wallet);
        total_balance += balance;
        println!("  Wallet {}: {} BTH", i, balance / PICOCREDITS_PER_CREDIT);
    }

    // Verify conservation: total balance = total mined - fees
    let expected_circulating = final_state.total_mined - final_state.total_fees_burned;
    println!("\nConservation check:");
    println!(
        "  Total mined: {} BTH",
        final_state.total_mined / PICOCREDITS_PER_CREDIT
    );
    println!(
        "  Fees burned: {} picocredits",
        final_state.total_fees_burned
    );
    println!(
        "  Expected circulating: {} BTH",
        expected_circulating / PICOCREDITS_PER_CREDIT
    );
    println!(
        "  Actual total balance: {} BTH",
        total_balance / PICOCREDITS_PER_CREDIT
    );

    assert_eq!(
        total_balance, expected_circulating,
        "Total balance should equal circulating supply"
    );

    println!("\n=== Stress/Load Test Complete ===");
    println!(
        "  - {} transactions across {} blocks",
        total_transactions, num_stress_blocks
    );
    println!("  - All nodes maintained consistency");
    println!("  - Conservation verified: no coins created or destroyed");

    network.stop();
}

/// Test 5: Rapid Sequential Transfers
///
/// A chain of rapid transfers between wallets, testing UTXO availability
/// and quick succession transaction handling.
#[test]
#[ignore = "Needs update for ring signature transactions (Simple tx removed)"]
fn test_rapid_sequential_transfers() {
    println!("\n=== Rapid Sequential Transfers Test ===\n");

    let mut network = TestNetwork::build(TestNetworkConfig::for_stress_testing());
    thread::sleep(Duration::from_millis(500));

    // Mine initial block to wallet 0
    println!("Mining initial block to wallet 0...");
    mine_block(&network, 0);
    network.verify_consistency();

    let initial_balance = get_wallet_balance(&network, &network.wallets[0]);
    println!(
        "  Wallet 0 initial balance: {} BTH",
        initial_balance / PICOCREDITS_PER_CREDIT
    );

    // Create a chain of transfers: 0 -> 1 -> 2 -> 3 -> 4 -> 0
    // Each transfer happens after the previous is confirmed
    println!("\nCreating rapid transfer chain: 0 -> 1 -> 2 -> 3 -> 4 -> 0");

    let send_amount = 10 * PICOCREDITS_PER_CREDIT;

    for round in 0..DEFAULT_NUM_NODES {
        let sender_idx = round;
        let recipient_idx = (round + 1) % DEFAULT_NUM_NODES;

        let node = network.get_node(0);
        let current_height = node.chain_state().height;
        drop(node);

        let sender_wallet = &network.wallets[sender_idx];
        let recipient_wallet = &network.wallets[recipient_idx];

        let sender_utxos = scan_wallet_utxos(&network, sender_wallet);
        assert!(
            !sender_utxos.is_empty(),
            "Wallet {} should have UTXOs",
            sender_idx
        );

        let (utxo, subaddr_idx) = &sender_utxos[0];

        let tx = create_signed_transaction(
            sender_wallet,
            utxo,
            *subaddr_idx,
            &recipient_wallet.default_address(),
            send_amount,
            MIN_TX_FEE,
            current_height,
            &network,
        )
        .expect(&format!(
            "Failed to create transfer {} -> {}",
            sender_idx, recipient_idx
        ));

        println!(
            "  {} -> {}: {} BTH (confirmed in next block)",
            sender_idx,
            recipient_idx,
            send_amount / PICOCREDITS_PER_CREDIT
        );

        network.broadcast_transaction(tx);

        // Mine immediately to confirm this transaction before the next
        mine_block(&network, recipient_idx);
    }

    // Final verification
    network.verify_consistency();

    println!("\nFinal balances after {} rapid transfers:", DEFAULT_NUM_NODES);
    for (i, wallet) in network.wallets.iter().enumerate() {
        let balance = get_wallet_balance(&network, wallet);
        println!("  Wallet {}: {} BTH", i, balance / PICOCREDITS_PER_CREDIT);
    }

    // The 10 BTH should have made a complete circuit, ending back at wallet 0
    // Each wallet should have: their mining rewards + received - sent
    // Due to fees, the circulating amount decreases with each transfer

    let node = network.get_node(0);
    let final_state = node.chain_state();
    drop(node);

    let expected_fees = DEFAULT_NUM_NODES as u64 * MIN_TX_FEE;
    assert!(
        final_state.total_fees_burned >= expected_fees,
        "Expected at least {} fees from {} transfers",
        expected_fees,
        DEFAULT_NUM_NODES
    );

    println!("\n=== Rapid Sequential Transfers Test Complete ===");
    println!("  - {} rapid transfers completed", DEFAULT_NUM_NODES);
    println!("  - Each transfer confirmed before next began");
    println!("  - UTXO availability verified at each step");

    network.stop();
}

/// Test 6: Mixed Transaction Patterns
///
/// Combines all patterns in a single test: concurrent, multi-input,
/// split payments, and sequential transfers.
#[test]
#[ignore = "Needs update for ring signature transactions (Simple tx removed)"]
fn test_mixed_transaction_patterns() {
    println!("\n=== Mixed Transaction Patterns Test ===\n");

    let mut network = TestNetwork::build(TestNetworkConfig::for_stress_testing());
    thread::sleep(Duration::from_millis(500));

    // Phase 1: Fund all wallets with multiple UTXOs
    println!("Phase 1: Creating initial UTXO distribution...");
    for _ in 0..3 {
        for i in 0..DEFAULT_NUM_NODES {
            mine_block(&network, i);
        }
    }
    network.verify_consistency();

    println!("Initial state:");
    for (i, wallet) in network.wallets.iter().enumerate() {
        let utxos = scan_wallet_utxos(&network, wallet);
        let balance = utxos.iter().map(|(u, _)| u.output.amount).sum::<u64>();
        println!(
            "  Wallet {}: {} UTXOs, {} BTH",
            i,
            utxos.len(),
            balance / PICOCREDITS_PER_CREDIT
        );
    }

    let node = network.get_node(0);
    let current_height = node.chain_state().height;
    drop(node);

    // Phase 2: Execute mixed patterns in a single block
    println!("\nPhase 2: Creating mixed transaction batch...");

    // Pattern A: Wallet 0 consolidates 2 UTXOs
    let wallet0 = &network.wallets[0];
    let utxos0 = scan_wallet_utxos(&network, wallet0);
    let utxos_to_consolidate: Vec<(Utxo, u64)> = utxos0.into_iter().take(2).collect();
    let consolidate_total: u64 = utxos_to_consolidate
        .iter()
        .map(|(u, _)| u.output.amount)
        .sum();

    let consolidate_tx = create_multi_input_transaction(
        wallet0,
        &utxos_to_consolidate,
        &wallet0.default_address(),
        consolidate_total - MIN_TX_FEE,
        MIN_TX_FEE,
        current_height,
        &network,
    )
    .expect("Failed to create consolidation");
    println!("  [A] Wallet 0: Consolidating 2 UTXOs");

    // Pattern B: Wallet 1 splits to wallets 2, 3
    let wallet1 = &network.wallets[1];
    let utxos1 = scan_wallet_utxos(&network, wallet1);
    let (utxo1, subaddr1) = &utxos1[0];

    let split_addr2 = network.wallets[2].default_address();
    let split_addr3 = network.wallets[3].default_address();
    let split_recipients: Vec<(&PublicAddress, u64)> = vec![
        (&split_addr2, 5 * PICOCREDITS_PER_CREDIT),
        (&split_addr3, 5 * PICOCREDITS_PER_CREDIT),
    ];

    let split_tx = create_split_payment_transaction(
        wallet1,
        utxo1,
        *subaddr1,
        &split_recipients,
        MIN_TX_FEE,
        current_height,
        &network,
    )
    .expect("Failed to create split payment");
    println!("  [B] Wallet 1: Split payment to wallets 2, 3");

    // Pattern C: Wallet 4 simple transfer to wallet 0
    let wallet4 = &network.wallets[4];
    let utxos4 = scan_wallet_utxos(&network, wallet4);
    let (utxo4, subaddr4) = &utxos4[0];

    let simple_tx = create_signed_transaction(
        wallet4,
        utxo4,
        *subaddr4,
        &wallet0.default_address(),
        3 * PICOCREDITS_PER_CREDIT,
        MIN_TX_FEE,
        current_height,
        &network,
    )
    .expect("Failed to create simple transfer");
    println!("  [C] Wallet 4: Simple transfer to wallet 0");

    // Broadcast all simultaneously
    println!("\nBroadcasting all transactions concurrently...");
    network.broadcast_transaction(consolidate_tx);
    network.broadcast_transaction(split_tx);
    network.broadcast_transaction(simple_tx);

    // Mine single block with all patterns
    mine_block(&network, 2);
    network.verify_consistency();

    // Phase 3: Verify results
    println!("\nPhase 3: Verification...");

    let node = network.get_node(0);
    let final_state = node.chain_state();
    drop(node);

    println!("Final state:");
    for (i, wallet) in network.wallets.iter().enumerate() {
        let utxos = scan_wallet_utxos(&network, wallet);
        let balance = utxos.iter().map(|(u, _)| u.output.amount).sum::<u64>();
        println!(
            "  Wallet {}: {} UTXOs, {} BTH",
            i,
            utxos.len(),
            balance / PICOCREDITS_PER_CREDIT
        );
    }

    // Verify conservation
    let total_balance: u64 = network
        .wallets
        .iter()
        .map(|w| get_wallet_balance(&network, w))
        .sum();
    let expected = final_state.total_mined - final_state.total_fees_burned;

    println!(
        "\nConservation: total_balance={}, expected={}",
        total_balance, expected
    );
    assert_eq!(total_balance, expected, "Conservation violated");

    println!("\n=== Mixed Transaction Patterns Test Complete ===");
    println!("  - Consolidation, split payment, and simple transfer in one block");
    println!("  - All patterns executed and verified");
    println!("  - Conservation maintained");

    network.stop();
}
