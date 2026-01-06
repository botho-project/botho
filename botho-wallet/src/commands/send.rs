//! Send transaction command

use anyhow::{anyhow, Result};
use std::path::Path;

use crate::{
    discovery::NodeDiscovery,
    fee_estimation::{format_fee_estimate, FeeEstimator, StoredTags},
    keys::WalletKeys,
    rpc_pool::RpcPool,
    storage::EncryptedWallet,
    transaction::{
        format_amount, parse_amount, sync_wallet, OwnedUtxo, TransactionBuilder, DUST_THRESHOLD,
        PICOCREDITS_PER_CAD,
    },
};

use super::{
    decrypt_wallet_with_rate_limiting, print_error, print_success, print_warning, prompt_confirm,
};

/// Run the send command
pub async fn run(
    wallet_path: &Path,
    address: &str,
    amount: f64,
    skip_confirm: bool,
    quantum_private: bool,
) -> Result<()> {
    // Handle deprecated quantum-private flag (per ADR-0001)
    if quantum_private {
        print_error("The --quantum-private flag has been removed per ADR-0001.");
        println!();
        println!("Post-quantum ring signatures were deprecated due to prohibitive size.");
        println!("Standard transactions use CLSAG rings with quantum-safe recipient privacy.");
        println!();
        println!("Please run your command without --quantum-private.");
        return Ok(());
    }

    // Standard transaction flow with CLSAG ring signatures
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

    // Load and decrypt wallet with rate limiting protection
    let (mut wallet, mnemonic, password) = decrypt_wallet_with_rate_limiting(wallet_path)?;

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

    // Calculate progressive fee using cluster-tax model
    // Use default base rate of 1 nanoBTH/byte (dynamic rate would come from
    // network)
    let fee_estimator = FeeEstimator::new();

    // Select UTXOs that would be used for this transaction (preview selection)
    let selected_utxos = select_utxos_for_preview(&utxos, amount_picocredits);

    // Prepare inputs for fee estimation (amount, tags)
    let default_tags = StoredTags::default();
    let inputs_for_estimation: Vec<(u64, &StoredTags)> = selected_utxos
        .iter()
        .map(|u| (u.amount, u.cluster_tags.as_ref().unwrap_or(&default_tags)))
        .collect();

    // Determine output count (recipient + change if applicable)
    let output_count = 2; // Assume we'll have change for estimation

    let fee_estimate = fee_estimator.estimate_fee(&inputs_for_estimation, output_count);
    let fee = fee_estimate.total_fee.max(1_000_000); // Enforce minimum fee

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
        println!(
            "  Fee:       {} (includes {} dust change)",
            format_amount(actual_fee),
            format_amount(expected_change)
        );
        print_warning("Change is below dust threshold - will be added to fee.");
    } else {
        println!("  Fee:       {}", format_amount(fee));
    }
    println!(
        "  Total:     {}",
        format_amount(amount_picocredits + actual_fee)
    );
    println!();

    // Show progressive fee breakdown
    println!("Fee breakdown (Cluster-Tax model):");
    println!(
        "{}",
        format_fee_estimate(&fee_estimate, PICOCREDITS_PER_CAD)
    );
    println!();

    if !dust_absorbed && expected_change > 0 {
        println!("  Change:        {}", format_amount(expected_change));
    }
    println!(
        "  Balance after: {}",
        format_amount(balance - amount_picocredits - actual_fee)
    );

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

        let view_bytes = hex::decode(parts[1]).map_err(|_| anyhow!("Invalid view key hex"))?;
        let spend_bytes = hex::decode(parts[2]).map_err(|_| anyhow!("Invalid spend key hex"))?;

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

/// Select UTXOs for fee estimation preview (largest-first selection).
///
/// This is a preview of which UTXOs would be selected for the given amount,
/// used for fee estimation before the actual transaction is built.
fn select_utxos_for_preview(utxos: &[OwnedUtxo], target_amount: u64) -> Vec<OwnedUtxo> {
    if utxos.is_empty() {
        return vec![];
    }

    // Sort by amount descending (largest first)
    let mut sorted: Vec<_> = utxos.to_vec();
    sorted.sort_by(|a, b| b.amount.cmp(&a.amount));

    let mut selected = Vec::new();
    let mut total = 0u64;

    // Select until we have enough to cover the target amount
    // (we add a buffer for fees, using 2x the amount as a rough estimate)
    let target_with_buffer = target_amount.saturating_mul(2);

    for utxo in sorted {
        if total >= target_with_buffer {
            break;
        }
        total = total.saturating_add(utxo.amount);
        selected.push(utxo);
    }

    selected
}
