// Copyright (c) 2024 Botho Foundation

use anyhow::{Context, Result};
use bth_cluster_tax::{FeeConfig, TransactionType};
use std::fs;
use std::path::Path;

use crate::address::Address;
use crate::config::{ledger_db_path_from_config, Config};
use crate::ledger::Ledger;
use crate::transaction::{MemoPayload, Transaction, TxOutput};
use crate::wallet::Wallet;

#[cfg(feature = "pq")]
use crate::transaction_pq::QuantumPrivateTransaction;

/// Pending transactions file name
const PENDING_TXS_FILE: &str = "pending_txs.bin";

/// Pending quantum-private transactions file name
#[cfg(feature = "pq")]
const PENDING_PQ_TXS_FILE: &str = "pending_pq_txs.bin";

/// Send BTH to an address
///
/// If `private` is true, uses ring signatures to hide which UTXO is being spent.
/// If `quantum` is true, uses post-quantum cryptography (ML-KEM + ML-DSA).
/// If recipient has a quantum address (botho://1q/), quantum mode is auto-enabled.
/// If `memo` is provided, it will be encrypted and attached to the recipient's output.
pub fn run(config_path: &Path, address_str: &str, amount_str: &str, private: bool, quantum: bool, memo: Option<&str>) -> Result<()> {
    let config = Config::load(config_path)
        .context("No wallet found. Run 'botho init' first.")?;

    let wallet_config = config.wallet
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("No wallet configured. Run 'botho init' first."))?;

    let wallet = Wallet::from_mnemonic(&wallet_config.mnemonic)?;
    let our_address = wallet.default_address();

    // Get the network type
    let network = config.network_type();

    // Parse recipient address and validate it's for the correct network
    let parsed_address = Address::parse_for_network(address_str, network)?;

    // Auto-enable quantum mode if recipient has a quantum address
    #[cfg(feature = "pq")]
    let quantum = quantum || parsed_address.is_quantum();

    // Get the classical address for standard operations
    let recipient = parsed_address.public_address();

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

    // Calculate fee using the cluster-tax fee curve
    // All transactions are now private (Standard-Private with CLSAG or PQ-Private with LION)
    let fee_config = FeeConfig::default();
    let tx_type = TransactionType::Hidden; // Standard-Private with CLSAG
    let _ = private; // Deprecated: all transactions are now private

    // Estimate fee based on typical transaction size
    // TODO: Compute cluster wealth from input UTXO cluster_tags
    // See botho/src/mempool.rs module docs for implementation requirements
    let cluster_wealth = 0u64;
    let num_memos = if memo.is_some() { 1 } else { 0 };
    let fee = fee_config.estimate_typical_fee(tx_type, cluster_wealth, num_memos);

    // Ensure minimum fee of at least 1 picocredit
    let fee = fee.max(1);

    let total_balance: u64 = utxos.iter().map(|u| u.output.amount).sum();
    let required = amount + fee;

    if total_balance < required {
        return Err(anyhow::anyhow!(
            "Insufficient balance: have {:.12} BTH, need {:.12} BTH (including {:.12} fee)",
            total_balance as f64 / 1_000_000_000_000.0,
            required as f64 / 1_000_000_000_000.0,
            fee as f64 / 1_000_000_000_000.0
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

    // Handle quantum-private transactions
    #[cfg(feature = "pq")]
    if quantum {
        let pq_recipient = parsed_address.quantum_address()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!(
                "Quantum transaction requires a quantum-safe address (botho://1q/...)"
            ))?;

        // For PQ transactions, use PqHidden type to estimate fee
        let pq_fee = fee_config.estimate_typical_fee(TransactionType::PqHidden, cluster_wealth, num_memos);
        return run_quantum(
            config_path,
            &pq_recipient,
            amount,
            pq_fee,
            selected_utxos,
            state.height,
            &wallet,
            network,
        );
    }

    #[cfg(not(feature = "pq"))]
    if quantum {
        return Err(anyhow::anyhow!(
            "Quantum-private transactions require the 'pq' feature. Rebuild with: cargo build --features pq"
        ));
    }

    // Build outputs
    let mut outputs = Vec::new();

    // Output to recipient (with optional memo)
    let memo_payload = memo.map(MemoPayload::destination);
    outputs.push(TxOutput::new_with_memo(amount, &recipient, memo_payload));

    // Change output (if any) - no memo on change
    let change = selected_amount - amount - fee;
    if change > 0 {
        outputs.push(TxOutput::new(change, &our_address));
    }

    // Create the transaction (always private with CLSAG ring signatures)
    // Note: All transactions are now private by default for sender anonymity
    println!();
    println!("Creating private transaction with CLSAG ring signatures...");

    let tx = wallet.create_private_transaction(
        &selected_utxos,
        outputs,
        fee,
        state.height,
        &ledger,
    )?;

    let tx_hash = tx.hash();
    let num_inputs = tx.inputs.len();
    let tx_type_str = "Private";

    // Display transaction details
    println!();
    println!("=== {} Transaction Created ===", tx_type_str);
    println!("From: your wallet");
    println!("To: {}", address_str);
    println!("Amount: {:.12} BTH", amount as f64 / 1_000_000_000_000.0);
    println!();
    println!("Fee breakdown:");
    println!("  Type: {} (ring signatures)", tx_type_str);
    println!("  Base rate: {} per byte", fee_config.fee_per_byte);
    if let Some(memo_text) = memo {
        println!("  Memo: \"{}\" (+{} per memo)",
            if memo_text.len() > 30 { format!("{}...", &memo_text[..30]) } else { memo_text.to_string() },
            fee_config.fee_per_memo);
    }
    println!("  Total fee: {:.12} BTH", fee as f64 / 1_000_000_000_000.0);
    println!();
    if change > 0 {
        println!("Change: {:.12} BTH", change as f64 / 1_000_000_000_000.0);
    }
    println!("Privacy: Ring signatures hide which UTXO was spent");
    println!();
    println!("Transaction hash: {}", hex::encode(&tx_hash[0..16]));
    println!("Inputs: {}", num_inputs);
    println!("Outputs: {}", tx.outputs.len());

    // Save transaction to pending file
    let pending_path = config_path.parent()
        .unwrap_or(Path::new("."))
        .join(PENDING_TXS_FILE);

    save_pending_tx(&pending_path, &tx)?;

    println!();
    println!("Transaction saved to pending queue.");
    println!("Start the node with 'botho run' to broadcast it.");
    println!();

    Ok(())
}

/// Save a transaction to the pending transactions file
fn save_pending_tx(path: &Path, tx: &Transaction) -> Result<()> {
    // Load existing pending transactions
    let mut pending: Vec<Transaction> = load_pending_txs(path).unwrap_or_default();

    // Check if already exists
    let tx_hash = tx.hash();
    if pending.iter().any(|t| t.hash() == tx_hash) {
        return Err(anyhow::anyhow!("Transaction already in pending queue"));
    }

    // Add new transaction
    pending.push(tx.clone());

    // Save back
    let bytes = bincode::serialize(&pending)
        .context("Failed to serialize pending transactions")?;
    fs::write(path, bytes)
        .context("Failed to save pending transactions")?;

    Ok(())
}

/// Load pending transactions from file
pub fn load_pending_txs(path: &Path) -> Result<Vec<Transaction>> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let bytes = fs::read(path)
        .context("Failed to read pending transactions")?;

    let pending: Vec<Transaction> = bincode::deserialize(&bytes)
        .context("Failed to deserialize pending transactions")?;

    Ok(pending)
}

