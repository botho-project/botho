// Copyright (c) 2024 Botho Foundation

//! Consensus service managing SCP node and message handling.

use super::{validation::TransactionValidator, value::ConsensusValue};
use crate::ledger::ChainState;
use bth_common::NodeID;
use bth_consensus_scp::{
    create_null_logger, msg::Msg as ScpMsg, node::Node, QuorumSet, ScpNode, SlotIndex,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    fmt,
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};
use tracing::{debug, info, instrument, trace, warn, Span};

/// Configuration for the consensus service
#[derive(Debug, Clone)]
pub struct ConsensusConfig {
    /// Slot duration (how often to try to close a slot)
    /// When dynamic_timing is enabled, this serves as the initial/fallback
    /// value
    pub slot_duration: Duration,

    /// Maximum transactions per slot
    pub max_txs_per_slot: usize,

    /// Timeout before re-broadcasting our values
    pub rebroadcast_interval: Duration,

    /// Enable dynamic block timing based on transaction load
    /// When true, slot_duration adjusts based on recent block throughput
    pub dynamic_timing: bool,
}

impl Default for ConsensusConfig {
    fn default() -> Self {
        Self {
            slot_duration: Duration::from_secs(20),
            max_txs_per_slot: 100,
            rebroadcast_interval: Duration::from_secs(5),
            dynamic_timing: true, // Enabled by default
        }
    }
}

impl ConsensusConfig {
    /// Minimum block time in seconds (from dynamic timing levels)
    /// This is the fastest block time the network will use.
    pub const MIN_BLOCK_TIME_SECS: u64 = 3;

    /// Create config with dynamic timing disabled (fixed slot duration)
    pub fn fixed_timing(slot_duration_secs: u64) -> Self {
        Self {
            slot_duration: Duration::from_secs(slot_duration_secs),
            dynamic_timing: false,
            ..Default::default()
        }
    }

    /// Check if a given slot duration is at the minimum (triggers dynamic fee
    /// adjustment)
    pub fn is_at_min_block_time(&self, duration: Duration) -> bool {
        duration.as_secs() <= Self::MIN_BLOCK_TIME_SECS
    }
}

/// Events emitted by the consensus service
#[derive(Debug, Clone)]
pub enum ConsensusEvent {
    /// A slot was externalized with these transaction hashes
    SlotExternalized {
        slot_index: SlotIndex,
        values: Vec<ConsensusValue>,
    },

    /// Need to broadcast an SCP message
    BroadcastMessage(ScpMessage),

    /// Consensus made progress
    Progress {
        slot_index: SlotIndex,
        phase: String,
    },
}

/// Serializable SCP message for gossip
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScpMessage {
    /// Sender node ID (as bytes for serialization)
    pub sender: Vec<u8>,

    /// Slot index
    pub slot_index: SlotIndex,

    /// Serialized message payload
    pub payload: Vec<u8>,
}

/// Transaction cache entry with metadata
#[derive(Clone)]
struct TxCacheEntry {
    /// Serialized transaction bytes
    data: Vec<u8>,
    /// Whether this is a minting transaction
    is_minting_tx: bool,
}

/// Recent block info for dynamic timing calculation
#[derive(Clone, Debug)]
pub struct RecentBlockInfo {
    /// Block timestamp
    pub timestamp: u64,
    /// Number of transactions in the block
    pub tx_count: usize,
}

/// Shared state for validation callbacks
struct SharedValidationState {
    /// Transaction cache (hash -> entry)
    tx_cache: HashMap<[u8; 32], TxCacheEntry>,
    /// Chain state for validation
    chain_state: ChainState,
    /// Recent blocks for dynamic timing (newest last)
    recent_blocks: VecDeque<RecentBlockInfo>,
}

/// The consensus service manages SCP participation
pub struct ConsensusService {
    /// Our node ID
    node_id: NodeID,

    /// Our quorum set
    quorum_set: QuorumSet,

    /// The SCP node
    scp_node: Node<ConsensusValue, String>,

    /// Configuration
    config: ConsensusConfig,

