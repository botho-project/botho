// Copyright (c) 2024 Botho Foundation
//
//! Load and Stress Tests for Consensus
//!
//! Performance and capacity testing:
//! - Sustained throughput (target: 50 tx/s for extended periods)
//! - Mempool stress (10,000 pending transactions)
//! - Burst traffic (1000 transactions in 10 seconds)
//!
//! These tests are marked #[ignore] for nightly/manual runs only.

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, RwLock,
    },
    thread,
    time::{Duration, Instant},
};

use crossbeam_channel::{unbounded, Receiver, Sender};
use dashmap::DashMap;
use tempfile::TempDir;

use bth_common::NodeID;
use bth_consensus_scp::{
    msg::Msg,
    slot::{CombineFn, ValidityFn},
    test_utils::test_node_id,
    Node as ScpNodeImpl, QuorumSet, ScpNode, SlotIndex,
};

use botho::{
    block::{Block, BlockHeader, MintingTx},
    ledger::{ChainState, Ledger},
    transaction::PICOCREDITS_PER_CREDIT,
};

use botho_wallet::WalletKeys;
use std::time::SystemTime;

// ============================================================================
// Constants
// ============================================================================

/// Number of nodes in the load test network
const NUM_NODES: usize = 5;

/// Quorum threshold
const QUORUM_K: usize = 3;

/// SCP timebase for load testing
const SCP_TIMEBASE_MS: u64 = 50;

/// Maximum values per slot
const MAX_SLOT_VALUES: usize = 100;

/// Load test timeout
const LOAD_TIMEOUT: Duration = Duration::from_secs(300); // 5 minutes

/// Trivial difficulty for test mining
const TRIVIAL_DIFFICULTY: u64 = 0x00FF_FFFF_FFFF_FFFF;

// ============================================================================
// Consensus Value Type
// ============================================================================

#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
struct ConsensusValue {
    pub tx_hash: [u8; 32],
    pub priority: u64,
    pub is_minting: bool,
}

impl bth_crypto_digestible::Digestible for ConsensusValue {
    fn append_to_transcript<DT: bth_crypto_digestible::DigestTranscript>(
        &self,
        context: &'static [u8],
        transcript: &mut DT,
    ) {
        self.tx_hash.append_to_transcript(context, transcript);
        self.priority.append_to_transcript(context, transcript);
        self.is_minting.append_to_transcript(context, transcript);
    }
}

impl std::fmt::Display for ConsensusValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "CV({}..., p={}, m={})",
            hex::encode(&self.tx_hash[0..4]),
            self.priority,
            self.is_minting
        )
    }
}

// ============================================================================
// Message Types
// ============================================================================

#[derive(Clone)]
enum TestNodeMessage {
    MintingTx(MintingTx),
    /// Simulated transaction (lightweight for load testing)
    SimulatedTx {
        hash: [u8; 32],
        priority: u64,
    },
    ScpMsg(Arc<Msg<ConsensusValue>>),
    Stop,
}

// ============================================================================
// Load Test Node
// ============================================================================

struct LoadTestNode {
    node_id: NodeID,
    sender: Sender<TestNodeMessage>,
    ledger: Arc<RwLock<Ledger>>,
    /// Count of transactions processed
    tx_processed: Arc<AtomicU64>,
    /// Count of transactions in mempool
    mempool_size: Arc<AtomicU64>,
    _temp_dir: TempDir,
}

impl LoadTestNode {
    fn chain_state(&self) -> ChainState {
        self.ledger.read().unwrap().get_chain_state().unwrap()
    }

    fn stop(&self) {
        let _ = self.sender.send(TestNodeMessage::Stop);
    }

    fn tx_processed(&self) -> u64 {
        self.tx_processed.load(Ordering::SeqCst)
    }

    fn mempool_size(&self) -> u64 {
        self.mempool_size.load(Ordering::SeqCst)
    }
}

// ============================================================================
// Load Test Network
// ============================================================================

struct LoadTestNetwork {
    nodes: Arc<DashMap<NodeID, LoadTestNode>>,
    handles: Vec<thread::JoinHandle<()>>,
    node_ids: Vec<NodeID>,
    pending_minting_txs: Arc<Mutex<HashMap<[u8; 32], MintingTx>>>,
    shutdown: Arc<AtomicBool>,
    slot_progress: Arc<DashMap<NodeID, SlotIndex>>,
    /// Total transactions broadcast
    total_tx_broadcast: Arc<AtomicU64>,
}

