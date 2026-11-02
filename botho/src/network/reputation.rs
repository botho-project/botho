// Copyright (c) 2024 Botho Foundation

//! Peer reputation tracking based on response latency and reliability.
//!
//! Tracks response times using exponential moving average and counts
//! successes/failures to score peers for selection priority.
//!
//! # Bans expire (#1819)
//!
//! A peer whose success rate drops below 25% (after [`MIN_SAMPLES`] requests)
//! is banned, but only for a bounded time: [`BAN_BASE_DURATION`] for its first
//! ban, doubling for each further ban of the same peer, capped at
//! [`BAN_MAX_DURATION`]. When a ban expires the peer's success/failure counters
//! are reset, so a single further failure does not immediately re-ban it; the
//! per-peer ban count carries the memory instead and lengthens the next ban.
//!
//! Before #1819 a ban never lifted. Status answers record no success (only
//! block responses do), so a peer this node never downloaded from was banned
//! after three status failures for the life of the process, and a node whose
//! peers were all banned could never download again.
//!
//! # Ban memory fades (#1832)
//!
//! `ban_count` is reset once a peer has gone [`BAN_COUNT_DECAY`] without being
//! banned (measured from the end of its last ban), so a peer that was flaky
//! long ago starts over at [`BAN_BASE_DURATION`] rather than at the cap.
//!
//! # Block responses are credited after they apply (#1832)
//!
//! A block response is not evidence of good service until the batch has been
//! validated and applied. The sync manager therefore splits the old
//! `response_received` into [`ReputationManager::take_latency`] (measured when
//! the response arrives) and [`ReputationManager::record_success_with`]
//! (called once the batch applied). Before #1832 a peer serving only invalid
//! batches scored one success per failure, about 50%, and was never banned.

use libp2p::PeerId;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

/// Weight for exponential moving average (higher = more weight on recent
/// samples)
const EMA_ALPHA: f64 = 0.3;

/// Latency penalty for failed requests (treated as this many ms)
const FAILURE_LATENCY_MS: u64 = 30_000;

/// Minimum requests before reputation is considered reliable
const MIN_SAMPLES: u32 = 3;

/// Success rate below which a peer (with at least [`MIN_SAMPLES`] requests) is
/// banned.
const BAN_SUCCESS_RATE: f64 = 0.25;

/// How long a peer's first ban lasts (#1819).
///
/// Long enough that a peer which really is failing is not retried on every
/// ~2s sync tick or every 30s status refresh (it needs three fresh failures to
/// be banned again in any case), short enough that a peer banned for a
/// transient reason (our own outage, a network blip that timed out several
/// status requests) is usable again within minutes.
pub const BAN_BASE_DURATION: Duration = Duration::from_secs(5 * 60);

/// Upper bound on a single ban's length (#1819).
///
/// Repeated bans of the same peer double the duration
/// (5, 10, 20, 40, 60, 60, ... minutes). The cap keeps a peer that was flaky
/// for a while from being shut out for hours after it recovers.
pub const BAN_MAX_DURATION: Duration = Duration::from_secs(60 * 60);

/// How long a peer must go without a ban before its `ban_count` resets
/// (#1832), measured from the end of its last ban.
///
/// The backoff in [`ban_duration`] is meant for a peer that keeps failing
/// within hours. A day with no ban is long past the longest backoff
/// ([`BAN_MAX_DURATION`]), so a peer flaky that long ago is treated as a
/// first offender again (a [`BAN_BASE_DURATION`] ban) instead of carrying
/// 1-hour bans forever.
pub const BAN_COUNT_DECAY: Duration = Duration::from_secs(24 * 60 * 60);

/// Length of a peer's `ban_number`-th ban (1-based): [`BAN_BASE_DURATION`]
/// doubled for each earlier ban, capped at [`BAN_MAX_DURATION`].
pub fn ban_duration(ban_number: u32) -> Duration {
    let doublings = ban_number.saturating_sub(1).min(16);
    BAN_BASE_DURATION
        .saturating_mul(1u32 << doublings)
        .min(BAN_MAX_DURATION)
}

