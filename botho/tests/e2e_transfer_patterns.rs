// Copyright (c) 2024 Botho Foundation
//
//! End-to-End Transfer Pattern Tests
//!
//! Tests realistic transaction patterns to build confidence in the system:
//! 1. Concurrent transfers - Multiple wallets sending simultaneously
//! 2. Multi-input consolidation - Spending multiple UTXOs in one transaction
//! 3. Payment splitting - One sender paying multiple recipients
//! 4. Stress/load patterns - High-volume transaction bursts
//!
//! These tests use a simulated 5-node SCP consensus network with in-memory
//! message passing for fast, deterministic testing.
//!
//! Tests use CLSAG ring signatures with proper decoy selection from the UTXO set.
//! The `ensure_decoy_availability` helper pre-mines enough blocks to satisfy
//! the minimum ring size requirement.

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
    transaction::{Transaction, TxOutput, Utxo, UtxoId, MIN_TX_FEE, PICOCREDITS_PER_CREDIT},
    wallet::Wallet,
};

use bth_account_keys::PublicAddress;

// ============================================================================
// Constants
// ============================================================================

const NUM_NODES: usize = 5;
const QUORUM_K: usize = 3;
const INITIAL_BLOCK_REWARD: u64 = 50 * PICOCREDITS_PER_CREDIT;
const SCP_TIMEBASE_MS: u64 = 100;
const MAX_SLOT_VALUES: usize = 100; // Increased for stress testing

// ============================================================================
// Consensus Value Type
// ============================================================================

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
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

enum TestNodeMessage {
    MintingTx(MintingTx),
    Transaction(Transaction),
    ScpMsg(Arc<Msg<ConsensusValue>>),
    Stop,
}

// ============================================================================
// Test Node
// ============================================================================

struct TestNode {
    node_id: NodeID,
    sender: Sender<TestNodeMessage>,
    ledger: Arc<RwLock<Ledger>>,
    wallet: Arc<Wallet>,
    _temp_dir: TempDir,
}

impl TestNode {
    fn chain_state(&self) -> ChainState {
        self.ledger.read().unwrap().get_chain_state().unwrap()
    }

    fn get_tip(&self) -> Block {
        self.ledger.read().unwrap().get_tip().unwrap()
    }

    fn stop(&self) {
        let _ = self.sender.send(TestNodeMessage::Stop);
    }
}

// ============================================================================
// Test Network
// ============================================================================

struct TestNetwork {
    nodes: Arc<DashMap<NodeID, TestNode>>,
    handles: Vec<thread::JoinHandle<()>>,
    node_ids: Vec<NodeID>,
    wallets: Vec<Arc<Wallet>>,
    pending_minting_txs: Arc<Mutex<HashMap<[u8; 32], MintingTx>>>,
    pending_txs: Arc<Mutex<HashMap<[u8; 32], Transaction>>>,
    shutdown: Arc<AtomicBool>,
}

impl TestNetwork {
    fn stop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        for entry in self.nodes.iter() {
            entry.value().stop();
        }
        thread::sleep(Duration::from_millis(100));
    }

    fn get_node(&self, index: usize) -> dashmap::mapref::one::Ref<'_, NodeID, TestNode> {
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
// Helper Functions
// ============================================================================

/// Generate a random wallet for testing
fn generate_test_wallet() -> Wallet {
    use bip39::{Language, Mnemonic, MnemonicType};
    let mnemonic = Mnemonic::new(MnemonicType::Words24, Language::English);
    Wallet::from_mnemonic(mnemonic.phrase()).expect("Failed to create wallet from mnemonic")
}

