use anyhow::{Context, Result};
use std::path::Path;

use crate::config::{ledger_db_path_from_config, Config};
use crate::ledger::Ledger;
use crate::wallet::Wallet;

/// Show wallet balance
pub fn run(config_path: &Path) -> Result<()> {
    let config = Config::load(config_path)
        .context("No wallet found. Run 'cadence init' first.")?;

    let wallet = Wallet::from_mnemonic(&config.wallet.mnemonic)?;
    let address = wallet.default_address();
    let wallet_view_key = address.view_public_key().to_bytes();
    let wallet_spend_key = address.spend_public_key().to_bytes();

    // Open ledger
    let ledger_path = ledger_db_path_from_config(config_path);
    let ledger = Ledger::open(&ledger_path)
        .map_err(|e| anyhow::anyhow!("Failed to open ledger: {}", e))?;

    // Get chain state
    let state = ledger
        .get_chain_state()
        .map_err(|e| anyhow::anyhow!("Failed to get chain state: {}", e))?;

    // Calculate balance by scanning all blocks
    let mut mined_balance: u64 = 0;
    let mut blocks_mined: u64 = 0;

    // Scan blocks starting from height 1 (skip genesis)
    let blocks = ledger
        .get_blocks(1, state.height as usize)
        .map_err(|e| anyhow::anyhow!("Failed to get blocks: {}", e))?;

    for block in blocks {
        // Check if this block's mining reward was sent to our address
        if block.mining_tx.recipient_view_key == wallet_view_key
            && block.mining_tx.recipient_spend_key == wallet_spend_key
        {
            mined_balance += block.mining_tx.reward;
            blocks_mined += 1;
        }
    }

    // Convert from picocredits to credits
    let credits = mined_balance as f64 / 1_000_000_000_000.0;

    println!();
    println!("=== Wallet Balance ===");
    println!("Mined: {:.12} credits ({} picocredits)", credits, mined_balance);
    println!("Blocks mined: {}", blocks_mined);
    println!();
    println!("Chain height: {}", state.height);
    println!("Total network mined: {:.12} credits", state.total_mined as f64 / 1_000_000_000_000.0);
    println!();

    Ok(())
}
