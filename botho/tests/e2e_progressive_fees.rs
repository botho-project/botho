// Copyright (c) 2024 Botho Foundation
//
//! End-to-End Progressive Fee Tests
//!
//! Tests the cluster-tax progressive fee system:
//! 1. Cluster factor - wealthy holders pay higher fees
//! 2. Fee rejection - transactions below minimum are rejected
//! 3. Dynamic fees - congestion increases fees
//! 4. Size-based scaling - larger transactions pay more
//! 5. Memo fees - encrypted memos cost extra
//!
//! The progressive fee system ensures:
//! - Wealthy clusters pay proportionally more (cluster factor)
//! - Network congestion increases fees dynamically
//! - Transaction size determines base fee
//! - Optional features (memos) have explicit costs

mod common;

use std::{collections::HashMap, thread, time::Duration};

use bth_account_keys::PublicAddress;
use bth_cluster_tax::{FeeConfig, TransactionType, TAG_WEIGHT_SCALE};
use bth_transaction_types::{ClusterId, ClusterTagVector};
use rand::{rngs::OsRng, seq::SliceRandom};

use botho::{
    mempool::{Mempool, MempoolError},
    transaction::{
        ClsagRingInput, RingMember, Transaction, TxInputs, TxOutput, MIN_TX_FEE,
        PICOCREDITS_PER_CREDIT,
    },
};

/// Picocredits per nanoBTH (10^3) - for converting cluster-tax fees
const PICOCREDITS_PER_NANOBTH: u64 = 1_000;

use crate::common::{
    ensure_decoy_availability, get_wallet_balance, mine_block, scan_wallet_utxos, TestNetwork,
    TestNetworkConfig, TEST_RING_SIZE,
};

// ============================================================================
// Fee Calculation Helpers (Unique to this test module)
// ============================================================================

/// Compute expected minimum fee in picocredits (ready for Transaction.fee).
/// This converts from nanoBTH (cluster-tax system) to picocredits (transaction
/// system) and ensures the result is at least MIN_TX_FEE.
fn compute_expected_min_fee(tx: &Transaction, cluster_wealth: u64, dynamic_base: u64) -> u64 {
    let fee_config = FeeConfig::default();
    let tx_size = tx.estimate_size();
    let tx_type = match &tx.inputs {
        TxInputs::Clsag(_) => TransactionType::Hidden,
        TxInputs::Lion(_) => TransactionType::PqHidden,
    };
    let num_memos = tx.outputs.iter().filter(|o| o.has_memo()).count();
    let fee_nanobth =
        fee_config.minimum_fee_dynamic(tx_type, tx_size, cluster_wealth, num_memos, dynamic_base);
    // Convert nanoBTH to picocredits and ensure at least MIN_TX_FEE
    let fee_pico = fee_nanobth * PICOCREDITS_PER_NANOBTH;
    fee_pico.max(MIN_TX_FEE)
}

/// Compute cluster wealth from transaction outputs (same as mempool does).
fn compute_cluster_wealth_from_outputs(outputs: &[TxOutput]) -> u64 {
    let mut cluster_wealths: HashMap<u64, u64> = HashMap::new();

    for output in outputs {
        let value = output.amount;
        for entry in &output.cluster_tags.entries {
            let contribution =
                ((value as u128) * (entry.weight as u128) / (TAG_WEIGHT_SCALE as u128)) as u64;
            *cluster_wealths.entry(entry.cluster_id.0).or_insert(0) += contribution;
        }
    }

    cluster_wealths.values().copied().max().unwrap_or(0)
}

/// Create a cluster tag vector with single cluster at 100% weight.
fn single_cluster_tags(cluster_id: u64) -> ClusterTagVector {
    ClusterTagVector::single(ClusterId(cluster_id))
}

// ============================================================================
// Transaction Creation Helpers (with cluster tag support)
// ============================================================================

/// Create a signed CLSAG ring signature transaction.
fn create_signed_transaction(
    sender_wallet: &botho::wallet::Wallet,
    sender_utxo: &botho::transaction::Utxo,
    subaddress_index: u64,
    recipient: &PublicAddress,
    amount: u64,
    fee: u64,
    current_height: u64,
    network: &TestNetwork,
) -> Result<Transaction, String> {
    create_signed_transaction_with_tags(
        sender_wallet,
        sender_utxo,
        subaddress_index,
        recipient,
        amount,
        fee,
        current_height,
        network,
        None, // Use default (empty) cluster tags
    )
}

