//! Address display command

use anyhow::{anyhow, Result};
use std::path::Path;

use crate::keys::WalletKeys;
use crate::storage::EncryptedWallet;

use super::{print_error, prompt_password};

/// Run the address command
pub async fn run(wallet_path: &Path, show_pq: bool) -> Result<()> {
    // Check wallet exists
    if !EncryptedWallet::exists(wallet_path) {
        print_error("No wallet found. Run 'botho-wallet init' first.");
        return Ok(());
    }

    // Load and decrypt wallet
    let wallet = EncryptedWallet::load(wallet_path)?;
    let password = prompt_password("Enter wallet password: ")?;

    let mnemonic = wallet.decrypt(&password)
        .map_err(|_| anyhow!("Failed to decrypt wallet - wrong password?"))?;

    let keys = WalletKeys::from_mnemonic(&mnemonic)?;

    // Display classical address
    println!();
    println!("Your receiving address (classical):");
    println!();
    println!("  {}", keys.address_string());
    println!();
    println!("Classical public keys:");
    println!("  View:  {}", hex::encode(keys.view_public_key_bytes()));
    println!("  Spend: {}", hex::encode(keys.spend_public_key_bytes()));

    // Display quantum-safe address if requested and feature is enabled
    #[cfg(feature = "pq")]
    if show_pq {
        println!();
        println!("Quantum-safe address (ML-KEM-768 + ML-DSA-65):");
        println!();
        let pq_addr = keys.pq_address_string();
        // The address is long, so we'll show it wrapped
        println!("  {}", pq_addr);
        println!();
        println!("  Note: This address is ~4.3KB and includes post-quantum public keys");
        println!("  for protection against future quantum computer attacks.");
    }

    #[cfg(not(feature = "pq"))]
    if show_pq {
        println!();
        println!("Quantum-safe addresses are not enabled in this build.");
        println!("Rebuild with --features pq to enable.");
    }

    Ok(())
}
