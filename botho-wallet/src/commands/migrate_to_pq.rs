//! Migrate to Post-Quantum command
//!
//! Migrates all classical UTXOs to quantum-safe outputs, protecting funds
//! against future quantum computer attacks.

use anyhow::{anyhow, Result};
use std::path::Path;

use crate::{
    discovery::NodeDiscovery,
    keys::WalletKeys,
    rpc_pool::RpcPool,
    storage::EncryptedWallet,
    transaction::{format_amount, sync_wallet, TransactionBuilder, DUST_THRESHOLD},
};

use super::{
    decrypt_wallet_with_rate_limiting, print_error, print_success, print_warning, prompt_confirm,
};

/// Run the migrate-to-pq command
#[cfg(feature = "pq")]
pub async fn run(
    wallet_path: &Path,
    dry_run: bool,
    status_only: bool,
    skip_confirm: bool,
) -> Result<()> {
    // Check wallet exists
    if !EncryptedWallet::exists(wallet_path) {
        print_error("No wallet found. Run 'botho-wallet init' first.");
        return Ok(());
    }

    // Load and decrypt wallet with rate limiting protection
    let (mut wallet, mnemonic, password) = decrypt_wallet_with_rate_limiting(wallet_path)?;

    let keys = WalletKeys::from_mnemonic(&mnemonic)?;

    // Get PQ address (destination for migration)
    let pq_address = keys.pq_public_address();
    let pq_address_str = keys.pq_address_string();

    // Connect to network
    println!();
    println!("Connecting to network...");

    let discovery = wallet
        .get_discovery_state(&password)?
        .unwrap_or_else(NodeDiscovery::new);

    let mut rpc = RpcPool::new(discovery);
    rpc.connect().await?;

    println!("Connected to {} nodes", rpc.connected_count());

    // Sync wallet - finds all UTXOs via classical stealth address scanning
    // Note: All UTXOs found this way are classical (not yet migrated to PQ)
    println!("Syncing wallet...");
    let (utxos, height) = sync_wallet(&mut rpc, &keys, wallet.sync_height).await?;

    // For now, all scanned UTXOs are classical (detected via classical stealth
    // addresses) Once PQ scanning is implemented, we'll be able to track PQ
    // UTXOs separately
    let classical_utxos = utxos;
    let classical_balance: u64 = classical_utxos.iter().map(|u| u.amount).sum();

    // TODO: In the future, implement PQ UTXO scanning to track migrated funds
    // For now, we just track how much has been migrated by the fact that
    // the classical balance decreases after successful migration

    // Status only mode
    if status_only {
        println!();
        println!("Migration Status:");
        println!(
            "  Classical UTXOs:    {} ({} BTH)",
            classical_utxos.len(),
            format_amount(classical_balance)
        );
        println!("  Quantum-safe UTXOs: (tracking not yet implemented)");
        println!();

        if classical_balance == 0 {
            print_success("No classical UTXOs found. Migration may be complete.");
            println!();
            println!("Note: PQ UTXO tracking is not yet implemented.");
            println!("Once implemented, this command will show your quantum-safe balance.");
        } else {
            println!("Run 'botho-wallet migrate-to-pq' to migrate classical funds.");
        }

        return Ok(());
    }

    // Check if there's anything to migrate
    if classical_utxos.is_empty() {
        println!();
        println!("No classical UTXOs to migrate. Wallet balance is 0.");
        println!();
        println!("Note: If you've already migrated, your funds are in PQ outputs.");
        println!("PQ UTXO tracking will be available in a future update.");
        return Ok(());
    }

    // Calculate migration fee
    // PQ transactions are ~19x larger than classical
    use botho::transaction_pq::calculate_pq_fee;
    let num_inputs = classical_utxos.len();
    let num_outputs = 1; // Single output to our PQ address (no change needed - we're sending to self)
    let fee = calculate_pq_fee(num_inputs, num_outputs);

    // Check if we have enough to cover fee
    if classical_balance <= fee {
        print_error(&format!(
            "Insufficient funds for migration. Balance: {}, estimated fee: {}",
            format_amount(classical_balance),
            format_amount(fee)
        ));
        println!();
        println!("Migration requires enough balance to cover the transaction fee.");
        println!("Note: Post-quantum transactions are ~19x larger than classical,");
        println!("so fees are proportionally higher.");
        return Ok(());
    }

    let amount_after_fee = classical_balance - fee;

    // Check dust threshold
    if amount_after_fee < DUST_THRESHOLD {
        print_warning(&format!(
            "After fees, remaining amount ({}) is below dust threshold.",
            format_amount(amount_after_fee)
        ));
        println!("The entire balance would be consumed by fees. Consider waiting for");
        println!("more funds before migrating, or this dust will be absorbed as fee.");
    }

    // Show migration plan
    println!();
    println!("Migration Plan:");
    println!("  Classical UTXOs to migrate: {}", classical_utxos.len());
    println!(
        "  Amount to migrate:          {}",
        format_amount(classical_balance)
    );
    println!(
        "  Estimated fee:              {} (PQ tx ~19x larger)",
        format_amount(fee)
    );
    println!(
        "  Amount after fee:           {}",
        format_amount(amount_after_fee)
    );
    println!();
    println!("  Destination (PQ address):");
    println!(
        "    {}...",
        &pq_address_str[..std::cmp::min(60, pq_address_str.len())]
    );
    println!();

    // Dry run mode
    if dry_run {
        println!("--dry-run mode: No changes will be made.");
        println!();
        println!("To execute migration, run:");
        println!("  botho-wallet migrate-to-pq");
        return Ok(());
    }

    // Confirm
    if !skip_confirm {
        println!();
        print_warning("This will move all classical funds to your quantum-safe address.");
        println!("Your funds will be protected against future quantum computers.");
        println!();
        if !prompt_confirm("Proceed with migration?")? {
            println!("Aborted.");
            return Ok(());
        }
    }

    // Build migration transaction
    println!();
    println!("Building migration transaction...");

    let builder = TransactionBuilder::new(keys.clone(), classical_utxos, height);

    // Build PQ transfer to our own PQ address
    let tx = builder.build_pq_transfer(&pq_address, amount_after_fee, fee)?;

    // Serialize and submit
    println!("Signing transaction with dual signatures (Schnorr + ML-DSA)...");
    let tx_bytes =
        bincode::serialize(&tx).map_err(|e| anyhow!("Failed to serialize transaction: {}", e))?;
    let tx_hex = hex::encode(&tx_bytes);

    println!("Submitting migration transaction...");
    let tx_hash = rpc.submit_pq_transaction(&tx_hex).await?;

    println!();
    print_success("Migration transaction submitted!");
    println!();
    println!("Transaction hash: {}", tx_hash);
    println!();
    println!("Migration details:");
    println!("  • {} classical UTXOs consolidated", num_inputs);
    println!(
        "  • {} BTH migrated to quantum-safe output",
        format_amount(amount_after_fee)
    );
    println!("  • Protected by ML-KEM-768 + ML-DSA-65");
    println!();
    println!("Your funds are now protected against future quantum computers.");
    println!();
    println!("Next steps:");
    println!("  1. Wait for confirmation (~10 seconds)");
    println!("  2. Verify with: botho-wallet migrate-to-pq --status");
    println!("  3. Share your PQ address with exchanges/counterparties");

    // Update sync height
    wallet.set_sync_height(height);
    wallet.set_discovery_state(rpc.discovery(), &password)?;
    wallet.save(wallet_path)?;

    Ok(())
}

/// Fallback when PQ feature is not enabled
#[cfg(not(feature = "pq"))]
pub async fn run(
    _wallet_path: &Path,
    _dry_run: bool,
    _status_only: bool,
    _skip_confirm: bool,
) -> Result<()> {
    print_error("Quantum-safe migration is not enabled in this build.");
    println!();
    println!("To enable post-quantum features, rebuild with:");
    println!("  cargo build --release --features pq -p botho-wallet");
    println!();
    println!("Or use a binary built with PQ support.");
    Ok(())
}
