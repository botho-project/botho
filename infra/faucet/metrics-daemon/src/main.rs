//! Botho Metrics Daemon
//!
//! Collects historical node metrics for the faucet web UI.
//!
//! Features:
//! - Polls node_getStatus every 5 minutes
//! - Stores metrics in SQLite with automatic rollup
//! - Serves historical data via HTTP API
//!
//! Usage:
//!   botho-metrics-daemon --node-url http://127.0.0.1:17101 --db /var/lib/botho-faucet/metrics.db

mod db;
mod collector;
mod api;
mod rollup;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use tracing::{info, error, Level};
use tracing_subscriber::FmtSubscriber;

use db::MetricsDb;

/// Botho Metrics Daemon - Historical metrics collection for faucet
#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Args {
    /// URL of the Botho node RPC endpoint
    #[arg(long, default_value = "http://127.0.0.1:17101")]
    node_url: String,

    /// Path to the SQLite database file
    #[arg(long, default_value = "/var/lib/botho-faucet/metrics.db")]
    db: PathBuf,

    /// Port for the metrics API server
    #[arg(long, default_value = "17102")]
    api_port: u16,

    /// Collection interval in seconds (default: 300 = 5 minutes)
    #[arg(long, default_value = "300")]
    interval: u64,

    /// Run once and exit (for cron-based operation)
    #[arg(long)]
    once: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .init();

    let args = Args::parse();

    info!("Botho Metrics Daemon starting");
    info!("Node URL: {}", args.node_url);
    info!("Database: {:?}", args.db);

    // Ensure parent directory exists
    if let Some(parent) = args.db.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Initialize database
    let db = Arc::new(Mutex::new(MetricsDb::open(&args.db)?));
    info!("Database initialized");

    // If --once flag is set, collect once and exit
    if args.once {
        info!("Running single collection (--once mode)");
        if let Err(e) = collector::collect_metrics(&args.node_url, &db).await {
            error!("Failed to collect metrics: {}", e);
            return Err(e);
        }

        // Run rollup
        {
            let mut db_lock = db.lock().unwrap();
            rollup::run_rollup(&mut db_lock)?;
        }

        info!("Collection complete, exiting");
        return Ok(());
    }

    // Start API server
    let db_clone = db.clone();
    let api_addr = format!("0.0.0.0:{}", args.api_port);
    info!("Starting API server on {}", api_addr);

    tokio::spawn(async move {
        if let Err(e) = api::serve(api_addr, db_clone).await {
            error!("API server error: {}", e);
        }
    });

    // Start collection loop
    let collection_interval = Duration::from_secs(args.interval);
    info!("Starting collection loop (interval: {:?})", collection_interval);

    let mut interval_timer = tokio::time::interval(collection_interval);

    // Run rollup every hour
    let mut rollup_counter = 0u32;
    let rollups_per_hour = 3600 / args.interval as u32;

    loop {
        interval_timer.tick().await;

        // Collect metrics
        match collector::collect_metrics(&args.node_url, &db).await {
            Ok(()) => info!("Metrics collected successfully"),
            Err(e) => error!("Failed to collect metrics: {}", e),
        }

        // Run rollup every hour
        rollup_counter += 1;
        if rollup_counter >= rollups_per_hour {
            rollup_counter = 0;
            let mut db_lock = db.lock().unwrap();
            match rollup::run_rollup(&mut db_lock) {
                Ok(()) => info!("Rollup completed successfully"),
                Err(e) => error!("Rollup failed: {}", e),
            }
        }
    }
}