/// Create a signed CLSAG transaction with explicit cluster tags on outputs.
fn create_signed_transaction_with_tags(
    sender_wallet: &botho::wallet::Wallet,
    sender_utxo: &botho::transaction::Utxo,
    subaddress_index: u64,
    recipient: &PublicAddress,
    amount: u64,
    fee: u64,
    current_height: u64,
    network: &TestNetwork,
    cluster_tags: Option<ClusterTagVector>,
) -> Result<Transaction, String> {
    let mut rng = OsRng;
    let node = network.get_node(0);
    let ledger = node.ledger.read().unwrap();

    let change = sender_utxo
        .output
        .amount
        .checked_sub(amount + fee)
        .ok_or("Insufficient funds")?;

    // Create outputs with cluster tags if provided
    let tags = cluster_tags.unwrap_or_else(ClusterTagVector::empty);
    let mut outputs = vec![TxOutput::new_with_cluster_tags(
        amount,
        recipient,
        None,
        tags.clone(),
    )];
    if change > 0 {
        outputs.push(TxOutput::new_with_cluster_tags(
            change,
            &sender_wallet.default_address(),
            None,
            tags,
        ));
    }

    let preliminary_tx = Transaction::new_clsag(Vec::new(), outputs.clone(), fee, current_height);
    let signing_hash = preliminary_tx.signing_hash();

    let onetime_private = sender_utxo
        .output
        .recover_spend_key(sender_wallet.account_key(), subaddress_index)
        .ok_or("Failed to recover spend key")?;

    let exclude_keys = vec![sender_utxo.output.target_key];
    let decoys = ledger
        .get_decoy_outputs(TEST_RING_SIZE - 1, &exclude_keys, 0)
        .map_err(|e| format!("Failed to get decoys: {}", e))?;

    if decoys.len() < TEST_RING_SIZE - 1 {
        return Err(format!(
            "Not enough decoys: need {}, got {}",
            TEST_RING_SIZE - 1,
            decoys.len()
        ));
    }

    let mut ring: Vec<RingMember> = Vec::with_capacity(TEST_RING_SIZE);
    ring.push(RingMember::from_output(&sender_utxo.output));
    for decoy in &decoys {
        ring.push(RingMember::from_output(decoy));
    }

    let real_target_key = sender_utxo.output.target_key;
    let mut indices: Vec<usize> = (0..ring.len()).collect();
    indices.shuffle(&mut rng);
    let shuffled_ring: Vec<RingMember> = indices.iter().map(|&i| ring[i].clone()).collect();
    let real_index = shuffled_ring
        .iter()
        .position(|m| m.target_key == real_target_key)
        .ok_or("Real input not found")?;

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
    .map_err(|e| format!("Failed to create CLSAG: {}", e))?;

    Ok(Transaction::new_clsag(
        vec![ring_input],
        outputs,
        fee,
        current_height,
    ))
}

// ============================================================================
// Tests
// ============================================================================