/// Per-peer reputation data
#[derive(Debug, Clone)]
pub struct PeerReputation {
    /// Exponential moving average of response latency (milliseconds)
    pub avg_latency_ms: f64,
    /// Total successful requests
    pub successes: u32,
    /// Total failed requests
    pub failures: u32,
    /// Last response time
    pub last_response: Option<Instant>,
    /// When this peer was first seen
    pub first_seen: Instant,
    /// End of the peer's current ban, if it has ever been banned (#1819).
    /// The peer is banned while the current time is before this instant.
    /// Tests backdate it to simulate expiry without sleeping.
    pub banned_until: Option<Instant>,
    /// Number of times this peer has been banned (#1819), so a repeat
    /// offender gets a longer ban (see [`ban_duration`]). Reset after
    /// [`BAN_COUNT_DECAY`] without a ban (#1832).
    pub ban_count: u32,
    /// End of the peer's most recent ban, kept after the ban is lifted
    /// (#1832). [`BAN_COUNT_DECAY`] is measured from here. Tests backdate it
    /// to simulate a quiet period without sleeping.
    pub last_ban_end: Option<Instant>,
}

impl PeerReputation {
    /// Create a new reputation entry for a peer
    pub fn new() -> Self {
        Self {
            avg_latency_ms: 0.0,
            successes: 0,
            failures: 0,
            last_response: None,
            first_seen: Instant::now(),
            banned_until: None,
            ban_count: 0,
            last_ban_end: None,
        }
    }

    /// Record a successful response with latency
    pub fn record_success(&mut self, latency: Duration) {
        let now = Instant::now();
        self.expire_ban_at(now);
        self.decay_ban_count_at(now);
        let latency_ms = latency.as_millis() as f64;
        self.update_latency(latency_ms);
        self.successes += 1;
        self.last_response = Some(now);
    }

    /// Record a failed request
    pub fn record_failure(&mut self) {
        self.record_failure_at(Instant::now());
    }

    /// Record a failed request at `now`. Bans the peer (until
    /// `now + ban_duration(..)`) if this failure takes it over the ban
    /// threshold and it is not already banned.
    pub fn record_failure_at(&mut self, now: Instant) {
        self.expire_ban_at(now);
        self.decay_ban_count_at(now);
        self.update_latency(FAILURE_LATENCY_MS as f64);
        self.failures += 1;
        if !self.is_banned_at(now) && self.meets_ban_threshold() {
            self.ban_count = self.ban_count.saturating_add(1);
            let until = now + ban_duration(self.ban_count);
            self.banned_until = Some(until);
            self.last_ban_end = Some(until);
        }
    }

    /// Reset `ban_count` if the peer is not banned and its last ban ended at
    /// least [`BAN_COUNT_DECAY`] before `now` (#1832). Returns `true` if the
    /// count was reset.
    pub fn decay_ban_count_at(&mut self, now: Instant) -> bool {
        if self.ban_count == 0 || self.is_banned_at(now) {
            return false;
        }
        match self.last_ban_end {
            Some(end) if now.saturating_duration_since(end) >= BAN_COUNT_DECAY => {
                self.ban_count = 0;
                self.last_ban_end = None;
                true
            }
            _ => false,
        }
    }

    /// If the peer's ban has expired by `now`, lift it and reset the
    /// success/failure counters and latency average (#1819), so the peer
    /// starts over as a fresh peer and needs [`MIN_SAMPLES`] new requests
    /// before it can be banned again. `ban_count` is kept, so the next ban
    /// is longer. Returns `true` if a ban was lifted.
    pub fn expire_ban_at(&mut self, now: Instant) -> bool {
        match self.banned_until {
            Some(until) if now >= until => {
                self.banned_until = None;
                self.successes = 0;
                self.failures = 0;
                self.avg_latency_ms = 0.0;
                true
            }
            _ => false,
        }
    }

    /// Update latency using exponential moving average
    fn update_latency(&mut self, latency_ms: f64) {
        if self.total_requests() == 0 {
            self.avg_latency_ms = latency_ms;
        } else {
            self.avg_latency_ms = EMA_ALPHA * latency_ms + (1.0 - EMA_ALPHA) * self.avg_latency_ms;
        }
    }

    /// Total requests (success + failure)
    pub fn total_requests(&self) -> u32 {
        self.successes + self.failures
    }

    /// Success rate as a fraction (0.0 to 1.0)
    pub fn success_rate(&self) -> f64 {
        if self.total_requests() == 0 {
            1.0
        } else {
            self.successes as f64 / self.total_requests() as f64
        }
    }

