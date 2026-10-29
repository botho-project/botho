use anyhow::{Context, Result};
use std::path::Path;

use crate::{
    config::{ledger_db_path_from_config, Config},
    ledger::Ledger,
    wallet::Wallet,
};

/// Picocredits per BTH (10^12) - the single base unit (#649/#694)
const PICOCREDITS_PER_BTH: u64 = 1_000_000_000_000;

/// The CLI wallet's spendable balance, as discovered on the ledger.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct WalletBalance {
    /// Sum of every unspent owned output, in picocredits.
    pub(crate) picocredits: u64,
    /// Number of unspent owned outputs.
    pub(crate) utxo_count: usize,
    /// How many of those are V2 lottery payouts.
    pub(crate) lottery_payout_count: usize,
}

/// Sum the wallet's unspent outputs through view-key discovery (#1662).
///
/// Outputs are one-time stealth keys, so they cannot be found by address:
/// `Ledger::get_utxos_for_address` keys on the address while the index is
/// keyed by target key, and it never matched a stealth output. Discovery
/// scans each output with the wallet's view key (hybrid ML-KEM included) and
/// recovers V2 lottery payouts through their persisted derivation contexts
/// (#1286 P7), exactly as `wallet_getBalance` does. Legacy aliased rows are
/// excluded by discovery.
pub(crate) fn summarize(ledger: &Ledger, wallet: &Wallet) -> Result<WalletBalance> {
    let owned = ledger
        .unspent_wallet_outputs(wallet)
        .map_err(|e| anyhow::anyhow!("Failed to scan wallet UTXOs: {}", e))?;
    Ok(WalletBalance {
        picocredits: owned.iter().map(|o| o.utxo.output.amount).sum(),
        utxo_count: owned.len(),
        lottery_payout_count: owned.iter().filter(|o| o.lottery_payout).count(),
    })
}

/// Show wallet balance
pub fn run(config_path: &Path) -> Result<()> {
    let config = Config::load(config_path).context("No wallet found. Run 'botho init' first.")?;

    let wallet_config = config
        .wallet
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("No wallet configured. Run 'botho init' first."))?;

    let wallet = Wallet::from_mnemonic(&wallet_config.mnemonic)?;

    // Open ledger
    let ledger_path = ledger_db_path_from_config(config_path);
    let ledger =
        Ledger::open(&ledger_path).map_err(|e| anyhow::anyhow!("Failed to open ledger: {}", e))?;

    // Get chain state
    let state = ledger
        .get_chain_state()
        .map_err(|e| anyhow::anyhow!("Failed to get chain state: {}", e))?;

    // Get balance from the wallet's discovered (stealth) outputs
    let balance = summarize(&ledger, &wallet)?;

    // Convert to display units: BTH (formatted) with the raw picocredit base
    // amount alongside. Picocredits are the only unit below the UI edge
    // (#649/#694); the former nanoBTH display tier is retired.
    let balance_bth = balance.picocredits as f64 / PICOCREDITS_PER_BTH as f64;

    println!();
    println!("=== Wallet Balance ===");
    println!("Balance: {:.12} BTH", balance_bth);
    println!("         {} picocredits (base unit)", balance.picocredits);
    println!("UTXOs: {}", balance.utxo_count);
    if balance.lottery_payout_count > 0 {
        println!(
            "  of which lottery payouts: {}",
            balance.lottery_payout_count
        );
    }
    println!();
    println!("Chain height: {}", state.height);
    println!(
        "Total network mined: {:.12} BTH",
        state.total_mined as f64 / PICOCREDITS_PER_BTH as f64
    );
    println!();

    Ok(())
}
