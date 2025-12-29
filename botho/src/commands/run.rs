// Copyright (c) 2024 Botho Foundation

use anyhow::{Context, Result};
use futures::StreamExt;
use bth_common::{NodeID, ResponderId};
use bth_consensus_scp::QuorumSet;
use bth_crypto_keys::Ed25519Public;
use std::net::SocketAddr;
use std::path::Path;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tracing::{debug, error, info, warn};

use crate::block::MiningTx;
use crate::config::{Config, QuorumMode};
use crate::consensus::{BlockBuilder, ConsensusConfig, ConsensusEvent, ConsensusService};
use crate::network::{NetworkDiscovery, NetworkEvent, QuorumBuilder};
use crate::node::Node;
use crate::rpc::{start_rpc_server, RpcState};
use crate::transaction::Transaction;

/// Timeout for initial peer discovery (seconds)
const DISCOVERY_TIMEOUT_SECS: u64 = 30;

/// Helper to get connected peer IDs as strings
fn get_connected_peer_ids(discovery: &NetworkDiscovery) -> Vec<String> {
    discovery
        .peer_table()
        .iter()
        .map(|p| p.peer_id.to_string())
        .collect()
}

/// Check if mining should be enabled based on quorum config and connected peers
fn check_mining_eligibility(
    config: &Config,
    connected_peers: &[String],
    want_to_mine: bool,
) -> (bool, String) {
    if !want_to_mine {
        return (false, "Mining not requested".to_string());
    }

    let (can_reach, quorum_size, threshold) = config.network.quorum.can_reach_quorum(connected_peers);

    if !can_reach {
        let mode_str = match config.network.quorum.mode {
            QuorumMode::Explicit => "explicit",
            QuorumMode::Recommended => "recommended",
        };
        return (
            false,
            format!(
                "Quorum not satisfied ({} mode): have {}, need {} nodes",
                mode_str, quorum_size, threshold
            ),
        );
    }

    (true, format!("Quorum satisfied: {}-of-{}", threshold, quorum_size))
}

