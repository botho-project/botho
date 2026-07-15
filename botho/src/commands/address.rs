use anyhow::{Context, Result};
use std::path::Path;

use crate::{
    address::{format_classical_address, Address},
    config::Config,
    wallet::Wallet,
};

/// Show receiving address
///
/// If `save_path` is provided, saves the address to a file instead of printing.
pub fn run(config_path: &Path, save_path: Option<&str>) -> Result<()> {
    let config = Config::load(config_path).context("No wallet found. Run 'botho init' first.")?;

    let mnemonic = config
        .mnemonic()
        .context("No wallet configured. Run 'botho init' to create one.")?;
    let wallet = Wallet::from_mnemonic(mnemonic).context("Failed to load wallet from mnemonic")?;

    // Get the network type from config
    let network = config.network_type();

    // Handle save option
    if let Some(path) = save_path {
        if path.ends_with(".pq") {
            // Separate quantum addresses are gone; refuse loudly instead of
            // silently writing a classical address under a .pq name.
            return Err(anyhow::anyhow!(
                "quantum addresses retired (ADR 0006): the .pq address format \
                 was removed. Save a classical address instead, e.g.: \
                 botho address --save myaddress.botho"
            ));
        }

        let addr = Address::classical(wallet.default_address(), network);
        addr.save_to_file(path)?;
        println!("Classical address saved to: {}", path);
        println!("Share this file with anyone who wants to send you BTH.");
        return Ok(());
    }

    // v2 post-quantum address string
    let classical_addr = format_classical_address(&wallet.default_address(), network)?;

    println!("=== Your Botho Address ===");
    println!();
    println!("Post-quantum (v2):");
    println!("{}", classical_addr);
    println!();
    println!("---");
    println!("Share this address to receive BTH.");

    Ok(())
}
