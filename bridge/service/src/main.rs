// Copyright (c) 2024 The Botho Foundation

//! BTH Bridge Service
//!
//! A centralized bridge service for transferring BTH to wrapped tokens
//! on Ethereum and Solana.

use clap::Parser;
use std::path::PathBuf;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

mod db;
mod engine;
mod watchers;

use bth_bridge_core::BridgeConfig;

/// BTH Bridge Service - Bridge BTH to Ethereum and Solana
#[derive(Parser, Debug)]
#[command(name = "bth-bridge")]
#[command(about = "Bridge service for BTH <-> wBTH transfers")]
struct Args {
    /// Path to configuration file
    #[arg(short, long, default_value = "bridge.toml")]
    config: PathBuf,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,

    /// Run database migrations only
    #[arg(long)]
    migrate: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Setup logging
    let log_level = if args.verbose {
        Level::DEBUG
    } else {
        Level::INFO
    };

    let subscriber = FmtSubscriber::builder()
        .with_max_level(log_level)
        .with_target(true)
        .with_thread_ids(true)
        .finish();

    tracing::subscriber::set_global_default(subscriber)?;

    info!("BTH Bridge Service starting...");

    // Load configuration
    let config_path = args.config.to_string_lossy();
    let config = if args.config.exists() {
        info!("Loading configuration from {}", config_path);
        BridgeConfig::from_file(&config_path)?
    } else {
        info!("Using default configuration");
        BridgeConfig::default()
    };

    // Initialize database
    info!("Initializing database at {}", config.bridge.db_path);
    let db = db::Database::open(&config.bridge.db_path)?;
    db.migrate()?;

    if args.migrate {
        info!("Database migration complete");
        return Ok(());
    }

    info!("Bridge configuration:");
    info!("  BTH RPC: {}", config.bth.rpc_url);
    info!("  ETH RPC: {}", config.ethereum.rpc_url);
    info!("  SOL RPC: {}", config.solana.rpc_url);
    info!("  Fee: {} bps", config.bridge.fee_bps);
    info!("  Testnet: {}", config.bridge.testnet);

    // Start the bridge engine
    let engine = engine::BridgeEngine::new(config.clone(), db);

    // Run the engine (this will spawn watchers and process orders)
    engine.run().await?;

    Ok(())
}