    /// Pending values to propose
    pending_values: BTreeSet<ConsensusValue>,

    /// Values we've already proposed in current slot
    proposed_values: BTreeSet<ConsensusValue>,

    /// Shared validation state (for SCP callbacks)
    shared_state: Arc<RwLock<SharedValidationState>>,

    /// Transaction validator
    validator: TransactionValidator,

    /// Outgoing events
    events: VecDeque<ConsensusEvent>,

    /// Last slot attempt time
    last_slot_attempt: Instant,

    /// Current slot's externalized values (if any)
    externalized: Option<Vec<ConsensusValue>>,
}

impl ConsensusService {
    /// Create a new consensus service
    pub fn new(
        node_id: NodeID,
        quorum_set: QuorumSet,
        config: ConsensusConfig,
        initial_chain_state: ChainState,
    ) -> Self {
        // Capture the initial height before moving chain state
        let initial_height = initial_chain_state.height;

        // Create shared state for validation callbacks
        let shared_state = Arc::new(RwLock::new(SharedValidationState {
            tx_cache: HashMap::new(),
            chain_state: initial_chain_state.clone(),
            recent_blocks: VecDeque::new(),
        }));

        // Create chain state reference for the validator
        let chain_state_ref = Arc::new(RwLock::new(initial_chain_state));
        let validator = TransactionValidator::new(chain_state_ref.clone());

        // Validation callback - validates transactions using shared state
        let validation_state = shared_state.clone();
        let validity_fn: Arc<dyn Fn(&ConsensusValue) -> Result<(), String> + Send + Sync> =
            Arc::new(move |value: &ConsensusValue| {
                let state = validation_state
                    .read()
                    .map_err(|_| "Failed to acquire validation state lock".to_string())?;

                // Look up transaction in cache
                let entry = state.tx_cache.get(&value.tx_hash).ok_or_else(|| {
                    format!("Transaction not in cache: {:?}", &value.tx_hash[0..8])
                })?;

                // Create a temporary validator for this check
                let temp_state = Arc::new(RwLock::new(state.chain_state.clone()));
                let temp_validator = TransactionValidator::new(temp_state);

                // Validate based on transaction type
                temp_validator
                    .validate_from_bytes(&entry.data, entry.is_minting_tx)
                    .map_err(|e| e.to_string())
            });

        // Combine callback - how to combine multiple values
        // Minting transactions are prioritized by PoW difficulty (higher priority =
        // harder PoW) This ensures the "best" minting tx wins if there are
        // multiple
        let combine_fn: Arc<
            dyn Fn(&[ConsensusValue]) -> Result<Vec<ConsensusValue>, String> + Send + Sync,
        > = Arc::new(|values: &[ConsensusValue]| {
            // Separate minting txs from regular txs
            let mut minting_txs: Vec<_> =
                values.iter().filter(|v| v.is_minting_tx).cloned().collect();
            let mut regular_txs: Vec<_> = values
                .iter()
                .filter(|v| !v.is_minting_tx)
                .cloned()
                .collect();

            // Sort minting txs by priority (highest first) - best PoW wins
            minting_txs.sort_by(|a, b| {
                b.priority
                    .cmp(&a.priority)
                    .then_with(|| a.tx_hash.cmp(&b.tx_hash))
            });

            // Only keep the best minting tx (one coinbase per block)
            minting_txs.truncate(1);

            // Sort regular txs by priority (fee), then hash for determinism
            regular_txs.sort_by(|a, b| {
                b.priority
                    .cmp(&a.priority)
                    .then_with(|| a.tx_hash.cmp(&b.tx_hash))
            });

            // Combine: best minting tx first, then regular txs
            let mut combined = minting_txs;
            combined.extend(regular_txs);
            Ok(combined)
        });

        // Create the SCP node, starting at the next block height
        // Slot index corresponds to the block height we're trying to build
        let initial_slot = initial_height + 1;
        debug!(
            slot = initial_slot,
            "Starting consensus at slot (next block height)"
        );

        let scp_node = Node::new(
            node_id.clone(),
            quorum_set.clone(),
            validity_fn,
            combine_fn,
            initial_slot,
            create_null_logger(),
        );

        Self {
            node_id,
            quorum_set,
            scp_node,
            config,
            pending_values: BTreeSet::new(),
            proposed_values: BTreeSet::new(),
            shared_state,
            validator,
            events: VecDeque::new(),
            last_slot_attempt: Instant::now(),
            externalized: None,
        }
    }

