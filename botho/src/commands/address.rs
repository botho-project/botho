use anyhow::{Context, Result};
use std::path::Path;

use crate::address::{format_classical_address, Address};
use crate::config::Config;
use crate::wallet::Wallet;

#[cfg(feature = "pq")]
use crate::address::format_quantum_address;

/// Show receiving address
///
/// If `save_path` is provided, saves the address to a file instead of printing.
/// File extension determines which address type to save:
/// - `.pq` extension saves the quantum-safe address
/// - Any other extension saves the classical address
pub fn run(config_path: &Path, save_path: Option<&str>) -> Result<()> {
    let config = Config::load(config_path)
        .context("No wallet found. Run 'botho init' first.")?;

    let mnemonic = config.mnemonic()
        .context("No wallet configured. Run 'botho init' to create one.")?;
    let wallet = Wallet::from_mnemonic(mnemonic)
        .context("Failed to load wallet from mnemonic")?;

    // Get the network type from config
    let network = config.network_type();

    // Handle save option
    if let Some(path) = save_path {
        #[cfg(feature = "pq")]
        if path.ends_with(".pq") {
            let addr = Address::quantum(wallet.quantum_safe_address(), network);
            addr.save_to_file(path)?;
            println!("Quantum-safe address saved to: {}", path);
            println!("Share this file with anyone who wants to send you BTH with PQ protection.");
            return Ok(());
        }

        let addr = Address::classical(wallet.default_address(), network);
        addr.save_to_file(path)?;
        println!("Classical address saved to: {}", path);
        println!("Share this file with anyone who wants to send you BTH.");
        return Ok(());
    }

    // Classical address (short form)
    let classical_addr = format_classical_address(&wallet.default_address(), network);

    println!("=== Your Botho Address ===");
    println!();
    println!("Classical (~90 chars):");
    println!("{}", classical_addr);
    println!();

    #[cfg(feature = "pq")]
    {
        // Quantum-safe address (full form)
        let quantum_addr = format_quantum_address(&wallet.quantum_safe_address(), network);

        println!("Quantum-Safe (~4400 chars):");
        println!("{}", quantum_addr);
        println!();
        println!("---");
        println!("Classical: Use for standard and ring-signature transactions");
        println!("Quantum:   Use for post-quantum protected transactions");
        println!();
        println!("Tip: Save the quantum address to a file for easy sharing:");
        println!("     botho address --save myaddress.pq");
    }

    #[cfg(not(feature = "pq"))]
    {
        println!("---");
        println!("Share this address to receive BTH.");
    }

    Ok(())
}