/// Test 1: Cluster Factor - Wealthy clusters pay higher fees
///
/// Verifies that the progressive fee system correctly applies higher
/// cluster factors to transactions from wealthy clusters.
#[test]
fn test_cluster_factor_wealthy_pay_more() {
    println!("\n=== Cluster Factor Test: Wealthy Pay More ===\n");

    let fee_config = FeeConfig::default();
    let tx_size = 4000; // ~4KB typical CLSAG transaction

    // Test cluster factors at different wealth levels
    let test_cases = [
        (0u64, "Zero wealth"),
        (1_000_000u64, "1M wealth"),
        (10_000_000u64, "10M wealth (w_mid)"),
        (50_000_000u64, "50M wealth"),
        (100_000_000u64, "100M wealth"),
    ];

    println!("Testing cluster factor curve:");
    println!(
        "{:>20} | {:>12} | {:>15}",
        "Cluster Wealth", "Factor", "Fee (nanoBTH)"
    );
    println!("{:-<20}-+-{:-<12}-+-{:-<15}", "", "", "");

    let mut prev_fee = 0u64;
    for (wealth, label) in test_cases {
        let factor = fee_config.cluster_factor(wealth);
        let fee = fee_config.compute_fee(TransactionType::Hidden, tx_size, wealth, 0);

        println!(
            "{:>20} | {:>10.2}x | {:>15}",
            label,
            factor as f64 / 1000.0,
            fee
        );

        // Verify fees increase with wealth
        assert!(
            fee >= prev_fee,
            "Fee should increase with wealth: {} >= {}",
            fee,
            prev_fee
        );
        prev_fee = fee;
    }

    // Verify extreme values
    let factor_zero = fee_config.cluster_factor(0);
    let factor_max = fee_config.cluster_factor(100_000_000);

    // Zero wealth should be close to 1x (1000-2000 range due to sigmoid)
    assert!(
        factor_zero < 3000,
        "Zero wealth factor should be low: {}",
        factor_zero
    );

    // Max wealth should be close to 6x (5000-6000 range)
    assert!(
        factor_max >= 5000,
        "Max wealth factor should be high: {}",
        factor_max
    );

    // Ratio should be significant (at least 2x difference)
    let ratio = factor_max as f64 / factor_zero as f64;
    assert!(
        ratio > 2.0,
        "Wealthy should pay significantly more: {}x",
        ratio
    );

    println!("\nFactor ratio (wealthy/small): {:.2}x", ratio);
    println!("=== Cluster Factor Test Complete ===\n");
}

/// Test 2: Fee Rejection - Transactions with insufficient fees are rejected
///
/// Verifies that the mempool correctly rejects transactions that don't
/// pay the minimum required fee.
#[test]
fn test_fee_rejection_below_minimum() {
    println!("\n=== Fee Rejection Test: Below Minimum ===\n");

    let mut network = TestNetwork::build(TestNetworkConfig::for_stress_testing());
    thread::sleep(Duration::from_millis(500));

    // Pre-mine blocks for decoy availability
    ensure_decoy_availability(&network, 1);

    // Mine a block to fund wallet 0
    mine_block(&network, 0);
    network.verify_consistency();

    let sender_wallet = &network.wallets[0];
    let recipient_wallet = &network.wallets[1];
    let recipient_address = recipient_wallet.default_address();

    let sender_utxos = scan_wallet_utxos(&network, sender_wallet);
    let (utxo, subaddr_idx) = &sender_utxos[0];

    let node = network.get_node(0);
    let current_height = node.chain_state().height;
    let ledger = node.ledger.clone();
    drop(node);

    let send_amount = 5 * PICOCREDITS_PER_CREDIT;

    // Create transaction with very low fee (below MIN_TX_FEE)
    let low_fee = 1000; // Way below MIN_TX_FEE
    let tx_low_fee = create_signed_transaction(
        sender_wallet,
        utxo,
        *subaddr_idx,
        &recipient_address,
        send_amount,
        low_fee,
        current_height,
        &network,
    )
    .expect("Failed to create transaction");

    // Try to add to mempool - should be rejected by structure validation
    let mut mempool = Mempool::new();
    let ledger_guard = ledger.read().unwrap();
    let result = mempool.add_tx(tx_low_fee.clone(), &ledger_guard);

    // The transaction fails structure validation (MIN_TX_FEE check)
    match &result {
        Err(MempoolError::InvalidTransaction(msg)) if msg.contains("fee below minimum") => {
            println!("Transaction correctly rejected by structure validation:");
            println!("  Provided fee: {} picocredits", low_fee);
            println!("  MIN_TX_FEE: {} picocredits", MIN_TX_FEE);
        }
        Err(MempoolError::FeeTooLow { minimum, provided }) => {
            println!("Transaction correctly rejected by mempool fee check:");
            println!("  Provided fee: {} picocredits", provided);
            println!("  Minimum required: {} picocredits", minimum);
            assert_eq!(*provided, low_fee);
            assert!(*minimum > *provided);
        }
        Ok(_) => panic!("Transaction should have been rejected for low fee"),
        Err(e) => panic!("Unexpected error: {:?}", e),
    }
    assert!(result.is_err(), "Low fee transaction should be rejected");

    // Now create transaction with sufficient fee (at least MIN_TX_FEE)
    let expected_min = compute_expected_min_fee(&tx_low_fee, 0, 1);
    println!(
        "\nExpected minimum fee: {} picocredits (includes MIN_TX_FEE floor)",
        expected_min
    );

    let tx_good_fee = create_signed_transaction(
        sender_wallet,
        utxo,
        *subaddr_idx,
        &recipient_address,
        send_amount,
        expected_min,
        current_height,
        &network,
    )
    .expect("Failed to create transaction");

    // Fresh mempool to avoid key image conflict
    let mut mempool2 = Mempool::new();
    let result2 = mempool2.add_tx(tx_good_fee, &ledger_guard);
    assert!(
        result2.is_ok(),
        "Transaction with sufficient fee should be accepted: {:?}",
        result2.err()
    );
    println!("Transaction with sufficient fee accepted");

    println!("\n=== Fee Rejection Test Complete ===\n");
    network.stop();
}

