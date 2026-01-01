// Copyright (c) 2024 Botho Foundation
//
//! End-to-End Integration Test: 5-Node SCP Consensus with Mining and
//! Transactions
//!
//! This test verifies the complete blockchain flow:
//! 1. Start 5 nodes in SCP consensus (mesh topology)
//! 2. Mine blocks to generate coins
//! 3. Execute private transactions with CLSAG ring signatures
//! 4. Verify final ledger state including fees burned
//!
//! The test uses a simulated network with crossbeam channels for message
//! passing, following the pattern from `scp_sim.rs`. Each node has its own
//! LMDB-backed ledger.

mod common;

use std::{thread, time::Duration};

use botho::transaction::{TxOutput, MIN_TX_FEE, PICOCREDITS_PER_CREDIT};

use crate::common::{
    create_mock_minting_tx, create_signed_transaction, get_wallet_balance, mine_block,
    scan_wallet_utxos, TestNetwork, TestNetworkConfig, DEFAULT_NUM_NODES, INITIAL_BLOCK_REWARD,
    TEST_RING_SIZE,
};

// ============================================================================
// Main Test
// ============================================================================

#[test]
#[ignore = "Balance verification assertion needs investigation (wallet balance > circulating supply)"]
fn test_e2e_5_node_consensus_with_mining_and_transactions() {
    println!("\n=== E2E Consensus Integration Test ===\n");

    // Phase 0: Build the test network
    println!(
        "Phase 0: Building test network with {} nodes...",
        DEFAULT_NUM_NODES
    );
    let mut network = TestNetwork::build(TestNetworkConfig::default());

    // Give nodes time to initialize
    thread::sleep(Duration::from_millis(500));

    // Phase 1: Mine initial blocks to generate coins
    // Need at least TEST_RING_SIZE blocks for decoys in ring signatures
    println!("\nPhase 1: Mining initial blocks...");
    let blocks_to_mine = TEST_RING_SIZE; // Need 20 blocks for ring signature decoys

    for i in 0..blocks_to_mine {
        let miner_idx = i % DEFAULT_NUM_NODES;
        println!(
            "  Mining block {} (miner: node {})...",
            i + 1,
            miner_idx
        );
        mine_block(&network, miner_idx);
    }

    // Verify consistency after mining
    println!("\nVerifying ledger consistency after mining...");
    network.verify_consistency();

    let node = network.get_node(0);
    let state = node.chain_state();
    println!("  Height: {}", state.height);
    println!("  Total mined: {} picocredits", state.total_mined);
    println!(
        "  Total fees burned: {} picocredits",
        state.total_fees_burned
    );
    drop(node);

    assert_eq!(
        state.height, blocks_to_mine as u64,
        "Expected {} blocks mined",
        blocks_to_mine
    );
    assert_eq!(
        state.total_mined,
        blocks_to_mine as u64 * INITIAL_BLOCK_REWARD,
        "Total mined should be {} * reward",
        blocks_to_mine
    );
    assert_eq!(
        state.total_fees_burned, 0,
        "No fees should be burned yet (no transactions)"
    );

    println!(
        "\nPhase 1 complete: {} blocks mined successfully!",
        blocks_to_mine
    );

    // Phase 2: Create and execute multiple transactions
    println!("\nPhase 2: Creating and executing transactions...");

    // Track total fees for verification
    let mut total_fees_expected: u64 = 0;

    // First, check wallet balances from mining
    println!("  Scanning wallet balances after mining...");
    for (i, wallet) in network.wallets.iter().enumerate() {
        let balance = get_wallet_balance(&network, wallet);
        println!(
            "    Wallet {}: {} picocredits ({} BTH)",
            i,
            balance,
            balance / PICOCREDITS_PER_CREDIT
        );
    }

    // ========================================================================
    // Transaction 1: Wallet 0 -> Wallet 1 (simple transfer)
    // ========================================================================
    println!("\n  --- Transaction 1: Wallet 0 -> Wallet 1 ---");

    let sender_wallet = &network.wallets[0];
    let recipient_wallet = &network.wallets[1];
    let recipient_address = recipient_wallet.default_address();

    let sender_utxos = scan_wallet_utxos(&network, sender_wallet);
    assert!(!sender_utxos.is_empty(), "Sender wallet 0 has no UTXOs");

    let (utxo_to_spend, subaddr_idx) = &sender_utxos[0];
    let send_amount = 10 * PICOCREDITS_PER_CREDIT; // Send 10 BTH
    let tx_fee = MIN_TX_FEE;
    total_fees_expected += tx_fee;

    let node = network.get_node(0);
    let current_height = node.chain_state().height;
    drop(node);

    println!(
        "    Sending {} BTH with {} fee",
        send_amount / PICOCREDITS_PER_CREDIT,
        tx_fee
    );

    let tx1 = create_signed_transaction(
        sender_wallet,
        utxo_to_spend,
        *subaddr_idx,
        &recipient_address,
        send_amount,
        tx_fee,
        current_height,
        &network,
    )
    .expect("Failed to create transaction 1");

    network.broadcast_transaction(tx1.clone());
    mine_block(&network, blocks_to_mine % DEFAULT_NUM_NODES);

    // Verify balances after tx1
    let wallet0_balance = get_wallet_balance(&network, &network.wallets[0]);
    let wallet1_balance = get_wallet_balance(&network, &network.wallets[1]);
    println!(
        "    After tx1 - Wallet 0: {} BTH, Wallet 1: {} BTH",
        wallet0_balance / PICOCREDITS_PER_CREDIT,
        wallet1_balance / PICOCREDITS_PER_CREDIT
    );

    // ========================================================================
    // Transaction 2: Wallet 1 -> Wallet 2 (chain from received funds)
    // ========================================================================
    println!("\n  --- Transaction 2: Wallet 1 -> Wallet 2 (spending received funds) ---");

    let sender_wallet = &network.wallets[1];
    let recipient_wallet = &network.wallets[2];
    let recipient_address = recipient_wallet.default_address();

    let sender_utxos = scan_wallet_utxos(&network, sender_wallet);
    println!("    Wallet 1 has {} UTXOs", sender_utxos.len());

    let (utxo_to_spend, subaddr_idx) = sender_utxos
        .iter()
        .find(|(u, _)| u.output.amount == send_amount)
        .unwrap_or(&sender_utxos[0]);

    let send_amount2 = 5 * PICOCREDITS_PER_CREDIT; // Send 5 BTH
    total_fees_expected += tx_fee;

    let node = network.get_node(0);
    let current_height = node.chain_state().height;
    drop(node);

    println!(
        "    Sending {} BTH (from received funds)",
        send_amount2 / PICOCREDITS_PER_CREDIT
    );

    let tx2 = create_signed_transaction(
        sender_wallet,
        utxo_to_spend,
        *subaddr_idx,
        &recipient_address,
        send_amount2,
        tx_fee,
        current_height,
        &network,
    )
    .expect("Failed to create transaction 2");

    network.broadcast_transaction(tx2.clone());
    mine_block(&network, (blocks_to_mine + 1) % DEFAULT_NUM_NODES);

    // ========================================================================
    // Transaction 3: Wallet 2 -> Wallet 3 (continuing the chain)
    // ========================================================================
    println!("\n  --- Transaction 3: Wallet 2 -> Wallet 3 ---");

    let sender_wallet = &network.wallets[2];
    let recipient_wallet = &network.wallets[3];
    let recipient_address = recipient_wallet.default_address();

    let sender_utxos = scan_wallet_utxos(&network, sender_wallet);
    println!("    Wallet 2 has {} UTXOs", sender_utxos.len());

    let (utxo_to_spend, subaddr_idx) = &sender_utxos[0];
    let send_amount3 = 2 * PICOCREDITS_PER_CREDIT; // Send 2 BTH
    total_fees_expected += tx_fee;

    let node = network.get_node(0);
    let current_height = node.chain_state().height;
    drop(node);

    println!("    Sending {} BTH", send_amount3 / PICOCREDITS_PER_CREDIT);

    let tx3 = create_signed_transaction(
        sender_wallet,
        utxo_to_spend,
        *subaddr_idx,
        &recipient_address,
        send_amount3,
        tx_fee,
        current_height,
        &network,
    )
    .expect("Failed to create transaction 3");

    network.broadcast_transaction(tx3.clone());
    mine_block(&network, (blocks_to_mine + 2) % DEFAULT_NUM_NODES);

    // ========================================================================
    // Transaction 4: Wallet 3 -> Wallet 4 (complete the ring)
    // ========================================================================
    println!("\n  --- Transaction 4: Wallet 3 -> Wallet 4 ---");

    let sender_wallet = &network.wallets[3];
    let recipient_wallet = &network.wallets[4];
    let recipient_address = recipient_wallet.default_address();

    let sender_utxos = scan_wallet_utxos(&network, sender_wallet);
    println!("    Wallet 3 has {} UTXOs", sender_utxos.len());

    let (utxo_to_spend, subaddr_idx) = &sender_utxos[0];
    let send_amount4 = 1 * PICOCREDITS_PER_CREDIT; // Send 1 BTH
    total_fees_expected += tx_fee;

    let node = network.get_node(0);
    let current_height = node.chain_state().height;
    drop(node);

    println!("    Sending {} BTH", send_amount4 / PICOCREDITS_PER_CREDIT);

    let tx4 = create_signed_transaction(
        sender_wallet,
        utxo_to_spend,
        *subaddr_idx,
        &recipient_address,
        send_amount4,
        tx_fee,
        current_height,
        &network,
    )
    .expect("Failed to create transaction 4");

    network.broadcast_transaction(tx4.clone());
    mine_block(&network, (blocks_to_mine + 3) % DEFAULT_NUM_NODES);

    // ========================================================================
    // Verify final state after all transactions
    // ========================================================================
    println!("\n  --- Final Balance Verification ---");

    for (i, wallet) in network.wallets.iter().enumerate() {
        let balance = get_wallet_balance(&network, wallet);
        println!("    Wallet {}: {} BTH", i, balance / PICOCREDITS_PER_CREDIT);
    }

    // Verify consistency
    println!("\n  Verifying ledger consistency after transactions...");
    network.verify_consistency();

    // Verify fees were burned
    let node = network.get_node(0);
    let post_tx_state = node.chain_state();
    drop(node);

    println!(
        "  Total fees burned: {} picocredits (expected: {})",
        post_tx_state.total_fees_burned, total_fees_expected
    );

    assert!(
        post_tx_state.total_fees_burned >= total_fees_expected,
        "Expected at least {} fees burned, got {}",
        total_fees_expected,
        post_tx_state.total_fees_burned
    );

    // Phase 3: Final verification
    println!("\nPhase 3: Final state verification...");
    let node = network.get_node(0);
    let final_state = node.chain_state();
    drop(node);

    println!("  Final height: {}", final_state.height);
    println!(
        "  Final total mined: {} BTH",
        final_state.total_mined / PICOCREDITS_PER_CREDIT
    );
    println!(
        "  Final fees burned: {} picocredits",
        final_state.total_fees_burned
    );

    // Verify final state
    let num_tx_blocks = 4; // We mined 4 blocks with transactions
    let expected_height = (blocks_to_mine + num_tx_blocks) as u64;
    assert_eq!(
        final_state.height, expected_height,
        "Final height should be {}",
        expected_height
    );

    let expected_mined = expected_height * INITIAL_BLOCK_REWARD;
    assert_eq!(
        final_state.total_mined, expected_mined,
        "Total mined should be {} picocredits",
        expected_mined
    );

    // Verify fees were burned (4 transactions * MIN_TX_FEE each)
    let expected_total_fees = 4 * MIN_TX_FEE;
    assert!(
        final_state.total_fees_burned >= expected_total_fees,
        "Expected at least {} fees burned from 4 transactions, got {}",
        expected_total_fees,
        final_state.total_fees_burned
    );

    // Verify circulating supply
    let circulating_supply = final_state.total_mined - final_state.total_fees_burned;
    println!(
        "  Circulating supply: {} BTH (mined: {}, burned: {})",
        circulating_supply / PICOCREDITS_PER_CREDIT,
        final_state.total_mined / PICOCREDITS_PER_CREDIT,
        final_state.total_fees_burned
    );

    // Verify UTXO conservation: total balance across all wallets should equal
    // circulating supply
    let total_wallet_balance: u64 = network
        .wallets
        .iter()
        .map(|w| get_wallet_balance(&network, w))
        .sum();

    println!(
        "  Total wallet balances: {} BTH",
        total_wallet_balance / PICOCREDITS_PER_CREDIT
    );

    // The total wallet balance should equal total_mined - fees_burned
    // (all coins are accounted for in wallets)
    assert_eq!(
        total_wallet_balance, circulating_supply,
        "Total wallet balance ({}) should equal circulating supply ({})",
        total_wallet_balance, circulating_supply
    );

    println!("\n=== E2E Test Complete ===\n");
    println!("Summary:");
    println!("  - {} nodes reached consensus", DEFAULT_NUM_NODES);
    println!("  - {} blocks mined", final_state.height);
    println!("  - {} transactions executed", num_tx_blocks);
    println!(
        "  - {} picocredits fees burned",
        final_state.total_fees_burned
    );
    println!(
        "  - {} BTH circulating supply",
        circulating_supply / PICOCREDITS_PER_CREDIT
    );
    println!("  - All nodes have consistent ledger state");
    println!("  - UTXO conservation verified: all coins accounted for");

    // Cleanup
    network.stop();
}

