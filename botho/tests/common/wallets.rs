// Copyright (c) 2024 Botho Foundation
//
//! Wallet utilities for test networks.

use bth_crypto_ring_signature::KeyImage;

use botho::{
    transaction::{Utxo, UtxoId},
    wallet::Wallet,
};

use crate::common::TestNetwork;

/// Generate a random wallet for testing using a BIP39 mnemonic.
pub fn generate_test_wallet() -> Wallet {
    use bip39::{Language, Mnemonic, MnemonicType};
    let mnemonic = Mnemonic::new(MnemonicType::Words24, Language::English);
    Wallet::from_mnemonic(mnemonic.phrase()).expect("Failed to create wallet from mnemonic")
}

/// Scan the ledger for all UTXOs belonging to a wallet.
///
/// Returns a vector of (UTXO, subaddress_index) tuples.
/// This scans all blocks from genesis to tip, checking both
/// coinbase outputs and regular transaction outputs.
pub fn scan_wallet_utxos(network: &TestNetwork, wallet: &Wallet) -> Vec<(Utxo, u64)> {
    let mut owned_utxos = Vec::new();

    let node = network.get_node(0);
    let ledger = node.ledger.read().unwrap();
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

            // Check lottery payout outputs. These are minted in add_block at
            // deterministic ids (block_hash, 1 + lottery_index) — the coinbase
            // occupies index 0. The payout inherits the winner's stealth keys,
            // so belongs_to() detects ownership the same way.
            for lottery_idx in 0..block.lottery_outputs.len() {
                let utxo_id = UtxoId::new(block.hash(), (lottery_idx as u32) + 1);
                if let Ok(Some(utxo)) = ledger.get_utxo(&utxo_id) {
                    if let Some(subaddr_idx) = utxo.output.belongs_to(wallet.account_key()) {
                        owned_utxos.push((utxo, subaddr_idx));
                    }
                }
            }
        }
    }

    // Exclude already-spent UTXOs. Ring-signature privacy means spending a
    // UTXO does not remove it from the ledger's UTXO set (which input was
    // spent is hidden) — only its key image is recorded. A naive sum would
    // therefore double-count a spent coinbase and the change it produced.
    // The owner, however, can derive each UTXO's key image from its one-time
    // private key and drop the ones the ledger has seen spent.
    owned_utxos.retain(|(utxo, subaddr_idx)| {
        match utxo
            .output
            .recover_spend_key(wallet.account_key(), *subaddr_idx)
        {
            Some(onetime_private) => {
                let key_image = *KeyImage::from(&onetime_private).as_bytes();
                // Keep the UTXO only if its key image is NOT spent.
                !matches!(ledger.is_key_image_spent(&key_image), Ok(Some(_)))
            }
            // If we cannot recover the key (shouldn't happen for owned
            // outputs), keep it rather than silently dropping value.
            None => true,
        }
    });

    owned_utxos
}

/// Get the total balance of a wallet by summing all its unspent UTXOs.
pub fn get_wallet_balance(network: &TestNetwork, wallet: &Wallet) -> u64 {
    scan_wallet_utxos(network, wallet)
        .iter()
        .map(|(utxo, _)| utxo.output.amount)
        .sum()
}

/// Get UTXOs for a specific wallet that have at least the minimum amount.
///
/// Useful for finding spendable UTXOs that can cover a transaction.
pub fn get_spendable_utxos(
    network: &TestNetwork,
    wallet: &Wallet,
    min_amount: u64,
) -> Vec<(Utxo, u64)> {
    scan_wallet_utxos(network, wallet)
        .into_iter()
        .filter(|(utxo, _)| utxo.output.amount >= min_amount)
        .collect()
}