    /// Calculate a score for peer selection (lower is better)
    pub fn score(&self) -> f64 {
        let base = if self.total_requests() < MIN_SAMPLES {
            500.0 // Neutral score for new peers
        } else {
            self.avg_latency_ms
        };

        // Reliability penalty: score * (2 - success_rate)
        let reliability_factor = 2.0 - self.success_rate();
        base * reliability_factor
    }

    /// Whether the counters are past the ban threshold.
    fn meets_ban_threshold(&self) -> bool {
        self.total_requests() >= MIN_SAMPLES && self.success_rate() < BAN_SUCCESS_RATE
    }

    /// Check if this peer should be avoided
    pub fn is_banned(&self) -> bool {
        self.is_banned_at(Instant::now())
    }

    /// Check if this peer is banned at `now`. A ban ends at
    /// [`Self::banned_until`] even before [`Self::expire_ban_at`] has reset
    /// the counters.
    pub fn is_banned_at(&self, now: Instant) -> bool {
        self.banned_until.is_some_and(|until| now < until)
    }
}

impl Default for PeerReputation {
    fn default() -> Self {
        Self::new()
    }
}

/// Manages reputation for all known peers
#[derive(Debug, Default)]
pub struct ReputationManager {
    peers: HashMap<PeerId, PeerReputation>,
    pending_requests: HashMap<PeerId, Instant>,
}

impl ReputationManager {
    /// Create a new reputation manager
    pub fn new() -> Self {
        Self {
            peers: HashMap::new(),
            pending_requests: HashMap::new(),
        }
    }

    /// Get or create reputation entry for a peer
    pub fn get_or_create(&mut self, peer_id: &PeerId) -> &mut PeerReputation {
        self.peers.entry(*peer_id).or_default()
    }

    /// Get reputation for a peer (if exists)
    pub fn get(&self, peer_id: &PeerId) -> Option<&PeerReputation> {
        self.peers.get(peer_id)
    }

    /// Record that a request was sent to a peer
    pub fn request_sent(&mut self, peer_id: PeerId) {
        self.pending_requests.insert(peer_id, Instant::now());
        self.get_or_create(&peer_id);
    }

    /// Record that a response was received from a peer and credit it as a
    /// success right away. For block responses the sync manager instead
    /// measures the latency with [`Self::take_latency`] and credits it with
    /// [`Self::record_success_with`] only once the batch applied (#1832).
    pub fn response_received(&mut self, peer_id: &PeerId) {
        let latency = self.take_latency(peer_id);
        self.record_success_with(peer_id, latency);
    }

    /// Latency of the response that just arrived from `peer_id`: the time
    /// since the last [`Self::request_sent`] to it, or a nominal 100ms when no
    /// request is recorded. Clears the pending request; records no success or
    /// failure (#1832).
    pub fn take_latency(&mut self, peer_id: &PeerId) -> Duration {
        self.pending_requests
            .remove(peer_id)
            .map(|start_time| start_time.elapsed())
            .unwrap_or(Duration::from_millis(100))
    }

    /// Credit `peer_id` with a successful request that took `latency`
    /// (#1832).
    pub fn record_success_with(&mut self, peer_id: &PeerId, latency: Duration) {
        self.get_or_create(peer_id).record_success(latency);
    }

    /// Record that a request to a peer failed
    pub fn request_failed(&mut self, peer_id: &PeerId) {
        self.pending_requests.remove(peer_id);
        self.get_or_create(peer_id).record_failure();
    }

    /// Lift every ban that has expired by `now` and reset those peers'
    /// counters (see [`PeerReputation::expire_ban_at`]). Returns the peers
    /// whose ban was lifted.
    ///
    /// Also resets the `ban_count` of every peer whose last ban ended
    /// [`BAN_COUNT_DECAY`] ago (#1832).
    pub fn expire_bans_at(&mut self, now: Instant) -> Vec<PeerId> {
        self.peers
            .iter_mut()
            .filter_map(|(peer, rep)| {
                let lifted = rep.expire_ban_at(now);
                rep.decay_ban_count_at(now);
                lifted.then_some(*peer)
            })
            .collect()
    }

    /// [`Self::expire_bans_at`] with the current time.
    pub fn expire_bans(&mut self) -> Vec<PeerId> {
        self.expire_bans_at(Instant::now())
    }

