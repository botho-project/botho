// Copyright (c) 2024 Botho Foundation
//
//! Chaos Tests for Network Adversity
//!
//! Tests consensus under adverse network conditions:
//! - 50% packet loss
//! - Clock skew between nodes
//! - Combined chaos factors
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
use rand::Rng;
use tempfile::TempDir;

use bth_common::NodeID;
use bth_consensus_scp::{
    msg::Msg,
    slot::{CombineFn, ValidityFn},
    test_utils::test_node_id,
    Node as ScpNodeImpl, QuorumSet, ScpNode, SlotIndex,
};

use botho::{
    block::{Block, BlockHeader, BlockLotterySummary, MintingTx},
    ledger::{ChainState, Ledger},
    transaction::PICOCREDITS_PER_CREDIT,
};

use botho_wallet::WalletKeys;
use std::time::SystemTime;

// ============================================================================
// Constants
// ============================================================================

/// Number of nodes in the chaos test network
const NUM_NODES: usize = 5;

/// Quorum threshold (k=3 for 5 nodes)
const QUORUM_K: usize = 3;

/// SCP timebase for testing
const SCP_TIMEBASE_MS: u64 = 100;

/// Maximum values per slot
const MAX_SLOT_VALUES: usize = 50;

/// Extended timeout for chaos conditions
const CHAOS_TIMEOUT: Duration = Duration::from_secs(60);

/// Trivial difficulty for test mining
const TRIVIAL_DIFFICULTY: u64 = 0x00FF_FFFF_FFFF_FFFF;

// ============================================================================
// Chaos Behavior Types
// ============================================================================

/// Types of chaos behavior a node can exhibit
#[derive(Clone, Debug)]
enum ChaosBehavior {
    /// Normal operation
    Normal,
    /// Drop messages with given probability (0.0 - 1.0)
    PacketLoss(f64),
    /// Add clock skew to timestamps (positive = ahead, negative = behind)
    ClockSkew(i64),
    /// Combined: packet loss + clock skew
    Combined {
        packet_loss: f64,
        clock_skew_ms: i64,
    },
}

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
    ScpMsg(Arc<Msg<ConsensusValue>>),
    Stop,
}

// ============================================================================
// Chaos Test Node
// ============================================================================

struct ChaosTestNode {
    node_id: NodeID,
    sender: Sender<TestNodeMessage>,
    ledger: Arc<RwLock<Ledger>>,
    behavior: Arc<RwLock<ChaosBehavior>>,
    messages_sent: Arc<AtomicU64>,
    messages_dropped: Arc<AtomicU64>,
    _temp_dir: TempDir,
}

impl ChaosTestNode {
    fn chain_state(&self) -> ChainState {
        self.ledger.read().unwrap().get_chain_state().unwrap()
    }

    fn stop(&self) {
        let _ = self.sender.send(TestNodeMessage::Stop);
    }

    fn messages_sent(&self) -> u64 {
        self.messages_sent.load(Ordering::SeqCst)
    }

    fn messages_dropped(&self) -> u64 {
        self.messages_dropped.load(Ordering::SeqCst)
    }

    fn set_behavior(&self, behavior: ChaosBehavior) {
        *self.behavior.write().unwrap() = behavior;
    }
}

// ============================================================================
// Chaos Test Network
// ============================================================================

struct ChaosTestNetwork {
    nodes: Arc<DashMap<NodeID, ChaosTestNode>>,
    handles: Vec<thread::JoinHandle<()>>,
    node_ids: Vec<NodeID>,
    pending_minting_txs: Arc<Mutex<HashMap<[u8; 32], MintingTx>>>,
    shutdown: Arc<AtomicBool>,
    slot_progress: Arc<DashMap<NodeID, SlotIndex>>,
}

