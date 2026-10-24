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
//!   by default). This alone bounds the flood: a 100 ms re-feed over 10 min
//!   sends a tx 5 times, not 6,000.
//! - **No stall gate.** Re-broadcasts keep going on the backoff schedule while
//!   the SCP slot is stalled. A peer validates a nominated transfer only if it
//!   holds the tx bytes, and gossip is the only way it gets them. If a
//!   validator missed the first relay, the slot stalls on that tx, and the
//!   re-send is what heals the stall. An earlier draft of #1547 suppressed all
//!   re-broadcasts while stalled. That turned such a stall into a permanent
//!   halt.
//! - **Reset on recovery.** When a stall clears (the slot externalized again)
//!   all backoff state is dropped, so every still-pending tx gets one immediate
//!   send instead of waiting out a backoff of up to `cap`.
//! - **Reset on peer connect.** A newly connected peer has seen none of the
//!   pending txs. [`RebroadcastSchedule::reset_all`] drops the backoff state so
//!   the peer gets them at once, not after up to `cap` (#674). This also covers
//!   a validator that restarted with an empty tx cache.
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

/// Per-tx rebroadcast backoff, reset on SCP recovery and peer connect. See
/// the module docs.
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
    /// Only the per-tx backoff decides. The SCP stall flag does not suppress
    /// sends (see the module docs). A tx the schedule has never seen is due
    /// immediately.
    pub fn should_send(&mut self, hash: [u8; 32], now: Instant) -> bool {
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

    /// The last SCP stall flag passed to [`Self::set_slot_stalled`]. Used for
    /// logging only. It does not gate sends.
    pub fn slot_stalled(&self) -> bool {
        self.slot_stalled
    }

    /// Drop all backoff state, so every still-pending tx is due at once.
    /// Called when a peer connects (#674). Returns the number of entries
    /// cleared.
    pub fn reset_all(&mut self) -> usize {
        let cleared = self.entries.len();
        self.entries.clear();
        cleared
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

    /// Regression for the #1554 review blocker. A validator missed the single
    /// first relay of H1, so the slot stalls on H1. The re-sends must keep
    /// going on the backoff schedule while stalled, because they are the only
    /// way the validator gets the tx bytes and the slot can externalize.
    #[test]
    fn stalled_slot_still_rebroadcasts_on_schedule() {
        let mut s = RebroadcastSchedule::new();
        let t0 = Instant::now();
        s.record_send(H1, t0); // the first relay, missed by one validator
                               // The stall trips after 3 slot durations, before the first retry.
        assert!(!s.set_slot_stalled(true));
        assert!(s.slot_stalled());
        assert!(!s.should_send(H1, t0 + secs(29)), "backoff still applies");
        assert!(s.should_send(H1, t0 + secs(30)), "re-sent while stalled");
        assert!(!s.should_send(H1, t0 + secs(89)));
        assert!(s.should_send(H1, t0 + secs(90)), "and again at 60 s");
        // An unseen tx is sent at once even while stalled.
        assert!(s.should_send(H2, t0 + secs(90)));
        // Staying stalled is not a recovery.
        assert!(!s.set_slot_stalled(true));
    }

    /// While stalled the backoff still bounds a 100 ms re-feed: the same 5
    /// sends in 10 min as an unstalled node.
    #[test]
    fn stalled_hundred_ms_refeed_is_bounded_by_backoff() {
        let mut s = RebroadcastSchedule::new();
        s.set_slot_stalled(true);
        let t0 = Instant::now();
        let sends = (0..6_000u64)
            .filter(|i| s.should_send(H1, t0 + Duration::from_millis(i * 100)))
            .count();
        assert_eq!(sends, 5);
    }

    /// A peer that connects late gets long-pending txs at once instead of
    /// waiting out a backoff of up to 10 min (#674).
    #[test]
    fn peer_connect_reset_makes_pending_txs_due() {
        let mut s = RebroadcastSchedule::new();
        let mut t = Instant::now();
        assert!(s.should_send(H1, t));
        for step in [30u64, 60, 120, 240, 480] {
            t += secs(step);
            assert!(s.should_send(H1, t));
        }
        s.record_send(H2, t);
        assert!(!s.should_send(H1, t + secs(1)), "next send is 10 min out");

        assert_eq!(s.reset_all(), 2);
        assert!(s.is_empty());
        assert!(s.should_send(H1, t + secs(1)), "due at once after connect");
        assert!(s.should_send(H2, t + secs(1)));
        assert!(!s.should_send(H1, t + secs(2)), "then backoff restarts");
        assert!(s.should_send(H1, t + secs(31)));
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
        assert!(!s.slot_stalled());
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