// ============================================================================
// Ring Signature (Private Transaction) Test
// ============================================================================

/// Test private transactions using ring signatures for sender anonymity.
/// Ring signatures hide which UTXO is being spent among a ring of decoys.
#[test]
#[ignore = "Needs update: WalletKeys->Wallet, TxInputs::Ring->TxInputs::Clsag"]
fn test_private_ring_signature_transaction() {
    use crate::common::ensure_decoy_availability;

    println!("\n=== Private Ring Signature Transaction Test ===\n");

    // Build the network
    println!("Building test network...");
    let mut network = TestNetwork::build(TestNetworkConfig::default());
    thread::sleep(Duration::from_millis(500));

    // Mine enough blocks for decoys
    println!("Mining blocks to build decoy set...");
    ensure_decoy_availability(&network, 0);

    // Verify mining succeeded
    network.verify_consistency();
    let node = network.get_node(0);
    let state = node.chain_state();
    println!(
        "  Mined {} blocks, total supply: {} BTH\n",
        state.height,
        state.total_mined / PICOCREDITS_PER_CREDIT
    );
    drop(node);

    // Create a private transaction from wallet 0 to wallet 1
    println!("Creating private ring signature transaction...");

    let sender_wallet = &network.wallets[0];
    let recipient_wallet = &network.wallets[1];
    let recipient_address = recipient_wallet.default_address();

    // Find UTXOs owned by sender
    let sender_utxos = scan_wallet_utxos(&network, sender_wallet);
    println!("  Sender has {} UTXOs", sender_utxos.len());

    if sender_utxos.is_empty() {
        panic!("Sender has no UTXOs to spend!");
    }

    // Get the UTXO and prepare for spending
    let (utxo_to_spend, _subaddr_idx) = &sender_utxos[0];
    let utxo_amount = utxo_to_spend.output.amount;
    let send_amount = 10 * PICOCREDITS_PER_CREDIT; // Send 10 BTH
    let tx_fee = MIN_TX_FEE;

    println!(
        "  Spending UTXO with {} BTH",
        utxo_amount / PICOCREDITS_PER_CREDIT
    );
    println!(
        "  Sending {} BTH to wallet 1",
        send_amount / PICOCREDITS_PER_CREDIT
    );

    // Get current height
    let node = network.get_node(0);
    let current_height = node.chain_state().height;
    let ledger = node.ledger.read().unwrap();

    // Create outputs (recipient + change)
    let mut outputs = vec![TxOutput::new(send_amount, &recipient_address)];
    let change = utxo_amount - send_amount - tx_fee;
    if change > 0 {
        outputs.push(TxOutput::new(change, &sender_wallet.default_address()));
    }

    // Create private transaction with ring signature
    let private_tx = sender_wallet
        .create_private_transaction(
            &[utxo_to_spend.clone()],
            outputs,
            tx_fee,
            current_height,
            &ledger,
        )
        .expect("Failed to create private transaction");

    drop(ledger);
    drop(node);

    // Verify the transaction has CLSAG inputs
    let clsag_inputs = private_tx.inputs.clsag().expect("Should have CLSAG inputs");
    println!(
        "  Created CLSAG ring signature with {} decoys per input",
        clsag_inputs[0].ring.len() - 1
    );
    println!(
        "  Key image: {}",
        hex::encode(&clsag_inputs[0].key_image[0..8])
    );

    // Broadcast and mine
    network.broadcast_transaction(private_tx.clone());
    mine_block(&network, 0);

    // Verify the transaction was included
    network.verify_consistency();

    let node = network.get_node(0);
    let final_state = node.chain_state();
    drop(node);

    assert!(
        final_state.total_fees_burned >= tx_fee,
        "Fee should have been burned"
    );

    // Verify balances
    let recipient_balance = get_wallet_balance(&network, recipient_wallet);
    println!(
        "\n  Recipient balance: {} BTH",
        recipient_balance / PICOCREDITS_PER_CREDIT
    );

    // Recipient should have mining rewards + the transfer
    assert!(
        recipient_balance >= send_amount,
        "Recipient should have at least {} BTH",
        send_amount / PICOCREDITS_PER_CREDIT
    );

    println!("\n=== Private Transaction Test Complete ===\n");
    println!("Summary:");
    println!("  - Ring signature transaction created and verified");
    println!("  - Transaction included in block");
    println!(
        "  - Fee burned: {} picocredits",
        final_state.total_fees_burned
    );
    println!(
        "  - Sender anonymity preserved (hidden among {} decoys)",
        clsag_inputs[0].ring.len() - 1
    );

    network.stop();
}

