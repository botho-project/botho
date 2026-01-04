//! Botho Exchange Scanner CLI
//!
//! A deposit detection tool for cryptocurrency exchanges.

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use botho_exchange_scanner::{
    config::ScannerConfig,
    output::{create_handler, OutputHandler},
    scanner::{ExchangeScanner, RpcOutput},
    subaddress::derive_subaddress_from_hex,
    sync::SyncState,
};

#[derive(Parser)]
#[command(name = "botho-exchange-scanner")]
#[command(about = "Exchange deposit scanner for Botho cryptocurrency")]
#[command(version)]
struct Cli {
    /// Path to configuration file
    #[arg(short, long, default_value = "scanner.toml")]
    config: PathBuf,

    /// Log level (trace, debug, info, warn, error)
    #[arg(short, long, default_value = "info")]
    log_level: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the scanner in continuous mode
    Scan {
        /// Start from a specific block height (overrides saved state)
        #[arg(long)]
        from_height: Option<u64>,

        /// Run once and exit (don't poll)
        #[arg(long)]
        once: bool,
    },

    /// Derive a subaddress for a customer
    DeriveAddress {
        /// Subaddress index
        #[arg(short, long)]
        index: u64,
    },

    /// Derive multiple subaddresses
    DeriveAddressBatch {
        /// Starting index
        #[arg(short, long)]
        start: u64,

        /// Number of addresses to derive
        #[arg(short, long, default_value = "10")]
        count: u64,
    },

    /// Show current sync status
    Status,

    /// Validate configuration file
    ValidateConfig,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging
    init_logging(&cli.log_level)?;

    // Load configuration
    let config = match ScannerConfig::from_file(&cli.config) {
        Ok(c) => c,
        Err(e) => {
            if matches!(cli.command, Commands::ValidateConfig) {
                eprintln!("Configuration validation failed: {}", e);
                std::process::exit(1);
            }
            anyhow::bail!("Failed to load config from {:?}: {}", cli.config, e);
        }
    };

    match cli.command {
        Commands::Scan { from_height, once } => run_scanner(&config, from_height, once).await,
        Commands::DeriveAddress { index } => derive_address(&config, index),
        Commands::DeriveAddressBatch { start, count } => {
            derive_address_batch(&config, start, count)
        }
        Commands::Status => show_status(&config),
        Commands::ValidateConfig => {
            println!("Configuration is valid.");
            println!("  RPC endpoints: {:?}", config.rpc_endpoints);
            println!(
                "  Subaddress range: {} - {} ({} addresses)",
                config.subaddress_min,
                config.subaddress_max,
                config.subaddress_count()
            );
            println!("  Min confirmations: {}", config.min_confirmations);
            println!("  Output mode: {:?}", config.output_mode);
            Ok(())
        }
    }
}

fn init_logging(level: &str) -> Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level));

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .init();

    Ok(())
}

async fn run_scanner(config: &ScannerConfig, from_height: Option<u64>, once: bool) -> Result<()> {
    tracing::info!("Starting exchange scanner");

    // Initialize scanner
    let scanner = ExchangeScanner::from_config(config)?;
    tracing::info!(
        "Scanner initialized with {} subaddresses",
        scanner.subaddress_count()
    );

    // Load or create sync state
    let mut sync_state = SyncState::load(&config.state_file)?;

    // Override start height if specified
    if let Some(height) = from_height {
        tracing::info!("Starting from specified height: {}", height);
        sync_state.last_scanned_height = if height > 0 { height - 1 } else { 0 };
    }

    // Create output handler
    let handler = create_handler(
        &config.output_mode,
        config.webhook_url.as_deref(),
        config.database_url.as_deref(),
    )?;

    // Create HTTP client for RPC
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    // Main scanning loop
    loop {
        match scan_batch(&client, &scanner, handler.as_ref(), &mut sync_state, config).await {
            Ok(deposits_found) => {
                if deposits_found > 0 {
                    tracing::info!(
                        "Found {} deposits, synced to height {}",
                        deposits_found,
                        sync_state.last_scanned_height
                    );
                }

                // Save state after each batch
                sync_state.save(&config.state_file)?;
            }
            Err(e) => {
                tracing::error!("Scan error: {}", e);
            }
        }

        if once {
            tracing::info!("Single scan complete, exiting");
            break;
        }

        // Wait before next poll
        tokio::time::sleep(std::time::Duration::from_secs(config.poll_interval_secs)).await;
    }

    Ok(())
}