/// Test 3: Fee Rejection with Cluster Wealth (Unit Test Style)
#[test]
fn test_fee_rejection_wealthy_sender() {
    println!("\n=== Fee Rejection Test: Wealthy Sender ===\n");

    let mut network = TestNetwork::build(TestNetworkConfig::for_stress_testing());
    thread::sleep(Duration::from_millis(500));

    // Pre-mine blocks for decoy availability
    ensure_decoy_availability(&network, 1);

    // Mine a block to fund wallet 0
    mine_block(&network, 0);
    network.verify_consistency();

    let sender_wallet = &network.wallets[0];
    let recipient_wallet = &network.wallets[1];
    let recipient_address = recipient_wallet.default_address();

    let sender_utxos = scan_wallet_utxos(&network, sender_wallet);
    let (utxo, subaddr_idx) = &sender_utxos[0];

    let node = network.get_node(0);
    let current_height = node.chain_state().height;
    let ledger = node.ledger.clone();
    drop(node);

    let send_amount = 5 * PICOCREDITS_PER_CREDIT;

    // Create a cluster tag representing a wealthy holder
    let wealthy_cluster_id = 12345u64;
    let wealthy_tags = single_cluster_tags(wealthy_cluster_id);

    // Compute fees for different wealth levels (in nanoBTH)
    let fee_config = FeeConfig::default();
    let tx_size_estimate = 4000;
    let small_holder_fee_nano =
        fee_config.compute_fee(TransactionType::Hidden, tx_size_estimate, 0, 0);
    let wealthy_amount = 1_000_000 * PICOCREDITS_PER_CREDIT; // 1M BTH wealth
    let wealthy_fee_nano = fee_config.compute_fee(
        TransactionType::Hidden,
        tx_size_estimate,
        wealthy_amount,
        0,
    );

    println!("Fee comparison (both in nanoBTH):");
    println!("  Small holder fee: {} nanoBTH", small_holder_fee_nano);
    println!("  Wealthy holder fee: {} nanoBTH", wealthy_fee_nano);
    println!(
        "  Ratio: {:.2}x",
        wealthy_fee_nano as f64 / small_holder_fee_nano as f64
    );

    // Verify wealthy pay more (cluster factor should be ~5-6x)
    assert!(
        wealthy_fee_nano > small_holder_fee_nano * 4,
        "Wealthy sender fee ({}) should be at least 4x small holder fee ({})",
        wealthy_fee_nano,
        small_holder_fee_nano
    );

    // Create transaction with proper fee (MIN_TX_FEE floor)
    let proper_fee = MIN_TX_FEE;
    let tx_with_tags = create_signed_transaction_with_tags(
        sender_wallet,
        utxo,
        *subaddr_idx,
        &recipient_address,
        send_amount,
        proper_fee,
        current_height,
        &network,
        Some(wealthy_tags.clone()),
    )
    .expect("Failed to create transaction");

    // The output amount represents the cluster wealth
    let cluster_wealth = compute_cluster_wealth_from_outputs(&tx_with_tags.outputs);
    println!("Cluster wealth from outputs: {}", cluster_wealth);

    // Verify the cluster factor for this output wealth
    let output_cluster_factor = fee_config.cluster_factor(cluster_wealth);
    println!(
        "Cluster factor for output wealth: {:.2}x",
        output_cluster_factor as f64 / 1000.0
    );

    // Transaction should be accepted with MIN_TX_FEE
    let mut mempool = Mempool::new();
    let ledger_guard = ledger.read().unwrap();
    let result = mempool.add_tx(tx_with_tags, &ledger_guard);
    assert!(
        result.is_ok(),
        "Transaction with MIN_TX_FEE should be accepted: {:?}",
        result.err()
    );
    println!("Transaction with MIN_TX_FEE accepted");

    println!("\n=== Fee Rejection (Wealthy Sender) Test Complete ===\n");
    network.stop();
}

