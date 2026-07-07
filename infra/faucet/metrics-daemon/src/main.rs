//! Botho Metrics Daemon
//!
//! Collects historical metrics from a fleet of Botho nodes for the
//! network stats dashboard.
//!
//! Features:
//! - Polls node_getStatus on every configured node every 5 minutes
//! - Stores per-node metrics in SQLite with automatic rollup
//! - Serves fleet latest + per-node history via HTTP API
//!
//! Usage (fleet):
//!   botho-metrics-daemon \
//!       --node seed=https://seed.botho.io/rpc \
//!       --node faucet=http://127.0.0.1:17101 \
//!       --db /var/lib/botho-metrics/metrics.db
//!
//! Usage (legacy single node, stored under node name "default"):
//!   botho-metrics-daemon --node-url http://127.0.0.1:17101 --db metrics.db

mod api;
mod collector;
mod db;
mod rollup;

use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::Result;
use clap::Parser;
use tracing::{error, info, Level};
use tracing_subscriber::FmtSubscriber;

use collector::NodeConfig;
use db::MetricsDb;

/// Botho Metrics Daemon - Fleet metrics collection for the network dashboard
#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Args {
    /// Node to poll, as NAME=URL (repeat for multiple nodes),
    /// e.g. --node seed=https://seed.botho.io/rpc --node faucet=http://127.0.0.1:17101
    #[arg(long = "node", value_name = "NAME=URL", value_parser = parse_node)]
    nodes: Vec<NodeConfig>,

    /// Single-node fallback: URL of a Botho node RPC endpoint, stored under
    /// node name "default". Ignored when --node is given.
    #[arg(long, default_value = "http://127.0.0.1:17101")]
    node_url: String,

    /// Path to the SQLite database file
    #[arg(long, default_value = "/var/lib/botho-metrics/metrics.db")]
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

/// Parse a NAME=URL node argument
fn parse_node(s: &str) -> Result<NodeConfig, String> {
    let (name, url) = s
        .split_once('=')
        .ok_or_else(|| format!("expected NAME=URL, got '{s}'"))?;
    if name.is_empty() || url.is_empty() {
        return Err(format!("expected NAME=URL with non-empty parts, got '{s}'"));
    }
    Ok(NodeConfig {
        name: name.to_string(),
        url: url.to_string(),
    })
}

/// Resolve the node list: --node entries win; otherwise fall back to the
/// legacy --node-url as a single node named "default".
fn resolve_nodes(args: &Args) -> Result<Vec<NodeConfig>> {
    let nodes = if args.nodes.is_empty() {
        vec![NodeConfig {
            name: "default".to_string(),
            url: args.node_url.clone(),
        }]
    } else {
        args.nodes.clone()
    };

    let mut seen = HashSet::new();
    for node in &nodes {
        if !seen.insert(node.name.as_str()) {
            anyhow::bail!("duplicate node name: '{}'", node.name);
        }
    }

    Ok(nodes)
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    FmtSubscriber::builder().with_max_level(Level::INFO).init();

    let args = Args::parse();
    let nodes = resolve_nodes(&args)?;

    info!("Botho Metrics Daemon starting");
    for node in &nodes {
        info!("Node '{}': {}", node.name, node.url);
    }
    info!("Database: {:?}", args.db);

    // Ensure parent directory exists
    if let Some(parent) = args.db.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Initialize database
    let db = Arc::new(Mutex::new(MetricsDb::open(&args.db)?));
    info!("Database initialized");

    // Shared HTTP client for all polls
    let client = reqwest::Client::new();

    // If --once flag is set, collect once and exit
    if args.once {
        info!("Running single collection (--once mode)");
        let collected = collector::collect_metrics(&client, &nodes, &db).await?;
        info!("Collected {}/{} nodes", collected, nodes.len());

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
    info!(
        "Starting collection loop (interval: {:?}, {} nodes)",
        collection_interval,
        nodes.len()
    );

    let mut interval_timer = tokio::time::interval(collection_interval);

    // Run rollup every hour
    let mut rollup_counter = 0u32;
    let rollups_per_hour = (3600 / args.interval.max(1)) as u32;

    loop {
        interval_timer.tick().await;

        // Collect metrics from all nodes (per-node failures are logged and
        // skipped inside collect_metrics)
        match collector::collect_metrics(&client, &nodes, &db).await {
            Ok(collected) => info!("Collected {}/{} nodes", collected, nodes.len()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_node() {
        let node = parse_node("seed=https://seed.botho.io/rpc").unwrap();
        assert_eq!(node.name, "seed");
        assert_eq!(node.url, "https://seed.botho.io/rpc");

        // URLs may contain '=' after the first separator
        let node = parse_node("x=http://h/p?a=b").unwrap();
        assert_eq!(node.url, "http://h/p?a=b");

        assert!(parse_node("no-separator").is_err());
        assert!(parse_node("=http://h").is_err());
        assert!(parse_node("name=").is_err());
    }

    #[test]
    fn test_resolve_nodes_fallback_and_duplicates() {
        // No --node args -> legacy --node-url becomes node "default"
        let args = Args::parse_from(["d", "--node-url", "http://127.0.0.1:17101"]);
        let nodes = resolve_nodes(&args).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "default");
        assert_eq!(nodes[0].url, "http://127.0.0.1:17101");

        // --node args win over --node-url
        let args = Args::parse_from(["d", "--node", "a=http://a", "--node", "b=http://b"]);
        let nodes = resolve_nodes(&args).unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].name, "a");
        assert_eq!(nodes[1].name, "b");

        // Duplicate names rejected
        let args = Args::parse_from(["d", "--node", "a=http://a", "--node", "a=http://b"]);
        assert!(resolve_nodes(&args).is_err());
    }
}
