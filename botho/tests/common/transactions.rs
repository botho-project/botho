// Copyright (c) 2024 Botho Foundation
//
//! Transaction building utilities for test networks.
//!
//! Provides helpers for creating CLSAG ring signature transactions
//! with proper decoy selection.

use bth_account_keys::PublicAddress;
use rand::{rngs::OsRng, seq::SliceRandom};

use botho::{
    transaction::{ClsagRingInput, RingMember, Transaction, TxOutput, Utxo},
    wallet::Wallet,
};

use crate::common::{TestNetwork, TEST_RING_SIZE};

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
    let mut outputs = vec![TxOutput::new(amount, recipient)];
    if change > 0 {
        outputs.push(TxOutput::new(change, &sender_wallet.default_address()));
    }

    // Create preliminary tx to get signing hash
    let preliminary_tx = Transaction::new_clsag(Vec::new(), outputs.clone(), fee, current_height);
    let signing_hash = preliminary_tx.signing_hash();

    // Recover the one-time private key for signing
    let onetime_private = sender_utxo
        .output
        .recover_spend_key(sender_wallet.account_key(), subaddress_index)
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
pub fn create_multi_input_transaction(
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
    let mut outputs = vec![TxOutput::new(amount, recipient)];
    if change > 0 {
        outputs.push(TxOutput::new(change, &sender_wallet.default_address()));
    }

    // Create preliminary tx to get signing hash
    let preliminary_tx = Transaction::new_clsag(Vec::new(), outputs.clone(), fee, current_height);
    let signing_hash = preliminary_tx.signing_hash();
    let total_output = outputs.iter().map(|o| o.amount).sum::<u64>() + fee;

    // Collect all real input keys for exclusion from decoys
    let exclude_keys: Vec<[u8; 32]> = utxos_to_spend
        .iter()
        .map(|(u, _)| u.output.target_key)
        .collect();

    // Create ring input for each UTXO
    let mut ring_inputs = Vec::new();
    for (utxo, subaddr_idx) in utxos_to_spend {
        let onetime_private = utxo
            .output
            .recover_spend_key(sender_wallet.account_key(), *subaddr_idx)
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
            total_output,
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
pub fn create_split_payment_transaction(
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
        .map(|(addr, amt)| TxOutput::new(*amt, addr))
        .collect();

    if change > 0 {
        outputs.push(TxOutput::new(change, &sender_wallet.default_address()));
    }

    // Create preliminary tx to get signing hash
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