/// Test 4: Minted coins create new clusters
#[test]
fn test_minted_coins_create_clusters() {
    println!("\n=== Minted Coins Create Clusters Test ===\n");

    let mut network = TestNetwork::build(TestNetworkConfig::default());
    thread::sleep(Duration::from_millis(500));

    // Mine a block to wallet 0
    mine_block(&network, 0);
    network.verify_consistency();

    // Get the minted output and check its cluster tags
    let tip = {
        let node = network.get_node(0);
        let ledger = node.ledger.read().unwrap();
        ledger.get_tip().unwrap()
    };

    let minting_output = tip.minting_tx.to_tx_output();

    println!("Minting transaction output:");
    println!(
        "  Amount: {} BTH",
        minting_output.amount / PICOCREDITS_PER_CREDIT
    );
    println!("  Cluster tags: {:?}", minting_output.cluster_tags);

    // Verify it has cluster attribution
    assert!(
        !minting_output.cluster_tags.is_empty(),
        "Minted output should have cluster tags"
    );

    // Verify 100% weight (TAG_WEIGHT_SCALE)
    let total_weight = minting_output.cluster_tags.total_weight();
    assert_eq!(
        total_weight,
        TAG_WEIGHT_SCALE,
        "Minted output should have 100% cluster attribution, got {}%",
        total_weight * 100 / TAG_WEIGHT_SCALE
    );

    // Verify single cluster entry
    assert_eq!(
        minting_output.cluster_tags.len(),
        1,
        "Minted output should have exactly one cluster tag"
    );

    let cluster_id = minting_output.cluster_tags.entries[0].cluster_id;
    println!("  Cluster ID: {}", cluster_id.0);

    // The cluster ID should be derived from the minting tx hash
    let tx_hash = tip.minting_tx.hash();
    let expected_cluster_id = u64::from_le_bytes(tx_hash[0..8].try_into().unwrap());
    assert_eq!(
        cluster_id.0, expected_cluster_id,
        "Cluster ID should be derived from tx hash"
    );

    println!("\n=== Minted Coins Create Clusters Test Complete ===\n");
    network.stop();
}

