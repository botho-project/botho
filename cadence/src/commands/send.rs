// Copyright (c) 2024 Cadence Foundation

use anyhow::{Context, Result};
use mc_account_keys::PublicAddress;
use std::path::Path;

use crate::config::{ledger_db_path_from_config, Config};
use crate::ledger::Ledger;
use crate::transaction::{Transaction, TxInput, TxOutput, UtxoId};
use crate::wallet::Wallet;

/// Minimum transaction fee in picocredits (0.0001 credits)
const MIN_FEE: u64 = 100_000_000;

/// Send credits to an address
pub fn run(config_path: &Path, address_str: &str, amount_str: &str) -> Result<()> {
    let config = Config::load(config_path)
        .context("No wallet found. Run 'cadence init' first.")?;

    let wallet = Wallet::from_mnemonic(&config.wallet.mnemonic)?;
    let our_address = wallet.default_address();

    // Parse recipient address (format: "view:<hex>,spend:<hex>")
    let recipient = parse_address(address_str)?;

    // Parse amount (in credits, convert to picocredits)
    let amount = parse_amount(amount_str)?;

    // Open ledger
    let ledger_path = ledger_db_path_from_config(config_path);
    let ledger = Ledger::open(&ledger_path)
        .map_err(|e| anyhow::anyhow!("Failed to open ledger: {}", e))?;

    // Get chain state
    let state = ledger
        .get_chain_state()
        .map_err(|e| anyhow::anyhow!("Failed to get chain state: {}", e))?;

    // Get our UTXOs
    let utxos = ledger
        .get_utxos_for_address(&our_address)
        .map_err(|e| anyhow::anyhow!("Failed to get UTXOs: {}", e))?;

    let total_balance: u64 = utxos.iter().map(|u| u.output.amount).sum();
    let required = amount + MIN_FEE;

    if total_balance < required {
        return Err(anyhow::anyhow!(
            "Insufficient balance: have {:.12} credits, need {:.12} credits (including {:.12} fee)",
            total_balance as f64 / 1_000_000_000_000.0,
            required as f64 / 1_000_000_000_000.0,
            MIN_FEE as f64 / 1_000_000_000_000.0
        ));
    }

    // Select UTXOs to spend (simple: use enough to cover amount + fee)
    let mut selected_utxos = Vec::new();
    let mut selected_amount = 0u64;

    for utxo in &utxos {
        if selected_amount >= required {
            break;
        }
        selected_utxos.push(utxo.clone());
        selected_amount += utxo.output.amount;
    }

    // Build inputs (for now, without real signatures)
    let inputs: Vec<TxInput> = selected_utxos
        .iter()
        .map(|utxo| TxInput {
            tx_hash: utxo.id.tx_hash,
            output_index: utxo.id.output_index,
            signature: vec![0u8; 64], // Placeholder signature
        })
        .collect();

    // Build outputs
    let mut outputs = Vec::new();

    // Output to recipient
    outputs.push(TxOutput::new(amount, &recipient));

    // Change output (if any)
    let change = selected_amount - amount - MIN_FEE;
    if change > 0 {
        outputs.push(TxOutput::new(change, &our_address));
    }

    // Create the transaction
    let tx = Transaction::new(inputs, outputs, MIN_FEE, state.height);

    // Display transaction details
    println!();
    println!("=== Transaction Created ===");
    println!("From: your wallet");
    println!("To: {}", address_str);
    println!("Amount: {:.12} credits", amount as f64 / 1_000_000_000_000.0);
    println!("Fee: {:.12} credits", MIN_FEE as f64 / 1_000_000_000_000.0);
    if change > 0 {
        println!("Change: {:.12} credits", change as f64 / 1_000_000_000_000.0);
    }
    println!();
    println!("Transaction hash: {}", hex::encode(&tx.hash()[0..16]));
    println!("Inputs: {}", tx.inputs.len());
    println!("Outputs: {}", tx.outputs.len());
    println!();
    println!("Transaction created but NOT YET BROADCAST.");
    println!("(Transaction broadcasting requires running a node with 'cadence run')");
    println!();

    // TODO: Store transaction in mempool or broadcast to network
    // For now, we just show the transaction details

    Ok(())
}

/// Parse an address string in the format "view:<hex>,spend:<hex>"
fn parse_address(s: &str) -> Result<PublicAddress> {
    // Expected format: "view:abcd1234...,spend:efgh5678..."
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 2 {
        return Err(anyhow::anyhow!(
            "Invalid address format. Expected: view:<hex>,spend:<hex>"
        ));
    }

    let view_part = parts[0].trim();
    let spend_part = parts[1].trim();

    if !view_part.starts_with("view:") {
        return Err(anyhow::anyhow!("Address must start with 'view:'"));
    }
    if !spend_part.starts_with("spend:") {
        return Err(anyhow::anyhow!("Address must contain 'spend:'"));
    }

    let view_hex = &view_part[5..];
    let spend_hex = &spend_part[6..];

    let view_bytes = hex::decode(view_hex)
        .context("Invalid hex in view key")?;
    let spend_bytes = hex::decode(spend_hex)
        .context("Invalid hex in spend key")?;

    if view_bytes.len() != 32 || spend_bytes.len() != 32 {
        return Err(anyhow::anyhow!("View and spend keys must be 32 bytes each"));
    }

    // Create PublicAddress from the keys
    let view_key = mc_crypto_keys::RistrettoPublic::try_from(&view_bytes[..])
        .map_err(|e| anyhow::anyhow!("Invalid view key: {}", e))?;
    let spend_key = mc_crypto_keys::RistrettoPublic::try_from(&spend_bytes[..])
        .map_err(|e| anyhow::anyhow!("Invalid spend key: {}", e))?;

    Ok(PublicAddress::new(&spend_key, &view_key))
}

/// Parse an amount string (in credits) to picocredits
fn parse_amount(s: &str) -> Result<u64> {
    let amount: f64 = s.parse()
        .context("Invalid amount. Please enter a number.")?;

    if amount <= 0.0 {
        return Err(anyhow::anyhow!("Amount must be positive"));
    }

    // Convert credits to picocredits
    let picocredits = (amount * 1_000_000_000_000.0) as u64;

    if picocredits == 0 {
        return Err(anyhow::anyhow!("Amount too small"));
    }

    Ok(picocredits)
}
