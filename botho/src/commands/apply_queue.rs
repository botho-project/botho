// Copyright (c) 2024 Botho Foundation

//! The node loop's FIFO block-apply queue (#1566, #1550 step 2).
//!
//! Every block apply path (our own externalized decision, a gossiped full
//! block, a reconstructed compact block and a sync batch) becomes one
//! [`ApplyJob`]. Jobs wait here in arrival order. At most one runs at a time,
//! off the node loop in `spawn_blocking`, under the apply mutex. Because a
//! job starts only when none is in flight, commits happen in enqueue order
//! and no two blocks commit concurrently (#558).
//!
//! This module holds the pure queue logic, so it can be unit-tested without
//! a swarm. The spawning, the completion hooks and the node-loop wiring live
//! in `run.rs`.

use crate::{
    block::Block,
    consensus::{ConsensusService, ConsensusValue},
};
use libp2p::PeerId;
use std::{collections::VecDeque, time::Instant};

/// Maximum number of jobs waiting behind the one in flight. When full,
/// gossiped, compact and `BlockTxn` blocks are dropped (sync and the next
/// status round recover them). Consensus and sync jobs are never dropped.
pub(crate) const APPLY_QUEUE_CAPACITY: usize = 16;

/// Where an apply job came from. The completion arm dispatches the
/// per-source post-commit hooks on it.
#[derive(Debug, Clone)]
pub(crate) enum ApplyOrigin {
    /// Our own externalized SCP decision. The only origin whose job runs the
    /// lottery, the only one whose failure can be terminal, and the only one
    /// whose completion calls `advance_slot`.
    Consensus {
        slot_index: u64,
        values: Vec<ConsensusValue>,
    },
    /// A full block from `NetworkEvent::NewBlock`.
    Gossip { received_at: Instant },
    /// A block reconstructed from a compact block, either directly
    /// (`NewCompactBlock`) or after a `BlockTxn` response.
    Compact { block_hash: [u8; 32] },
    /// One `SyncResponse::Blocks` batch, applied in order as one job.
    /// `last_tx_count` is the batch's last block's tx count, which feeds the
    /// dynamic fee exactly as the inline batch loop did.
    Sync { peer: PeerId, last_tx_count: usize },
}

impl ApplyOrigin {
    pub(crate) fn is_consensus(&self) -> bool {
        matches!(self, ApplyOrigin::Consensus { .. })
    }

    pub(crate) fn is_sync(&self) -> bool {
        matches!(self, ApplyOrigin::Sync { .. })
    }

    /// A short label for logs.
    pub(crate) fn label(&self) -> &'static str {
        match self {
            ApplyOrigin::Consensus { .. } => "consensus",
            ApplyOrigin::Gossip { .. } => "gossip",
            ApplyOrigin::Compact { .. } => "compact",
            ApplyOrigin::Sync { .. } => "sync",
        }
    }
}

/// One unit of apply work: the blocks to commit, in order, and their origin.
#[derive(Debug)]
pub(crate) struct ApplyJob {
    pub(crate) origin: ApplyOrigin,
    pub(crate) blocks: Vec<Block>,
}

impl ApplyJob {
    fn top_height(&self) -> Option<u64> {
        self.blocks.iter().map(Block::height).max()
    }
}

/// The job currently running. Its blocks moved into the worker; the queue
/// keeps the origin and the highest height for the projected tip.
#[derive(Debug)]
pub(crate) struct InFlight<H> {
    pub(crate) origin: ApplyOrigin,
    pub(crate) top_height: Option<u64>,
    pub(crate) handle: H,
}

/// The verdict on a gossiped, compact or `BlockTxn` block at enqueue time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Admission {
    /// Queued behind any earlier jobs.
    Enqueued,
    /// At or below the projected tip: a block for that height is already
    /// committed, queued or in flight (gossip N vs compact N vs our own
    /// consensus N).
    Duplicate { projected_tip: u64 },
    /// More than one above the projected tip. It could only fail "Expected
    /// height"; the #423 catch-up path backfills the gap instead.
    Ahead { projected_tip: u64 },
    /// The queue is at [`APPLY_QUEUE_CAPACITY`].
    Full,
}

/// FIFO of apply jobs plus at most one in flight. `H` is the in-flight
/// job's handle (a `JoinHandle` in production, `()` in unit tests).
#[derive(Debug)]
pub(crate) struct ApplyQueue<H> {
    queued: VecDeque<ApplyJob>,
    in_flight: Option<InFlight<H>>,
}

impl<H> Default for ApplyQueue<H> {
    fn default() -> Self {
        Self {
            queued: VecDeque::new(),
            in_flight: None,
        }
    }
}

