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
//! Our own decision for a height more than one above the ledger tip is
//! **parked** outside the FIFO (#1583): the node is behind, not broken. The
//! gap blocks (sync or gossip) keep flowing through the FIFO, and the parked
//! job runs once [`consensus_readiness`] says its parent is committed.
//!
//! This module holds the pure queue logic, so it can be unit-tested without
//! a swarm. The spawning, the completion hooks and the node-loop wiring live
//! in `run.rs`.

use crate::{
    block::Block,
    consensus::{ConsensusService, ConsensusValue},
};
use libp2p::PeerId;
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

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

/// Whether a `Consensus` job can be dispatched now (#1583).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Readiness {
    /// Its parent is committed (H = tip + 1), or its height is already
    /// filled (H <= tip). Either way it runs: the unchanged #421
    /// `height_already_applied` classification calls the second case a
    /// benign duplicate.
    Ready,
    /// Its parent is not committed yet (H > tip + 1). Running it could only
    /// fail "Expected height", which is not a consensus failure: the node is
    /// behind. Park it until the gap is filled.
    Park,
}

/// The dispatch rule for our own decision (#1583). Evaluated against the
/// REAL ledger tip at dispatch time (nothing in flight), never against the
/// projected tip: a queued job ahead of it can still fail partway.
pub(crate) fn consensus_readiness(block_height: u64, ledger_tip: u64) -> Readiness {
    if block_height <= ledger_tip.saturating_add(1) {
        Readiness::Ready
    } else {
        Readiness::Park
    }
}

/// A `Consensus` job waiting outside the FIFO for its parent (#1583). The
/// block was built at externalize time, so tx-cache eviction while waiting
/// does not matter.
#[derive(Debug)]
struct ParkedConsensus {
    job: ApplyJob,
    parked_at: Instant,
    /// How many parked-too-long warnings have been reported.
    warnings: u32,
}

/// What [`ApplyQueue::dispatch_next`] did.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Dispatch {
    /// A job was started.
    pub(crate) started: bool,
    /// A `Consensus` job was parked by this call (not a deferred second one).
    pub(crate) parked: Option<ParkedDecision>,
}

/// A summary of the parked decision, for logs and the sync hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ParkedDecision {
    pub(crate) slot_index: u64,
    /// The decided block's height, H.
    pub(crate) height: u64,
    /// The decided block's parent hash: the block at H - 1 that sync must
    /// deliver (and that `add_block` checks it against, #420).
    pub(crate) prev_block_hash: [u8; 32],
    /// How long it has been parked (at the time of the query).
    pub(crate) parked_for: Duration,
}

/// FIFO of apply jobs plus at most one in flight, and the parked decisions
/// (#1583). `H` is the in-flight job's handle (a `JoinHandle` in production,
/// `()` in unit tests).
#[derive(Debug)]
pub(crate) struct ApplyQueue<H> {
    queued: VecDeque<ApplyJob>,
    in_flight: Option<InFlight<H>>,
    /// Parked `Consensus` jobs. Only the front is ever evaluated; a second
    /// decision (which the service cannot surface while one is pending) is
    /// deferred behind it, never replaces it and is never dropped.
    parked: VecDeque<ParkedConsensus>,
}

