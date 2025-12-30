// Copyright (c) 2024 Botho Foundation
//
//! End-to-End Integration Test: 5-Node SCP Consensus with Mining and Transactions
//!
//! This test verifies the complete blockchain flow:
//! 1. Start 5 nodes in SCP consensus (mesh topology)
//! 2. Mine blocks to generate coins
//! 3. Execute visible (Schnorr) and quantum-private (ML-DSA-65) transactions
//! 4. Verify final ledger state including fees burned
//!
//! The test uses a simulated network with crossbeam channels for message passing,
//! following the pattern from `scp_sim.rs`. Each node has its own LMDB-backed ledger.
//!
//! NOTE: Transaction tests are currently ignored because they use the removed Simple
//! transaction type. They need to be rewritten to use CLSAG ring signatures
//! with proper decoy selection from the UTXO set.

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
    block::{Block, MintingTx},
    ledger::{ChainState, Ledger},
    transaction::{Transaction, TxInput, TxInputs, TxOutput, Utxo, UtxoId, MIN_TX_FEE, PICOCREDITS_PER_CREDIT},
};

use bth_account_keys::PublicAddress;
use bth_crypto_keys::RistrettoSignature;
use botho_wallet::WalletKeys;

// ============================================================================
// Constants
// ============================================================================

/// Number of nodes in the test network
const NUM_NODES: usize = 5;

/// Quorum threshold (k=3 for 5 nodes is BFT optimal: 2f+1 where f=1)
const QUORUM_K: usize = 3;

/// Initial block reward (50 BTH in picocredits)
const INITIAL_BLOCK_REWARD: u64 = 50 * PICOCREDITS_PER_CREDIT;

/// SCP timebase for testing (faster than production)
const SCP_TIMEBASE_MS: u64 = 100;

/// Maximum values per slot
const MAX_SLOT_VALUES: usize = 50;

// ============================================================================
// Consensus Value Type
// ============================================================================

/// A value to be agreed upon by consensus.
/// Wraps transaction hashes with priority for ordering.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
struct ConsensusValue {
    /// Hash of the transaction or minting tx
    pub tx_hash: [u8; 32],
    /// Priority (PoW difficulty for minting, timestamp for regular tx)
    pub priority: u64,
    /// Whether this is a minting transaction
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
            "CV({}...{}, p={}, m={})",
            hex::encode(&self.tx_hash[0..4]),
            hex::encode(&self.tx_hash[28..32]),
            self.priority,
            self.is_minting
        )
    }
}

// ============================================================================
// Message Types
// ============================================================================

/// Messages passed between test nodes
enum TestNodeMessage {
    /// A minting transaction (coinbase) to propose
    MintingTx(MintingTx),
    /// A regular transaction to propose
    Transaction(Transaction),
    /// SCP consensus message from a peer
    ScpMsg(Arc<Msg<ConsensusValue>>),
    /// Signal to stop the node
    Stop,
}

// ============================================================================
// Test Node
// ============================================================================

/// A test node with ledger, wallet, and SCP consensus
struct TestNode {
    /// Node identifier
    node_id: NodeID,
    /// Channel to send messages to this node's event loop
    sender: Sender<TestNodeMessage>,
    /// Shared ledger (LMDB-backed)
    ledger: Arc<RwLock<Ledger>>,
    /// Wallet keys for this miner
    wallet: Arc<WalletKeys>,
    /// Temp directory for ledger storage (kept alive for test duration)
    _temp_dir: TempDir,
}

impl TestNode {
    /// Get the current chain state
    fn chain_state(&self) -> ChainState {
        self.ledger.read().unwrap().get_chain_state().unwrap()
    }

    /// Get the tip block
    fn get_tip(&self) -> Block {
        self.ledger.read().unwrap().get_tip().unwrap()
    }

    /// Send a stop message to this node
    fn stop(&self) {
        let _ = self.sender.send(TestNodeMessage::Stop);
    }
}

// ============================================================================
// Test Network
// ============================================================================