async fn scan_batch(
    client: &reqwest::Client,
    scanner: &ExchangeScanner,
    handler: &dyn OutputHandler,
    sync_state: &mut SyncState,
    config: &ScannerConfig,
) -> Result<u64> {
    let start_height = sync_state.next_scan_height();

    // Get chain info
    let chain_info = get_chain_info(client, &config.rpc_endpoints[0]).await?;
    let chain_height = chain_info.height;

    if start_height > chain_height {
        tracing::debug!("Already at chain tip ({})", chain_height);
        return Ok(0);
    }

    // Calculate end height for this batch
    let end_height = std::cmp::min(start_height + config.batch_size - 1, chain_height);

    tracing::debug!(
        "Scanning blocks {} to {} (chain height: {})",
        start_height,
        end_height,
        chain_height
    );

    // Get outputs from RPC
    let outputs = get_outputs(client, &config.rpc_endpoints[0], start_height, end_height).await?;

    // Scan outputs
    let deposits = scanner.scan_outputs(&outputs, chain_height);

    // Filter by minimum confirmations
    let confirmed_deposits: Vec<_> = deposits
        .into_iter()
        .filter(|d| d.confirmations >= config.min_confirmations)
        .collect();

    let deposit_count = confirmed_deposits.len() as u64;
    let total_amount: u64 = confirmed_deposits.iter().map(|d| d.amount).sum();

    // Output deposits
    if !confirmed_deposits.is_empty() {
        handler.handle_batch(&confirmed_deposits).await?;
    }

    // Update sync state
    sync_state.update_after_batch(
        end_height,
        &chain_info.tip_hash,
        deposit_count,
        total_amount,
        outputs.len() as u64,
    );

    Ok(deposit_count)
}

#[derive(serde::Deserialize)]
struct ChainInfo {
    height: u64,
    #[serde(rename = "tipHash")]
    tip_hash: String,
}

async fn get_chain_info(client: &reqwest::Client, endpoint: &str) -> Result<ChainInfo> {
    let response: serde_json::Value = client
        .post(endpoint)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "getChainInfo",
            "params": {},
            "id": 1
        }))
        .send()
        .await?
        .json()
        .await?;

    let result = response
        .get("result")
        .ok_or_else(|| anyhow::anyhow!("No result in response"))?;

    Ok(serde_json::from_value(result.clone())?)
}

async fn get_outputs(
    client: &reqwest::Client,
    endpoint: &str,
    start_height: u64,
    end_height: u64,
) -> Result<Vec<RpcOutput>> {
    let response: serde_json::Value = client
        .post(endpoint)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "chain_getOutputs",
            "params": {
                "start_height": start_height,
                "end_height": end_height
            },
            "id": 1
        }))
        .send()
        .await?
        .json()
        .await?;

    let result = response
        .get("result")
        .ok_or_else(|| anyhow::anyhow!("No result in response"))?;

    let blocks = result
        .get("blocks")
        .and_then(|b| b.as_array())
        .ok_or_else(|| anyhow::anyhow!("No blocks in response"))?;

    let mut outputs = Vec::new();

    for block in blocks {
        let block_height = block.get("height").and_then(|h| h.as_u64()).unwrap_or(0);

        let block_outputs = block
            .get("outputs")
            .and_then(|o| o.as_array())
            .map(|arr| arr.as_slice())
            .unwrap_or(&[]);

        for output in block_outputs {
            let tx_hash = output
                .get("txHash")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let output_index = output
                .get("outputIndex")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;

            let target_key = output
                .get("targetKey")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let public_key = output
                .get("publicKey")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let amount = output
                .get("amountCommitment")
                .and_then(|v| v.as_str())
                .and_then(parse_amount_le)
                .unwrap_or(0);

            outputs.push(RpcOutput {
                tx_hash,
                output_index,
                target_key,
                public_key,
                amount,
                block_height,
            });
        }
    }

    Ok(outputs)
}

fn parse_amount_le(hex_str: &str) -> Option<u64> {
    let bytes = hex::decode(hex_str).ok()?;
    if bytes.len() >= 8 {
        Some(u64::from_le_bytes(bytes[..8].try_into().ok()?))
    } else {
        None
    }
}

fn derive_address(config: &ScannerConfig, index: u64) -> Result<()> {
    let derived =
        derive_subaddress_from_hex(&config.view_private_key, &config.spend_public_key, index)?;

    println!("Subaddress {}:", index);
    println!("  Address: {}", derived.address_string);
    println!("  View Public Key: {}", derived.view_public_key_hex);
    println!("  Spend Public Key: {}", derived.spend_public_key_hex);

    Ok(())
}

fn derive_address_batch(config: &ScannerConfig, start: u64, count: u64) -> Result<()> {
    println!(
        "Deriving {} subaddresses starting from index {}:",
        count, start
    );
    println!();

    for i in 0..count {
        let index = start + i;
        let derived =
            derive_subaddress_from_hex(&config.view_private_key, &config.spend_public_key, index)?;

        println!("{}: {}", index, derived.address_string);
    }

    Ok(())
}

fn show_status(config: &ScannerConfig) -> Result<()> {
    let state = SyncState::load(&config.state_file)?;
    println!("{}", state.summary());
    Ok(())
}
