//! Transaction history command

use anyhow::Result;
use std::path::Path;

use crate::{
    discovery::NodeDiscovery,
    rpc_pool::RpcPool,
    storage::EncryptedWallet,
    transaction::{format_amount, OwnedUtxo},
};

use super::{decrypt_wallet_with_rate_limiting, print_error, print_success, print_warning};

/// Transaction type from RPC
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxCryptoType {
    /// CLSAG ring signatures (standard per ADR-0001)
    Clsag,
    /// Hybrid (both classical and PQ signatures)
    Hybrid,
    /// Unknown type
    Unknown,
}

impl TxCryptoType {
    /// Parse from RPC response string
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "clsag" => Self::Clsag,
            "hybrid" => Self::Hybrid,
            _ => Self::Unknown,
        }
    }

    /// Short code for table display
    pub fn code(&self) -> &'static str {
        match self {
            Self::Clsag => "CLSAG",
            Self::Hybrid => "HYBRID",
            Self::Unknown => "???",
        }
    }

    /// ANSI color code for terminal display
    pub fn color(&self) -> &'static str {
        match self {
            Self::Clsag => "\x1b[34m",   // Blue
            Self::Hybrid => "\x1b[36m",  // Cyan
            Self::Unknown => "\x1b[37m", // White
        }
    }
}

/// Transaction history entry
#[derive(Debug)]
pub struct HistoryEntry {
    /// Transaction hash
    pub tx_hash: String,
    /// Amount received (in picocredits)
    pub amount: u64,
    /// Block height
    pub block_height: u64,
    /// Confirmations
    pub confirmations: u64,
    /// Cryptographic type
    pub crypto_type: TxCryptoType,
    /// Transaction fee
    pub fee: u64,
}

/// Run the history command
pub async fn run(wallet_path: &Path, limit: usize) -> Result<()> {
    // Check wallet exists
    if !EncryptedWallet::exists(wallet_path) {
        print_error("No wallet found. Run 'botho-wallet init' first.");
        return Ok(());
    }

    // Load and decrypt wallet with rate limiting protection (verify password)
    let (wallet, _mnemonic, password) = decrypt_wallet_with_rate_limiting(wallet_path)?;

    // Get wallet's data directory for UTXO cache
    let data_dir = wallet_path.parent().unwrap_or(Path::new("."));
    let utxo_cache_path = data_dir.join("utxos.json");

    // Load cached UTXOs if available
    let utxos: Vec<OwnedUtxo> = if utxo_cache_path.exists() {
        let json = std::fs::read_to_string(&utxo_cache_path)?;
        serde_json::from_str(&json).unwrap_or_default()
    } else {
        print_warning("No transaction cache found. Run 'botho-wallet sync' to sync transactions.");
        return Ok(());
    };

    if utxos.is_empty() {
        println!();
        println!("No transactions found.");
        println!("Run 'botho-wallet sync' to scan for transactions.");
        return Ok(());
    }

    // Connect to RPC to get transaction details
    let discovery = wallet
        .get_discovery_state(&password)?
        .unwrap_or_else(NodeDiscovery::new);

    let mut rpc = RpcPool::new(discovery);
    if let Err(e) = rpc.connect().await {
        print_warning(&format!(
            "Could not connect to RPC: {}. Showing cached data only.",
            e
        ));
        // Show basic history without crypto type
        print_basic_history(&utxos, limit);
        return Ok(());
    }

    // Fetch transaction details from RPC
    let mut history: Vec<HistoryEntry> = Vec::new();

    for utxo in utxos.iter().take(limit) {
        let tx_hash_hex = hex::encode(utxo.tx_hash);

        // Query RPC for transaction details
        match rpc.get_transaction(&tx_hash_hex).await {
            Ok(tx_info) => {
                let crypto_type = tx_info
                    .tx_type
                    .as_deref()
                    .map(TxCryptoType::from_str)
                    .unwrap_or(TxCryptoType::Unknown);

                history.push(HistoryEntry {
                    tx_hash: tx_hash_hex,
                    amount: utxo.amount,
                    block_height: tx_info.block_height.unwrap_or(utxo.created_at),
                    confirmations: tx_info.confirmations,
                    crypto_type,
                    fee: tx_info.fee.unwrap_or(0),
                });
            }
            Err(_) => {
                // RPC error, use cached data
                history.push(HistoryEntry {
                    tx_hash: tx_hash_hex,
                    amount: utxo.amount,
                    block_height: utxo.created_at,
                    confirmations: 0,
                    crypto_type: TxCryptoType::Unknown,
                    fee: 0,
                });
            }
        }
    }

    // Sort by block height (most recent first)
    history.sort_by(|a, b| b.block_height.cmp(&a.block_height));

    // Print header
    println!();
    print_success(&format!(
        "Transaction History ({} transactions)",
        history.len()
    ));
    println!();
    println!(
        "{:<10} {:<14} {:<12} {:<8} {:<12}",
        "Height", "Amount", "Type", "Conf", "Tx Hash"
    );
    println!("{}", "-".repeat(70));

    // Print transactions
    for entry in &history {
        let amount_str = format_amount(entry.amount);
        let tx_hash_short = &entry.tx_hash[..12];
        let type_colored = format!(
            "{}{}{}",
            entry.crypto_type.color(),
            entry.crypto_type.code(),
            "\x1b[0m" // Reset color
        );

        println!(
            "{:<10} {:<14} {:<12} {:<8} {}...",
            entry.block_height, amount_str, type_colored, entry.confirmations, tx_hash_short
        );
    }

    println!();

    // Print legend
    println!("Transaction Types:");
    println!(
        "  {}CLSAG\x1b[0m  = Ring signatures (standard)",
        TxCryptoType::Clsag.color()
    );
    println!(
        "  {}HYBRID\x1b[0m = Hybrid classical+PQ signatures",
        TxCryptoType::Hybrid.color()
    );
    println!();

    Ok(())
}

/// Print basic history without RPC connection
fn print_basic_history(utxos: &[OwnedUtxo], limit: usize) {
    // Sort by height (most recent first)
    let mut sorted: Vec<_> = utxos.iter().collect();
    sorted.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    println!();
    print_success(&format!(
        "Transaction History ({} transactions, cached)",
        sorted.len().min(limit)
    ));
    println!();
    println!("{:<10} {:<14} {:<12}", "Height", "Amount", "Tx Hash");
    println!("{}", "-".repeat(50));

    for utxo in sorted.iter().take(limit) {
        let amount_str = format_amount(utxo.amount);
        let tx_hash_hex = hex::encode(utxo.tx_hash);
        let tx_hash_short = &tx_hash_hex[..12];

        println!(
            "{:<10} {:<14} {}...",
            utxo.created_at, amount_str, tx_hash_short
        );
    }

    println!();
    println!("Note: Connect to a node to see transaction types.");
    println!();
}