impl<H> ApplyQueue<H> {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The highest height queued or in flight, or the ledger tip if that is
    /// higher (or nothing is queued). Enqueue-time height checks use this
    /// instead of a ledger read, so gossip N+1 arriving while N is still
    /// queued is contiguous, not a gap.
    pub(crate) fn projected_tip(&self, ledger_tip: u64) -> u64 {
        self.queued
            .iter()
            .filter_map(ApplyJob::top_height)
            .chain(self.in_flight.as_ref().and_then(|job| job.top_height))
            .fold(ledger_tip, u64::max)
    }

    /// `Some(projected_tip)` when a block at `height` is ahead of the
    /// projected tip by a gap gossip cannot bridge (#423 RC2), else `None`.
    pub(crate) fn catch_up_gap(&self, height: u64, ledger_tip: u64) -> Option<u64> {
        let projected = self.projected_tip(ledger_tip);
        (height > projected.saturating_add(1)).then_some(projected)
    }

    /// Admit a gossiped, compact or `BlockTxn` block. Only the block at
    /// exactly projected tip + 1 is queued, and only while there is room.
    pub(crate) fn admit_network_block(
        &mut self,
        origin: ApplyOrigin,
        block: Block,
        ledger_tip: u64,
    ) -> Admission {
        debug_assert!(!origin.is_consensus() && !origin.is_sync());
        let projected_tip = self.projected_tip(ledger_tip);
        let height = block.height();
        if height <= projected_tip {
            return Admission::Duplicate { projected_tip };
        }
        if height > projected_tip.saturating_add(1) {
            return Admission::Ahead { projected_tip };
        }
        if self.queued.len() >= APPLY_QUEUE_CAPACITY {
            return Admission::Full;
        }
        self.queued.push_back(ApplyJob {
            origin,
            blocks: vec![block],
        });
        Admission::Enqueued
    }

    /// Queue our own externalized decision. Never deduplicated and never
    /// dropped: if a gossiped block for the same height commits first, the
    /// completion arm classifies this job's failure as a #421 benign
    /// duplicate, exactly as before.
    pub(crate) fn push_consensus(
        &mut self,
        slot_index: u64,
        values: Vec<ConsensusValue>,
        block: Block,
    ) {
        self.queued.push_back(ApplyJob {
            origin: ApplyOrigin::Consensus { slot_index, values },
            blocks: vec![block],
        });
    }

    /// Queue one sync batch as one job. Not filtered here: the job skips
    /// blocks at or below the tip when it starts (#641 overlap tolerance),
    /// and a batch that was entirely at or below it drives #644
    /// `on_zero_progress`.
    pub(crate) fn push_sync(&mut self, peer: PeerId, blocks: Vec<Block>) {
        let last_tx_count = blocks.last().map_or(0, |b| b.transactions.len());
        self.queued.push_back(ApplyJob {
            origin: ApplyOrigin::Sync {
                peer,
                last_tx_count,
            },
            blocks,
        });
    }

    /// Start the next queued job, but only when nothing is in flight. This
    /// is what orders commits. Returns whether a job was started.
    pub(crate) fn start_next(&mut self, spawn: impl FnOnce(ApplyJob) -> H) -> bool {
        if self.in_flight.is_some() {
            return false;
        }
        let Some(job) = self.queued.pop_front() else {
            return false;
        };
        let origin = job.origin.clone();
        let top_height = job.top_height();
        self.in_flight = Some(InFlight {
            origin,
            top_height,
            handle: spawn(job),
        });
        true
    }

    pub(crate) fn in_flight_mut(&mut self) -> Option<&mut InFlight<H>> {
        self.in_flight.as_mut()
    }

    /// Remove the in-flight job once it has completed.
    pub(crate) fn take_in_flight(&mut self) -> Option<InFlight<H>> {
        self.in_flight.take()
    }

    /// Remove every queued (not started) job, for shutdown.
    pub(crate) fn drain_queued(&mut self) -> Vec<ApplyJob> {
        self.queued.drain(..).collect()
    }

    pub(crate) fn has_in_flight(&self) -> bool {
        self.in_flight.is_some()
    }

    /// Any apply queued or in flight. Gates the minter re-feed: a commit not
    /// yet followed by `remove_invalid` leaves conflicting transfers in the
    /// mempool (#1565 step 9).
    pub(crate) fn is_busy(&self) -> bool {
        self.in_flight.is_some() || !self.queued.is_empty()
    }

    /// A `Consensus` job queued or in flight: the service still holds
    /// `externalized`, so no further decision is taken and slot
    /// reconciles are refused.
    pub(crate) fn consensus_pending(&self) -> bool {
        self.any_origin(ApplyOrigin::is_consensus)
    }

