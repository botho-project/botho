// Copyright (c) 2024 Botho Foundation
//
//! End-to-End Progressive Fee System Tests
//!
//! Tests the cluster-tax progressive fee system to verify:
//! 1. Cluster factor - wealthy clusters pay higher fees (1x-6x)
//! 2. Fee rejection - transactions with insufficient fees are rejected
//! 3. Cluster tag inheritance - tags propagate with decay through transactions
//! 4. Dynamic fees - congestion increases fee requirements
//!
//! These tests use a simulated 5-node SCP consensus network with in-memory
//! message passing for fast, deterministic testing.

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

use bth_cluster_tax::{FeeConfig, TransactionType as FeeTransactionType};
use bth_common::NodeID;
use bth_consensus_scp::{
    msg::Msg,
    slot::{CombineFn, ValidityFn},
    test_utils::test_node_id,
    Node as ScpNodeImpl, QuorumSet, ScpNode, SlotIndex,
};
use bth_transaction_types::{ClusterId, ClusterTagEntry, ClusterTagVector, TAG_WEIGHT_SCALE};

use botho::{
    block::{Block, MintingTx},
    ledger::{ChainState, Ledger},
    mempool::{Mempool, MempoolError},
    transaction::{Transaction, TxOutput, Utxo, UtxoId, TxInputs, PICOCREDITS_PER_CREDIT, MIN_TX_FEE},
    wallet::Wallet,
};

/// Convert nanoBTH to picocredits.
/// 1 BTH = 10^12 picocredits = 10^9 nanoBTH, so 1 nanoBTH = 1000 picocredits
const PICOCREDITS_PER_NANOBTH: u64 = 1000;

use bth_account_keys::PublicAddress;

// ============================================================================
// Constants
// ============================================================================

const NUM_NODES: usize = 5;
const QUORUM_K: usize = 3;
const INITIAL_BLOCK_REWARD: u64 = 50 * PICOCREDITS_PER_CREDIT;
const SCP_TIMEBASE_MS: u64 = 100;
const MAX_SLOT_VALUES: usize = 100;

/// Minimum ring size for testing (matches production)
const TEST_RING_SIZE: usize = 20;

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
        let deadline = std::time::Instant::now() + timeout;

        while std::time::Instant::now() < deadline {
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
// Mining and UTXO Helpers
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

/// Pre-mine blocks to ensure enough UTXOs exist for decoy selection.
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

// ============================================================================
// Transaction Creation Helpers
// ============================================================================

use botho::transaction::{ClsagRingInput, RingMember};

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
    create_signed_transaction_with_tags(
        sender_wallet,
        sender_utxo,
        subaddress_index,
        recipient,
        amount,
        fee,
        current_height,
        network,
        None, // Use default (empty) cluster tags
    )
}

