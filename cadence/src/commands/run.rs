// Copyright (c) 2024 Cadence Foundation

use anyhow::{Context, Result};
use futures::StreamExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

use crate::config::Config;
use crate::network::{NetworkDiscovery, NetworkEvent, QuorumBuilder, QuorumValidation};
use crate::node::Node;

/// Minimum peers required before mining can start
const MIN_PEERS_FOR_MINING: usize = 1;

/// Timeout for initial peer discovery (seconds)
const DISCOVERY_TIMEOUT_SECS: u64 = 30;

/// Run the node
pub fn run(config_path: &Path, mine: bool) -> Result<()> {
    let config = Config::load(config_path).context("No wallet found. Run 'cadence init' first.")?;

    println!("Cadence node starting. Press Ctrl+C to stop.");

    // Create tokio runtime for async networking
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async { run_async(config, config_path, mine).await })
}

async fn run_async(config: Config, config_path: &Path, mine: bool) -> Result<()> {
    // Set up shutdown signal
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = shutdown.clone();
    ctrlc::set_handler(move || {
        shutdown_clone.store(true, Ordering::SeqCst);
    })?;

    // Start network discovery
    let mut discovery = NetworkDiscovery::new(
        config.network.gossip_port,
        config.network.bootstrap_peers.clone(),
    );

    let mut swarm = discovery.start().await?;

    // Wait for peers with timeout
    info!(
        "Waiting for peers (timeout: {}s)...",
        DISCOVERY_TIMEOUT_SECS
    );

    let start = std::time::Instant::now();
    let deadline = Duration::from_secs(DISCOVERY_TIMEOUT_SECS);

    while start.elapsed() < deadline && discovery.peer_count() < MIN_PEERS_FOR_MINING {
        if shutdown.load(Ordering::SeqCst) {
            return Ok(());
        }

        tokio::select! {
            event = swarm.select_next_some() => {
                if let Some(net_event) = discovery.process_event(event) {
                    if let NetworkEvent::PeerDiscovered(peer_id) = net_event {
                        info!("Connected to peer: {}", peer_id);
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }

    let has_peers = discovery.peer_count() >= MIN_PEERS_FOR_MINING;

    // Show peer status
    println!();
    if has_peers {
        println!("=== Connected Peers: {} ===", discovery.peer_count());
        for peer in discovery.peer_table() {
            println!("  - {}", peer.peer_id);
        }
    } else {
        println!("=== No peers connected ===");
        if config.network.bootstrap_peers.is_empty() {
            warn!("No bootstrap peers configured. Add bootstrap_peers to config.toml");
        }
    }
    println!();

    // Check quorum status
    let mut quorum = QuorumBuilder::new(config.network.quorum.threshold);
    for peer in discovery.peer_table() {
        quorum.add_member(peer.peer_id);
    }

    let validation = QuorumValidation::new(&quorum);
    println!("Quorum: {}", validation.message);

    // Create the node
    let mut node = Node::new(config.clone(), config_path)?;

    // Determine if we can mine
    let can_mine = if !mine {
        false
    } else if !has_peers {
        warn!("Mining disabled: no peers connected");
        false
    } else if !validation.is_valid {
        warn!("Mining disabled: quorum not satisfied");
        false
    } else {
        true
    };

    println!();
    node.print_status_public()?;

    if can_mine {
        info!(
            "Starting mining with {}-of-{} quorum",
            quorum.threshold(),
            quorum.member_count()
        );
        node.start_mining_public()?;
    }

    // Run the combined event loop
    let mut status_interval = tokio::time::interval(Duration::from_secs(10));

    loop {
        if shutdown.load(Ordering::SeqCst) {
            info!("Shutting down...");
            break;
        }

        tokio::select! {
            // Network events
            event = swarm.select_next_some() => {
                if let Some(net_event) = discovery.process_event(event) {
                    match net_event {
                        NetworkEvent::NewBlock(block) => {
                            info!("Received block {} from network", block.height());
                            if let Err(e) = node.add_block_from_network(&block) {
                                warn!("Failed to add network block: {}", e);
                            }
                        }
                        NetworkEvent::PeerDiscovered(peer_id) => {
                            info!("Peer connected: {}", peer_id);
                        }
                        NetworkEvent::PeerDisconnected(peer_id) => {
                            warn!("Peer disconnected: {}", peer_id);
                        }
                    }
                }
            }

            // Periodic status
            _ = status_interval.tick() => {
                if can_mine {
                    // Just show mining stats inline
                }
            }

            // Check for mined blocks
            _ = tokio::time::sleep(Duration::from_millis(100)) => {
                if let Some(block) = node.check_mined_block()? {
                    info!("Mined block {}! Broadcasting to network...", block.height());

                    // Broadcast to network
                    if let Err(e) = NetworkDiscovery::broadcast_block(&mut swarm, &block) {
                        warn!("Failed to broadcast block: {}", e);
                    } else {
                        info!("Block broadcast successfully");
                    }
                }
            }
        }
    }

    node.stop_mining_public();
    Ok(())
}
