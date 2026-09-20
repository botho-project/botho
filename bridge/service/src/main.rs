// Copyright (c) 2024 The Botho Foundation

//! BTH Bridge Service
//!
//! A centralized bridge service for transferring BTH to wrapped tokens
//! on Ethereum and Solana.

use clap::Parser;
use std::path::PathBuf;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

// The module tree lives in the library target (`src/lib.rs`, #897); the
// binary is a thin wire-up shim over the `#[doc(hidden)]` entry-point facade.
use bth_bridge_service::bin_support::{BridgeEngine, Database, SolMinter};

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

    /// Perform one bounded read-only Squads history reconciliation tick for an
    /// existing order. Repeat to resume its durable cursor; this never
    /// signs or sends transactions.
    #[arg(long, conflicts_with = "migrate")]
    reconcile_solana: Option<uuid::Uuid>,

    /// Inspect a persisted per-member Squads signing/fee ledger; no RPC or
    /// signing.
    #[arg(long, conflicts_with_all=["migrate","reconcile_solana"])]
    solana_budget: Option<uuid::Uuid>,
    #[arg(long, requires = "solana_budget")]
    budget_member: Option<String>,
    /// Expected revision for an explicit bounded extension (-1: legacy import).
    #[arg(long, requires_all=["solana_budget","budget_member","budget_add_attempts","budget_add_fees","budget_reason"], allow_hyphen_values=true)]
    budget_revision: Option<i64>,
    #[arg(long, requires = "budget_revision")]
    budget_add_attempts: Option<u32>,
    #[arg(long, requires = "budget_revision")]
    budget_add_fees: Option<u64>,
    #[arg(long, requires = "budget_revision")]
    budget_reason: Option<String>,

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
    let db = Database::open(&config.bridge.db_path)?;
    db.migrate()?;

    if args.migrate {
        info!("Database migration complete");
        return Ok(());
    }

    if let Some(id) = args.solana_budget {
        let member = args
            .budget_member
            .as_deref()
            .ok_or("--budget-member is required")?;
        if let Some(revision) = args.budget_revision {
            db.extend_solana_budget(
                &id,
                member,
                revision,
                args.budget_add_attempts.unwrap(),
                args.budget_add_fees.unwrap(),
                args.budget_reason.as_deref().unwrap(),
            )?;
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&db.solana_budget(&id, member)?)?
        );
        return Ok(());
    }
    if let Some(id) = args.reconcile_solana {
        let order = db.get_order(&id)?.ok_or("unknown bridge order")?;
        if order.status != bth_bridge_core::OrderStatus::MintPending {
            return Err("reconciliation requires a MintPending order".into());
        }
        let minter = SolMinter::new(config.solana.clone())?.with_store(db.clone());
        let result = minter.reconcile_squads_history(&order).await?;
        let diagnostic = db
            .solana_history(&id)?
            .and_then(|h| serde_json::from_str::<serde_json::Value>(&h.progress).ok())
            .and_then(|p| {
                p.get("diagnostic")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
            });
        info!(
            ?result,
            ?diagnostic,
            "Read-only Solana history reconciliation tick complete"
        );
        return Ok(());
    }

    info!("Bridge configuration:");
    info!("  BTH RPC: {}", config.bth.rpc_url);
    info!("  ETH RPC: {}", config.ethereum.rpc_url);
    info!("  SOL RPC: {}", config.solana.rpc_url);
    info!("  Fee: {} bps", config.bridge.fee_bps);
    info!("  Testnet: {}", config.bridge.testnet);

    // Start the bridge engine
    let engine = BridgeEngine::new(config.clone(), db);

    // Run the engine (this will spawn watchers and process orders)
    engine.run().await?;

    Ok(())
}