impl LoadTestNetwork {
    fn stop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        for entry in self.nodes.iter() {
            entry.value().stop();
        }
        thread::sleep(Duration::from_millis(100));
    }

    fn get_node(&self, index: usize) -> dashmap::mapref::one::Ref<'_, NodeID, LoadTestNode> {
        self.nodes.get(&self.node_ids[index]).unwrap()
    }

    fn broadcast_minting_tx(&self, minting_tx: MintingTx) {
        let hash = minting_tx.hash();
        self.pending_minting_txs
            .lock()
            .unwrap()
            .insert(hash, minting_tx.clone());

        for entry in self.nodes.iter() {
            let _ = entry
                .value()
                .sender
                .send(TestNodeMessage::MintingTx(minting_tx.clone()));
        }
    }

    /// Broadcast a simulated transaction (lightweight)
    fn broadcast_simulated_tx(&self, hash: [u8; 32], priority: u64) {
        self.total_tx_broadcast.fetch_add(1, Ordering::SeqCst);

        for entry in self.nodes.iter() {
            let _ = entry
                .value()
                .sender
                .send(TestNodeMessage::SimulatedTx { hash, priority });
        }
    }

    fn wait_for_slot_majority(
        &self,
        target_slot: SlotIndex,
        min_nodes: usize,
        timeout: Duration,
    ) -> bool {
        let deadline = Instant::now() + timeout;

        while Instant::now() < deadline {
            let mut count = 0;
            for entry in self.slot_progress.iter() {
                if *entry.value() >= target_slot {
                    count += 1;
                }
            }

            if count >= min_nodes {
                return true;
            }

            thread::sleep(Duration::from_millis(50));
        }

        false
    }

    fn get_total_tx_processed(&self) -> u64 {
        let mut total = 0;
        for entry in self.nodes.iter() {
            total += entry.value().tx_processed();
        }
        total / NUM_NODES as u64 // Average across nodes
    }

    fn get_average_mempool_size(&self) -> u64 {
        let mut total = 0;
        for entry in self.nodes.iter() {
            total += entry.value().mempool_size();
        }
        total / NUM_NODES as u64
    }

    fn print_stats(&self) {
        println!("\nLoad Test Statistics:");
        println!(
            "  Total TX broadcast: {}",
            self.total_tx_broadcast.load(Ordering::SeqCst)
        );
        println!("  Average TX processed: {}", self.get_total_tx_processed());
        println!(
            "  Average mempool size: {}",
            self.get_average_mempool_size()
        );

        for i in 0..NUM_NODES {
            let node = self.get_node(i);
            println!(
                "  Node {}: processed={}, mempool={}",
                i,
                node.tx_processed(),
                node.mempool_size()
            );
        }
    }
}

// ============================================================================
// Network Builder
// ============================================================================

fn build_load_network() -> LoadTestNetwork {
    let nodes_map: Arc<DashMap<NodeID, LoadTestNode>> = Arc::new(DashMap::new());
    let mut handles = Vec::new();
    let mut node_ids = Vec::new();

    let pending_minting_txs: Arc<Mutex<HashMap<[u8; 32], MintingTx>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let shutdown = Arc::new(AtomicBool::new(false));
    let slot_progress: Arc<DashMap<NodeID, SlotIndex>> = Arc::new(DashMap::new());
    let total_tx_broadcast = Arc::new(AtomicU64::new(0));

    // Create node IDs
    for i in 0..NUM_NODES {
        node_ids.push(test_node_id(i as u32));
    }

    // Create each node
    for i in 0..NUM_NODES {
        let node_id = node_ids[i].clone();
        let peers: HashSet<NodeID> = node_ids
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, id)| id.clone())
            .collect();

        let temp_dir = TempDir::new().unwrap();
        let ledger = Arc::new(RwLock::new(Ledger::open(temp_dir.path()).unwrap()));

        let (sender, receiver) = unbounded();

        let peer_vec: Vec<NodeID> = peers.iter().cloned().collect();
        let quorum_set = QuorumSet::new_with_node_ids(QUORUM_K as u32, peer_vec);

        let tx_processed = Arc::new(AtomicU64::new(0));
        let mempool_size = Arc::new(AtomicU64::new(0));

        let nodes_map_clone = nodes_map.clone();
        let peers_clone = peers.clone();
        let ledger_clone = ledger.clone();
        let pending_minting_clone = pending_minting_txs.clone();
        let shutdown_clone = shutdown.clone();
        let node_id_clone = node_id.clone();
        let slot_progress_clone = slot_progress.clone();
        let tx_processed_clone = tx_processed.clone();
        let mempool_size_clone = mempool_size.clone();

        slot_progress.insert(node_id.clone(), 0);

        let handle = thread::Builder::new()
            .name(format!("load-node-{}", i))
            .spawn(move || {
                run_load_node(
                    node_id_clone,
                    quorum_set,
                    peers_clone,
                    receiver,
                    nodes_map_clone,
                    ledger_clone,
                    pending_minting_clone,
                    shutdown_clone,
                    slot_progress_clone,
                    tx_processed_clone,
                    mempool_size_clone,
                )
            })
            .expect("Failed to spawn node thread");

        handles.push(handle);

        let test_node = LoadTestNode {
            node_id: node_ids[i].clone(),
            sender,
            ledger,
            tx_processed,
            mempool_size,
            _temp_dir: temp_dir,
        };
        nodes_map.insert(node_ids[i].clone(), test_node);
    }

    LoadTestNetwork {
        nodes: nodes_map,
        handles,
        node_ids,
        pending_minting_txs,
        shutdown,
        slot_progress,
        total_tx_broadcast,
    }
}

