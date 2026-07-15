//! Address display command

use anyhow::Result;
use std::path::Path;

use crate::{keys::WalletKeys, storage::EncryptedWallet};

use super::{decrypt_wallet_with_rate_limiting, print_error};

/// Run the address command
pub async fn run(wallet_path: &Path, show_pq: bool) -> Result<()> {
    // Check wallet exists
    if !EncryptedWallet::exists(wallet_path) {
        print_error("No wallet found. Run 'botho-wallet init' first.");
        return Ok(());
    }

    // Load and decrypt wallet with rate limiting protection
    let (_wallet, mnemonic, _password) = decrypt_wallet_with_rate_limiting(wallet_path)?;

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
        let pq_addr = keys.pq_public_address();
        println!(
            "  ML-KEM-768 public key ({} bytes):",
            pq_addr.kem_public_key().len()
        );
        println!("    {}", hex::encode(pq_addr.kem_public_key()));
        println!(
            "  ML-DSA-65 public key ({} bytes):",
            pq_addr.dsa_public_key().len()
        );
        println!("    {}", hex::encode(pq_addr.dsa_public_key()));
        println!();
        println!("  Note: post-quantum public keys are now part of the unified");
        println!("  address (format v2). The compact base58 v2 address string is");
        println!("  introduced in a later rollout step.");
    }

    #[cfg(not(feature = "pq"))]
    if show_pq {
        println!();
        println!("Quantum-safe addresses are not enabled in this build.");
        println!("Rebuild with --features pq to enable.");
    }

    Ok(())
}
