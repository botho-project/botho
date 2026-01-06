// Copyright (c) 2024 Botho Foundation
//
//! Test network infrastructure for e2e integration tests.
//!
//! Provides a simulated multi-node SCP consensus network with:
//! - Crossbeam channels for message passing
//! - LMDB-backed ledgers per node
//! - Configurable node count and quorum threshold

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, RwLock,
    },
    thread,
    time::Duration,
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
    block::{Block, BlockLotterySummary, MintingTx},
    ledger::{ChainState, Ledger},
    transaction::Transaction,
    wallet::Wallet,
};

use crate::common::{
    ConsensusValue, TestNodeMessage, DEFAULT_MAX_SLOT_VALUES, DEFAULT_NUM_NODES, DEFAULT_QUORUM_K,
    SCP_TIMEBASE_MS,
};

/// Configuration for building a test network
#[derive(Clone)]
pub struct TestNetworkConfig {
    /// Number of nodes in the network
    pub num_nodes: usize,
    /// Quorum threshold for SCP consensus
    pub quorum_k: usize,
    /// Maximum values per consensus slot
    pub max_slot_values: usize,
}

impl Default for TestNetworkConfig {
    fn default() -> Self {
        Self {
            num_nodes: DEFAULT_NUM_NODES,
            quorum_k: DEFAULT_QUORUM_K,
            max_slot_values: DEFAULT_MAX_SLOT_VALUES,
        }
    }
}

impl TestNetworkConfig {
    /// Create config with higher slot capacity for stress testing
    pub fn for_stress_testing() -> Self {
        Self {
            max_slot_values: 100,
            ..Default::default()
        }
    }
}

/// A test node with ledger, wallet, and SCP consensus
pub struct TestNode {
    /// Node identifier
    pub node_id: NodeID,
    /// Channel to send messages to this node's event loop
    pub sender: Sender<TestNodeMessage>,
    /// Shared ledger (LMDB-backed)
    pub ledger: Arc<RwLock<Ledger>>,
    /// Wallet for this node
    pub wallet: Arc<Wallet>,
    /// Temp directory for ledger storage (kept alive for test duration)
    pub _temp_dir: TempDir,
}

impl TestNode {
    /// Get the current chain state
    pub fn chain_state(&self) -> ChainState {
        self.ledger.read().unwrap().get_chain_state().unwrap()
    }

    /// Get the tip block
    pub fn get_tip(&self) -> Block {
        self.ledger.read().unwrap().get_tip().unwrap()
    }

    /// Send a stop message to this node
    pub fn stop(&self) {
        let _ = self.sender.send(TestNodeMessage::Stop);
    }
}

/// A simulated network of test nodes
pub struct TestNetwork {
    /// Map of node ID to test node
    pub nodes: Arc<DashMap<NodeID, TestNode>>,
    /// Thread handles for node event loops
    pub handles: Vec<thread::JoinHandle<()>>,
    /// All node IDs in the network
    pub node_ids: Vec<NodeID>,
    /// Wallets for each node (for creating transactions)
    pub wallets: Vec<Arc<Wallet>>,
    /// Pending minting transactions (shared storage for block building)
    pub pending_minting_txs: Arc<Mutex<HashMap<[u8; 32], MintingTx>>>,
    /// Pending regular transactions
    pub pending_txs: Arc<Mutex<HashMap<[u8; 32], Transaction>>>,
    /// Shutdown flag
    pub shutdown: Arc<AtomicBool>,
    /// Network configuration
    pub config: TestNetworkConfig,
}

impl TestNetwork {
    /// Build a new test network with the given configuration
    pub fn build(config: TestNetworkConfig) -> Self {
        build_test_network_with_config(config)
    }

    /// Stop all nodes and join their threads
    pub fn stop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        for entry in self.nodes.iter() {
            entry.value().stop();
        }
        // Wait a bit for nodes to process stop messages
        thread::sleep(Duration::from_millis(100));
    }

    /// Get a reference to a node by index
    pub fn get_node(&self, index: usize) -> dashmap::mapref::one::Ref<'_, NodeID, TestNode> {
        self.nodes.get(&self.node_ids[index]).unwrap()
    }

    /// Broadcast a minting transaction to all nodes
    pub fn broadcast_minting_tx(&self, minting_tx: MintingTx) {
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

    /// Broadcast a regular transaction to all nodes
    pub fn broadcast_transaction(&self, tx: Transaction) {
        let hash = tx.hash();
        self.pending_txs.lock().unwrap().insert(hash, tx.clone());

        for entry in self.nodes.iter() {
            let _ = entry
                .value()
                .sender
                .send(TestNodeMessage::Transaction(tx.clone()));
        }
    }

    /// Verify all nodes have consistent chain state
    pub fn verify_consistency(&self) {
        let first_node = self.get_node(0);
        let first_state = first_node.chain_state();

        for i in 1..self.config.num_nodes {
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

            assert_eq!(
                first_state.total_mined, state.total_mined,
                "Node {} total_mined mismatch",
                i
            );

            assert_eq!(
                first_state.total_fees_burned, state.total_fees_burned,
                "Node {} total_fees_burned mismatch",
                i
            );
        }
    }

    /// Wait for all nodes to reach the target height
    pub fn wait_for_height(&self, target_height: u64, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;

        while std::time::Instant::now() < deadline {
            let mut all_synced = true;
            for i in 0..self.config.num_nodes {
                let node = self.get_node(i);
                let state = node.chain_state();
                if state.height < target_height {
                    all_synced = false;
                    break;
                }
            }

            if all_synced {
                return true;
            }

            thread::sleep(Duration::from_millis(50));
        }

        false
    }
}