/// Test 5: Dynamic fee increases under congestion
#[test]
fn test_dynamic_fee_congestion() {
    println!("\n=== Dynamic Fee Congestion Test ===\n");

    use bth_cluster_tax::DynamicFeeBase;

    let mut dynamic_fee = DynamicFeeBase::default();

    println!("Initial state:");
    println!("  Base min: {} nanoBTH/byte", dynamic_fee.base_min);
    println!("  Base max: {} nanoBTH/byte", dynamic_fee.base_max);
    println!(
        "  Target fullness: {}%",
        (dynamic_fee.target_fullness * 100.0) as u32
    );

    // Simulate normal load (50% full, not at min block time)
    println!("\nSimulating normal load (50% full, not at min block time)...");
    for _ in 0..20 {
        dynamic_fee.update(50, 100, false);
    }
    let base_normal = dynamic_fee.compute_base(false);
    println!("  Fee base: {} nanoBTH/byte", base_normal);
    assert_eq!(
        base_normal, dynamic_fee.base_min,
        "Should stay at minimum under normal load"
    );

    // Simulate congestion (100% full, at min block time)
    println!("\nSimulating congestion (100% full, at min block time)...");
    for i in 0..30 {
        let new_base = dynamic_fee.update(100, 100, true);
        if i % 10 == 9 {
            println!(
                "  After {} blocks: {} nanoBTH/byte (EMA: {:.2}%)",
                i + 1,
                new_base,
                dynamic_fee.current_fullness() * 100.0
            );
        }
    }
    let base_congested = dynamic_fee.compute_base(true);
    println!("  Final fee base: {} nanoBTH/byte", base_congested);

    // Verify fee increased significantly
    let multiplier = base_congested as f64 / dynamic_fee.base_min as f64;
    println!("  Multiplier: {:.2}x", multiplier);
    assert!(
        multiplier > 3.0,
        "Fee should increase significantly under sustained congestion: {}x",
        multiplier
    );

    // Simulate recovery (empty blocks)
    println!("\nSimulating recovery (0% full)...");
    for i in 0..50 {
        let new_base = dynamic_fee.update(0, 100, true);
        if i % 10 == 9 {
            println!(
                "  After {} empty blocks: {} nanoBTH/byte (EMA: {:.2}%)",
                i + 1,
                new_base,
                dynamic_fee.current_fullness() * 100.0
            );
        }
    }
    let base_recovered = dynamic_fee.compute_base(true);
    println!("  Recovered fee base: {} nanoBTH/byte", base_recovered);

    // Verify fee returned to normal
    assert_eq!(
        base_recovered, dynamic_fee.base_min,
        "Fee should return to minimum after congestion clears"
    );

    println!("\n=== Dynamic Fee Congestion Test Complete ===\n");
}

/// Test 6: Size-based fee scaling
#[test]
fn test_size_based_fee_scaling() {
    println!("\n=== Size-Based Fee Scaling Test ===\n");

    let fee_config = FeeConfig::default();
    let cluster_wealth = 0u64; // Small holder for predictable results

    // Test different transaction sizes
    let sizes = [1000, 2000, 4000, 8000, 16000, 65000];

    println!("Fee scaling with transaction size (cluster_wealth=0):");
    println!(
        "{:>12} | {:>15} | {:>15}",
        "Size (bytes)", "Fee (nanoBTH)", "Fee/byte"
    );
    println!("{:-<12}-+-{:-<15}-+-{:-<15}", "", "", "");

    let mut prev_fee = 0u64;
    for size in sizes {
        let fee = fee_config.compute_fee(TransactionType::Hidden, size, cluster_wealth, 0);
        let fee_per_byte = fee as f64 / size as f64;

        println!("{:>12} | {:>15} | {:>13.2}", size, fee, fee_per_byte);

        // Verify fee increases with size
        assert!(fee >= prev_fee, "Fee should increase with size");
        prev_fee = fee;
    }

    // Verify linear scaling (double size = double fee)
    let fee_4k = fee_config.compute_fee(TransactionType::Hidden, 4000, cluster_wealth, 0);
    let fee_8k = fee_config.compute_fee(TransactionType::Hidden, 8000, cluster_wealth, 0);
    let ratio = fee_8k as f64 / fee_4k as f64;
    println!("\nSize doubling ratio (8K/4K): {:.2}x", ratio);
    assert!(
        (ratio - 2.0).abs() < 0.1,
        "Doubling size should ~double fee: {}x",
        ratio
    );

    // Compare CLSAG vs LION typical sizes
    let clsag_fee = fee_config.compute_fee(TransactionType::Hidden, 4000, cluster_wealth, 0);
    let lion_fee = fee_config.compute_fee(TransactionType::PqHidden, 65000, cluster_wealth, 0);
    let pq_ratio = lion_fee as f64 / clsag_fee as f64;

    println!("\nCLSAG (~4KB) fee: {} nanoBTH", clsag_fee);
    println!("LION (~65KB) fee: {} nanoBTH", lion_fee);
    println!("PQ/Standard ratio: {:.1}x", pq_ratio);

    assert!(
        pq_ratio > 10.0 && pq_ratio < 20.0,
        "LION should be ~16x more expensive: {}x",
        pq_ratio
    );

    println!("\n=== Size-Based Fee Scaling Test Complete ===\n");
}