/// A simulated network of test nodes
struct TestNetwork {
    /// Map of node ID to test node
    nodes: Arc<DashMap<NodeID, TestNode>>,
    /// Thread handles for node event loops
    handles: Vec<thread::JoinHandle<()>>,
    /// All node IDs in the network
    node_ids: Vec<NodeID>,
    /// Wallets for each node (for creating transactions)
    wallets: Vec<Arc<WalletKeys>>,
    /// Pending minting transactions (shared storage for block building)
    pending_minting_txs: Arc<Mutex<HashMap<[u8; 32], MintingTx>>>,
    /// Pending regular transactions
    pending_txs: Arc<Mutex<HashMap<[u8; 32], Transaction>>>,
    /// Shutdown flag
    shutdown: Arc<AtomicBool>,
}

impl TestNetwork {
    /// Stop all nodes and join their threads
    fn stop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        for entry in self.nodes.iter() {
            entry.value().stop();
        }
        // Wait a bit for nodes to process stop messages
        thread::sleep(Duration::from_millis(100));
    }

    /// Get a reference to a node by index
    fn get_node(&self, index: usize) -> dashmap::mapref::one::Ref<'_, NodeID, TestNode> {
        self.nodes.get(&self.node_ids[index]).unwrap()
    }

    /// Broadcast a minting tx to all nodes
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

    /// Broadcast a transaction to all nodes
    fn broadcast_transaction(&self, tx: Transaction) {
        let hash = tx.hash();
        self.pending_txs.lock().unwrap().insert(hash, tx.clone());

        for entry in self.nodes.iter() {
            let _ = entry
                .value()
                .sender
                .send(TestNodeMessage::Transaction(tx.clone()));
        }
    }

    /// Verify all nodes have the same ledger state
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

    /// Wait for all nodes to reach a specific height
    fn wait_for_height(&self, target_height: u64, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;

        while Instant::now() < deadline {
            let mut all_synced = true;
            for i in 0..NUM_NODES {
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

// ============================================================================
// Network Builder
// ============================================================================

/// Build a test network with the specified number of nodes
fn build_test_network() -> TestNetwork {
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

        // Generate a new wallet for this node
        let wallet = Arc::new(WalletKeys::generate().expect("Failed to generate wallet"));
        wallets.push(wallet.clone());

        // Create temp directory for ledger
        let temp_dir = TempDir::new().unwrap();
        let ledger = Arc::new(RwLock::new(Ledger::open(temp_dir.path()).unwrap()));

        // Create channel for message passing
        let (sender, receiver) = unbounded();

        // Build quorum set
        let peer_vec: Vec<NodeID> = peers.iter().cloned().collect();
        let quorum_set = QuorumSet::new_with_node_ids(QUORUM_K as u32, peer_vec);

        // Clone references for thread
        let nodes_map_clone = nodes_map.clone();
        let peers_clone = peers.clone();
        let ledger_clone = ledger.clone();
        let pending_minting_clone = pending_minting_txs.clone();
        let pending_txs_clone = pending_txs.clone();
        let shutdown_clone = shutdown.clone();
        let node_id_clone = node_id.clone();

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
    }
}

// ============================================================================
// Node Event Loop
// ============================================================================

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
        result.extend(regular_txs.into_iter().take(MAX_SLOT_VALUES - 1));

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
            let to_propose: BTreeSet<ConsensusValue> =
                pending_values.iter().take(MAX_SLOT_VALUES).cloned().collect();

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
            if let Err(e) = apply_externalized_block(
                &ledger,
                &pending_minting_txs,
                &pending_txs,
                &externalized,
            ) {
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

// ============================================================================
// Block Building and Application
// ============================================================================

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
    };

    // Add block to ledger
    let ledger_write = ledger.read().map_err(|e| e.to_string())?;
    ledger_write.add_block(&block).map_err(|e| e.to_string())?;

    Ok(())
}

// ============================================================================
// Mining Helper
// ============================================================================

/// Create a minting transaction with mock PoW (trivial difficulty for fast testing)
fn create_mock_minting_tx(
    height: u64,
    reward: u64,
    minter_address: &PublicAddress,
    prev_block_hash: [u8; 32],
) -> MintingTx {
    // Create minting tx with very high difficulty threshold (easy to satisfy)
    let mut minting_tx = MintingTx::new(
        height,
        reward,
        minter_address,
        prev_block_hash,
        u64::MAX - 1, // Trivial difficulty
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    );

    // Find a valid nonce (should be instant with trivial difficulty)
    for nonce in 0..1000 {
        minting_tx.nonce = nonce;
        if minting_tx.verify_pow() {
            break;
        }
    }

    minting_tx
}

// ============================================================================
// UTXO Scanning
// ============================================================================

/// Scan all nodes' ledgers for UTXOs belonging to a wallet.
/// Returns a list of UTXOs that this wallet can spend.
fn scan_wallet_utxos(
    network: &TestNetwork,
    wallet: &WalletKeys,
) -> Vec<(Utxo, u64)> {
    // Vec of (Utxo, subaddress_index)
    let mut owned_utxos = Vec::new();

    // Use node 0's ledger for scanning
    let node = network.get_node(0);
    let ledger = node.ledger.read().unwrap();
    let state = ledger.get_chain_state().unwrap();

    // Scan all blocks for outputs belonging to this wallet
    for height in 0..=state.height {
        if let Ok(block) = ledger.get_block(height) {
            // Check coinbase output (minting tx creates output at index 0 with block hash)
            let coinbase_output = block.minting_tx.to_tx_output();
            if let Some(subaddr_idx) = coinbase_output.belongs_to(wallet.account_key()) {
                let block_hash = block.hash();
                let utxo_id = UtxoId::new(block_hash, 0);
                // Check if UTXO still exists (hasn't been spent)
                if let Ok(Some(utxo)) = ledger.get_utxo(&utxo_id) {
                    owned_utxos.push((utxo, subaddr_idx));
                }
            }

            // Check transaction outputs
            for tx in &block.transactions {
                let tx_hash = tx.hash();
                for (idx, output) in tx.outputs.iter().enumerate() {
                    if let Some(subaddr_idx) = output.belongs_to(wallet.account_key()) {
                        let utxo_id = UtxoId::new(tx_hash, idx as u32);
                        // Check if UTXO still exists
                        if let Ok(Some(utxo)) = ledger.get_utxo(&utxo_id) {
                            owned_utxos.push((utxo, subaddr_idx));
                        }
                    }
                }
            }
        }
    }

    owned_utxos
}

/// Get the total balance of a wallet by scanning for unspent UTXOs
fn get_wallet_balance(network: &TestNetwork, wallet: &WalletKeys) -> u64 {
    scan_wallet_utxos(network, wallet)
        .iter()
        .map(|(utxo, _)| utxo.output.amount)
        .sum()
}

// ============================================================================
// Transaction Creation with Proper Signing
// ============================================================================

/// Mine a block that includes any pending transactions.
fn mine_block_with_txs(network: &TestNetwork, block_index: usize) {
    let miner_idx = block_index % NUM_NODES;
    let miner_wallet = &network.wallets[miner_idx];
    let miner_address = miner_wallet.public_address();

    let node = network.get_node(0);
    let state = node.chain_state();
    let prev_block = node.get_tip();
    let prev_hash = prev_block.hash();
    let height = state.height + 1;
    drop(node);

    // Clear pending minting txs and broadcast new one
    network.pending_minting_txs.lock().unwrap().clear();
    let minting_tx = create_mock_minting_tx(height, INITIAL_BLOCK_REWARD, &miner_address, prev_hash);
    network.broadcast_minting_tx(minting_tx);

    if !network.wait_for_height(height, Duration::from_secs(30)) {
        panic!("Timeout waiting for block {}", height);
    }

    thread::sleep(Duration::from_millis(150));
}

/// Create a properly signed visible transaction.
/// This uses stealth address key recovery to sign with the correct one-time private key.
/// TODO: This function needs to be updated to use CLSAG ring signatures
/// instead of Simple transactions. The Simple variant has been removed.
#[allow(dead_code)]
fn create_signed_transaction(
    _sender_wallet: &WalletKeys,
    _sender_utxo: &Utxo,
    _subaddress_index: u64,
    _recipient: &PublicAddress,
    _amount: u64,
    _fee: u64,
    _current_height: u64,
) -> Result<Transaction, String> {
    todo!("Update to use CLSAG ring signatures instead of Simple transactions")
}

// ============================================================================
// Main Test
// ============================================================================

// TODO: Update to use CLSAG ring signatures instead of Simple transactions
// The create_signed_transfer function needs to be rewritten to:
// 1. Select decoy outputs from the UTXO set
// 2. Create CLSAG ring inputs with the real input hidden among decoys
// 3. Sign with CLSAG instead of direct Schnorr signatures
#[test]
#[ignore = "Needs update for ring signature transactions (Simple tx removed)"]
fn test_e2e_5_node_consensus_with_mining_and_transactions() {
    println!("\n=== E2E Consensus Integration Test ===\n");

    // Phase 0: Build the test network
    println!("Phase 0: Building test network with {} nodes...", NUM_NODES);
    let mut network = build_test_network();

    // Give nodes time to initialize
    thread::sleep(Duration::from_millis(500));

    // Phase 1: Mine initial blocks to generate coins
    println!("\nPhase 1: Mining initial blocks...");
    let blocks_to_mine = 5; // Reduced for faster testing

    for i in 0..blocks_to_mine {
        let miner_idx = i % NUM_NODES;
        let miner_wallet = &network.wallets[miner_idx];
        let miner_address = miner_wallet.public_address();

        // Get current chain state from any node - ensure all nodes are synced first
        let node = network.get_node(0);
        let state = node.chain_state();
        let prev_block = node.get_tip();
        let prev_hash = prev_block.hash();
        let height = state.height + 1;
        drop(node);

        let reward = INITIAL_BLOCK_REWARD; // Simplified: use constant reward

        // Create and broadcast minting tx
        let minting_tx = create_mock_minting_tx(height, reward, &miner_address, prev_hash);
        println!(
            "  Mining block {} (miner: node {})...",
            height, miner_idx
        );

        // Clear pending minting txs before broadcasting new one
        network.pending_minting_txs.lock().unwrap().clear();
        network.broadcast_minting_tx(minting_tx);

        // Wait for block to be applied by all nodes
        if !network.wait_for_height(height, Duration::from_secs(30)) {
            panic!("Timeout waiting for block {} to be mined", height);
        }

        // Small delay to let state settle
        thread::sleep(Duration::from_millis(100));
    }

    // Verify consistency after mining
    println!("\nVerifying ledger consistency after mining...");
    network.verify_consistency();

    let node = network.get_node(0);
    let state = node.chain_state();
    println!("  Height: {}", state.height);
    println!("  Total mined: {} picocredits", state.total_mined);
    println!("  Total fees burned: {} picocredits", state.total_fees_burned);
    drop(node);

    assert_eq!(
        state.height,
        blocks_to_mine as u64,
        "Expected {} blocks mined",
        blocks_to_mine
    );
    assert_eq!(
        state.total_mined,
        blocks_to_mine as u64 * INITIAL_BLOCK_REWARD,
        "Total mined should be {} * reward",
        blocks_to_mine
    );
    assert_eq!(
        state.total_fees_burned, 0,
        "No fees should be burned yet (no transactions)"
    );

    println!("\nPhase 1 complete: {} blocks mined successfully!", blocks_to_mine);

    // Phase 2: Create and execute multiple transactions
    println!("\nPhase 2: Creating and executing transactions...");

    // Track total fees for verification
    let mut total_fees_expected: u64 = 0;

    // First, check wallet balances from mining
    println!("  Scanning wallet balances after mining...");
    let mut initial_balances = Vec::new();
    for (i, wallet) in network.wallets.iter().enumerate() {
        let balance = get_wallet_balance(&network, wallet);
        initial_balances.push(balance);
        println!("    Wallet {}: {} picocredits ({} BTH)", i, balance, balance / PICOCREDITS_PER_CREDIT);
    }

    // ========================================================================
    // Transaction 1: Wallet 0 -> Wallet 1 (simple transfer)
    // ========================================================================
    println!("\n  --- Transaction 1: Wallet 0 -> Wallet 1 ---");

    let sender_wallet = &network.wallets[0];
    let recipient_wallet = &network.wallets[1];
    let recipient_address = recipient_wallet.public_address();

    let sender_utxos = scan_wallet_utxos(&network, sender_wallet);
    assert!(!sender_utxos.is_empty(), "Sender wallet 0 has no UTXOs");

    let (utxo_to_spend, subaddr_idx) = &sender_utxos[0];
    let utxo_amount = utxo_to_spend.output.amount;
    let send_amount = 10 * PICOCREDITS_PER_CREDIT; // Send 10 BTH
    let tx_fee = MIN_TX_FEE;
    total_fees_expected += tx_fee;

    let node = network.get_node(0);
    let current_height = node.chain_state().height;
    drop(node);

    println!("    Sending {} BTH with {} fee", send_amount / PICOCREDITS_PER_CREDIT, tx_fee);

    let tx1 = create_signed_transaction(
        sender_wallet,
        utxo_to_spend,
        *subaddr_idx,
        &recipient_address,
        send_amount,
        tx_fee,
        current_height,
    ).expect("Failed to create transaction 1");

    network.broadcast_transaction(tx1.clone());

    // Mine block with tx1
    mine_block_with_txs(&network, blocks_to_mine);

    // Verify balances after tx1
    let wallet0_balance = get_wallet_balance(&network, &network.wallets[0]);
    let wallet1_balance = get_wallet_balance(&network, &network.wallets[1]);
    println!("    After tx1 - Wallet 0: {} BTH, Wallet 1: {} BTH",
        wallet0_balance / PICOCREDITS_PER_CREDIT,
        wallet1_balance / PICOCREDITS_PER_CREDIT);

    // Wallet 0 should have: original - send - fee + mining_reward (if they mined this block)
    // Wallet 1 should have: original + send + mining_reward (if they mined)

    // ========================================================================
    // Transaction 2: Wallet 1 -> Wallet 2 (chain from received funds)
    // ========================================================================
    println!("\n  --- Transaction 2: Wallet 1 -> Wallet 2 (spending received funds) ---");

    let sender_wallet = &network.wallets[1];
    let recipient_wallet = &network.wallets[2];
    let recipient_address = recipient_wallet.public_address();

    // Wallet 1 should now have UTXOs from mining + the transfer from wallet 0
    let sender_utxos = scan_wallet_utxos(&network, sender_wallet);
    println!("    Wallet 1 has {} UTXOs", sender_utxos.len());

    // Find the UTXO we received (should be the smaller one, around 10 BTH)
    let (utxo_to_spend, subaddr_idx) = sender_utxos.iter()
        .find(|(u, _)| u.output.amount == send_amount)
        .unwrap_or(&sender_utxos[0]);

    let send_amount2 = 5 * PICOCREDITS_PER_CREDIT; // Send 5 BTH
    total_fees_expected += tx_fee;

    let node = network.get_node(0);
    let current_height = node.chain_state().height;
    drop(node);

    println!("    Sending {} BTH (from received funds)", send_amount2 / PICOCREDITS_PER_CREDIT);

    let tx2 = create_signed_transaction(
        sender_wallet,
        utxo_to_spend,
        *subaddr_idx,
        &recipient_address,
        send_amount2,
        tx_fee,
        current_height,
    ).expect("Failed to create transaction 2");

    network.broadcast_transaction(tx2.clone());

    // Mine block with tx2
    mine_block_with_txs(&network, blocks_to_mine + 1);

    // ========================================================================
    // Transaction 3: Wallet 2 -> Wallet 3 (continuing the chain)
    // ========================================================================
    println!("\n  --- Transaction 3: Wallet 2 -> Wallet 3 ---");

    let sender_wallet = &network.wallets[2];
    let recipient_wallet = &network.wallets[3];
    let recipient_address = recipient_wallet.public_address();

    let sender_utxos = scan_wallet_utxos(&network, sender_wallet);
    println!("    Wallet 2 has {} UTXOs", sender_utxos.len());

    let (utxo_to_spend, subaddr_idx) = &sender_utxos[0];
    let send_amount3 = 2 * PICOCREDITS_PER_CREDIT; // Send 2 BTH
    total_fees_expected += tx_fee;

    let node = network.get_node(0);
    let current_height = node.chain_state().height;
    drop(node);

    println!("    Sending {} BTH", send_amount3 / PICOCREDITS_PER_CREDIT);

    let tx3 = create_signed_transaction(
        sender_wallet,
        utxo_to_spend,
        *subaddr_idx,
        &recipient_address,
        send_amount3,
        tx_fee,
        current_height,
    ).expect("Failed to create transaction 3");

    network.broadcast_transaction(tx3.clone());

    // Mine block with tx3
    mine_block_with_txs(&network, blocks_to_mine + 2);

    // ========================================================================
    // Transaction 4: Wallet 3 -> Wallet 4 (complete the ring)
    // ========================================================================
    println!("\n  --- Transaction 4: Wallet 3 -> Wallet 4 ---");

    let sender_wallet = &network.wallets[3];
    let recipient_wallet = &network.wallets[4];
    let recipient_address = recipient_wallet.public_address();

    let sender_utxos = scan_wallet_utxos(&network, sender_wallet);
    println!("    Wallet 3 has {} UTXOs", sender_utxos.len());

    let (utxo_to_spend, subaddr_idx) = &sender_utxos[0];
    let send_amount4 = 1 * PICOCREDITS_PER_CREDIT; // Send 1 BTH
    total_fees_expected += tx_fee;

    let node = network.get_node(0);
    let current_height = node.chain_state().height;
    drop(node);

    println!("    Sending {} BTH", send_amount4 / PICOCREDITS_PER_CREDIT);

    let tx4 = create_signed_transaction(
        sender_wallet,
        utxo_to_spend,
        *subaddr_idx,
        &recipient_address,
        send_amount4,
        tx_fee,
        current_height,
    ).expect("Failed to create transaction 4");

    network.broadcast_transaction(tx4.clone());

    // Mine block with tx4
    mine_block_with_txs(&network, blocks_to_mine + 3);

    // ========================================================================
    // Verify final state after all transactions
    // ========================================================================
    println!("\n  --- Final Balance Verification ---");

    for (i, wallet) in network.wallets.iter().enumerate() {
        let balance = get_wallet_balance(&network, wallet);
        println!("    Wallet {}: {} BTH", i, balance / PICOCREDITS_PER_CREDIT);
    }

    // Verify consistency
    println!("\n  Verifying ledger consistency after transactions...");
    network.verify_consistency();

    // Verify fees were burned
    let node = network.get_node(0);
    let post_tx_state = node.chain_state();
    drop(node);

    println!(
        "  Total fees burned: {} picocredits (expected: {})",
        post_tx_state.total_fees_burned, total_fees_expected
    );

    assert!(
        post_tx_state.total_fees_burned >= total_fees_expected,
        "Expected at least {} fees burned, got {}",
        total_fees_expected,
        post_tx_state.total_fees_burned
    );

    // Phase 3: Final verification
    println!("\nPhase 3: Final state verification...");
    let node = network.get_node(0);
    let final_state = node.chain_state();
    drop(node);

    println!("  Final height: {}", final_state.height);
    println!("  Final total mined: {} BTH", final_state.total_mined / PICOCREDITS_PER_CREDIT);
    println!(
        "  Final fees burned: {} picocredits",
        final_state.total_fees_burned
    );

    // Verify final state
    let num_tx_blocks = 4; // We mined 4 blocks with transactions
    let expected_height = (blocks_to_mine + num_tx_blocks) as u64;
    assert_eq!(
        final_state.height, expected_height,
        "Final height should be {}",
        expected_height
    );

    let expected_mined = expected_height * INITIAL_BLOCK_REWARD;
    assert_eq!(
        final_state.total_mined, expected_mined,
        "Total mined should be {} picocredits",
        expected_mined
    );

    // Verify fees were burned (4 transactions * MIN_TX_FEE each)
    let expected_total_fees = 4 * MIN_TX_FEE;
    assert!(
        final_state.total_fees_burned >= expected_total_fees,
        "Expected at least {} fees burned from 4 transactions, got {}",
        expected_total_fees,
        final_state.total_fees_burned
    );

    // Verify circulating supply
    let circulating_supply = final_state.total_mined - final_state.total_fees_burned;
    println!(
        "  Circulating supply: {} BTH (mined: {}, burned: {})",
        circulating_supply / PICOCREDITS_PER_CREDIT,
        final_state.total_mined / PICOCREDITS_PER_CREDIT,
        final_state.total_fees_burned
    );

    // Verify UTXO conservation: total balance across all wallets should equal circulating supply
    let total_wallet_balance: u64 = network.wallets.iter()
        .map(|w| get_wallet_balance(&network, w))
        .sum();

    println!("  Total wallet balances: {} BTH", total_wallet_balance / PICOCREDITS_PER_CREDIT);

    // The total wallet balance should equal total_mined - fees_burned
    // (all coins are accounted for in wallets)
    assert_eq!(
        total_wallet_balance, circulating_supply,
        "Total wallet balance ({}) should equal circulating supply ({})",
        total_wallet_balance, circulating_supply
    );

    println!("\n=== E2E Test Complete ===\n");
    println!("Summary:");
    println!("  - {} nodes reached consensus", NUM_NODES);
    println!("  - {} blocks mined", final_state.height);
    println!("  - {} transactions executed", num_tx_blocks);
    println!("  - {} picocredits fees burned", final_state.total_fees_burned);
    println!(
        "  - {} BTH circulating supply",
        circulating_supply / PICOCREDITS_PER_CREDIT
    );
    println!("  - All nodes have consistent ledger state");
    println!("  - UTXO conservation verified: all coins accounted for");

    // Cleanup
    network.stop();
}

// ============================================================================
// Ring Signature (Private Transaction) Test
// ============================================================================

/// Test private transactions using ring signatures for sender anonymity.
/// Ring signatures hide which UTXO is being spent among a ring of decoys.
#[test]
#[ignore = "Needs update for ring signature transactions (Ring tx removed, use Clsag)"]
fn test_private_ring_signature_transaction() {
    use botho::wallet::Wallet;

    println!("\n=== Private Ring Signature Transaction Test ===\n");

    // Build the network
    println!("Building test network...");
    let mut network = build_test_network();
    thread::sleep(Duration::from_millis(500));

    // Mine enough blocks to have sufficient decoys for ring signatures
    // Ring size requires MIN_RING_SIZE (7) outputs
    // Decoys require min_confirmations (10), so we need at least 20 blocks
    // to have 10 confirmed outputs available as decoys
    let blocks_to_mine = 20; // Mine 20 blocks for sufficient confirmed decoys
    println!("Mining {} blocks to build decoy set...", blocks_to_mine);

    for i in 0..blocks_to_mine {
        let miner_idx = i % NUM_NODES;
        let miner_wallet = &network.wallets[miner_idx];
        let miner_address = miner_wallet.public_address();

        let node = network.get_node(0);
        let state = node.chain_state();
        let prev_block = node.get_tip();
        let prev_hash = prev_block.hash();
        let height = state.height + 1;
        drop(node);

        network.pending_minting_txs.lock().unwrap().clear();
        let minting_tx = create_mock_minting_tx(height, INITIAL_BLOCK_REWARD, &miner_address, prev_hash);
        network.broadcast_minting_tx(minting_tx);

        if !network.wait_for_height(height, Duration::from_secs(30)) {
            panic!("Timeout waiting for block {}", height);
        }
        thread::sleep(Duration::from_millis(100));
    }

    // Verify mining succeeded
    network.verify_consistency();
    let node = network.get_node(0);
    let state = node.chain_state();
    println!("  Mined {} blocks, total supply: {} BTH\n",
        state.height, state.total_mined / PICOCREDITS_PER_CREDIT);
    drop(node);

    // Create a private transaction from wallet 0 to wallet 1
    println!("Creating private ring signature transaction...");

    let sender_wallet_keys = &network.wallets[0];
    let recipient_wallet_keys = &network.wallets[1];
    let recipient_address = recipient_wallet_keys.public_address();

    // Create a Wallet from the WalletKeys mnemonic for private tx creation
    let sender_wallet = Wallet::from_mnemonic(sender_wallet_keys.mnemonic_phrase())
        .expect("Failed to create wallet from mnemonic");

    // Find UTXOs owned by sender
    let sender_utxos = scan_wallet_utxos(&network, sender_wallet_keys);
    println!("  Sender has {} UTXOs", sender_utxos.len());

    if sender_utxos.is_empty() {
        panic!("Sender has no UTXOs to spend!");
    }

    // Get the UTXO and prepare for spending
    let (utxo_to_spend, _subaddr_idx) = &sender_utxos[0];
    let utxo_amount = utxo_to_spend.output.amount;
    let send_amount = 10 * PICOCREDITS_PER_CREDIT; // Send 10 BTH
    let tx_fee = MIN_TX_FEE;

    println!("  Spending UTXO with {} BTH", utxo_amount / PICOCREDITS_PER_CREDIT);
    println!("  Sending {} BTH to wallet 1", send_amount / PICOCREDITS_PER_CREDIT);

    // Get current height
    let node = network.get_node(0);
    let current_height = node.chain_state().height;
    let ledger = node.ledger.read().unwrap();

    // Create outputs (recipient + change)
    let mut outputs = vec![TxOutput::new(send_amount, &recipient_address)];
    let change = utxo_amount - send_amount - tx_fee;
    if change > 0 {
        outputs.push(TxOutput::new(change, &sender_wallet_keys.public_address()));
    }

    // Create private transaction with ring signature
    let private_tx = sender_wallet.create_private_transaction(
        &[utxo_to_spend.clone()],
        outputs,
        tx_fee,
        current_height,
        &ledger,
    ).expect("Failed to create private transaction");

    drop(ledger);
    drop(node);

    // Verify the transaction has ring inputs
    assert!(private_tx.is_private(), "Transaction should be private");
    let ring_inputs = private_tx.inputs.ring().expect("Should have ring inputs");
    println!("  Created ring signature with {} decoys per input", ring_inputs[0].ring.len() - 1);
    println!("  Key image: {}", hex::encode(&ring_inputs[0].key_image[0..8]));

    // Verify the ring signature is valid
    private_tx.verify_ring_signatures().expect("Ring signature should be valid");
    println!("  Ring signature verified successfully");

    // Broadcast and mine
    network.broadcast_transaction(private_tx.clone());
    mine_block_with_txs(&network, blocks_to_mine);

    // Verify the transaction was included
    network.verify_consistency();

    let node = network.get_node(0);
    let final_state = node.chain_state();
    drop(node);

    assert!(
        final_state.total_fees_burned >= tx_fee,
        "Fee should have been burned"
    );

    // Verify balances
    let recipient_balance = get_wallet_balance(&network, recipient_wallet_keys);
    println!("\n  Recipient balance: {} BTH", recipient_balance / PICOCREDITS_PER_CREDIT);

    // Recipient should have mining rewards + the transfer
    assert!(
        recipient_balance >= send_amount,
        "Recipient should have at least {} BTH",
        send_amount / PICOCREDITS_PER_CREDIT
    );

    println!("\n=== Private Transaction Test Complete ===\n");
    println!("Summary:");
    println!("  - Ring signature transaction created and verified");
    println!("  - Transaction included in block");
    println!("  - Fee burned: {} picocredits", final_state.total_fees_burned);
    println!("  - Sender anonymity preserved (hidden among {} decoys)", ring_inputs[0].ring.len() - 1);

    network.stop();
}

// ============================================================================
// Additional Tests
// ============================================================================

#[test]
fn test_network_builds_successfully() {
    let mut network = build_test_network();
    assert_eq!(network.node_ids.len(), NUM_NODES);
    assert_eq!(network.wallets.len(), NUM_NODES);

    // Verify all wallets are different
    for i in 0..NUM_NODES {
        for j in (i + 1)..NUM_NODES {
            assert_ne!(
                network.wallets[i].public_address().view_public_key().to_bytes(),
                network.wallets[j].public_address().view_public_key().to_bytes(),
                "Wallets {} and {} should be different",
                i,
                j
            );
        }
    }

    network.stop();
}

#[test]
fn test_mock_minting_tx_has_valid_pow() {
    // Use a properly generated wallet
    let wallet = WalletKeys::generate().unwrap();

    let address = wallet.public_address();
    let prev_hash = [0u8; 32];

    let minting_tx = create_mock_minting_tx(1, INITIAL_BLOCK_REWARD, &address, prev_hash);

    assert!(minting_tx.verify_pow(), "Mock minting tx should have valid PoW");
    assert_eq!(minting_tx.block_height, 1);
    assert_eq!(minting_tx.reward, INITIAL_BLOCK_REWARD);
}
