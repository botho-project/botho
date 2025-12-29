// Copyright (c) 2024 Botho Foundation

//! CLI tool for discovering Botho network topology and generating configurations.
//!
//! This tool connects to the gossip network, discovers peers, and helps users
//! configure their nodes by suggesting quorum sets based on observed trust patterns.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use libp2p::Multiaddr;
use bt_common::ResponderId;
use bt_consensus_scp_types::QuorumSet;
use bt_gossip::{
    GossipConfig, GossipConfigBuilder, GossipEvent, GossipService, NodeCapabilities,
    QuorumStrategy, TopologyAnalyzer,
};
use bt_util_from_random::FromRandom;
use std::{
    path::PathBuf,
    str::FromStr,
    time::Duration,
};
use tokio::time::timeout;
use tracing::{info, warn};

#[derive(Parser)]
#[command(name = "mc-discover")]
#[command(about = "Discover Botho network topology and generate configurations")]
#[command(version)]
struct Cli {
    /// Bootstrap peers to connect to (libp2p multiaddr format)
    #[arg(short, long, env = "MC_BOOTSTRAP_PEERS")]
    bootstrap: Vec<Multiaddr>,

    /// Port to listen on for gossip connections
    #[arg(short, long, default_value = "7100", env = "MC_GOSSIP_PORT")]
    port: u16,

    /// Timeout for discovery in seconds
    #[arg(short, long, default_value = "30")]
    timeout: u64,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Discover peers on the network
    Peers {
        /// Show detailed information for each peer
        #[arg(short, long)]
        detailed: bool,

        /// Filter by capability (consensus, relay, archive)
        #[arg(short, long)]
        capability: Option<String>,
    },

    /// Show network topology statistics
    Stats,

    /// Suggest quorum set configurations
    Suggest {
        /// Strategy to use: top-n, hierarchical, conservative, aggressive
        #[arg(short, long, default_value = "top-n")]
        strategy: String,

        /// Number of nodes for top-n strategy
        #[arg(short, long, default_value = "5")]
        count: usize,

        /// Threshold percentage
        #[arg(short = 'T', long, default_value = "67")]
        threshold: u32,
    },

    /// Generate a network configuration file
    Generate {
        /// Output file path
        #[arg(short, long, default_value = "network.toml")]
        output: PathBuf,

        /// Strategy for quorum set: top-n, hierarchical, conservative, aggressive
        #[arg(short, long, default_value = "top-n")]
        strategy: String,

        /// Format: toml or json
        #[arg(short, long, default_value = "toml")]
        format: String,
    },

    /// Validate a quorum set configuration
    Validate {
        /// Path to network config file
        #[arg(short, long)]
        config: PathBuf,
    },

    /// Show trust relationships in the network
    Trust {
        /// Show who trusts a specific node
        #[arg(short, long)]
        node: Option<String>,

        /// Show trust clusters
        #[arg(short, long)]
        clusters: bool,
    },

    /// Run in interactive mode
    Interactive,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging
    let filter = if cli.verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    // Build gossip config
    let config = GossipConfigBuilder::new()
        .listen_port(cli.port)
        .bootstrap_peers(cli.bootstrap.clone())
        .announce_interval_secs(300)
        .sync_interval_secs(10) // Faster sync for discovery
        .build();

    match cli.command {
        Commands::Peers { detailed, capability } => {
            run_peers_command(config, cli.timeout, detailed, capability).await
        }
        Commands::Stats => run_stats_command(config, cli.timeout).await,
        Commands::Suggest {
            strategy,
            count,
            threshold,
        } => run_suggest_command(config, cli.timeout, &strategy, count, threshold).await,
        Commands::Generate {
            output,
            strategy,
            format,
        } => run_generate_command(config, cli.timeout, output, &strategy, &format).await,
        Commands::Validate { config: cfg_path } => run_validate_command(config, cli.timeout, cfg_path).await,
        Commands::Trust { node, clusters } => run_trust_command(config, cli.timeout, node, clusters).await,
        Commands::Interactive => run_interactive(config).await,
    }
}

