//! Address display command

use anyhow::{anyhow, Result};
use std::path::Path;

use crate::keys::WalletKeys;
use crate::storage::EncryptedWallet;

use super::{print_error, prompt_password};

/// Run the address command
pub async fn run(wallet_path: &Path) -> Result<()> {
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

    // Display address
    println!();
    println!("Your receiving address:");
    println!();
    println!("  {}", keys.address_string());
    println!();
    println!("Full public keys:");
    println!("  View:  {}", hex::encode(keys.view_public_key_bytes()));
    println!("  Spend: {}", hex::encode(keys.spend_public_key_bytes()));

    Ok(())
}
