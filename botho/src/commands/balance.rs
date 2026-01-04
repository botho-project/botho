use anyhow::{Context, Result};
use std::path::Path;

use crate::{
    config::{ledger_db_path_from_config, Config},
    ledger::Ledger,
    wallet::Wallet,
};

/// Picocredits per BTH (10^12) - internal precision
const PICOCREDITS_PER_BTH: u64 = 1_000_000_000_000;

/// Picocredits per nanoBTH (10^3) - for nanoBTH display
const PICOCREDITS_PER_NANOBTH: u64 = 1_000;

/// Show wallet balance
pub fn run(config_path: &Path) -> Result<()> {
    let config = Config::load(config_path).context("No wallet found. Run 'botho init' first.")?;

    let wallet_config = config
        .wallet
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("No wallet configured. Run 'botho init' first."))?;

    let wallet = Wallet::from_mnemonic(&wallet_config.mnemonic)?;
    let address = wallet.default_address();

    // Open ledger
    let ledger_path = ledger_db_path_from_config(config_path);
    let ledger =
        Ledger::open(&ledger_path).map_err(|e| anyhow::anyhow!("Failed to open ledger: {}", e))?;

    // Get chain state
    let state = ledger
        .get_chain_state()
        .map_err(|e| anyhow::anyhow!("Failed to get chain state: {}", e))?;

    // Get balance from UTXOs
    let utxos = ledger
        .get_utxos_for_address(&address)
        .map_err(|e| anyhow::anyhow!("Failed to get UTXOs: {}", e))?;

    let balance_picocredits: u64 = utxos.iter().map(|u| u.output.amount).sum();
    let utxo_count = utxos.len();

    // Convert to display units
    let balance_bth = balance_picocredits as f64 / PICOCREDITS_PER_BTH as f64;
    let balance_nanobth = balance_picocredits / PICOCREDITS_PER_NANOBTH;

    println!();
    println!("=== Wallet Balance ===");
    println!("Balance: {:.12} BTH", balance_bth);
    println!("         {} nanoBTH", balance_nanobth);
    println!("         {} picocredits (internal)", balance_picocredits);
    println!("UTXOs: {}", utxo_count);
    println!();
    println!("Chain height: {}", state.height);
    println!(
        "Total network mined: {:.12} BTH",
        state.total_mined as f64 / PICOCREDITS_PER_BTH as f64
    );
    println!();

    Ok(())
}
