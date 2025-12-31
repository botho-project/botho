//! Send transaction command

use anyhow::{anyhow, Result};
use std::path::Path;

use crate::discovery::NodeDiscovery;
use crate::keys::WalletKeys;
use crate::rpc_pool::RpcPool;
use crate::storage::EncryptedWallet;
use crate::transaction::{format_amount, parse_amount, sync_wallet, TransactionBuilder, DUST_THRESHOLD};

use super::{print_error, print_success, print_warning, prompt_confirm, prompt_password};

/// Run the send command
pub async fn run(
    wallet_path: &Path,
    address: &str,
    amount: f64,
    skip_confirm: bool,
    quantum_private: bool,
) -> Result<()> {
    // Handle quantum-private transaction request
    #[cfg(not(feature = "pq"))]
    if quantum_private {
        print_error("Quantum-private transactions are not enabled in this build.");
        println!("Rebuild with --features pq to enable post-quantum transactions.");
        return Ok(());
    }

    #[cfg(feature = "pq")]
    if quantum_private {
        return run_quantum_private(wallet_path, address, amount, skip_confirm).await;
    }

    // Classical transaction flow
    run_classical(wallet_path, address, amount, skip_confirm).await
}

/// Run a classical (non-PQ) transaction
async fn run_classical(
    wallet_path: &Path,
    address: &str,
    amount: f64,
    skip_confirm: bool,
) -> Result<()> {
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

    // Warn if amount is below dust threshold
    if amount_picocredits < DUST_THRESHOLD {
        print_error(&format!(
            "Amount {} is below the dust threshold of {}",
            format_amount(amount_picocredits),
            format_amount(DUST_THRESHOLD)
        ));
        println!("Outputs this small would be unspendable (cost more in fees than they're worth).");
        return Ok(());
    }

    // Warn if amount is close to dust threshold (within 10x)
    if amount_picocredits < DUST_THRESHOLD * 10 {
        print_warning(&format!(
            "Note: {} is a small output (close to dust threshold of {}).",
            format_amount(amount_picocredits),
            format_amount(DUST_THRESHOLD)
        ));
        println!("         Small outputs may cost more in fees to spend later.");
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

    // Calculate expected change to warn about dust absorption
    let expected_change = balance - total_needed;
    let (actual_fee, dust_absorbed) = if expected_change > 0 && expected_change < DUST_THRESHOLD {
        // Dust change will be absorbed into fee
        (fee + expected_change, true)
    } else {
        (fee, false)
    };

    // Show transaction details
    println!();
    println!("Transaction details:");
    println!("  Recipient: {}", address);
    println!("  Amount:    {}", format_amount(amount_picocredits));
    if dust_absorbed {
        println!("  Fee:       {} (includes {} dust change)", format_amount(actual_fee), format_amount(expected_change));
        print_warning("Change is below dust threshold - will be added to fee.");
    } else {
        println!("  Fee:       {}", format_amount(fee));
    }
    println!("  Total:     {}", format_amount(amount_picocredits + actual_fee));
    println!();
    if !dust_absorbed && expected_change > 0 {
        println!("  Change:        {}", format_amount(expected_change));
    }
    println!("  Balance after: {}", format_amount(balance - amount_picocredits - actual_fee));

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
fn parse_address(address: &str) -> Result<bth_account_keys::PublicAddress> {
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
                let view_key = bth_crypto_keys::RistrettoPublic::try_from(&v[..])
                    .map_err(|_| anyhow!("Invalid view public key"))?;
                let spend_key = bth_crypto_keys::RistrettoPublic::try_from(&s[..])
                    .map_err(|_| anyhow!("Invalid spend public key"))?;

                return Ok(bth_account_keys::PublicAddress::new(&spend_key, &view_key));
            }
            _ => return Err(anyhow!("Invalid address key lengths")),
        }
    }

    Err(anyhow!(
        "Invalid address format. Expected 'cad:<view>:<spend>' or 'view:<hex>\\nspend:<hex>'"
    ))
}

