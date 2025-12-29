//! Send transaction command

use anyhow::{anyhow, Result};
use std::path::Path;

use crate::discovery::NodeDiscovery;
use crate::keys::WalletKeys;
use crate::rpc_pool::RpcPool;
use crate::storage::EncryptedWallet;
use crate::transaction::{format_amount, parse_amount, sync_wallet, TransactionBuilder};

use super::{print_error, print_success, prompt_confirm, prompt_password};

/// Run the send command
pub async fn run(wallet_path: &Path, address: &str, amount: f64, skip_confirm: bool) -> Result<()> {
    // Check wallet exists
    if !EncryptedWallet::exists(wallet_path) {
        print_error("No wallet found. Run 'botho-wallet init' first.");
        return Ok(());
    }

    // Parse amount
    let amount_str = format!("{}", amount);
    let amount_picocredits = parse_amount(&amount_str)?;

    if amount_picocredits == 0 {
        return Err(anyhow!("Amount must be greater than 0"));
    }

    // Load and decrypt wallet
    let mut wallet = EncryptedWallet::load(wallet_path)?;
    let password = prompt_password("Enter wallet password: ")?;

    let mnemonic = wallet.decrypt(&password)
        .map_err(|_| anyhow!("Failed to decrypt wallet - wrong password?"))?;

    let keys = WalletKeys::from_mnemonic(&mnemonic)?;

    // Parse recipient address
    let recipient = parse_address(address)?;

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

    // Get fee estimate
    let fee = rpc.estimate_fee("medium").await.unwrap_or(1_000_000);

    // Build transaction
    let builder = TransactionBuilder::new(keys.clone(), utxos, height);
    let balance = builder.balance();

    // Check sufficient funds
    let total_needed = amount_picocredits + fee;
    if balance < total_needed {
        print_error(&format!(
            "Insufficient funds. Balance: {}, needed: {}",
            format_amount(balance),
            format_amount(total_needed)
        ));
        return Ok(());
    }

    // Show transaction details
    println!();
    println!("Transaction details:");
    println!("  Recipient: {}", address);
    println!("  Amount:    {}", format_amount(amount_picocredits));
    println!("  Fee:       {}", format_amount(fee));
    println!("  Total:     {}", format_amount(total_needed));
    println!();
    println!("  Balance after: {}", format_amount(balance - total_needed));

    // Confirm
    if !skip_confirm {
        println!();
        if !prompt_confirm("Send this transaction?")? {
            println!("Aborted.");
            return Ok(());
        }
    }

    // Build and sign transaction
    println!();
    println!("Signing transaction...");

    let tx = builder.build_transfer(&recipient, amount_picocredits, fee)?;

    // Submit transaction
    println!("Submitting transaction...");

    let tx_hash = rpc.submit_transaction(&tx.to_hex()).await?;

    println!();
    print_success("Transaction sent!");
    println!();
    println!("Transaction hash: {}", tx_hash);

    // Update sync height
    wallet.set_sync_height(height);
    wallet.set_discovery_state(rpc.discovery(), &password)?;
    wallet.save(wallet_path)?;

    Ok(())
}

/// Parse a recipient address from string
fn parse_address(address: &str) -> Result<bt_account_keys::PublicAddress> {
    // Expected format: cad:<view_key_hex>:<spend_key_hex>
    // or: view:<hex>\nspend:<hex>

    if address.starts_with("cad:") {
        let parts: Vec<&str> = address.split(':').collect();
        if parts.len() != 3 {
            return Err(anyhow!("Invalid address format"));
        }

        let view_bytes = hex::decode(parts[1])
            .map_err(|_| anyhow!("Invalid view key hex"))?;
        let spend_bytes = hex::decode(parts[2])
            .map_err(|_| anyhow!("Invalid spend key hex"))?;

        // For now, we need the full 32-byte keys
        // The cad: format uses 16-byte prefixes for display
        if view_bytes.len() < 16 || spend_bytes.len() < 16 {
            return Err(anyhow!("Address keys too short"));
        }

        // This is a simplified version - in production we'd need full keys
        return Err(anyhow!(
            "Short address format not yet supported. Please provide full public keys."
        ));
    }

    // Try parsing as view:<hex>\nspend:<hex> format
    if address.contains("view:") && address.contains("spend:") {
        let mut view_bytes = None;
        let mut spend_bytes = None;

        for line in address.lines() {
            let line = line.trim();
            if let Some(hex) = line.strip_prefix("view:") {
                view_bytes = Some(hex::decode(hex.trim())?);
            } else if let Some(hex) = line.strip_prefix("spend:") {
                spend_bytes = Some(hex::decode(hex.trim())?);
            }
        }

        match (view_bytes, spend_bytes) {
            (Some(v), Some(s)) if v.len() == 32 && s.len() == 32 => {
                let view_key = bt_crypto_keys::RistrettoPublic::try_from(&v[..])
                    .map_err(|_| anyhow!("Invalid view public key"))?;
                let spend_key = bt_crypto_keys::RistrettoPublic::try_from(&s[..])
                    .map_err(|_| anyhow!("Invalid spend public key"))?;

                return Ok(bt_account_keys::PublicAddress::new(&spend_key, &view_key));
            }
            _ => return Err(anyhow!("Invalid address key lengths")),
        }
    }

    Err(anyhow!(
        "Invalid address format. Expected 'cad:<view>:<spend>' or 'view:<hex>\\nspend:<hex>'"
    ))
}