/// Build a test network with the given configuration
fn build_test_network_with_config(config: TestNetworkConfig) -> TestNetwork {
    let nodes_map: Arc<DashMap<NodeID, TestNode>> = Arc::new(DashMap::new());
    let mut handles = Vec::new();
    let mut node_ids = Vec::new();
    let mut wallets = Vec::new();

    let pending_minting_txs: Arc<Mutex<HashMap<[u8; 32], MintingTx>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let pending_txs: Arc<Mutex<HashMap<[u8; 32], Transaction>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let shutdown = Arc::new(AtomicBool::new(false));

    // Create node IDs first
    for i in 0..config.num_nodes {
        node_ids.push(test_node_id(i as u32));
    }

    // Create each node
    for i in 0..config.num_nodes {
        let node_id = node_ids[i].clone();
        let peers: HashSet<NodeID> = node_ids
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, id)| id.clone())
            .collect();

        // Generate a new wallet for this node
        let wallet = Arc::new(generate_test_wallet());
        wallets.push(wallet.clone());

        // Create temp directory for ledger
        let temp_dir = TempDir::new().unwrap();
        let ledger = Arc::new(RwLock::new(Ledger::open(temp_dir.path()).unwrap()));

        // Create channel for message passing
        let (sender, receiver) = unbounded();

        // Build quorum set
        let peer_vec: Vec<NodeID> = peers.iter().cloned().collect();
        let quorum_set = QuorumSet::new_with_node_ids(config.quorum_k as u32, peer_vec);

        // Clone references for thread
        let nodes_map_clone = nodes_map.clone();
        let peers_clone = peers.clone();
        let ledger_clone = ledger.clone();
        let pending_minting_clone = pending_minting_txs.clone();
        let pending_txs_clone = pending_txs.clone();
        let shutdown_clone = shutdown.clone();
        let node_id_clone = node_id.clone();
        let max_slot_values = config.max_slot_values;

        // Spawn node thread
        let handle = thread::Builder::new()
            .name(format!("node-{}", i))
            .spawn(move || {
                run_test_node(
                    node_id_clone,
                    quorum_set,
                    peers_clone,
                    receiver,
                    nodes_map_clone,
                    ledger_clone,
                    pending_minting_clone,
                    pending_txs_clone,
                    shutdown_clone,
                    max_slot_values,
                )
            })
            .expect("Failed to spawn node thread");

        handles.push(handle);

        // Store node in map
        let test_node = TestNode {
            node_id: node_ids[i].clone(),
            sender,
            ledger,
            wallet,
            _temp_dir: temp_dir,
        };
        nodes_map.insert(node_ids[i].clone(), test_node);
    }

    TestNetwork {
        nodes: nodes_map,
        handles,
        node_ids,
        wallets,
        pending_minting_txs,
        pending_txs,
        shutdown,
        config,
    }
}

/// Generate a random wallet for testing
fn generate_test_wallet() -> Wallet {
    use bip39::{Language, Mnemonic, MnemonicType};
    let mnemonic = Mnemonic::new(MnemonicType::Words24, Language::English);
    Wallet::from_mnemonic(mnemonic.phrase()).expect("Failed to create wallet from mnemonic")
}