    /// A `Sync` job queued or in flight. While true the loop issues no new
    /// sync `GetBlocks` request: `on_blocks` already cleared the sync
    /// manager's in-flight guard, so a re-request of the same range would be
    /// a pure overlap that #644 would penalize an honest peer for.
    pub(crate) fn sync_outstanding(&self) -> bool {
        self.any_origin(ApplyOrigin::is_sync)
    }

    pub(crate) fn queued_len(&self) -> usize {
        self.queued.len()
    }

    fn any_origin(&self, pred: impl Fn(&ApplyOrigin) -> bool) -> bool {
        self.in_flight.as_ref().is_some_and(|job| pred(&job.origin))
            || self.queued.iter().any(|job| pred(&job.origin))
    }
}

/// The loop-side retry of a refused #770/#419 slot reconcile (#1575).
///
/// `sync_scp_slot_to_chain` refuses while an externalized decision awaits
/// application (#1565). When that refusal happens because a `Consensus` job
/// is queued or in flight, remember it and re-run the reconcile once, right
/// after that job's `advance_slot`. The service's own guards are unchanged:
/// strictly-behind target, no ballot/commit state (#436), so there is still
/// no fast-forward out of Commit.
#[derive(Debug, Default)]
pub(crate) struct SlotReconcile {
    pending: bool,
}

impl SlotReconcile {
    /// Run the reconcile now. Returns whether the slot advanced.
    pub(crate) fn reconcile(
        &mut self,
        consensus: &mut ConsensusService,
        chain_height: u64,
        consensus_apply_pending: bool,
    ) -> bool {
        let advanced = consensus.sync_scp_slot_to_chain(chain_height);
        if !advanced && consensus_apply_pending {
            self.pending = true;
        }
        advanced
    }

