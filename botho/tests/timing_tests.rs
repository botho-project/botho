// Copyright (c) 2024 Botho Foundation
//
//! Transaction Propagation Timing Tests
//!
//! Verifies that transactions propagate through the network within expected timeframes:
//! - Transaction reaches all nodes within 5 seconds
//! - Consensus is achieved within timeout period
//! - Block propagation meets latency requirements

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

/// Number of nodes in the timing test network
const NUM_NODES: usize = 5;

/// Quorum threshold (k=3 for 5 nodes)
const QUORUM_K: usize = 3;

/// SCP timebase for testing
const SCP_TIMEBASE_MS: u64 = 50;

/// Maximum values per slot
const MAX_SLOT_VALUES: usize = 50;

/// Target transaction propagation time
const TX_PROPAGATION_TARGET: Duration = Duration::from_secs(5);

/// Consensus timeout
const CONSENSUS_TIMEOUT: Duration = Duration::from_secs(30);

/// Trivial difficulty for test mining
const TRIVIAL_DIFFICULTY: u64 = 0x00FF_FFFF_FFFF_FFFF;

// ============================================================================
// Consensus Value Type (shared with other tests)
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
    ScpMsg(Arc<Msg<ConsensusValue>>),
    Stop,
}

// ============================================================================
// Timing Test Node
// ============================================================================

struct TimingTestNode {
    node_id: NodeID,
    sender: Sender<TestNodeMessage>,
    ledger: Arc<RwLock<Ledger>>,
    /// Track when transactions are received at this node
    tx_receive_times: Arc<DashMap<[u8; 32], Instant>>,
    _temp_dir: TempDir,
}

impl TimingTestNode {
    fn chain_state(&self) -> ChainState {
        self.ledger.read().unwrap().get_chain_state().unwrap()
    }

    fn stop(&self) {
        let _ = self.sender.send(TestNodeMessage::Stop);
    }

    /// Check if this node has received a transaction
    fn has_received_tx(&self, tx_hash: &[u8; 32]) -> bool {
        self.tx_receive_times.contains_key(tx_hash)
    }

    /// Get the time when a transaction was received
    fn get_tx_receive_time(&self, tx_hash: &[u8; 32]) -> Option<Instant> {
        self.tx_receive_times.get(tx_hash).map(|r| *r.value())
    }
}

// ============================================================================
// Timing Test Network
// ============================================================================

struct TimingTestNetwork {
    nodes: Arc<DashMap<NodeID, TimingTestNode>>,
    handles: Vec<thread::JoinHandle<()>>,
    node_ids: Vec<NodeID>,
    pending_minting_txs: Arc<Mutex<HashMap<[u8; 32], MintingTx>>>,
    shutdown: Arc<AtomicBool>,
    slot_progress: Arc<DashMap<NodeID, SlotIndex>>,
}

impl TimingTestNetwork {
    fn stop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        for entry in self.nodes.iter() {
            entry.value().stop();
        }
        thread::sleep(Duration::from_millis(100));
    }

    fn get_node(&self, index: usize) -> dashmap::mapref::one::Ref<'_, NodeID, TimingTestNode> {
        self.nodes.get(&self.node_ids[index]).unwrap()
    }

    /// Broadcast a minting tx and record the broadcast time
    fn broadcast_minting_tx(&self, minting_tx: MintingTx) -> Instant {
        let hash = minting_tx.hash();
        self.pending_minting_txs
            .lock()
            .unwrap()
            .insert(hash, minting_tx.clone());

        let broadcast_time = Instant::now();

        for entry in self.nodes.iter() {
            let _ = entry
                .value()
                .sender
                .send(TestNodeMessage::MintingTx(minting_tx.clone()));
        }

        broadcast_time
    }

    /// Wait for all nodes to receive a transaction
    fn wait_for_tx_in_all_nodes(&self, tx_hash: &[u8; 32], timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;

        while Instant::now() < deadline {
            let mut all_received = true;
            for entry in self.nodes.iter() {
                if !entry.value().has_received_tx(tx_hash) {
                    all_received = false;
                    break;
                }
            }

            if all_received {
                return true;
            }

            thread::sleep(Duration::from_millis(10));
        }

        false
    }

    /// Measure propagation time to all nodes
    fn measure_propagation_times(
        &self,
        tx_hash: &[u8; 32],
        broadcast_time: Instant,
    ) -> Vec<Duration> {
        let mut times = Vec::new();

        for entry in self.nodes.iter() {
            if let Some(receive_time) = entry.value().get_tx_receive_time(tx_hash) {
                times.push(receive_time.duration_since(broadcast_time));
            }
        }

        times
    }

    /// Wait for majority to reach slot
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

    /// Verify all nodes have consistent state
    fn verify_consistency(&self) {
        let first_node = self.get_node(0);
        let first_state = first_node.chain_state();

        for i in 1..NUM_NODES {
            let node = self.get_node(i);
            let state = node.chain_state();

            assert_eq!(
                first_state.height, state.height,
                "Node {} height mismatch: expected {}, got {}",
                i, first_state.height, state.height
            );

            assert_eq!(
                first_state.tip_hash, state.tip_hash,
                "Node {} tip hash mismatch",
                i
            );
        }
    }
}