/// Connect to the network and wait for discovery.
async fn discover_network(
    config: GossipConfig,
    timeout_secs: u64,
) -> Result<bt_gossip::SharedPeerStore> {
    // Create a minimal node identity for discovery
    let node_id = bt_common::NodeID {
        responder_id: ResponderId::from_str("discover-client:0").unwrap(),
        public_key: bt_crypto_keys::Ed25519Public::default(),
    };

    let signing_key = std::sync::Arc::new(bt_crypto_keys::Ed25519Pair::from_random(&mut rand::thread_rng()));

    let mut service = GossipService::new(
        node_id,
        signing_key,
        QuorumSet::empty(),
        vec![],
        NodeCapabilities::GOSSIP,
        env!("CARGO_PKG_VERSION").to_string(),
        config,
    );

    let store = service.shared_store();

    service.start().await?;

    info!("Connecting to network...");

    // Wait for bootstrapping or timeout
    let discovery_timeout = Duration::from_secs(timeout_secs);
    let start = std::time::Instant::now();

    while start.elapsed() < discovery_timeout {
        if let Ok(Some(event)) =
            timeout(Duration::from_millis(500), service.next_event()).await
        {
            match event {
                GossipEvent::Bootstrapped => {
                    info!("Connected to network");
                }
                GossipEvent::PeerDiscovered(peer) => {
                    info!(?peer, "Discovered peer");
                }
                GossipEvent::AnnouncementReceived(ann) => {
                    info!(
                        responder_id = %ann.node_id.responder_id,
                        "Received announcement"
                    );
                }
                _ => {}
            }
        }

        // Check if we have enough peers
        if store.len() >= 3 {
            // Give a bit more time to discover additional peers
            tokio::time::sleep(Duration::from_secs(2)).await;
            break;
        }
    }

    let _ = service.shutdown().await;

    Ok(store)
}

async fn run_peers_command(
    config: GossipConfig,
    timeout_secs: u64,
    detailed: bool,
    capability: Option<String>,
) -> Result<()> {
    let store = discover_network(config, timeout_secs).await?;

    let announcements = if let Some(cap) = capability {
        let cap_flag = match cap.to_lowercase().as_str() {
            "consensus" => NodeCapabilities::CONSENSUS,
            "relay" => NodeCapabilities::RELAY,
            "archive" => NodeCapabilities::ARCHIVE,
            _ => {
                warn!("Unknown capability: {}", cap);
                NodeCapabilities::empty()
            }
        };
        store.get_with_capabilities(cap_flag)
    } else {
        store.get_all()
    };

    println!("\nDiscovered {} peers:\n", announcements.len());

    for ann in &announcements {
        if detailed {
            println!("Node: {}", ann.node_id.responder_id);
            let pk_bytes: &[u8] = ann.node_id.public_key.as_ref();
            println!("  Public Key: {}", hex::encode(pk_bytes));
            println!("  Endpoints: {:?}", ann.endpoints);
            println!("  Capabilities: {:?}", ann.capabilities);
            println!("  Version: {}", ann.version);
            println!(
                "  Quorum Set: {} members, threshold {}",
                ann.quorum_set.members.len(),
                ann.quorum_set.threshold
            );
            println!();
        } else {
            let caps: Vec<&str> = [
                ann.capabilities
                    .contains(NodeCapabilities::CONSENSUS)
                    .then_some("consensus"),
                ann.capabilities
                    .contains(NodeCapabilities::RELAY)
                    .then_some("relay"),
                ann.capabilities
                    .contains(NodeCapabilities::ARCHIVE)
                    .then_some("archive"),
            ]
            .into_iter()
            .flatten()
            .collect();

            println!(
                "  {} [{}]",
                ann.node_id.responder_id,
                caps.join(", ")
            );
        }
    }

    Ok(())
}

async fn run_stats_command(config: GossipConfig, timeout_secs: u64) -> Result<()> {
    let store = discover_network(config, timeout_secs).await?;
    let analyzer = TopologyAnalyzer::new(store);
    let stats = analyzer.stats();

    println!("\nNetwork Topology Statistics:");
    println!("============================");
    println!("Total nodes:           {}", stats.total_nodes);
    println!("Consensus nodes:       {}", stats.consensus_nodes);
    println!("Avg quorum set size:   {:.1}", stats.avg_quorum_set_size);
    println!("Avg threshold:         {:.1}%", stats.avg_threshold_pct);
    println!("Trust clusters:        {}", stats.cluster_count);

    if let Some(node) = stats.most_trusted_node {
        println!(
            "Most trusted node:     {} (trusted by {} nodes)",
            node, stats.max_trust_count
        );
    }

    Ok(())
}