/// Clear pending transactions file
pub fn clear_pending_txs(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_file(path)
            .context("Failed to remove pending transactions file")?;
    }
    Ok(())
}

/// Parse an amount string (in credits) to picocredits
fn parse_amount(s: &str) -> Result<u64> {
    let amount: f64 = s.parse()
        .context("Invalid amount. Please enter a number.")?;

    if amount <= 0.0 {
        return Err(anyhow::anyhow!("Amount must be positive"));
    }

    // Maximum safe amount that can be converted without overflow
    // u64::MAX / 1_000_000_000_000 ≈ 18,446 credits
    const MAX_CREDITS: f64 = 18_000.0; // Conservative limit

    if amount > MAX_CREDITS {
        return Err(anyhow::anyhow!(
            "Amount too large. Maximum is {} credits",
            MAX_CREDITS
        ));
    }

    // Convert credits to picocredits with explicit rounding
    let picocredits_f64 = (amount * 1_000_000_000_000.0).round();

    // Verify the conversion is valid
    if picocredits_f64 < 0.0 || picocredits_f64 > u64::MAX as f64 {
        return Err(anyhow::anyhow!("Amount conversion overflow"));
    }

    let picocredits = picocredits_f64 as u64;

    if picocredits == 0 {
        return Err(anyhow::anyhow!("Amount too small"));
    }

    Ok(picocredits)
}

