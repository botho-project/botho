//! Balance check command

use anyhow::Result;
use std::path::Path;

use crate::{
    discovery::NodeDiscovery, keys::WalletKeys, rpc_pool::RpcPool, storage::EncryptedWallet,
    transaction::format_amount,
};

use crate::transaction::sync_wallet;

use super::{decrypt_wallet_with_rate_limiting, print_error, print_success};

/// Run the balance command
pub async fn run(wallet_path: &Path, detailed: bool) -> Result<()> {
    // Check wallet exists
    if !EncryptedWallet::exists(wallet_path) {
        print_error("No wallet found. Run 'botho-wallet init' first.");
        return Ok(());
    }

    // Load and decrypt wallet with rate limiting protection
    let (wallet, mnemonic, password) = decrypt_wallet_with_rate_limiting(wallet_path)?;

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

    // Sync wallet and display balance
    println!("Syncing wallet...");

    #[cfg(feature = "pq")]
    {
        display_balance_pq(&mut rpc, &keys, &wallet, detailed).await
    }

    #[cfg(not(feature = "pq"))]
    {
        display_balance_classical(&mut rpc, &keys, &wallet, detailed).await
    }
}

/// Display balance for classical-only mode (no PQ feature)
#[cfg(not(feature = "pq"))]
async fn display_balance_classical(
    rpc: &mut RpcPool,
    keys: &WalletKeys,
    wallet: &EncryptedWallet,
    detailed: bool,
) -> Result<()> {
    let (utxos, height) = sync_wallet(rpc, keys, wallet.sync_height).await?;

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

/// Display balance with hybrid (quantum-safe) UTXO scanning.
///
/// Since issue #970 there is a single scan path: `sync_wallet` detects both
/// classical and hybrid (ML-KEM) outputs. Hybrid UTXOs are those carrying a KEM
/// ciphertext; the breakdown below classifies discovered UTXOs on that field.
#[cfg(feature = "pq")]
async fn display_balance_pq(
    rpc: &mut RpcPool,
    keys: &WalletKeys,
    wallet: &EncryptedWallet,
    detailed: bool,
) -> Result<()> {
    let (utxos, height) = sync_wallet(rpc, keys, wallet.sync_height).await?;

    // Classify on the unified scan result: KEM ciphertext present => hybrid.
    let (pq_utxos, classical_utxos): (Vec<_>, Vec<_>) =
        utxos.iter().partition(|u| u.kem_ciphertext.is_some());

    let classical_total: u64 = classical_utxos.iter().map(|u| u.amount).sum();
    let pq_total: u64 = pq_utxos.iter().map(|u| u.amount).sum();
    let total = classical_total + pq_total;

    println!();

    // Show combined balance prominently
    print_success(&format!("Total Balance: {}", format_amount(total)));

    // Show breakdown if there are both types or if detailed mode
    if pq_total > 0 || detailed {
        println!();
        println!("Balance Breakdown:");
        println!("  Classical:    {}", format_amount(classical_total));
        println!("  Quantum-safe: {}", format_amount(pq_total));
    }

    println!();
    println!("Synced to block {}", height);

    if detailed {
        // Show classical UTXOs
        if !classical_utxos.is_empty() {
            println!();
            println!("Classical UTXOs ({}):", classical_utxos.len());
            for (i, utxo) in classical_utxos.iter().enumerate() {
                println!(
                    "  {}. {} (block {})",
                    i + 1,
                    format_amount(utxo.amount),
                    utxo.created_at
                );
            }
        }

        // Show hybrid (quantum-safe) UTXOs
        if !pq_utxos.is_empty() {
            println!();
            println!("Quantum-safe UTXOs ({}):", pq_utxos.len());
            for (i, utxo) in pq_utxos.iter().enumerate() {
                println!(
                    "  {}. {} (block {}) [PQ]",
                    i + 1,
                    format_amount(utxo.amount),
                    utxo.created_at
                );
            }
        }

        // Show summary if no UTXOs at all
        if classical_utxos.is_empty() && pq_utxos.is_empty() {
            println!();
            println!("No UTXOs found.");
        }
    }

    Ok(())
}