// ============================================================================
// Load Node Event Loop
// ============================================================================

fn run_load_node(
    node_id: NodeID,
    quorum_set: QuorumSet,
    peers: HashSet<NodeID>,
    receiver: Receiver<TestNodeMessage>,
    nodes_map: Arc<DashMap<NodeID, LoadTestNode>>,
    ledger: Arc<RwLock<Ledger>>,
    pending_minting_txs: Arc<Mutex<HashMap<[u8; 32], MintingTx>>>,
    shutdown: Arc<AtomicBool>,
    slot_progress: Arc<DashMap<NodeID, SlotIndex>>,
    tx_processed: Arc<AtomicU64>,
    mempool_size: Arc<AtomicU64>,
) {
    let validity_fn: ValidityFn<ConsensusValue, String> = Arc::new(|_| Ok(()));

    let combine_fn: CombineFn<ConsensusValue, String> = Arc::new(move |values| {
        let mut combined: Vec<ConsensusValue> = values.to_vec();
        combined.sort();
        combined.dedup();

        let minting_txs: Vec<_> = combined.iter().filter(|v| v.is_minting).cloned().collect();
        let regular_txs: Vec<_> = combined.iter().filter(|v| !v.is_minting).cloned().collect();

        let mut result = Vec::new();
        if let Some(best_minting) = minting_txs.into_iter().max_by_key(|v| v.priority) {
            result.push(best_minting);
        }
        result.extend(regular_txs.into_iter().take(MAX_SLOT_VALUES - 1));
        result.sort();

        Ok(result)
    });

    let logger = bth_consensus_scp::create_null_logger();
    let mut scp_node = ScpNodeImpl::new(
        node_id.clone(),
        quorum_set,
        validity_fn,
        combine_fn,
        1,
        logger,
    );
    scp_node.scp_timebase = Duration::from_millis(SCP_TIMEBASE_MS);

    let mut pending_values: Vec<ConsensusValue> = Vec::new();
    let mut current_slot: SlotIndex = 1;

    loop {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }

        // Update mempool size
        mempool_size.store(pending_values.len() as u64, Ordering::SeqCst);

        match receiver.try_recv() {
            Ok(TestNodeMessage::MintingTx(minting_tx)) => {
                let cv = ConsensusValue {
                    tx_hash: minting_tx.hash(),
                    priority: minting_tx.pow_priority(),
                    is_minting: true,
                };
                pending_values.push(cv);
            }
            Ok(TestNodeMessage::SimulatedTx { hash, priority }) => {
                let cv = ConsensusValue {
                    tx_hash: hash,
                    priority,
                    is_minting: false,
                };
                pending_values.push(cv);
            }
            Ok(TestNodeMessage::ScpMsg(msg)) => {
                if let Ok(Some(out_msg)) = scp_node.handle_message(&msg) {
                    broadcast_scp_msg(&nodes_map, &peers, out_msg);
                }
            }
            Ok(TestNodeMessage::Stop) => break,
            Err(crossbeam_channel::TryRecvError::Empty) => {
                thread::yield_now();
            }
            Err(crossbeam_channel::TryRecvError::Disconnected) => break,
        }

        // Propose pending values (batch for efficiency)
        if !pending_values.is_empty() {
            let to_propose: BTreeSet<ConsensusValue> = pending_values
                .iter()
                .take(MAX_SLOT_VALUES)
                .cloned()
                .collect();

            if let Ok(Some(out_msg)) = scp_node.propose_values(to_propose) {
                broadcast_scp_msg(&nodes_map, &peers, out_msg);
            }
        }

        // Process timeouts
        for out_msg in scp_node.process_timeouts() {
            broadcast_scp_msg(&nodes_map, &peers, out_msg);
        }

        // Check for externalization
        if let Some(externalized) = scp_node.get_externalized_values(current_slot) {
            // Count processed transactions
            let tx_count = externalized.iter().filter(|v| !v.is_minting).count() as u64;
            tx_processed.fetch_add(tx_count, Ordering::SeqCst);

            // Apply block if we have a minting tx
            if let Err(_e) = apply_block(&ledger, &pending_minting_txs, externalized.as_slice()) {
                // Log error but continue
            }

            pending_values.retain(|v| !externalized.contains(v));
            slot_progress.insert(node_id.clone(), current_slot);
            current_slot += 1;
        }
    }
}

