use anyhow::{Context, Result};
use std::path::Path;

use crate::config::{ledger_db_path_from_config, Config};
use crate::ledger::Ledger;
use crate::wallet::Wallet;

/// Show node and wallet status
pub fn run(config_path: &Path) -> Result<()> {
    let config = Config::load(config_path)
        .context("No config found. Run 'botho init' first.")?;

    // Wallet is optional (relay nodes don't have one)
    let wallet = config.wallet.as_ref().and_then(|w| {
        Wallet::from_mnemonic(&w.mnemonic).ok()
    });

    // Open ledger
    let ledger_path = ledger_db_path_from_config(config_path);
    let ledger = Ledger::open(&ledger_path)
        .map_err(|e| anyhow::anyhow!("Failed to open ledger: {}", e))?;

    // Get chain state
    let state = ledger
        .get_chain_state()
        .map_err(|e| anyhow::anyhow!("Failed to get chain state: {}", e))?;

    println!();
    println!("=== Botho Status ===");
    println!();
    println!("Wallet:");
    if let Some(ref w) = wallet {
        println!("  Address: {}", w.address_string().replace('\n', ", "));
    } else {
        println!("  (No wallet configured - running as relay node)");
    }
    println!();
    println!("Chain:");
    println!("  Height: {}", state.height);
    println!("  Tip hash: {}", hex::encode(&state.tip_hash[0..8]));
    println!(
        "  Total mined: {:.12} BTH",
        state.total_mined as f64 / 1_000_000_000_000.0
    );
    println!(
        "  Difficulty: {} (0x{:016x})",
        state.difficulty, state.difficulty
    );
    println!();
    let network = config.network_type();
    println!("Network:");
    println!("  Type: {}", network.display_name());
    println!("  Gossip port: {}", config.network.gossip_port(network));
    println!("  Bootstrap peers: {}", config.network.bootstrap_peers(network).len());
    println!(
        "  DNS seed discovery: {}",
        if config.network.dns_seeds.enabled { "enabled" } else { "disabled" }
    );
    if let Some(ref domain) = config.network.dns_seeds.domain {
        println!("  DNS seed domain: {}", domain);
    }
    if config.network.bootstrap_peers.is_empty() && !config.network.dns_seeds.enabled {
        println!("  (No bootstrap peers - solo minting only)");
    }
    println!();
    println!("Minting:");
    println!(
        "  Enabled in config: {}",
        if config.minting.enabled { "yes" } else { "no" }
    );
    println!(
        "  Threads: {}",
        if config.minting.threads == 0 {
            "auto".to_string()
        } else {
            config.minting.threads.to_string()
        }
    );
    println!();

    Ok(())
}
