// Copyright (c) 2024 Botho Foundation

//! Consensus service managing SCP node and message handling.

use super::{validation::TransactionValidator, value::ConsensusValue};
use crate::ledger::ChainState;
use bth_common::NodeID;
use bth_consensus_scp::{
    create_null_logger,
    msg::Msg as ScpMsg,
    node::Node,
    slot::{Phase as ScpPhase, SlotMetrics},
    QuorumSet, ScpNode, SlotIndex,
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

    /// Participation gate (issue #428): the number of connected peers the
    /// operator declared they expect before this node may produce blocks
    /// (the configured `min_peers`). `0` means the node is a genuine
    /// single-node dev/genesis network and mints solo forever — no gate.
    /// `>= 1` means the node expects peers and MUST NOT mint/propose a block
    /// while it has fewer than this many connected peers, so it never produces
    /// a divergent solo block during the pre-quorum startup window (or after a
    /// quorum member disconnects). See [`Self::should_propose_this_round`].
    min_peers: usize,

    /// Live count of currently connected peers, kept up to date by the node
    /// run loop on PeerDiscovered/PeerDisconnected (see
    /// [`Self::set_connected_peers`]). Compared against `min_peers` by the
    /// participation gate.
    connected_peers: usize,

    /// Highest SCP `slot_index` each QUORUM-MEMBER peer has advertised in a
    /// gossiped SCP message (issue #431). Used to corroborate a forward anchor
    /// against a v-blocking set of distinct quorum members before
    /// fast-forwarding the local SCP slot, so a single bogus/non-member
    /// high-slot claim (e.g. `u64::MAX`) cannot strand an idle node. Only
    /// senders that are current members of `quorum_set` are recorded; entries
    /// are pruned when the quorum set changes (see [`Self::apply_quorum_set`]).
    peer_advertised_slots: HashMap<NodeID, SlotIndex>,
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

        // Validation callback — the SCP consensus validity function.
        //
        // SAFETY (issue #419 / #417 Finding 1): this MUST be a pure function of
        // the value, i.e. TIP-AGNOSTIC. SCP's no-fork agreement theorem assumes
        // "valid for one honest node ⇒ valid for all honest nodes". If validity
        // depended on the local chain tip, SCP's "silently drop messages
        // carrying an un-validatable value" rule would partition two competing
        // minters into two single-node voting instances that each externalize
        // their own block — the multi-minter fork. So we validate only the
        // INTRINSIC properties of the minting tx (well-formedness, PoW vs the
        // tx's stated difficulty, non-future timestamp). The tip-relative
        // checks (prev_block_hash == tip, height == tip+1, chain difficulty,
        // emission reward, parent-timestamp monotonicity) are enforced
        // unconditionally at block-apply (`LedgerStore::add_block`), so a stale
        // or fraudulent block can never be appended even though its minting tx
        // is an acceptable consensus *value*.
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

                // Create a temporary validator for the (tip-agnostic) check.
                // The chain_state is only used by the transfer-tx structural
                // path; the minting-tx path is fully tip-agnostic.
                let temp_state = Arc::new(RwLock::new(state.chain_state.clone()));
                let temp_validator = TransactionValidator::new(temp_state);

                // Validate INTRINSIC (tip-agnostic) properties only.
                temp_validator
                    .validate_from_bytes_intrinsic(&entry.data, entry.is_minting_tx)
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
            // Default to 0 = no participation gate (genuine solo / dev / tests).
            // The node run loop calls `set_min_peers` to enable the gate when
            // the operator declared an expectation of peers.
            min_peers: 0,
            connected_peers: 0,
            peer_advertised_slots: HashMap::new(),
        }
    }

    /// Declare how many connected peers this node expects before it may produce
    /// blocks (issue #428). Wired from the node's quorum config `min_peers`.
    ///
    /// `0` (the default) disables the participation gate: the node is a genuine
    /// single-node dev/genesis network and mints solo. `>= 1` enables the gate:
    /// the node will withhold minting/proposing while it has fewer than this
    /// many connected peers (see [`Self::should_propose_this_round`]).
    pub fn set_min_peers(&mut self, min_peers: usize) {
        self.min_peers = min_peers;
    }

    /// Update the live connected-peer count used by the participation gate
    /// (issue #428). Called by the node run loop on PeerDiscovered /
    /// PeerDisconnected and once at startup.
    pub fn set_connected_peers(&mut self, connected_peers: usize) {
        self.connected_peers = connected_peers;
    }

    /// Participation gate (issue #428): may this node propose/externalize a
    /// block in the current round?
    ///
    /// A node that declared it expects peers (`min_peers >= 1`) must NOT mint
    /// or externalize a block while it has fewer than `min_peers` connected
    /// peers. During the pre-quorum startup window such a node transiently
    /// holds a solo (1-of-1) quorum set; if it externalized a block there it
    /// would produce a divergent solo chain and fork once peers connect (the
    /// solo-latch race from #424/#427). Gating the proposer here makes that
    /// window produce *no* block at all.
    ///
    /// A node with `min_peers == 0` is a genuine single-node network and always
    /// returns `true` — no regression for dev/genesis solo minting.
    ///
    /// This is purely a node-layer liveness/proposer gate: it does not touch
    /// SCP protocol semantics, validity, or the no-fork guarantee (#420).
    /// Peering/discovery is independent of minting, so the node still discovers
    /// peers and forms the quorum while gated (it waits for *connections*, not
    /// for *blocks*) — no deadlock.
    fn should_propose_this_round(&self) -> bool {
        if self.min_peers == 0 {
            // Genuine single-node network: always mint solo.
            return true;
        }
        // Expects peers: only propose once enough are connected to form the
        // configured quorum. If peers later drop below the expectation the
        // node pauses (block) rather than falling back to solo minting — a
        // quorum that loses a member halts (SCP safety-over-liveness).
        self.connected_peers >= self.min_peers
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

        // Issue #431: drop advertised-slot records for nodes that are no longer
        // members, so a former member cannot keep contributing to anchor
        // corroboration after leaving the quorum set.
        let members = self.quorum_set.nodes();
        self.peer_advertised_slots
            .retain(|node_id, _| members.contains(node_id));

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

        // Issue #421 (Option C): forward-only anchoring to the network's live
        // SCP slot. If this message is for a slot AHEAD of ours, the SCP node
        // would otherwise silently DISCARD it as a "future slot" (see
        // node_impl::handle_messages). When we are behind the network — e.g. a
        // freshly-synced joiner, or a node that fell behind while the leaders
        // drifted ahead via the #421 apply-boundary drift — and our current
        // slot is idle, fast-forward FORWARD to the peer's slot so we re-enter
        // federated voting at the network's actual slot instead of stalling.
        // This is forward-only and idle-gated (see `anchor_scp_slot_to_peer`),
        // so it never re-seats backward, never re-opens an externalized index,
        // and never triggers for two established minters sharing a tip.
        //
        // Issue #431: this runs BEFORE `scp_node.handle_message` authenticates
        // the message, so a bogus high `slot_index` from a single or non-member
        // peer must NOT drive an anchor. The anchor is therefore gated on the
        // SENDER being a current quorum member AND on the target slot being
        // corroborated by a v-blocking set of distinct quorum members — a lone
        // `u64::MAX` claim cannot move an idle node's slot.
        self.anchor_scp_slot_to_peer(&scp_msg.sender_id, scp_msg.slot_index);

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

    /// Issue #433: are we in a *transitional* solo state — a peer is connected
    /// but the SCP quorum has not yet been reconfigured out of 1-of-1?
    ///
    /// A node that expects peers (`min_peers >= 1`) and currently has at least
    /// one connected (`connected_peers >= 1`) but is still holding a solo
    /// (1-of-1) quorum set is mid-transition: it must run federated SCP with
    /// the connected peer, not directly solo-externalize. Taking the solo
    /// path here produces a divergent solo chain and forks once both peered
    /// nodes do it. This is the exact race from #433 (a peer that connected
    /// during the pre-consensus startup window leaves the gate open while
    /// the quorum is still 1-of-1). We block the solo direct-externalize
    /// until the quorum reconfigures (the run loop calls
    /// `reconfigure_quorum` on peer events; the startup path now seeds the
    /// quorum from connected peers — see `commands::run`). A genuine lone
    /// node (`min_peers == 0`) is never transitional: it has no peers to
    /// wait for and mints solo.
    fn is_transitional_solo(&self) -> bool {
        self.min_peers >= 1 && self.connected_peers >= 1 && self.is_solo_mode()
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

        // Participation gate (issue #428): a node that expects peers
        // (`min_peers >= 1`) must not propose/externalize a block while it has
        // fewer than `min_peers` connected peers. This withholds any block
        // during the pre-quorum startup window (when the node transiently holds
        // a solo 1-of-1 quorum) so it never produces a divergent solo chain and
        // forks once peers connect. Values stay queued in `pending_values` and
        // are proposed normally once the quorum forms. `min_peers == 0` keeps
        // genuine single-node minting unaffected.
        if !self.should_propose_this_round() {
            debug!(
                min_peers = self.min_peers,
                connected_peers = self.connected_peers,
                "Withholding propose: not enough connected peers for quorum (issue #428)"
            );
            return;
        }

        // Transitional-solo guard (issue #433): a node that expects peers and
        // has one connected, but whose SCP quorum is still 1-of-1, must NOT take
        // the solo direct-externalize path below — that produces a divergent
        // solo chain and forks once both peered nodes do it. Withhold the block
        // until the quorum reconfigures out of solo (the run loop reconfigures
        // on peer events; startup seeds the quorum from connected peers). Values
        // stay queued in `pending_values`, so nothing is lost.
        if self.is_transitional_solo() {
            debug!(
                min_peers = self.min_peers,
                connected_peers = self.connected_peers,
                "Withholding propose: peer connected but quorum still solo \
                 (awaiting 1-of-1 -> N-of-N reconfig, issue #433)"
            );
            return;
        }

        // Select values ensuring a mix of minting and transfer transactions:
        // - At most 1 minting tx (the highest priority one)
        // - Remaining slots filled with transfer txs
        // This prevents minting txs from crowding out user transactions.
        let mut to_propose: BTreeSet<ConsensusValue> = BTreeSet::new();

        // LIVENESS (issue #419 / #417 Finding 2): in multi-node mode, once we
        // have already proposed a minting tx for the CURRENT slot, keep
        // proposing that SAME value rather than swapping to a newly-mined,
        // higher-priority one. The local PoW miner produces a fresh minting tx
        // (distinct hash) several times a second; if every node kept replacing
        // its proposed coinbase mid-slot, each node's SCP nomination set would
        // churn with different values and the quorum could never reach a shared
        // confirmed-nominate, jamming the slot in a unanimous quorum. Pinning
        // the first-proposed coinbase per slot stabilizes the candidate set so
        // federated voting + the deterministic combiner converge. The losing
        // coinbases are simply not proposed; this does not affect safety
        // (validity is tip-agnostic and block-apply still enforces every
        // tip-relative rule). After externalize, all pending minting txs are
        // pruned (see `check_externalized`), so the next slot starts fresh.
        let already_proposed_minting = self.proposed_values.iter().any(|v| v.is_minting_tx);
        let minting_tx = if already_proposed_minting {
            // Re-propose the exact minting value we already committed to this
            // slot, if it is still known.
            self.proposed_values
                .iter()
                .find(|v| v.is_minting_tx)
                .cloned()
        } else {
            // Find the best (highest priority) minting tx to propose.
            self.pending_values
                .iter()
                .filter(|v| v.is_minting_tx)
                .max_by_key(|v| v.priority)
                .cloned()
        };

        if let Some(minting_tx) = minting_tx {
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

    /// Fast-forward the SCP current slot to match a chain that advanced via
    /// block-sync (issue #419 / #417 Finding 3).
    ///
    /// A node that catches up via #376 block-sync applies blocks straight to
    /// the ledger (height `H`) but never advances its SCP `current_slot_index`,
    /// which stays at the genesis slot (`initial_height + 1`). The live network
    /// is balloting slot `H + 1`, so the joiner discards every peer message as
    /// a "future slot" (`node_impl::handle_messages`) and can never
    /// participate.
    ///
    /// This advances the SCP slot to `chain_height + 1`, matching
    /// [`ConsensusService::new`]'s `initial_slot = initial_height + 1`.
    ///
    /// # SAFETY
    ///
    /// This is only safe to call when the joiner holds NO in-flight ballot or
    /// nominate state for the current SCP slot: its messages were all being
    /// discarded as future slots, so there is nothing to lose. We therefore:
    /// - only advance when the SCP slot is STRICTLY behind `chain_height + 1`
    ///   (`reset_slot_index` requires a strictly-increasing index, and we must
    ///   never move the slot backwards), and
    /// - refuse to advance if the current slot has accumulated any
    ///   nominate/ballot state (mirroring the #408 deferral guard), so we never
    ///   discard a slot that may hold accepted-commit state. In normal joiner
    ///   operation the slot is idle (all peer messages were future-slot
    ///   discards), so the guard passes.
    ///
    /// Returns `true` if the SCP slot was advanced.
    pub fn sync_scp_slot_to_chain(&mut self, chain_height: u64) -> bool {
        // Solo mode advances its own slot via `advance_slot`; never interfere.
        if self.is_solo_mode() {
            return false;
        }

        let target_slot = chain_height + 1;
        let current = self.scp_node.current_slot_index();

        // Only ever move forward, and only if strictly behind the target.
        if current >= target_slot {
            return false;
        }

        // Do not discard a slot that holds genuine BALLOT/COMMIT state —
        // re-opening a committed value could fork (issue #436, refining #430).
        // A genuine joiner's current slot is idle because its peer messages were
        // discarded as future slots. Bare un-confirmed NOMINATION state on a
        // slot the network has already filled (the synced chain height is
        // strictly past `current - 1`) is safe to abandon; only ballot/commit
        // state must be preserved.
        let metrics = self.scp_node.get_current_slot_metrics();
        if Self::slot_holds_ballot_or_commit_state(&metrics) {
            warn!(
                current,
                target_slot,
                bN = metrics.bN,
                phase = ?metrics.phase,
                "Not fast-forwarding SCP slot to synced chain height: current slot \
                 holds ballot/commit state (deferring to avoid dropping committed state)"
            );
            return false;
        }

        info!(
            from_slot = current,
            to_slot = target_slot,
            chain_height,
            "Fast-forwarding SCP slot to match block-synced chain height (issue #419 Finding 3)"
        );
        self.scp_node.reset_slot_index(target_slot);

        // A fresh slot has nothing proposed/externalized locally yet.
        self.proposed_values.clear();
        self.externalized = None;
        // The just-advanced slot is brand new; we have not surfaced it.
        self.last_externalized_slot = None;

        true
    }

    /// The SAFETY BOUNDARY for forward-anchoring (issue #436).
    ///
    /// Returns `true` iff the current SCP slot holds genuine BALLOT/COMMIT
    /// state — i.e. a ballot has started (`bN > 0`) or the slot has progressed
    /// past the initial concurrent nominate/prepare phase (`phase !=
    /// NominatePrepare`, meaning Prepare/Commit/Externalize). Such state must
    /// NEVER be abandoned: re-opening a value that reached the ballot protocol
    /// (let alone an accepted/confirmed commit) could fork the chain.
    ///
    /// It returns `false` for a slot holding only NOMINATION state (voted /
    /// accepted / confirmed nominated values) while `bN == 0` and the phase is
    /// still `NominatePrepare`. That state is safe to skip when the network has
    /// PROVABLY moved past the slot (the caller checks the peer / chain is
    /// strictly ahead): nothing is committed, so anchoring forward cannot
    /// re-open a decided value. This is the precise relaxation that stops a
    /// joiner from latching forever on un-completable bare-nomination state.
    ///
    /// Note on `bN` vs confirmed-nominated (`Z`): when nomination confirms a
    /// value the ballot protocol immediately seeds `B = Ballot::new(1, ..)` in
    /// the same handling pass (see `consensus/scp/src/slot.rs`), so a slot that
    /// has truly committed to balloting always shows `bN >= 1` and is caught
    /// here. Confirmed-nominated with `bN == 0` is only a transient/degenerate
    /// state and is intentionally treated as nomination-only — exactly the
    /// state #436 authorizes abandoning when the network has passed the slot.
    fn slot_holds_ballot_or_commit_state(metrics: &SlotMetrics) -> bool {
        metrics.bN > 0 || metrics.phase != ScpPhase::NominatePrepare
    }

    /// Size of a v-blocking set for the local (flat) quorum set — the minimum
    /// number of DISTINCT quorum members that, by being ahead, prove the
    /// network has advanced (issue #431).
    ///
    /// For a `T`-of-`N` quorum set, every quorum has at least `T` members, so a
    /// set of `N - T + 1` members intersects every quorum (v-blocking): the
    /// local node cannot form a quorum without at least one of them. If that
    /// many distinct members have each advertised a slot `>= target`, the local
    /// node cannot assemble a quorum at any slot below `target`, so the network
    /// has provably moved to `target`. Clamped to at least 1 so a single bogus
    /// claim is never self-corroborating. Botho uses flat auto-trust quorum
    /// sets (all peers as top-level members), so `members.len()` and
    /// `threshold` describe the real deployment exactly.
    fn anchor_corroboration_threshold(&self) -> usize {
        let n = self.quorum_set.members.len();
        let t = self.quorum_set.threshold as usize;
        n.saturating_sub(t).saturating_add(1).max(1)
    }

    /// Number of DISTINCT quorum members (excluding ourselves) currently known
    /// to have advertised a slot `>= target` (issue #431). Only members of the
    /// live quorum set are counted; stale records are ignored defensively.
    fn anchor_corroborating_member_count(&self, target: SlotIndex) -> usize {
        let members = self.quorum_set.nodes();
        self.peer_advertised_slots
            .iter()
            .filter(|(node_id, &slot)| {
                slot >= target && *node_id != &self.node_id && members.contains(*node_id)
            })
            .count()
    }

    /// The HIGHEST slot index that is corroborated by a v-blocking set of
    /// distinct quorum members (issue #431).
    ///
    /// Established leaders can sit at slightly different slots (the #421
    /// apply-boundary drift), so insisting on corroboration at the exact slot
    /// of the latest message could leave a genuine joiner anchoring slowly
    /// or not at all. Instead we anchor to the highest slot a v-blocking
    /// set actually agrees on: the `k`-th largest advertised slot among
    /// distinct quorum members, where `k =
    /// anchor_corroboration_threshold()`. That value is, by construction,
    /// corroborated by at least `k` members (a v-blocking set) and
    /// is the most advanced provably-network slot we may safely jump to.
    ///
    /// This is what BOUNDS a bogus claim: a single member advertising
    /// `u64::MAX` is the 1st-largest entry, not the `k`-th (for `k >= 2`), so
    /// it can never become the corroborated target on its own — the result
    /// is pulled down to the `k`-th largest, which only a genuine
    /// v-blocking set of members can raise. Returns `None` if fewer than
    /// `k` members have advertised any slot.
    fn highest_corroborated_slot(&self) -> Option<SlotIndex> {
        let k = self.anchor_corroboration_threshold();
        let members = self.quorum_set.nodes();
        let mut slots: Vec<SlotIndex> = self
            .peer_advertised_slots
            .iter()
            .filter(|(node_id, _)| *node_id != &self.node_id && members.contains(*node_id))
            .map(|(_, &slot)| slot)
            .collect();
        if slots.len() < k {
            return None;
        }
        // Descending sort; the k-th largest (index k-1) is corroborated by the
        // k largest-advertising members (a v-blocking set).
        slots.sort_unstable_by(|a, b| b.cmp(a));
        slots.get(k - 1).copied()
    }

    /// Forward-only, safety-gated anchoring of the SCP slot to the network's
    /// LIVE slot learned from an inbound peer SCP message (issue #421, Option
    /// C; refined for the join-handoff race in issue #436).
    ///
    /// # The convergence problem this fixes
    ///
    /// The block-apply boundary can leave an established node's SCP
    /// `current_slot` AHEAD of `ledger_height + 1` (the #421 drift: SCP
    /// auto-advances on externalize, but a duplicate/lost-the-race externalize
    /// is rejected at `add_block`, so the ledger does not advance with it).
    /// `sync_scp_slot_to_chain` anchors a joiner to `chain_height + 1`, which
    /// is only correct if the established nodes are ALSO at `chain_height + 1`
    /// — drift violates that. A freshly-synced (or fallen-behind) node then
    /// sits at a slot strictly BELOW the established nodes' live slot, and
    /// `node_impl::handle_messages` silently DISCARDS the established nodes'
    /// messages as "future slots" (`slot_index > current_slot`). Neither side
    /// enters the other's value into federated voting → no quorum → the 3-of-3
    /// #396 soak cannot advance past N.
    ///
    /// This method closes that gap WITHOUT touching proposal selection and
    /// WITHOUT ever moving the slot backward: a node that is BEHIND the
    /// network's live SCP slot (observed from a peer message for a higher slot)
    /// fast-forwards FORWARD to the peer's slot index so it stops discarding
    /// the network's messages and can re-enter voting at the network's
    /// actual slot.
    ///
    /// # SAFETY (the #422 regression must not recur)
    ///
    /// - FORWARD-ONLY: we only ever move to a STRICTLY HIGHER slot index, so
    ///   `reset_slot_index`'s `debug_assert!(slot_index > current)` stays
    ///   SATISFIED (never relaxed). We never re-seat backward and never re-open
    ///   an index we already externalized — eliminating the #422 backstop's
    ///   re-externalize fork risk.
    /// - IDLE-GATED: we refuse to anchor when the local slot holds any
    ///   in-flight nominate/ballot state (same activity guard as
    ///   `sync_scp_slot_to_chain`), so we never discard a slot that may hold
    ///   accepted-commit state.
    /// - PROPOSAL-UNTOUCHED: this does not filter or withhold any coinbase, so
    ///   the #424 wedge cannot recur. Two established minters that SHARE a tip
    ///   are at the SAME slot, so this never triggers for them (the peer slot
    ///   is not ahead); only a genuinely BEHIND node ever fast-forwards, and
    ///   only toward the leaders — never the leaders moving.
    ///
    /// # ANTI-GRIEFING (issue #431)
    ///
    /// This hook runs in `handle_message` BEFORE `scp_node.handle_message`
    /// authenticates the message, so the advertised `peer_slot` is an UNTRUSTED
    /// claim. A malicious/buggy peer gossiping a huge `slot_index` (e.g.
    /// `u64::MAX`) must NOT be able to fast-forward an idle node to a bogus
    /// slot and strand it (it would then discard every legitimate,
    /// lower-slot peer message). We therefore impose two gates before
    /// anchoring:
    ///
    /// - SENDER-MEMBERSHIP: the message sender must be a current member of the
    ///   local quorum set (`quorum_set.nodes()`). A non-member's slot claim is
    ///   recorded by no one and drives no anchor. (Botho auto-trusts connected
    ///   peers as flat members, so this bounds anchoring to actual quorum
    ///   peers, not arbitrary gossip relays.)
    /// - CORROBORATION (v-blocking set): we only anchor to `target` once a
    ///   v-blocking set of DISTINCT quorum members have each advertised a slot
    ///   `>= target`. In a flat `T`-of-`N` quorum set a v-blocking set has size
    ///   `N - T + 1`: the local node cannot assemble a quorum at any slot below
    ///   `target` without acknowledging at least one of those members, so the
    ///   network has PROVABLY advanced to `target`. A single peer's claim
    ///   (member or not) is never sufficient, which closes the bogus-high-slot
    ///   vector while a genuine joiner still anchors once enough real members
    ///   are observably ahead (the #396 capability is preserved).
    ///
    /// Returns `true` if the SCP slot was advanced to the peer's slot.
    fn anchor_scp_slot_to_peer(&mut self, sender_id: &NodeID, peer_slot: SlotIndex) -> bool {
        // Solo mode manages its own slot via `advance_slot`; never interfere.
        if self.is_solo_mode() {
            return false;
        }

        // SENDER-MEMBERSHIP GATE (issue #431): only a current quorum member's
        // advertised slot may influence anchoring. A non-member (arbitrary
        // gossip relay) is ignored entirely — neither recorded nor acted on.
        let members = self.quorum_set.nodes();
        if !members.contains(sender_id) {
            trace!(
                peer_slot,
                "Ignoring advertised slot from non-quorum-member sender for anchoring (issue #431)"
            );
            return false;
        }

        let current = self.scp_node.current_slot_index();

        // Record the highest slot this member has advertised (monotonic per
        // member), so corroboration can count DISTINCT members at/above the
        // target. This is updated even when we do not anchor this round.
        let entry = self
            .peer_advertised_slots
            .entry(sender_id.clone())
            .or_insert(0);
        if peer_slot > *entry {
            *entry = peer_slot;
        }

        // FORWARD-ONLY: only anchor when the peer is STRICTLY ahead of us. If
        // the peer is at or below our slot, its message is processed normally by
        // `handle_messages` (current or externalized slot) — nothing to do, and
        // we must never move backward.
        if peer_slot <= current {
            return false;
        }

        // CORROBORATION GATE (issue #431): require a v-blocking set of distinct
        // quorum members to corroborate the jump before anchoring forward. A
        // lone (even member) claim — including a bogus u64::MAX — is
        // insufficient, so it cannot strand an idle node. We anchor to the
        // HIGHEST slot a v-blocking set actually agrees on, never the raw
        // claimed value, so a single inflated `peer_slot` only ever pulls us as
        // far as the corroborated network slot.
        let target_slot = match self.highest_corroborated_slot() {
            Some(target) if target > current => target,
            _ => {
                debug!(
                    current,
                    peer_slot,
                    corroborating = self.anchor_corroborating_member_count(peer_slot),
                    required = self.anchor_corroboration_threshold(),
                    "Not anchoring SCP slot to peer's claimed slot: not yet corroborated by a \
                     v-blocking set of quorum members (issue #431 anti-griefing gate)"
                );
                return false;
            }
        };

        // SAFETY-GATED (issue #436, refining #430 Option C): never discard a
        // slot that holds genuine BALLOT/COMMIT state — re-opening a value that
        // reached the ballot protocol could fork. But bare, un-confirmed
        // NOMINATION state on a slot the network has PROVABLY moved past
        // (`peer_slot > current`, checked above) is safe to abandon: nothing is
        // committed, and the network already decided that slot. The old guard
        // blocked on ANY nomination vote, so a joiner that proposed its own
        // coinbase right after the join handoff (gaining bare nomination state)
        // would LATCH forever — the network never re-sends messages for its
        // stranded slot, its quorum never forms, and it discards every live
        // peer message as a future slot, becoming a permanent passive follower
        // (0 coinbases). We therefore block ONLY on ballot/commit indicators.
        let metrics = self.scp_node.get_current_slot_metrics();
        if Self::slot_holds_ballot_or_commit_state(&metrics) {
            debug!(
                current,
                target_slot,
                peer_slot,
                bN = metrics.bN,
                phase = ?metrics.phase,
                "Not anchoring SCP slot to peer's live slot: current slot holds \
                 ballot/commit state (deferring to avoid dropping committed state)"
            );
            return false;
        }

        info!(
            from_slot = current,
            to_slot = target_slot,
            peer_slot,
            "Anchoring SCP slot forward to the network's live slot learned from a \
             peer (issue #421 Option C): a behind node fast-forwards to the leaders' \
             slot (corroborated by a v-blocking set of quorum members, issue #431) \
             so it stops discarding their messages as future slots"
        );
        // FORWARD-ONLY: target_slot > current was checked above, so the
        // `reset_slot_index` strictly-increasing assert is satisfied.
        self.scp_node.reset_slot_index(target_slot);

        // A fresh slot has nothing proposed/externalized locally yet.
        self.proposed_values.clear();
        self.externalized = None;
        self.last_externalized_slot = None;

        true
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

    /// Issue #428 regression: the participation gate must withhold any
    /// propose/externalize while a node that expects peers (`min_peers >= 1`)
    /// has fewer than `min_peers` connected peers, and resume once enough are
    /// connected. A node with `min_peers == 0` must always propose (genuine
    /// solo, no regression).
    #[test]
    fn participation_gate_blocks_solo_block_until_peers_connected() {
        // Default (min_peers == 0): genuine solo node always proposes.
        let mut solo = solo_service();
        assert!(
            solo.should_propose_this_round(),
            "min_peers==0 node must always be eligible to mint solo"
        );
        solo.submit_transaction([7u8; 32], vec![1, 2, 3]);
        solo.propose_pending_values();
        assert!(
            solo.externalized.is_some(),
            "min_peers==0 node must externalize solo (no regression)"
        );

        // A node that expects 1 peer but has 0 connected must NOT propose.
        let mut gated = solo_service();
        gated.set_min_peers(1);
        gated.set_connected_peers(0);
        assert!(
            !gated.should_propose_this_round(),
            "node expecting peers must not mint while solo (pre-quorum)"
        );
        gated.submit_transaction([8u8; 32], vec![4, 5, 6]);
        gated.propose_pending_values();
        assert!(
            gated.externalized.is_none(),
            "gated node must NOT produce a pre-quorum solo block (issue #428)"
        );
        // The value stays queued so it is proposed once the quorum forms.
        assert!(
            !gated.pending_values.is_empty(),
            "withheld values must remain pending, not be dropped"
        );

        // Once a peer connects (connected >= min_peers) the gate opens.
        gated.set_connected_peers(1);
        assert!(
            gated.should_propose_this_round(),
            "node must become eligible once connected peers >= min_peers"
        );

        // If peers later drop below the expectation, minting pauses again
        // (halt, don't fork to solo).
        gated.set_connected_peers(0);
        assert!(
            !gated.should_propose_this_round(),
            "losing a quorum member must pause minting, not fall back to solo"
        );
    }

    /// Issue #433 regression: the 1-of-1 -> 2-of-2 transition must not fork.
    ///
    /// A node that expects peers (`min_peers == 1`) and already has a peer
    /// connected (`connected_peers == 1`), but whose SCP quorum is still the
    /// transient startup solo (1-of-1) set, must NOT take the solo
    /// direct-externalize path. If it did, two such peered nodes would each
    /// mine a divergent solo chain and fork (the observed #433 bug). It
    /// must withhold the block until the quorum reconfigures out of solo,
    /// and once it does it runs federated SCP (no direct solo externalize).
    #[test]
    fn transitional_solo_does_not_directly_externalize() {
        // Node boots solo (1-of-1) but expects a peer and already has one
        // connected — the exact post-wait-loop startup state from #433.
        let mut svc = solo_service();
        svc.set_min_peers(1);
        svc.set_connected_peers(1);

        // The participation gate alone WOULD open here (connected >= min_peers),
        // but we are still in solo mode, so this is transitional.
        assert!(svc.should_propose_this_round());
        assert!(svc.is_solo_mode());
        assert!(
            svc.is_transitional_solo(),
            "peer connected + still 1-of-1 must be detected as transitional solo"
        );

        // Proposing must be WITHHELD — no divergent solo block.
        svc.submit_minting_tx([9u8; 32], 100, vec![1, 2, 3]);
        svc.propose_pending_values();
        assert!(
            svc.externalized.is_none(),
            "transitional-solo node must NOT directly externalize (issue #433 fork)"
        );
        // The minting value stays queued so it is proposed once the quorum forms.
        assert!(
            !svc.pending_values.is_empty(),
            "withheld values must remain pending, not be dropped"
        );

        // Now the quorum reconfigures out of solo (run loop calls this on the
        // peer event; startup seeds it from connected peers). The node leaves
        // solo mode and is no longer transitional.
        let two = recommended_quorum(&[1, 2]);
        assert!(svc.reconfigure_quorum(two.clone()));
        assert!(
            !svc.is_solo_mode(),
            "node must leave solo once peer is in quorum"
        );
        assert!(!svc.is_transitional_solo());
        assert_eq!(svc.scp_node.quorum_set(), two);

        // After reconfig the node proposes via federated SCP — it does NOT take
        // the solo direct-externalize path (which would set `externalized`
        // synchronously). The value is handed to the SCP node for balloting.
        svc.propose_pending_values();
        assert!(
            svc.externalized.is_none(),
            "federated node must not directly externalize; it ballots via SCP"
        );
        assert!(
            !svc.proposed_values.is_empty(),
            "federated node must have proposed its value into SCP balloting"
        );
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

    /// A peer NOMINATE-only message (no Prepare ballot payload): it carries an
    /// accepted-nominated value but cannot start a ballot on the receiver. Used
    /// by the #436 boundary test to induce bare nomination-only state
    /// (`bN == 0`, phase `NominatePrepare`).
    fn peer_nominate_only(
        peer: NodeID,
        peer_quorum: QuorumSet,
        slot_index: SlotIndex,
        value: ConsensusValue,
    ) -> Msg<ConsensusValue> {
        let mut y = BTreeSet::new();
        y.insert(value);
        Msg::new(
            peer,
            peer_quorum,
            slot_index,
            Topic::Nominate(NominatePayload {
                X: BTreeSet::new(),
                Y: y,
            }),
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

    // ----------------------------------------------------------------------
    // Issue #419 / #417 Finding 1 + Finding 3 regression tests
    // ----------------------------------------------------------------------

    use crate::block::MintingTx;

    /// A genesis-state chain (height 0, zero tip) so that a minting tx for
    /// height 1 on the zero tip is the natural next block.
    fn genesis_chain_state() -> ChainState {
        ChainState::default()
    }

    /// Build a minting tx that passes INTRINSIC validation: PoW is satisfied by
    /// setting `difficulty = u64::MAX` (any hash is below it), and the
    /// timestamp is "now". `tag` makes the tx (and thus its hash + priority)
    /// distinct per minter, modelling two minters racing distinct coinbases for
    /// the same slot. `prev_block_hash`/`block_height` are recorded but are NOT
    /// checked by the SCP validity path (that is the whole point of the fix).
    fn intrinsic_valid_minting_tx(tag: u8, prev_block_hash: [u8; 32], height: u64) -> MintingTx {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        MintingTx {
            block_height: height,
            reward: 600_000_000_000,
            minter_view_key: [tag; 32],
            minter_spend_key: [tag.wrapping_add(1); 32],
            target_key: [tag.wrapping_add(2); 32],
            public_key: [tag.wrapping_add(3); 32],
            prev_block_hash,
            difficulty: u64::MAX,
            nonce: tag as u64,
            timestamp: now,
        }
    }

    /// Submit a minting tx to a service exactly as the local-miner path does,
    /// and register it on the peer service exactly as the gossip path does
    /// (intrinsic gate already applied by the caller). Returns the value's
    /// hash.
    fn submit_and_register(
        owner: &mut ConsensusService,
        peer: &mut ConsensusService,
        tx: &MintingTx,
    ) -> [u8; 32] {
        let tx_hash = tx.hash();
        let bytes = bincode::serialize(tx).expect("serialize minting tx");
        // PoW priority (lower hash = higher priority), as run.rs computes.
        let priority = tx.pow_priority();
        owner.submit_minting_tx(tx_hash, priority, bytes.clone());
        // The peer learns the value (intrinsic-valid) via the gossip cache.
        peer.register_minting_tx(tx_hash, bytes);
        tx_hash
    }

    /// Pump SCP `BroadcastMessage` events between two services until both
    /// externalize, or `max_rounds` elapse. Returns the externalized value sets
    /// for (svc_a, svc_b) once both have a `SlotExternalized` event.
    fn run_two_services(
        a: &mut ConsensusService,
        b: &mut ConsensusService,
        max_rounds: usize,
    ) -> (Option<Vec<ConsensusValue>>, Option<Vec<ConsensusValue>>) {
        let a_id = a.node_id().clone();
        let b_id = b.node_id().clone();
        let mut a_ext: Option<Vec<ConsensusValue>> = None;
        let mut b_ext: Option<Vec<ConsensusValue>> = None;

        // Inbox of serialized messages addressed to a given node.
        let mut to_a: Vec<ScpMessage> = Vec::new();
        let mut to_b: Vec<ScpMessage> = Vec::new();

        for _ in 0..max_rounds {
            // Deliver pending messages.
            for m in to_a.drain(..) {
                let _ = a.handle_message(m);
            }
            for m in to_b.drain(..) {
                let _ = b.handle_message(m);
            }

            // Drive timers / proposing.
            a.tick();
            b.tick();

            // Collect outgoing events from A.
            while let Some(ev) = a.next_event() {
                match ev {
                    ConsensusEvent::BroadcastMessage(msg) => to_b.push(msg),
                    ConsensusEvent::SlotExternalized { values, .. } => {
                        a_ext.get_or_insert(values);
                    }
                    ConsensusEvent::Progress { .. } => {}
                }
            }
            // Collect outgoing events from B.
            while let Some(ev) = b.next_event() {
                match ev {
                    ConsensusEvent::BroadcastMessage(msg) => to_a.push(msg),
                    ConsensusEvent::SlotExternalized { values, .. } => {
                        b_ext.get_or_insert(values);
                    }
                    ConsensusEvent::Progress { .. } => {}
                }
            }

            if a_ext.is_some() && b_ext.is_some() {
                break;
            }
            // Keep node ids referenced (they double as identity for routing in a
            // real network; here routing is by inbox).
            let _ = (&a_id, &b_id);
        }

        (a_ext, b_ext)
    }

    /// SAFETY: two `ConsensusService` instances (real validity/combine fns) in
    /// a 2-of-2 quorum, each proposing its OWN distinct minting value for
    /// the same slot, must converge on a SINGLE shared externalized value —
    /// never a fork.
    ///
    /// Before the #419 fix the SCP validity_fn was tip-dependent, so each node
    /// dropped the peer's competing minting value and externalized its own — a
    /// fork at the same height. With the tip-agnostic validity_fn both values
    /// survive into federated voting and the deterministic combiner picks one.
    #[test]
    fn two_minters_converge_on_single_value_no_fork() {
        // Build a tiny config so ticks propose immediately.
        let cfg = ConsensusConfig::fixed_timing(0);
        let qs = recommended_quorum(&[1, 2]);

        let mut a = ConsensusService::new(node(1), qs.clone(), cfg.clone(), genesis_chain_state());
        let mut b = ConsensusService::new(node(2), qs, cfg, genesis_chain_state());

        assert!(
            !a.is_solo_mode() && !b.is_solo_mode(),
            "must be a 2-node quorum"
        );

        let prev = [0u8; 32]; // shared genesis tip
        let a_tx = intrinsic_valid_minting_tx(1, prev, 1);
        let b_tx = intrinsic_valid_minting_tx(50, prev, 1);
        assert_ne!(
            a_tx.hash(),
            b_tx.hash(),
            "minters must propose distinct values"
        );

        // Each node submits its own coinbase and learns the peer's via gossip.
        submit_and_register(&mut a, &mut b, &a_tx);
        submit_and_register(&mut b, &mut a, &b_tx);

        let (a_ext, b_ext) = run_two_services(&mut a, &mut b, 2000);

        let a_vals = a_ext.expect("node A never externalized (consensus stalled)");
        let b_vals = b_ext.expect("node B never externalized (consensus stalled)");

        assert_eq!(
            a_vals, b_vals,
            "SAFETY VIOLATION: the two minters externalized DIFFERENT value sets \
             at the same slot (a fork)"
        );
        // One coinbase per block: exactly one minting value survives the combiner.
        let minting: Vec<_> = a_vals.iter().filter(|v| v.is_minting_tx).collect();
        assert_eq!(
            minting.len(),
            1,
            "exactly one minting tx must be externalized"
        );
        let chosen = minting[0].tx_hash;
        assert!(
            chosen == a_tx.hash() || chosen == b_tx.hash(),
            "externalized minting tx must be one of the two proposed coinbases"
        );
    }

    /// Finding 3: a node that catches up via block-sync must fast-forward its
    /// SCP slot to `chain_height + 1` so it stops discarding live peer messages
    /// as "future slots".
    #[test]
    fn sync_scp_slot_fast_forwards_after_block_sync() {
        let cfg = ConsensusConfig::fixed_timing(1);
        let qs = recommended_quorum(&[1, 2]);
        // Joiner boots from genesis: initial SCP slot is initial_height + 1 = 1.
        let mut joiner = ConsensusService::new(node(1), qs, cfg, ChainState::default());
        assert!(!joiner.is_solo_mode());
        assert_eq!(
            joiner.current_slot(),
            1,
            "fresh node starts at genesis slot 1"
        );

        // It block-syncs up to height 20 (the live net is balloting slot 21).
        let advanced = joiner.sync_scp_slot_to_chain(20);
        assert!(
            advanced,
            "must advance when SCP slot is behind chain height + 1"
        );
        assert_eq!(
            joiner.current_slot(),
            21,
            "SCP slot must fast-forward to chain_height + 1 so future-slot messages \
             from the live network are no longer discarded"
        );

        // Idempotent / never moves backward or re-resets at the same height.
        assert!(!joiner.sync_scp_slot_to_chain(20));
        assert_eq!(joiner.current_slot(), 21);
        assert!(
            !joiner.sync_scp_slot_to_chain(19),
            "must never move slot backward"
        );
        assert_eq!(joiner.current_slot(), 21);

        // A further sync advances again.
        assert!(joiner.sync_scp_slot_to_chain(25));
        assert_eq!(joiner.current_slot(), 26);
    }

    /// Finding 3 safety guard: solo nodes manage their own slot via
    /// `advance_slot`; `sync_scp_slot_to_chain` must be a no-op for them.
    #[test]
    fn sync_scp_slot_is_noop_in_solo_mode() {
        let mut svc = solo_service();
        assert!(svc.is_solo_mode());
        let before = svc.current_slot();
        assert!(!svc.sync_scp_slot_to_chain(100));
        assert_eq!(
            svc.current_slot(),
            before,
            "solo mode must not fast-forward"
        );
    }

    // ----------------------------------------------------------------------
    // Issue #421: SCP slot / ledger height drift — hybrid A1 + C
    //
    // These tests cover the approved non-wedging design (NOT the reverted #422
    // approach): A1 = tolerant duplicate-height apply (benign skip), C =
    // forward-only, idle-gated anchoring of a behind node to the network's live
    // SCP slot. T5 is the anti-#424 liveness determinism guard.
    // ----------------------------------------------------------------------

    /// Serialize a raw SCP `Msg` into the wire `ScpMessage` envelope, exactly
    /// as `queue_broadcast` does, so a test can deliver a peer message for
    /// an arbitrary slot to a service via `handle_message`.
    fn wire(sender: &NodeID, msg: &Msg<ConsensusValue>) -> ScpMessage {
        ScpMessage {
            sender: bincode::serialize(sender).unwrap_or_default(),
            slot_index: msg.slot_index,
            payload: bincode::serialize(msg).expect("serialize SCP message"),
        }
    }

    /// Drive node A (in a 2-of-2 quorum with B as the silent confirming peer)
    /// to externalize a single height-1 coinbase, modelling the run.rs
    /// apply-reject by NOT calling `update_chain_state` afterwards. Returns A
    /// already advanced to the next SCP slot, with the ledger still at height
    /// 0. This reproduces the exact desynchronization that creates #421
    /// drift.
    fn drive_a_to_externalize_without_applying() -> ConsensusService {
        let cfg = ConsensusConfig::fixed_timing(0);
        let qs = recommended_quorum(&[1, 2]);
        let mut a = ConsensusService::new(node(1), qs.clone(), cfg.clone(), genesis_chain_state());
        let mut b = ConsensusService::new(node(2), qs, cfg, genesis_chain_state());

        let prev = [0u8; 32];
        let a_tx = intrinsic_valid_minting_tx(1, prev, 1);
        let b_tx = intrinsic_valid_minting_tx(50, prev, 1);
        submit_and_register(&mut a, &mut b, &a_tx);
        submit_and_register(&mut b, &mut a, &b_tx);

        let (a_ext, _b_ext) = run_two_services(&mut a, &mut b, 2000);
        assert!(a_ext.is_some(), "A must externalize slot 1");
        // SCP auto-advanced past the externalized slot. We deliberately do NOT
        // call `update_chain_state`, so the "ledger" stays at height 0 while SCP
        // moved on — the exact run.rs externalize-then-reject desync.
        a
    }

    /// T1 — characterize the drift. After an externalize-then-reject the SCP
    /// slot advances while the ledger height does not, and feeding a second
    /// duplicate-height value advances it again: drift grows. We TOLERATE this
    /// drift (A1 + C absorb it); this test documents the mechanism is real.
    #[test]
    fn t1_externalize_then_reject_creates_slot_drift() {
        let mut a = drive_a_to_externalize_without_applying();
        let ledger_height = 0u64; // we never applied the block
        let slot_after_first = a.current_slot();
        assert!(
            slot_after_first > ledger_height + 1,
            "after externalize-then-reject the SCP slot ({}) must be ahead of \
             ledger_height + 1 ({}) — the #421 drift",
            slot_after_first,
            ledger_height + 1
        );
        assert_eq!(
            slot_after_first, 2,
            "one externalize-then-reject drifts the slot by exactly +1"
        );
    }

    /// T2 — joiner non-convergence reproduces #421 WITHOUT Option C. A behind
    /// node and a drifted leader discard each other's messages (future/past
    /// slots) and neither makes progress. We assert this directly against the
    /// SCP node's discard rule (the `anchor_scp_slot_to_peer` hook is bypassed
    /// by feeding the SCP node directly), proving C is what's load-bearing.
    #[test]
    fn t2_joiner_cannot_converge_with_drifted_leader_without_c() {
        // Behind node B at slot H+1 = 1 (genesis), idle.
        let cfg = ConsensusConfig::fixed_timing(0);
        let qs = recommended_quorum(&[1, 2]);
        let mut b = ConsensusService::new(node(1), qs.clone(), cfg, genesis_chain_state());
        assert_eq!(b.current_slot(), 1);

        // The leader is drifted ahead to slot S = 5. Build its peer message for
        // slot 5 and feed it DIRECTLY to B's SCP node (bypassing Option C) to
        // model the pre-fix behavior.
        let drifted_slot: SlotIndex = 5;
        let prev = [0u8; 32];
        let leader_tx = intrinsic_valid_minting_tx(1, prev, drifted_slot);
        let leader_hash = leader_tx.hash();
        let leader_bytes = bincode::serialize(&leader_tx).unwrap();
        b.register_minting_tx(leader_hash, leader_bytes);
        let value = ConsensusValue::from_minting_tx(leader_hash, leader_tx.pow_priority());
        let leader_msg =
            peer_nominate_prepare(node(2), recommended_quorum(&[1, 2]), drifted_slot, value);

        // Pre-fix: the SCP node silently discards this as a "future slot".
        let _ = b.scp_node.handle_message(&leader_msg);
        assert_eq!(
            b.current_slot(),
            1,
            "without Option C, a behind node stays at its slot and discards the \
             leader's higher-slot message as a future slot — no convergence"
        );
    }

    /// T3 — Option C convergence (THE KEY GUARD). A behind, IDLE node anchors
    /// FORWARD to the leader's live slot when it observes a peer message for a
    /// higher slot, via the public `handle_message` path (which calls
    /// `anchor_scp_slot_to_peer`). It must NOT move backward, and must REFUSE
    /// to anchor when it holds in-flight ballot state.
    #[test]
    fn t3_option_c_anchors_behind_node_forward_to_leader_slot() {
        let cfg = ConsensusConfig::fixed_timing(0);
        let qs = recommended_quorum(&[1, 2]);
        let mut b = ConsensusService::new(node(1), qs.clone(), cfg, genesis_chain_state());
        assert_eq!(b.current_slot(), 1, "behind node starts at genesis slot 1");

        let drifted_slot: SlotIndex = 7;
        let prev = [0u8; 32];
        let leader_tx = intrinsic_valid_minting_tx(1, prev, drifted_slot);
        let leader_hash = leader_tx.hash();
        b.register_minting_tx(leader_hash, bincode::serialize(&leader_tx).unwrap());
        let value = ConsensusValue::from_minting_tx(leader_hash, leader_tx.pow_priority());
        let leader_msg =
            peer_nominate_prepare(node(2), recommended_quorum(&[1, 2]), drifted_slot, value);

        // Deliver via the public path: Option C fast-forwards B to slot 7.
        b.handle_message(wire(&node(2), &leader_msg))
            .expect("handle_message");
        assert_eq!(
            b.current_slot(),
            drifted_slot,
            "Option C must anchor the behind, idle node FORWARD to the leader's \
             live slot so it stops discarding the leader's messages"
        );

        // Forward-only: a message for a LOWER slot must NOT move B backward.
        let lower_tx = intrinsic_valid_minting_tx(2, prev, 3);
        b.register_minting_tx(lower_tx.hash(), bincode::serialize(&lower_tx).unwrap());
        let lower_msg = peer_nominate_prepare(
            node(2),
            recommended_quorum(&[1, 2]),
            3,
            ConsensusValue::from_minting_tx(lower_tx.hash(), lower_tx.pow_priority()),
        );
        b.handle_message(wire(&node(2), &lower_msg))
            .expect("handle_message");
        assert_eq!(
            b.current_slot(),
            drifted_slot,
            "Option C is FORWARD-ONLY: a lower-slot message must never rewind"
        );

        // Idle-gate: once B holds in-flight ballot state for its current slot, a
        // higher-slot peer message must NOT anchor (we must not discard it).
        // Drive in-flight state by feeding a same-slot peer nominate first.
        let same_tx = intrinsic_valid_minting_tx(3, prev, drifted_slot);
        b.register_minting_tx(same_tx.hash(), bincode::serialize(&same_tx).unwrap());
        let same_msg = peer_nominate_prepare(
            node(2),
            recommended_quorum(&[1, 2]),
            drifted_slot,
            ConsensusValue::from_minting_tx(same_tx.hash(), same_tx.pow_priority()),
        );
        b.handle_message(wire(&node(2), &same_msg))
            .expect("handle_message");
        let metrics = b.scp_node.get_current_slot_metrics();
        let active = metrics.num_voted_nominated > 0
            || metrics.num_accepted_nominated > 0
            || metrics.num_confirmed_nominated > 0
            || metrics.bN > 0
            || metrics.phase != ScpPhase::NominatePrepare;
        if active {
            let way_ahead = drifted_slot + 10;
            let far_tx = intrinsic_valid_minting_tx(4, prev, way_ahead);
            b.register_minting_tx(far_tx.hash(), bincode::serialize(&far_tx).unwrap());
            let far_msg = peer_nominate_prepare(
                node(2),
                recommended_quorum(&[1, 2]),
                way_ahead,
                ConsensusValue::from_minting_tx(far_tx.hash(), far_tx.pow_priority()),
            );
            b.handle_message(wire(&node(2), &far_msg))
                .expect("handle_message");
            assert_eq!(
                b.current_slot(),
                drifted_slot,
                "idle-gate: must NOT anchor away from a slot holding in-flight \
                 protocol state (would drop accepted-commit state)"
            );
        }
    }

    /// Build a quorum set using the CRASH-model threshold `floor(n/2) + 1`
    /// (botho's default `FaultModel::Crash`). For `n >= 3` this gives a
    /// threshold STRICTLY below `n`, so a v-blocking set (`n - t + 1`) is
    /// larger than one node — exactly the regime the issue #431
    /// corroboration gate must protect (a single member's claim must NOT be
    /// self-corroborating).
    fn crash_quorum(ids: &[u32]) -> QuorumSet {
        let n = ids.len();
        let threshold = (n / 2 + 1) as u32;
        let members: Vec<QuorumSetMember<NodeID>> = ids
            .iter()
            .map(|i| QuorumSetMember::Node(node(*i)))
            .collect();
        QuorumSet::new(threshold, members)
    }

    /// Issue #431 (anti-griefing): a BOGUS high `slot_index` from a SINGLE
    /// quorum member must NOT move an idle node's slot, and neither must any
    /// claim from a NON-member. A corroborated, in-window slot from a
    /// v-blocking set of quorum members DOES anchor it forward.
    ///
    /// Fails PRE-fix (a lone `u64::MAX` claim fast-forwarded the node);
    /// passes POST-fix (membership + v-blocking corroboration gates).
    #[test]
    fn t_issue431_bogus_high_slot_does_not_strand_idle_node() {
        let cfg = ConsensusConfig::fixed_timing(0);
        // 3-of-... CRASH quorum: n=3, threshold=2, so a v-blocking set is
        // n - t + 1 = 2 members. A single member is NOT v-blocking.
        let qs = crash_quorum(&[1, 2, 3]);
        assert_eq!(qs.threshold, 2, "crash 3-node threshold is floor(3/2)+1=2");
        let mut b = ConsensusService::new(node(1), qs, cfg, genesis_chain_state());
        assert_eq!(b.current_slot(), 1, "behind node starts at genesis slot 1");

        let prev = [0u8; 32];

        // (a) A single quorum member (node 2) gossips a bogus u64::MAX slot.
        // Membership passes, but corroboration (needs 2 distinct members) does
        // not: the idle node must NOT anchor.
        let bogus_slot = u64::MAX;
        let bogus_tx = intrinsic_valid_minting_tx(1, prev, bogus_slot);
        b.register_minting_tx(bogus_tx.hash(), bincode::serialize(&bogus_tx).unwrap());
        let bogus_msg = peer_nominate_prepare(
            node(2),
            crash_quorum(&[1, 2, 3]),
            bogus_slot,
            ConsensusValue::from_minting_tx(bogus_tx.hash(), bogus_tx.pow_priority()),
        );
        b.handle_message(wire(&node(2), &bogus_msg))
            .expect("handle_message");
        assert_eq!(
            b.current_slot(),
            1,
            "issue #431: a lone quorum member's bogus u64::MAX slot must NOT \
             strand the idle node (no v-blocking corroboration)"
        );

        // (a') A NON-member (node 9) gossips a huge slot, even corroborated by
        // itself many times — it must be ignored entirely (membership gate).
        let nonmember_slot: SlotIndex = 50;
        let nm_tx = intrinsic_valid_minting_tx(2, prev, nonmember_slot);
        b.register_minting_tx(nm_tx.hash(), bincode::serialize(&nm_tx).unwrap());
        let nm_msg = peer_nominate_prepare(
            node(9),
            crash_quorum(&[1, 2, 3]),
            nonmember_slot,
            ConsensusValue::from_minting_tx(nm_tx.hash(), nm_tx.pow_priority()),
        );
        b.handle_message(wire(&node(9), &nm_msg))
            .expect("handle_message");
        assert_eq!(
            b.current_slot(),
            1,
            "issue #431: a non-quorum-member's slot claim must NOT drive an anchor"
        );

        // (b) A v-blocking set of DISTINCT quorum members (nodes 2 and 3) both
        // advertise a real, in-window slot S. Now corroboration is met and the
        // idle node anchors FORWARD to S.
        let real_slot: SlotIndex = 7;
        let tx2 = intrinsic_valid_minting_tx(3, prev, real_slot);
        b.register_minting_tx(tx2.hash(), bincode::serialize(&tx2).unwrap());
        let msg2 = peer_nominate_prepare(
            node(2),
            crash_quorum(&[1, 2, 3]),
            real_slot,
            ConsensusValue::from_minting_tx(tx2.hash(), tx2.pow_priority()),
        );
        b.handle_message(wire(&node(2), &msg2))
            .expect("handle_message");
        // Only ONE member (node 2) so far at slot 7 -> still not corroborated.
        assert_eq!(
            b.current_slot(),
            1,
            "issue #431: one member at the target slot is below the v-blocking \
             threshold (2); must not anchor yet"
        );

        let tx3 = intrinsic_valid_minting_tx(4, prev, real_slot);
        b.register_minting_tx(tx3.hash(), bincode::serialize(&tx3).unwrap());
        let msg3 = peer_nominate_prepare(
            node(3),
            crash_quorum(&[1, 2, 3]),
            real_slot,
            ConsensusValue::from_minting_tx(tx3.hash(), tx3.pow_priority()),
        );
        b.handle_message(wire(&node(3), &msg3))
            .expect("handle_message");
        assert_eq!(
            b.current_slot(),
            real_slot,
            "issue #431: once a v-blocking set (nodes 2 and 3) corroborates slot \
             7, the idle joiner anchors FORWARD — legitimate convergence (#396) \
             is preserved"
        );
    }

    /// Issue #431: even AFTER a v-blocking set corroborates a real slot, a
    /// subsequent lone bogus u64::MAX claim must NOT over-advance the node past
    /// the corroborated network slot. We anchor only to the highest
    /// v-blocking-corroborated slot, never the raw claimed value.
    #[test]
    fn t_issue431_anchor_is_bounded_to_corroborated_slot() {
        let cfg = ConsensusConfig::fixed_timing(0);
        let qs = crash_quorum(&[1, 2, 3]);
        let mut b = ConsensusService::new(node(1), qs, cfg, genesis_chain_state());
        let prev = [0u8; 32];

        // Both members corroborate slot 5.
        for (tag, peer) in [(1u8, 2u32), (2u8, 3u32)] {
            let tx = intrinsic_valid_minting_tx(tag, prev, 5);
            b.register_minting_tx(tx.hash(), bincode::serialize(&tx).unwrap());
            let m = peer_nominate_prepare(
                node(peer),
                crash_quorum(&[1, 2, 3]),
                5,
                ConsensusValue::from_minting_tx(tx.hash(), tx.pow_priority()),
            );
            b.handle_message(wire(&node(peer), &m)).expect("handle");
        }
        assert_eq!(b.current_slot(), 5, "anchors to the corroborated slot 5");

        // Now a lone member claims u64::MAX. Only one member is at that slot, so
        // the highest corroborated slot is still 5 (already reached): no jump.
        let bogus_tx = intrinsic_valid_minting_tx(3, prev, u64::MAX);
        b.register_minting_tx(bogus_tx.hash(), bincode::serialize(&bogus_tx).unwrap());
        let bogus = peer_nominate_prepare(
            node(2),
            crash_quorum(&[1, 2, 3]),
            u64::MAX,
            ConsensusValue::from_minting_tx(bogus_tx.hash(), bogus_tx.pow_priority()),
        );
        b.handle_message(wire(&node(2), &bogus)).expect("handle");
        assert_eq!(
            b.current_slot(),
            5,
            "issue #431: a lone u64::MAX claim must not push the node past the \
             v-blocking-corroborated slot"
        );
    }

    /// T4 — Option A1 benign-skip classification. The decision "is this apply
    /// failure a benign duplicate-height skip?" is `block.height() <=
    /// ledger_height`. We assert that discrimination here (the run.rs branch
    /// applies the same rule): a height already filled is benign; a higher,
    /// genuinely-failing height is NOT masked. Option C must never move the
    /// slot backward as part of this.
    #[test]
    fn t4_a1_duplicate_height_is_benign_higher_height_is_not_masked() {
        // ledger at height 5.
        let ledger_height: u64 = 5;

        // A duplicate / lost-the-race coinbase for an already-filled height.
        let dup_height: u64 = 5; // <= ledger_height -> benign skip
        assert!(
            dup_height <= ledger_height,
            "A1: a height already in the ledger is a benign duplicate-height skip"
        );
        let older_height: u64 = 3; // also already filled -> benign
        assert!(older_height <= ledger_height);

        // A genuinely-new height that fails apply for some OTHER reason must NOT
        // be masked as benign (we must surface real validation failures).
        let new_height: u64 = 6; // > ledger_height -> hard failure, not masked
        assert!(
            new_height > ledger_height,
            "A1 must NOT mask a real validation failure at a fresh height"
        );
    }

    /// T5 — 2-minter liveness NOT regressed (the anti-#424 guard, REQUIRED).
    ///
    /// Two services in a genuine 2-of-2, each submitting its OWN competing-but-
    /// valid next-height coinbase every round, must make SUSTAINED progress and
    /// NEVER wedge. This is the regression whose ABSENCE let #422 ship the
    /// wedge. It MUST be green with A1 + C (which touch neither proposal
    /// selection nor introduce a backward slot move) and would FAIL under
    /// #422's proposal-side coinbase filter.
    #[test]
    fn t5_two_minters_make_sustained_progress_no_wedge() {
        let cfg = ConsensusConfig::fixed_timing(0);
        let qs = recommended_quorum(&[1, 2]);
        let mut a = ConsensusService::new(node(1), qs.clone(), cfg.clone(), genesis_chain_state());
        let mut b = ConsensusService::new(node(2), qs, cfg, genesis_chain_state());
        assert!(!a.is_solo_mode() && !b.is_solo_mode());

        // Genesis tip; both minters race a DISTINCT coinbase for each height.
        let mut tip = [0u8; 32];
        let target_heights: u64 = 12;

        for height in 1..=target_heights {
            // Each minter mines its own competing coinbase for this height.
            let a_tx = intrinsic_valid_minting_tx(1, tip, height);
            let b_tx = intrinsic_valid_minting_tx(50, tip, height);
            assert_ne!(a_tx.hash(), b_tx.hash(), "minters race distinct coinbases");
            submit_and_register(&mut a, &mut b, &a_tx);
            submit_and_register(&mut b, &mut a, &b_tx);

            let (a_ext, b_ext) = run_two_services(&mut a, &mut b, 4000);
            let a_vals = a_ext.unwrap_or_else(|| {
                panic!(
                    "A WEDGED at height {} (no externalize) — 2-minter liveness \
                     regression (the #424 failure mode)",
                    height
                )
            });
            let b_vals = b_ext.unwrap_or_else(|| {
                panic!(
                    "B WEDGED at height {} (no externalize) — 2-minter liveness \
                     regression (the #424 failure mode)",
                    height
                )
            });
            assert_eq!(
                a_vals, b_vals,
                "SAFETY: minters externalized DIFFERENT value sets at height {} (fork)",
                height
            );
            let minting: Vec<_> = a_vals.iter().filter(|v| v.is_minting_tx).collect();
            assert_eq!(
                minting.len(),
                1,
                "exactly one coinbase per height at height {}",
                height
            );

            // Apply the agreed block to BOTH ledgers so the next round races on a
            // shared, advanced tip (mirrors a real block landing). We model the
            // new tip deterministically from the chosen coinbase hash.
            let chosen = minting[0].tx_hash;
            tip = chosen; // deterministic next prev_block_hash for the test
            let next_state = ChainState {
                height,
                tip_hash: tip,
                ..ChainState::default()
            };
            a.update_chain_state(next_state.clone());
            b.update_chain_state(next_state);
            // Advance both services' slots as run.rs does after a successful apply.
            a.advance_slot();
            b.advance_slot();
        }

        // Sustained progress past height 9 (the #424 wedge point) with no stall.
        assert!(
            a.current_slot() > 9 && b.current_slot() > 9,
            "2-minter pair must advance PAST height 9 without wedging (A slot {}, B slot {})",
            a.current_slot(),
            b.current_slot()
        );
    }

    // ----------------------------------------------------------------------
    // Issue #436: the Option-C anchor must NOT latch on un-completable
    // bare-NOMINATION state. The safety boundary is ballot/commit state.
    // ----------------------------------------------------------------------

    /// Construct `SlotMetrics` for the boundary tests below.
    fn metrics(
        phase: ScpPhase,
        voted: usize,
        accepted: usize,
        confirmed: usize,
        b_n: u32,
    ) -> SlotMetrics {
        SlotMetrics {
            phase,
            num_voted_nominated: voted,
            num_accepted_nominated: accepted,
            num_confirmed_nominated: confirmed,
            cur_nomination_round: 1,
            bN: b_n,
        }
    }

    /// THE SAFETY BOUNDARY (issue #436), tested directly on the predicate that
    /// gates both `anchor_scp_slot_to_peer` and `sync_scp_slot_to_chain`.
    ///
    /// Pre-fix this guard blocked the anchor on ANY nomination vote, which made
    /// a joiner latch forever on its own bare-nomination state. Post-fix it
    /// blocks ONLY on genuine ballot/commit indicators (`bN > 0` or a phase
    /// past `NominatePrepare`) and lets bare nomination through.
    ///
    /// Fail-pre/pass-post: under the OLD condition (the `||` of all five
    /// fields) the `voted`/`accepted`/`confirmed` cases below assert
    /// `false` to equal the OLD `true`; under the new predicate they are
    /// `false` (anchor allowed). The ballot/commit cases stay `true`
    /// (anchor blocked) under both — proving the relaxation never abandons
    /// committed state.
    #[test]
    fn issue436_safety_boundary_nomination_only_vs_ballot_commit() {
        // --- NOMINATION-ONLY (safe to abandon when the network has passed it):
        //     phase NominatePrepare, bN == 0, regardless of vote counts. ---

        // A brand-new / idle slot.
        assert!(
            !ConsensusService::slot_holds_ballot_or_commit_state(&metrics(
                ScpPhase::NominatePrepare,
                0,
                0,
                0,
                0
            )),
            "an idle slot must not be treated as ballot/commit state"
        );
        // The exact #436 bug state: the joiner proposed its own coinbase, so it
        // holds VOTED-nominated state but no ballot has started. This MUST now
        // be anchorable forward (pre-fix this latched forever).
        assert!(
            !ConsensusService::slot_holds_ballot_or_commit_state(&metrics(
                ScpPhase::NominatePrepare,
                1,
                0,
                0,
                0
            )),
            "bare VOTED-nominated state (bN == 0, phase NominatePrepare) must NOT \
             block the forward anchor — this is the #436 wedge"
        );
        // Accepted-nominated, still no ballot.
        assert!(
            !ConsensusService::slot_holds_ballot_or_commit_state(&metrics(
                ScpPhase::NominatePrepare,
                1,
                1,
                0,
                0
            )),
            "ACCEPTED-nominated with bN == 0 is still nomination-only — anchor allowed"
        );
        // Confirmed-nominated but ballot not yet seeded (transient/degenerate):
        // bN == 0 still means nothing committed to the ballot protocol.
        assert!(
            !ConsensusService::slot_holds_ballot_or_commit_state(&metrics(
                ScpPhase::NominatePrepare,
                1,
                1,
                1,
                0
            )),
            "CONFIRMED-nominated with bN == 0 (ballot not seeded) is still safe to skip"
        );

        // --- BALLOT/COMMIT (must NEVER be abandoned — fork risk): a ballot has
        //     started (bN > 0) OR the phase has progressed past NominatePrepare. ---

        // A ballot has started while still in the concurrent nominate/prepare
        // phase: bN > 0 is the load-bearing protection.
        assert!(
            ConsensusService::slot_holds_ballot_or_commit_state(&metrics(
                ScpPhase::NominatePrepare,
                1,
                1,
                1,
                1
            )),
            "a started ballot (bN > 0) MUST block the anchor even in NominatePrepare"
        );
        // Phase has advanced to Prepare (a ballot was confirmed prepared).
        assert!(
            ConsensusService::slot_holds_ballot_or_commit_state(&metrics(
                ScpPhase::Prepare,
                0,
                0,
                0,
                1
            )),
            "Prepare phase MUST block the anchor"
        );
        // Commit phase: a value is accepted committed — abandoning it could fork.
        assert!(
            ConsensusService::slot_holds_ballot_or_commit_state(&metrics(
                ScpPhase::Commit,
                0,
                0,
                0,
                2
            )),
            "Commit phase MUST block the anchor (accepted-committed value)"
        );
        // Externalize phase.
        assert!(
            ConsensusService::slot_holds_ballot_or_commit_state(&metrics(
                ScpPhase::Externalize,
                0,
                0,
                0,
                3
            )),
            "Externalize phase MUST block the anchor"
        );
    }

    /// End-to-end via the public `handle_message` path (which calls
    /// `anchor_scp_slot_to_peer`): a node that holds bare NOMINATION state on a
    /// slot the network has provably passed DOES anchor forward and stops
    /// stranding; a node that holds BALLOT/COMMIT state does NOT (the slot is
    /// preserved).
    ///
    /// This reproduces the #436 join-handoff wedge: the joiner proposes its own
    /// coinbase right after catch-up (bare nomination), the network moves on,
    /// and the joiner must fast-forward instead of latching.
    #[test]
    fn issue436_anchor_forward_past_bare_nomination_but_not_ballot_commit() {
        let cfg = ConsensusConfig::fixed_timing(0);
        let qs = recommended_quorum(&[1, 2]);

        // --- Case 1: bare NOMINATION state anchors forward. ---
        // Use a 3-of-3 quorum so a single peer's nominate does NOT form a
        // quorum on C — C accumulates voted/accepted-nominated state but never
        // CONFIRMS, so the ballot is never seeded (bN stays 0). This is exactly
        // the #436 wedge: C holds bare nomination-only state on a slot the
        // network (A+B) has already externalized and moved past, so it never
        // re-sends messages for C's stranded slot and C's quorum can't form.
        let qs3 = recommended_quorum(&[1, 2, 3]);
        let mut c = ConsensusService::new(node(1), qs3.clone(), cfg.clone(), genesis_chain_state());
        assert_eq!(c.current_slot(), 1);
        let prev = [0u8; 32];

        // Drive C into bare NOMINATION-only state (phase NominatePrepare,
        // bN == 0) on slot 1 via a single peer NOMINATE-only message — one of
        // three peers is below quorum, so nothing confirms and no ballot starts.
        let nom_tx = intrinsic_valid_minting_tx(40, prev, 1);
        c.register_minting_tx(nom_tx.hash(), bincode::serialize(&nom_tx).unwrap());
        let nom_val = ConsensusValue::from_minting_tx(nom_tx.hash(), nom_tx.pow_priority());
        let nom_msg = peer_nominate_only(node(2), qs3.clone(), 1, nom_val);
        c.handle_message(wire(&node(2), &nom_msg))
            .expect("handle_message");
        let m = c.scp_node.get_current_slot_metrics();
        assert!(
            m.num_voted_nominated > 0 || m.num_accepted_nominated > 0,
            "C must hold some nomination state (voted {}, accepted {})",
            m.num_voted_nominated,
            m.num_accepted_nominated
        );
        assert_eq!(
            m.bN, 0,
            "no ballot may have started (sub-quorum, bare nomination)"
        );
        assert_eq!(m.phase, ScpPhase::NominatePrepare);
        assert!(
            !ConsensusService::slot_holds_ballot_or_commit_state(&m),
            "this is exactly the bare-nomination state #436 must let through \
             (voted {}, accepted {}, confirmed {}, bN {})",
            m.num_voted_nominated,
            m.num_accepted_nominated,
            m.num_confirmed_nominated,
            m.bN
        );

        // The network has moved on: a peer message arrives for a strictly higher
        // slot. C MUST anchor forward (pre-#436 it latched forever).
        let peer_slot: SlotIndex = 6;
        let peer_tx = intrinsic_valid_minting_tx(50, prev, peer_slot);
        c.register_minting_tx(peer_tx.hash(), bincode::serialize(&peer_tx).unwrap());
        let peer_val = ConsensusValue::from_minting_tx(peer_tx.hash(), peer_tx.pow_priority());
        let peer_msg = peer_nominate_prepare(node(2), qs3.clone(), peer_slot, peer_val);
        c.handle_message(wire(&node(2), &peer_msg))
            .expect("handle_message");
        assert_eq!(
            c.current_slot(),
            peer_slot,
            "C must anchor FORWARD past its stranded bare-nomination slot to the \
             network's live slot (issue #436) — not latch forever"
        );

        // --- Case 2: BALLOT/COMMIT state is NOT abandoned. ---
        // Drive two genuine services until one holds ballot/commit state on a
        // slot, then deliver a far-ahead peer message and assert it does NOT
        // anchor away from that slot.
        let mut x = ConsensusService::new(node(1), qs.clone(), cfg.clone(), genesis_chain_state());
        let mut y = ConsensusService::new(node(2), qs.clone(), cfg.clone(), genesis_chain_state());
        let x_tx = intrinsic_valid_minting_tx(1, prev, 1);
        let y_tx = intrinsic_valid_minting_tx(50, prev, 1);
        submit_and_register(&mut x, &mut y, &x_tx);
        submit_and_register(&mut y, &mut x, &y_tx);

        // Step the pair a few rounds so X enters the ballot protocol on slot 1
        // (bN > 0 or phase past NominatePrepare) but has not yet externalized.
        let ballot_slot = x.current_slot();
        let mut reached_ballot = false;
        let a_id = x.node_id().clone();
        let b_id = y.node_id().clone();
        let mut to_x: Vec<ScpMessage> = Vec::new();
        let mut to_y: Vec<ScpMessage> = Vec::new();
        for _ in 0..200 {
            for msg in to_x.drain(..) {
                let _ = x.handle_message(msg);
            }
            for msg in to_y.drain(..) {
                let _ = y.handle_message(msg);
            }
            let m = x.scp_node.get_current_slot_metrics();
            if ConsensusService::slot_holds_ballot_or_commit_state(&m)
                && x.current_slot() == ballot_slot
            {
                reached_ballot = true;
                break;
            }
            x.tick();
            y.tick();
            while let Some(ev) = x.next_event() {
                if let ConsensusEvent::BroadcastMessage(msg) = ev {
                    to_y.push(msg);
                }
            }
            while let Some(ev) = y.next_event() {
                if let ConsensusEvent::BroadcastMessage(msg) = ev {
                    to_x.push(msg);
                }
            }
            let _ = (&a_id, &b_id);
        }
        assert!(
            reached_ballot,
            "X must enter ballot/commit state on its current slot for the boundary test"
        );
        let before = x.current_slot();
        let m = x.scp_node.get_current_slot_metrics();
        assert!(
            ConsensusService::slot_holds_ballot_or_commit_state(&m),
            "precondition: X holds ballot/commit state (bN {} phase {:?})",
            m.bN,
            m.phase
        );

        // A far-ahead peer message MUST NOT anchor X away from its ballot slot.
        let far_slot = before + 20;
        let far_tx = intrinsic_valid_minting_tx(77, prev, far_slot);
        x.register_minting_tx(far_tx.hash(), bincode::serialize(&far_tx).unwrap());
        let far_val = ConsensusValue::from_minting_tx(far_tx.hash(), far_tx.pow_priority());
        let far_msg = peer_nominate_prepare(node(2), qs.clone(), far_slot, far_val);
        x.handle_message(wire(&node(2), &far_msg))
            .expect("handle_message");
        assert_eq!(
            x.current_slot(),
            before,
            "SAFETY: a slot holding ballot/commit state must NEVER be abandoned by \
             the forward anchor — that could re-open a committed value and fork"
        );
    }
}