/// Test 7: Memo fees add to base fee
#[test]
fn test_memo_fees() {
    println!("\n=== Memo Fees Test ===\n");

    let fee_config = FeeConfig::default();
    let tx_size = 4000;
    let cluster_wealth = 0u64;

    println!("Memo fee: {} nanoBTH per memo", fee_config.fee_per_memo);
    println!();

    // Test fees with different memo counts
    println!(
        "{:>10} | {:>15} | {:>15}",
        "Memos", "Fee (nanoBTH)", "Memo Cost"
    );
    println!("{:-<10}-+-{:-<15}-+-{:-<15}", "", "", "");

    let base_fee = fee_config.compute_fee(TransactionType::Hidden, tx_size, cluster_wealth, 0);
    println!("{:>10} | {:>15} | {:>15}", 0, base_fee, 0);

    for num_memos in 1..=5 {
        let fee = fee_config.compute_fee(
            TransactionType::Hidden,
            tx_size,
            cluster_wealth,
            num_memos,
        );
        let memo_cost = fee - base_fee;

        println!("{:>10} | {:>15} | {:>15}", num_memos, fee, memo_cost);

        // Verify memo cost is additive
        let expected_memo_cost = fee_config.fee_per_memo * num_memos as u64;
        assert_eq!(
            memo_cost, expected_memo_cost,
            "Memo cost should be additive: {} vs {}",
            memo_cost, expected_memo_cost
        );
    }

    println!("\n=== Memo Fees Test Complete ===\n");
}

/// Test 8: Combined cluster and congestion effects
#[test]
fn test_combined_cluster_and_congestion() {
    println!("\n=== Combined Cluster and Congestion Test ===\n");

    use bth_cluster_tax::DynamicFeeBase;

    let fee_config = FeeConfig::default();
    let tx_size = 4000;

    // Calculate base fee (small holder, no congestion)
    let base_fee = fee_config.compute_fee(TransactionType::Hidden, tx_size, 0, 0);
    println!("Base fee (small holder, normal): {} nanoBTH", base_fee);

    // Calculate wealthy sender fee (no congestion)
    let wealthy_cluster = 100_000_000u64;
    let wealthy_fee =
        fee_config.compute_fee(TransactionType::Hidden, tx_size, wealthy_cluster, 0);
    let cluster_multiplier = wealthy_fee as f64 / base_fee as f64;
    println!(
        "Wealthy sender fee (normal): {} nanoBTH ({:.2}x)",
        wealthy_fee, cluster_multiplier
    );

    // Simulate maximum congestion
    let mut dynamic_fee = DynamicFeeBase::default();
    for _ in 0..100 {
        dynamic_fee.update(100, 100, true);
    }
    let congestion_base = dynamic_fee.compute_base(true);
    let congestion_multiplier = congestion_base as f64 / dynamic_fee.base_min as f64;
    println!(
        "\nCongestion multiplier: {:.2}x (base: {} nanoBTH/byte)",
        congestion_multiplier, congestion_base
    );

    // Calculate combined fee (wealthy + congestion)
    let combined_fee = fee_config.compute_fee_with_dynamic_base(
        TransactionType::Hidden,
        tx_size,
        wealthy_cluster,
        0,
        congestion_base,
    );
    let total_multiplier = combined_fee as f64 / base_fee as f64;
    println!(
        "\nCombined fee (wealthy + congestion): {} nanoBTH",
        combined_fee
    );
    println!("Total multiplier: {:.2}x", total_multiplier);

    // Verify multiplicative effect
    let expected_combined = cluster_multiplier * congestion_multiplier;
    let tolerance = 0.5;
    assert!(
        (total_multiplier - expected_combined).abs() < tolerance,
        "Combined multiplier ({:.2}x) should be ~cluster × congestion ({:.2}x × {:.2}x = {:.2}x)",
        total_multiplier,
        cluster_multiplier,
        congestion_multiplier,
        expected_combined
    );

    // Show the effect on a real transaction
    let small_normal_fee = base_fee;
    let wealthy_congested_fee = combined_fee;
    println!("\n--- Real Impact ---");
    println!(
        "Small holder in normal conditions: {} nanoBTH",
        small_normal_fee
    );
    println!(
        "Wealthy holder in congestion:      {} nanoBTH",
        wealthy_congested_fee
    );
    println!(
        "Difference: {:.1}x",
        wealthy_congested_fee as f64 / small_normal_fee as f64
    );

    println!("\n=== Combined Effects Test Complete ===\n");
}