/// Handle quantum-private transaction creation
#[cfg(feature = "pq")]
fn run_quantum(
    config_path: &Path,
    recipient: &bth_account_keys::QuantumSafePublicAddress,
    amount: u64,
    fee: u64,
    selected_utxos: Vec<crate::transaction::Utxo>,
    current_height: u64,
    wallet: &Wallet,
    network: bth_transaction_types::constants::Network,
) -> Result<()> {
    use crate::transaction_pq::calculate_pq_fee;
    use crate::address::format_quantum_address;

    // Calculate quantum-safe fee (larger transactions)
    let pq_fee = calculate_pq_fee(selected_utxos.len(), 2); // 2 outputs: recipient + change
    let effective_fee = std::cmp::max(fee, pq_fee);

    println!();
    println!("Creating quantum-private transaction...");
    println!("Using ML-KEM-768 for key exchange, ML-DSA-65 for signatures");

    // Create the quantum-private transaction
    let tx = wallet.create_quantum_private_transaction(
        &selected_utxos,
        recipient,
        amount,
        effective_fee,
        current_height,
    )?;

    let tx_hash = tx.hash();
    let num_inputs = tx.inputs.len();

    // Format address for display (truncate long PQ address)
    let addr_display = format_quantum_address(recipient, network);
    let addr_short = if addr_display.len() > 60 {
        format!("{}...{}", &addr_display[..30], &addr_display[addr_display.len()-20..])
    } else {
        addr_display
    };

    // Display transaction details
    println!();
    println!("=== Quantum-Private Transaction Created ===");
    println!("From: your wallet");
    println!("To: {}", addr_short);
    println!("Amount: {:.12} BTH", amount as f64 / 1_000_000_000_000.0);
    println!();
    println!("Fee breakdown:");
    println!("  Type: PQ-Private (LION ring signatures)");
    println!("  Size: ~65 KB (lattice-based signatures)");
    println!("  Fee: {:.12} BTH (size-based)", effective_fee as f64 / 1_000_000_000_000.0);
    println!();

    let selected_amount: u64 = selected_utxos.iter().map(|u| u.output.amount).sum();
    let change = selected_amount - amount - effective_fee;
    if change > 0 {
        println!("Change: {:.12} BTH", change as f64 / 1_000_000_000_000.0);
    }

    println!();
    println!("Security:");
    println!("  - Outputs: ML-KEM-768 encapsulation (1088 bytes)");
    println!("  - Inputs: Schnorr + ML-DSA-65 signatures (64 + 3309 bytes)");
    println!("  - Protected against \"harvest now, decrypt later\" attacks");
    println!();
    println!("Transaction hash: {}", hex::encode(&tx_hash[0..16]));
    println!("Inputs: {}", num_inputs);
    println!("Outputs: {}", tx.outputs.len());

    // Save transaction to pending file
    let pending_path = config_path.parent()
        .unwrap_or(Path::new("."))
        .join(PENDING_PQ_TXS_FILE);

    save_pending_pq_tx(&pending_path, &tx)?;

    println!();
    println!("Quantum-private transaction saved to pending queue.");
    println!("Start the node with 'botho run' to broadcast it.");
    println!();

    Ok(())
}

/// Save a quantum-private transaction to the pending transactions file
#[cfg(feature = "pq")]
fn save_pending_pq_tx(path: &Path, tx: &QuantumPrivateTransaction) -> Result<()> {
    // Load existing pending transactions
    let mut pending: Vec<QuantumPrivateTransaction> = load_pending_pq_txs(path).unwrap_or_default();

    // Check if already exists
    let tx_hash = tx.hash();
    if pending.iter().any(|t| t.hash() == tx_hash) {
        return Err(anyhow::anyhow!("Transaction already in pending queue"));
    }

    // Add new transaction
    pending.push(tx.clone());

    // Save back
    let bytes = bincode::serialize(&pending)
        .context("Failed to serialize pending quantum-private transactions")?;
    fs::write(path, bytes)
        .context("Failed to save pending quantum-private transactions")?;

    Ok(())
}

/// Load pending quantum-private transactions from file
#[cfg(feature = "pq")]
pub fn load_pending_pq_txs(path: &Path) -> Result<Vec<QuantumPrivateTransaction>> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let bytes = fs::read(path)
        .context("Failed to read pending quantum-private transactions")?;

    let pending: Vec<QuantumPrivateTransaction> = bincode::deserialize(&bytes)
        .context("Failed to deserialize pending quantum-private transactions")?;

    Ok(pending)
}
