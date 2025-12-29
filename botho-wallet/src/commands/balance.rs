//! Balance check command

use anyhow::{anyhow, Result};
use std::path::Path;

use crate::discovery::NodeDiscovery;
use crate::keys::WalletKeys;
use crate::rpc_pool::RpcPool;
use crate::storage::EncryptedWallet;
use crate::transaction::{format_amount, sync_wallet};

use super::{print_error, print_success, prompt_password};

/// Run the balance command
pub async fn run(wallet_path: &Path, detailed: bool) -> Result<()> {
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

    // Connect to network
    println!();
    println!("Connecting to network...");

    let discovery = wallet
        .get_discovery_state(&password)?
        .unwrap_or_else(NodeDiscovery::new);

    let mut rpc = RpcPool::new(discovery);
    rpc.connect().await?;

    println!("Connected to {} nodes", rpc.connected_count());

    // Sync wallet
    println!("Syncing wallet...");
    let (utxos, height) = sync_wallet(&mut rpc, &keys, wallet.sync_height).await?;

    // Calculate balance
    let total: u64 = utxos.iter().map(|u| u.amount).sum();

    println!();
    print_success(&format!("Balance: {}", format_amount(total)));
    println!();
    println!("Synced to block {}", height);

    if detailed && !utxos.is_empty() {
        println!();
        println!("UTXOs ({}):", utxos.len());
        for (i, utxo) in utxos.iter().enumerate() {
            println!(
                "  {}. {} (block {})",
                i + 1,
                format_amount(utxo.amount),
                utxo.created_at
            );
        }
    }

    Ok(())
}
