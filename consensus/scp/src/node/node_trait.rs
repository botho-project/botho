// Copyright (c) 2018-2022 The Botho Foundation

use crate::{msg::Msg, slot::SlotMetrics, QuorumSet, SlotIndex, Value};
use bth_common::NodeID;
use mockall::*;
use std::collections::BTreeSet;

/// A node capable of participating in SCP.
#[automock]
pub trait ScpNode<V: Value>: Send {
    /// Get local node ID.
    fn node_id(&self) -> NodeID;

    /// Get local node quorum set.
    fn quorum_set(&self) -> QuorumSet;

    /// Replace this node's quorum set, rebuilding the current slot so the new
    /// membership/threshold takes effect immediately.
    ///
    /// This abandons any in-progress state for the current slot and any stored
    /// externalized slots, so callers must only invoke it at a slot boundary
    /// (e.g. before the current slot has nominated/proposed any values). The
    /// current slot index is preserved.
    fn set_quorum_set(&mut self, quorum_set: QuorumSet);

    /// Propose values for this node to nominate.
    fn propose_values(&mut self, values: BTreeSet<V>) -> Result<Option<Msg<V>>, String>;

    /// Handle incoming message from the network.
    fn handle_message(&mut self, msg: &Msg<V>) -> Result<Option<Msg<V>>, String>;

    /// Handle incoming messages from the network.
    fn handle_messages(&mut self, msgs: Vec<Msg<V>>) -> Result<Vec<Msg<V>>, String>;

    /// Maximum number of stored externalized slots.
    fn max_externalized_slots(&self) -> usize;

    /// Set the maximum number of stored externalized slots. Must be non-zero.
    fn set_max_externalized_slots(&mut self, n: usize);

    /// Get externalized values (or an empty vector) for a given slot index.
    fn get_externalized_values(&self, slot_index: SlotIndex) -> Option<Vec<V>>;

    /// Process pending timeouts.
    fn process_timeouts(&mut self) -> Vec<Msg<V>>;

    /// Get the current slot's index.
    fn current_slot_index(&self) -> SlotIndex;

    /// Get metrics for the current slot.
    fn get_current_slot_metrics(&mut self) -> SlotMetrics;

    /// Additional debug info, e.g. a JSON representation of the Slot's state.
    fn get_slot_debug_snapshot(&mut self, slot_index: SlotIndex) -> Option<String>;

    /// Set the node's current slot index, abandoning any current and
    /// externalized slots.
    ///
    /// This is FORWARD-ONLY: callers must pass an index strictly greater than
    /// the current one (a `debug_assert` enforces this). To move the slot
    /// *backward* under tightly-controlled conditions, use
    /// [`realign_slot_to`](Self::realign_slot_to) instead.
    fn reset_slot_index(&mut self, slot_index: SlotIndex);

    /// Re-seat the current slot at `slot_index`, allowed to move the slot
    /// **backward** (`slot_index <= current_slot_index()`), abandoning any
    /// current and externalized slots.
    ///
    /// # SAFETY — never re-open an externalized index
    ///
    /// SCP's agreement theorem assumes a slot index externalizes at most once.
    /// Re-balloting an index this node has already externalized could
    /// externalize a *second, different* value at the same index — a fork. This
    /// primitive therefore exists ONLY to clean up slot indices that were
    /// created by the auto-advance of a *doomed* externalize (a value that was
    /// externalized by SCP but then rejected at block-apply, so no real block
    /// ever landed and this node never produced a usable value at the higher
    /// index).
    ///
    /// This method does NOT itself know which indices were "real"; the caller
    /// MUST enforce, before invoking it, that:
    /// - the target `slot_index` is strictly greater than the highest index
    ///   this node has externalized into a *finalized* block (so we never
    ///   re-open a committed index), and
    /// - the current slot is idle (no in-flight nominate/ballot state).
    ///
    /// Returns `false` and does nothing if `slot_index` is greater than the
    /// current index (use `reset_slot_index` for forward moves so the
    /// forward-only invariant stays auditable). Otherwise re-seats at
    /// `slot_index` and returns `true`.
    fn realign_slot_to(&mut self, slot_index: SlotIndex) -> bool;
}
