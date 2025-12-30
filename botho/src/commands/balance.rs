use anyhow::{Context, Result};
use std::path::Path;

use crate::config::{ledger_db_path_from_config, Config};
use crate::ledger::Ledger;
use crate::wallet::Wallet;

/// Show wallet balance
pub fn run(config_path: &Path) -> Result<()> {
    let config = Config::load(config_path)
        .context("No wallet found. Run 'botho init' first.")?;

    let wallet_config = config.wallet
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("No wallet configured. Run 'botho init' first."))?;

    let wallet = Wallet::from_mnemonic(&wallet_config.mnemonic)?;
    let address = wallet.default_address();

    // Open ledger
    let ledger_path = ledger_db_path_from_config(config_path);
    let ledger = Ledger::open(&ledger_path)
        .map_err(|e| anyhow::anyhow!("Failed to open ledger: {}", e))?;

    // Get chain state
    let state = ledger
        .get_chain_state()
        .map_err(|e| anyhow::anyhow!("Failed to get chain state: {}", e))?;

    // Get balance from UTXOs
    let utxos = ledger
        .get_utxos_for_address(&address)
        .map_err(|e| anyhow::anyhow!("Failed to get UTXOs: {}", e))?;

    let balance: u64 = utxos.iter().map(|u| u.output.amount).sum();
    let utxo_count = utxos.len();

    // Convert from picocredits to BTH
    let bth = balance as f64 / 1_000_000_000_000.0;

    println!();
    println!("=== Wallet Balance ===");
    println!("Balance: {:.12} BTH ({} picocredits)", bth, balance);
    println!("UTXOs: {}", utxo_count);
    println!();
    println!("Chain height: {}", state.height);
    println!("Total network mined: {:.12} BTH", state.total_mined as f64 / 1_000_000_000_000.0);
    println!();

    Ok(())
}
