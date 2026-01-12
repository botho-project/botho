// Copyright (c) 2024 Botho Foundation

use anyhow::{Context, Result};
use bth_common::{NodeID, ResponderId};
use bth_consensus_scp::QuorumSet;
use bth_crypto_keys::Ed25519Public;
use futures::StreamExt;
use std::{
    net::SocketAddr,
    path::Path,
    str::FromStr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
    time::Duration,
};
use tracing::{debug, error, info, warn};

use std::collections::HashMap;

use crate::{
    block::MintingTx,
    config::{Config, QuorumMode},
    consensus::{
        BlockBuilder, ConsensusConfig, ConsensusEvent, ConsensusService, LotteryFeeConfig,
        TransactionValidator,
    },
    node::SharedLedger,
    network::{
        BlockTxn, CompactBlock, GetBlockTxn, NetworkDiscovery, NetworkEvent, QuorumBuilder,
        ReconstructionResult,
    },
    node::{MintedMintingTx, Node},
    rpc::{
        calculate_dir_size, init_metrics, start_metrics_server, start_rpc_server, MetricsUpdater,
        RpcState, WsBroadcaster, DATA_DIR_USAGE_BYTES,
    },
    transaction::Transaction,
};

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

/// Check if minting should be enabled based on quorum config and connected
/// peers
fn check_minting_eligibility(
    config: &Config,
    connected_peers: &[String],
    want_to_mint: bool,
) -> (bool, String) {
    if !want_to_mint {
        return (false, "Minting not requested".to_string());
    }

    let (can_reach, quorum_size, threshold) =
        config.network.quorum.can_reach_quorum(connected_peers);

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

    (
        true,
        format!("Quorum satisfied: {}-of-{}", threshold, quorum_size),
    )
}

/// Run the node
pub fn run(config_path: &Path, mint: bool, metrics_port_override: Option<u16>) -> Result<()> {
    let mut config =
        Config::load(config_path).context("Config not found. Run 'botho init' first.")?;

    // Apply metrics port override from CLI
    if let Some(port) = metrics_port_override {
        config.network.metrics_port = Some(port);
    }

    // Check if minting is requested without a wallet
    if mint && !config.has_wallet() {
        return Err(anyhow::anyhow!(
            "Cannot mine without a wallet. Run 'botho init' to create one, or remove --mint flag."
        ));
    }

    if config.has_wallet() {
        println!("Botho node starting. Press Ctrl+C to stop.");
    } else {
        println!("Botho relay node starting (no wallet). Press Ctrl+C to stop.");
    }

    // Create tokio runtime for async networking
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async { run_async(config, config_path, mint).await })
}