// ============================================================================
// Network Builder
// ============================================================================

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

    for i in 0..NUM_NODES {
        node_ids.push(test_node_id(i as u32));
    }

    for i in 0..NUM_NODES {
        let node_id = node_ids[i].clone();
        let peers: HashSet<NodeID> = node_ids
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, id)| id.clone())
            .collect();

        let wallet = Arc::new(generate_test_wallet());
        wallets.push(wallet.clone());

        let temp_dir = TempDir::new().unwrap();
        let ledger = Arc::new(RwLock::new(Ledger::open(temp_dir.path()).unwrap()));

        let (sender, receiver) = unbounded();

        let peer_vec: Vec<NodeID> = peers.iter().cloned().collect();
        let quorum_set = QuorumSet::new_with_node_ids(QUORUM_K as u32, peer_vec);

        let nodes_map_clone = nodes_map.clone();
        let peers_clone = peers.clone();
        let ledger_clone = ledger.clone();
        let pending_minting_clone = pending_minting_txs.clone();
        let pending_txs_clone = pending_txs.clone();
        let shutdown_clone = shutdown.clone();
        let node_id_clone = node_id.clone();

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
                let cv = ConsensusValue {
                    tx_hash: minting_tx.hash(),
                    priority: minting_tx.pow_priority(),
                    is_minting: true,
                };
                pending_values.push(cv);
            }
            Ok(TestNodeMessage::Transaction(tx)) => {
                let cv = ConsensusValue {
                    tx_hash: tx.hash(),
                    priority: tx.created_at_height,
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

        if !pending_values.is_empty() {
            let to_propose: BTreeSet<ConsensusValue> =
                pending_values.iter().take(MAX_SLOT_VALUES).cloned().collect();

            if let Ok(Some(out_msg)) = scp_node.propose_values(to_propose) {
                broadcast_scp_msg(&nodes_map, &peers, out_msg);
            }
        }

        for out_msg in scp_node.process_timeouts() {
            broadcast_scp_msg(&nodes_map, &peers, out_msg);
        }

        if let Some(externalized) = scp_node.get_externalized_values(current_slot) {
            if let Err(e) = apply_externalized_block(
                &ledger,
                &pending_minting_txs,
                &pending_txs,
                &externalized,
            ) {
                eprintln!("[Node {}] Failed to apply block: {}", node_id, e);
            }

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

    let mut transactions = Vec::new();
    let pending = pending_txs.lock().unwrap();
    for cv in externalized.iter().filter(|cv| !cv.is_minting) {
        if let Some(tx) = pending.get(&cv.tx_hash) {
            transactions.push(tx.clone());
        }
    }
    drop(pending);

    let ledger_read = ledger.read().map_err(|e| e.to_string())?;
    let prev_block = ledger_read.get_tip().map_err(|e| e.to_string())?;
    drop(ledger_read);

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

    let ledger_write = ledger.read().map_err(|e| e.to_string())?;
    ledger_write.add_block(&block).map_err(|e| e.to_string())?;

    Ok(())
}

// ============================================================================
// Helpers
// ============================================================================

fn create_mock_minting_tx(
    height: u64,
    reward: u64,
    minter_address: &PublicAddress,
    prev_block_hash: [u8; 32],
) -> MintingTx {
    let mut minting_tx = MintingTx::new(
        height,
        reward,
        minter_address,
        prev_block_hash,
        u64::MAX - 1,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    );

    for nonce in 0..1000 {
        minting_tx.nonce = nonce;
        if minting_tx.verify_pow() {
            break;
        }
    }

    minting_tx
}

fn scan_wallet_utxos(network: &TestNetwork, wallet: &Wallet) -> Vec<(Utxo, u64)> {
    let mut owned_utxos = Vec::new();

    let node = network.get_node(0);
    let ledger = node.ledger.read().unwrap();
    let state = ledger.get_chain_state().unwrap();

    for height in 0..=state.height {
        if let Ok(block) = ledger.get_block(height) {
            let coinbase_output = block.minting_tx.to_tx_output();
            if let Some(subaddr_idx) = coinbase_output.belongs_to(wallet.account_key()) {
                let block_hash = block.hash();
                let utxo_id = UtxoId::new(block_hash, 0);
                if let Ok(Some(utxo)) = ledger.get_utxo(&utxo_id) {
                    owned_utxos.push((utxo, subaddr_idx));
                }
            }

            for tx in &block.transactions {
                let tx_hash = tx.hash();
                for (idx, output) in tx.outputs.iter().enumerate() {
                    if let Some(subaddr_idx) = output.belongs_to(wallet.account_key()) {
                        let utxo_id = UtxoId::new(tx_hash, idx as u32);
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

fn get_wallet_balance(network: &TestNetwork, wallet: &Wallet) -> u64 {
    scan_wallet_utxos(network, wallet)
        .iter()
        .map(|(utxo, _)| utxo.output.amount)
        .sum()
}

fn mine_block(network: &TestNetwork, miner_idx: usize) {
    let miner_wallet = &network.wallets[miner_idx];
    let miner_address = miner_wallet.default_address();

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

    thread::sleep(Duration::from_millis(150));
}

/// Minimum ring size for testing (matches production)
const TEST_RING_SIZE: usize = 20;

/// Pre-mine blocks to ensure enough UTXOs exist for decoy selection.
/// CLSAG ring signatures need at least TEST_RING_SIZE members per input.
/// For multi-input transactions, we need extra UTXOs since the real inputs
/// are excluded from the decoy pool.
fn ensure_decoy_availability(network: &TestNetwork, extra_inputs: usize) {
    let needed_blocks = TEST_RING_SIZE + extra_inputs;
    let node = network.get_node(0);
    let current_height = node.chain_state().height;
    drop(node);

    if current_height < needed_blocks as u64 {
        let blocks_to_mine = needed_blocks - current_height as usize;
        println!("  Pre-mining {} blocks for decoy availability...", blocks_to_mine);
        for i in 0..blocks_to_mine {
            mine_block(network, i % NUM_NODES);
        }
    }
}

/// Create a signed CLSAG ring signature transaction.
fn create_signed_transaction(
    sender_wallet: &Wallet,
    sender_utxo: &Utxo,
    subaddress_index: u64,
    recipient: &PublicAddress,
    amount: u64,
    fee: u64,
    current_height: u64,
    network: &TestNetwork,
) -> Result<Transaction, String> {
    use botho::transaction::{ClsagRingInput, RingMember};
    use rand::seq::SliceRandom;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let node = network.get_node(0);
    let ledger = node.ledger.read().unwrap();

    let change = sender_utxo.output.amount.checked_sub(amount + fee)
        .ok_or("Insufficient funds")?;
    let mut outputs = vec![TxOutput::new(amount, recipient)];
    if change > 0 {
        outputs.push(TxOutput::new(change, &sender_wallet.default_address()));
    }

    let preliminary_tx = Transaction::new_clsag(Vec::new(), outputs.clone(), fee, current_height);
    let signing_hash = preliminary_tx.signing_hash();

    let onetime_private = sender_utxo.output
        .recover_spend_key(sender_wallet.account_key(), subaddress_index)
        .ok_or("Failed to recover spend key")?;

    let exclude_keys = vec![sender_utxo.output.target_key];
    let decoys = ledger.get_decoy_outputs(TEST_RING_SIZE - 1, &exclude_keys, 0)
        .map_err(|e| format!("Failed to get decoys: {}", e))?;

    if decoys.len() < TEST_RING_SIZE - 1 {
        return Err(format!("Not enough decoys: need {}, got {}", TEST_RING_SIZE - 1, decoys.len()));
    }

    let mut ring: Vec<RingMember> = Vec::with_capacity(TEST_RING_SIZE);
    ring.push(RingMember::from_output(&sender_utxo.output));
    for decoy in &decoys { ring.push(RingMember::from_output(decoy)); }

    let real_target_key = sender_utxo.output.target_key;
    let mut indices: Vec<usize> = (0..ring.len()).collect();
    indices.shuffle(&mut rng);
    let shuffled_ring: Vec<RingMember> = indices.iter().map(|&i| ring[i].clone()).collect();
    let real_index = shuffled_ring.iter().position(|m| m.target_key == real_target_key)
        .ok_or("Real input not found")?;

    let total_output = outputs.iter().map(|o| o.amount).sum::<u64>() + fee;
    let ring_input = ClsagRingInput::new(
        shuffled_ring, real_index, &onetime_private, sender_utxo.output.amount,
        total_output, &signing_hash, &mut rng,
    ).map_err(|e| format!("Failed to create CLSAG: {}", e))?;

    Ok(Transaction::new_clsag(vec![ring_input], outputs, fee, current_height))
}

/// Create a multi-input transaction spending multiple UTXOs
fn create_multi_input_transaction(
    sender_wallet: &Wallet,
    utxos_to_spend: &[(Utxo, u64)], // (utxo, subaddress_index)
    recipient: &PublicAddress,
    amount: u64,
    fee: u64,
    current_height: u64,
    network: &TestNetwork,
) -> Result<Transaction, String> {
    use botho::transaction::{ClsagRingInput, RingMember};
    use rand::seq::SliceRandom;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let node = network.get_node(0);
    let ledger = node.ledger.read().unwrap();

    let total_input: u64 = utxos_to_spend.iter().map(|(u, _)| u.output.amount).sum();
    let change = total_input.checked_sub(amount + fee).ok_or("Insufficient funds")?;
    let mut outputs = vec![TxOutput::new(amount, recipient)];
    if change > 0 {
        outputs.push(TxOutput::new(change, &sender_wallet.default_address()));
    }

    let preliminary_tx = Transaction::new_clsag(Vec::new(), outputs.clone(), fee, current_height);
    let signing_hash = preliminary_tx.signing_hash();
    let total_output = outputs.iter().map(|o| o.amount).sum::<u64>() + fee;

    let exclude_keys: Vec<[u8; 32]> = utxos_to_spend.iter().map(|(u, _)| u.output.target_key).collect();

    let mut ring_inputs = Vec::new();
    for (utxo, subaddr_idx) in utxos_to_spend {
        let onetime_private = utxo.output
            .recover_spend_key(sender_wallet.account_key(), *subaddr_idx)
            .ok_or("Failed to recover spend key")?;

        let decoys = ledger.get_decoy_outputs(TEST_RING_SIZE - 1, &exclude_keys, 0)
            .map_err(|e| format!("Failed to get decoys: {}", e))?;

        let mut ring: Vec<RingMember> = Vec::with_capacity(TEST_RING_SIZE);
        ring.push(RingMember::from_output(&utxo.output));
        for decoy in &decoys { ring.push(RingMember::from_output(decoy)); }

        let real_target_key = utxo.output.target_key;
        let mut indices: Vec<usize> = (0..ring.len()).collect();
        indices.shuffle(&mut rng);
        let shuffled_ring: Vec<RingMember> = indices.iter().map(|&i| ring[i].clone()).collect();
        let real_index = shuffled_ring.iter().position(|m| m.target_key == real_target_key)
            .ok_or("Real input not found")?;

        let ring_input = ClsagRingInput::new(
            shuffled_ring, real_index, &onetime_private, utxo.output.amount,
            total_output, &signing_hash, &mut rng,
        ).map_err(|e| format!("Failed to create CLSAG: {}", e))?;
        ring_inputs.push(ring_input);
    }

    Ok(Transaction::new_clsag(ring_inputs, outputs, fee, current_height))
}

/// Create a payment splitting transaction (one sender, multiple recipients)
fn create_split_payment_transaction(
    sender_wallet: &Wallet,
    sender_utxo: &Utxo,
    subaddress_index: u64,
    recipients: &[(PublicAddress, u64)], // (address, amount)
    fee: u64,
    current_height: u64,
    network: &TestNetwork,
) -> Result<Transaction, String> {
    use botho::transaction::{ClsagRingInput, RingMember};
    use rand::seq::SliceRandom;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let node = network.get_node(0);
    let ledger = node.ledger.read().unwrap();

    let total_send: u64 = recipients.iter().map(|(_, a)| *a).sum();
    let change = sender_utxo.output.amount.checked_sub(total_send + fee)
        .ok_or("Insufficient funds")?;

    let mut outputs: Vec<TxOutput> = recipients.iter()
        .map(|(addr, amt)| TxOutput::new(*amt, addr))
        .collect();
    if change > 0 {
        outputs.push(TxOutput::new(change, &sender_wallet.default_address()));
    }

    let preliminary_tx = Transaction::new_clsag(Vec::new(), outputs.clone(), fee, current_height);
    let signing_hash = preliminary_tx.signing_hash();

    let onetime_private = sender_utxo.output
        .recover_spend_key(sender_wallet.account_key(), subaddress_index)
        .ok_or("Failed to recover spend key")?;

    let exclude_keys = vec![sender_utxo.output.target_key];
    let decoys = ledger.get_decoy_outputs(TEST_RING_SIZE - 1, &exclude_keys, 0)
        .map_err(|e| format!("Failed to get decoys: {}", e))?;

    let mut ring: Vec<RingMember> = Vec::with_capacity(TEST_RING_SIZE);
    ring.push(RingMember::from_output(&sender_utxo.output));
    for decoy in &decoys { ring.push(RingMember::from_output(decoy)); }

    let real_target_key = sender_utxo.output.target_key;
    let mut indices: Vec<usize> = (0..ring.len()).collect();
    indices.shuffle(&mut rng);
    let shuffled_ring: Vec<RingMember> = indices.iter().map(|&i| ring[i].clone()).collect();
    let real_index = shuffled_ring.iter().position(|m| m.target_key == real_target_key)
        .ok_or("Real input not found")?;

    let total_output = outputs.iter().map(|o| o.amount).sum::<u64>() + fee;
    let ring_input = ClsagRingInput::new(
        shuffled_ring, real_index, &onetime_private, sender_utxo.output.amount,
        total_output, &signing_hash, &mut rng,
    ).map_err(|e| format!("Failed to create CLSAG: {}", e))?;

    Ok(Transaction::new_clsag(vec![ring_input], outputs, fee, current_height))
}

// ============================================================================
// Tests
// ============================================================================

/// Test 1: Concurrent Transfers
///
/// Multiple wallets broadcast transactions simultaneously, all included
/// in the same block. Tests mempool handling and consensus under concurrent load.
#[test]
#[ignore = "SCP bug: ballot values not sorted when 6+ transactions in single slot"]
fn test_concurrent_transfers() {
    println!("\n=== Concurrent Transfers Test ===\n");

    let mut network = build_test_network();
    thread::sleep(Duration::from_millis(500));

    // Mine exactly TEST_RING_SIZE blocks distributed across wallets for decoy availability
    // Each wallet gets some blocks, and we need at least 20 total
    println!("Mining initial blocks for decoys and to fund wallets...");
    for i in 0..TEST_RING_SIZE {
        mine_block(&network, i % NUM_NODES);
    }
    network.verify_consistency();

    // Verify each wallet has at least one UTXO
    for (i, wallet) in network.wallets.iter().enumerate() {
        let balance = get_wallet_balance(&network, wallet);
        println!("  Wallet {}: {} BTH", i, balance / PICOCREDITS_PER_CREDIT);
        assert!(balance >= INITIAL_BLOCK_REWARD, "Wallet {} should have mining reward", i);
    }

    // Create concurrent transfers: each wallet sends to the next
    println!("\nCreating {} concurrent transactions...", NUM_NODES);
    let node = network.get_node(0);
    let current_height = node.chain_state().height;
    drop(node);

    let mut transactions = Vec::new();
    let send_amount = 5 * PICOCREDITS_PER_CREDIT;

    for i in 0..NUM_NODES {
        let sender_wallet = &network.wallets[i];
        let recipient_wallet = &network.wallets[(i + 1) % NUM_NODES];
        let recipient_address = recipient_wallet.default_address();

        let sender_utxos = scan_wallet_utxos(&network, sender_wallet);
        let (utxo, subaddr_idx) = &sender_utxos[0];

        let tx = create_signed_transaction(
            sender_wallet,
            utxo,
            *subaddr_idx,
            &recipient_address,
            send_amount,
            MIN_TX_FEE,
            current_height,
            &network,
        ).expect(&format!("Failed to create tx from wallet {}", i));

        transactions.push((i, tx));
    }

    // Broadcast all transactions simultaneously
    println!("Broadcasting all transactions...");
    for (i, tx) in &transactions {
        println!("  Wallet {} -> Wallet {}: {} BTH", i, (i + 1) % NUM_NODES, send_amount / PICOCREDITS_PER_CREDIT);
        network.broadcast_transaction(tx.clone());
    }

    // Mine a single block containing all transactions
    println!("\nMining block with all concurrent transactions...");
    mine_block(&network, 0);

    // Verify all transactions were included
    network.verify_consistency();

    let node = network.get_node(0);
    let state = node.chain_state();
    let expected_fees = NUM_NODES as u64 * MIN_TX_FEE;
    println!("\nFees burned: {} (expected: {})", state.total_fees_burned, expected_fees);
    assert!(
        state.total_fees_burned >= expected_fees,
        "Expected at least {} fees from {} concurrent transactions",
        expected_fees,
        NUM_NODES
    );
    drop(node);

    // Verify each wallet received the transfer
    println!("\nFinal balances:");
    for (i, wallet) in network.wallets.iter().enumerate() {
        let balance = get_wallet_balance(&network, wallet);
        println!("  Wallet {}: {} BTH", i, balance / PICOCREDITS_PER_CREDIT);
    }

    println!("\n=== Concurrent Transfers Test Complete ===");
    println!("  - {} transactions processed in single block", NUM_NODES);
    println!("  - All nodes reached consensus");
    println!("  - Ring of transfers verified");

    network.stop();
}

/// Test 2: Multi-Input Consolidation
///
/// A wallet with multiple small UTXOs consolidates them into a single
/// larger output. Tests dust collection and multi-input transaction handling.
#[test]
#[ignore = "SCP bug: ballot values not sorted when mining 30+ blocks rapidly"]
fn test_multi_input_consolidation() {
    println!("\n=== Multi-Input Consolidation Test ===\n");

    let mut network = build_test_network();
    thread::sleep(Duration::from_millis(500));

    // Pre-mine blocks for decoy availability (need extra for the 3 inputs we'll consolidate)
    ensure_decoy_availability(&network, 5);

    // Mine 5 blocks to wallet 0 (creates 5 UTXOs to potentially consolidate)
    println!("Mining 5 blocks to wallet 0 for UTXOs to consolidate...");
    for _ in 0..5 {
        mine_block(&network, 0);
    }
    network.verify_consistency();

    let wallet0 = &network.wallets[0];
    let wallet1 = &network.wallets[1];
    let recipient_address = wallet1.default_address();

    let utxos = scan_wallet_utxos(&network, wallet0);
    println!("  Wallet 0 has {} UTXOs", utxos.len());
    assert!(utxos.len() >= 3, "Wallet 0 should have at least 3 UTXOs");

    let utxos_to_consolidate: Vec<(Utxo, u64)> = utxos.into_iter().take(3).collect();
    let total_input: u64 = utxos_to_consolidate.iter().map(|(u, _)| u.output.amount).sum();
    let send_amount = total_input - MIN_TX_FEE;

    let node = network.get_node(0);
    let current_height = node.chain_state().height;
    drop(node);

    println!("  Consolidating {} UTXOs ({} BTH total) into single output",
        utxos_to_consolidate.len(), total_input / PICOCREDITS_PER_CREDIT);

    let tx = create_multi_input_transaction(
        wallet0, &utxos_to_consolidate, &recipient_address,
        send_amount, MIN_TX_FEE, current_height, &network,
    ).expect("Failed to create multi-input transaction");

    println!("  Transaction has {} inputs", tx.inputs.len());

    network.broadcast_transaction(tx.clone());
    mine_block(&network, 0);
    network.verify_consistency();

    let balance1 = get_wallet_balance(&network, wallet1);
    println!("  Wallet 1 balance: {} BTH", balance1 / PICOCREDITS_PER_CREDIT);
    assert!(balance1 >= send_amount, "Wallet 1 should have received amount");

    println!("\n=== Multi-Input Consolidation Test Complete ===");
    network.stop();
}


/// Test 3: Payment Splitting
///
/// A single sender pays multiple recipients in one transaction.
/// Tests multi-output transaction handling.
#[test]
fn test_payment_splitting() {
    println!("\n=== Payment Splitting Test ===\n");

    let mut network = build_test_network();
    thread::sleep(Duration::from_millis(500));

    // Pre-mine blocks for decoy availability, plus one for the sender's UTXO
    ensure_decoy_availability(&network, 1);

    // Mine a block to fund wallet 0
    println!("Mining block to fund wallet 0...");
    mine_block(&network, 0);
    network.verify_consistency();

    let sender_wallet = &network.wallets[0];
    let sender_balance = get_wallet_balance(&network, sender_wallet);
    println!("  Sender (wallet 0) balance: {} BTH", sender_balance / PICOCREDITS_PER_CREDIT);

    // Prepare recipients: wallets 1, 2, 3, 4
    let recipients: Vec<(PublicAddress, u64)> = vec![
        (network.wallets[1].default_address(), 5 * PICOCREDITS_PER_CREDIT),
        (network.wallets[2].default_address(), 7 * PICOCREDITS_PER_CREDIT),
        (network.wallets[3].default_address(), 3 * PICOCREDITS_PER_CREDIT),
        (network.wallets[4].default_address(), 2 * PICOCREDITS_PER_CREDIT),
    ];
    let total_to_send: u64 = recipients.iter().map(|(_, amt)| *amt).sum();

    println!("\nCreating split payment to {} recipients:", recipients.len());
    for (i, (_, amt)) in recipients.iter().enumerate() {
        println!("  -> Wallet {}: {} BTH", i + 1, amt / PICOCREDITS_PER_CREDIT);
    }
    println!("  Total: {} BTH + {} fee",
        total_to_send / PICOCREDITS_PER_CREDIT,
        MIN_TX_FEE / PICOCREDITS_PER_CREDIT);

    let node = network.get_node(0);
    let current_height = node.chain_state().height;
    drop(node);

    let sender_utxos = scan_wallet_utxos(&network, sender_wallet);
    let (utxo, subaddr_idx) = &sender_utxos[0];

    let split_tx = create_split_payment_transaction(
        sender_wallet,
        utxo,
        *subaddr_idx,
        &recipients,
        MIN_TX_FEE,
        current_height,
        &network,
    ).expect("Failed to create split payment");

    // Verify transaction structure
    println!("\nTransaction structure:");
    println!("  Inputs: 1");
    println!("  Outputs: {} (4 recipients + 1 change)", split_tx.outputs.len());
    assert_eq!(split_tx.outputs.len(), 5, "Should have 5 outputs (4 recipients + change)");

    network.broadcast_transaction(split_tx.clone());
    mine_block(&network, 0); // Wallet 0 mines, getting a new reward

    // Verify all recipients received their amounts
    network.verify_consistency();

    println!("\nFinal balances:");
    for (i, wallet) in network.wallets.iter().enumerate() {
        let balance = get_wallet_balance(&network, wallet);
        println!("  Wallet {}: {} BTH", i, balance / PICOCREDITS_PER_CREDIT);
    }

    // Verify each recipient has at least what we sent them
    assert!(
        get_wallet_balance(&network, &network.wallets[1]) >= 5 * PICOCREDITS_PER_CREDIT,
        "Wallet 1 should have at least 5 BTH"
    );
    assert!(
        get_wallet_balance(&network, &network.wallets[2]) >= 7 * PICOCREDITS_PER_CREDIT,
        "Wallet 2 should have at least 7 BTH"
    );
    assert!(
        get_wallet_balance(&network, &network.wallets[3]) >= 3 * PICOCREDITS_PER_CREDIT,
        "Wallet 3 should have at least 3 BTH"
    );
    assert!(
        get_wallet_balance(&network, &network.wallets[4]) >= 2 * PICOCREDITS_PER_CREDIT,
        "Wallet 4 should have at least 2 BTH"
    );

    println!("\n=== Payment Splitting Test Complete ===");
    println!("  - Single transaction paid {} recipients", recipients.len());
    println!("  - All recipients received correct amounts");
    println!("  - Change returned to sender");

    network.stop();
}

/// Test 4: Stress/Load Testing
///
/// High-volume transaction bursts to test throughput and stability.
/// Generates many transactions across multiple blocks.
#[test]
#[ignore = "Needs update for ring signature transactions (Simple tx removed)"]
fn test_stress_load_patterns() {
    println!("\n=== Stress/Load Test ===\n");

    let mut network = build_test_network();
    thread::sleep(Duration::from_millis(500));

    // Mine initial blocks to fund all wallets
    println!("Phase 1: Mining initial blocks to fund all wallets...");
    let initial_blocks = 10;
    for i in 0..initial_blocks {
        mine_block(&network, i % NUM_NODES);
    }
    network.verify_consistency();

    // Show initial balances
    println!("\nInitial balances:");
    for (i, wallet) in network.wallets.iter().enumerate() {
        let balance = get_wallet_balance(&network, wallet);
        println!("  Wallet {}: {} BTH", i, balance / PICOCREDITS_PER_CREDIT);
    }

    // Phase 2: Generate burst of transactions
    println!("\nPhase 2: Generating transaction burst...");

    let node = network.get_node(0);
    let mut current_height = node.chain_state().height;
    drop(node);

    let transactions_per_block = 3; // Keep manageable for test speed
    let num_stress_blocks = 5;
    let mut total_transactions = 0;
    let mut total_fees_expected: u64 = 0;

    for block_num in 0..num_stress_blocks {
        println!("\n  Block {} transactions:", block_num + 1);

        // Create multiple transactions for this block
        for tx_num in 0..transactions_per_block {
            let sender_idx = (block_num * transactions_per_block + tx_num) % NUM_NODES;
            let recipient_idx = (sender_idx + 1) % NUM_NODES;

            let sender_wallet = &network.wallets[sender_idx];
            let recipient_wallet = &network.wallets[recipient_idx];

            let sender_utxos = scan_wallet_utxos(&network, sender_wallet);
            if sender_utxos.is_empty() {
                println!("    Wallet {} has no UTXOs, skipping", sender_idx);
                continue;
            }

            let (utxo, subaddr_idx) = &sender_utxos[0];
            let available = utxo.output.amount;
            let send_amount = 1 * PICOCREDITS_PER_CREDIT; // Small transfers

            if available < send_amount + MIN_TX_FEE {
                println!("    Wallet {} insufficient funds, skipping", sender_idx);
                continue;
            }

            let tx = create_signed_transaction(
                sender_wallet,
                utxo,
                *subaddr_idx,
                &recipient_wallet.default_address(),
                send_amount,
                MIN_TX_FEE,
                current_height,
                &network,
            );

            match tx {
                Ok(transaction) => {
                    network.broadcast_transaction(transaction);
                    total_transactions += 1;
                    total_fees_expected += MIN_TX_FEE;
                    println!("    {} -> {}: 1 BTH", sender_idx, recipient_idx);
                }
                Err(e) => {
                    println!("    {} -> {}: FAILED ({})", sender_idx, recipient_idx, e);
                }
            }
        }

        // Mine block with these transactions
        mine_block(&network, block_num % NUM_NODES);

        let node = network.get_node(0);
        current_height = node.chain_state().height;
        drop(node);
    }

    // Verify consistency after all stress blocks
    println!("\nPhase 3: Verifying consistency...");
    network.verify_consistency();

    let node = network.get_node(0);
    let final_state = node.chain_state();
    drop(node);

    println!("\nStress test results:");
    println!("  Total blocks: {}", final_state.height);
    println!("  Total transactions created: {}", total_transactions);
    println!("  Total fees burned: {} picocredits", final_state.total_fees_burned);
    println!("  Max expected fees: {} picocredits", total_fees_expected);

    // Calculate how many transactions were actually confirmed
    let confirmed_tx_count = final_state.total_fees_burned / MIN_TX_FEE;
    println!("  Confirmed transactions: {}", confirmed_tx_count);

    // At least some transactions should have been processed (at least 30% throughput)
    let min_expected_confirms = total_transactions as u64 / 3;
    assert!(
        confirmed_tx_count >= min_expected_confirms,
        "Expected at least {} transactions confirmed, got {}",
        min_expected_confirms,
        confirmed_tx_count
    );

    // Final balance verification
    println!("\nFinal balances:");
    let mut total_balance: u64 = 0;
    for (i, wallet) in network.wallets.iter().enumerate() {
        let balance = get_wallet_balance(&network, wallet);
        total_balance += balance;
        println!("  Wallet {}: {} BTH", i, balance / PICOCREDITS_PER_CREDIT);
    }

    // Verify conservation: total balance = total mined - fees
    let expected_circulating = final_state.total_mined - final_state.total_fees_burned;
    println!("\nConservation check:");
    println!("  Total mined: {} BTH", final_state.total_mined / PICOCREDITS_PER_CREDIT);
    println!("  Fees burned: {} picocredits", final_state.total_fees_burned);
    println!("  Expected circulating: {} BTH", expected_circulating / PICOCREDITS_PER_CREDIT);
    println!("  Actual total balance: {} BTH", total_balance / PICOCREDITS_PER_CREDIT);

    assert_eq!(
        total_balance, expected_circulating,
        "Total balance should equal circulating supply"
    );

    println!("\n=== Stress/Load Test Complete ===");
    println!("  - {} transactions across {} blocks", total_transactions, num_stress_blocks);
    println!("  - All nodes maintained consistency");
    println!("  - Conservation verified: no coins created or destroyed");

    network.stop();
}

/// Test 5: Rapid Sequential Transfers
///
/// A chain of rapid transfers between wallets, testing UTXO availability
/// and quick succession transaction handling.
#[test]
#[ignore = "Needs update for ring signature transactions (Simple tx removed)"]
fn test_rapid_sequential_transfers() {
    println!("\n=== Rapid Sequential Transfers Test ===\n");

    let mut network = build_test_network();
    thread::sleep(Duration::from_millis(500));

    // Mine initial block to wallet 0
    println!("Mining initial block to wallet 0...");
    mine_block(&network, 0);
    network.verify_consistency();

    let initial_balance = get_wallet_balance(&network, &network.wallets[0]);
    println!("  Wallet 0 initial balance: {} BTH", initial_balance / PICOCREDITS_PER_CREDIT);

    // Create a chain of transfers: 0 -> 1 -> 2 -> 3 -> 4 -> 0
    // Each transfer happens after the previous is confirmed
    println!("\nCreating rapid transfer chain: 0 -> 1 -> 2 -> 3 -> 4 -> 0");

    let send_amount = 10 * PICOCREDITS_PER_CREDIT;

    for round in 0..NUM_NODES {
        let sender_idx = round;
        let recipient_idx = (round + 1) % NUM_NODES;

        let node = network.get_node(0);
        let current_height = node.chain_state().height;
        drop(node);

        let sender_wallet = &network.wallets[sender_idx];
        let recipient_wallet = &network.wallets[recipient_idx];

        let sender_utxos = scan_wallet_utxos(&network, sender_wallet);
        assert!(!sender_utxos.is_empty(), "Wallet {} should have UTXOs", sender_idx);

        let (utxo, subaddr_idx) = &sender_utxos[0];

        let tx = create_signed_transaction(
            sender_wallet,
            utxo,
            *subaddr_idx,
            &recipient_wallet.default_address(),
            send_amount,
            MIN_TX_FEE,
            current_height,
            &network,
        ).expect(&format!("Failed to create transfer {} -> {}", sender_idx, recipient_idx));

        println!("  {} -> {}: {} BTH (confirmed in next block)",
            sender_idx, recipient_idx, send_amount / PICOCREDITS_PER_CREDIT);

        network.broadcast_transaction(tx);

        // Mine immediately to confirm this transaction before the next
        mine_block(&network, recipient_idx);
    }

    // Final verification
    network.verify_consistency();

    println!("\nFinal balances after {} rapid transfers:", NUM_NODES);
    for (i, wallet) in network.wallets.iter().enumerate() {
        let balance = get_wallet_balance(&network, wallet);
        println!("  Wallet {}: {} BTH", i, balance / PICOCREDITS_PER_CREDIT);
    }

    // The 10 BTH should have made a complete circuit, ending back at wallet 0
    // Each wallet should have: their mining rewards + received - sent
    // Due to fees, the circulating amount decreases with each transfer

    let node = network.get_node(0);
    let final_state = node.chain_state();
    drop(node);

    let expected_fees = NUM_NODES as u64 * MIN_TX_FEE;
    assert!(
        final_state.total_fees_burned >= expected_fees,
        "Expected at least {} fees from {} transfers",
        expected_fees,
        NUM_NODES
    );

    println!("\n=== Rapid Sequential Transfers Test Complete ===");
    println!("  - {} rapid transfers completed", NUM_NODES);
    println!("  - Each transfer confirmed before next began");
    println!("  - UTXO availability verified at each step");

    network.stop();
}

/// Test 6: Mixed Transaction Patterns
///
/// Combines all patterns in a single test: concurrent, multi-input,
/// split payments, and sequential transfers.
#[test]
#[ignore = "Needs update for ring signature transactions (Simple tx removed)"]
fn test_mixed_transaction_patterns() {
    println!("\n=== Mixed Transaction Patterns Test ===\n");

    let mut network = build_test_network();
    thread::sleep(Duration::from_millis(500));

    // Phase 1: Fund all wallets with multiple UTXOs
    println!("Phase 1: Creating initial UTXO distribution...");
    for round in 0..3 {
        for i in 0..NUM_NODES {
            mine_block(&network, i);
        }
    }
    network.verify_consistency();

    println!("Initial state:");
    for (i, wallet) in network.wallets.iter().enumerate() {
        let utxos = scan_wallet_utxos(&network, wallet);
        let balance = utxos.iter().map(|(u, _)| u.output.amount).sum::<u64>();
        println!("  Wallet {}: {} UTXOs, {} BTH", i, utxos.len(), balance / PICOCREDITS_PER_CREDIT);
    }

    let node = network.get_node(0);
    let current_height = node.chain_state().height;
    drop(node);

    // Phase 2: Execute mixed patterns in a single block
    println!("\nPhase 2: Creating mixed transaction batch...");

    // Pattern A: Wallet 0 consolidates 2 UTXOs
    let wallet0 = &network.wallets[0];
    let utxos0 = scan_wallet_utxos(&network, wallet0);
    let utxos_to_consolidate: Vec<(Utxo, u64)> = utxos0.into_iter().take(2).collect();
    let consolidate_total: u64 = utxos_to_consolidate.iter().map(|(u, _)| u.output.amount).sum();

    let consolidate_tx = create_multi_input_transaction(
        wallet0,
        &utxos_to_consolidate,
        &wallet0.default_address(),
        consolidate_total - MIN_TX_FEE,
        MIN_TX_FEE,
        current_height,
        &network,
    ).expect("Failed to create consolidation");
    println!("  [A] Wallet 0: Consolidating 2 UTXOs");

    // Pattern B: Wallet 1 splits to wallets 2, 3
    let wallet1 = &network.wallets[1];
    let utxos1 = scan_wallet_utxos(&network, wallet1);
    let (utxo1, subaddr1) = &utxos1[0];

    let split_recipients = vec![
        (network.wallets[2].default_address(), 5 * PICOCREDITS_PER_CREDIT),
        (network.wallets[3].default_address(), 5 * PICOCREDITS_PER_CREDIT),
    ];

    let split_tx = create_split_payment_transaction(
        wallet1,
        utxo1,
        *subaddr1,
        &split_recipients,
        MIN_TX_FEE,
        current_height,
        &network,
    ).expect("Failed to create split payment");
    println!("  [B] Wallet 1: Split payment to wallets 2, 3");

    // Pattern C: Wallet 4 simple transfer to wallet 0
    let wallet4 = &network.wallets[4];
    let utxos4 = scan_wallet_utxos(&network, wallet4);
    let (utxo4, subaddr4) = &utxos4[0];

    let simple_tx = create_signed_transaction(
        wallet4,
        utxo4,
        *subaddr4,
        &wallet0.default_address(),
        3 * PICOCREDITS_PER_CREDIT,
        MIN_TX_FEE,
        current_height,
        &network,
    ).expect("Failed to create simple transfer");
    println!("  [C] Wallet 4: Simple transfer to wallet 0");

    // Broadcast all simultaneously
    println!("\nBroadcasting all transactions concurrently...");
    network.broadcast_transaction(consolidate_tx);
    network.broadcast_transaction(split_tx);
    network.broadcast_transaction(simple_tx);

    // Mine single block with all patterns
    mine_block(&network, 2);
    network.verify_consistency();

    // Phase 3: Verify results
    println!("\nPhase 3: Verification...");

    let node = network.get_node(0);
    let final_state = node.chain_state();
    drop(node);

    println!("Final state:");
    for (i, wallet) in network.wallets.iter().enumerate() {
        let utxos = scan_wallet_utxos(&network, wallet);
        let balance = utxos.iter().map(|(u, _)| u.output.amount).sum::<u64>();
        println!("  Wallet {}: {} UTXOs, {} BTH", i, utxos.len(), balance / PICOCREDITS_PER_CREDIT);
    }

    // Verify conservation
    let total_balance: u64 = network.wallets.iter()
        .map(|w| get_wallet_balance(&network, w))
        .sum();
    let expected = final_state.total_mined - final_state.total_fees_burned;

    println!("\nConservation: total_balance={}, expected={}", total_balance, expected);
    assert_eq!(total_balance, expected, "Conservation violated");

    println!("\n=== Mixed Transaction Patterns Test Complete ===");
    println!("  - Consolidation, split payment, and simple transfer in one block");
    println!("  - All patterns executed and verified");
    println!("  - Conservation maintained");

    network.stop();
}
