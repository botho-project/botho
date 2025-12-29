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
            .or_insert_with(PeerReputation::new)
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
}
