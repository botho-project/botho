//! Transaction history command

use anyhow::{anyhow, Result};
use std::path::Path;

use crate::storage::EncryptedWallet;

use super::{print_error, print_warning, prompt_password};

/// Run the history command
pub async fn run(wallet_path: &Path, limit: usize) -> Result<()> {
    // Check wallet exists
    if !EncryptedWallet::exists(wallet_path) {
        print_error("No wallet found. Run 'botho-wallet init' first.");
        return Ok(());
    }

    // Load wallet (just to verify password)
    let wallet = EncryptedWallet::load(wallet_path)?;
    let password = prompt_password("Enter wallet password: ")?;

    wallet.decrypt(&password)
        .map_err(|_| anyhow!("Failed to decrypt wallet - wrong password?"))?;

    println!();
    print_warning("Transaction history is not yet implemented.");
    println!();
    println!("This feature requires:");
    println!("  1. Storing transaction history locally");
    println!("  2. Querying nodes for transaction confirmations");
    println!();
    println!("Requested limit: {} transactions", limit);

    Ok(())
}