impl<H> Default for ApplyQueue<H> {
    fn default() -> Self {
        Self {
            queued: VecDeque::new(),
            in_flight: None,
            parked: VecDeque::new(),
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
    ///
    /// #1583: a parked decision never counts, and neither does a queued
    /// `Consensus` job more than one above the projection so far (it will
    /// park at dispatch). Counting either would drop the very gap blocks
    /// (gossip or sync, heights in (tip, H-1]) it is waiting on as
    /// duplicates, and the gap would never fill.
    pub(crate) fn projected_tip(&self, ledger_tip: u64) -> u64 {
        let mut projected = self
            .in_flight
            .as_ref()
            .and_then(|job| job.top_height)
            .map_or(ledger_tip, |height| height.max(ledger_tip));
        for job in &self.queued {
            let Some(height) = job.top_height() else {
                continue;
            };
            if job.origin.is_consensus()
                && consensus_readiness(height, projected) == Readiness::Park
            {
                continue;
            }
            projected = projected.max(height);
        }
        projected
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

    /// Start the next queued job without the #1583 readiness rule (every
    /// `Consensus` job is treated as ready). Returns whether a job started.
    #[cfg(test)]
    pub(crate) fn start_next(&mut self, spawn: impl FnOnce(ApplyJob) -> H) -> bool {
        self.dispatch_next(|| None, Instant::now(), spawn).started
    }

    /// Start the next job, but only when nothing is in flight. This is what
    /// orders commits.
    ///
    /// #1583: a ready parked decision goes first, ahead of the FIFO. A
    /// `Consensus` job reaching the head of the FIFO is checked with
    /// [`consensus_readiness`] against `ledger_tip` (read lazily, only when
    /// a decision is involved; `None`, an unreadable ledger, counts as
    /// ready, the pre-#1583 behaviour). If it is not ready it moves to the
    /// parked slot and the FIFO carries on, so it never blocks the gap
    /// fill. Because the ledger tip only moves when a job completes, calling
    /// this after every completion re-checks the parked decision exactly
    /// when its readiness can change.
    pub(crate) fn dispatch_next(
        &mut self,
        ledger_tip: impl FnOnce() -> Option<u64>,
        now: Instant,
        spawn: impl FnOnce(ApplyJob) -> H,
    ) -> Dispatch {
        let mut dispatch = Dispatch::default();
        if self.in_flight.is_some() {
            return dispatch;
        }
        // Read the ledger tip at most once, and only if a decision is
        // involved.
        let mut read_tip = Some(ledger_tip);
        let mut tip: Option<Option<u64>> = None;
        let mut ready = |height: u64| {
            tip.get_or_insert_with(|| read_tip.take().and_then(|read| read()))
                .is_none_or(|tip| consensus_readiness(height, tip) == Readiness::Ready)
        };

        let mut next = None;
        if let Some(front) = self.parked.front() {
            if ready(front.job.top_height().unwrap_or_default()) {
                next = self.parked.pop_front().map(|parked| parked.job);
            }
        }
        while next.is_none() {
            let Some(job) = self.queued.pop_front() else {
                break;
            };
            if job.origin.is_consensus() {
                if !self.parked.is_empty() {
                    // Never overwrite or drop the parked decision: defer
                    // this one behind it, in order.
                    self.parked.push_back(ParkedConsensus {
                        job,
                        parked_at: now,
                        warnings: 0,
                    });
                    continue;
                }
                if !ready(job.top_height().unwrap_or_default()) {
                    let parked = ParkedConsensus {
                        job,
                        parked_at: now,
                        warnings: 0,
                    };
                    dispatch.parked = Self::describe(&parked, now);
                    self.parked.push_back(parked);
                    continue;
                }
            }
            next = Some(job);
        }

        if let Some(job) = next {
            let origin = job.origin.clone();
            let top_height = job.top_height();
            self.in_flight = Some(InFlight {
                origin,
                top_height,
                handle: spawn(job),
            });
            dispatch.started = true;
        }
        dispatch
    }

    fn describe(parked: &ParkedConsensus, now: Instant) -> Option<ParkedDecision> {
        let ApplyOrigin::Consensus { slot_index, .. } = &parked.job.origin else {
            return None;
        };
        let block = parked.job.blocks.first()?;
        Some(ParkedDecision {
            slot_index: *slot_index,
            height: block.height(),
            prev_block_hash: block.header.prev_block_hash,
            parked_for: now.saturating_duration_since(parked.parked_at),
        })
    }

    /// The decision parked at the front, if any (#1583).
    pub(crate) fn parked(&self, now: Instant) -> Option<ParkedDecision> {
        self.parked
            .front()
            .and_then(|parked| Self::describe(parked, now))
    }

    /// Report the front parked decision once per `every` it has been parked:
    /// first at `every`, then at each further multiple (#1583). `None` when
    /// nothing is parked or no new interval has elapsed.
    pub(crate) fn parked_warning_due(
        &mut self,
        now: Instant,
        every: Duration,
    ) -> Option<ParkedDecision> {
        if every.is_zero() {
            return None;
        }
        let parked = self.parked.front_mut()?;
        let parked_for = now.saturating_duration_since(parked.parked_at);
        let due = every
            .checked_mul(parked.warnings.saturating_add(1))
            .is_some_and(|threshold| parked_for >= threshold);
        if !due {
            return None;
        }
        parked.warnings = parked.warnings.saturating_add(1);
        let parked = self.parked.front()?;
        Self::describe(parked, now)
    }

    /// Remove every parked decision, for shutdown. Parking is not a failure:
    /// nothing is captured, and sync recovers the decision on restart.
    pub(crate) fn drain_parked(&mut self) -> Vec<ApplyJob> {
        self.parked.drain(..).map(|parked| parked.job).collect()
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

    /// Any apply queued, in flight or parked. Gates the minter re-feed: a
    /// commit not yet followed by `remove_invalid` leaves conflicting
    /// transfers in the mempool (#1565 step 9), and a parked decision is
    /// such a commit still to come (#1583).
    pub(crate) fn is_busy(&self) -> bool {
        self.in_flight.is_some() || !self.queued.is_empty() || !self.parked.is_empty()
    }

    /// A `Consensus` job queued, in flight or parked: the service still
    /// holds `externalized`, so no further decision is taken and slot
    /// reconciles are refused.
    pub(crate) fn consensus_pending(&self) -> bool {
        !self.parked.is_empty() || self.any_origin(ApplyOrigin::is_consensus)
    }

    /// A decision is parked waiting for its parent (#1583).
    #[cfg(test)]
    pub(crate) fn has_parked(&self) -> bool {
        !self.parked.is_empty()
    }

    /// A `Sync` job queued or in flight. While true the loop issues no new
    /// sync `GetBlocks` request: `on_blocks` already cleared the sync
    /// manager's in-flight guard, so a re-request of the same range would be
    /// a pure overlap that #644 would penalize an honest peer for.
    pub(crate) fn sync_outstanding(&self) -> bool {
        self.any_origin(ApplyOrigin::is_sync)
    }

    /// Whether the loop must hold sync `GetBlocks` requests: a `Sync` job
    /// is outstanding, or a `Consensus` job is queued or in flight (#1583).
    /// The latter's commit moves the tip a request would be computed from,
    /// so a batch landing after it could be entirely at or below the tip
    /// and drive a #644 penalty against an honest peer. A PARKED decision
    /// does not hold requests: sync is exactly what fills its gap.
    pub(crate) fn holds_sync_requests(&self) -> bool {
        self.sync_outstanding() || self.any_origin(ApplyOrigin::is_consensus)
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

    // ---- #1583: parked decisions ----

    /// Dispatch against `tip` and record the started job as `start` does.
    fn dispatch(queue: &mut ApplyQueue<(String, Vec<u64>)>, tip: u64, now: Instant) -> Dispatch {
        queue.dispatch_next(
            || Some(tip),
            now,
            |job| {
                (
                    job.origin.label().to_string(),
                    job.blocks.iter().map(Block::height).collect(),
                )
            },
        )
    }

    fn consensus_block(height: u64, prev: u8) -> Block {
        let mut block = block_at(height);
        block.header.prev_block_hash = [prev; 32];
        block
    }

    #[test]
    fn consensus_readiness_truth_table() {
        assert_eq!(consensus_readiness(0, 0), Readiness::Ready, "filled");
        assert_eq!(consensus_readiness(5, 7), Readiness::Ready, "filled");
        assert_eq!(consensus_readiness(7, 7), Readiness::Ready, "filled");
        assert_eq!(consensus_readiness(8, 7), Readiness::Ready, "tip + 1");
        assert_eq!(consensus_readiness(9, 7), Readiness::Park, "tip + 2");
        assert_eq!(consensus_readiness(100, 7), Readiness::Park);
        assert_eq!(consensus_readiness(u64::MAX, u64::MAX), Readiness::Ready);
    }

    #[test]
    fn decision_above_tip_plus_one_parks_and_does_not_block_the_fifo() {
        let mut queue = ApplyQueue::new();
        let now = Instant::now();
        queue.push_consensus(12, vec![], consensus_block(6, 9));
        queue.push_sync(PeerId::random(), vec![block_at(5)]);

        // Tip 4: the decision at 6 parks; the sync job behind it starts.
        let outcome = dispatch(&mut queue, 4, now);
        assert!(outcome.started);
        assert_eq!(
            outcome.parked,
            Some(ParkedDecision {
                slot_index: 12,
                height: 6,
                prev_block_hash: [9; 32],
                parked_for: Duration::ZERO,
            })
        );
        assert!(queue.has_parked());
        assert!(queue.consensus_pending(), "a parked decision is pending");
        assert!(
            queue.is_busy(),
            "a parked decision gates the minter re-feed"
        );
        assert!(
            !queue.holds_sync_requests() || queue.sync_outstanding(),
            "only the sync job may hold requests"
        );
        assert_eq!(finish(&mut queue), ("sync".to_string(), vec![5]));
        assert!(
            !queue.holds_sync_requests(),
            "parked alone holds no request"
        );

        // Re-evaluated after the completion: the sync job did not commit
        // (tip still 4), so it stays parked and nothing starts.
        let outcome = dispatch(&mut queue, 4, now);
        assert_eq!(outcome, Dispatch::default());
        assert!(queue.has_parked());

        // The tip reached 5: the parked decision is dispatched, and the
        // dispatch reports no new parking.
        let outcome = dispatch(&mut queue, 5, now);
        assert!(outcome.started && outcome.parked.is_none());
        assert!(!queue.has_parked());
        assert!(
            queue.holds_sync_requests(),
            "a ready decision in flight holds requests"
        );
        assert_eq!(finish(&mut queue), ("consensus".to_string(), vec![6]));
        assert!(!queue.is_busy() && !queue.consensus_pending());
    }

    #[test]
    fn ready_parked_decision_is_dispatched_before_the_fifo() {
        let mut queue = ApplyQueue::new();
        let now = Instant::now();
        queue.push_consensus(3, vec![], block_at(6));
        assert!(!dispatch(&mut queue, 4, now).started, "parked, FIFO empty");
        queue.push_sync(PeerId::random(), vec![block_at(5)]);
        assert!(dispatch(&mut queue, 4, now).started);
        finish(&mut queue);
        // Queue more FIFO work; the now-ready decision still goes first.
        queue.push_sync(PeerId::random(), vec![block_at(6), block_at(7)]);
        assert!(dispatch(&mut queue, 5, now).started);
        assert_eq!(finish(&mut queue), ("consensus".to_string(), vec![6]));
        assert!(dispatch(&mut queue, 6, now).started);
        assert_eq!(finish(&mut queue), ("sync".to_string(), vec![6, 7]));
    }

    #[test]
    fn decisions_at_or_below_tip_plus_one_run_and_are_classified_downstream() {
        let mut queue = ApplyQueue::new();
        let now = Instant::now();
        queue.push_consensus(1, vec![], block_at(5));
        let outcome = dispatch(&mut queue, 4, now);
        assert!(outcome.started && outcome.parked.is_none(), "tip + 1 runs");
        finish(&mut queue);
        queue.push_consensus(2, vec![], block_at(3));
        let outcome = dispatch(&mut queue, 4, now);
        assert!(
            outcome.started && outcome.parked.is_none(),
            "a filled height runs (#421 benign duplicate)"
        );
        finish(&mut queue);
        // An unreadable ledger keeps the pre-#1583 behaviour: run it.
        queue.push_consensus(3, vec![], block_at(50));
        assert!(
            queue
                .dispatch_next(|| None, now, |_| (String::new(), vec![]))
                .started
        );
    }

    /// The #1593 review note: gap blocks below a too-far-ahead decision must
    /// not be dropped as duplicates, whether it is parked or still queued.
    #[test]
    fn parked_and_far_ahead_queued_decisions_do_not_count_toward_projected_tip() {
        let mut queue = ApplyQueue::new();
        let now = Instant::now();
        let tip = 4;

        // Still queued behind an in-flight gossip job (projected 5): the
        // decision at 7 is far ahead and must not count.
        assert_eq!(
            queue.admit_network_block(gossip(), block_at(5), tip),
            Admission::Enqueued
        );
        assert!(dispatch(&mut queue, tip, now).started);
        queue.push_consensus(9, vec![], block_at(7));
        assert_eq!(queue.projected_tip(tip), 5);
        assert_eq!(
            queue.admit_network_block(gossip(), block_at(6), tip),
            Admission::Enqueued,
            "gap block 6 was dropped behind a queued decision at 7"
        );
        // The decision sits AHEAD of gossip 6 in the FIFO, so it would reach
        // the head with at most 5 committed and park: it still does not count.
        assert_eq!(queue.projected_tip(tip), 6);

        // Gossip 5 failed to commit (tip still 4): the decision parks at
        // dispatch, and the projected tip no longer includes it.
        finish(&mut queue);
        let outcome = dispatch(&mut queue, tip, now);
        assert!(outcome.parked.is_some(), "decision at 7 must park at tip 4");
        assert_eq!(finish(&mut queue), ("gossip".to_string(), vec![6]));
        assert_eq!(queue.projected_tip(tip), tip);
        assert_eq!(queue.catch_up_gap(6, tip), Some(tip));
        assert_eq!(
            queue.admit_network_block(compact(5), block_at(5), tip),
            Admission::Enqueued,
            "gap block 5 was dropped below a parked decision"
        );
        assert_eq!(queue.projected_tip(tip), 5);
    }

    #[test]
    fn a_second_decision_defers_behind_the_parked_one_and_never_replaces_it() {
        let mut queue = ApplyQueue::new();
        let now = Instant::now();
        queue.push_consensus(10, vec![], consensus_block(6, 1));
        assert!(dispatch(&mut queue, 4, now).parked.is_some());
        // Defensive case (the service cannot surface it): a ready decision.
        queue.push_consensus(11, vec![], consensus_block(5, 2));
        let outcome = dispatch(&mut queue, 4, now);
        assert_eq!(
            outcome,
            Dispatch::default(),
            "deferred, not run or reported"
        );
        assert_eq!(queue.parked(now).map(|p| p.slot_index), Some(10));
        assert_eq!(queue.queued_len(), 0);

        // Both survive to shutdown, in order.
        let drained = queue.drain_parked();
        let slots: Vec<_> = drained
            .iter()
            .map(|job| match job.origin {
                ApplyOrigin::Consensus { slot_index, .. } => slot_index,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(slots, vec![10, 11]);
        assert!(!queue.is_busy() && !queue.consensus_pending());
    }

    #[test]
    fn deferred_decision_runs_after_the_parked_one() {
        let mut queue = ApplyQueue::new();
        let now = Instant::now();
        queue.push_consensus(10, vec![], block_at(6));
        dispatch(&mut queue, 4, now);
        queue.push_consensus(11, vec![], block_at(7));
        dispatch(&mut queue, 4, now);
        assert!(dispatch(&mut queue, 5, now).started);
        assert_eq!(finish(&mut queue), ("consensus".to_string(), vec![6]));
        assert!(dispatch(&mut queue, 6, now).started);
        assert_eq!(finish(&mut queue), ("consensus".to_string(), vec![7]));
    }

    #[test]
    fn parked_warning_fires_once_per_interval() {
        let mut queue: ApplyQueue<(String, Vec<u64>)> = ApplyQueue::new();
        let t0 = Instant::now();
        let every = Duration::from_secs(60);
        assert!(
            queue.parked_warning_due(t0, every).is_none(),
            "nothing parked"
        );
        queue.push_consensus(4, vec![], block_at(9));
        dispatch(&mut queue, 1, t0);

        let at = |secs| t0 + Duration::from_secs(secs);
        assert!(queue.parked_warning_due(at(59), every).is_none());
        let first = queue
            .parked_warning_due(at(60), every)
            .expect("due at 60 s");
        assert_eq!((first.slot_index, first.height), (4, 9));
        assert_eq!(first.parked_for, Duration::from_secs(60));
        assert!(queue.parked_warning_due(at(61), every).is_none());
        assert!(queue.parked_warning_due(at(119), every).is_none());
        assert!(queue.parked_warning_due(at(120), every).is_some());
        assert!(queue.parked_warning_due(at(120), every).is_none());
        assert!(queue.parked_warning_due(at(185), every).is_some());
        assert_eq!(
            queue.parked(at(185)).unwrap().parked_for,
            Duration::from_secs(185)
        );
    }
}