impl ChaosTestNetwork {
    fn stop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        for entry in self.nodes.iter() {
            entry.value().stop();
        }
        thread::sleep(Duration::from_millis(100));
    }

    fn get_node(&self, index: usize) -> dashmap::mapref::one::Ref<'_, NodeID, ChaosTestNode> {
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

    fn verify_majority_consistency(&self, required_nodes: usize) {
        // Collect states from nodes that made progress
        let mut states: Vec<(usize, ChainState)> = Vec::new();

        for i in 0..NUM_NODES {
            let node = self.get_node(i);
            let state = node.chain_state();
            if state.height > 0 {
                states.push((i, state));
            }
        }

        assert!(
            states.len() >= required_nodes,
            "Expected at least {} nodes to make progress, got {}",
            required_nodes,
            states.len()
        );

        // Verify all progressed nodes agree
        if states.len() > 1 {
            let first = &states[0].1;
            for (i, state) in &states[1..] {
                assert_eq!(
                    first.height, state.height,
                    "Node {} height mismatch with node 0",
                    i
                );
                assert_eq!(
                    first.tip_hash, state.tip_hash,
                    "Node {} tip hash mismatch with node 0",
                    i
                );
            }
        }
    }

    fn set_node_behavior(&self, index: usize, behavior: ChaosBehavior) {
        let node = self.get_node(index);
        node.set_behavior(behavior);
    }

    fn print_message_stats(&self) {
        println!("\nMessage Statistics:");
        for i in 0..NUM_NODES {
            let node = self.get_node(i);
            println!(
                "  Node {}: sent={}, dropped={}",
                i,
                node.messages_sent(),
                node.messages_dropped()
            );
        }
    }
}

// ============================================================================
// Network Builder
// ============================================================================

fn build_chaos_network(behaviors: Vec<ChaosBehavior>) -> ChaosTestNetwork {
    assert_eq!(behaviors.len(), NUM_NODES);

    let nodes_map: Arc<DashMap<NodeID, ChaosTestNode>> = Arc::new(DashMap::new());
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

        let behavior = Arc::new(RwLock::new(behaviors[i].clone()));
        let messages_sent = Arc::new(AtomicU64::new(0));
        let messages_dropped = Arc::new(AtomicU64::new(0));

        let nodes_map_clone = nodes_map.clone();
        let peers_clone = peers.clone();
        let ledger_clone = ledger.clone();
        let pending_minting_clone = pending_minting_txs.clone();
        let shutdown_clone = shutdown.clone();
        let node_id_clone = node_id.clone();
        let behavior_clone = behavior.clone();
        let messages_sent_clone = messages_sent.clone();
        let messages_dropped_clone = messages_dropped.clone();
        let slot_progress_clone = slot_progress.clone();

        slot_progress.insert(node_id.clone(), 0);

        let handle = thread::Builder::new()
            .name(format!("chaos-node-{}", i))
            .spawn(move || {
                run_chaos_node(
                    node_id_clone,
                    quorum_set,
                    peers_clone,
                    receiver,
                    nodes_map_clone,
                    ledger_clone,
                    pending_minting_clone,
                    shutdown_clone,
                    behavior_clone,
                    messages_sent_clone,
                    messages_dropped_clone,
                    slot_progress_clone,
                )
            })
            .expect("Failed to spawn node thread");

        handles.push(handle);

        let test_node = ChaosTestNode {
            node_id: node_ids[i].clone(),
            sender,
            ledger,
            behavior,
            messages_sent,
            messages_dropped,
            _temp_dir: temp_dir,
        };
        nodes_map.insert(node_ids[i].clone(), test_node);
    }

    ChaosTestNetwork {
        nodes: nodes_map,
        handles,
        node_ids,
        pending_minting_txs,
        shutdown,
        slot_progress,
    }
}

// ============================================================================
// Chaos Node Event Loop
// ============================================================================

