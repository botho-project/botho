//! Wallet sync command

use anyhow::Result;
use std::path::Path;

use crate::{
    discovery::NodeDiscovery,
    fee_estimation::PendingChangeTags,
    keys::WalletKeys,
    rpc_pool::RpcPool,
    storage::EncryptedWallet,
    transaction::{apply_pending_change_tags, format_amount, sync_wallet},
};

use super::{decrypt_wallet_with_rate_limiting, print_error, print_success};

/// Run the sync command
pub async fn run(wallet_path: &Path, full: bool) -> Result<()> {
    // Check wallet exists
    if !EncryptedWallet::exists(wallet_path) {
        print_error("No wallet found. Run 'botho-wallet init' first.");
        return Ok(());
    }

    // Load and decrypt wallet with rate limiting protection
    let (mut wallet, mnemonic, password) = decrypt_wallet_with_rate_limiting(wallet_path)?;

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
    let (mut utxos, height) = sync_wallet(&mut rpc, &keys, from_height).await?;

    // Apply pending change tags to discovered UTXOs (issue #249)
    // This propagates cluster attribution from inputs to change outputs.
    let mut pending_tags = wallet
        .get_pending_change_tags(&password)?
        .unwrap_or_else(PendingChangeTags::new);

    // Clean up stale pending tags (older than 1000 blocks)
    pending_tags.cleanup_stale(height, 1000);

    // Apply pending tags to matching change outputs
    let tags_applied = apply_pending_change_tags(&mut utxos, &mut pending_tags);

    // Save pending tags if any were applied or cleaned up
    if tags_applied || !pending_tags.is_empty() {
        wallet.set_pending_change_tags(&pending_tags, &password)?;
    }

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