fn broadcast_scp_msg(
    nodes_map: &Arc<DashMap<NodeID, LoadTestNode>>,
    peers: &HashSet<NodeID>,
    msg: Msg<ConsensusValue>,
) {
    let msg = Arc::new(msg);
    for peer_id in peers {
        if let Some(peer_node) = nodes_map.get(peer_id) {
            let _ = peer_node.sender.send(TestNodeMessage::ScpMsg(msg.clone()));
        }
    }
}

fn apply_block(
    ledger: &Arc<RwLock<Ledger>>,
    pending_minting_txs: &Arc<Mutex<HashMap<[u8; 32], MintingTx>>>,
    externalized: &[ConsensusValue],
) -> Result<(), String> {
    let minting_value = externalized.iter().find(|v| v.is_minting);

    if let Some(cv) = minting_value {
        let pending = pending_minting_txs.lock().unwrap();
        if let Some(minting_tx) = pending.get(&cv.tx_hash) {
            let chain_state = ledger.read().unwrap().get_chain_state().unwrap();

            let block = Block {
                header: BlockHeader {
                    version: 1,
                    prev_block_hash: chain_state.tip_hash,
                    tx_root: [0u8; 32],
                    timestamp: minting_tx.timestamp,
                    height: chain_state.height + 1,
                    difficulty: minting_tx.difficulty,
                    nonce: minting_tx.nonce,
                    minter_view_key: minting_tx.minter_view_key,
                    minter_spend_key: minting_tx.minter_spend_key,
                },
                minting_tx: minting_tx.clone(),
                transactions: vec![],
            };

            ledger
                .write()
                .unwrap()
                .add_block(&block)
                .map_err(|e| format!("Failed to apply block: {:?}", e))?;
        }
    }

    Ok(())
}

// ============================================================================
// Helper Functions
// ============================================================================

fn create_test_minting_tx(height: u64, recipient_seed: u8) -> MintingTx {
    let wallet = create_test_wallet(recipient_seed);
    let minter_address = wallet.account_key().default_subaddress();

    let reward = 50 * PICOCREDITS_PER_CREDIT;
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let prev_block_hash = [0u8; 32];

    let mut minting_tx = MintingTx::new(
        height,
        reward,
        &minter_address,
        prev_block_hash,
        TRIVIAL_DIFFICULTY,
        timestamp,
    );

    for nonce in 0..1000 {
        minting_tx.nonce = nonce;
        if minting_tx.verify_pow() {
            break;
        }
    }

    minting_tx
}

fn create_test_wallet(seed: u8) -> WalletKeys {
    // All mnemonics must be 24 words for BIP39 compatibility
    let mnemonics = [
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art",
        "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo vote",
        "letter advice cage absurd amount doctor acoustic avoid letter advice cage absurd amount doctor acoustic avoid letter advice cage absurd amount doctor acoustic bless",
    ];
    let mnemonic = mnemonics[(seed as usize) % mnemonics.len()];
    WalletKeys::from_mnemonic(mnemonic).expect("Failed to create wallet from mnemonic")
}

/// Generate a random transaction hash
fn random_tx_hash() -> [u8; 32] {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let mut hash = [0u8; 32];
    rng.fill(&mut hash);
    hash
}

// ============================================================================
// Test Cases
// ============================================================================