// ============================================================================
// Network Builder
// ============================================================================

fn build_timing_network() -> TimingTestNetwork {
    let nodes_map: Arc<DashMap<NodeID, TimingTestNode>> = Arc::new(DashMap::new());
    let mut handles = Vec::new();
    let mut node_ids = Vec::new();

    let pending_minting_txs: Arc<Mutex<HashMap<[u8; 32], MintingTx>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let shutdown = Arc::new(AtomicBool::new(false));
    let slot_progress: Arc<DashMap<NodeID, SlotIndex>> = Arc::new(DashMap::new());

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

        let tx_receive_times: Arc<DashMap<[u8; 32], Instant>> = Arc::new(DashMap::new());

        let nodes_map_clone = nodes_map.clone();
        let peers_clone = peers.clone();
        let ledger_clone = ledger.clone();
        let pending_minting_clone = pending_minting_txs.clone();
        let shutdown_clone = shutdown.clone();
        let node_id_clone = node_id.clone();
        let slot_progress_clone = slot_progress.clone();
        let tx_receive_times_clone = tx_receive_times.clone();

        slot_progress.insert(node_id.clone(), 0);

        let handle = thread::Builder::new()
            .name(format!("timing-node-{}", i))
            .spawn(move || {
                run_timing_node(
                    node_id_clone,
                    quorum_set,
                    peers_clone,
                    receiver,
                    nodes_map_clone,
                    ledger_clone,
                    pending_minting_clone,
                    shutdown_clone,
                    slot_progress_clone,
                    tx_receive_times_clone,
                )
            })
            .expect("Failed to spawn node thread");

        handles.push(handle);

        let test_node = TimingTestNode {
            node_id: node_ids[i].clone(),
            sender,
            ledger,
            tx_receive_times,
            _temp_dir: temp_dir,
        };
        nodes_map.insert(node_ids[i].clone(), test_node);
    }

    TimingTestNetwork {
        nodes: nodes_map,
        handles,
        node_ids,
        pending_minting_txs,
        shutdown,
        slot_progress,
    }
}

// ============================================================================
// Timing Node Event Loop
// ============================================================================

