// Copyright (c) 2024 Botho Foundation

//! Peer reputation tracking based on response latency and reliability.
//!
//! Tracks response times using exponential moving average and counts
//! successes/failures to score peers for selection priority.

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
        }
    }

    /// Record a successful response with latency
    pub fn record_success(&mut self, latency: Duration) {
        let latency_ms = latency.as_millis() as f64;
        self.update_latency(latency_ms);
        self.successes += 1;
        self.last_response = Some(Instant::now());
    }

    /// Record a failed request
    pub fn record_failure(&mut self) {
        self.update_latency(FAILURE_LATENCY_MS as f64);
        self.failures += 1;
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

    /// Check if this peer should be avoided
    pub fn is_banned(&self) -> bool {
        self.total_requests() >= MIN_SAMPLES && self.success_rate() < 0.25
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
        self.peers
            .entry(*peer_id)
            .or_default()
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

    /// Record that a response was received from a peer
    pub fn response_received(&mut self, peer_id: &PeerId) {
        if let Some(start_time) = self.pending_requests.remove(peer_id) {
            let latency = start_time.elapsed();
            self.get_or_create(peer_id).record_success(latency);
        } else {
            self.get_or_create(peer_id)
                .record_success(Duration::from_millis(100));
        }
    }

    /// Record that a request to a peer failed
    pub fn request_failed(&mut self, peer_id: &PeerId) {
        self.pending_requests.remove(peer_id);
        self.get_or_create(peer_id).record_failure();
    }

    /// Get the best peer from a list (lowest score = best)
    pub fn best_peer<'a>(
        &self,
        candidates: impl IntoIterator<Item = &'a PeerId>,
    ) -> Option<PeerId> {
        candidates
            .into_iter()
            .filter(|p| !self.is_banned(p))
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

        let candidates = vec![peer1, peer2];
        assert!(manager.best_peer(&candidates).is_none());
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