    /// Get the best peer from a list (lowest score = best).
    ///
    /// Banned peers are skipped unless every candidate is banned, in which
    /// case bans are ignored and the best-scoring candidate is returned
    /// (#1819): a ban is a preference between peers, and must never leave
    /// the caller with no peer at all.
    pub fn best_peer<'a>(
        &self,
        candidates: impl IntoIterator<Item = &'a PeerId>,
    ) -> Option<PeerId> {
        let candidates: Vec<&PeerId> = candidates.into_iter().collect();
        let all_banned = candidates.iter().all(|p| self.is_banned(p));
        candidates
            .into_iter()
            .filter(|p| all_banned || !self.is_banned(p))
            .min_by(|a, b| {
                let score_a = self.peers.get(a).map(|r| r.score()).unwrap_or(500.0);
                let score_b = self.peers.get(b).map(|r| r.score()).unwrap_or(500.0);
                score_a
                    .partial_cmp(&score_b)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .copied()
    }

    /// Check if a peer is banned
    pub fn is_banned(&self, peer_id: &PeerId) -> bool {
        self.peers
            .get(peer_id)
            .map(|r| r.is_banned())
            .unwrap_or(false)
    }

    /// Get all peer scores for debugging
    pub fn all_scores(&self) -> Vec<(PeerId, f64, u32, u32)> {
        self.peers
            .iter()
            .map(|(id, rep)| (*id, rep.score(), rep.successes, rep.failures))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_peer_reputation() {
        let rep = PeerReputation::new();
        assert_eq!(rep.successes, 0);
        assert_eq!(rep.failures, 0);
        assert_eq!(rep.success_rate(), 1.0);
        assert!(!rep.is_banned());
    }

    #[test]
    fn test_record_success() {
        let mut rep = PeerReputation::new();
        rep.record_success(Duration::from_millis(100));

        assert_eq!(rep.successes, 1);
        assert_eq!(rep.avg_latency_ms, 100.0);
    }

    #[test]
    fn test_ema_latency() {
        let mut rep = PeerReputation::new();
        rep.record_success(Duration::from_millis(100));
        assert_eq!(rep.avg_latency_ms, 100.0);

        rep.record_success(Duration::from_millis(200));
        // EMA: 0.3 * 200 + 0.7 * 100 = 130
        assert!((rep.avg_latency_ms - 130.0).abs() < 0.01);
    }

    #[test]
    fn test_ban_threshold() {
        let mut rep = PeerReputation::new();
        // 0 successes, 4 failures = 0% success rate (< 25%)
        rep.record_failure();
        rep.record_failure();
        rep.record_failure();
        rep.record_failure();

        assert!(rep.is_banned());
    }

    #[test]
    fn test_reputation_manager_best_peer() {
        let mut manager = ReputationManager::new();

        let fast_peer = PeerId::random();
        let slow_peer = PeerId::random();

        // Fast peer: low latency
        for _ in 0..3 {
            manager
                .get_or_create(&fast_peer)
                .record_success(Duration::from_millis(50));
        }

        // Slow peer: high latency
        for _ in 0..3 {
            manager
                .get_or_create(&slow_peer)
                .record_success(Duration::from_millis(500));
        }

        let candidates = vec![fast_peer, slow_peer];
        let best = manager.best_peer(&candidates);

        assert_eq!(best, Some(fast_peer));
    }

    // ========================================================================
    // Additional PeerReputation tests
    // ========================================================================

    #[test]
    fn test_record_failure() {
        let mut rep = PeerReputation::new();
        rep.record_failure();

        assert_eq!(rep.failures, 1);
        assert_eq!(rep.successes, 0);
        assert_eq!(rep.avg_latency_ms, FAILURE_LATENCY_MS as f64);
    }

    #[test]
    fn test_total_requests() {
        let mut rep = PeerReputation::new();
        assert_eq!(rep.total_requests(), 0);

        rep.record_success(Duration::from_millis(100));
        assert_eq!(rep.total_requests(), 1);

        rep.record_failure();
        assert_eq!(rep.total_requests(), 2);
    }

    #[test]
    fn test_success_rate_calculations() {
        let mut rep = PeerReputation::new();

        // 0 requests = 100% success rate (optimistic default)
        assert_eq!(rep.success_rate(), 1.0);

        // 1 success = 100%
        rep.record_success(Duration::from_millis(100));
        assert_eq!(rep.success_rate(), 1.0);

        // 1 success, 1 failure = 50%
        rep.record_failure();
        assert!((rep.success_rate() - 0.5).abs() < 0.001);

        // 1 success, 2 failures = 33.3%
        rep.record_failure();
        assert!((rep.success_rate() - 0.333).abs() < 0.01);
    }

    #[test]
    fn test_score_for_new_peer() {
        let rep = PeerReputation::new();
        // New peers get neutral score (500) before MIN_SAMPLES
        assert_eq!(rep.score(), 500.0);
    }

    #[test]
    fn test_score_after_min_samples() {
        let mut rep = PeerReputation::new();

        // Record MIN_SAMPLES successes with 100ms latency
        for _ in 0..MIN_SAMPLES {
            rep.record_success(Duration::from_millis(100));
        }

        // Score should be based on actual latency now
        // With 100% success rate, reliability_factor = 2.0 - 1.0 = 1.0
        // Score = 100.0 * 1.0 = 100.0 (approximately, due to EMA)
        assert!(rep.score() < 200.0);
        assert!(rep.score() > 50.0);
    }

    #[test]
    fn test_score_penalizes_unreliability() {
        let mut reliable = PeerReputation::new();
        let mut unreliable = PeerReputation::new();

        // Reliable: 3 successes
        for _ in 0..3 {
            reliable.record_success(Duration::from_millis(100));
        }

        // Unreliable: 1 success, 2 failures
        unreliable.record_success(Duration::from_millis(100));
        unreliable.record_failure();
        unreliable.record_failure();

        // Unreliable should have worse (higher) score
        assert!(unreliable.score() > reliable.score());
    }

    #[test]
    fn test_ban_not_applied_before_min_samples() {
        let mut rep = PeerReputation::new();
        // 2 failures is < MIN_SAMPLES
        rep.record_failure();
        rep.record_failure();

        // Should not be banned yet (not enough samples)
        assert!(!rep.is_banned());
    }

    #[test]
    fn test_ban_threshold_exactly_25_percent() {
        let mut rep = PeerReputation::new();
        // 1 success, 3 failures = 25% (at threshold, not banned)
        rep.record_success(Duration::from_millis(100));
        rep.record_failure();
        rep.record_failure();
        rep.record_failure();

        // 25% is not < 25%, so should not be banned
        assert!(!rep.is_banned());
    }

    #[test]
    fn test_last_response_updated() {
        let mut rep = PeerReputation::new();
        assert!(rep.last_response.is_none());

        rep.record_success(Duration::from_millis(100));
        assert!(rep.last_response.is_some());
    }

    #[test]
    fn test_default_trait() {
        let rep = PeerReputation::default();
        assert_eq!(rep.successes, 0);
        assert_eq!(rep.failures, 0);
    }

    // ========================================================================
    // Additional ReputationManager tests
    // ========================================================================

    #[test]
    fn test_manager_get_nonexistent_peer() {
        let manager = ReputationManager::new();
        let peer = PeerId::random();

        assert!(manager.get(&peer).is_none());
    }

    #[test]
    fn test_manager_get_or_create_creates() {
        let mut manager = ReputationManager::new();
        let peer = PeerId::random();

        // Should create new entry
        let rep = manager.get_or_create(&peer);
        assert_eq!(rep.total_requests(), 0);

        // Should return same entry
        rep.record_success(Duration::from_millis(100));
        let rep2 = manager.get(&peer).unwrap();
        assert_eq!(rep2.total_requests(), 1);
    }

    #[test]
    fn test_manager_request_sent_tracking() {
        let mut manager = ReputationManager::new();
        let peer = PeerId::random();

        manager.request_sent(peer);

        // Peer should now exist
        assert!(manager.get(&peer).is_some());
    }

    #[test]
    fn test_manager_response_received_with_pending() {
        let mut manager = ReputationManager::new();
        let peer = PeerId::random();

        manager.request_sent(peer);
        std::thread::sleep(Duration::from_millis(10));
        manager.response_received(&peer);

        let rep = manager.get(&peer).unwrap();
        assert_eq!(rep.successes, 1);
        assert!(rep.avg_latency_ms >= 10.0);
    }

    #[test]
    fn test_manager_response_without_pending() {
        let mut manager = ReputationManager::new();
        let peer = PeerId::random();

        // Response without prior request_sent
        manager.response_received(&peer);

        let rep = manager.get(&peer).unwrap();
        assert_eq!(rep.successes, 1);
        // Should use default 100ms
        assert_eq!(rep.avg_latency_ms, 100.0);
    }

    #[test]
    fn test_manager_request_failed() {
        let mut manager = ReputationManager::new();
        let peer = PeerId::random();

        manager.request_sent(peer);
        manager.request_failed(&peer);

        let rep = manager.get(&peer).unwrap();
        assert_eq!(rep.failures, 1);
    }

    #[test]
    fn test_manager_is_banned() {
        let mut manager = ReputationManager::new();
        let peer = PeerId::random();

        // Not banned: unknown peer
        assert!(!manager.is_banned(&peer));

        // Get 4 failures to trigger ban
        for _ in 0..4 {
            manager.get_or_create(&peer).record_failure();
        }

        assert!(manager.is_banned(&peer));
    }

    #[test]
    fn test_manager_best_peer_excludes_banned() {
        let mut manager = ReputationManager::new();

        let good_peer = PeerId::random();
        let banned_peer = PeerId::random();

        // Good peer: 3 successes
        for _ in 0..3 {
            manager
                .get_or_create(&good_peer)
                .record_success(Duration::from_millis(200));
        }

        // Banned peer: 4 failures (even if it had faster times)
        for _ in 0..4 {
            manager.get_or_create(&banned_peer).record_failure();
        }

        let candidates = vec![good_peer, banned_peer];
        let best = manager.best_peer(&candidates);

        assert_eq!(best, Some(good_peer));
    }

    #[test]
    fn test_manager_best_peer_with_unknown_candidates() {
        let manager = ReputationManager::new();
        let peer1 = PeerId::random();
        let peer2 = PeerId::random();

        // Unknown peers get neutral score (500)
        let candidates = vec![peer1, peer2];
        let best = manager.best_peer(&candidates);

        // Should return one of them (both have same neutral score)
        assert!(best.is_some());
    }

    #[test]
    fn test_manager_best_peer_empty_candidates() {
        let manager = ReputationManager::new();
        let candidates: Vec<PeerId> = vec![];

        assert!(manager.best_peer(&candidates).is_none());
    }

    #[test]
    fn test_manager_best_peer_all_banned() {
        let mut manager = ReputationManager::new();
        let peer1 = PeerId::random();
        let peer2 = PeerId::random();

        // Ban both peers
        for _ in 0..4 {
            manager.get_or_create(&peer1).record_failure();
            manager.get_or_create(&peer2).record_failure();
        }

        // #1819: when every candidate is banned, bans are ignored rather than
        // leaving the caller with no peer at all.
        let candidates = vec![peer1, peer2];
        let best = manager.best_peer(&candidates);
        assert!(best == Some(peer1) || best == Some(peer2), "got {best:?}");
        assert!(manager.best_peer(&Vec::<PeerId>::new()).is_none());
    }

    // ========================================================================
    // #1819: bans expire, with per-peer backoff
    // ========================================================================

    /// Ban `rep` at `now` with failures only; returns the ban's end.
    fn ban_at(rep: &mut PeerReputation, now: Instant) -> Instant {
        for _ in 0..MIN_SAMPLES {
            rep.record_failure_at(now);
        }
        assert!(rep.is_banned_at(now));
        rep.banned_until.expect("banned")
    }

    #[test]
    fn test_ban_duration_doubles_and_caps() {
        assert_eq!(ban_duration(0), BAN_BASE_DURATION, "defensive: as the 1st");
        assert_eq!(ban_duration(1), Duration::from_secs(5 * 60));
        assert_eq!(ban_duration(2), Duration::from_secs(10 * 60));
        assert_eq!(ban_duration(3), Duration::from_secs(20 * 60));
        assert_eq!(ban_duration(4), Duration::from_secs(40 * 60));
        assert_eq!(ban_duration(5), BAN_MAX_DURATION, "80 min capped to 60");
        assert_eq!(ban_duration(6), BAN_MAX_DURATION);
        assert_eq!(ban_duration(u32::MAX), BAN_MAX_DURATION, "no overflow");
        assert!(BAN_BASE_DURATION < BAN_MAX_DURATION);
    }

    #[test]
    fn test_ban_expires_after_base_duration() {
        let t0 = Instant::now();
        let mut rep = PeerReputation::new();
        let until = ban_at(&mut rep, t0);
        assert_eq!(until, t0 + BAN_BASE_DURATION);
        assert_eq!(rep.ban_count, 1);

        let just_before = until - Duration::from_secs(1);
        assert!(rep.is_banned_at(just_before));
        assert!(!rep.expire_ban_at(just_before), "not yet");
        assert!(!rep.is_banned_at(until), "a ban ends at banned_until");
    }

    #[test]
    fn test_expiry_resets_counters_so_one_failure_does_not_reban() {
        let t0 = Instant::now();
        let mut rep = PeerReputation::new();
        let until = ban_at(&mut rep, t0);

        assert!(rep.expire_ban_at(until));
        assert_eq!((rep.successes, rep.failures), (0, 0), "counters reset");
        assert_eq!(rep.banned_until, None);
        assert_eq!(rep.ban_count, 1, "the ban count carries the memory");
        assert_eq!(rep.score(), PeerReputation::new().score(), "fresh score");

        // One (and two) further failures do not re-ban; the third does.
        rep.record_failure_at(until);
        rep.record_failure_at(until);
        assert!(!rep.is_banned_at(until));
        rep.record_failure_at(until);
        assert!(rep.is_banned_at(until));
    }

    #[test]
    fn test_failure_after_expiry_resets_lazily() {
        // Without an explicit expire_ban_at call, the next failure after the
        // ban's end resets the counters first.
        let t0 = Instant::now();
        let mut rep = PeerReputation::new();
        let until = ban_at(&mut rep, t0);
        rep.record_failure_at(until);
        assert_eq!(rep.failures, 1);
        assert!(!rep.is_banned_at(until));
    }

    #[test]
    fn test_repeat_bans_back_off_and_cap() {
        let mut now = Instant::now();
        let mut rep = PeerReputation::new();
        let expected_minutes = [5, 10, 20, 40, 60, 60];
        for (i, minutes) in expected_minutes.into_iter().enumerate() {
            let until = ban_at(&mut rep, now);
            assert_eq!(rep.ban_count, i as u32 + 1);
            assert_eq!(
                until - now,
                Duration::from_secs(minutes * 60),
                "ban #{}",
                i + 1
            );
            now = until;
            assert!(rep.expire_ban_at(now));
        }
    }

    #[test]
    fn test_failures_while_banned_do_not_extend_the_ban() {
        let t0 = Instant::now();
        let mut rep = PeerReputation::new();
        let until = ban_at(&mut rep, t0);
        rep.record_failure_at(t0 + Duration::from_secs(60));
        assert_eq!(rep.banned_until, Some(until));
        assert_eq!(rep.ban_count, 1);
    }

    #[test]
    fn test_manager_expire_bans_reports_lifted_peers() {
        let mut manager = ReputationManager::new();
        let banned = PeerId::random();
        let fine = PeerId::random();
        let t0 = Instant::now();
        let until = ban_at(manager.get_or_create(&banned), t0);
        manager
            .get_or_create(&fine)
            .record_success(Duration::from_millis(50));

        assert!(manager.expire_bans_at(t0).is_empty());
        assert!(manager.is_banned(&banned));
        assert_eq!(manager.expire_bans_at(until), vec![banned]);
        assert!(!manager.is_banned(&banned));
        assert_eq!(manager.get(&banned).unwrap().failures, 0);
        assert_eq!(manager.get(&fine).unwrap().successes, 1, "untouched");
    }

    #[test]
    fn test_manager_best_peer_skips_single_banned_among_good() {
        // The all-banned floor only applies when *every* candidate is banned.
        let mut manager = ReputationManager::new();
        let good = PeerId::random();
        let bad = PeerId::random();
        for _ in 0..4 {
            manager.get_or_create(&bad).record_failure();
        }
        manager
            .get_or_create(&good)
            .record_success(Duration::from_secs(20));
        assert_eq!(manager.best_peer(&[bad, good]), Some(good));
    }

    // ========================================================================
    // #1832: ban_count decays after a quiet period
    // ========================================================================

    #[test]
    fn test_ban_count_resets_after_quiet_period() {
        let t0 = Instant::now();
        let mut rep = PeerReputation::new();
        let mut now = t0;
        // Three bans in a row: the third lasts 20 minutes.
        for _ in 0..3 {
            now = ban_at(&mut rep, now);
            assert!(rep.expire_ban_at(now));
        }
        assert_eq!(rep.ban_count, 3);
        let last_end = rep.last_ban_end.expect("recorded");
        assert_eq!(last_end, now);

        // Just short of a quiet day: the memory is kept and the next ban is
        // the 4th (40 minutes).
        let mut kept = rep.clone();
        let almost = last_end + BAN_COUNT_DECAY - Duration::from_secs(1);
        assert!(!kept.decay_ban_count_at(almost));
        let until = ban_at(&mut kept, almost);
        assert_eq!(kept.ban_count, 4);
        assert_eq!(until - almost, ban_duration(4));

        // A full quiet day: the next ban is a first offence again.
        let later = last_end + BAN_COUNT_DECAY;
        let until = ban_at(&mut rep, later);
        assert_eq!(rep.ban_count, 1, "ban_count reset after the quiet period");
        assert_eq!(until - later, BAN_BASE_DURATION);
    }

    #[test]
    fn test_ban_count_never_decays_while_banned() {
        let t0 = Instant::now();
        let mut rep = PeerReputation::new();
        ban_at(&mut rep, t0);
        // Pretend the ban is (absurdly) still running a day after it ended
        // per `last_ban_end`: an active ban is never forgotten.
        rep.banned_until = Some(t0 + 2 * BAN_COUNT_DECAY);
        assert!(!rep.decay_ban_count_at(t0 + BAN_COUNT_DECAY + BAN_BASE_DURATION));
        assert_eq!(rep.ban_count, 1);
    }

    #[test]
    fn test_manager_expire_bans_decays_ban_counts() {
        let mut manager = ReputationManager::new();
        let peer = PeerId::random();
        let t0 = Instant::now();
        let until = ban_at(manager.get_or_create(&peer), t0);
        assert_eq!(manager.expire_bans_at(until), vec![peer]);
        assert_eq!(manager.get(&peer).unwrap().ban_count, 1);
        assert!(manager.expire_bans_at(until + BAN_COUNT_DECAY).is_empty());
        let rep = manager.get(&peer).unwrap();
        assert_eq!(rep.ban_count, 0);
        assert_eq!(rep.last_ban_end, None);
    }

    #[test]
    fn test_take_latency_records_nothing_until_credited() {
        let mut manager = ReputationManager::new();
        let peer = PeerId::random();
        manager.request_sent(peer);
        let latency = manager.take_latency(&peer);
        let rep = manager.get(&peer).unwrap();
        assert_eq!(rep.total_requests(), 0, "no verdict yet");
        manager.record_success_with(&peer, latency);
        assert_eq!(manager.get(&peer).unwrap().successes, 1);
        // The pending request was consumed: a second take uses the default.
        assert_eq!(manager.take_latency(&peer), Duration::from_millis(100));
    }

    #[test]
    fn test_manager_all_scores() {
        let mut manager = ReputationManager::new();
        let peer1 = PeerId::random();
        let peer2 = PeerId::random();

        manager
            .get_or_create(&peer1)
            .record_success(Duration::from_millis(100));
        manager.get_or_create(&peer2).record_failure();

        let scores = manager.all_scores();
        assert_eq!(scores.len(), 2);

        // Find peer1's entry
        let p1_score = scores.iter().find(|(id, _, _, _)| *id == peer1);
        assert!(p1_score.is_some());
        let (_, _, successes, failures) = p1_score.unwrap();
        assert_eq!(*successes, 1);
        assert_eq!(*failures, 0);

        // Find peer2's entry
        let p2_score = scores.iter().find(|(id, _, _, _)| *id == peer2);
        assert!(p2_score.is_some());
        let (_, _, successes, failures) = p2_score.unwrap();
        assert_eq!(*successes, 0);
        assert_eq!(*failures, 1);
    }

    #[test]
    fn test_manager_default_trait() {
        let manager = ReputationManager::default();
        let peer = PeerId::random();
        assert!(manager.get(&peer).is_none());
    }
}
