// Copyright (c) 2024 Botho Foundation

//! Snapshot command for creating and loading UTXO snapshots.

use anyhow::{Context, Result};
use std::path::Path;
use tracing::info;

use crate::{
    config::{ledger_db_path_from_config, Config},
    ledger::{Ledger, UtxoSnapshotV2},
};

/// Create a snapshot of the current UTXO set.
pub fn create(config_path: &Path, output: &str) -> Result<()> {
    let _config =
        Config::load(config_path).context("Failed to load config. Run 'botho init' first.")?;

    let ledger_path = ledger_db_path_from_config(config_path);
    let ledger = Ledger::open(&ledger_path).context("Failed to open ledger")?;

    let output_path = Path::new(output);

    println!("Creating UTXO snapshot...");

    let state = ledger.get_chain_state()?;
    println!("Current chain state:");
    println!("  Height: {}", state.height);
    println!("  Total mined: {} picocredits", state.total_mined);
    println!("  Fees burned: {} picocredits", state.total_fees_burned);

    let size = ledger.write_snapshot_to_file(output_path)?;

    println!("\nSnapshot created successfully!");
    println!("  File: {}", output_path.display());
    println!(
        "  Size: {} bytes ({:.2} MB)",
        size,
        size as f64 / 1_048_576.0
    );
    println!("  Block hash: {}", hex::encode(state.tip_hash));

    info!(
        path = %output_path.display(),
        size_bytes = size,
        height = state.height,
        "Snapshot created"
    );

    Ok(())
}

/// Load a snapshot from a file.
pub fn load(config_path: &Path, input: &str, verify_hash: Option<&str>) -> Result<()> {
    let _config =
        Config::load(config_path).context("Failed to load config. Run 'botho init' first.")?;

    let ledger_path = ledger_db_path_from_config(config_path);
    let ledger = Ledger::open(&ledger_path).context("Failed to open ledger")?;

    let input_path = Path::new(input);

    // Parse optional block hash for verification
    let expected_hash = if let Some(hash_hex) = verify_hash {
        let bytes = hex::decode(hash_hex).context("Invalid block hash hex")?;
        if bytes.len() != 32 {
            anyhow::bail!("Block hash must be 32 bytes (64 hex characters)");
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Some(arr)
    } else {
        None
    };

    println!("Loading UTXO snapshot from {}...", input_path.display());

    // First, show snapshot info
    let file = std::fs::File::open(input_path).context("Failed to open snapshot file")?;
    let reader = std::io::BufReader::new(file);
    let snapshot = UtxoSnapshotV2::read_from(reader).context("Failed to read snapshot")?;

    println!("Snapshot information:");
    println!("  Version: {}", snapshot.version);
    println!("  Height: {}", snapshot.height);
    println!("  UTXOs: {}", snapshot.utxo_count);
    println!("  Derivation contexts: {}", snapshot.context_count);
    println!("  Key images: {}", snapshot.key_image_count);
    println!("  Block hash: {}", hex::encode(snapshot.block_hash));
    println!("  Genesis hash: {}", hex::encode(snapshot.genesis_hash));
    println!("  Commitment: {}", hex::encode(snapshot.commitment));
    println!("  Compressed size: {} bytes", snapshot.compressed_size());

    if verify_hash.is_some() {
        println!("  Verifying against provided block hash...");
    }

    println!("\nVerifying snapshot integrity...");
    snapshot.verify().context("Snapshot verification failed")?;
    println!("  Commitment and Merkle roots verified!");

    println!("\nLoading snapshot into ledger...");
    println!("  (V2 snapshots load only into a freshly initialized, genesis-only ledger)");

    let utxo_count = ledger.load_from_snapshot(&snapshot, expected_hash.as_ref())?;

    println!("\nSnapshot loaded successfully!");
    println!("  UTXOs loaded: {}", utxo_count);
    println!("  Height set to: {}", snapshot.height);
    println!("\nNote: Run 'botho run' to sync remaining blocks from network.");

    info!(
        path = %input_path.display(),
        height = snapshot.height,
        utxo_count = utxo_count,
        "Snapshot loaded"
    );

    Ok(())
}

/// Show information about a snapshot file.
pub fn info(file: &str) -> Result<()> {
    let file_path = Path::new(file);

    let file = std::fs::File::open(file_path).context("Failed to open snapshot file")?;

    let metadata = file.metadata()?;
    let file_size = metadata.len();

    let reader = std::io::BufReader::new(file);
    let snapshot = UtxoSnapshotV2::read_from(reader).context("Failed to read snapshot")?;

    println!("Snapshot Information");
    println!("====================");
    println!();
    println!("File: {}", file_path.display());
    println!(
        "File size: {} bytes ({:.2} MB)",
        file_size,
        file_size as f64 / 1_048_576.0
    );
    println!();
    println!("Format version: {}", snapshot.version);
    println!("Block height: {}", snapshot.height);
    println!("Block hash: {}", hex::encode(snapshot.block_hash));
    println!("Genesis hash: {}", hex::encode(snapshot.genesis_hash));
    println!(
        "Storage schema: {}",
        String::from_utf8_lossy(&snapshot.storage_schema)
    );
    println!("Commitment: {}", hex::encode(snapshot.commitment));
    println!();
    println!("Contents:");
    println!("  UTXOs: {}", snapshot.utxo_count);
    println!("  Derivation contexts: {}", snapshot.context_count);
    println!("  Key images: {}", snapshot.key_image_count);
    println!("  Cluster wealth entries: {}", snapshot.cluster_count);
    println!(
        "  Bridge-import clusters: {}",
        snapshot.import_cluster_count
    );
    println!();
    println!("Data sizes:");
    println!("  Tip block: {} bytes", snapshot.tip_block_data.len());
    println!("  UTXO data: {} bytes", snapshot.utxo_data.len());
    println!("  Context data: {} bytes", snapshot.context_data.len());
    println!(
        "  Address index data: {} bytes",
        snapshot.address_index_data.len()
    );
    println!("  Key image data: {} bytes", snapshot.key_image_data.len());
    println!(
        "  Cluster wealth data: {} bytes",
        snapshot.cluster_wealth_data.len()
    );
    println!("  Compressed total: {} bytes", snapshot.compressed_size());
    println!();
    println!("Merkle roots:");
    println!("  UTXO: {}", hex::encode(snapshot.utxo_merkle_root));
    println!(
        "  Derivation context: {}",
        hex::encode(snapshot.context_merkle_root)
    );
    println!(
        "  Key image: {}",
        hex::encode(snapshot.key_image_merkle_root)
    );
    println!();
    println!("Chain state at snapshot:");
    println!(
        "  Total mined: {} picocredits",
        snapshot.chain_state.total_mined
    );
    println!(
        "  Fees burned: {} picocredits",
        snapshot.chain_state.total_fees_burned
    );
    println!("  Difficulty: {}", snapshot.chain_state.difficulty);
    println!(
        "  Current reward: {} picocredits",
        snapshot.chain_state.current_reward
    );
    println!();

    // Verify integrity
    print!("Verifying integrity... ");
    match snapshot.verify() {
        Ok(_) => println!("OK"),
        Err(e) => println!("FAILED: {}", e),
    }

    Ok(())
}