    /// Update the chain state (call when chain tip changes)
    pub fn update_chain_state(&mut self, chain_state: ChainState) {
        if let Ok(mut state) = self.shared_state.write() {
            state.chain_state = chain_state;
        }
    }

    /// Record a finalized block for dynamic timing calculation.
    ///
    /// Call this after each block is finalized to update the timing history.
    pub fn record_block(&mut self, timestamp: u64, tx_count: usize) {
        use crate::block::dynamic_timing::SMOOTHING_WINDOW;

        if let Ok(mut state) = self.shared_state.write() {
            state.recent_blocks.push_back(RecentBlockInfo {
                timestamp,
                tx_count,
            });

            // Keep only the last SMOOTHING_WINDOW blocks
            while state.recent_blocks.len() > SMOOTHING_WINDOW {
                state.recent_blocks.pop_front();
            }
        }
    }

    /// Get the current slot duration based on dynamic timing.
    ///
    /// If dynamic timing is disabled, returns the configured fixed duration.
    /// Otherwise, computes based on recent transaction throughput.
    pub fn current_slot_duration(&self) -> Duration {
        if !self.config.dynamic_timing {
            return self.config.slot_duration;
        }

        // Compute dynamic slot time from recent blocks
        let block_time_secs = if let Ok(state) = self.shared_state.read() {
            if state.recent_blocks.len() < 2 {
                // Not enough history, use default
                self.config.slot_duration.as_secs()
            } else {
                // Compute tx rate from recent blocks
                let blocks: Vec<_> = state.recent_blocks.iter().collect();
                let first = blocks.first().unwrap();
                let last = blocks.last().unwrap();
                let window_time = last.timestamp.saturating_sub(first.timestamp);

                if window_time == 0 {
                    self.config.slot_duration.as_secs()
                } else {
                    let total_txs: usize = blocks.iter().map(|b| b.tx_count).sum();
                    let tx_rate = total_txs as f64 / window_time as f64;

                    // Find appropriate level
                    use crate::block::dynamic_timing::BLOCK_TIME_LEVELS;
                    let mut block_time = crate::block::dynamic_timing::MAX_BLOCK_TIME;
                    for (threshold, time) in BLOCK_TIME_LEVELS {
                        if tx_rate >= threshold {
                            block_time = time;
                            break;
                        }
                    }
                    block_time
                }
            }
        } else {
            self.config.slot_duration.as_secs()
        };

        Duration::from_secs(block_time_secs)
    }

    /// Get our node ID
    pub fn node_id(&self) -> &NodeID {
        &self.node_id
    }

    /// Get current slot index
    pub fn current_slot(&self) -> SlotIndex {
        self.scp_node.current_slot_index()
    }

    /// Submit a transaction for consensus
    pub fn submit_transaction(&mut self, tx_hash: [u8; 32], tx_data: Vec<u8>) {
        let value = ConsensusValue::from_transaction(tx_hash);

        // Add to shared cache for validation callback
        if let Ok(mut state) = self.shared_state.write() {
            state.tx_cache.insert(
                tx_hash,
                TxCacheEntry {
                    data: tx_data,
                    is_minting_tx: false,
                },
            );
        }

        self.pending_values.insert(value);
        debug!(?value, "Transaction submitted for consensus");
    }

    /// Submit a minting transaction for consensus
    pub fn submit_minting_tx(&mut self, tx_hash: [u8; 32], pow_priority: u64, tx_data: Vec<u8>) {
        let value = ConsensusValue::from_minting_tx(tx_hash, pow_priority);

        // Add to shared cache for validation callback
        if let Ok(mut state) = self.shared_state.write() {
            state.tx_cache.insert(
                tx_hash,
                TxCacheEntry {
                    data: tx_data,
                    is_minting_tx: true,
                },
            );
        }

        self.pending_values.insert(value);
        info!(?value, "Minting transaction submitted for consensus");
    }