/// Run the event loop for a test node
fn run_test_node(
    node_id: NodeID,
    quorum_set: QuorumSet,
    peers: HashSet<NodeID>,
    receiver: Receiver<TestNodeMessage>,
    nodes_map: Arc<DashMap<NodeID, TestNode>>,
    ledger: Arc<RwLock<Ledger>>,
    pending_minting_txs: Arc<Mutex<HashMap<[u8; 32], MintingTx>>>,
    pending_txs: Arc<Mutex<HashMap<[u8; 32], Transaction>>>,
    shutdown: Arc<AtomicBool>,
    max_slot_values: usize,
) {
    // Create validity and combine functions
    let validity_fn: ValidityFn<ConsensusValue, String> = Arc::new(|_| Ok(()));
    let combine_fn: CombineFn<ConsensusValue, String> = Arc::new(move |values| {
        let mut combined: Vec<ConsensusValue> = values.to_vec();
        combined.sort();
        combined.dedup();

        // Ensure only one minting tx (highest priority wins)
        let minting_txs: Vec<_> = combined.iter().filter(|v| v.is_minting).cloned().collect();
        let regular_txs: Vec<_> = combined.iter().filter(|v| !v.is_minting).cloned().collect();

        let mut result = Vec::new();
        if let Some(best_minting) = minting_txs.into_iter().max_by_key(|v| v.priority) {
            result.push(best_minting);
        }
        result.extend(regular_txs.into_iter().take(max_slot_values - 1));

        // IMPORTANT: SCP requires ballot values to be sorted for consensus safety
        result.sort();
        Ok(result)
    });

    // Create SCP node
    let logger = bth_consensus_scp::create_null_logger();
    let mut scp_node = ScpNodeImpl::new(
        node_id.clone(),
        quorum_set,
        validity_fn,
        combine_fn,
        1, // Start at slot 1 (slot 0 is genesis)
        logger,
    );
    scp_node.scp_timebase = Duration::from_millis(SCP_TIMEBASE_MS);

    let mut pending_values: Vec<ConsensusValue> = Vec::new();
    let mut current_slot: SlotIndex = 1;

    loop {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }

        // Non-blocking receive
        match receiver.try_recv() {
            Ok(TestNodeMessage::MintingTx(minting_tx)) => {
                let cv = ConsensusValue {
                    tx_hash: minting_tx.hash(),
                    priority: minting_tx.pow_priority(), // Higher = better PoW
                    is_minting: true,
                };
                pending_values.push(cv);
            }
            Ok(TestNodeMessage::Transaction(tx)) => {
                let cv = ConsensusValue {
                    tx_hash: tx.hash(),
                    priority: tx.created_at_height, // Simple priority
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

        // Propose pending values
        if !pending_values.is_empty() {
            let to_propose: BTreeSet<ConsensusValue> = pending_values
                .iter()
                .take(max_slot_values)
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
            // Build and apply block
            if let Err(e) =
                apply_externalized_block(&ledger, &pending_minting_txs, &pending_txs, &externalized)
            {
                eprintln!("[Node {}] Failed to apply block: {}", node_id, e);
            }

            // Remove externalized values from pending
            pending_values.retain(|v| !externalized.contains(v));
            current_slot += 1;
        }
    }
}

fn broadcast_scp_msg(
    nodes_map: &Arc<DashMap<NodeID, TestNode>>,
    peers: &HashSet<NodeID>,
    msg: Msg<ConsensusValue>,
) {
    let msg = Arc::new(msg);
    for peer_id in peers {
        if let Some(peer) = nodes_map.get(peer_id) {
            let _ = peer.sender.send(TestNodeMessage::ScpMsg(msg.clone()));
        }
    }
}

fn apply_externalized_block(
    ledger: &Arc<RwLock<Ledger>>,
    pending_minting_txs: &Arc<Mutex<HashMap<[u8; 32], MintingTx>>>,
    pending_txs: &Arc<Mutex<HashMap<[u8; 32], Transaction>>>,
    externalized: &[ConsensusValue],
) -> Result<(), String> {
    // Find the minting tx (there should be exactly one)
    let minting_cv = externalized
        .iter()
        .find(|cv| cv.is_minting)
        .ok_or("No minting tx in externalized values")?;

    let minting_tx = pending_minting_txs
        .lock()
        .unwrap()
        .get(&minting_cv.tx_hash)
        .cloned()
        .ok_or("Minting tx not found in pending")?;

    // Collect regular transactions
    let mut transactions = Vec::new();
    let pending = pending_txs.lock().unwrap();
    for cv in externalized.iter().filter(|cv| !cv.is_minting) {
        if let Some(tx) = pending.get(&cv.tx_hash) {
            transactions.push(tx.clone());
        }
    }
    drop(pending);

    // Build the block using the agreed-upon minting transaction
    let ledger_read = ledger.read().map_err(|e| e.to_string())?;
    let prev_block = ledger_read.get_tip().map_err(|e| e.to_string())?;
    drop(ledger_read);

    // Compute transaction root
    let tx_root = {
        use sha2::{Digest, Sha256};
        if transactions.is_empty() {
            [0u8; 32]
        } else {
            let mut hasher = Sha256::new();
            for tx in &transactions {
                hasher.update(tx.hash());
            }
            hasher.finalize().into()
        }
    };

    // Build the block directly from the externalized minting tx
    let block = Block {
        header: botho::block::BlockHeader {
            version: 1,
            prev_block_hash: prev_block.hash(),
            tx_root,
            timestamp: minting_tx.timestamp,
            height: minting_tx.block_height,
            difficulty: minting_tx.difficulty,
            nonce: minting_tx.nonce,
            minter_view_key: minting_tx.minter_view_key,
            minter_spend_key: minting_tx.minter_spend_key,
        },
        minting_tx,
        transactions,
        lottery_outputs: Vec::new(),
        lottery_summary: BlockLotterySummary::default(),
    };

    // Add block to ledger
    let ledger_write = ledger.read().map_err(|e| e.to_string())?;
    ledger_write.add_block(&block).map_err(|e| e.to_string())?;

    Ok(())
}