/// Run the node
pub fn run(config_path: &Path, mine: bool) -> Result<()> {
    let config = Config::load(config_path).context("No wallet found. Run 'botho init' first.")?;

    println!("Botho node starting. Press Ctrl+C to stop.");

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

    // Determine minimum peers to wait for based on quorum config
    let min_peers_to_wait = match config.network.quorum.mode {
        QuorumMode::Explicit => 1, // In explicit mode, wait for at least one peer
        QuorumMode::Recommended => config.network.quorum.min_peers as usize,
    };

    // Wait for peers with timeout
    info!(
        "Waiting for peers (min: {}, timeout: {}s)...",
        min_peers_to_wait, DISCOVERY_TIMEOUT_SECS
    );

    let start = std::time::Instant::now();
    let deadline = Duration::from_secs(DISCOVERY_TIMEOUT_SECS);

    while start.elapsed() < deadline && discovery.peer_count() < min_peers_to_wait {
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

    // Show peer status
    println!();
    let connected_peers = get_connected_peer_ids(&discovery);
    if !connected_peers.is_empty() {
        println!("=== Connected Peers: {} ===", connected_peers.len());
        for peer in &connected_peers {
            println!("  - {}", peer);
        }
    } else {
        println!("=== No peers connected ===");
        if config.network.bootstrap_peers.is_empty() {
            warn!("No bootstrap peers configured. Add bootstrap_peers to config.toml");
        }
    }
    println!();

    // Check quorum status using new config-based logic
    let (can_mine_now, quorum_message) = check_mining_eligibility(&config, &connected_peers, mine);

    // Display quorum status
    let mode_str = match config.network.quorum.mode {
        QuorumMode::Explicit => format!("explicit (threshold: {})", config.network.quorum.threshold),
        QuorumMode::Recommended => format!("recommended (min_peers: {})", config.network.quorum.min_peers),
    };
    println!("Quorum mode: {}", mode_str);
    println!("Quorum status: {}", quorum_message);

    // Build QuorumBuilder for SCP (still needed for consensus service)
    let mut quorum = QuorumBuilder::new(config.network.quorum.threshold);
    for peer in discovery.peer_table() {
        quorum.add_member(peer.peer_id);
    }

    // Create the node
    let mut node = Node::new(config.clone(), config_path)?;

    // Start RPC server for thin wallet connections
    let rpc_addr: SocketAddr = format!("0.0.0.0:{}", config.network.rpc_port)
        .parse()
        .expect("Invalid RPC address");

    // Create shared state for RPC
    let mining_active = Arc::new(RwLock::new(false));
    let peer_count = Arc::new(RwLock::new(discovery.peer_count()));

    let rpc_state = Arc::new(RpcState::from_shared(
        node.shared_ledger(),
        node.shared_mempool(),
        mining_active.clone(),
        peer_count.clone(),
        node.wallet_view_key(),
        node.wallet_spend_key(),
    ));

    // Spawn RPC server task
    let rpc_state_clone = rpc_state.clone();
    tokio::spawn(async move {
        if let Err(e) = start_rpc_server(rpc_addr, rpc_state_clone).await {
            error!("RPC server error: {}", e);
        }
    });

    info!("RPC server listening on {}", rpc_addr);

    // Get chain state for consensus
    let ledger = node.shared_ledger();
    let chain_state = ledger.read().unwrap().get_chain_state()
        .map_err(|e| anyhow::anyhow!("Failed to get chain state: {}", e))?;

    // Create consensus service using local peer ID for node identity
    let local_peer_id = discovery.local_peer_id();
    let node_id = peer_id_to_node_id(&local_peer_id);

    // Build SCP quorum set from connected peers
    let scp_quorum_set = build_scp_quorum_set(&quorum);

    let mut consensus = ConsensusService::new(
        node_id,
        scp_quorum_set,
        ConsensusConfig::default(),
        chain_state,
    );

    info!("Consensus service initialized at slot {}", consensus.current_slot());

    // Track mining state - can change as peers connect/disconnect
    let mut mining_enabled = false;

    println!();
    node.print_status_public()?;

    // Start mining if quorum is satisfied
    if can_mine_now {
        info!("Starting mining - {}", quorum_message);
        node.start_mining_public()?;
        mining_enabled = true;
        *mining_active.write().unwrap() = true;
    } else if mine {
        warn!("Mining requested but {}", quorum_message);
        println!("Mining will start when quorum is satisfied.");
    }

    // Load pending transactions from file and broadcast them
    match node.load_pending_transactions() {
        Ok(pending_txs) if !pending_txs.is_empty() => {
            info!("Broadcasting {} pending transactions to network", pending_txs.len());
            for tx in &pending_txs {
                if let Err(e) = NetworkDiscovery::broadcast_transaction(&mut swarm, tx) {
                    debug!("Failed to broadcast pending tx: {}", e);
                }
            }
        }
        Ok(_) => {}
        Err(e) => {
            warn!("Failed to load pending transactions: {}", e);
        }
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
                                let state = node.shared_ledger().read().unwrap().get_chain_state();
                                if let Ok(state) = state {
                                    consensus.update_chain_state(state);
                                }
                            }
                        }
                        NetworkEvent::NewTransaction(tx) => {
                            debug!("Received transaction {} from network", hex::encode(&tx.hash()[0..8]));
                            // Add to mempool for inclusion in next block
                            if let Err(e) = node.submit_transaction(tx) {
                                debug!("Failed to add network transaction to mempool: {}", e);
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
                            // Update RPC peer count
                            *peer_count.write().unwrap() = discovery.peer_count();

                            // Re-evaluate mining eligibility
                            if mine && !mining_enabled {
                                let connected = get_connected_peer_ids(&discovery);
                                let (can_mine_now, msg) = check_mining_eligibility(&config, &connected, mine);
                                if can_mine_now {
                                    info!("Quorum reached! {}", msg);
                                    if let Err(e) = node.start_mining_public() {
                                        warn!("Failed to start mining: {}", e);
                                    } else {
                                        mining_enabled = true;
                                        *mining_active.write().unwrap() = true;
                                    }
                                }
                            }
                        }
                        NetworkEvent::PeerDisconnected(peer_id) => {
                            warn!("Peer disconnected: {}", peer_id);
                            // Update RPC peer count
                            *peer_count.write().unwrap() = discovery.peer_count();

                            // Re-evaluate mining eligibility
                            if mining_enabled {
                                let connected = get_connected_peer_ids(&discovery);
                                let (can_mine_now, msg) = check_mining_eligibility(&config, &connected, mine);
                                if !can_mine_now {
                                    warn!("Quorum lost! {} - stopping mining", msg);
                                    node.stop_mining_public();
                                    mining_enabled = false;
                                    *mining_active.write().unwrap() = false;
                                }
                            }
                        }
                        NetworkEvent::SyncRequest { peer, request_id: _, request, channel } => {
                            use crate::network::{SyncRequest, SyncResponse};
                            debug!("Sync request from {:?}: {:?}", peer, request);
                            // Handle the sync request
                            let shared_ledger = node.shared_ledger();
                            let response = match request {
                                SyncRequest::GetStatus => {
                                    let ledger = shared_ledger.read().unwrap();
                                    let state = ledger.get_chain_state().unwrap_or_default();
                                    SyncResponse::Status {
                                        height: state.height,
                                        tip_hash: state.tip_hash,
                                    }
                                }
                                SyncRequest::GetBlocks { start_height, count } => {
                                    let ledger = shared_ledger.read().unwrap();
                                    let mut blocks = Vec::new();
                                    let end_height = start_height.saturating_add(count as u64).saturating_sub(1);
                                    for height in start_height..=end_height.min(start_height + 99) {
                                        if let Ok(block) = ledger.get_block(height) {
                                            blocks.push(block);
                                        } else {
                                            break;
                                        }
                                    }
                                    let has_more = blocks.len() == count as usize;
                                    SyncResponse::Blocks { blocks, has_more }
                                }
                            };
                            // Send response
                            if let Err(e) = NetworkDiscovery::send_sync_response(&mut swarm, channel, response) {
                                warn!("Failed to send sync response: {:?}", e);
                            }
                        }
                        NetworkEvent::SyncResponse { peer: _, request_id: _, response } => {
                            use crate::network::SyncResponse;
                            match response {
                                SyncResponse::Blocks { blocks, has_more: _ } => {
                                    debug!("Received {} blocks from sync", blocks.len());
                                    for block in blocks {
                                        if let Err(e) = node.add_block_from_network(&block) {
                                            warn!("Failed to add synced block: {}", e);
                                            break;
                                        }
                                    }
                                    // Update consensus chain state
                                    let shared_ledger = node.shared_ledger();
                                    let state = shared_ledger.read().unwrap().get_chain_state();
                                    if let Ok(state) = state {
                                        consensus.update_chain_state(state);
                                    }
                                }
                                SyncResponse::Status { height, tip_hash } => {
                                    debug!("Peer at height {} with tip {}", height, hex::encode(&tip_hash[0..8]));
                                }
                                SyncResponse::Error(e) => {
                                    warn!("Sync error from peer: {}", e);
                                }
                            }
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
                                        let state = node.shared_ledger().read().unwrap().get_chain_state();
                                        if let Ok(state) = state {
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
                let connected = get_connected_peer_ids(&discovery);
                let (_, quorum_status) = check_mining_eligibility(&config, &connected, mine);
                info!(
                    "Peers: {} | Mining: {} | {}",
                    connected.len(),
                    if mining_enabled { "active" } else { "inactive" },
                    quorum_status
                );
            }

            // Check for mined mining transactions
            _ = tokio::time::sleep(Duration::from_millis(100)) => {
                // Only check for mined transactions if mining is enabled
                if !mining_enabled {
                    continue;
                }
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

                    // Broadcast to network so other nodes see it
                    if let Err(e) = NetworkDiscovery::broadcast_transaction(&mut swarm, &tx) {
                        debug!("Failed to broadcast transaction: {}", e);
                    }

                    // Submit to consensus for ordering
                    consensus.submit_transaction(tx_hash, tx_bytes);
                }
            }
        }
    }

    node.stop_mining_public();
    // Update RPC mining status
    *mining_active.write().unwrap() = false;
    Ok(())
}

