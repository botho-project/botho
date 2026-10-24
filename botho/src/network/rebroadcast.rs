// Copyright (c) 2024 Botho Foundation

//! Per-transaction rebroadcast schedule (#1547).
//!
//! The node re-publishes pending mempool transfers so a tx submitted while the
//! node had no (or stale) peers still propagates (#674). Before #1547 those
//! re-publishes had no per-tx memory. The 100 ms minting-check arm re-fed
//! `get_pending_transactions(10)` to gossip on every accepted minted tx. While
//! SCP was stalled at one height the minter kept finding current-work-version
//! solutions, so the same pending transfers went out up to 10x per second.
//! Gossipsub message ids are source+seqno, so every re-publish was a fresh
//! message for peers. In the w12 run that showed up as about 108 tx msgs/s per
//! peer dropped at the gossip rate limiter.
//!
//! [`RebroadcastSchedule`] gates every *re*-broadcast of a pending transfer:
//!
//! - **Exponential backoff per tx.** After the k-th send the tx is not sent
//!   again for `base * 2^(k-1)`, capped at `cap` (30 s, 60 s, 120 s, ... 10 min
//!   by default).
//! - **Stall gate.** While the SCP slot is stalled nothing can confirm, so
//!   re-broadcasts are pointless load. [`RebroadcastSchedule::should_send`]
//!   returns `false` for every tx while the stall flag is set.
//! - **Reset on recovery.** When the stall clears (the slot externalized again)
//!   all backoff state is dropped, so every still-pending tx gets one immediate
//!   send instead of waiting out a backoff of up to `cap`.
//! - **Pruning.** Entries for txs that left the mempool (confirmed or evicted)
//!   are removed via [`RebroadcastSchedule::prune`].
//!
//! The first RPC relay of a tx is never gated. It calls
//! [`RebroadcastSchedule::record_send`] to seed the schedule.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

/// First backoff interval after a tx has been sent once.
pub const REBROADCAST_BASE_INTERVAL: Duration = Duration::from_secs(30);

/// Upper bound on the per-tx backoff interval.
pub const REBROADCAST_MAX_INTERVAL: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, Copy)]
struct Entry {
    last_sent: Instant,
    /// Number of sends recorded so far (always >= 1 once an entry exists).
    sends: u32,
}

/// Per-tx rebroadcast backoff with an SCP-stall gate. See the module docs.
#[derive(Debug)]
pub struct RebroadcastSchedule {
    entries: HashMap<[u8; 32], Entry>,
    base: Duration,
    cap: Duration,
    slot_stalled: bool,
}

impl Default for RebroadcastSchedule {
    fn default() -> Self {
        Self::new()
    }
}

impl RebroadcastSchedule {
    /// Schedule with the default 30 s base / 10 min cap.
    pub fn new() -> Self {
        Self::with_intervals(REBROADCAST_BASE_INTERVAL, REBROADCAST_MAX_INTERVAL)
    }

    /// Schedule with custom intervals. `cap` is clamped to at least `base`.
    pub fn with_intervals(base: Duration, cap: Duration) -> Self {
        Self {
            entries: HashMap::new(),
            base,
            cap: cap.max(base),
            slot_stalled: false,
        }
    }

    /// Wait required after a tx has been sent `sends` times.
    pub fn backoff_after(&self, sends: u32) -> Duration {
        let shift = sends.saturating_sub(1).min(31);
        self.base
            .checked_mul(1u32 << shift)
            .unwrap_or(self.cap)
            .min(self.cap)
    }

    /// Record an unconditional send, e.g. the first RPC relay of a tx. This
    /// seeds the schedule so later re-broadcasts back off from `now`.
    pub fn record_send(&mut self, hash: [u8; 32], now: Instant) {
        let entry = self.entries.entry(hash).or_insert(Entry {
            last_sent: now,
            sends: 0,
        });
        entry.last_sent = now;
        entry.sends = entry.sends.saturating_add(1);
    }

    /// Whether a re-broadcast of `hash` should go out now. Records the send
    /// when it returns `true`.
    ///
    /// Returns `false` for every tx while the SCP slot is stalled. A tx the
    /// schedule has never seen is due immediately.
    pub fn should_send(&mut self, hash: [u8; 32], now: Instant) -> bool {
        if self.slot_stalled {
            return false;
        }
        let due = match self.entries.get(&hash) {
            None => true,
            Some(e) => now.saturating_duration_since(e.last_sent) >= self.backoff_after(e.sends),
        };
        if due {
            self.record_send(hash, now);
        }
        due
    }

    /// Update the SCP stall flag from the latest slot snapshot.
    ///
    /// Returns `true` on the stalled -> not-stalled transition. At that point
    /// all backoff state is cleared, so each still-pending tx gets one
    /// immediate send.
    pub fn set_slot_stalled(&mut self, stalled: bool) -> bool {
        let recovered = self.slot_stalled && !stalled;
        if recovered {
            self.entries.clear();
        }
        self.slot_stalled = stalled;
        recovered
    }

    /// Whether re-broadcasts are currently paused by the stall gate.
    pub fn is_paused(&self) -> bool {
        self.slot_stalled
    }

    /// Drop entries for txs that are no longer pending. `still_pending`
    /// returns `true` for hashes that should be kept. Returns the number of
    /// entries removed.
    pub fn prune(&mut self, mut still_pending: impl FnMut(&[u8; 32]) -> bool) -> usize {
        let before = self.entries.len();
        self.entries.retain(|hash, _| still_pending(hash));
        before - self.entries.len()
    }

