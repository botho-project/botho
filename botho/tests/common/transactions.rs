// Copyright (c) 2024 Botho Foundation
//
//! Transaction building utilities for test networks.
//!
//! Provides helpers for creating CLSAG ring signature transactions
//! with proper decoy selection.

use bth_account_keys::PublicAddress;
use bth_util_from_random::OsRng;
use rand::seq::SliceRandom;

use botho::{
    transaction::{ClsagRingInput, RingMember, Transaction, TxOutput, Utxo},
    wallet::Wallet,
};

use crate::common::{TestNetwork, TEST_RING_SIZE};

/// A hybrid ML-KEM output at its real `index` (#1677: `Ledger::add_block`
/// enforces the ciphertext shape SCP's transfer gate requires, so classical
/// outputs are invalid on a KEM-enforcing chain). A recipient that publishes
/// no ML-KEM key keeps a classical output.
fn output(amount: u64, recipient: &PublicAddress, index: usize) -> TxOutput {
    TxOutput::new_hybrid_to_address(amount, recipient, index as u32, None, Default::default())
        .unwrap_or_else(|_| TxOutput::new(amount, recipient))
}

/// Create a signed CLSAG ring signature transaction.
///
/// This creates a transaction spending a single UTXO with proper ring signature
/// privacy. Decoys are selected from the ledger automatically.
///
/// # Arguments
///
/// * `sender_wallet` - The wallet spending the UTXO
/// * `sender_utxo` - The UTXO to spend
/// * `subaddress_index` - The subaddress index that received the UTXO
/// * `recipient` - The address to send to
/// * `amount` - Amount to send (must be less than UTXO amount minus fee)
/// * `fee` - Transaction fee
/// * `current_height` - Current chain height (used for tx metadata)
/// * `network` - The test network (for decoy selection)
fn build_signed_transaction(
    sender_wallet: &Wallet,
    sender_utxo: &Utxo,
    subaddress_index: u64,
    recipient: &PublicAddress,
    amount: u64,
    fee: u64,
    current_height: u64,
    network: &TestNetwork,
) -> Result<Transaction, String> {
    let mut rng = OsRng;
    let node = network.get_node(0);
    let ledger = node.ledger.read().unwrap();

    // Calculate change
    let change = sender_utxo
        .output
        .amount
        .checked_sub(amount + fee)
        .ok_or("Insufficient funds")?;

    // Build outputs
    let mut outputs = vec![output(amount, recipient, 0)];
    if change > 0 {
        outputs.push(output(change, &sender_wallet.default_address(), 1));
    }

    // Create preliminary tx to get signing hash
    let preliminary_tx = Transaction::new_clsag(Vec::new(), outputs.clone(), fee, current_height);
    let signing_hash = preliminary_tx.signing_hash();

    // Recover the one-time private key for signing
    let onetime_private = sender_wallet
        .recover_output_spend_key(
            &sender_utxo.output,
            subaddress_index,
            sender_utxo.id.output_index,
        )
        .ok_or("Failed to recover spend key")?;

    // Get decoys (excluding our real input)
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

    // Build ring with real input and decoys
    let mut ring: Vec<RingMember> = Vec::with_capacity(TEST_RING_SIZE);
    ring.push(RingMember::from_output(&sender_utxo.output));
    for decoy in &decoys {
        ring.push(RingMember::from_output(decoy));
    }

    // Shuffle ring to hide real input position
    let real_target_key = sender_utxo.output.target_key;
    let mut indices: Vec<usize> = (0..ring.len()).collect();
    indices.shuffle(&mut rng);
    let shuffled_ring: Vec<RingMember> = indices.iter().map(|&i| ring[i].clone()).collect();
    let real_index = shuffled_ring
        .iter()
        .position(|m| m.target_key == real_target_key)
        .ok_or("Real input not found in shuffled ring")?;

    // Create CLSAG ring input. The per-input pseudo-output amount is the real
    // UTXO's amount; the transaction-level balance check enforces the sum.
    let ring_input = ClsagRingInput::new(
        shuffled_ring,
        real_index,
        &onetime_private,
        sender_utxo.output.amount,
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

/// Create a multi-input transaction spending multiple UTXOs.
///
/// This is useful for consolidating multiple UTXOs into one, or for
/// spending multiple inputs to cover a large payment.
///
/// # Arguments
///
/// * `sender_wallet` - The wallet spending the UTXOs
/// * `utxos_to_spend` - Vector of (UTXO, subaddress_index) tuples
/// * `recipient` - The address to send to
/// * `amount` - Amount to send
/// * `fee` - Transaction fee
/// * `current_height` - Current chain height
/// * `network` - The test network
fn build_multi_input_transaction(
    sender_wallet: &Wallet,
    utxos_to_spend: &[(Utxo, u64)],
    recipient: &PublicAddress,
    amount: u64,
    fee: u64,
    current_height: u64,
    network: &TestNetwork,
) -> Result<Transaction, String> {
    let mut rng = OsRng;
    let node = network.get_node(0);
    let ledger = node.ledger.read().unwrap();

    // Calculate total input and change
    let total_input: u64 = utxos_to_spend.iter().map(|(u, _)| u.output.amount).sum();
    let change = total_input
        .checked_sub(amount + fee)
        .ok_or("Insufficient funds")?;

    // Build outputs
    let mut outputs = vec![output(amount, recipient, 0)];
    if change > 0 {
        outputs.push(output(change, &sender_wallet.default_address(), 1));
    }

    // Create preliminary tx to get signing hash
    let preliminary_tx = Transaction::new_clsag(Vec::new(), outputs.clone(), fee, current_height);
    let signing_hash = preliminary_tx.signing_hash();

    // Collect all real input keys for exclusion from decoys
    let exclude_keys: Vec<[u8; 32]> = utxos_to_spend
        .iter()
        .map(|(u, _)| u.output.target_key)
        .collect();

    // Create ring input for each UTXO
    let mut ring_inputs = Vec::new();
    for (utxo, subaddr_idx) in utxos_to_spend {
        let onetime_private = sender_wallet
            .recover_output_spend_key(&utxo.output, *subaddr_idx, utxo.id.output_index)
            .ok_or("Failed to recover spend key")?;

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

        // Build and shuffle ring
        let mut ring: Vec<RingMember> = Vec::with_capacity(TEST_RING_SIZE);
        ring.push(RingMember::from_output(&utxo.output));
        for decoy in &decoys {
            ring.push(RingMember::from_output(decoy));
        }

        let real_target_key = utxo.output.target_key;
        let mut indices: Vec<usize> = (0..ring.len()).collect();
        indices.shuffle(&mut rng);
        let shuffled_ring: Vec<RingMember> = indices.iter().map(|&i| ring[i].clone()).collect();
        let real_index = shuffled_ring
            .iter()
            .position(|m| m.target_key == real_target_key)
            .ok_or("Real input not found")?;

        let ring_input = ClsagRingInput::new(
            shuffled_ring,
            real_index,
            &onetime_private,
            utxo.output.amount,
            &signing_hash,
            &mut rng,
        )
        .map_err(|e| format!("Failed to create CLSAG: {}", e))?;

        ring_inputs.push(ring_input);
    }

    Ok(Transaction::new_clsag(
        ring_inputs,
        outputs,
        fee,
        current_height,
    ))
}

/// Create a split payment transaction sending to multiple recipients.
///
/// # Arguments
///
/// * `sender_wallet` - The wallet spending the UTXO
/// * `sender_utxo` - The UTXO to spend
/// * `subaddress_index` - The subaddress index that received the UTXO
/// * `recipients` - Vector of (address, amount) tuples
/// * `fee` - Transaction fee
/// * `current_height` - Current chain height
/// * `network` - The test network
fn build_split_payment_transaction(
    sender_wallet: &Wallet,
    sender_utxo: &Utxo,
    subaddress_index: u64,
    recipients: &[(&PublicAddress, u64)],
    fee: u64,
    current_height: u64,
    network: &TestNetwork,
) -> Result<Transaction, String> {
    let mut rng = OsRng;
    let node = network.get_node(0);
    let ledger = node.ledger.read().unwrap();

    // Calculate total payment and change
    let total_payment: u64 = recipients.iter().map(|(_, amt)| *amt).sum();
    let change = sender_utxo
        .output
        .amount
        .checked_sub(total_payment + fee)
        .ok_or("Insufficient funds")?;

    // Build outputs for all recipients
    let mut outputs: Vec<TxOutput> = recipients
        .iter()
        .enumerate()
        .map(|(i, (addr, amt))| output(*amt, addr, i))
        .collect();

    if change > 0 {
        let index = outputs.len();
        outputs.push(output(change, &sender_wallet.default_address(), index));
    }

    // Create preliminary tx to get signing hash
    let preliminary_tx = Transaction::new_clsag(Vec::new(), outputs.clone(), fee, current_height);
    let signing_hash = preliminary_tx.signing_hash();

    let onetime_private = sender_wallet
        .recover_output_spend_key(
            &sender_utxo.output,
            subaddress_index,
            sender_utxo.id.output_index,
        )
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

    let ring_input = ClsagRingInput::new(
        shuffled_ring,
        real_index,
        &onetime_private,
        sender_utxo.output.amount,
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

/// The demurrage part of the consensus fee floor for `tx` in the next block:
/// `consensus_fee_floor − size/factor base`.
///
/// Since protocol 10.0.0 (#1685) the capitalized class-exit charge (the #925
/// downgrade charge) applies from block 1. These helpers write background
/// (empty-tag) outputs, so spending a cluster-tagged coinbase is a class
/// downgrade and owes that charge even in epoch 0; before 10.0.0 it was 0
/// until block 6,307,200.
pub fn exit_charge(network: &TestNetwork, tx: &Transaction) -> u64 {
    use bth_cluster_tax::{FeeConfig, TransactionType};
    let node = network.get_node(0);
    let height = node.chain_state().height + 1;
    let ledger = node.ledger.read().unwrap();
    let floor = ledger
        .consensus_fee_floor(tx, height)
        .expect("consensus fee floor");
    let wealth = ledger
        .effective_cluster_wealth_from_outputs(&tx.outputs)
        .expect("claimed wealth");
    let base = FeeConfig::default().minimum_fee_dynamic_with_outputs(
        TransactionType::Hidden,
        tx.estimate_size(),
        wealth,
        tx.outputs.len(),
        tx.outputs.iter().filter(|o| o.has_memo()).count(),
        1, // CONSENSUS_FEE_BASE
    );
    floor.saturating_sub(base)
}

/// Sign with `fee` on top of the transaction's exit charge
/// ([`exit_charge`]): build at `fee`, and if the next block's floor carries an
/// exit charge, re-sign at `fee + charge` plus 1/32 headroom (each signing
/// draws fresh decoys, which moves the ring floor a little). The caller's
/// `fee` stays the part above the charge, so relative fees between test
/// transactions are preserved. Read the paid fee from `tx.fee`.
fn with_exit_charge(
    network: &TestNetwork,
    fee: u64,
    mut build: impl FnMut(u64) -> Result<Transaction, String>,
) -> Result<Transaction, String> {
    let mut paid = fee;
    for _ in 0..8 {
        let tx = build(paid)?;
        let need = fee.saturating_add(exit_charge(network, &tx));
        if paid >= need {
            return Ok(tx);
        }
        paid = need.saturating_add((need - fee) / 32);
    }
    Err("could not settle a fee covering the exit charge".into())
}

/// Create a signed CLSAG ring signature transaction.
///
/// `fee` is paid on top of any class-exit charge the spend owes (see
/// [`exit_charge`]); the paid fee is `tx.fee`. Other arguments as
/// [`build_signed_transaction`].
#[allow(clippy::too_many_arguments)]
pub fn create_signed_transaction(
    sender_wallet: &Wallet,
    sender_utxo: &Utxo,
    subaddress_index: u64,
    recipient: &PublicAddress,
    amount: u64,
    fee: u64,
    current_height: u64,
    network: &TestNetwork,
) -> Result<Transaction, String> {
    with_exit_charge(network, fee, |fee| {
        build_signed_transaction(
            sender_wallet,
            sender_utxo,
            subaddress_index,
            recipient,
            amount,
            fee,
            current_height,
            network,
        )
    })
}

/// Create a multi-input transaction; `fee` is paid on top of any exit charge
/// (see [`create_signed_transaction`]).
pub fn create_multi_input_transaction(
    sender_wallet: &Wallet,
    utxos_to_spend: &[(Utxo, u64)],
    recipient: &PublicAddress,
    amount: u64,
    fee: u64,
    current_height: u64,
    network: &TestNetwork,
) -> Result<Transaction, String> {
    with_exit_charge(network, fee, |fee| {
        build_multi_input_transaction(
            sender_wallet,
            utxos_to_spend,
            recipient,
            amount,
            fee,
            current_height,
            network,
        )
    })
}

/// Create a split payment transaction; `fee` is paid on top of any exit
/// charge (see [`create_signed_transaction`]).
pub fn create_split_payment_transaction(
    sender_wallet: &Wallet,
    sender_utxo: &Utxo,
    subaddress_index: u64,
    recipients: &[(&PublicAddress, u64)],
    fee: u64,
    current_height: u64,
    network: &TestNetwork,
) -> Result<Transaction, String> {
    with_exit_charge(network, fee, |fee| {
        build_split_payment_transaction(
            sender_wallet,
            sender_utxo,
            subaddress_index,
            recipients,
            fee,
            current_height,
            network,
        )
    })
}
