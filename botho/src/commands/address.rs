use anyhow::{Context, Result};
use std::path::Path;

use crate::config::Config;
use crate::wallet::Wallet;

/// Show receiving address
pub fn run(config_path: &Path) -> Result<()> {
    let config = Config::load(config_path)
        .context("No wallet found. Run 'botho init' first.")?;

    let mnemonic = config.mnemonic()
        .context("No wallet configured. Run 'botho init' to create one.")?;
    let wallet = Wallet::from_mnemonic(mnemonic)
        .context("Failed to load wallet from mnemonic")?;

    println!("Your receiving address:");
    println!();
    println!("{}", wallet.address_string());
    println!();
    println!("Share this address to receive credits.");

    Ok(())
}