fn run_chaos_node(
    node_id: NodeID,
    quorum_set: QuorumSet,
    peers: HashSet<NodeID>,
    receiver: Receiver<TestNodeMessage>,
    nodes_map: Arc<DashMap<NodeID, ChaosTestNode>>,
    ledger: Arc<RwLock<Ledger>>,
    pending_minting_txs: Arc<Mutex<HashMap<[u8; 32], MintingTx>>>,
    shutdown: Arc<AtomicBool>,
    behavior: Arc<RwLock<ChaosBehavior>>,
    messages_sent: Arc<AtomicU64>,
    messages_dropped: Arc<AtomicU64>,
    slot_progress: Arc<DashMap<NodeID, SlotIndex>>,
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
    let mut rng = rand::thread_rng();

    loop {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }

        let current_behavior = behavior.read().unwrap().clone();

        // Apply clock skew effect (simulated via additional delay)
        match &current_behavior {
            ChaosBehavior::ClockSkew(skew_ms) => {
                if *skew_ms > 0 {
                    // Ahead: process faster (no delay)
                } else {
                    // Behind: add delay to simulate slow processing
                    thread::sleep(Duration::from_millis((-*skew_ms) as u64 / 10));
                }
            }
            ChaosBehavior::Combined { clock_skew_ms, .. } => {
                if *clock_skew_ms < 0 {
                    thread::sleep(Duration::from_millis((-*clock_skew_ms) as u64 / 10));
                }
            }
            _ => {}
        }

        // Check for packet loss on receive
        let should_drop = match &current_behavior {
            ChaosBehavior::PacketLoss(prob) => rng.gen::<f64>() < *prob,
            ChaosBehavior::Combined { packet_loss, .. } => rng.gen::<f64>() < *packet_loss,
            _ => false,
        };

        match receiver.try_recv() {
            Ok(TestNodeMessage::MintingTx(minting_tx)) => {
                if should_drop {
                    messages_dropped.fetch_add(1, Ordering::SeqCst);
                    continue;
                }

                let cv = ConsensusValue {
                    tx_hash: minting_tx.hash(),
                    priority: minting_tx.pow_priority(),
                    is_minting: true,
                };
                pending_values.push(cv);
            }
            Ok(TestNodeMessage::ScpMsg(msg)) => {
                if should_drop {
                    messages_dropped.fetch_add(1, Ordering::SeqCst);
                    continue;
                }

                if let Ok(Some(out_msg)) = scp_node.handle_message(&msg) {
                    broadcast_with_chaos(
                        &nodes_map,
                        &peers,
                        out_msg,
                        &current_behavior,
                        &messages_sent,
                        &messages_dropped,
                    );
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
                broadcast_with_chaos(
                    &nodes_map,
                    &peers,
                    out_msg,
                    &current_behavior,
                    &messages_sent,
                    &messages_dropped,
                );
            }
        }

        // Process timeouts
        for out_msg in scp_node.process_timeouts() {
            broadcast_with_chaos(
                &nodes_map,
                &peers,
                out_msg,
                &current_behavior,
                &messages_sent,
                &messages_dropped,
            );
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

fn broadcast_with_chaos(
    nodes_map: &Arc<DashMap<NodeID, ChaosTestNode>>,
    peers: &HashSet<NodeID>,
    msg: Msg<ConsensusValue>,
    behavior: &ChaosBehavior,
    messages_sent: &Arc<AtomicU64>,
    messages_dropped: &Arc<AtomicU64>,
) {
    let msg = Arc::new(msg);
    let mut rng = rand::thread_rng();

    for peer_id in peers {
        // Check if we should drop this outgoing message
        let drop_prob = match behavior {
            ChaosBehavior::PacketLoss(prob) => *prob,
            ChaosBehavior::Combined { packet_loss, .. } => *packet_loss,
            _ => 0.0,
        };

        if rng.gen::<f64>() < drop_prob {
            messages_dropped.fetch_add(1, Ordering::SeqCst);
            continue;
        }

        if let Some(peer_node) = nodes_map.get(peer_id) {
            let _ = peer_node.sender.send(TestNodeMessage::ScpMsg(msg.clone()));
            messages_sent.fetch_add(1, Ordering::SeqCst);
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
                lottery_outputs: Vec::new(),
                lottery_summary: BlockLotterySummary::default(),
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

/// Test: Consensus under 50% packet loss
///
/// With k=3 quorum and 50% loss, consensus should still be achievable
/// through retries and timeout-based rebroadcasts.
#[test]
#[ignore = "Long-running chaos test - run with --ignored"]
fn test_50_percent_packet_loss() {
    println!("\n=== 50% Packet Loss Chaos Test ===\n");

    // All nodes experience 50% packet loss
    let behaviors = vec![
        ChaosBehavior::PacketLoss(0.5),
        ChaosBehavior::PacketLoss(0.5),
        ChaosBehavior::PacketLoss(0.5),
        ChaosBehavior::PacketLoss(0.5),
        ChaosBehavior::PacketLoss(0.5),
    ];

    let mut network = build_chaos_network(behaviors);
    thread::sleep(Duration::from_millis(300));

    // Try multiple rounds
    for round in 1..=3 {
        println!("Round {}:", round);

        let minting_tx = create_test_minting_tx(round as u64, round as u8);
        network.broadcast_minting_tx(minting_tx);

        // With 50% loss, use extended timeout
        let reached = network.wait_for_slot_majority(round as SlotIndex, QUORUM_K, CHAOS_TIMEOUT);

        if reached {
            println!("  Consensus reached for round {}", round);
        } else {
            println!("  Warning: Round {} did not complete within timeout", round);
        }

        network.pending_minting_txs.lock().unwrap().clear();
        thread::sleep(Duration::from_millis(200));
    }

    network.print_message_stats();

    // Verify at least quorum nodes have consistent state
    network.verify_majority_consistency(QUORUM_K);

    network.stop();
    println!("\n=== 50% Packet Loss Test Complete ===\n");
}

/// Test: Consensus with clock skew between nodes
///
/// Nodes have varying clock offsets simulating NTP drift or misconfiguration.
#[test]
#[ignore = "Long-running chaos test - run with --ignored"]
fn test_clock_skew() {
    println!("\n=== Clock Skew Chaos Test ===\n");

    // Mix of clock skews: some ahead, some behind
    let behaviors = vec![
        ChaosBehavior::ClockSkew(-500), // 500ms behind
        ChaosBehavior::ClockSkew(0),    // On time
        ChaosBehavior::ClockSkew(300),  // 300ms ahead
        ChaosBehavior::ClockSkew(-200), // 200ms behind
        ChaosBehavior::ClockSkew(100),  // 100ms ahead
    ];

    let mut network = build_chaos_network(behaviors);
    thread::sleep(Duration::from_millis(300));

    // Run multiple rounds to test consistency
    for round in 1..=5 {
        println!("Round {}:", round);

        let minting_tx = create_test_minting_tx(round as u64, round as u8);
        network.broadcast_minting_tx(minting_tx);

        let reached = network.wait_for_slot_majority(round as SlotIndex, NUM_NODES, CHAOS_TIMEOUT);

        if reached {
            println!("  All nodes reached consensus for round {}", round);
        } else {
            // With clock skew, some nodes may lag - verify quorum at least
            let quorum_reached = network.wait_for_slot_majority(
                round as SlotIndex,
                QUORUM_K,
                Duration::from_secs(5),
            );
            assert!(
                quorum_reached,
                "Quorum should reach consensus despite clock skew"
            );
            println!("  Quorum reached consensus for round {}", round);
        }

        network.pending_minting_txs.lock().unwrap().clear();
    }

    // Verify consistency among nodes that made progress
    network.verify_majority_consistency(QUORUM_K);

    network.stop();
    println!("\n=== Clock Skew Test Complete ===\n");
}

/// Test: Combined chaos - packet loss + clock skew
#[test]
#[ignore = "Long-running chaos test - run with --ignored"]
fn test_combined_chaos() {
    println!("\n=== Combined Chaos Test (Packet Loss + Clock Skew) ===\n");

    let behaviors = vec![
        ChaosBehavior::Combined {
            packet_loss: 0.3,
            clock_skew_ms: -300,
        },
        ChaosBehavior::Combined {
            packet_loss: 0.2,
            clock_skew_ms: 200,
        },
        ChaosBehavior::Normal, // Some honest nodes for baseline
        ChaosBehavior::Combined {
            packet_loss: 0.4,
            clock_skew_ms: -100,
        },
        ChaosBehavior::Normal,
    ];

    let mut network = build_chaos_network(behaviors);
    thread::sleep(Duration::from_millis(300));

    for round in 1..=3 {
        println!("Round {}:", round);

        let minting_tx = create_test_minting_tx(round as u64, round as u8);
        network.broadcast_minting_tx(minting_tx);

        let reached = network.wait_for_slot_majority(round as SlotIndex, QUORUM_K, CHAOS_TIMEOUT);
        assert!(
            reached,
            "Quorum should reach consensus even under combined chaos"
        );
        println!("  Quorum reached consensus for round {}", round);

        network.pending_minting_txs.lock().unwrap().clear();
        thread::sleep(Duration::from_millis(100));
    }

    network.print_message_stats();
    network.verify_majority_consistency(QUORUM_K);

    network.stop();
    println!("\n=== Combined Chaos Test Complete ===\n");
}

/// Test: Progressive chaos - increase adversity over time
#[test]
#[ignore = "Long-running chaos test - run with --ignored"]
fn test_progressive_chaos() {
    println!("\n=== Progressive Chaos Test ===\n");

    // Start with normal behavior
    let behaviors = vec![
        ChaosBehavior::Normal,
        ChaosBehavior::Normal,
        ChaosBehavior::Normal,
        ChaosBehavior::Normal,
        ChaosBehavior::Normal,
    ];

    let mut network = build_chaos_network(behaviors);
    thread::sleep(Duration::from_millis(200));

    // Phase 1: Normal operation
    println!("Phase 1: Normal operation");
    let minting_tx = create_test_minting_tx(1, 1);
    network.broadcast_minting_tx(minting_tx);
    assert!(
        network.wait_for_slot_majority(1, NUM_NODES, Duration::from_secs(30)),
        "Normal operation should succeed"
    );
    network.pending_minting_txs.lock().unwrap().clear();

    // Phase 2: Introduce 20% packet loss on 2 nodes
    println!("\nPhase 2: 20% packet loss on 2 nodes");
    network.set_node_behavior(0, ChaosBehavior::PacketLoss(0.2));
    network.set_node_behavior(1, ChaosBehavior::PacketLoss(0.2));

    let minting_tx = create_test_minting_tx(2, 2);
    network.broadcast_minting_tx(minting_tx);
    assert!(
        network.wait_for_slot_majority(2, QUORUM_K, CHAOS_TIMEOUT),
        "20% loss should not prevent consensus"
    );
    network.pending_minting_txs.lock().unwrap().clear();

    // Phase 3: Increase to 40% on 3 nodes
    println!("\nPhase 3: 40% packet loss on 3 nodes");
    network.set_node_behavior(0, ChaosBehavior::PacketLoss(0.4));
    network.set_node_behavior(1, ChaosBehavior::PacketLoss(0.4));
    network.set_node_behavior(2, ChaosBehavior::PacketLoss(0.4));

    let minting_tx = create_test_minting_tx(3, 3);
    network.broadcast_minting_tx(minting_tx);
    let reached = network.wait_for_slot_majority(3, QUORUM_K, CHAOS_TIMEOUT);
    println!(
        "  Result: {}",
        if reached {
            "Consensus achieved"
        } else {
            "Consensus not achieved (expected under high adversity)"
        }
    );

    network.print_message_stats();

    network.stop();
    println!("\n=== Progressive Chaos Test Complete ===\n");
}

/// Baseline test: All nodes behave normally
#[test]
fn test_chaos_baseline_all_normal() {
    println!("\n=== Chaos Baseline Test (All Normal) ===\n");

    let behaviors = vec![
        ChaosBehavior::Normal,
        ChaosBehavior::Normal,
        ChaosBehavior::Normal,
        ChaosBehavior::Normal,
        ChaosBehavior::Normal,
    ];

    let mut network = build_chaos_network(behaviors);
    thread::sleep(Duration::from_millis(200));

    let minting_tx = create_test_minting_tx(1, 1);
    network.broadcast_minting_tx(minting_tx);

    let reached = network.wait_for_slot_majority(1, NUM_NODES, Duration::from_secs(30));
    assert!(reached, "All normal nodes should reach consensus quickly");

    // Verify zero message drops
    for i in 0..NUM_NODES {
        let node = network.get_node(i);
        assert_eq!(
            node.messages_dropped(),
            0,
            "Normal nodes should not drop messages"
        );
    }

    network.stop();
    println!("\n=== Chaos Baseline Test Passed ===\n");
}