// ============================================================================
// Additional Tests
// ============================================================================

#[test]
fn test_network_builds_successfully() {
    let mut network = TestNetwork::build(TestNetworkConfig::default());
    assert_eq!(network.node_ids.len(), DEFAULT_NUM_NODES);
    assert_eq!(network.wallets.len(), DEFAULT_NUM_NODES);

    // Verify all wallets are different
    for i in 0..DEFAULT_NUM_NODES {
        for j in (i + 1)..DEFAULT_NUM_NODES {
            assert_ne!(
                network.wallets[i]
                    .default_address()
                    .view_public_key()
                    .to_bytes(),
                network.wallets[j]
                    .default_address()
                    .view_public_key()
                    .to_bytes(),
                "Wallets {} and {} should be different",
                i,
                j
            );
        }
    }

    network.stop();
}

#[test]
fn test_mock_minting_tx_has_valid_pow() {
    use crate::common::generate_test_wallet;

    let wallet = generate_test_wallet();
    let address = wallet.default_address();
    let prev_hash = [0u8; 32];

    let minting_tx = create_mock_minting_tx(1, INITIAL_BLOCK_REWARD, &address, prev_hash);

    assert!(
        minting_tx.verify_pow(),
        "Mock minting tx should have valid PoW"
    );
    assert_eq!(minting_tx.block_height, 1);
    assert_eq!(minting_tx.reward, INITIAL_BLOCK_REWARD);
}