    /// Number of txs currently tracked.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no txs are tracked.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H1: [u8; 32] = [1u8; 32];
    const H2: [u8; 32] = [2u8; 32];

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn unknown_tx_is_sent_immediately_then_backs_off() {
        let mut s = RebroadcastSchedule::new();
        let t0 = Instant::now();
        assert!(s.should_send(H1, t0), "first send of an unseen tx goes out");
        assert!(!s.should_send(H1, t0), "no immediate repeat");
        assert!(!s.should_send(H1, t0 + secs(29)), "inside 30 s backoff");
        assert!(s.should_send(H1, t0 + secs(30)), "due at 30 s");
    }

    #[test]
    fn backoff_doubles_and_caps() {
        let s = RebroadcastSchedule::new();
        assert_eq!(s.backoff_after(1), secs(30));
        assert_eq!(s.backoff_after(2), secs(60));
        assert_eq!(s.backoff_after(3), secs(120));
        assert_eq!(s.backoff_after(4), secs(240));
        assert_eq!(s.backoff_after(5), secs(480));
        assert_eq!(s.backoff_after(6), secs(600), "capped at 10 min");
        assert_eq!(s.backoff_after(50), secs(600), "no overflow far past cap");
        assert_eq!(s.backoff_after(u32::MAX), secs(600));
    }

    #[test]
    fn schedule_follows_doubling_intervals() {
        let mut s = RebroadcastSchedule::new();
        let mut t = Instant::now();
        assert!(s.should_send(H1, t));
        for expected in [30u64, 60, 120, 240, 480, 600, 600] {
            assert!(!s.should_send(H1, t + secs(expected - 1)));
            t += secs(expected);
            assert!(s.should_send(H1, t), "due after {expected} s");
        }
    }

    /// The w12 flood shape: a re-feed every 100 ms. Over ten minutes the tx
    /// goes out only on the backoff schedule (0, 30, 90, 210, 450 s), not
    /// 6,000 times.
    #[test]
    fn hundred_ms_refeed_is_bounded_by_backoff() {
        let mut s = RebroadcastSchedule::new();
        let t0 = Instant::now();
        let sends = (0..6_000u64)
            .filter(|i| s.should_send(H1, t0 + Duration::from_millis(i * 100)))
            .count();
        assert_eq!(sends, 5);
    }

    #[test]
    fn record_send_seeds_the_schedule() {
        let mut s = RebroadcastSchedule::new();
        let t0 = Instant::now();
        s.record_send(H1, t0); // first RPC relay
        assert!(!s.should_send(H1, t0 + secs(1)));
        assert!(s.should_send(H1, t0 + secs(30)));
        // Second re-broadcast backs off 60 s from the last send.
        assert!(!s.should_send(H1, t0 + secs(89)));
        assert!(s.should_send(H1, t0 + secs(90)));
    }

    #[test]
    fn txs_are_scheduled_independently() {
        let mut s = RebroadcastSchedule::new();
        let t0 = Instant::now();
        assert!(s.should_send(H1, t0));
        assert!(s.should_send(H2, t0 + secs(10)));
        assert!(s.should_send(H1, t0 + secs(30)));
        assert!(!s.should_send(H2, t0 + secs(30)));
        assert!(s.should_send(H2, t0 + secs(40)));
    }

    #[test]
    fn prune_drops_txs_no_longer_pending() {
        let mut s = RebroadcastSchedule::new();
        let t0 = Instant::now();
        s.record_send(H1, t0);
        s.record_send(H2, t0);
        assert_eq!(s.len(), 2);
        let removed = s.prune(|h| *h == H2);
        assert_eq!(removed, 1);
        assert_eq!(s.len(), 1);
        // A pruned tx that shows up again is treated as unseen.
        assert!(s.should_send(H1, t0 + secs(1)));
        assert!(!s.should_send(H2, t0 + secs(1)));
    }

    #[test]
    fn stall_gate_suppresses_all_rebroadcasts() {
        let mut s = RebroadcastSchedule::new();
        let t0 = Instant::now();
        s.record_send(H1, t0);
        assert!(!s.set_slot_stalled(true));
        assert!(s.is_paused());
        // Neither a due tx nor an unseen tx goes out while stalled.
        assert!(!s.should_send(H1, t0 + secs(3_600)));
        assert!(!s.should_send(H2, t0 + secs(3_600)));
        // Staying stalled is not a recovery.
        assert!(!s.set_slot_stalled(true));
    }

    #[test]
    fn recovery_resets_backoff_for_an_immediate_send() {
        let mut s = RebroadcastSchedule::new();
        let mut t = Instant::now();
        // Drive H1 deep into backoff (next send would be 10 min out).
        assert!(s.should_send(H1, t));
        for step in [30u64, 60, 120, 240, 480] {
            t += secs(step);
            assert!(s.should_send(H1, t));
        }
        assert!(!s.should_send(H1, t + secs(60)));

        s.set_slot_stalled(true);
        assert!(s.set_slot_stalled(false), "stalled -> clear is a recovery");
        assert!(!s.is_paused());
        assert!(s.is_empty(), "backoff state cleared on recovery");
        assert!(
            s.should_send(H1, t + secs(60)),
            "one immediate send after externalization"
        );
        assert!(!s.should_send(H1, t + secs(61)), "then backoff restarts");
        assert!(s.should_send(H1, t + secs(90)));
    }

    #[test]
    fn clear_without_prior_stall_keeps_state() {
        let mut s = RebroadcastSchedule::new();
        let t0 = Instant::now();
        s.record_send(H1, t0);
        assert!(!s.set_slot_stalled(false));
        assert_eq!(s.len(), 1);
        assert!(!s.should_send(H1, t0 + secs(1)));
    }
}
