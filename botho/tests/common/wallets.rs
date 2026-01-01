// Copyright (c) 2024 Botho Foundation
//
//! Wallet utilities for test networks.

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
        }
    }

    owned_utxos
}

/// Get the total balance of a wallet by summing all its UTXOs.
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