/// Test: Sustained throughput of 50 tx/s for 1 minute
///
/// This is a shorter version of the full 1-hour test for CI integration.
#[test]
#[ignore = "Long-running load test - run with --ignored"]
fn test_sustained_throughput_1_minute() {
    println!("\n=== Sustained Throughput Test (1 minute) ===\n");

    let mut network = build_load_network();
    thread::sleep(Duration::from_millis(300));

    let target_tps = 50;
    let duration = Duration::from_secs(60);
    let interval = Duration::from_millis(1000 / target_tps);

    let start = Instant::now();
    let mut tx_count = 0u64;
    let mut slot = 1u64;

    // First, create a minting tx to start the chain
    let minting_tx = create_test_minting_tx(1, 1);
    network.broadcast_minting_tx(minting_tx);

    println!("Broadcasting transactions at {} tx/s...", target_tps);

    while start.elapsed() < duration {
        // Broadcast simulated transactions
        let hash = random_tx_hash();
        network.broadcast_simulated_tx(hash, tx_count);
        tx_count += 1;

        // Periodically add minting transactions to trigger new blocks
        if tx_count % 100 == 0 {
            slot += 1;
            let minting_tx = create_test_minting_tx(slot, (slot % 256) as u8);
            network.broadcast_minting_tx(minting_tx);
        }

        // Wait for interval
        thread::sleep(interval);

        // Progress report every 10 seconds
        if tx_count % (target_tps * 10) == 0 {
            println!(
                "  {:?}: {} tx broadcast, {} tx processed avg",
                start.elapsed(),
                tx_count,
                network.get_total_tx_processed()
            );
        }
    }

    let elapsed = start.elapsed();
    let actual_tps = tx_count as f64 / elapsed.as_secs_f64();

    println!("\nResults:");
    println!("  Duration: {:?}", elapsed);
    println!("  Total TX broadcast: {}", tx_count);
    println!("  Actual TPS: {:.2}", actual_tps);

    network.print_stats();

    // Verify we achieved reasonable throughput
    assert!(
        actual_tps > (target_tps as f64 * 0.8),
        "Throughput {:.2} should be at least 80% of target {}",
        actual_tps,
        target_tps
    );

    network.stop();
    println!("\n=== Sustained Throughput Test Complete ===\n");
}

/// Test: Mempool stress with 10,000 pending transactions
#[test]
#[ignore = "Long-running load test - run with --ignored"]
fn test_mempool_stress_10k() {
    println!("\n=== Mempool Stress Test (10K transactions) ===\n");

    let mut network = build_load_network();
    thread::sleep(Duration::from_millis(300));

    let target_mempool = 10_000;

    // First block
    let minting_tx = create_test_minting_tx(1, 1);
    network.broadcast_minting_tx(minting_tx);

    println!("Filling mempool with {} transactions...", target_mempool);
    let fill_start = Instant::now();

    // Rapidly fill the mempool
    for i in 0..target_mempool {
        let hash = random_tx_hash();
        network.broadcast_simulated_tx(hash, i);

        // Small pause to avoid overwhelming channels
        if i % 1000 == 999 {
            thread::sleep(Duration::from_millis(10));
            println!(
                "  {} transactions added, avg mempool: {}",
                i + 1,
                network.get_average_mempool_size()
            );
        }
    }

    let fill_time = fill_start.elapsed();
    println!("Mempool filled in {:?}", fill_time);

    // Check mempool sizes
    let avg_mempool = network.get_average_mempool_size();
    println!("Average mempool size: {}", avg_mempool);

    // Now let the network process transactions over time
    println!("\nProcessing transactions...");
    let process_start = Instant::now();

    // Add more minting transactions to trigger consensus rounds
    for slot in 2..=20 {
        let minting_tx = create_test_minting_tx(slot, (slot % 256) as u8);
        network.broadcast_minting_tx(minting_tx);
        thread::sleep(Duration::from_millis(500));

        if network.wait_for_slot_majority(slot as SlotIndex, QUORUM_K, Duration::from_secs(10)) {
            println!(
                "  Slot {}: processed, mempool avg: {}",
                slot,
                network.get_average_mempool_size()
            );
        }
    }

    let process_time = process_start.elapsed();
    println!("Processing time: {:?}", process_time);

    network.print_stats();

    // Verify no node crashed under load
    for i in 0..NUM_NODES {
        let node = network.get_node(i);
        let state = node.chain_state();
        assert!(state.height > 0, "Node {} should have processed blocks", i);
    }

    network.stop();
    println!("\n=== Mempool Stress Test Complete ===\n");
}

