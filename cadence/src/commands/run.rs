// Copyright (c) 2024 Cadence Foundation

use anyhow::{Context, Result};
use futures::StreamExt;
use mc_common::{NodeID, ResponderId};
use mc_consensus_scp::QuorumSet;
use mc_crypto_keys::Ed25519Public;
use std::path::Path;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

use crate::block::MiningTx;
use crate::config::Config;
use crate::consensus::{BlockBuilder, ConsensusConfig, ConsensusEvent, ConsensusService};
use crate::network::{NetworkDiscovery, NetworkEvent, QuorumBuilder, QuorumValidation};
use crate::node::Node;
use crate::transaction::Transaction;

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

    // Get chain state for consensus
    let chain_state = node.ledger().get_chain_state()
        .map_err(|e| anyhow::anyhow!("Failed to get chain state: {}", e))?;

    // Create consensus service
    let _local_peer_id = discovery.local_peer_id();
    let responder_id = ResponderId::from_str(&format!("localhost:{}", config.network.gossip_port))
        .unwrap_or_else(|_| ResponderId::from_str("localhost:8443").unwrap());
    let node_id = NodeID {
        responder_id,
        public_key: Ed25519Public::default(), // TODO: use real key
    };

    // Build SCP quorum set from connected peers
    let scp_quorum_set = build_scp_quorum_set(&quorum);

    let mut consensus = ConsensusService::new(
        node_id,
        scp_quorum_set,
        ConsensusConfig::default(),
        chain_state,
    );

    info!("Consensus service initialized at slot {}", consensus.current_slot());

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
    let mut consensus_tick = tokio::time::interval(Duration::from_millis(500));

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
                            } else {
                                // Update consensus chain state
                                if let Ok(state) = node.ledger().get_chain_state() {
                                    consensus.update_chain_state(state);
                                }
                            }
                        }
                        NetworkEvent::ScpMessage(msg) => {
                            // Handle SCP consensus message
                            debug!(slot = msg.slot_index, "Received SCP message from network");
                            if let Err(e) = consensus.handle_message(msg) {
                                warn!("Failed to handle SCP message: {}", e);
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

            // Consensus tick for timeouts and value proposal
            _ = consensus_tick.tick() => {
                consensus.tick();

                // Process any consensus events
                while let Some(event) = consensus.next_event() {
                    match event {
                        ConsensusEvent::SlotExternalized { slot_index, values } => {
                            info!(slot = slot_index, count = values.len(), "Slot externalized!");

                            // Build block from externalized values
                            match build_block_from_externalized(&values, &consensus) {
                                Ok(block) => {
                                    info!("Built block {} from consensus", block.height());

                                    // Add to ledger
                                    if let Err(e) = node.add_block_from_network(&block) {
                                        warn!("Failed to add consensus block: {}", e);
                                    } else {
                                        // Update consensus chain state
                                        if let Ok(state) = node.ledger().get_chain_state() {
                                            consensus.update_chain_state(state);
                                        }

                                        // Broadcast block to network
                                        if let Err(e) = NetworkDiscovery::broadcast_block(&mut swarm, &block) {
                                            warn!("Failed to broadcast block: {}", e);
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!("Failed to build block from consensus: {}", e);
                                }
                            }

                            // Advance to next slot
                            consensus.advance_slot();
                        }
                        ConsensusEvent::BroadcastMessage(msg) => {
                            // Broadcast SCP message to network
                            if let Err(e) = NetworkDiscovery::broadcast_scp(&mut swarm, &msg) {
                                warn!("Failed to broadcast SCP message: {}", e);
                            }
                        }
                        ConsensusEvent::Progress { slot_index, phase } => {
                            debug!(slot = slot_index, phase = %phase, "Consensus progress");
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

            // Check for mined mining transactions
            _ = tokio::time::sleep(Duration::from_millis(100)) => {
                // Check for mined transactions and submit to consensus
                if let Some(mined_tx) = node.check_mined_mining_tx()? {
                    let mining_tx = &mined_tx.mining_tx;
                    info!(
                        height = mining_tx.block_height,
                        priority = mined_tx.pow_priority,
                        "Submitting mining tx to consensus"
                    );

                    // Serialize and submit to consensus
                    let tx_bytes = bincode::serialize(mining_tx)
                        .expect("Failed to serialize mining tx");
                    let tx_hash = mining_tx.hash();

                    consensus.submit_mining_tx(tx_hash, mined_tx.pow_priority, tx_bytes);
                }

                // Also check for pending transfer transactions in mempool
                // and submit them to consensus
                for tx in node.get_pending_transactions(10) {
                    let tx_hash = tx.hash();
                    let tx_bytes = bincode::serialize(&tx)
                        .expect("Failed to serialize transaction");

                    // Priority based on fee (higher fee = higher priority)
                    consensus.submit_transaction(tx_hash, tx_bytes);
                }
            }
        }
    }

    node.stop_mining_public();
    Ok(())
}

/// Build SCP quorum set from QuorumBuilder
fn build_scp_quorum_set(quorum: &QuorumBuilder) -> QuorumSet {
    use mc_consensus_scp_types::QuorumSetMember;

    // Create NodeIDs for each peer (simplified for now)
    let members: Vec<QuorumSetMember<NodeID>> = (0..quorum.member_count())
        .map(|i| {
            let responder_id = ResponderId::from_str(&format!("peer{}:8443", i + 1))
                .unwrap_or_else(|_| ResponderId::from_str("peer:8443").unwrap());
            let node_id = NodeID {
                responder_id,
                public_key: Ed25519Public::default(), // TODO: use real key from peers
            };
            QuorumSetMember::Node(node_id)
        })
        .collect();

    QuorumSet::new(quorum.threshold() as u32, members)
}

/// Build a block from externalized consensus values
fn build_block_from_externalized(
    values: &[crate::consensus::ConsensusValue],
    consensus: &ConsensusService,
) -> Result<crate::block::Block> {
    BlockBuilder::build_from_externalized(
        values,
        |hash| {
            // Get mining tx from consensus cache
            consensus.get_tx_data(hash).and_then(|bytes| {
                bincode::deserialize::<MiningTx>(&bytes).ok()
            })
        },
        |hash| {
            // Get transfer tx from consensus cache
            consensus.get_tx_data(hash).and_then(|bytes| {
                bincode::deserialize::<Transaction>(&bytes).ok()
            })
        },
    )
    .map(|built| built.block)
    .map_err(|e| anyhow::anyhow!("Block build error: {}", e))
}