async fn run_suggest_command(
    config: GossipConfig,
    timeout_secs: u64,
    strategy: &str,
    count: usize,
    threshold: u32,
) -> Result<()> {
    let store = discover_network(config, timeout_secs).await?;
    let analyzer = TopologyAnalyzer::new(store);

    let suggestion = match strategy.to_lowercase().as_str() {
        "top-n" => analyzer.suggest_top_n(count, threshold),
        "hierarchical" => analyzer.suggest_hierarchical(),
        "conservative" => analyzer.suggest_quorum_set(QuorumStrategy::Conservative),
        "aggressive" => analyzer.suggest_quorum_set(QuorumStrategy::Aggressive),
        _ => {
            anyhow::bail!("Unknown strategy: {}", strategy);
        }
    };

    if let Some(suggestion) = suggestion {
        println!("\nSuggested Quorum Set:");
        println!("=====================");
        println!("Strategy:   {:?}", suggestion.strategy);
        println!("Confidence: {:.0}%", suggestion.confidence * 100.0);
        println!("Rationale:  {}", suggestion.rationale);
        println!();
        println!("Threshold: {}", suggestion.quorum_set.threshold);
        println!("Members ({}):", suggestion.quorum_set.members.len());

        print_quorum_set(&suggestion.quorum_set, 1);
    } else {
        println!("Could not generate a suggestion. Not enough peers discovered.");
    }

    Ok(())
}

fn print_quorum_set(qs: &QuorumSet<ResponderId>, indent: usize) {
    let prefix = "  ".repeat(indent);
    for member in &qs.members {
        if let Some(m) = member.as_ref() {
            match m {
                bt_consensus_scp_types::QuorumSetMember::Node(id) => {
                    println!("{}- {}", prefix, id);
                }
                bt_consensus_scp_types::QuorumSetMember::InnerSet(inner) => {
                    println!(
                        "{}- InnerSet (threshold {}, {} members):",
                        prefix,
                        inner.threshold,
                        inner.members.len()
                    );
                    print_quorum_set(inner, indent + 1);
                }
            }
        }
    }
}

async fn run_generate_command(
    config: GossipConfig,
    timeout_secs: u64,
    output: PathBuf,
    strategy: &str,
    format: &str,
) -> Result<()> {
    let store = discover_network(config, timeout_secs).await?;
    let analyzer = TopologyAnalyzer::new(store.clone());

    let suggestion = match strategy.to_lowercase().as_str() {
        "top-n" => analyzer.suggest_top_n(5, 67),
        "hierarchical" => analyzer.suggest_hierarchical(),
        "conservative" => analyzer.suggest_quorum_set(QuorumStrategy::Conservative),
        "aggressive" => analyzer.suggest_quorum_set(QuorumStrategy::Aggressive),
        _ => anyhow::bail!("Unknown strategy: {}", strategy),
    }
    .context("Could not generate quorum set suggestion")?;

    // Build broadcast_peers from known nodes
    let announcements = store.get_consensus_nodes();
    let broadcast_peers: Vec<String> = announcements
        .iter()
        .filter_map(|a| a.endpoints.first().cloned())
        .take(10)
        .collect();

    // Build tx_source_urls from archive nodes
    let tx_source_urls: Vec<String> = store
        .get_with_capabilities(NodeCapabilities::ARCHIVE)
        .iter()
        .flat_map(|a| a.tx_source_urls.clone())
        .take(5)
        .collect();

    // Create the network config structure
    let network_config = NetworkConfigOutput {
        quorum_set: suggestion.quorum_set,
        broadcast_peers,
        tx_source_urls,
        known_peers: None,
    };

    let content = match format.to_lowercase().as_str() {
        "toml" => toml::to_string_pretty(&network_config)?,
        "json" => serde_json::to_string_pretty(&network_config)?,
        _ => anyhow::bail!("Unknown format: {}", format),
    };

    std::fs::write(&output, content)?;
    println!("Generated config written to: {}", output.display());

    Ok(())
}

#[derive(serde::Serialize, serde::Deserialize)]
struct NetworkConfigOutput {
    quorum_set: QuorumSet<ResponderId>,
    broadcast_peers: Vec<String>,
    tx_source_urls: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    known_peers: Option<Vec<String>>,
}