/// Build SCP quorum set from QuorumBuilder
fn build_scp_quorum_set(quorum: &QuorumBuilder) -> QuorumSet {
    use bth_consensus_scp_types::QuorumSetMember;

    // Create NodeIDs from actual PeerIds
    let members: Vec<QuorumSetMember<NodeID>> = quorum
        .members()
        .into_iter()
        .map(|peer_id| {
            let node_id = peer_id_to_node_id(&peer_id);
            QuorumSetMember::Node(node_id)
        })
        .collect();

    QuorumSet::new(quorum.threshold() as u32, members)
}

/// Convert a libp2p PeerId to an SCP NodeID
fn peer_id_to_node_id(peer_id: &libp2p::PeerId) -> NodeID {
    // Use the PeerId's string representation as the responder ID
    // This provides a deterministic mapping from PeerId to NodeID
    let peer_str = peer_id.to_string();
    let responder_id = ResponderId::from_str(&format!("{}:8443", &peer_str[..12.min(peer_str.len())]))
        .unwrap_or_else(|_| ResponderId::from_str("peer:8443").unwrap());

    // Derive a deterministic Ed25519 public key from the PeerId bytes
    // This is a placeholder - in production, peers should exchange actual keys
    let peer_bytes = peer_id.to_bytes();
    let mut key_bytes = [0u8; 32];
    let copy_len = peer_bytes.len().min(32);
    key_bytes[..copy_len].copy_from_slice(&peer_bytes[..copy_len]);

    NodeID {
        responder_id,
        public_key: Ed25519Public::try_from(&key_bytes[..])
            .unwrap_or_else(|_| Ed25519Public::default()),
    }
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
