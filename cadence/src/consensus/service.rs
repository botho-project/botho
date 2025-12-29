// Copyright (c) 2024 Cadence Foundation

//! Consensus service managing SCP node and message handling.

use super::value::ConsensusValue;
use mc_common::{logger::create_null_logger, NodeID};
use mc_consensus_scp::{
    msg::Msg as ScpMsg,
    node::Node,
    ScpNode, QuorumSet, SlotIndex,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// Configuration for the consensus service
#[derive(Debug, Clone)]
pub struct ConsensusConfig {
    /// Slot duration (how often to try to close a slot)
    pub slot_duration: Duration,

    /// Maximum transactions per slot
    pub max_txs_per_slot: usize,

    /// Timeout before re-broadcasting our values
    pub rebroadcast_interval: Duration,
}

impl Default for ConsensusConfig {
    fn default() -> Self {
        Self {
            slot_duration: Duration::from_secs(5),
            max_txs_per_slot: 100,
            rebroadcast_interval: Duration::from_secs(2),
        }
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

    /// Transaction data cache (hash -> full tx bytes)
    tx_cache: HashMap<[u8; 32], Vec<u8>>,

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
    ) -> Self {
        // Validation callback - we accept all properly formatted values
        let validity_fn: Arc<dyn Fn(&ConsensusValue) -> Result<(), String> + Send + Sync> =
            Arc::new(|_value: &ConsensusValue| Ok(()));

        // Combine callback - how to combine multiple values (note: takes slice of values, not slice of refs)
        let combine_fn: Arc<dyn Fn(&[ConsensusValue]) -> Result<Vec<ConsensusValue>, String> + Send + Sync> =
            Arc::new(|values: &[ConsensusValue]| {
                // Sort by priority (highest first) then by hash for determinism
                let mut sorted: Vec<_> = values.to_vec();
                sorted.sort_by(|a, b| {
                    b.priority.cmp(&a.priority).then_with(|| a.tx_hash.cmp(&b.tx_hash))
                });
                Ok(sorted)
            });

        // Create the SCP node
        let scp_node = Node::new(
            node_id.clone(),
            quorum_set.clone(),
            validity_fn,
            combine_fn,
            0, // Start at slot 0
            create_null_logger(),
        );

        Self {
            node_id,
            quorum_set,
            scp_node,
            config,
            pending_values: BTreeSet::new(),
            proposed_values: BTreeSet::new(),
            tx_cache: HashMap::new(),
            events: VecDeque::new(),
            last_slot_attempt: Instant::now(),
            externalized: None,
        }
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
        self.tx_cache.insert(tx_hash, tx_data);
        self.pending_values.insert(value);
        debug!(?value, "Transaction submitted for consensus");
    }

    /// Submit a mining transaction for consensus
    pub fn submit_mining_tx(&mut self, tx_hash: [u8; 32], pow_priority: u64, tx_data: Vec<u8>) {
        let value = ConsensusValue::from_mining_tx(tx_hash, pow_priority);
        self.tx_cache.insert(tx_hash, tx_data);
        self.pending_values.insert(value);
        info!(?value, "Mining transaction submitted for consensus");
    }

    /// Handle an incoming SCP message from gossip
    pub fn handle_message(&mut self, msg: ScpMessage) -> Result<(), String> {
        // Deserialize the SCP message
        let scp_msg: ScpMsg<ConsensusValue> = bincode::deserialize(&msg.payload)
            .map_err(|e| format!("Failed to deserialize SCP message: {}", e))?;

        debug!(slot = msg.slot_index, "Received SCP message");

        // Handle the message
        if let Some(response) = self.scp_node.handle_message(&scp_msg)? {
            self.queue_broadcast(response);
        }

        // Check if slot externalized
        self.check_externalized();

        Ok(())
    }

    /// Process timeouts and periodic tasks
    pub fn tick(&mut self) {
        // Process SCP timeouts
        for msg in self.scp_node.process_timeouts() {
            self.queue_broadcast(msg);
        }

        // Try to propose values if we have pending ones
        if !self.pending_values.is_empty()
            && self.last_slot_attempt.elapsed() >= self.config.slot_duration
        {
            self.propose_pending_values();
            self.last_slot_attempt = Instant::now();
        }

        // Check if slot externalized
        self.check_externalized();
    }

    /// Propose pending values to SCP
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

        info!(
            slot = self.scp_node.current_slot_index(),
            count = to_propose.len(),
            "Proposing values to SCP"
        );

        match self.scp_node.propose_values(to_propose.clone()) {
            Ok(Some(msg)) => {
                self.proposed_values.extend(to_propose);
                self.queue_broadcast(msg);
            }
            Ok(None) => {
                // No message to send (might be waiting for quorum)
                self.proposed_values.extend(to_propose);
            }
            Err(e) => {
                warn!("Failed to propose values: {}", e);
            }
        }
    }

    /// Check if the current slot has externalized
    fn check_externalized(&mut self) {
        let slot = self.scp_node.current_slot_index();

        if let Some(values) = self.scp_node.get_externalized_values(slot) {
            if self.externalized.is_none() {
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

        self.events.push_back(ConsensusEvent::BroadcastMessage(scp_msg));
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
    pub fn get_tx_data(&self, tx_hash: &[u8; 32]) -> Option<&Vec<u8>> {
        self.tx_cache.get(tx_hash)
    }

    /// Advance to next slot (called after processing externalized values)
    pub fn advance_slot(&mut self) {
        self.externalized = None;
        self.proposed_values.clear();
        // SCP node automatically advances after externalization
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