async fn run_validate_command(
    config: GossipConfig,
    timeout_secs: u64,
    cfg_path: PathBuf,
) -> Result<()> {
    let store = discover_network(config, timeout_secs).await?;
    let analyzer = TopologyAnalyzer::new(store);

    // Read and parse the config file
    let content = std::fs::read_to_string(&cfg_path)?;
    let network_config: NetworkConfigOutput = if cfg_path.extension().map(|e| e == "json").unwrap_or(false) {
        serde_json::from_str(&content)?
    } else {
        toml::from_str(&content)?
    };

    let validation = analyzer.validate_quorum_set(&network_config.quorum_set);

    println!("\nQuorum Set Validation:");
    println!("======================");
    println!(
        "Valid: {}",
        if validation.is_valid { "Yes" } else { "No" }
    );
    println!("Threshold: {:.1}%", validation.threshold_pct);

    if !validation.warnings.is_empty() {
        println!("\nWarnings:");
        for warning in &validation.warnings {
            println!("  - {}", warning);
        }
    }

    if !validation.unknown_nodes.is_empty() {
        println!("\nUnknown nodes:");
        for node in &validation.unknown_nodes {
            println!("  - {}", node);
        }
    }

    if !validation.low_trust_nodes.is_empty() {
        println!("\nLow-trust nodes:");
        for node in &validation.low_trust_nodes {
            println!("  - {}", node);
        }
    }

    Ok(())
}

async fn run_trust_command(
    config: GossipConfig,
    timeout_secs: u64,
    node: Option<String>,
    show_clusters: bool,
) -> Result<()> {
    let store = discover_network(config, timeout_secs).await?;
    let analyzer = TopologyAnalyzer::new(store.clone());

    if let Some(node_str) = node {
        let responder_id = ResponderId::from_str(&node_str)
            .context("Invalid node format. Use host:port")?;

        let trusters = store.get_trusters(&responder_id);
        println!("\nNodes that trust {}:", node_str);
        println!("========================");

        if trusters.is_empty() {
            println!("  (none)");
        } else {
            for truster in &trusters {
                println!("  - {}", truster);
            }
        }
        println!("\nTotal: {} nodes", trusters.len());
    }

    if show_clusters {
        let clusters = analyzer.find_trust_clusters();
        println!("\nTrust Clusters:");
        println!("===============");

        if clusters.is_empty() {
            println!("  (no clusters found)");
        } else {
            for cluster in &clusters {
                println!(
                    "\n{} ({} members, {:.0}% cohesion):",
                    cluster.name,
                    cluster.members.len(),
                    cluster.cohesion * 100.0
                );
                for member in &cluster.members {
                    println!("  - {}", member);
                }
            }
        }
    }

    Ok(())
}

async fn run_interactive(config: GossipConfig) -> Result<()> {
    println!("Botho Network Discovery - Interactive Mode");
    println!("=============================================");
    println!();

    let store = discover_network(config, 30).await?;
    let analyzer = TopologyAnalyzer::new(store.clone());

    let stats = analyzer.stats();
    println!("Discovered {} peers on the network.\n", stats.total_nodes);

    if stats.total_nodes == 0 {
        println!("No peers discovered. Check your bootstrap peer configuration.");
        return Ok(());
    }

    println!("Trust Analysis:");
    println!("  - {} consensus nodes", stats.consensus_nodes);
    println!("  - {} trust clusters identified", stats.cluster_count);
    println!(
        "  - Average quorum set size: {:.1} nodes",
        stats.avg_quorum_set_size
    );

    // Show suggestions
    println!("\nQuorum Set Suggestions:");
    println!("-----------------------");

    let suggestions = analyzer.get_all_suggestions();
    for (i, suggestion) in suggestions.iter().enumerate() {
        println!(
            "\n{}. {:?} (confidence: {:.0}%)",
            i + 1,
            suggestion.strategy,
            suggestion.confidence * 100.0
        );
        println!("   {}", suggestion.rationale);
        println!(
            "   Threshold: {}/{}",
            suggestion.quorum_set.threshold,
            suggestion.quorum_set.members.len()
        );
    }

    println!("\nTo generate a config file, run:");
    println!("  mc-discover generate --strategy top-n --output network.toml");

    Ok(())
}