async fn run_async(config: Config, config_path: &Path, mint: bool) -> Result<()> {
    // Set up shutdown signal
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = shutdown.clone();
    ctrlc::set_handler(move || {
        shutdown_clone.store(true, Ordering::SeqCst);
    })?;

    // Get network type for port/peer defaults
    let network_type = config.network_type();

    // Discover bootstrap peers (uses DNS if enabled)
    let bootstrap_peers = config.network.bootstrap_peers_async(network_type).await;
    info!(
        "Using {} bootstrap peers (DNS: {})",
        bootstrap_peers.len(),
        config.network.dns_seeds.enabled
    );

    // Start network discovery
    let mut discovery =
        NetworkDiscovery::new(config.network.gossip_port(network_type), bootstrap_peers);

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
        if config.network.bootstrap_peers.is_empty() && !config.network.dns_seeds.enabled {
            warn!("No bootstrap peers configured and DNS discovery disabled. Add bootstrap_peers to config.toml or enable dns_seeds");
        }
    }
    println!();

    // Check quorum status using new config-based logic
    let (can_mint_now, quorum_message) = check_minting_eligibility(&config, &connected_peers, mint);

    // Display quorum status
    let mode_str = match config.network.quorum.mode {
        QuorumMode::Explicit => {
            format!("explicit (threshold: {})", config.network.quorum.threshold)
        }
        QuorumMode::Recommended => format!(
            "recommended (min_peers: {})",
            config.network.quorum.min_peers
        ),
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
    let rpc_addr: SocketAddr = format!("0.0.0.0:{}", config.network.rpc_port(network_type))
        .parse()
        .expect("Invalid RPC address");

    // Create shared state for RPC
    let minting_active = Arc::new(RwLock::new(false));
    let peer_count = Arc::new(RwLock::new(discovery.peer_count()));
    // SCP peer count tracks consensus participants (currently equals peer_count
    // as all peers participate in SCP consensus)
    let scp_peer_count = Arc::new(RwLock::new(discovery.peer_count()));
    let ws_broadcaster = Arc::new(WsBroadcaster::new(1024));

    let rpc_state = Arc::new(RpcState::from_shared(
        node.shared_ledger(),
        node.shared_mempool(),
        minting_active.clone(),
        peer_count.clone(),
        scp_peer_count.clone(),
        node.wallet_view_key(),
        node.wallet_spend_key(),
        config.network.cors_origins.clone(),
        ws_broadcaster.clone(),
    ));

    // Spawn RPC server task
    let rpc_state_clone = rpc_state.clone();
    tokio::spawn(async move {
        if let Err(e) = start_rpc_server(rpc_addr, rpc_state_clone).await {
            error!("RPC server error: {}", e);
        }
    });

    info!("RPC server listening on {}", rpc_addr);

    // Initialize and start Prometheus metrics server
    let metrics_updater = MetricsUpdater::new();
    init_metrics();

    // Update data directory size metric (initial + periodic updates every 60s)
    let data_dir = config_path.parent().unwrap_or(config_path).to_path_buf();
    // Initial update at startup
    if let Ok(size) = calculate_dir_size(&data_dir) {
        DATA_DIR_USAGE_BYTES.set(size as i64);
        debug!("Initial data_dir_usage_bytes: {} bytes", size);
    }
    // Spawn background task for periodic updates
    let data_dir_clone = data_dir.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        interval.tick().await; // Skip the immediate first tick (already done above)
        loop {
            interval.tick().await;
            if let Ok(size) = calculate_dir_size(&data_dir_clone) {
                DATA_DIR_USAGE_BYTES.set(size as i64);
                debug!("Updated data_dir_usage_bytes: {} bytes", size);
            }
        }
    });

    if let Some(metrics_port) = config.network.metrics_port(network_type) {
        let metrics_addr: SocketAddr = format!("0.0.0.0:{}", metrics_port)
            .parse()
            .expect("Invalid metrics address");

        tokio::spawn(async move {
            if let Err(e) = start_metrics_server(metrics_addr).await {
                error!("Metrics server error: {}", e);
            }
        });
    } else {
        info!("Prometheus metrics disabled (metrics_port = 0)");
    }

    // Get chain state for consensus
    let ledger = node.shared_ledger();
    let chain_state = ledger
        .read()
        .map_err(|_| anyhow::anyhow!("Ledger lock poisoned"))?
        .get_chain_state()
        .map_err(|e| anyhow::anyhow!("Failed to get chain state: {}", e))?;

    // Create consensus service using local peer ID for node identity
    let local_peer_id = discovery.local_peer_id();
    let node_id = peer_id_to_node_id(&local_peer_id);

    // Build SCP quorum set from connected peers (or just ourselves for solo mining)
    let scp_quorum_set = build_scp_quorum_set(&quorum, &local_peer_id);

    // Update initial metrics before chain_state is moved
    metrics_updater.set_block_height(chain_state.height);
    metrics_updater.set_difficulty(chain_state.difficulty);
    metrics_updater.set_total_minted(chain_state.total_mined);
    metrics_updater.set_total_fees_burned(chain_state.total_fees_burned);
    metrics_updater.set_peer_count(discovery.peer_count());
    if let Ok(mempool) = node.shared_mempool().read() {
        metrics_updater.set_mempool_size(mempool.len());
    }

    let mut consensus = ConsensusService::new(
        node_id,
        scp_quorum_set,
        ConsensusConfig::default(),
        chain_state,
    );

    info!(
        "Consensus service initialized at slot {}",
        consensus.current_slot()
    );

    // Track minting state - can change as peers connect/disconnect
    let mut minting_enabled = false;

    println!();
    node.print_status_public()?;

    // Start minting if quorum is satisfied
    if can_mint_now {
        info!("Starting minting - {}", quorum_message);
        node.start_minting_public()?;
        minting_enabled = true;
        if let Ok(mut active) = minting_active.write() {
            *active = true;
        }
        // Update metrics
        metrics_updater.set_minting_active(true);
        // Broadcast initial minting status to WebSocket clients
        ws_broadcaster.minting_status(true, 0.0, 0);
    } else if mint {
        warn!("Minting requested but {}", quorum_message);
        println!("Minting will start when quorum is satisfied.");
    }

    // Load pending transactions from file and broadcast them
    match node.load_pending_transactions() {
        Ok(pending_txs) if !pending_txs.is_empty() => {
            info!(
                "Broadcasting {} pending transactions to network",
                pending_txs.len()
            );
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
    let mut minting_check_interval = tokio::time::interval(Duration::from_millis(100));

    // Track pending compact blocks awaiting missing transactions
    // Key: block hash, Value: (compact block, missing indices)
    let mut pending_compact_blocks: HashMap<[u8; 32], (CompactBlock, Vec<u16>)> = HashMap::new();

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
                            let block_start = std::time::Instant::now();
                            if let Err(e) = node.add_block_from_network(&block) {
                                warn!("Failed to add network block: {}", e);
                            } else {
                                // Record block processing time
                                metrics_updater.observe_block_processing(block_start.elapsed().as_secs_f64());
                                metrics_updater.inc_blocks_processed();
                                metrics_updater.add_transactions_processed(block.transactions.len() as u64);

                                // Record for dynamic timing
                                consensus.record_block(block.header.timestamp, block.transactions.len());

                                // Update dynamic fee based on congestion
                                let slot_duration = consensus.current_slot_duration();
                                let at_min_time = ConsensusConfig::is_at_min_block_time(
                                    &ConsensusConfig::default(),
                                    slot_duration,
                                );
                                let max_txs = ConsensusConfig::default().max_txs_per_slot;
                                node.update_dynamic_fee_after_block(
                                    block.transactions.len(),
                                    max_txs,
                                    at_min_time,
                                );

                                // Broadcast to WebSocket clients
                                ws_broadcaster.new_block(
                                    block.height(),
                                    &block.hash(),
                                    block.header.timestamp,
                                    block.transactions.len(),
                                    block.header.difficulty,
                                );

                                // Update consensus chain state and metrics
                                if let Ok(ledger) = node.shared_ledger().read() {
                                    if let Ok(state) = ledger.get_chain_state() {
                                        consensus.update_chain_state(state.clone());
                                        metrics_updater.set_block_height(state.height);
                                        metrics_updater.set_difficulty(state.difficulty);
                                        metrics_updater.set_total_minted(state.total_mined);
                                        metrics_updater.set_total_fees_burned(state.total_fees_burned);
                                    }
                                }
                            }
                        }
                        NetworkEvent::NewTransaction(tx) => {
                            debug!("Received transaction {} from network", hex::encode(&tx.hash()[0..8]));
                            let tx_hash = tx.hash();
                            let tx_fee = tx.fee;
                            // Add to mempool for inclusion in next block
                            if let Err(e) = node.submit_transaction(tx) {
                                debug!("Failed to add network transaction to mempool: {}", e);
                            } else {
                                // Broadcast transaction and mempool update to WebSocket clients
                                ws_broadcaster.new_transaction(&tx_hash, tx_fee, None);
                                if let Ok(mempool) = node.shared_mempool().read() {
                                    ws_broadcaster.mempool_update(mempool.len(), mempool.total_fees());
                                    metrics_updater.set_mempool_size(mempool.len());
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
                            let new_peer_count = discovery.peer_count();

                            // Update RPC peer count and metrics
                            if let Ok(mut count) = peer_count.write() {
                                *count = new_peer_count;
                            }
                            // Update SCP peer count (all peers participate in consensus)
                            if let Ok(mut count) = scp_peer_count.write() {
                                *count = new_peer_count;
                            }
                            metrics_updater.set_peer_count(new_peer_count);

                            // Broadcast peer event to WebSocket clients
                            ws_broadcaster.peer_connected(new_peer_count, &peer_id.to_string());

                            // Re-evaluate minting eligibility
                            if mint && !minting_enabled {
                                let connected = get_connected_peer_ids(&discovery);
                                let (can_mint_now, msg) = check_minting_eligibility(&config, &connected, mint);
                                if can_mint_now {
                                    info!("Quorum reached! {}", msg);
                                    if let Err(e) = node.start_minting_public() {
                                        warn!("Failed to start minting: {}", e);
                                    } else {
                                        minting_enabled = true;
                                        if let Ok(mut active) = minting_active.write() {
                                            *active = true;
                                        }
                                        // Update metrics
                                        metrics_updater.set_minting_active(true);
                                        // Broadcast minting status change
                                        ws_broadcaster.minting_status(true, 0.0, 0);
                                    }
                                }
                            }
                        }
                        NetworkEvent::PeerDisconnected(peer_id) => {
                            warn!("Peer disconnected: {}", peer_id);
                            let new_peer_count = discovery.peer_count();

                            // Update RPC peer count and metrics
                            if let Ok(mut count) = peer_count.write() {
                                *count = new_peer_count;
                            }
                            // Update SCP peer count (all peers participate in consensus)
                            if let Ok(mut count) = scp_peer_count.write() {
                                *count = new_peer_count;
                            }
                            metrics_updater.set_peer_count(new_peer_count);

                            // Broadcast peer event to WebSocket clients
                            ws_broadcaster.peer_disconnected(new_peer_count, &peer_id.to_string());

                            // Re-evaluate minting eligibility
                            if minting_enabled {
                                let connected = get_connected_peer_ids(&discovery);
                                let (can_mint_now, msg) = check_minting_eligibility(&config, &connected, mint);
                                if !can_mint_now {
                                    warn!("Quorum lost! {} - stopping minting", msg);
                                    node.stop_minting_public();
                                    minting_enabled = false;
                                    if let Ok(mut active) = minting_active.write() {
                                        *active = false;
                                    }
                                    // Update metrics
                                    metrics_updater.set_minting_active(false);
                                    // Broadcast minting status change
                                    ws_broadcaster.minting_status(false, 0.0, 0);
                                }
                            }
                        }
                        NetworkEvent::SyncRequest { peer, request_id: _, request, channel } => {
                            use crate::network::{SyncRequest, SyncResponse};
                            debug!("Sync request from {:?}: {:?}", peer, request);
                            // Handle the sync request
                            let shared_ledger = node.shared_ledger();
                            let response = match shared_ledger.read() {
                                Ok(ledger) => match request {
                                    SyncRequest::GetStatus => {
                                        let state = ledger.get_chain_state().unwrap_or_default();
                                        SyncResponse::Status {
                                            height: state.height,
                                            tip_hash: state.tip_hash,
                                        }
                                    }
                                    SyncRequest::GetBlocks { start_height, count } => {
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
                                },
                                Err(_) => SyncResponse::Error("Internal error".to_string()),
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
                                    for block in &blocks {
                                        if let Err(e) = node.add_block_from_network(block) {
                                            warn!("Failed to add synced block: {}", e);
                                            break;
                                        }
                                        // Record for dynamic timing
                                        consensus.record_block(block.header.timestamp, block.transactions.len());
                                    }
                                    // Update dynamic fee after syncing all blocks (use last block's tx count)
                                    if let Some(last_block) = blocks.last() {
                                        let slot_duration = consensus.current_slot_duration();
                                        let at_min_time = ConsensusConfig::is_at_min_block_time(
                                            &ConsensusConfig::default(),
                                            slot_duration,
                                        );
                                        let max_txs = ConsensusConfig::default().max_txs_per_slot;
                                        node.update_dynamic_fee_after_block(
                                            last_block.transactions.len(),
                                            max_txs,
                                            at_min_time,
                                        );
                                    }
                                    // Update consensus chain state
                                    if let Ok(ledger) = node.shared_ledger().read() {
                                        if let Ok(state) = ledger.get_chain_state() {
                                            consensus.update_chain_state(state);
                                        }
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

                        // Compact block relay events
                        NetworkEvent::NewCompactBlock(compact_block) => {
                            let block_hash = compact_block.hash();
                            let height = compact_block.height();
                            info!(
                                height = height,
                                txs = compact_block.short_ids.len(),
                                "Received compact block"
                            );

                            // Check if we already have this block
                            if let Ok(ledger) = node.shared_ledger().read() {
                                if let Ok(state) = ledger.get_chain_state() {
                                    if state.height >= height {
                                        debug!("Already have block {}, ignoring compact block", height);
                                        continue;
                                    }
                                }
                            }

                            // Attempt reconstruction from mempool
                            let mempool = node.shared_mempool();
                            let reconstruction_result = if let Ok(mempool_guard) = mempool.read() {
                                compact_block.reconstruct(&*mempool_guard)
                            } else {
                                warn!("Failed to lock mempool for compact block reconstruction");
                                continue;
                            };

                            match reconstruction_result {
                                ReconstructionResult::Complete(block) => {
                                    info!(height = height, "Reconstructed block from compact block");

                                    // Remove from pending if it was there
                                    pending_compact_blocks.remove(&block_hash);

                                    // Add to ledger
                                    if let Err(e) = node.add_block_from_network(&block) {
                                        warn!("Failed to add reconstructed block: {}", e);
                                    } else {
                                        // Record for dynamic timing
                                        consensus.record_block(block.header.timestamp, block.transactions.len());

                                        // Update dynamic fee based on congestion
                                        let slot_duration = consensus.current_slot_duration();
                                        let at_min_time = ConsensusConfig::is_at_min_block_time(
                                            &ConsensusConfig::default(),
                                            slot_duration,
                                        );
                                        let max_txs = ConsensusConfig::default().max_txs_per_slot;
                                        node.update_dynamic_fee_after_block(
                                            block.transactions.len(),
                                            max_txs,
                                            at_min_time,
                                        );

                                        // Broadcast to WebSocket clients
                                        ws_broadcaster.new_block(
                                            block.height(),
                                            &block.hash(),
                                            block.header.timestamp,
                                            block.transactions.len(),
                                            block.header.difficulty,
                                        );

                                        // Update consensus chain state
                                        if let Ok(ledger) = node.shared_ledger().read() {
                                            if let Ok(state) = ledger.get_chain_state() {
                                                consensus.update_chain_state(state);
                                            }
                                        }
                                    }
                                }
                                ReconstructionResult::Incomplete { missing_indices } => {
                                    info!(
                                        height = height,
                                        missing = missing_indices.len(),
                                        "Compact block missing {} transactions, requesting",
                                        missing_indices.len()
                                    );

                                    // Store pending and request missing transactions
                                    pending_compact_blocks.insert(
                                        block_hash,
                                        (compact_block, missing_indices.clone()),
                                    );

                                    let request = GetBlockTxn {
                                        block_hash,
                                        indices: missing_indices,
                                    };

                                    if let Err(e) = NetworkDiscovery::request_block_txns(&mut swarm, &request) {
                                        warn!("Failed to request missing transactions: {}", e);
                                    }
                                }
                            }
                        }

                        NetworkEvent::GetBlockTxn { peer: _, request } => {
                            debug!(
                                block = hex::encode(&request.block_hash[0..8]),
                                indices = request.indices.len(),
                                "Received GetBlockTxn request"
                            );

                            // Look up the block and extract requested transactions
                            let response = if let Ok(ledger) = node.shared_ledger().read() {
                                // Search recent blocks (last 100) for the requested hash
                                match ledger.get_block_by_hash(&request.block_hash, 100) {
                                    Ok(Some(block)) => {
                                        let txs: Vec<Transaction> = request
                                            .indices
                                            .iter()
                                            .filter_map(|&idx| block.transactions.get(idx as usize).cloned())
                                            .collect();

                                        Some(BlockTxn {
                                            block_hash: request.block_hash,
                                            txs,
                                        })
                                    }
                                    Ok(None) => {
                                        debug!("Block not found for GetBlockTxn request");
                                        None
                                    }
                                    Err(e) => {
                                        warn!("Error looking up block: {}", e);
                                        None
                                    }
                                }
                            } else {
                                None
                            };

                            if let Some(response) = response {
                                if let Err(e) = NetworkDiscovery::respond_block_txns(&mut swarm, &response) {
                                    warn!("Failed to send BlockTxn response: {}", e);
                                }
                            }
                        }

                        NetworkEvent::BlockTxn(response) => {
                            debug!(
                                block = hex::encode(&response.block_hash[0..8]),
                                txs = response.txs.len(),
                                "Received BlockTxn response"
                            );

                            // Find the pending compact block
                            if let Some((mut compact_block, missing_indices)) =
                                pending_compact_blocks.remove(&response.block_hash)
                            {
                                // Add received transactions as prefilled
                                compact_block.add_transactions(&missing_indices, response.txs);

                                // Retry reconstruction
                                let mempool = node.shared_mempool();
                                let reconstruction_result = if let Ok(mempool_guard) = mempool.read() {
                                    compact_block.reconstruct(&*mempool_guard)
                                } else {
                                    warn!("Failed to lock mempool for retry reconstruction");
                                    continue;
                                };

                                match reconstruction_result {
                                    ReconstructionResult::Complete(block) => {
                                        info!(
                                            height = block.height(),
                                            "Completed block reconstruction after receiving missing txs"
                                        );

                                        if let Err(e) = node.add_block_from_network(&block) {
                                            warn!("Failed to add completed block: {}", e);
                                        } else {
                                            // Record for dynamic timing
                                            consensus.record_block(block.header.timestamp, block.transactions.len());

                                            // Update dynamic fee based on congestion
                                            let slot_duration = consensus.current_slot_duration();
                                            let at_min_time = ConsensusConfig::is_at_min_block_time(
                                                &ConsensusConfig::default(),
                                                slot_duration,
                                            );
                                            let max_txs = ConsensusConfig::default().max_txs_per_slot;
                                            node.update_dynamic_fee_after_block(
                                                block.transactions.len(),
                                                max_txs,
                                                at_min_time,
                                            );

                                            // Broadcast to WebSocket clients
                                            ws_broadcaster.new_block(
                                                block.height(),
                                                &block.hash(),
                                                block.header.timestamp,
                                                block.transactions.len(),
                                                block.header.difficulty,
                                            );

                                            // Update consensus chain state
                                            if let Ok(ledger) = node.shared_ledger().read() {
                                                if let Ok(state) = ledger.get_chain_state() {
                                                    consensus.update_chain_state(state);
                                                }
                                            }
                                        }
                                    }
                                    ReconstructionResult::Incomplete { missing_indices: still_missing } => {
                                        warn!(
                                            "Block still incomplete after BlockTxn, {} txs still missing",
                                            still_missing.len()
                                        );
                                        // Give up - will get full block from fallback
                                    }
                                }
                            } else {
                                debug!("Received BlockTxn for unknown compact block, ignoring");
                            }
                        }

                        NetworkEvent::UpgradeAnnouncement(announcement) => {
                            // Log upgrade announcements prominently
                            if announcement.is_hard_fork {
                                warn!(
                                    target_version = %announcement.target_version,
                                    target_block_version = announcement.target_block_version,
                                    activation_height = ?announcement.activation_height,
                                    activation_timestamp = ?announcement.activation_timestamp,
                                    description = %announcement.description,
                                    "⚠️  HARD FORK UPGRADE ANNOUNCED - Node upgrade required!"
                                );
                            } else {
                                info!(
                                    target_version = %announcement.target_version,
                                    target_block_version = announcement.target_block_version,
                                    description = %announcement.description,
                                    "Soft fork upgrade announced"
                                );
                            }
                        }

                        NetworkEvent::PeerVersionWarning { peer, peer_version, min_version } => {
                            warn!(
                                %peer,
                                peer_version = %peer_version,
                                min_version = %min_version,
                                "Connected to peer with outdated protocol version"
                            );
                        }

                        NetworkEvent::PexAddresses(addrs) => {
                            // Connect to new peers discovered via PEX
                            for addr in addrs {
                                debug!("Connecting to PEX-discovered peer: {}", addr);
                                if let Err(e) = swarm.dial(addr.clone()) {
                                    debug!("Failed to dial PEX peer: {}", e);
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
                                    // Apply lottery fee redistribution
                                    let block = apply_lottery_to_block(block, &node.shared_ledger());
                                    info!("Built block {} from consensus", block.height());

                                    // Add to ledger
                                    if let Err(e) = node.add_block_from_network(&block) {
                                        warn!("Failed to add consensus block: {}", e);
                                    } else {
                                        // Record for dynamic timing
                                        consensus.record_block(block.header.timestamp, block.transactions.len());

                                        // Broadcast to WebSocket clients
                                        ws_broadcaster.new_block(
                                            block.height(),
                                            &block.hash(),
                                            block.header.timestamp,
                                            block.transactions.len(),
                                            block.header.difficulty,
                                        );

                                        // Update consensus chain state
                                        if let Ok(ledger) = node.shared_ledger().read() {
                                            if let Ok(state) = ledger.get_chain_state() {
                                                consensus.update_chain_state(state);
                                            }
                                        }

                                        // Broadcast block with bandwidth optimization
                                        // Only send full block if there are legacy peers
                                        let legacy_peers = discovery.legacy_peer_count() > 0;
                                        if let Err(e) = NetworkDiscovery::broadcast_block_smart(
                                            &mut swarm,
                                            &block,
                                            legacy_peers,
                                        ) {
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

                            // For solo mining: loop our own message back to ourselves
                            // This is required because SCP needs to see its own messages to advance
                            if discovery.peer_count() == 0 {
                                if let Err(e) = consensus.handle_message(msg) {
                                    debug!("Failed to process own SCP message: {}", e);
                                }
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
                let (_, quorum_status) = check_minting_eligibility(&config, &connected, mint);
                info!(
                    "Peers: {} | Minting: {} | {}",
                    connected.len(),
                    if minting_enabled { "active" } else { "inactive" },
                    quorum_status
                );
            }

            // Check for minted minting transactions
            _ = minting_check_interval.tick() => {
                // Only check for mined transactions if minting is enabled
                if !minting_enabled {
                    continue;
                }

                // Drain all available transactions from the channel
                // This is important because many stale transactions may be queued
                let current_version = node.current_minting_work_version();
                let mut valid_tx: Option<MintedMintingTx> = None;
                let mut stale_count = 0u64;

                while let Some(minted_tx) = node.check_minted_minting_tx()? {
                    // Quick version check: discard stale transactions
                    if minted_tx.work_version != current_version {
                        stale_count += 1;
                        continue;
                    }
                    // Keep the transaction with highest priority
                    if valid_tx.as_ref().map(|t| minted_tx.pow_priority > t.pow_priority).unwrap_or(true) {
                        valid_tx = Some(minted_tx);
                    }
                }

                if stale_count > 0 {
                    debug!(stale_count, "Drained stale minting txs from channel");
                }

                let Some(minted_tx) = valid_tx else {
                    continue;
                };

                let minting_tx = &minted_tx.minting_tx;

                // Pre-validate the minting transaction before submitting to consensus
                // This catches any remaining invalid transactions
                let chain_state = match node.shared_ledger().read() {
                    Ok(ledger) => match ledger.get_chain_state() {
                        Ok(state) => state,
                        Err(e) => {
                            warn!("Cannot get chain state for validation: {}", e);
                            continue;
                        }
                    },
                    Err(_) => {
                        warn!("Cannot acquire ledger lock for validation");
                        continue;
                    }
                };

                let temp_state = Arc::new(RwLock::new(chain_state));
                let validator = TransactionValidator::new(temp_state);

                if let Err(e) = validator.validate_minting_tx(minting_tx) {
                    debug!(
                        height = minting_tx.block_height,
                        error = %e,
                        "Discarding stale minting tx (chain advanced)"
                    );
                    continue;
                }

                info!(
                    height = minting_tx.block_height,
                    priority = minted_tx.pow_priority,
                    "Submitting minting tx to consensus"
                );

                // Serialize and submit to consensus
                let tx_bytes = bincode::serialize(minting_tx)
                    .expect("Failed to serialize minting tx");
                let tx_hash = minting_tx.hash();

                consensus.submit_minting_tx(tx_hash, minted_tx.pow_priority, tx_bytes);

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

    node.stop_minting_public();
    // Update RPC minting status and metrics
    if let Ok(mut active) = minting_active.write() {
        *active = false;
    }
    metrics_updater.set_minting_active(false);
    Ok(())
}

/// Build SCP quorum set from QuorumBuilder
/// For solo mining (no peers), includes the local node as the only member
fn build_scp_quorum_set(quorum: &QuorumBuilder, local_peer_id: &libp2p::PeerId) -> QuorumSet {
    use bth_consensus_scp_types::QuorumSetMember;

    // Create NodeIDs from actual PeerIds
    let mut members: Vec<QuorumSetMember<NodeID>> = quorum
        .members()
        .into_iter()
        .map(|peer_id| {
            let node_id = peer_id_to_node_id(&peer_id);
            QuorumSetMember::Node(node_id)
        })
        .collect();

    // For solo mining: if no peers, include ourselves as the only quorum member
    if members.is_empty() {
        let local_node_id = peer_id_to_node_id(local_peer_id);
        members.push(QuorumSetMember::Node(local_node_id));
    }

    // Threshold is 1 for solo mining, otherwise use configured threshold
    let threshold = if members.len() == 1 {
        1
    } else {
        quorum.threshold()
    };

    QuorumSet::new(threshold, members)
}

/// Convert a libp2p PeerId to an SCP NodeID
fn peer_id_to_node_id(peer_id: &libp2p::PeerId) -> NodeID {
    // Use the PeerId's string representation as the responder ID
    // This provides a deterministic mapping from PeerId to NodeID
    let peer_str = peer_id.to_string();
    let responder_id =
        ResponderId::from_str(&format!("{}:8443", &peer_str[..12.min(peer_str.len())]))
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

/// Lottery configuration constants
/// Minimum UTXO age in blocks for lottery eligibility (roughly 1 hour)
const LOTTERY_MIN_AGE_BLOCKS: u64 = 360;
/// Minimum UTXO value for lottery eligibility (0.001 credits)
const LOTTERY_MIN_VALUE: u64 = 1_000_000_000;
/// Maximum lottery candidates to consider (DoS protection)
const LOTTERY_MAX_CANDIDATES: usize = 10_000;

/// Build a block from externalized consensus values
fn build_block_from_externalized(
    values: &[crate::consensus::ConsensusValue],
    consensus: &ConsensusService,
) -> Result<crate::block::Block> {
    BlockBuilder::build_from_externalized(
        values,
        |hash| {
            // Get minting tx from consensus cache
            consensus
                .get_tx_data(hash)
                .and_then(|bytes| bincode::deserialize::<MintingTx>(&bytes).ok())
        },
        |hash| {
            // Get transfer tx from consensus cache
            consensus
                .get_tx_data(hash)
                .and_then(|bytes| bincode::deserialize::<Transaction>(&bytes).ok())
        },
    )
    .map(|built| built.block)
    .map_err(|e| anyhow::anyhow!("Block build error: {}", e))
}

/// Apply lottery to a block using UTXOs from the ledger.
///
/// This draws lottery winners from the UTXO set and adds lottery outputs
/// to the block for fee redistribution.
fn apply_lottery_to_block(
    block: crate::block::Block,
    shared_ledger: &SharedLedger,
) -> crate::block::Block {
    // Skip lottery if no fees in the block
    let total_fees: u64 = block.transactions.iter().map(|tx| tx.fee).sum();
    if total_fees == 0 {
        return block;
    }

    // Get lottery candidates from ledger
    let candidates: Vec<crate::transaction::Utxo> = match shared_ledger.read() {
        Ok(ledger) => {
            match ledger.get_lottery_candidates(
                block.height(),
                LOTTERY_MIN_AGE_BLOCKS,
                LOTTERY_MIN_VALUE,
                LOTTERY_MAX_CANDIDATES,
            ) {
                Ok(c) => c,
                Err(e) => {
                    warn!("Failed to get lottery candidates: {}", e);
                    return block;
                }
            }
        }
        Err(e) => {
            warn!("Failed to acquire ledger lock for lottery: {}", e);
            return block;
        }
    };

    if candidates.is_empty() {
        debug!("No lottery candidates available, skipping lottery");
        return block;
    }

    info!(
        candidates = candidates.len(),
        fees = total_fees,
        "Applying lottery to block"
    );

    // Create UTXO lookup function for winner key recovery
    let ledger_clone = shared_ledger.clone();
    let utxo_lookup = move |utxo_id: &[u8; 36]| {
        let ledger = ledger_clone.read().ok()?;
        ledger.get_utxo_by_id(utxo_id).ok().flatten()
    };

    // Apply lottery with default configuration
    let lottery_config = LotteryFeeConfig::default();
    BlockBuilder::apply_lottery(block, &candidates, utxo_lookup, &lottery_config)
}
