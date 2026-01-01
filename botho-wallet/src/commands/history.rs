//! Transaction history command

use anyhow::Result;
use std::path::Path;

use crate::storage::EncryptedWallet;

use super::{decrypt_wallet_with_rate_limiting, print_error, print_warning};

/// Run the history command
pub async fn run(wallet_path: &Path, limit: usize) -> Result<()> {
    // Check wallet exists
    if !EncryptedWallet::exists(wallet_path) {
        print_error("No wallet found. Run 'botho-wallet init' first.");
        return Ok(());
    }

    // Load and decrypt wallet with rate limiting protection (verify password)
    let (_wallet, _mnemonic, _password) = decrypt_wallet_with_rate_limiting(wallet_path)?;

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
