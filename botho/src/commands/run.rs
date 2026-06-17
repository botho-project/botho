// Copyright (c) 2024 Botho Foundation

use anyhow::{Context, Result};
use bth_common::{NodeID, ResponderId};
use bth_consensus_scp::QuorumSet;
use bth_crypto_keys::Ed25519Public;
use bth_gossip::{GossipConfig, PeerRateLimitConfig};
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
    network::{
        BlockTxn, ChainSyncManager, CompactBlock, GetBlockTxn, NetworkDiscovery, NetworkEvent,
        QuorumBuilder, ReconstructionResult, SyncAction, SyncRequest, SyncResponse,
    },
    node::{MintedMintingTx, Node, SharedLedger},
    rpc::{
        calculate_dir_size, init_metrics, start_metrics_server, start_rpc_server, FaucetState,
        MetricsUpdater, RpcState, WsBroadcaster, DATA_DIR_USAGE_BYTES,
    },
    transaction::Transaction,
    wallet::Wallet,
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

/// Helper to get connected peers as libp2p `PeerId`s.
///
/// Used to drive the chain-sync state machine, which needs the typed peer
/// identifiers (not their string form) to address sync request/response
/// messages.
fn get_connected_peers(discovery: &NetworkDiscovery) -> Vec<libp2p::PeerId> {
    discovery.peer_table().iter().map(|p| p.peer_id).collect()
}

/// Decide whether the faucet should pause minting due to high confirmed
/// balance.
///
/// The faucet pauses minting when its confirmed balance climbs above
/// `high_threshold` to avoid accumulating coins indefinitely. **Crucially, this
/// pause must only apply when there are no pending transactions to mine.** When
/// the faucet is the sole minter and the mempool is non-empty, pausing would
/// deadlock the chain: the pending transaction (e.g. a dispense) can never be
/// mined, so it never confirms, so the confirmed balance never drops, so
/// minting never resumes. See issue #386.
fn should_pause_for_balance(balance: u64, high_threshold: u64, mempool_len: usize) -> bool {
    balance > high_threshold && mempool_len == 0
}