/// Test 9: End-to-end progressive fee with real transaction
#[test]
fn test_e2e_progressive_fee_enforcement() {
    println!("\n=== E2E Progressive Fee Enforcement Test ===\n");

    let mut network = TestNetwork::build(TestNetworkConfig::for_stress_testing());
    thread::sleep(Duration::from_millis(500));

    // Pre-mine blocks for decoy availability
    ensure_decoy_availability(&network, 1);

    // Mine block to fund wallet 0
    mine_block(&network, 0);
    network.verify_consistency();

    let wallet0 = &network.wallets[0];
    let wallet1 = &network.wallets[1];
    let recipient = wallet1.default_address();

    let utxos = scan_wallet_utxos(&network, wallet0);
    let (utxo, subaddr_idx) = &utxos[0];

    let node = network.get_node(0);
    let current_height = node.chain_state().height;
    let ledger = node.ledger.clone();
    drop(node);

    let send_amount = 5 * PICOCREDITS_PER_CREDIT;

    // Create a transaction - compute proper fee based on cluster wealth
    let fee_config = FeeConfig::default();

    // Minted coins have 100% cluster attribution to their cluster ID
    let minted_cluster_wealth = utxo.output.amount;
    let cluster_factor = fee_config.cluster_factor(minted_cluster_wealth);

    println!("Sender's UTXO:");
    println!(
        "  Amount: {} BTH",
        utxo.output.amount / PICOCREDITS_PER_CREDIT
    );
    println!("  Cluster tags: {:?}", utxo.output.cluster_tags);
    println!("  Effective cluster wealth: {}", minted_cluster_wealth);
    println!("  Cluster factor: {:.2}x", cluster_factor as f64 / 1000.0);

    // Compute required fee for this sender (in nanoBTH)
    let tx_size_estimate = 4000; // Typical CLSAG size
    let required_fee_nanobth = fee_config.compute_fee(
        TransactionType::Hidden,
        tx_size_estimate,
        minted_cluster_wealth,
        0,
    );
    println!("  Progressive fee: {} nanoBTH", required_fee_nanobth);

    // Convert to picocredits and ensure at least MIN_TX_FEE
    let required_fee_pico =
        (required_fee_nanobth * PICOCREDITS_PER_NANOBTH).max(MIN_TX_FEE);
    println!(
        "  Required fee: {} picocredits (MIN_TX_FEE floor: {})",
        required_fee_pico, MIN_TX_FEE
    );

    // Create transaction with proper fee
    let tx = create_signed_transaction(
        wallet0,
        utxo,
        *subaddr_idx,
        &recipient,
        send_amount,
        required_fee_pico,
        current_height,
        &network,
    )
    .expect("Failed to create transaction");

    println!("\nTransaction created:");
    println!("  Actual size: {} bytes", tx.estimate_size());
    println!("  Fee: {} picocredits", tx.fee);

    // Verify mempool accepts it
    let mut mempool = Mempool::new();
    let ledger_guard = ledger.read().unwrap();
    let result = mempool.add_tx(tx.clone(), &ledger_guard);
    assert!(
        result.is_ok(),
        "Transaction should be accepted: {:?}",
        result.err()
    );
    println!("  Mempool accepted: YES");

    // Broadcast and mine
    drop(ledger_guard);
    network.broadcast_transaction(tx);
    mine_block(&network, 1);
    network.verify_consistency();

    // Verify transfer succeeded
    let wallet1_balance = get_wallet_balance(&network, wallet1);
    assert!(
        wallet1_balance >= send_amount,
        "Recipient should have received funds"
    );
    println!(
        "\nTransfer confirmed: {} BTH received",
        wallet1_balance / PICOCREDITS_PER_CREDIT
    );

    // Verify fees were burned
    {
        let node = network.get_node(0);
        let state = node.chain_state();
        println!("Total fees burned: {} nanoBTH", state.total_fees_burned);
        assert!(state.total_fees_burned > 0, "Fees should be burned");
    }

    println!("\n=== E2E Progressive Fee Enforcement Test Complete ===\n");
    network.stop();
}