    /// Handle an incoming SCP message from gossip
    #[instrument(
        name = "consensus.handle_message",
        skip(self, msg),
        fields(
            slot_index = msg.slot_index,
            peer_id = %hex::encode(&msg.sender.get(..8).unwrap_or(&msg.sender)),
            msg_size = msg.payload.len(),
        )
    )]
    pub fn handle_message(&mut self, msg: ScpMessage) -> Result<(), String> {
        // Deserialize the SCP message
        let scp_msg: ScpMsg<ConsensusValue> = bincode::deserialize(&msg.payload)
            .map_err(|e| format!("Failed to deserialize SCP message: {}", e))?;

        // Record message type in current span
        let msg_type = crate::telemetry::msg_type_name(&scp_msg.topic);
        Span::current().record("msg_type", msg_type);
        trace!(msg_type, "Processing SCP message");

        // Handle the message
        if let Some(response) = self.scp_node.handle_message(&scp_msg)? {
            self.queue_broadcast(response);
        }

        // Check if slot externalized
        self.check_externalized();

        Ok(())
    }

    /// Process timeouts and periodic tasks
    #[instrument(
        name = "consensus.tick",
        skip(self),
        fields(
            slot_index = self.scp_node.current_slot_index(),
            pending_count = self.pending_values.len(),
        )
    )]
    pub fn tick(&mut self) {
        // Process SCP timeouts
        let timeout_msgs = self.scp_node.process_timeouts();
        if !timeout_msgs.is_empty() {
            trace!(count = timeout_msgs.len(), "Processing SCP timeouts");
            for msg in timeout_msgs {
                self.queue_broadcast(msg);
            }
        }

        // Get current slot duration (dynamic or fixed)
        let slot_duration = self.current_slot_duration();

        // Try to propose values if we have pending ones
        if !self.pending_values.is_empty() && self.last_slot_attempt.elapsed() >= slot_duration {
            self.propose_pending_values();
            self.last_slot_attempt = Instant::now();
        }

        // Check if slot externalized
        self.check_externalized();
    }

    /// Check if we're in solo mining mode (1-of-1 quorum with ourselves)
    fn is_solo_mode(&self) -> bool {
        self.quorum_set.threshold == 1 && self.quorum_set.members.len() == 1
    }

    /// Propose pending values to SCP
    #[instrument(
        name = "consensus.propose_values",
        skip(self),
        fields(
            slot_index = self.scp_node.current_slot_index(),
            value_count = tracing::field::Empty,
            solo_mode = self.is_solo_mode(),
        )
    )]
    fn propose_pending_values(&mut self) {
        if self.pending_values.is_empty() {
            return;
        }

        // Take up to max_txs_per_slot values
        let to_propose: BTreeSet<ConsensusValue> = self
            .pending_values
            .iter()
            .take(self.config.max_txs_per_slot)
            .cloned()
            .collect();

        if to_propose.is_subset(&self.proposed_values) {
            // Already proposed all these
            return;
        }

        Span::current().record("value_count", to_propose.len());

        let slot = self.scp_node.current_slot_index();
        info!(
            slot = slot,
            count = to_propose.len(),
            "Proposing values to SCP"
        );

        // Solo mining mode: bypass SCP and directly externalize
        // This is safe because we're the only validator in a 1-of-1 quorum
        if self.is_solo_mode() {
            let values: Vec<ConsensusValue> = to_propose.iter().cloned().collect();
            info!(
                slot,
                count = values.len(),
                "Solo mode: directly externalizing values"
            );

            // Remove values from pending
            for v in &values {
                self.pending_values.remove(v);
            }

            self.externalized = Some(values.clone());
            self.events.push_back(ConsensusEvent::SlotExternalized {
                slot_index: slot,
                values,
            });
            return;
        }

        // Normal SCP path for multi-node consensus
        match self.scp_node.propose_values(to_propose.clone()) {
            Ok(Some(msg)) => {
                trace!("Proposal accepted, broadcasting message");
                self.proposed_values.extend(to_propose);
                self.queue_broadcast(msg);
            }
            Ok(None) => {
                // No message to send (might be waiting for quorum)
                trace!("Proposal queued, waiting for quorum");
                self.proposed_values.extend(to_propose);
            }
            Err(e) => {
                warn!("Failed to propose values: {}", e);
            }
        }
    }

    /// Check if the current slot has externalized
    #[instrument(
        name = "consensus.check_externalized",
        skip(self),
        fields(
            slot_index = self.scp_node.current_slot_index(),
            externalized = tracing::field::Empty,
            value_count = tracing::field::Empty,
        )
    )]
    fn check_externalized(&mut self) {
        let slot = self.scp_node.current_slot_index();

        if let Some(values) = self.scp_node.get_externalized_values(slot) {
            if self.externalized.is_none() {
                Span::current().record("externalized", true);
                Span::current().record("value_count", values.len());
                info!(slot, count = values.len(), "Slot externalized!");

                // Remove externalized values from pending
                for v in &values {
                    self.pending_values.remove(v);
                    self.proposed_values.remove(v);
                }

                self.externalized = Some(values.clone());

                self.events.push_back(ConsensusEvent::SlotExternalized {
                    slot_index: slot,
                    values,
                });
            }
        } else {
            Span::current().record("externalized", false);
        }
    }

    /// Queue a message for broadcast
    fn queue_broadcast(&mut self, msg: ScpMsg<ConsensusValue>) {
        let payload = bincode::serialize(&msg).expect("Failed to serialize SCP message");

        let scp_msg = ScpMessage {
            sender: bincode::serialize(&self.node_id).unwrap_or_default(),
            slot_index: msg.slot_index,
            payload,
        };

        self.events
            .push_back(ConsensusEvent::BroadcastMessage(scp_msg));
    }

    /// Get the next event (if any)
    pub fn next_event(&mut self) -> Option<ConsensusEvent> {
        self.events.pop_front()
    }

    /// Get externalized values for a slot
    pub fn get_externalized(&self, slot: SlotIndex) -> Option<Vec<ConsensusValue>> {
        self.scp_node.get_externalized_values(slot)
    }

    /// Get cached transaction data
    pub fn get_tx_data(&self, tx_hash: &[u8; 32]) -> Option<Vec<u8>> {
        self.shared_state
            .read()
            .ok()
            .and_then(|state| state.tx_cache.get(tx_hash).map(|e| e.data.clone()))
    }

    /// Get cached transaction entry (data + type info)
    pub fn get_tx_entry(&self, tx_hash: &[u8; 32]) -> Option<(Vec<u8>, bool)> {
        self.shared_state.read().ok().and_then(|state| {
            state
                .tx_cache
                .get(tx_hash)
                .map(|e| (e.data.clone(), e.is_minting_tx))
        })
    }

    /// Advance to next slot (called after processing externalized values)
    pub fn advance_slot(&mut self) {
        // Clean up externalized transactions from cache
        if let Some(ref values) = self.externalized {
            if let Ok(mut state) = self.shared_state.write() {
                for v in values {
                    state.tx_cache.remove(&v.tx_hash);
                }
            }
        }

        self.externalized = None;
        self.proposed_values.clear();

        // For solo mode, we need to explicitly advance the SCP slot
        // since we bypassed the normal SCP externalization path
        if self.is_solo_mode() {
            let next_slot = self.scp_node.current_slot_index() + 1;
            self.scp_node.reset_slot_index(next_slot);
            info!(slot = next_slot, "Advanced to next slot (solo mode)");
        }
        // In multi-node mode, SCP node automatically advances after
        // externalization
    }

    /// Get pending transaction count
    pub fn pending_count(&self) -> usize {
        self.pending_values.len()
    }
}

impl fmt::Debug for ConsensusService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConsensusService")
            .field("node_id", &self.node_id.responder_id)
            .field("slot", &self.scp_node.current_slot_index())
            .field("pending", &self.pending_values.len())
            .finish()
    }
}
