use anyhow::{Context, Result};
use std::path::Path;

use crate::config::Config;

/// Send credits to an address
pub fn run(config_path: &Path, address: &str, amount: &str) -> Result<()> {
    let _config = Config::load(config_path)
        .context("No wallet found. Run 'cadence init' first.")?;

    // TODO: Implement transaction building and sending

    println!("Sending {} credits to {}...", amount, address);
    println!();
    println!("(not yet implemented)");

    Ok(())
}