fn run_timing_node(
    node_id: NodeID,
    quorum_set: QuorumSet,
    peers: HashSet<NodeID>,
    receiver: Receiver<TestNodeMessage>,
    nodes_map: Arc<DashMap<NodeID, TimingTestNode>>,
    ledger: Arc<RwLock<Ledger>>,
    pending_minting_txs: Arc<Mutex<HashMap<[u8; 32], MintingTx>>>,
    shutdown: Arc<AtomicBool>,
    slot_progress: Arc<DashMap<NodeID, SlotIndex>>,
    tx_receive_times: Arc<DashMap<[u8; 32], Instant>>,
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

        match receiver.try_recv() {
            Ok(TestNodeMessage::MintingTx(minting_tx)) => {
                let hash = minting_tx.hash();
                // Record receive time
                tx_receive_times.entry(hash).or_insert_with(Instant::now);

                let cv = ConsensusValue {
                    tx_hash: hash,
                    priority: minting_tx.pow_priority(),
                    is_minting: true,
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

        // Propose pending values
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
    nodes_map: &Arc<DashMap<NodeID, TimingTestNode>>,
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

// ============================================================================
// Test Cases
// ============================================================================

/// Test: Transaction propagates to all nodes within 5 seconds
#[test]
fn test_transaction_propagation_within_5_seconds() {
    println!("\n=== Transaction Propagation Timing Test ===\n");

    let mut network = build_timing_network();

    // Allow network to initialize
    thread::sleep(Duration::from_millis(200));

    // Create and broadcast a minting transaction
    let minting_tx = create_test_minting_tx(1, 1);
    let tx_hash = minting_tx.hash();

    println!("Broadcasting minting transaction...");
    let broadcast_time = network.broadcast_minting_tx(minting_tx);

    // Wait for propagation
    let propagated = network.wait_for_tx_in_all_nodes(&tx_hash, TX_PROPAGATION_TARGET);

    assert!(
        propagated,
        "Transaction should propagate to all nodes within {:?}",
        TX_PROPAGATION_TARGET
    );

    // Measure actual propagation times
    let propagation_times = network.measure_propagation_times(&tx_hash, broadcast_time);

    println!("Propagation times per node:");
    for (i, duration) in propagation_times.iter().enumerate() {
        println!("  Node {}: {:?}", i, duration);
    }

    let max_propagation = propagation_times.iter().max().unwrap();
    println!("Maximum propagation time: {:?}", max_propagation);

    assert!(
        *max_propagation < TX_PROPAGATION_TARGET,
        "Maximum propagation time {:?} exceeds target {:?}",
        max_propagation,
        TX_PROPAGATION_TARGET
    );

    network.stop();
    println!("\n=== Propagation Timing Test Passed ===\n");
}

/// Test: Consensus is achieved within timeout
#[test]
fn test_consensus_achieved_within_timeout() {
    println!("\n=== Consensus Timing Test ===\n");

    let mut network = build_timing_network();
    thread::sleep(Duration::from_millis(200));

    let minting_tx = create_test_minting_tx(1, 1);

    println!("Broadcasting minting transaction...");
    let start = Instant::now();
    network.broadcast_minting_tx(minting_tx);

    // Wait for all nodes to reach slot 1
    let reached = network.wait_for_slot_majority(1, NUM_NODES, CONSENSUS_TIMEOUT);

    let consensus_time = start.elapsed();
    println!("Consensus achieved in: {:?}", consensus_time);

    assert!(reached, "All nodes should reach consensus within timeout");
    assert!(
        consensus_time < CONSENSUS_TIMEOUT,
        "Consensus time {:?} exceeds timeout {:?}",
        consensus_time,
        CONSENSUS_TIMEOUT
    );

    // Verify consistency
    network.verify_consistency();

    network.stop();
    println!("\n=== Consensus Timing Test Passed ===\n");
}

/// Test: Multiple transactions propagate efficiently
#[test]
fn test_multiple_transaction_propagation() {
    println!("\n=== Multiple Transaction Propagation Test ===\n");

    let mut network = build_timing_network();
    thread::sleep(Duration::from_millis(200));

    let num_transactions = 5;
    let mut broadcast_times = Vec::new();
    let mut tx_hashes = Vec::new();

    // Broadcast multiple transactions
    for i in 1..=num_transactions {
        let minting_tx = create_test_minting_tx(i as u64, i as u8);
        let tx_hash = minting_tx.hash();
        tx_hashes.push(tx_hash);

        let broadcast_time = network.broadcast_minting_tx(minting_tx);
        broadcast_times.push(broadcast_time);

        // Small delay between broadcasts
        thread::sleep(Duration::from_millis(100));
    }

    // Verify all transactions propagated
    for (i, tx_hash) in tx_hashes.iter().enumerate() {
        let propagated = network.wait_for_tx_in_all_nodes(tx_hash, TX_PROPAGATION_TARGET);
        assert!(
            propagated,
            "Transaction {} should propagate within target time",
            i + 1
        );

        let times = network.measure_propagation_times(tx_hash, broadcast_times[i]);
        let max_time = times.iter().max().unwrap();
        println!("Transaction {} max propagation: {:?}", i + 1, max_time);
    }

    network.stop();
    println!("\n=== Multiple Transaction Propagation Test Passed ===\n");
}

/// Test: Measure consensus latency statistics
#[test]
fn test_consensus_latency_statistics() {
    println!("\n=== Consensus Latency Statistics Test ===\n");

    let mut network = build_timing_network();
    thread::sleep(Duration::from_millis(200));

    let num_rounds = 3;
    let mut latencies = Vec::new();

    for round in 1..=num_rounds {
        let minting_tx = create_test_minting_tx(round as u64, round as u8);

        let start = Instant::now();
        network.broadcast_minting_tx(minting_tx);

        let reached = network.wait_for_slot_majority(round as SlotIndex, NUM_NODES, CONSENSUS_TIMEOUT);
        assert!(reached, "Round {} should complete", round);

        let latency = start.elapsed();
        latencies.push(latency);
        println!("Round {} latency: {:?}", round, latency);

        // Clear pending for next round
        network.pending_minting_txs.lock().unwrap().clear();
    }

    // Calculate statistics
    let avg_latency: Duration = latencies.iter().sum::<Duration>() / latencies.len() as u32;
    let max_latency = latencies.iter().max().unwrap();
    let min_latency = latencies.iter().min().unwrap();

    println!("\nLatency Statistics:");
    println!("  Average: {:?}", avg_latency);
    println!("  Min: {:?}", min_latency);
    println!("  Max: {:?}", max_latency);

    // Verify all latencies are within acceptable range
    assert!(
        *max_latency < CONSENSUS_TIMEOUT,
        "Max latency should be within timeout"
    );

    network.stop();
    println!("\n=== Consensus Latency Statistics Test Passed ===\n");
}