/// Decide whether a faucet that is currently paused-for-balance should resume
/// minting.
///
/// Two independent conditions trigger a resume:
/// 1. The confirmed balance has dropped below `low_threshold` (the original
///    anti-accumulation hysteresis), or
/// 2. There are pending transactions in the mempool that need to be mined. When
///    the faucet is the sole minter, leaving them unmined deadlocks the chain
///    (issue #386), so a non-empty mempool always forces a resume regardless of
///    balance. Once the mempool drains and the balance is still high, the pause
///    re-engages via [`should_pause_for_balance`].
///
/// The caller is responsible for the additional quorum eligibility check before
/// actually resuming.
fn should_resume_from_balance_pause(balance: u64, low_threshold: u64, mempool_len: usize) -> bool {
    balance < low_threshold || mempool_len > 0
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

/// Build the consensus config, honoring an optional test-only fixed-timing
/// override.
///
/// Production runs use [`ConsensusConfig::default`] (dynamic block timing).
/// When `BOTHO_SLOT_DURATION_SECS` is set to a positive integer, consensus
/// instead uses a *fixed* slot duration of that many seconds with dynamic
/// timing disabled. This exists purely so automated end-to-end tests (e.g. the
/// web-wallet → tx → ledger node-backed test) can drive a solo node fast enough
/// to pre-mine the ~20 blocks needed for a CLSAG decoy ring without waiting on
/// the default 40s dynamic block time. It is a no-op when the variable is
/// unset, so it never changes mainnet/testnet behavior.
fn consensus_config_from_env() -> ConsensusConfig {
    match std::env::var("BOTHO_SLOT_DURATION_SECS") {
        Ok(raw) => match raw.trim().parse::<u64>() {
            Ok(secs) if secs > 0 => {
                warn!(
                    "BOTHO_SLOT_DURATION_SECS={} set: using fixed {}s consensus slot timing \
                     (test/dev only, dynamic timing disabled)",
                    secs, secs
                );
                ConsensusConfig::fixed_timing(secs)
            }
            _ => {
                warn!(
                    "Ignoring invalid BOTHO_SLOT_DURATION_SECS={:?}: must be a positive integer",
                    raw
                );
                ConsensusConfig::default()
            }
        },
        Err(_) => ConsensusConfig::default(),
    }
}

/// Run the node
pub fn run(
    config_path: &Path,
    mint: bool,
    mint_threads: Option<u32>,
    metrics_port_override: Option<u16>,
) -> Result<()> {
    let mut config =
        Config::load(config_path).context("Config not found. Run 'botho init' first.")?;

    // Apply CLI overrides
    if let Some(port) = metrics_port_override {
        config.network.metrics_port = Some(port);
    }
    if let Some(threads) = mint_threads {
        config.minting.threads = threads;
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

    // Keep a copy of the bootstrap multiaddrs so the main loop can re-dial them
    // if the node ends up with no peers. The initial dial happens during the
    // pre-loop discovery window; if that connection is lost (e.g. a transient
    // drop right after startup) there is otherwise no path back to the network
    // for a node whose only peers are bootstrap nodes (issue #409).
    let reconnect_bootstrap_peers = bootstrap_peers.clone();

    // Start network discovery.
    //
    // Size the gossipsub per-peer rate limiter from the *effective* slot
    // cadence and the peer ceiling so honest multi-node SCP/minting traffic is
    // never silently dropped (issue #413). The fastest cadence the node may run
    // at determines the highest honest message rate: under dynamic timing the
    // slot duration can drop to `MIN_BLOCK_TIME_SECS`, and under the
    // `BOTHO_SLOT_DURATION_SECS` test override it is whatever was configured.
    // We use the smaller of the two as the rate-limit basis (a smaller slot ->
    // higher rate -> higher cap), and the gossip layer's connection ceiling as
    // the peer bound (each connected peer may broadcast SCP/minting traffic).
    let consensus_cfg = consensus_config_from_env();
    let effective_slot_secs = if consensus_cfg.dynamic_timing {
        ConsensusConfig::MIN_BLOCK_TIME_SECS
    } else {
        consensus_cfg.slot_duration.as_secs().max(1)
    };
    let rate_limit_peers = GossipConfig::default().max_connections.max(1) as u32;
    let rate_limit_config =
        PeerRateLimitConfig::for_slot_duration(effective_slot_secs, rate_limit_peers);
    let mut discovery = NetworkDiscovery::with_rate_limit_config(
        config.network.gossip_port(network_type),
        bootstrap_peers,
        rate_limit_config,
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
                    match net_event {
                        NetworkEvent::PeerDiscovered(peer_id) => {
                            info!("Connected to peer: {}", peer_id);
                        }
                        NetworkEvent::PeerVersionIncompatible { peer, .. } => {
                            let _ = swarm.disconnect_peer_id(peer);
                        }
                        _ => {}
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

    let mut rpc_state = RpcState::from_shared(
        node.shared_ledger(),
        node.shared_mempool(),
        minting_active.clone(),
        peer_count.clone(),
        scp_peer_count.clone(),
        config.network_type,
        node.wallet_view_key(),
        node.wallet_spend_key(),
        config.network.cors_origins.clone(),
        ws_broadcaster.clone(),
    );

    // Initialize wallet for RPC (balance checking, faucet, etc.)
    if let Some(mnemonic) = config.mnemonic() {
        match Wallet::from_mnemonic(mnemonic) {
            Ok(wallet) => {
                // Initialize faucet if enabled in config (testnet only)
                if config.faucet.enabled {
                    if !config.network_type.is_production() {
                        info!(
                            "Faucet enabled: {} BTH per request",
                            config.faucet.amount as f64 / 1_000_000_000_000.0
                        );
                        rpc_state =
                            rpc_state.with_faucet(FaucetState::new(config.faucet.clone()), wallet);
                    } else {
                        warn!("Faucet is configured but disabled on mainnet for safety");
                        rpc_state = rpc_state.with_wallet(wallet);
                    }
                } else {
                    // Add wallet for balance checking even without faucet
                    rpc_state = rpc_state.with_wallet(wallet);
                }
            }
            Err(e) => {
                warn!("Wallet not initialized: {}", e);
            }
        }
    } else {
        debug!("No wallet configured (running in relay mode)");
    }

    let rpc_state = Arc::new(rpc_state);

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

    // Create consensus service using local peer ID for node identity.
    // Own the PeerId (rather than borrowing from `discovery`) so we can still
    // call `&mut discovery` methods later while rebuilding the quorum set on
    // peer connect/disconnect.
    let local_peer_id = *discovery.local_peer_id();
    let node_id = peer_id_to_node_id(&local_peer_id);

    // Build SCP quorum set from connected peers (or just ourselves for solo mining)
    let scp_quorum_set = build_scp_quorum_set(&quorum, &local_peer_id);

    // Capture the starting height before chain_state is moved into the
    // consensus service; the sync state machine needs it to know how far
    // behind the network we are at startup.
    let local_height = chain_state.height;

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
        consensus_config_from_env(),
        chain_state,
    );

    info!(
        "Consensus service initialized at slot {}",
        consensus.current_slot()
    );

    // Chain-sync state machine: drives initial block download (IBD) / catch-up.
    //
    // A node joining an existing chain only learns about the current tip via
    // gossip; that tip is rejected by the ledger because the intermediate
    // blocks are missing ("Expected height 1, got N"). The sync manager closes
    // this gap by polling peers for their chain status and, when we are behind,
    // requesting the missing block range and applying it sequentially.
    let mut sync_manager = ChainSyncManager::new(local_height);

    // Track minting state - can change as peers connect/disconnect
    let mut minting_enabled = false;
    // Track if minting was paused due to high faucet balance
    let mut minting_paused_for_balance = false;

    // Faucet balance thresholds (in picocredits)
    // Stop minting if balance > 10,000 BTH
    const FAUCET_BALANCE_HIGH: u64 = 10_000_000_000_000_000;
    // Resume minting if balance < 5,000 BTH
    const FAUCET_BALANCE_LOW: u64 = 5_000_000_000_000_000;

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
    // Drive the chain-sync (IBD / catch-up) state machine. A short interval
    // keeps a freshly joined node requesting peer status and missing block
    // ranges promptly so it can reach the network tip without waiting on
    // gossip alone.
    let mut sync_tick = tokio::time::interval(Duration::from_secs(2));
    let mut minting_check_interval = tokio::time::interval(Duration::from_millis(100));
    let mut faucet_balance_interval = tokio::time::interval(Duration::from_secs(10));
    // Periodically re-dial bootstrap peers when we have no connections, so a
    // node that lost its only connection right after startup can rejoin
    // (issue #409). Cheap and a no-op once peers are connected.
    let mut reconnect_interval = tokio::time::interval(Duration::from_secs(5));

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
                        NetworkEvent::NewMintingTx(minting_tx) => {
                            // A peer proposed this minting tx for consensus.
                            // Validate it against our chain state, then register
                            // it in the consensus tx cache so the local SCP node
                            // can validate (and therefore accept) the peer's
                            // nominate/ballot messages referencing it (issue #409).
                            let tx_hash = minting_tx.hash();
                            debug!("Received minting tx {} from network", hex::encode(&tx_hash[0..8]));

                            let chain_state = node
                                .shared_ledger()
                                .read()
                                .ok()
                                .and_then(|ledger| ledger.get_chain_state().ok());

                            if let Some(chain_state) = chain_state {
                                let temp_state = Arc::new(RwLock::new(chain_state));
                                let validator = TransactionValidator::new(temp_state);
                                match validator.validate_minting_tx(&minting_tx) {
                                    Ok(()) => {
                                        let tx_bytes = bincode::serialize(&minting_tx)
                                            .expect("Failed to serialize minting tx");
                                        consensus.register_minting_tx(tx_hash, tx_bytes);
                                    }
                                    Err(e) => {
                                        debug!(error = %e, "Rejecting invalid peer minting tx");
                                    }
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

                            // Reconfigure the consensus quorum to include the
                            // newly connected peer. This is what lifts a node
                            // out of latched solo mode without a restart.
                            let new_qs = rebuild_scp_quorum_set(&config, &discovery, &local_peer_id);
                            if consensus.reconfigure_quorum(new_qs) {
                                info!(
                                    threshold = consensus.quorum_set().threshold,
                                    members = consensus.quorum_set().members.len(),
                                    "Consensus quorum reconfigured after peer connect"
                                );
                            }

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
                            // Drop any sync state tied to this peer so we
                            // re-discover and re-select a sync source.
                            sync_manager.on_peer_disconnected(&peer_id);
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

                            // Shrink the consensus quorum to drop the departed
                            // peer so we don't deadlock waiting on a member that
                            // is gone. rebuild_scp_quorum_set always yields a
                            // non-empty set (local node is always a member) and
                            // recomputes a satisfiable threshold, so churn can
                            // never produce an empty or unreachable quorum.
                            let new_qs = rebuild_scp_quorum_set(&config, &discovery, &local_peer_id);
                            if consensus.reconfigure_quorum(new_qs) {
                                info!(
                                    threshold = consensus.quorum_set().threshold,
                                    members = consensus.quorum_set().members.len(),
                                    "Consensus quorum reconfigured after peer disconnect"
                                );
                            }

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
                        NetworkEvent::SyncResponse { peer, request_id: _, response } => {
                            match response {
                                SyncResponse::Blocks { blocks, has_more } => {
                                    debug!("Received {} blocks from sync (has_more={})", blocks.len(), has_more);
                                    // Hand the batch to the sync state machine so it can
                                    // advance its download cursor and decide what to fetch
                                    // next. Blocks are applied sequentially via the ledger,
                                    // which is what lets a fresh node catch up to a chain
                                    // that is already at height N.
                                    if let Some(SyncAction::AddBlocks(blocks)) =
                                        sync_manager.on_blocks(&peer, blocks, has_more)
                                    {
                                        let mut applied_any = false;
                                        for block in &blocks {
                                            if let Err(e) = node.add_block_from_network(block) {
                                                warn!("Failed to add synced block {}: {}", block.height(), e);
                                                sync_manager.on_failure(Some(&peer), e.to_string());
                                                break;
                                            }
                                            applied_any = true;
                                            // Record for dynamic timing
                                            consensus.record_block(block.header.timestamp, block.transactions.len());
                                        }

                                        if applied_any {
                                            // Update dynamic fee after syncing (use last block's tx count)
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
                                            // Update consensus chain state and inform the
                                            // sync manager of our new height so it can tell
                                            // whether we have caught up to the target.
                                            if let Ok(ledger) = node.shared_ledger().read() {
                                                if let Ok(state) = ledger.get_chain_state() {
                                                    metrics_updater.set_block_height(state.height);
                                                    sync_manager.on_blocks_added(state.height);
                                                    consensus.update_chain_state(state);
                                                }
                                            }
                                        }
                                    }
                                }
                                SyncResponse::Status { height, tip_hash } => {
                                    debug!("Peer {:?} at height {} with tip {}", peer, height, hex::encode(&tip_hash[0..8]));
                                    sync_manager.on_status(peer, height, tip_hash);
                                }
                                SyncResponse::Error(e) => {
                                    warn!("Sync error from peer {:?}: {}", peer, e);
                                    sync_manager.on_failure(Some(&peer), e);
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

                        NetworkEvent::PeerVersionIncompatible { peer, peer_version, local_version } => {
                            warn!(
                                %peer,
                                peer_version = %peer_version,
                                local_version = %local_version,
                                "Disconnecting consensus-incompatible peer (protocol major mismatch)"
                            );
                            let _ = swarm.disconnect_peer_id(peer);
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

            // Reconnect tick: if we have no peers, re-dial configured bootstrap
            // peers. The initial dial happens before the main loop; a node that
            // lost that connection during startup would otherwise be stranded
            // with no way back to the network (issue #409).
            _ = reconnect_interval.tick() => {
                if discovery.peer_count() == 0 && !reconnect_bootstrap_peers.is_empty() {
                    for peer_addr in &reconnect_bootstrap_peers {
                        if let Ok(addr) = peer_addr.parse::<libp2p::Multiaddr>() {
                            debug!("Reconnect: re-dialing bootstrap peer {}", addr);
                            if let Err(e) = swarm.dial(addr) {
                                debug!("Reconnect dial failed: {}", e);
                            }
                        }
                    }
                }
            }

            // Chain sync (IBD / catch-up) tick. Drives the sync state machine,
            // emitting status/block requests as needed so a node behind the
            // network tip backfills the missing blocks from a peer.
            _ = sync_tick.tick() => {
                // Keep the sync manager's view of our local height current so
                // it can detect when we have fallen behind (e.g. after gossip
                // delivered blocks, or a peer advanced past us).
                if let Ok(ledger) = node.shared_ledger().read() {
                    if let Ok(state) = ledger.get_chain_state() {
                        sync_manager.set_local_height(state.height);
                    }
                }

                let connected = get_connected_peers(&discovery);
                if let Some(action) = sync_manager.tick(&connected) {
                    match action {
                        SyncAction::RequestStatus(peer) => {
                            debug!("Sync: requesting status from {:?}", peer);
                            sync_manager.on_request_sent(peer);
                            NetworkDiscovery::send_sync_request(&mut swarm, peer, SyncRequest::GetStatus);
                        }
                        SyncAction::RequestBlocks { peer, start_height, count } => {
                            debug!(
                                "Sync: requesting blocks [{}..{}] from {:?}",
                                start_height,
                                start_height + count as u64 - 1,
                                peer
                            );
                            sync_manager.on_request_sent(peer);
                            NetworkDiscovery::send_sync_request(
                                &mut swarm,
                                peer,
                                SyncRequest::GetBlocks { start_height, count },
                            );
                        }
                        SyncAction::Synced => {
                            debug!("Sync: caught up with network tip");
                        }
                        SyncAction::AddBlocks(_) | SyncAction::Wait(_) => {
                            // AddBlocks is produced only by on_blocks() (handled
                            // in the SyncResponse arm); Wait is advisory.
                        }
                    }
                }
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

                // Broadcast the minting tx so peers can validate the
                // corresponding SCP consensus value when its hash appears in a
                // nominate/ballot message. Without this, peers reject our SCP
                // messages ("Transaction not in cache") and multi-node
                // nomination never reaches quorum (issue #409).
                if let Err(e) = NetworkDiscovery::broadcast_minting_tx(&mut swarm, minting_tx) {
                    debug!("Failed to broadcast minting tx: {}", e);
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

            // Check faucet balance and control minting accordingly
            _ = faucet_balance_interval.tick() => {
                // Only check balance if faucet is configured with a wallet
                if let Some(wallet) = &rpc_state.wallet {
                    // Get wallet balance by scanning UTXOs
                    let balance = match rpc_state.ledger.read() {
                        Ok(ledger) => {
                            match ledger.scan_utxos_for_account(wallet.account_key()) {
                                Ok(utxos) => {
                                    // Filter to unspent UTXOs and sum amounts
                                    let mempool = rpc_state.mempool.read().ok();
                                    let mut total = 0u64;
                                    for utxo in &utxos {
                                        if let Some(subaddr_idx) = utxo.output.belongs_to(wallet.account_key()) {
                                            if let Some(onetime_key) = utxo.output.recover_spend_key(wallet.account_key(), subaddr_idx) {
                                                let key_image = bth_crypto_ring_signature::KeyImage::from(&onetime_key);
                                                let key_image_bytes = key_image.as_bytes();

                                                // Check if pending in mempool
                                                if let Some(ref mp) = mempool {
                                                    if mp.is_key_image_pending(key_image_bytes) {
                                                        continue;
                                                    }
                                                }

                                                // Check if spent on-chain
                                                if let Ok(None) = ledger.is_key_image_spent(key_image_bytes) {
                                                    total += utxo.output.amount;
                                                }
                                            }
                                        }
                                    }
                                    total
                                }
                                Err(e) => {
                                    warn!("Failed to scan UTXOs for balance check: {}", e);
                                    continue;
                                }
                            }
                        }
                        Err(e) => {
                            warn!("Failed to read ledger for balance check: {}", e);
                            continue;
                        }
                    };

                    let balance_bth = balance as f64 / 1_000_000_000_000.0;

                    // Number of pending transactions awaiting inclusion in a
                    // block. The balance-gated pause must never apply while
                    // there is work to mine, otherwise a sole-minter faucet
                    // deadlocks the chain (issue #386): a pending dispense can
                    // never confirm, so the confirmed balance never drops, so
                    // minting never resumes.
                    let mempool_len = rpc_state
                        .mempool
                        .read()
                        .map(|mp| mp.len())
                        .unwrap_or(0);

                    // Check if we should pause minting due to high balance.
                    // Only pause when the mempool is empty (nothing to mine).
                    if minting_enabled
                        && !minting_paused_for_balance
                        && should_pause_for_balance(balance, FAUCET_BALANCE_HIGH, mempool_len)
                    {
                        info!(
                            "Faucet balance ({:.2} BTH) exceeds threshold ({:.2} BTH) and mempool is empty - pausing minting",
                            balance_bth,
                            FAUCET_BALANCE_HIGH as f64 / 1_000_000_000_000.0
                        );
                        node.stop_minting_public();
                        minting_enabled = false;
                        minting_paused_for_balance = true;
                        if let Ok(mut active) = minting_active.write() {
                            *active = false;
                        }
                        metrics_updater.set_minting_active(false);
                        ws_broadcaster.minting_status(false, 0.0, 0);
                    }

                    // Check if we should resume minting. We resume when the
                    // balance has dropped below the low threshold (original
                    // hysteresis) OR when there are pending transactions to
                    // mine (which forces a block even at high balance, breaking
                    // the sole-minter deadlock).
                    if minting_paused_for_balance
                        && should_resume_from_balance_pause(balance, FAUCET_BALANCE_LOW, mempool_len)
                    {
                        // Check quorum before resuming
                        let connected = get_connected_peer_ids(&discovery);
                        let (can_mint, _) = check_minting_eligibility(&config, &connected, mint);
                        if can_mint {
                            let reason = if mempool_len > 0 {
                                format!("{} pending transaction(s) to mine", mempool_len)
                            } else {
                                format!(
                                    "balance ({:.2} BTH) below threshold ({:.2} BTH)",
                                    balance_bth,
                                    FAUCET_BALANCE_LOW as f64 / 1_000_000_000_000.0
                                )
                            };
                            info!("Resuming minting: {}", reason);
                            if let Err(e) = node.start_minting_public() {
                                warn!("Failed to resume minting: {}", e);
                            } else {
                                minting_enabled = true;
                                minting_paused_for_balance = false;
                                if let Ok(mut active) = minting_active.write() {
                                    *active = true;
                                }
                                metrics_updater.set_minting_active(true);
                                ws_broadcaster.minting_status(true, 0.0, 0);
                            }
                        }
                    }
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

/// Rebuild the SCP quorum set from the *current* set of connected peers.
///
/// Called whenever peers connect or disconnect so the consensus quorum tracks
/// live membership instead of being frozen at startup. Members are the local
/// node plus every connected peer; the threshold follows the configured quorum
/// policy:
///
/// - `Recommended`: BFT threshold `n - floor((n-1)/3)` over `n = peers + 1`
///   (matches [`QuorumConfig::effective_threshold`]).
/// - `Explicit`: the configured threshold, clamped to the member count so we
///   never demand more confirmations than there are members.
///
/// A lone node (no peers) yields a 1-of-1 solo quorum.
fn rebuild_scp_quorum_set(
    config: &Config,
    discovery: &NetworkDiscovery,
    local_peer_id: &libp2p::PeerId,
) -> QuorumSet {
    use bth_consensus_scp_types::QuorumSetMember;

    // Local node is always a member.
    let mut members: Vec<QuorumSetMember<NodeID>> =
        vec![QuorumSetMember::Node(peer_id_to_node_id(local_peer_id))];

    for peer in discovery.peer_table() {
        members.push(QuorumSetMember::Node(peer_id_to_node_id(&peer.peer_id)));
    }

    let n = members.len();
    let threshold = match config.network.quorum.mode {
        QuorumMode::Recommended => config.network.quorum.effective_threshold(n - 1) as u32,
        QuorumMode::Explicit => {
            // Clamp to member count; a threshold larger than n can never be met
            // and would deadlock consensus.
            (config.network.quorum.threshold).min(n as u32).max(1)
        }
    };

    QuorumSet::new(threshold, members)
}

/// Convert a libp2p PeerId to an SCP NodeID.
///
/// SCP identifies, hashes and orders nodes solely by their Ed25519 *public
/// key* (see `bth_common::NodeID`'s `Eq`/`Hash`/`Ord` impls, which ignore the
/// `responder_id`). The quorum set is therefore considered invalid — and every
/// outgoing SCP message is rejected by `Msg::validate` — if two distinct peers
/// map to the same public key. So the mapping from `PeerId` to public key MUST
/// be both *deterministic* (every node derives the same key for a given peer)
/// and *injective* (distinct peers get distinct, valid keys).
///
/// The previous implementation copied the first 32 bytes of
/// `peer_id.to_bytes()` straight into the key. For libp2p Ed25519 peers,
/// `to_bytes()` is an identity multihash whose first six bytes (`00 24 08 01 12
/// 20`) are a fixed prefix shared by *every* Ed25519 peer, and the trailing key
/// bytes are truncated. In practice this collapsed multiple peers onto the same
/// (often invalid) public key, producing an "Invalid quorum set" rejection that
/// silently prevented any block from being externalized in multi-node consensus
/// (issue #414).
///
/// We now hash the *full* PeerId bytes with a domain separator to obtain a
/// well-distributed 32-byte seed, then derive a real Ed25519 keypair from it.
/// Any 32-byte seed is a valid Ed25519 private key (the scalar is clamped
/// internally), so the resulting public key is always a valid curve point, and
/// distinct PeerIds yield distinct keys with overwhelming probability.
///
/// NOTE: this remains a deterministic *stand-in* for the peer's real signing
/// key — it is not the key the peer actually signs SCP messages with. It exists
/// only so every node builds an identical, valid quorum set from the same set
/// of connected PeerIds. Exchanging and verifying real per-peer signing keys is
/// tracked separately; this fix is limited to making the quorum-set membership
/// well-formed so SCP can externalize.
fn peer_id_to_node_id(peer_id: &libp2p::PeerId) -> NodeID {
    use sha2::{Digest, Sha256};

    // Use the PeerId's string representation as the responder ID. This is purely
    // informational for SCP (NodeID equality ignores it) but keeps logs
    // readable.
    let peer_str = peer_id.to_string();
    let responder_id =
        ResponderId::from_str(&format!("{}:8443", &peer_str[..12.min(peer_str.len())]))
            .unwrap_or_else(|_| ResponderId::from_str("peer:8443").unwrap());

    // Derive a deterministic, unique, valid Ed25519 public key from the *full*
    // PeerId bytes. Hashing avoids the fixed-multihash-prefix collision and the
    // truncation of the old implementation.
    let mut hasher = Sha256::new();
    hasher.update(b"botho-scp-node-id-v1");
    hasher.update(peer_id.to_bytes());
    let seed = hasher.finalize();

    // Any 32-byte value is a valid Ed25519 private key; deriving the public key
    // cannot fail for a 32-byte input, but fall back defensively just in case.
    let public_key = bth_crypto_keys::Ed25519Private::try_from(&seed[..])
        .map(|private| Ed25519Public::from(&private))
        .unwrap_or_default();

    NodeID {
        responder_id,
        public_key,
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
    let total_fees: u64 = block.transactions.iter().map(|tx| tx.fee).sum();
    let emission_share = block.minting_tx.lottery_emission_share();

    // Helper for error cases: burn only the fee burn share (the pool share
    // carries over via the persistent lottery pool), matching validation.
    let lottery_config = LotteryFeeConfig::default();
    let (_, fee_burn) = lottery_config.split_fees(total_fees);
    let burn_fee_share = |mut block: crate::block::Block| {
        block.lottery_summary = crate::block::BlockLotterySummary {
            total_fees,
            pool_distributed: 0,
            amount_burned: fee_burn,
            lottery_seed: [0u8; 32],
        };
        block
    };

    // Get the carryover pool and candidates from the ledger using the SAME
    // function and config that block validation uses — lottery verification
    // re-runs the draw, so proposer and validator state must be identical.
    let (stored_pool, candidates) = match shared_ledger.read() {
        Ok(ledger) => {
            let pool = match ledger.get_lottery_pool() {
                Ok(p) => p,
                Err(e) => {
                    warn!("Failed to get lottery pool: {}", e);
                    return burn_fee_share(block);
                }
            };
            match ledger
                .get_lottery_validation_candidates(block.height(), &lottery_config.draw_config)
            {
                Ok(c) => (pool, c),
                Err(e) => {
                    warn!("Failed to get lottery candidates: {}", e);
                    return burn_fee_share(block);
                }
            }
        }
        Err(e) => {
            warn!("Failed to acquire ledger lock for lottery: {}", e);
            return burn_fee_share(block);
        }
    };

    // Skip entirely only when there is nothing flowing in or out
    if total_fees == 0 && emission_share == 0 && stored_pool == 0 {
        return block;
    }

    info!(
        candidates = candidates.len(),
        fees = total_fees,
        emission_share = emission_share,
        stored_pool = stored_pool,
        "Applying lottery to block"
    );

    // Create UTXO lookup function for winner key recovery
    let ledger_clone = shared_ledger.clone();
    let utxo_lookup = move |utxo_id: &[u8; 36]| {
        let ledger = ledger_clone.read().ok()?;
        ledger.get_utxo_by_id(utxo_id).ok().flatten()
    };

    BlockBuilder::apply_lottery(
        block,
        &candidates,
        stored_pool,
        utxo_lookup,
        &lottery_config,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mirror the production faucet thresholds used in `run`.
    const HIGH: u64 = 10_000_000_000_000_000; // 10,000 BTH
    const LOW: u64 = 5_000_000_000_000_000; // 5,000 BTH

    #[test]
    fn pauses_when_balance_high_and_mempool_empty() {
        // Original anti-accumulation behavior: balance above HIGH and nothing
        // to mine -> pause.
        assert!(should_pause_for_balance(HIGH + 1, HIGH, 0));
    }

    #[test]
    fn does_not_pause_when_balance_high_but_mempool_nonempty() {
        // Regression test for issue #386: a sole-minter faucet must keep minting
        // when there are pending transactions, even with a high balance.
        // Otherwise the pending tx never confirms and the chain deadlocks.
        assert!(!should_pause_for_balance(HIGH + 1, HIGH, 1));
        assert!(!should_pause_for_balance(HIGH + 1, HIGH, 42));
    }

    #[test]
    fn does_not_pause_when_balance_below_high() {
        // Below the high threshold there is no reason to pause regardless of
        // mempool state.
        assert!(!should_pause_for_balance(HIGH, HIGH, 0));
        assert!(!should_pause_for_balance(HIGH - 1, HIGH, 0));
        assert!(!should_pause_for_balance(0, HIGH, 5));
    }

    #[test]
    fn resumes_when_balance_drops_below_low() {
        // Original hysteresis: balance falls below LOW -> resume.
        assert!(should_resume_from_balance_pause(LOW - 1, LOW, 0));
    }

    #[test]
    fn resumes_when_mempool_nonempty_even_at_high_balance() {
        // Regression test for issue #386: pending transactions force a resume
        // even when the balance is still well above the high threshold, so the
        // pending tx gets mined and the deadlock is broken.
        assert!(should_resume_from_balance_pause(HIGH + 1, LOW, 1));
        assert!(should_resume_from_balance_pause(LOW + 1, LOW, 3));
    }

    #[test]
    fn stays_paused_when_balance_high_and_mempool_empty() {
        // With nothing to mine and a balance still above LOW, the pause holds.
        assert!(!should_resume_from_balance_pause(HIGH + 1, LOW, 0));
        assert!(!should_resume_from_balance_pause(LOW + 1, LOW, 0));
        assert!(!should_resume_from_balance_pause(LOW, LOW, 0));
    }

    #[test]
    fn pause_and_resume_are_consistent_at_steady_state() {
        // When the mempool is empty and balance is between LOW and HIGH, the
        // faucet neither pauses (balance not above HIGH) nor, if already paused,
        // resumes (balance not below LOW): stable hysteresis band.
        let mid = LOW + (HIGH - LOW) / 2;
        assert!(!should_pause_for_balance(mid, HIGH, 0));
        assert!(!should_resume_from_balance_pause(mid, LOW, 0));
    }

    // ---- Issue #414: PeerId -> NodeID mapping must be deterministic + injective
    // ----

    /// Build a real libp2p Ed25519 PeerId from a 32-byte secret seed.
    fn ed25519_peer_id(seed: [u8; 32]) -> libp2p::PeerId {
        let secret = libp2p::identity::ed25519::SecretKey::try_from_bytes(seed.to_vec())
            .expect("valid ed25519 secret");
        let keypair =
            libp2p::identity::Keypair::from(libp2p::identity::ed25519::Keypair::from(secret));
        keypair.public().to_peer_id()
    }

    #[test]
    fn peer_id_to_node_id_is_deterministic() {
        // Regression test for issue #414: the mapping must be deterministic so
        // every node derives the SAME quorum-set membership for a given peer.
        let pid = ed25519_peer_id([7u8; 32]);
        let a = peer_id_to_node_id(&pid);
        let b = peer_id_to_node_id(&pid);
        assert_eq!(a, b, "same PeerId must map to the same NodeID");
        assert_eq!(a.public_key, b.public_key);
    }

    #[test]
    fn peer_id_to_node_id_is_injective_for_distinct_peers() {
        // Regression test for issue #414: distinct peers MUST map to distinct
        // public keys. NodeID equality is by public key only, so a collision
        // here makes the quorum set invalid (`Msg::validate` rejects every
        // outgoing SCP message) and multi-node consensus can never externalize.
        //
        // The previous implementation copied the first 32 bytes of
        // `peer_id.to_bytes()`, whose leading bytes are a fixed multihash prefix
        // shared by all Ed25519 peers, so distinct peers collided.
        let n = 16;
        let mut keys = std::collections::HashSet::new();
        let mut node_ids = std::collections::HashSet::new();
        for i in 0..n {
            let mut seed = [0u8; 32];
            seed[0] = i as u8 + 1;
            seed[31] = 0xAB;
            let pid = ed25519_peer_id(seed);
            let node_id = peer_id_to_node_id(&pid);
            // Distinct public keys.
            assert!(
                keys.insert(node_id.public_key.clone()),
                "duplicate public key derived for distinct PeerId {pid}"
            );
            // Distinct NodeIDs (which compare by public key).
            assert!(node_ids.insert(node_id));
        }
        assert_eq!(keys.len(), n);
    }

    #[test]
    fn two_peers_yield_a_valid_quorum_set() {
        // Regression test for issue #414: a 2-of-2 quorum built from two distinct
        // peers must be VALID. The bug produced two members with identical
        // public keys, so `QuorumSet::is_valid()` returned false (duplicate
        // member) and the externalize message was never broadcast, leaving both
        // minters stuck at height 0.
        use bth_consensus_scp_types::QuorumSetMember;

        let local = peer_id_to_node_id(&ed25519_peer_id([1u8; 32]));
        let peer = peer_id_to_node_id(&ed25519_peer_id([2u8; 32]));

        assert_ne!(
            local.public_key, peer.public_key,
            "two distinct peers must not share a public key"
        );

        let qs = QuorumSet::new(
            2,
            vec![QuorumSetMember::Node(local), QuorumSetMember::Node(peer)],
        );
        assert!(
            qs.is_valid(),
            "2-of-2 quorum from distinct peers must be valid: {qs:?}"
        );
        assert_eq!(
            qs.nodes().len(),
            2,
            "quorum must contain two distinct nodes"
        );
    }
}