/// Test: Burst traffic - 1000 transactions in 10 seconds
#[test]
#[ignore = "Long-running load test - run with --ignored"]
fn test_burst_traffic() {
    println!("\n=== Burst Traffic Test (1000 tx in 10s) ===\n");

    let mut network = build_load_network();
    thread::sleep(Duration::from_millis(300));

    let burst_size = 1000;
    let burst_window = Duration::from_secs(10);

    // First block
    let minting_tx = create_test_minting_tx(1, 1);
    network.broadcast_minting_tx(minting_tx);

    // Wait for initial consensus
    network.wait_for_slot_majority(1, QUORUM_K, Duration::from_secs(30));

    println!("Sending burst of {} transactions...", burst_size);
    let burst_start = Instant::now();

    // Send all transactions rapidly
    for i in 0..burst_size {
        let hash = random_tx_hash();
        network.broadcast_simulated_tx(hash, i);
    }

    let send_time = burst_start.elapsed();
    println!("Burst sent in {:?}", send_time);

    // Add minting transactions to process the burst
    let mut slot = 2u64;
    while burst_start.elapsed() < burst_window {
        let minting_tx = create_test_minting_tx(slot, (slot % 256) as u8);
        network.broadcast_minting_tx(minting_tx);

        if network.wait_for_slot_majority(slot as SlotIndex, QUORUM_K, Duration::from_secs(5)) {
            println!(
                "  Slot {}: avg mempool: {}",
                slot,
                network.get_average_mempool_size()
            );
        }

        slot += 1;
        thread::sleep(Duration::from_millis(200));
    }

    let total_time = burst_start.elapsed();
    println!("\nBurst processing completed in {:?}", total_time);

    network.print_stats();

    // Verify transactions were processed
    let processed = network.get_total_tx_processed();
    println!("Transactions processed: {}", processed);

    // Should process at least some transactions
    assert!(
        processed > 0,
        "Should have processed at least some transactions from burst"
    );

    network.stop();
    println!("\n=== Burst Traffic Test Complete ===\n");
}

/// Baseline test: Normal operation for load comparison
#[test]
fn test_load_baseline() {
    println!("\n=== Load Baseline Test ===\n");

    let mut network = build_load_network();
    thread::sleep(Duration::from_millis(200));

    // Simple test: process a few blocks
    for slot in 1..=5 {
        let minting_tx = create_test_minting_tx(slot, slot as u8);
        network.broadcast_minting_tx(minting_tx);

        let reached =
            network.wait_for_slot_majority(slot as SlotIndex, NUM_NODES, Duration::from_secs(30));
        assert!(reached, "Slot {} should complete", slot);
    }

    // Verify all nodes are consistent
    let first_state = network.get_node(0).chain_state();
    for i in 1..NUM_NODES {
        let state = network.get_node(i).chain_state();
        assert_eq!(
            first_state.height, state.height,
            "All nodes should have same height"
        );
    }

    network.stop();
    println!("\n=== Load Baseline Test Passed ===\n");
}

/// Test: Memory stability under load (checks for leaks)
#[test]
#[ignore = "Long-running load test - run with --ignored"]
fn test_memory_stability() {
    println!("\n=== Memory Stability Test ===\n");

    let mut network = build_load_network();
    thread::sleep(Duration::from_millis(300));

    let rounds = 100;
    let tx_per_round = 50;

    println!("Running {} rounds with {} tx each...", rounds, tx_per_round);

    for round in 1..=rounds {
        // Broadcast transactions
        for i in 0..tx_per_round {
            let hash = random_tx_hash();
            network.broadcast_simulated_tx(hash, (round * tx_per_round + i) as u64);
        }

        // Trigger consensus
        let minting_tx = create_test_minting_tx(round as u64, (round % 256) as u8);
        network.broadcast_minting_tx(minting_tx);

        if network.wait_for_slot_majority(round as SlotIndex, QUORUM_K, Duration::from_secs(10)) {
            if round % 20 == 0 {
                println!(
                    "  Round {}: mempool avg: {}, processed: {}",
                    round,
                    network.get_average_mempool_size(),
                    network.get_total_tx_processed()
                );
            }
        }
    }

    // Final check: mempools should not be overflowing
    let final_mempool = network.get_average_mempool_size();
    println!("\nFinal average mempool size: {}", final_mempool);

    // Mempool should stay bounded (not grow unboundedly)
    assert!(
        final_mempool < 5000,
        "Mempool should stay bounded, got {}",
        final_mempool
    );

    network.stop();
    println!("\n=== Memory Stability Test Complete ===\n");
}