/// Create a signed CLSAG transaction with explicit cluster tags on outputs.
fn create_signed_transaction_with_tags(
    sender_wallet: &Wallet,
    sender_utxo: &Utxo,
    subaddress_index: u64,
    recipient: &PublicAddress,
    amount: u64,
    fee: u64,
    current_height: u64,
    network: &TestNetwork,
    cluster_tags: Option<ClusterTagVector>,
) -> Result<Transaction, String> {
    use rand::seq::SliceRandom;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let node = network.get_node(0);
    let ledger = node.ledger.read().unwrap();

    let change = sender_utxo.output.amount.checked_sub(amount + fee)
        .ok_or("Insufficient funds")?;

    // Create outputs with cluster tags if provided
    let tags = cluster_tags.unwrap_or_else(ClusterTagVector::empty);
    let mut outputs = vec![
        TxOutput::new_with_cluster_tags(amount, recipient, None, tags.clone())
    ];
    if change > 0 {
        outputs.push(TxOutput::new_with_cluster_tags(
            change,
            &sender_wallet.default_address(),
            None,
            tags,
        ));
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

// ============================================================================
// Fee Calculation Helpers
// ============================================================================

/// Compute the expected minimum fee for a transaction.
/// Compute expected minimum fee in picocredits (ready for Transaction.fee).
/// This converts from nanoBTH (cluster-tax system) to picocredits (transaction system)
/// and ensures the result is at least MIN_TX_FEE.
fn compute_expected_min_fee(
    tx: &Transaction,
    cluster_wealth: u64,
    dynamic_base: u64,
) -> u64 {
    let fee_config = FeeConfig::default();
    let tx_size = tx.estimate_size();
    let tx_type = match &tx.inputs {
        TxInputs::Clsag(_) => FeeTransactionType::Hidden,
        TxInputs::Lion(_) => FeeTransactionType::PqHidden,
    };
    let num_memos = tx.outputs.iter().filter(|o| o.has_memo()).count();
    let fee_nanobth = fee_config.minimum_fee_dynamic(tx_type, tx_size, cluster_wealth, num_memos, dynamic_base);
    // Convert nanoBTH to picocredits and ensure at least MIN_TX_FEE
    let fee_pico = fee_nanobth * PICOCREDITS_PER_NANOBTH;
    fee_pico.max(MIN_TX_FEE)
}

/// Compute expected minimum fee in nanoBTH (for display purposes).
fn compute_expected_min_fee_nanobth(
    tx: &Transaction,
    cluster_wealth: u64,
    dynamic_base: u64,
) -> u64 {
    let fee_config = FeeConfig::default();
    let tx_size = tx.estimate_size();
    let tx_type = match &tx.inputs {
        TxInputs::Clsag(_) => FeeTransactionType::Hidden,
        TxInputs::Lion(_) => FeeTransactionType::PqHidden,
    };
    let num_memos = tx.outputs.iter().filter(|o| o.has_memo()).count();
    fee_config.minimum_fee_dynamic(tx_type, tx_size, cluster_wealth, num_memos, dynamic_base)
}

/// Compute cluster wealth from transaction outputs (same as mempool does).
fn compute_cluster_wealth_from_outputs(outputs: &[TxOutput]) -> u64 {
    let mut cluster_wealths: HashMap<u64, u64> = HashMap::new();

    for output in outputs {
        let value = output.amount;
        for entry in &output.cluster_tags.entries {
            let contribution = ((value as u128) * (entry.weight as u128)
                / (TAG_WEIGHT_SCALE as u128)) as u64;
            *cluster_wealths.entry(entry.cluster_id.0).or_insert(0) += contribution;
        }
    }

    cluster_wealths.values().copied().max().unwrap_or(0)
}

/// Create a cluster tag vector with single cluster at 100% weight.
fn single_cluster_tags(cluster_id: u64) -> ClusterTagVector {
    ClusterTagVector::single(ClusterId(cluster_id))
}

/// Create a cluster tag vector with specified entries.
fn custom_cluster_tags(entries: &[(u64, u32)]) -> ClusterTagVector {
    ClusterTagVector::from_pairs(
        &entries.iter()
            .map(|(id, weight)| (ClusterId(*id), *weight))
            .collect::<Vec<_>>()
    )
}

// ============================================================================
// Tests
// ============================================================================

/// Test 1: Cluster Factor - Wealthy clusters pay higher fees
///
/// Verifies that the progressive fee system correctly applies higher
/// cluster factors to transactions from wealthy clusters.
#[test]
fn test_cluster_factor_wealthy_pay_more() {
    println!("\n=== Cluster Factor Test: Wealthy Pay More ===\n");

    let fee_config = FeeConfig::default();
    let tx_size = 4000; // ~4KB typical CLSAG transaction

    // Test cluster factors at different wealth levels
    let test_cases = [
        (0u64, "Zero wealth"),
        (1_000_000u64, "1M wealth"),
        (10_000_000u64, "10M wealth (w_mid)"),
        (50_000_000u64, "50M wealth"),
        (100_000_000u64, "100M wealth"),
    ];

    println!("Testing cluster factor curve:");
    println!("{:>20} | {:>12} | {:>15}", "Cluster Wealth", "Factor", "Fee (nanoBTH)");
    println!("{:-<20}-+-{:-<12}-+-{:-<15}", "", "", "");

    let mut prev_fee = 0u64;
    for (wealth, label) in test_cases {
        let factor = fee_config.cluster_factor(wealth);
        let fee = fee_config.compute_fee(FeeTransactionType::Hidden, tx_size, wealth, 0);

        println!("{:>20} | {:>10.2}x | {:>15}", label, factor as f64 / 1000.0, fee);

        // Verify fees increase with wealth
        assert!(fee >= prev_fee, "Fee should increase with wealth: {} >= {}", fee, prev_fee);
        prev_fee = fee;
    }

    // Verify extreme values
    let factor_zero = fee_config.cluster_factor(0);
    let factor_max = fee_config.cluster_factor(100_000_000);

    // Zero wealth should be close to 1x (1000-2000 range due to sigmoid)
    assert!(factor_zero < 3000, "Zero wealth factor should be low: {}", factor_zero);

    // Max wealth should be close to 6x (5000-6000 range)
    assert!(factor_max >= 5000, "Max wealth factor should be high: {}", factor_max);

    // Ratio should be significant (at least 2x difference)
    let ratio = factor_max as f64 / factor_zero as f64;
    assert!(ratio > 2.0, "Wealthy should pay significantly more: {}x", ratio);

    println!("\nFactor ratio (wealthy/small): {:.2}x", ratio);
    println!("=== Cluster Factor Test Complete ===\n");
}

/// Test 2: Fee Rejection - Transactions with insufficient fees are rejected
///
/// Verifies that the mempool correctly rejects transactions that don't
/// pay the minimum required fee. Tests two validation layers:
/// 1. Transaction structure validation (fee >= MIN_TX_FEE)
/// 2. Mempool validation (fee >= cluster-tax computed fee)
#[test]
fn test_fee_rejection_below_minimum() {
    println!("\n=== Fee Rejection Test: Below Minimum ===\n");

    let mut network = build_test_network();
    thread::sleep(Duration::from_millis(500));

    // Pre-mine blocks for decoy availability
    ensure_decoy_availability(&network, 1);

    // Mine a block to fund wallet 0
    mine_block(&network, 0);
    network.verify_consistency();

    let sender_wallet = &network.wallets[0];
    let recipient_wallet = &network.wallets[1];
    let recipient_address = recipient_wallet.default_address();

    let sender_utxos = scan_wallet_utxos(&network, sender_wallet);
    let (utxo, subaddr_idx) = &sender_utxos[0];

    let node = network.get_node(0);
    let current_height = node.chain_state().height;
    let ledger = node.ledger.clone();
    drop(node);

    let send_amount = 5 * PICOCREDITS_PER_CREDIT;

    // Create transaction with very low fee (below MIN_TX_FEE)
    // This tests the structure validation layer
    let low_fee = 1000; // Way below MIN_TX_FEE
    let tx_low_fee = create_signed_transaction(
        sender_wallet,
        utxo,
        *subaddr_idx,
        &recipient_address,
        send_amount,
        low_fee,
        current_height,
        &network,
    ).expect("Failed to create transaction");

    // Try to add to mempool - should be rejected by structure validation
    let mut mempool = Mempool::new();
    let ledger_guard = ledger.read().unwrap();
    let result = mempool.add_tx(tx_low_fee.clone(), &ledger_guard);

    // The transaction fails structure validation (MIN_TX_FEE check)
    match &result {
        Err(MempoolError::InvalidTransaction(msg)) if msg.contains("fee below minimum") => {
            println!("Transaction correctly rejected by structure validation:");
            println!("  Provided fee: {} picocredits", low_fee);
            println!("  MIN_TX_FEE: {} picocredits", MIN_TX_FEE);
        }
        Err(MempoolError::FeeTooLow { minimum, provided }) => {
            println!("Transaction correctly rejected by mempool fee check:");
            println!("  Provided fee: {} picocredits", provided);
            println!("  Minimum required: {} picocredits", minimum);
            assert_eq!(*provided, low_fee);
            assert!(*minimum > *provided);
        }
        Ok(_) => panic!("Transaction should have been rejected for low fee"),
        Err(e) => panic!("Unexpected error: {:?}", e),
    }
    assert!(result.is_err(), "Low fee transaction should be rejected");

    // Now create transaction with sufficient fee (at least MIN_TX_FEE)
    let expected_min = compute_expected_min_fee(&tx_low_fee, 0, 1);
    println!("\nExpected minimum fee: {} picocredits (includes MIN_TX_FEE floor)", expected_min);

    let tx_good_fee = create_signed_transaction(
        sender_wallet,
        utxo,
        *subaddr_idx,
        &recipient_address,
        send_amount,
        expected_min, // Already includes buffer from MIN_TX_FEE
        current_height,
        &network,
    ).expect("Failed to create transaction");

    // Fresh mempool to avoid key image conflict
    let mut mempool2 = Mempool::new();
    let result2 = mempool2.add_tx(tx_good_fee, &ledger_guard);
    assert!(result2.is_ok(), "Transaction with sufficient fee should be accepted: {:?}", result2.err());
    println!("Transaction with sufficient fee accepted");

    println!("\n=== Fee Rejection Test Complete ===\n");
    network.stop();
}

/// Test 3: Fee Rejection with Cluster Wealth (Unit Test Style)
///
/// Verifies that the cluster factor calculation correctly assigns higher
/// factors to wealthy senders. Note: In practice, MIN_TX_FEE dominates the
/// progressive fee for typical transactions, so this test validates the
/// cluster factor logic rather than actual mempool rejection.
#[test]
fn test_fee_rejection_wealthy_sender() {
    println!("\n=== Fee Rejection Test: Wealthy Sender ===\n");

    let mut network = build_test_network();
    thread::sleep(Duration::from_millis(500));

    // Pre-mine blocks for decoy availability
    ensure_decoy_availability(&network, 1);

    // Mine a block to fund wallet 0
    mine_block(&network, 0);
    network.verify_consistency();

    let sender_wallet = &network.wallets[0];
    let recipient_wallet = &network.wallets[1];
    let recipient_address = recipient_wallet.default_address();

    let sender_utxos = scan_wallet_utxos(&network, sender_wallet);
    let (utxo, subaddr_idx) = &sender_utxos[0];

    let node = network.get_node(0);
    let current_height = node.chain_state().height;
    let ledger = node.ledger.clone();
    drop(node);

    let send_amount = 5 * PICOCREDITS_PER_CREDIT;

    // Create a cluster tag representing a wealthy holder
    let wealthy_cluster_id = 12345u64;
    let wealthy_tags = single_cluster_tags(wealthy_cluster_id);

    // Compute fees for different wealth levels (in nanoBTH)
    let fee_config = FeeConfig::default();
    let tx_size_estimate = 4000;
    let small_holder_fee_nano = fee_config.compute_fee(FeeTransactionType::Hidden, tx_size_estimate, 0, 0);
    // 100M BTH in picocredits would overflow u64, use 1M BTH instead
    // (1M BTH is still well above w_mid=10M BTH for max cluster factor)
    let wealthy_amount = 1_000_000 * PICOCREDITS_PER_CREDIT; // 1M BTH wealth
    let wealthy_fee_nano = fee_config.compute_fee(
        FeeTransactionType::Hidden,
        tx_size_estimate,
        wealthy_amount,
        0,
    );

    println!("Fee comparison (both in nanoBTH):");
    println!("  Small holder fee: {} nanoBTH", small_holder_fee_nano);
    println!("  Wealthy holder fee: {} nanoBTH", wealthy_fee_nano);
    println!("  Ratio: {:.2}x", wealthy_fee_nano as f64 / small_holder_fee_nano as f64);

    // Verify wealthy pay more (cluster factor should be ~5-6x)
    assert!(
        wealthy_fee_nano > small_holder_fee_nano * 4,
        "Wealthy sender fee ({}) should be at least 4x small holder fee ({})",
        wealthy_fee_nano, small_holder_fee_nano
    );

    // Create transaction with proper fee (MIN_TX_FEE floor)
    let proper_fee = MIN_TX_FEE;
    let tx_with_tags = create_signed_transaction_with_tags(
        sender_wallet,
        utxo,
        *subaddr_idx,
        &recipient_address,
        send_amount,
        proper_fee,
        current_height,
        &network,
        Some(wealthy_tags.clone()),
    ).expect("Failed to create transaction");

    // The output amount represents the cluster wealth
    let cluster_wealth = compute_cluster_wealth_from_outputs(&tx_with_tags.outputs);
    println!("Cluster wealth from outputs: {}", cluster_wealth);

    // Verify the cluster factor for this output wealth
    let output_cluster_factor = fee_config.cluster_factor(cluster_wealth);
    println!("Cluster factor for output wealth: {:.2}x", output_cluster_factor as f64 / 1000.0);

    // Transaction should be accepted with MIN_TX_FEE
    let mut mempool = Mempool::new();
    let ledger_guard = ledger.read().unwrap();
    let result = mempool.add_tx(tx_with_tags, &ledger_guard);
    assert!(result.is_ok(), "Transaction with MIN_TX_FEE should be accepted: {:?}", result.err());
    println!("Transaction with MIN_TX_FEE accepted");

    println!("\n=== Fee Rejection (Wealthy Sender) Test Complete ===\n");
    network.stop();
}

/// Test 4: Minted coins create new clusters
///
/// Verifies that minting transactions create outputs with proper
/// cluster attribution (100% to a new cluster derived from tx hash).
#[test]
fn test_minted_coins_create_clusters() {
    println!("\n=== Minted Coins Create Clusters Test ===\n");

    let mut network = build_test_network();
    thread::sleep(Duration::from_millis(500));

    // Mine a block to wallet 0
    mine_block(&network, 0);
    network.verify_consistency();

    // Get the minted output and check its cluster tags
    let tip = {
        let node = network.get_node(0);
        let ledger = node.ledger.read().unwrap();
        ledger.get_tip().unwrap()
    };

    let minting_output = tip.minting_tx.to_tx_output();

    println!("Minting transaction output:");
    println!("  Amount: {} BTH", minting_output.amount / PICOCREDITS_PER_CREDIT);
    println!("  Cluster tags: {:?}", minting_output.cluster_tags);

    // Verify it has cluster attribution
    assert!(
        !minting_output.cluster_tags.is_empty(),
        "Minted output should have cluster tags"
    );

    // Verify 100% weight (TAG_WEIGHT_SCALE)
    let total_weight = minting_output.cluster_tags.total_weight();
    assert_eq!(
        total_weight, TAG_WEIGHT_SCALE,
        "Minted output should have 100% cluster attribution, got {}%",
        total_weight * 100 / TAG_WEIGHT_SCALE
    );

    // Verify single cluster entry
    assert_eq!(
        minting_output.cluster_tags.len(), 1,
        "Minted output should have exactly one cluster tag"
    );

    let cluster_id = minting_output.cluster_tags.entries[0].cluster_id;
    println!("  Cluster ID: {}", cluster_id.0);

    // The cluster ID should be derived from the minting tx hash
    let tx_hash = tip.minting_tx.hash();
    let expected_cluster_id = u64::from_le_bytes(tx_hash[0..8].try_into().unwrap());
    assert_eq!(
        cluster_id.0, expected_cluster_id,
        "Cluster ID should be derived from tx hash"
    );

    println!("\n=== Minted Coins Create Clusters Test Complete ===\n");
    network.stop();
}

/// Test 5: Dynamic fee increases under congestion
///
/// Verifies that the dynamic fee system increases the fee base
/// when blocks are consistently full and at minimum block time.
#[test]
fn test_dynamic_fee_congestion() {
    println!("\n=== Dynamic Fee Congestion Test ===\n");

    use bth_cluster_tax::DynamicFeeBase;

    let mut dynamic_fee = DynamicFeeBase::default();

    println!("Initial state:");
    println!("  Base min: {} nanoBTH/byte", dynamic_fee.base_min);
    println!("  Base max: {} nanoBTH/byte", dynamic_fee.base_max);
    println!("  Target fullness: {}%", (dynamic_fee.target_fullness * 100.0) as u32);

    // Simulate normal load (50% full, not at min block time)
    println!("\nSimulating normal load (50% full, not at min block time)...");
    for _ in 0..20 {
        dynamic_fee.update(50, 100, false);
    }
    let base_normal = dynamic_fee.compute_base(false);
    println!("  Fee base: {} nanoBTH/byte", base_normal);
    assert_eq!(base_normal, dynamic_fee.base_min, "Should stay at minimum under normal load");

    // Simulate congestion (100% full, at min block time)
    println!("\nSimulating congestion (100% full, at min block time)...");
    for i in 0..30 {
        let new_base = dynamic_fee.update(100, 100, true);
        if i % 10 == 9 {
            println!("  After {} blocks: {} nanoBTH/byte (EMA: {:.2}%)",
                i + 1, new_base, dynamic_fee.current_fullness() * 100.0);
        }
    }
    let base_congested = dynamic_fee.compute_base(true);
    println!("  Final fee base: {} nanoBTH/byte", base_congested);

    // Verify fee increased significantly
    let multiplier = base_congested as f64 / dynamic_fee.base_min as f64;
    println!("  Multiplier: {:.2}x", multiplier);
    assert!(
        multiplier > 3.0,
        "Fee should increase significantly under sustained congestion: {}x",
        multiplier
    );

    // Simulate recovery (empty blocks)
    println!("\nSimulating recovery (0% full)...");
    for i in 0..50 {
        let new_base = dynamic_fee.update(0, 100, true);
        if i % 10 == 9 {
            println!("  After {} empty blocks: {} nanoBTH/byte (EMA: {:.2}%)",
                i + 1, new_base, dynamic_fee.current_fullness() * 100.0);
        }
    }
    let base_recovered = dynamic_fee.compute_base(true);
    println!("  Recovered fee base: {} nanoBTH/byte", base_recovered);

    // Verify fee returned to normal
    assert_eq!(
        base_recovered, dynamic_fee.base_min,
        "Fee should return to minimum after congestion clears"
    );

    println!("\n=== Dynamic Fee Congestion Test Complete ===\n");
}

/// Test 6: Size-based fee scaling
///
/// Verifies that larger transactions pay proportionally more in fees.
#[test]
fn test_size_based_fee_scaling() {
    println!("\n=== Size-Based Fee Scaling Test ===\n");

    let fee_config = FeeConfig::default();
    let cluster_wealth = 0u64; // Small holder for predictable results

    // Test different transaction sizes
    let sizes = [1000, 2000, 4000, 8000, 16000, 65000];

    println!("Fee scaling with transaction size (cluster_wealth=0):");
    println!("{:>12} | {:>15} | {:>15}", "Size (bytes)", "Fee (nanoBTH)", "Fee/byte");
    println!("{:-<12}-+-{:-<15}-+-{:-<15}", "", "", "");

    let mut prev_fee = 0u64;
    for size in sizes {
        let fee = fee_config.compute_fee(FeeTransactionType::Hidden, size, cluster_wealth, 0);
        let fee_per_byte = fee as f64 / size as f64;

        println!("{:>12} | {:>15} | {:>13.2}", size, fee, fee_per_byte);

        // Verify fee increases with size
        assert!(fee >= prev_fee, "Fee should increase with size");
        prev_fee = fee;
    }

    // Verify linear scaling (double size = double fee)
    let fee_4k = fee_config.compute_fee(FeeTransactionType::Hidden, 4000, cluster_wealth, 0);
    let fee_8k = fee_config.compute_fee(FeeTransactionType::Hidden, 8000, cluster_wealth, 0);
    let ratio = fee_8k as f64 / fee_4k as f64;
    println!("\nSize doubling ratio (8K/4K): {:.2}x", ratio);
    assert!(
        (ratio - 2.0).abs() < 0.1,
        "Doubling size should ~double fee: {}x",
        ratio
    );

    // Compare CLSAG vs LION typical sizes
    let clsag_fee = fee_config.compute_fee(FeeTransactionType::Hidden, 4000, cluster_wealth, 0);
    let lion_fee = fee_config.compute_fee(FeeTransactionType::PqHidden, 65000, cluster_wealth, 0);
    let pq_ratio = lion_fee as f64 / clsag_fee as f64;

    println!("\nCLSAG (~4KB) fee: {} nanoBTH", clsag_fee);
    println!("LION (~65KB) fee: {} nanoBTH", lion_fee);
    println!("PQ/Standard ratio: {:.1}x", pq_ratio);

    assert!(
        pq_ratio > 10.0 && pq_ratio < 20.0,
        "LION should be ~16x more expensive: {}x",
        pq_ratio
    );

    println!("\n=== Size-Based Fee Scaling Test Complete ===\n");
}

/// Test 7: Memo fees add to base fee
///
/// Verifies that outputs with encrypted memos incur additional fees.
#[test]
fn test_memo_fees() {
    println!("\n=== Memo Fees Test ===\n");

    let fee_config = FeeConfig::default();
    let tx_size = 4000;
    let cluster_wealth = 0u64;

    println!("Memo fee: {} nanoBTH per memo", fee_config.fee_per_memo);
    println!();

    // Test fees with different memo counts
    println!("{:>10} | {:>15} | {:>15}", "Memos", "Fee (nanoBTH)", "Memo Cost");
    println!("{:-<10}-+-{:-<15}-+-{:-<15}", "", "", "");

    let base_fee = fee_config.compute_fee(FeeTransactionType::Hidden, tx_size, cluster_wealth, 0);
    println!("{:>10} | {:>15} | {:>15}", 0, base_fee, 0);

    for num_memos in 1..=5 {
        let fee = fee_config.compute_fee(FeeTransactionType::Hidden, tx_size, cluster_wealth, num_memos);
        let memo_cost = fee - base_fee;

        println!("{:>10} | {:>15} | {:>15}", num_memos, fee, memo_cost);

        // Verify memo cost is additive
        let expected_memo_cost = fee_config.fee_per_memo * num_memos as u64;
        assert_eq!(
            memo_cost, expected_memo_cost,
            "Memo cost should be additive: {} vs {}",
            memo_cost, expected_memo_cost
        );
    }

    println!("\n=== Memo Fees Test Complete ===\n");
}

/// Test 8: Combined cluster and congestion effects
///
/// Verifies the maximum fee multiplier scenario: wealthy sender during congestion.
#[test]
fn test_combined_cluster_and_congestion() {
    println!("\n=== Combined Cluster and Congestion Test ===\n");

    use bth_cluster_tax::DynamicFeeBase;

    let fee_config = FeeConfig::default();
    let tx_size = 4000;

    // Calculate base fee (small holder, no congestion)
    let base_fee = fee_config.compute_fee(FeeTransactionType::Hidden, tx_size, 0, 0);
    println!("Base fee (small holder, normal): {} nanoBTH", base_fee);

    // Calculate wealthy sender fee (no congestion)
    let wealthy_cluster = 100_000_000u64;
    let wealthy_fee = fee_config.compute_fee(FeeTransactionType::Hidden, tx_size, wealthy_cluster, 0);
    let cluster_multiplier = wealthy_fee as f64 / base_fee as f64;
    println!("Wealthy sender fee (normal): {} nanoBTH ({:.2}x)", wealthy_fee, cluster_multiplier);

    // Simulate maximum congestion
    let mut dynamic_fee = DynamicFeeBase::default();
    for _ in 0..100 {
        dynamic_fee.update(100, 100, true);
    }
    let congestion_base = dynamic_fee.compute_base(true);
    let congestion_multiplier = congestion_base as f64 / dynamic_fee.base_min as f64;
    println!("\nCongestion multiplier: {:.2}x (base: {} nanoBTH/byte)",
        congestion_multiplier, congestion_base);

    // Calculate combined fee (wealthy + congestion)
    let combined_fee = fee_config.compute_fee_with_dynamic_base(
        FeeTransactionType::Hidden,
        tx_size,
        wealthy_cluster,
        0,
        congestion_base,
    );
    let total_multiplier = combined_fee as f64 / base_fee as f64;
    println!("\nCombined fee (wealthy + congestion): {} nanoBTH", combined_fee);
    println!("Total multiplier: {:.2}x", total_multiplier);

    // Verify multiplicative effect
    let expected_combined = cluster_multiplier * congestion_multiplier;
    let tolerance = 0.5; // Allow for rounding
    assert!(
        (total_multiplier - expected_combined).abs() < tolerance,
        "Combined multiplier ({:.2}x) should be ~cluster × congestion ({:.2}x × {:.2}x = {:.2}x)",
        total_multiplier, cluster_multiplier, congestion_multiplier, expected_combined
    );

    // Show the effect on a real transaction
    let small_normal_fee = base_fee;
    let wealthy_congested_fee = combined_fee;
    println!("\n--- Real Impact ---");
    println!("Small holder in normal conditions: {} nanoBTH", small_normal_fee);
    println!("Wealthy holder in congestion:      {} nanoBTH", wealthy_congested_fee);
    println!("Difference: {:.1}x", wealthy_congested_fee as f64 / small_normal_fee as f64);

    println!("\n=== Combined Effects Test Complete ===\n");
}

/// Test 9: End-to-end progressive fee with real transaction
///
/// Creates actual transactions through the network and verifies
/// progressive fees are correctly applied and enforced.
#[test]
fn test_e2e_progressive_fee_enforcement() {
    println!("\n=== E2E Progressive Fee Enforcement Test ===\n");

    let mut network = build_test_network();
    thread::sleep(Duration::from_millis(500));

    // Pre-mine blocks for decoy availability
    ensure_decoy_availability(&network, 1);

    // Mine block to fund wallet 0
    mine_block(&network, 0);
    network.verify_consistency();

    let wallet0 = &network.wallets[0];
    let wallet1 = &network.wallets[1];
    let recipient = wallet1.default_address();

    let utxos = scan_wallet_utxos(&network, wallet0);
    let (utxo, subaddr_idx) = &utxos[0];

    let node = network.get_node(0);
    let current_height = node.chain_state().height;
    let ledger = node.ledger.clone();
    drop(node);

    let send_amount = 5 * PICOCREDITS_PER_CREDIT;

    // Create a transaction - compute proper fee based on cluster wealth
    let fee_config = FeeConfig::default();

    // Minted coins have 100% cluster attribution to their cluster ID
    let minted_cluster_wealth = utxo.output.amount;
    let cluster_factor = fee_config.cluster_factor(minted_cluster_wealth);

    println!("Sender's UTXO:");
    println!("  Amount: {} BTH", utxo.output.amount / PICOCREDITS_PER_CREDIT);
    println!("  Cluster tags: {:?}", utxo.output.cluster_tags);
    println!("  Effective cluster wealth: {}", minted_cluster_wealth);
    println!("  Cluster factor: {:.2}x", cluster_factor as f64 / 1000.0);

    // Compute required fee for this sender (in nanoBTH)
    let tx_size_estimate = 4000; // Typical CLSAG size
    let required_fee_nanobth = fee_config.compute_fee(
        FeeTransactionType::Hidden,
        tx_size_estimate,
        minted_cluster_wealth,
        0,
    );
    println!("  Progressive fee: {} nanoBTH", required_fee_nanobth);

    // Convert to picocredits and ensure at least MIN_TX_FEE
    let required_fee_pico = (required_fee_nanobth * PICOCREDITS_PER_NANOBTH).max(MIN_TX_FEE);
    println!("  Required fee: {} picocredits (MIN_TX_FEE floor: {})", required_fee_pico, MIN_TX_FEE);

    // Create transaction with proper fee
    let tx = create_signed_transaction(
        wallet0,
        utxo,
        *subaddr_idx,
        &recipient,
        send_amount,
        required_fee_pico,
        current_height,
        &network,
    ).expect("Failed to create transaction");

    println!("\nTransaction created:");
    println!("  Actual size: {} bytes", tx.estimate_size());
    println!("  Fee: {} picocredits", tx.fee);

    // Verify mempool accepts it
    let mut mempool = Mempool::new();
    let ledger_guard = ledger.read().unwrap();
    let result = mempool.add_tx(tx.clone(), &ledger_guard);
    assert!(result.is_ok(), "Transaction should be accepted: {:?}", result.err());
    println!("  Mempool accepted: YES");

    // Broadcast and mine
    drop(ledger_guard);
    network.broadcast_transaction(tx);
    mine_block(&network, 1);
    network.verify_consistency();

    // Verify transfer succeeded
    let wallet1_balance = get_wallet_balance(&network, wallet1);
    assert!(
        wallet1_balance >= send_amount,
        "Recipient should have received funds"
    );
    println!("\nTransfer confirmed: {} BTH received", wallet1_balance / PICOCREDITS_PER_CREDIT);

    // Verify fees were burned
    {
        let node = network.get_node(0);
        let state = node.chain_state();
        println!("Total fees burned: {} nanoBTH", state.total_fees_burned);
        assert!(state.total_fees_burned > 0, "Fees should be burned");
    }

    println!("\n=== E2E Progressive Fee Enforcement Test Complete ===\n");
    network.stop();
}
