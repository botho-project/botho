//! Wallet sync command

use anyhow::{anyhow, Result};
use std::path::Path;

use crate::discovery::NodeDiscovery;
use crate::keys::WalletKeys;
use crate::rpc_pool::RpcPool;
use crate::storage::EncryptedWallet;
use crate::transaction::{format_amount, sync_wallet};

use super::{print_error, print_success, prompt_password};

/// Run the sync command
pub async fn run(wallet_path: &Path, full: bool) -> Result<()> {
    // Check wallet exists
    if !EncryptedWallet::exists(wallet_path) {
        print_error("No wallet found. Run 'botho-wallet init' first.");
        return Ok(());
    }

    // Load and decrypt wallet
    let mut wallet = EncryptedWallet::load(wallet_path)?;
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

    // Determine starting height
    let from_height = if full {
        println!("Performing full rescan from genesis...");
        0
    } else {
        let h = wallet.sync_height;
        if h > 0 {
            println!("Resuming sync from block {}...", h);
        } else {
            println!("Performing initial sync...");
        }
        h
    };

    // Get current chain height
    let chain_info = rpc.get_chain_info().await?;
    let current_height = chain_info.height;

    if from_height >= current_height {
        println!();
        print_success("Wallet is already synced!");
        println!("Current height: {}", current_height);
        return Ok(());
    }

    let blocks_to_scan = current_height - from_height;
    println!("Scanning {} blocks...", blocks_to_scan);

    // Sync wallet
    let (utxos, height) = sync_wallet(&mut rpc, &keys, from_height).await?;

    // Calculate balance
    let total: u64 = utxos.iter().map(|u| u.amount).sum();

    // Update wallet state
    wallet.set_sync_height(height);
    wallet.set_discovery_state(rpc.discovery(), &password)?;
    wallet.save(wallet_path)?;

    println!();
    print_success("Sync complete!");
    println!();
    println!("Synced to block: {}", height);
    println!("UTXOs found: {}", utxos.len());
    println!("Balance: {}", format_amount(total));

    Ok(())
}
