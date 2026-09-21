//! Existing ordinary positive CLSAG fixture, extracted from
//! tx_lifecycle_integration. Callers supply actual persisted decoys; no network
//! submission occurs here.
use super::node::transaction::{Transaction, TxOutput, Utxo, MIN_RING_SIZE};
use botho_wallet::WalletKeys;
use bth_account_keys::PublicAddress;

pub fn with_decoys(
    sender_wallet: &WalletKeys,
    sender_utxo: &Utxo,
    subaddress_index: u64,
    recipient: &PublicAddress,
    amount: u64,
    fee: u64,
    current_height: u64,
    decoys: &[TxOutput],
) -> Transaction {
    use super::node::transaction::{ClsagRingInput, RingMember};
    use bth_util_from_random::OsRng;
    use rand::seq::SliceRandom;

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

    // Recover the one-time private key for the real input.
    //
    // Protocol 6.0.0: a coinbase UTXO is a hybrid ML-KEM stealth output whose
    // one-time key folds in the ML-KEM shared secret bound to the output index,
    // so the classical `recover_spend_key` returns the wrong scalar and the
    // resulting CLSAG signature fails to verify. Route through the unified
    // `recover_spend_key_for` (#970) at the UTXO's own output index — it uses
    // the hybrid recovery for ciphertext-bearing outputs (coinbases) and the
    // classical path for the classical change/received outputs these tests mint.
    let output_index = sender_utxo.id.output_index;
    #[cfg(feature = "pq")]
    let onetime_private = {
        let pq = sender_wallet.pq_account_key();
        sender_utxo
            .output
            .recover_spend_key_for(
                sender_wallet.account_key(),
                pq.pq_kem_keypair(),
                subaddress_index,
                output_index,
            )
            .expect("Failed to recover spend key - UTXO doesn't belong to wallet")
    };
    #[cfg(not(feature = "pq"))]
    let onetime_private = {
        let _ = output_index;
        sender_utxo
            .output
            .recover_spend_key(sender_wallet.account_key(), subaddress_index)
            .expect("Failed to recover spend key - UTXO doesn't belong to wallet")
    };

    let decoys_needed = MIN_RING_SIZE - 1;
    assert!(
        decoys.len() >= decoys_needed,
        "Not enough decoys: need {}, got {}. Mine more blocks first.",
        decoys_needed,
        decoys.len()
    );

    // Build ring: real output + decoys
    let mut ring: Vec<RingMember> = Vec::with_capacity(MIN_RING_SIZE);
    ring.push(RingMember::from_output(&sender_utxo.output));
    for decoy in decoys {
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
    let ring_input = ClsagRingInput::new(
        shuffled_ring,
        real_index,
        &onetime_private,
        sender_utxo.output.amount,
        &signing_hash,
        &mut rng,
    )
    .expect("Failed to create CLSAG ring signature");

    // Create final transaction
    Transaction::new_clsag(vec![ring_input], outputs, fee, current_height)
}