/// Run a quantum-private transaction
#[cfg(feature = "pq")]
async fn run_quantum_private(
    wallet_path: &Path,
    address: &str,
    amount: f64,
    skip_confirm: bool,
) -> Result<()> {
    use bth_account_keys::QuantumSafePublicAddress;

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

    // Warn if amount is below dust threshold
    if amount_picocredits < DUST_THRESHOLD {
        print_error(&format!(
            "Amount {} is below the dust threshold of {}",
            format_amount(amount_picocredits),
            format_amount(DUST_THRESHOLD)
        ));
        println!("Outputs this small would be unspendable (cost more in fees than they're worth).");
        return Ok(());
    }

    // Warn if amount is close to dust threshold (within 10x)
    if amount_picocredits < DUST_THRESHOLD * 10 {
        print_warning(&format!(
            "Note: {} is a small output (close to dust threshold of {}).",
            format_amount(amount_picocredits),
            format_amount(DUST_THRESHOLD)
        ));
        println!("         Small outputs may cost more in fees to spend later.");
    }

    // Parse quantum-safe recipient address
    // The address is validated here; full transaction building will use it
    let pq_recipient = QuantumSafePublicAddress::from_address_string(address)
        .map_err(|e| anyhow!("Invalid quantum-safe address: {:?}", e))?;

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

    // Sync wallet
    println!("Syncing wallet...");
    let (utxos, height) = sync_wallet(&mut rpc, &keys, wallet.sync_height).await?;

    // Calculate PQ fee (higher than classical due to larger tx size)
    // Simple 1-input, 2-output PQ transaction fee
    use botho::transaction_pq::calculate_pq_fee;
    let estimated_inputs = std::cmp::max(1, (amount_picocredits / 1_000_000_000_000).saturating_add(1) as usize);
    let fee = calculate_pq_fee(estimated_inputs, 2);

    // Build classical transaction builder to check balance
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

    // Calculate expected change to warn about dust absorption
    let expected_change = balance - total_needed;
    let (actual_fee, dust_absorbed) = if expected_change > 0 && expected_change < DUST_THRESHOLD {
        // Dust change will be absorbed into fee
        (fee + expected_change, true)
    } else {
        (fee, false)
    };

    // Show transaction details
    println!();
    println!("Quantum-Private Transaction Details:");
    println!("  Type:      Post-Quantum (ML-KEM-768 + ML-DSA-65)");
    println!("  Recipient: {}...", &address[..std::cmp::min(50, address.len())]);
    println!("  Amount:    {}", format_amount(amount_picocredits));
    if dust_absorbed {
        println!("  Fee:       {} (includes {} dust change, ~19x larger tx)", format_amount(actual_fee), format_amount(expected_change));
        print_warning("Change is below dust threshold - will be added to fee.");
    } else {
        println!("  Fee:       {} (higher due to ~19x larger tx size)", format_amount(fee));
    }
    println!("  Total:     {}", format_amount(amount_picocredits + actual_fee));
    println!();
    if !dust_absorbed && expected_change > 0 {
        println!("  Change:        {}", format_amount(expected_change));
    }
    println!("  Balance after: {}", format_amount(balance - amount_picocredits - actual_fee));
    println!();
    print_warning("Quantum-private transactions are larger and cost more in fees,");
    println!("         but provide protection against future quantum computers.");

    // Confirm
    if !skip_confirm {
        println!();
        if !prompt_confirm("Send this quantum-private transaction?")? {
            println!("Aborted.");
            return Ok(());
        }
    }

    // Build and sign quantum-private transaction
    println!();
    println!("Building quantum-private transaction...");

    let tx = builder.build_pq_transfer(&pq_recipient, amount_picocredits, fee)?;

    // Serialize transaction for submission
    println!("Signing transaction with dual signatures (Schnorr + ML-DSA)...");
    let tx_bytes = bincode::serialize(&tx)
        .map_err(|e| anyhow!("Failed to serialize transaction: {}", e))?;
    let tx_hex = hex::encode(&tx_bytes);

    // Submit transaction
    println!("Submitting quantum-private transaction...");

    // Use pq_tx_submit endpoint for PQ transactions
    let tx_hash = rpc.submit_pq_transaction(&tx_hex).await?;

    println!();
    print_success("Quantum-private transaction sent!");
    println!();
    println!("Transaction hash: {}", tx_hash);
    println!();
    println!("This transaction uses:");
    println!("  • ML-KEM-768 for key encapsulation (recipient output)");
    println!("  • ML-DSA-65 for signatures (quantum-safe authentication)");

    // Update sync height
    wallet.set_sync_height(height);
    wallet.set_discovery_state(rpc.discovery(), &password)?;
    wallet.save(wallet_path)?;

    Ok(())
}