    /// Call after `advance_slot`. Re-runs a refused reconcile once against
    /// the current chain height. `None` when nothing was pending, otherwise
    /// whether the slot advanced.
    pub(crate) fn retry_after_advance(
        &mut self,
        consensus: &mut ConsensusService,
        chain_height: u64,
    ) -> Option<bool> {
        if !std::mem::take(&mut self.pending) {
            return None;
        }
        Some(consensus.sync_scp_slot_to_chain(chain_height))
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block_at(height: u64) -> Block {
        let mut block = Block::genesis();
        block.header.height = height;
        block.minting_tx.block_height = height;
        block
    }

    fn gossip() -> ApplyOrigin {
        ApplyOrigin::Gossip {
            received_at: Instant::now(),
        }
    }

    fn compact(tag: u8) -> ApplyOrigin {
        ApplyOrigin::Compact {
            block_hash: [tag; 32],
        }
    }

    /// Start the next job with a handle recording its origin label and
    /// heights, as the production spawner would hand it to the worker.
    fn start(queue: &mut ApplyQueue<(String, Vec<u64>)>) -> bool {
        queue.start_next(|job| {
            (
                job.origin.label().to_string(),
                job.blocks.iter().map(Block::height).collect(),
            )
        })
    }

    fn finish(queue: &mut ApplyQueue<(String, Vec<u64>)>) -> (String, Vec<u64>) {
        queue.take_in_flight().expect("a job is in flight").handle
    }

    #[test]
    fn jobs_start_in_fifo_order_one_at_a_time() {
        let mut queue = ApplyQueue::new();
        assert_eq!(
            queue.admit_network_block(gossip(), block_at(1), 0),
            Admission::Enqueued
        );
        queue.push_consensus(1, vec![], block_at(1));
        queue.push_sync(PeerId::random(), vec![block_at(2), block_at(3)]);
        assert_eq!(
            queue.admit_network_block(compact(4), block_at(4), 0),
            Admission::Enqueued
        );

        let mut order = Vec::new();
        while start(&mut queue) {
            // Single in flight: nothing else starts until this one is taken.
            assert!(!start(&mut queue), "started a second job concurrently");
            assert!(queue.has_in_flight());
            order.push(finish(&mut queue));
        }
        assert_eq!(
            order,
            vec![
                ("gossip".to_string(), vec![1]),
                ("consensus".to_string(), vec![1]),
                ("sync".to_string(), vec![2, 3]),
                ("compact".to_string(), vec![4]),
            ]
        );
        assert!(!queue.is_busy());
    }

    #[test]
    fn projected_tip_covers_queued_and_in_flight_heights() {
        let mut queue: ApplyQueue<(String, Vec<u64>)> = ApplyQueue::new();
        assert_eq!(queue.projected_tip(7), 7);
        queue.push_sync(PeerId::random(), vec![block_at(8), block_at(9)]);
        assert_eq!(queue.projected_tip(7), 9);
        assert!(start(&mut queue));
        assert_eq!(queue.projected_tip(7), 9, "in-flight heights count");
        // The ledger tip wins when it is ahead (e.g. the job already
        // committed but has not been taken yet).
        assert_eq!(queue.projected_tip(12), 12);
    }

    /// The #423 RC2 check uses the projected tip: gossip N+1 arriving while
    /// N is in flight is contiguous, applies after N, and fires no catch-up.
    #[test]
    fn gossip_n_plus_one_while_n_in_flight_is_queued_without_catch_up() {
        let mut queue = ApplyQueue::new();
        let ledger_tip = 4;
        queue.push_consensus(10, vec![], block_at(5));
        assert!(start(&mut queue));

        assert_eq!(
            queue.catch_up_gap(6, ledger_tip),
            None,
            "fired #423 catch-up"
        );
        assert_eq!(
            queue.admit_network_block(compact(6), block_at(6), ledger_tip),
            Admission::Enqueued
        );
        // A real gap is still detected against the projected tip.
        assert_eq!(queue.catch_up_gap(8, ledger_tip), Some(6));

        assert_eq!(finish(&mut queue), ("consensus".to_string(), vec![5]));
        assert!(start(&mut queue));
        assert_eq!(finish(&mut queue), ("compact".to_string(), vec![6]));
    }

    #[test]
    fn duplicate_and_non_contiguous_network_blocks_are_not_queued() {
        let mut queue: ApplyQueue<(String, Vec<u64>)> = ApplyQueue::new();
        // Our own decision for height 3 is queued: gossip 3 and compact 3
        // are duplicates of it.
        queue.push_consensus(3, vec![], block_at(3));
        assert_eq!(
            queue.admit_network_block(gossip(), block_at(3), 2),
            Admission::Duplicate { projected_tip: 3 }
        );
        assert_eq!(
            queue.admit_network_block(compact(3), block_at(3), 2),
            Admission::Duplicate { projected_tip: 3 }
        );
        // At or below the ledger tip.
        assert_eq!(
            queue.admit_network_block(gossip(), block_at(1), 2),
            Admission::Duplicate { projected_tip: 3 }
        );
        // Beyond projected + 1.
        assert_eq!(
            queue.admit_network_block(gossip(), block_at(5), 2),
            Admission::Ahead { projected_tip: 3 }
        );
        assert_eq!(queue.queued_len(), 1);
    }

    #[test]
    fn full_queue_drops_network_blocks_but_never_consensus_or_sync() {
        let mut queue: ApplyQueue<(String, Vec<u64>)> = ApplyQueue::new();
        for height in 1..=APPLY_QUEUE_CAPACITY as u64 {
            assert_eq!(
                queue.admit_network_block(gossip(), block_at(height), 0),
                Admission::Enqueued
            );
        }
        let next = APPLY_QUEUE_CAPACITY as u64 + 1;
        assert_eq!(
            queue.admit_network_block(gossip(), block_at(next), 0),
            Admission::Full
        );
        assert_eq!(
            queue.admit_network_block(compact(1), block_at(next), 0),
            Admission::Full
        );
        queue.push_consensus(1, vec![], block_at(next));
        queue.push_sync(PeerId::random(), vec![block_at(next)]);
        assert_eq!(queue.queued_len(), APPLY_QUEUE_CAPACITY + 2);
        assert!(queue.consensus_pending());
        assert!(queue.sync_outstanding());
    }

    #[test]
    fn predicates_track_queued_and_in_flight_jobs() {
        let mut queue: ApplyQueue<(String, Vec<u64>)> = ApplyQueue::new();
        assert!(!queue.is_busy() && !queue.consensus_pending() && !queue.sync_outstanding());

        queue.push_sync(PeerId::random(), vec![block_at(1)]);
        assert!(queue.is_busy());
        assert!(queue.sync_outstanding(), "a queued sync job is outstanding");
        assert!(!queue.consensus_pending());

        assert!(start(&mut queue));
        assert!(
            queue.sync_outstanding(),
            "an in-flight sync job is outstanding"
        );

        queue.push_consensus(1, vec![], block_at(2));
        assert!(
            queue.consensus_pending(),
            "a queued consensus job is pending"
        );
        finish(&mut queue);
        assert!(!queue.sync_outstanding(), "sync job completed");
        assert!(queue.consensus_pending());
        assert!(start(&mut queue));
        assert!(
            queue.consensus_pending(),
            "an in-flight consensus job is pending"
        );
        finish(&mut queue);
        assert!(!queue.is_busy());

        // A gossip-only queue is busy (gates the minter re-feed) but holds
        // neither a decision nor a sync batch.
        queue.admit_network_block(gossip(), block_at(1), 0);
        assert!(queue.is_busy());
        assert!(!queue.consensus_pending() && !queue.sync_outstanding());
        assert_eq!(queue.drain_queued().len(), 1);
        assert!(!queue.is_busy());
    }
}
