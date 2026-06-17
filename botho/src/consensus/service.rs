// Copyright (c) 2024 Botho Foundation

//! Consensus service managing SCP node and message handling.

use super::{validation::TransactionValidator, value::ConsensusValue};
use crate::ledger::ChainState;
use bth_common::NodeID;
use bth_consensus_scp::{
    create_null_logger, msg::Msg as ScpMsg, node::Node, slot::Phase as ScpPhase, QuorumSet,
    ScpNode, SlotIndex,
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

    /// Highest SCP slot index already surfaced as a `SlotExternalized` event.
    /// Used in the multi-node path so an externalized slot is only emitted once
    /// (the SCP node auto-advances on externalize, so the just-externalized
    /// slot stays readable for several ticks).
    last_externalized_slot: Option<SlotIndex>,

    /// A quorum set reconfiguration that arrived mid-round and was deferred to
    /// the next slot boundary to avoid stranding an in-flight slot.
    pending_quorum_set: Option<QuorumSet>,
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
            last_externalized_slot: None,
            pending_quorum_set: None,
        }
    }

    /// Update the chain state (call when chain tip changes)
    pub fn update_chain_state(&mut self, chain_state: ChainState) {
        if let Ok(mut state) = self.shared_state.write() {
            state.chain_state = chain_state;
        }
    }

    /// Get the current quorum set (for status/RPC/test assertions).
    pub fn quorum_set(&self) -> &QuorumSet {
        &self.quorum_set
    }

    /// Reconfigure the consensus quorum set as peers connect or disconnect.
    ///
    /// This recomputes membership/threshold and toggles solo mode off (or back
    /// on) without requiring a restart. To avoid corrupting an active consensus
    /// round, the change is only applied to the SCP node at a slot boundary:
    ///
    /// - If the current slot has not yet proposed/externalized anything, the
    ///   new quorum set takes effect immediately (the current slot is rebuilt
    ///   at the same index).
    /// - Otherwise the change is stashed and applied on the next
    ///   [`advance_slot`](Self::advance_slot), so an in-flight slot is never
    ///   stranded mid-round.
    ///
    /// Churn that does not change the effective membership/threshold (e.g. a
    /// transient disconnect that immediately reconnects the same peer set) is a
    /// no-op: the new set is compared against both the active set and any
    /// already-pending set before anything is touched.
    ///
    /// Returns `true` if the quorum set changed (applied or deferred).
    pub fn reconfigure_quorum(&mut self, new_quorum_set: QuorumSet) -> bool {
        // Never allow an empty quorum set; that would be unsafe and is almost
        // certainly a wiring bug. Keep the current configuration instead.
        if new_quorum_set.members.is_empty() {
            warn!("Ignoring quorum reconfiguration with empty member set");
            return false;
        }

        // Compare against whatever the node will eventually run with: a pending
        // set (not yet applied) takes precedence over the currently active one.
        let effective = self.pending_quorum_set.as_ref().unwrap_or(&self.quorum_set);
        if effective == &new_quorum_set {
            // No effective change — debounce churn / flapping.
            return false;
        }

        // Determine whether the current slot is at a safe boundary to swap the
        // quorum set without stranding an in-flight round.
        //
        // The local service fields (`proposed_values`, `externalized`) only
        // reflect *this* node's activity for the slot. In multi-node operation,
        // inbound peer nominate/ballot messages accumulate directly inside the
        // SCP node's `current_slot` and are NOT mirrored into those fields. So
        // we must also consult the SCP node's own slot phase: if peers have
        // populated the slot (non-empty nomination sets, a ballot counter past
        // zero, or a phase past the initial NominatePrepare), an "immediate"
        // `set_quorum_set` would silently discard that agreed-upon protocol
        // state. Treat any such slot as in-flight and defer instead.
        //
        // A brand-new slot is in `NominatePrepare` with empty X/Y/Z and
        // `bN == 0`; the empty-nomination-sets + `bN == 0` signal is the robust
        // boundary indicator, with the phase check as a secondary guard.
        let metrics = self.scp_node.get_current_slot_metrics();
        let scp_slot_active = metrics.num_voted_nominated > 0
            || metrics.num_accepted_nominated > 0
            || metrics.num_confirmed_nominated > 0
            || metrics.bN > 0
            || metrics.phase != ScpPhase::NominatePrepare;
        let at_slot_boundary =
            self.proposed_values.is_empty() && self.externalized.is_none() && !scp_slot_active;

        if at_slot_boundary {
            self.apply_quorum_set(new_quorum_set);
        } else {
            info!(
                slot = self.scp_node.current_slot_index(),
                threshold = new_quorum_set.threshold,
                members = new_quorum_set.members.len(),
                "Quorum reconfiguration deferred to next slot boundary (round in flight)"
            );
            self.pending_quorum_set = Some(new_quorum_set);
        }

        true
    }

    /// Apply a new quorum set to both the service and the SCP node, rebuilding
    /// the current slot at its existing index.
    fn apply_quorum_set(&mut self, new_quorum_set: QuorumSet) {
        let was_solo = self.is_solo_mode();
        info!(
            slot = self.scp_node.current_slot_index(),
            threshold = new_quorum_set.threshold,
            members = new_quorum_set.members.len(),
            was_solo,
            "Applying quorum set reconfiguration"
        );
        self.quorum_set = new_quorum_set.clone();
        self.scp_node.set_quorum_set(new_quorum_set);

        if was_solo && !self.is_solo_mode() {
            info!("Exited solo mode: peers present, switching to SCP consensus path");
        } else if !was_solo && self.is_solo_mode() {
            info!("Entered solo mode: no peers, using direct-externalize path");
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

    /// Register a minting transaction received from a peer into the validation
    /// cache, without adding it to our own pending/proposed set.
    ///
    /// SCP messages only carry a value's `tx_hash`. When a peer nominates its
    /// minting tx, every node in the quorum must be able to validate that value
    /// or the SCP slot rejects the peer's message (validity_fn: "Transaction
    /// not in cache") and nomination never reaches quorum. Feeding the raw tx
    /// bytes into the cache lets the local SCP node accept the peer's nominate
    /// votes so balloting can begin. See issue #409.
    pub fn register_minting_tx(&mut self, tx_hash: [u8; 32], tx_data: Vec<u8>) {
        if let Ok(mut state) = self.shared_state.write() {
            state.tx_cache.entry(tx_hash).or_insert(TxCacheEntry {
                data: tx_data,
                is_minting_tx: true,
            });
        }
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

        // Select values ensuring a mix of minting and transfer transactions:
        // - At most 1 minting tx (the highest priority one)
        // - Remaining slots filled with transfer txs
        // This prevents minting txs from crowding out user transactions.
        let mut to_propose: BTreeSet<ConsensusValue> = BTreeSet::new();

        // Find the best minting tx (highest priority)
        let best_minting_tx = self
            .pending_values
            .iter()
            .filter(|v| v.is_minting_tx)
            .max_by_key(|v| v.priority)
            .cloned();

        if let Some(minting_tx) = best_minting_tx {
            to_propose.insert(minting_tx);
        }

        // Fill remaining slots with transfer txs
        let remaining_slots = self
            .config
            .max_txs_per_slot
            .saturating_sub(to_propose.len());
        let transfer_txs: Vec<ConsensusValue> = self
            .pending_values
            .iter()
            .filter(|v| !v.is_minting_tx)
            .take(remaining_slots)
            .cloned()
            .collect();

        to_propose.extend(transfer_txs);

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
                minting_txs = values.iter().filter(|v| v.is_minting_tx).count(),
                transfer_txs = values.iter().filter(|v| !v.is_minting_tx).count(),
                "Solo mode: directly externalizing values"
            );

            // Remove externalized values from pending
            for v in &values {
                self.pending_values.remove(v);
            }

            // Also remove ALL remaining minting txs from pending
            // (we only include the best one per block, others are discarded)
            self.pending_values.retain(|v| !v.is_minting_tx);

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
        // In solo mode the slot is externalized directly in
        // `propose_pending_values` (which sets `self.externalized`), so there is
        // nothing for the SCP-driven path to detect here.
        if self.is_solo_mode() {
            Span::current().record("externalized", false);
            return;
        }

        // Once an externalize is pending application (block build + advance_slot),
        // don't look for another.
        if self.externalized.is_some() {
            Span::current().record("externalized", false);
            return;
        }

        // When the SCP node externalizes a slot it AUTOMATICALLY advances its
        // current slot to `N + 1` and files the just-externalized slot `N` in
        // its `externalized_slots` store (see node_impl::handle_messages ->
        // externalize). So the slot we must read the externalized values from is
        // the one immediately BELOW the current slot index, not the current one.
        //
        // Issue #414: the previous code queried `current_slot_index()` (i.e. the
        // freshly-started slot `N + 1`), which never has externalized values, so
        // `Slot externalized!` never fired and the chain stayed at height 0 even
        // though SCP was reaching the Externalize phase and advancing slots.
        let current = self.scp_node.current_slot_index();
        let Some(highest_externalized) = current.checked_sub(1) else {
            Span::current().record("externalized", false);
            return;
        };

        // Surface externalized slots strictly in order, one per call, so we never
        // skip a slot if the SCP node advanced several slots between ticks. The
        // next slot to surface is the one immediately after the last we emitted.
        let externalized_slot = match self.last_externalized_slot {
            Some(last) => last + 1,
            None => highest_externalized,
        };
        if externalized_slot > highest_externalized {
            Span::current().record("externalized", false);
            return;
        }

        if let Some(values) = self.scp_node.get_externalized_values(externalized_slot) {
            Span::current().record("externalized", true);
            Span::current().record("value_count", values.len());
            info!(
                slot = externalized_slot,
                count = values.len(),
                "Slot externalized!"
            );
            self.last_externalized_slot = Some(externalized_slot);

            // Remove externalized values from pending
            for v in &values {
                self.pending_values.remove(v);
                self.proposed_values.remove(v);
            }

            // Drop ALL pending minting txs: a minting tx is bound to a specific
            // block height / prev-block-hash, so every minting tx proposed for
            // the slot we just externalized is now stale (it builds on the old
            // tip). Leaving them queued makes the node keep re-proposing minting
            // txs with the wrong prev_block_hash, which never validate against
            // the advanced chain and pile up in `pending_values` forever,
            // stalling the next slot. This mirrors the solo-mode path, which
            // already prunes stale minting txs after externalizing.
            self.pending_values.retain(|v| !v.is_minting_tx);

            self.externalized = Some(values.clone());

            self.events.push_back(ConsensusEvent::SlotExternalized {
                slot_index: externalized_slot,
                values,
            });
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
        // since we bypassed the normal SCP externalization path. Capture
        // solo-ness BEFORE applying any pending quorum reconfiguration, since
        // gaining a peer flips us out of solo mode.
        if self.is_solo_mode() {
            let next_slot = self.scp_node.current_slot_index() + 1;
            self.scp_node.reset_slot_index(next_slot);
            info!(slot = next_slot, "Advanced to next slot (solo mode)");
        }
        // In multi-node mode, SCP node automatically advances after
        // externalization.

        // Now that the slot has advanced and no values are proposed for the new
        // slot, it is safe to apply any quorum reconfiguration that arrived
        // mid-round.
        if let Some(pending) = self.pending_quorum_set.take() {
            // Re-check it still differs from the active set (a later churn
            // event may have already converged it).
            if self.quorum_set != pending {
                self.apply_quorum_set(pending);
            }
        }
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::transaction::{
        ClsagRingInput, RingMember, Transaction, TxOutput, MIN_RING_SIZE, MIN_TX_FEE,
    };
    use bth_consensus_scp::{
        ballot::Ballot,
        msg::{Msg, NominatePayload, PreparePayload, Topic},
        QuorumSetMember, ScpNode,
    };
    use bth_consensus_scp_types::test_utils::test_node_id;
    use bth_transaction_types::ClusterTagVector;

    /// A NodeID for each numeric id; node 1 is "us".
    fn node(n: u32) -> NodeID {
        test_node_id(n)
    }

    /// Build a quorum set whose members are the given node ids, using the
    /// recommended BFT threshold n - floor((n-1)/3).
    fn recommended_quorum(ids: &[u32]) -> QuorumSet {
        let n = ids.len();
        let f = n.saturating_sub(1) / 3;
        let threshold = (n - f) as u32;
        let members: Vec<QuorumSetMember<NodeID>> = ids
            .iter()
            .map(|i| QuorumSetMember::Node(node(*i)))
            .collect();
        QuorumSet::new(threshold, members)
    }

    /// A solo (1-of-1) service whose only member is node 1.
    fn solo_service() -> ConsensusService {
        ConsensusService::new(
            node(1),
            QuorumSet::new(1, vec![QuorumSetMember::Node(node(1))]),
            ConsensusConfig::fixed_timing(1),
            ChainState::default(),
        )
    }

    #[test]
    fn boots_in_solo_mode() {
        let svc = solo_service();
        assert!(svc.is_solo_mode(), "1-of-1 quorum must be solo mode");
        assert_eq!(svc.quorum_set().members.len(), 1);
        assert_eq!(svc.quorum_set().threshold, 1);
    }

    /// Acceptance criterion 1: a node that boots solo and later gains a peer
    /// transitions out of solo mode and includes the peer in its quorum set,
    /// without a restart.
    #[test]
    fn solo_to_peered_transition_lifts_solo_mode() {
        let mut svc = solo_service();
        assert!(svc.is_solo_mode());

        // A peer (node 2) connects: 2-of-2 recommended quorum.
        let new_qs = recommended_quorum(&[1, 2]);
        let changed = svc.reconfigure_quorum(new_qs.clone());

        assert!(changed, "gaining a peer must change the quorum set");
        assert!(
            !svc.is_solo_mode(),
            "node must leave solo mode once a peer is present"
        );
        assert_eq!(svc.quorum_set().members.len(), 2);
        assert_eq!(svc.quorum_set(), &new_qs);
        // The SCP node itself must also have the new quorum set.
        assert_eq!(svc.scp_node.quorum_set(), new_qs);
    }

    #[test]
    fn reconfigure_with_same_membership_is_a_noop() {
        let mut svc = solo_service();
        // Reconfiguring to the identical 1-of-1 set is a no-op (churn debounce).
        let same = QuorumSet::new(1, vec![QuorumSetMember::Node(node(1))]);
        assert!(!svc.reconfigure_quorum(same));

        // Add a peer, then re-send the same 2-node set: still a no-op.
        let two = recommended_quorum(&[1, 2]);
        assert!(svc.reconfigure_quorum(two.clone()));
        assert!(!svc.reconfigure_quorum(two));
    }

    #[test]
    fn empty_quorum_set_is_rejected() {
        let mut svc = solo_service();
        let empty = QuorumSet::new(1, vec![]);
        assert!(!svc.reconfigure_quorum(empty));
        // Quorum is unchanged and still solo.
        assert!(svc.is_solo_mode());
    }

    /// Acceptance criterion 2: PeerDisconnected shrinks the quorum sensibly and
    /// does not panic on churn back to solo.
    #[test]
    fn peer_disconnect_shrinks_quorum_without_panic() {
        let mut svc = solo_service();

        // Grow to three nodes, then churn members down repeatedly.
        assert!(svc.reconfigure_quorum(recommended_quorum(&[1, 2, 3])));
        assert!(!svc.is_solo_mode());
        assert_eq!(svc.quorum_set().members.len(), 3);

        // Node 3 leaves -> 2-of-2.
        assert!(svc.reconfigure_quorum(recommended_quorum(&[1, 2])));
        assert!(!svc.is_solo_mode());
        assert_eq!(svc.quorum_set().members.len(), 2);

        // Node 2 leaves -> back to solo. Must not deadlock or panic.
        assert!(svc.reconfigure_quorum(QuorumSet::new(1, vec![QuorumSetMember::Node(node(1))])));
        assert!(svc.is_solo_mode());
        assert_eq!(svc.quorum_set().members.len(), 1);

        // Rapid flapping: connect, disconnect, connect again — no panic.
        for _ in 0..5 {
            svc.reconfigure_quorum(recommended_quorum(&[1, 2]));
            svc.reconfigure_quorum(QuorumSet::new(1, vec![QuorumSetMember::Node(node(1))]));
        }
    }

    /// A reconfiguration that arrives while a slot is in flight must be
    /// deferred to the next slot boundary so the active round is not
    /// stranded.
    #[test]
    fn reconfigure_mid_round_is_deferred_until_advance_slot() {
        let mut svc = solo_service();

        // Drive a solo round: submit a tx and externalize it directly.
        svc.submit_transaction([7u8; 32], vec![1, 2, 3]);
        svc.propose_pending_values();
        assert!(
            svc.externalized.is_some(),
            "solo mode should have externalized the value"
        );

        // A peer connects mid-round. The change is recorded but deferred.
        let two = recommended_quorum(&[1, 2]);
        assert!(svc.reconfigure_quorum(two.clone()));
        assert!(
            svc.is_solo_mode(),
            "active round must keep the old (solo) quorum until the slot boundary"
        );
        assert!(svc.pending_quorum_set.is_some());

        // Advancing the slot applies the deferred quorum set.
        svc.advance_slot();
        assert!(svc.pending_quorum_set.is_none());
        assert!(!svc.is_solo_mode());
        assert_eq!(svc.quorum_set(), &two);
        assert_eq!(svc.scp_node.quorum_set(), two);
    }

    /// A multi-node service whose quorum is the given node ids (node 1 is us).
    fn peered_service(ids: &[u32]) -> ConsensusService {
        ConsensusService::new(
            node(1),
            recommended_quorum(ids),
            ConsensusConfig::fixed_timing(1),
            ChainState::default(),
        )
    }

    /// A structurally valid transfer transaction that passes
    /// `validate_transfer_tx` (one input, one nonzero output, fresh height) so
    /// the SCP validity callback accepts the value it references.
    fn valid_transfer_tx() -> Transaction {
        let ring: Vec<RingMember> = (0..MIN_RING_SIZE)
            .map(|i| RingMember {
                target_key: [i as u8; 32],
                public_key: [(i as u8).wrapping_add(1); 32],
                commitment: [(i as u8).wrapping_add(2); 32],
            })
            .collect();
        let input = ClsagRingInput {
            ring,
            key_image: [9u8; 32],
            commitment_key_image: [109u8; 32],
            clsag_signature: vec![0u8; 32 + 32 * MIN_RING_SIZE],
            pseudo_output_amount: 0,
        };
        let output = TxOutput {
            amount: 1000,
            target_key: [1u8; 32],
            public_key: [2u8; 32],
            e_memo: None,
            cluster_tags: ClusterTagVector::empty(),
        };
        Transaction::new_clsag(vec![input], vec![output], MIN_TX_FEE, 0)
    }

    /// Construct a peer `NominatePrepare` message for `slot_index` in which the
    /// peer has voted and accepted `value`, signed by `peer`.
    fn peer_nominate_prepare(
        peer: NodeID,
        peer_quorum: QuorumSet,
        slot_index: SlotIndex,
        value: ConsensusValue,
    ) -> Msg<ConsensusValue> {
        let mut y = BTreeSet::new();
        y.insert(value.clone());
        let ballot = Ballot::new(1, &[value]);
        Msg::new(
            peer,
            peer_quorum,
            slot_index,
            Topic::NominatePrepare(
                NominatePayload {
                    X: BTreeSet::new(),
                    Y: y,
                },
                PreparePayload {
                    B: ballot.clone(),
                    P: Some(ballot.clone()),
                    PP: None,
                    HN: ballot.N,
                    CN: 0,
                },
            ),
        )
    }

    /// Acceptance criterion: a reconfiguration that arrives while the SCP slot
    /// has accumulated PEER nominate/ballot state (not just local solo state)
    /// must be deferred, so that `set_quorum_set` does not discard the
    /// in-flight round mid-stream.
    ///
    /// This exercises the peer-populated path that the local-only guard
    /// (`proposed_values.is_empty() && externalized.is_none()`) cannot see: the
    /// SCP node's `current_slot` holds peer-driven state even though this node
    /// has proposed nothing and seen no externalization locally.
    #[test]
    fn reconfigure_during_peer_populated_slot_is_deferred() {
        let mut svc = peered_service(&[1, 2]);
        let slot = svc.scp_node.current_slot_index();

        // The peer is nominating a transfer tx. Put a structurally valid,
        // serialized tx in the cache so the SCP validity callback accepts the
        // peer's value, mirroring real operation where the tx is gossiped
        // before the SCP message references it.
        let tx = valid_transfer_tx();
        let tx_bytes = bincode::serialize(&tx).expect("tx serializes");
        let tx_hash = [7u8; 32];
        if let Ok(mut state) = svc.shared_state.write() {
            state.tx_cache.insert(
                tx_hash,
                TxCacheEntry {
                    data: tx_bytes,
                    is_minting_tx: false,
                },
            );
        }
        let value = ConsensusValue::from_transaction(tx_hash);

        // Feed an inbound peer message directly into the SCP node so its
        // `current_slot` accumulates peer-driven nominate/ballot state.
        let peer_msg = peer_nominate_prepare(node(2), recommended_quorum(&[1, 2]), slot, value);
        let _ = svc
            .scp_node
            .handle_message(&peer_msg)
            .expect("peer message should be accepted");

        // Sanity: the local-only guard would think the slot is idle, but the
        // SCP node's own metrics show peer-driven state.
        assert!(
            svc.proposed_values.is_empty() && svc.externalized.is_none(),
            "local guard fields must still look idle for this to be a real test"
        );
        let metrics = svc.scp_node.get_current_slot_metrics();
        let scp_active = metrics.num_voted_nominated > 0
            || metrics.num_accepted_nominated > 0
            || metrics.num_confirmed_nominated > 0
            || metrics.bN > 0
            || metrics.phase != ScpPhase::NominatePrepare;
        assert!(
            scp_active,
            "peer message must leave the SCP slot in an in-flight state: \
             X={} Y={} Z={} bN={} phase={:?}",
            metrics.num_voted_nominated,
            metrics.num_accepted_nominated,
            metrics.num_confirmed_nominated,
            metrics.bN,
            metrics.phase
        );

        // A membership change now must be DEFERRED, not applied, so the
        // peer-populated round is not discarded by `set_quorum_set`.
        let active_before = svc.scp_node.quorum_set();
        let new_qs = recommended_quorum(&[1, 2, 3]);
        assert!(svc.reconfigure_quorum(new_qs.clone()));
        assert!(
            svc.pending_quorum_set.is_some(),
            "reconfiguration during a peer-populated slot must be deferred"
        );
        assert_eq!(
            svc.quorum_set(),
            &recommended_quorum(&[1, 2]),
            "active service quorum set must be unchanged while deferred"
        );
        assert_eq!(
            svc.scp_node.quorum_set(),
            active_before,
            "SCP node quorum set must be unchanged (set_quorum_set not invoked)"
        );

        // The peer-driven slot state must survive the deferral untouched.
        let after = svc.scp_node.get_current_slot_metrics();
        assert_eq!(
            (
                after.num_voted_nominated,
                after.num_accepted_nominated,
                after.bN,
                after.phase
            ),
            (
                metrics.num_voted_nominated,
                metrics.num_accepted_nominated,
                metrics.bN,
                metrics.phase
            ),
            "deferral must not disturb the in-flight SCP slot state"
        );

        // Advancing the slot finally applies the deferred reconfiguration.
        svc.advance_slot();
        assert!(svc.pending_quorum_set.is_none());
        assert_eq!(svc.quorum_set(), &new_qs);
        assert_eq!(svc.scp_node.quorum_set(), new_qs);
    }

    /// Regression: a genuinely idle/fresh slot (no peer state, nothing locally
    /// proposed) must still take the immediate-apply fast path.
    #[test]
    fn reconfigure_on_idle_slot_applies_immediately() {
        let mut svc = peered_service(&[1, 2]);

        // Fresh slot: no local proposal, no externalization, no peer messages.
        assert!(svc.proposed_values.is_empty() && svc.externalized.is_none());
        let metrics = svc.scp_node.get_current_slot_metrics();
        assert_eq!(metrics.num_voted_nominated, 0);
        assert_eq!(metrics.num_accepted_nominated, 0);
        assert_eq!(metrics.bN, 0);
        assert_eq!(metrics.phase, ScpPhase::NominatePrepare);

        // Membership change on an idle slot applies immediately (no deferral).
        let new_qs = recommended_quorum(&[1, 2, 3]);
        assert!(svc.reconfigure_quorum(new_qs.clone()));
        assert!(
            svc.pending_quorum_set.is_none(),
            "idle slot must apply the reconfiguration immediately, not defer"
        );
        assert_eq!(svc.quorum_set(), &new_qs);
        assert_eq!(svc.scp_node.quorum_set(), new_qs);
    }
}
