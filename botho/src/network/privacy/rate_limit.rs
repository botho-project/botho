// Copyright (c) 2024 Botho Foundation

//! Per-peer rate limiting for relay traffic.
//!
//! This module implements token bucket rate limiting to protect relays from
//! abuse. It enforces separate limits for:
//!
//! - Circuit CREATE requests: 10/min per peer (expensive operations)
//! - Relay messages: 100/sec per peer (forwarding traffic)
//! - Bandwidth: 1 MB/s per peer (total bytes)
//!
//! Peers that repeatedly exceed limits accumulate violations and are
//! disconnected after reaching the threshold.
//!
//! # Security
//!
//! Rate limiting prevents DoS attacks where malicious peers flood relays
//! with messages. The token bucket algorithm allows controlled bursts while
//! enforcing long-term rate limits.
//!
//! # References
//!
//! - Design doc: `docs/design/traffic-privacy-roadmap.md` (Section 1.5)
//! - Parent issue: #157 (Rate limiting for relay traffic)

use libp2p::PeerId;
use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

/// Default circuit CREATE requests allowed per minute.
pub const DEFAULT_CIRCUIT_CREATES_PER_MIN: u32 = 10;

/// Default relay messages allowed per second.
pub const DEFAULT_RELAY_MSGS_PER_SEC: u32 = 100;

/// Default relay bandwidth per peer in bytes/second (1 MB/s).
pub const DEFAULT_RELAY_BANDWIDTH_PER_PEER: u64 = 1_000_000;

/// Default violation threshold before disconnect.
pub const DEFAULT_VIOLATION_THRESHOLD: u32 = 5;

/// Configuration for relay rate limits.
///
/// These limits are applied per-peer to prevent any single peer from
/// abusing the relay.
#[derive(Debug, Clone)]
pub struct RelayRateLimits {
    /// Max circuit CREATE requests per minute per peer.
    pub circuit_creates_per_min: u32,

    /// Max relay messages per second per peer.
    pub relay_msgs_per_sec: u32,

    /// Max total relay bandwidth per peer (bytes/sec).
    pub relay_bandwidth_per_peer: u64,

    /// Violation threshold before disconnect.
    pub violation_threshold: u32,

    /// Whether rate limiting is enabled.
    pub enabled: bool,
}

impl Default for RelayRateLimits {
    fn default() -> Self {
        Self {
            circuit_creates_per_min: DEFAULT_CIRCUIT_CREATES_PER_MIN,
            relay_msgs_per_sec: DEFAULT_RELAY_MSGS_PER_SEC,
            relay_bandwidth_per_peer: DEFAULT_RELAY_BANDWIDTH_PER_PEER,
            violation_threshold: DEFAULT_VIOLATION_THRESHOLD,
            enabled: true,
        }
    }
}

/// Token bucket for rate limiting.
///
/// Implements a classic token bucket algorithm that allows controlled
/// bursts while enforcing a long-term rate limit. Tokens are refilled
/// at a constant rate up to the bucket capacity.
///
/// # Example
///
/// ```
/// use botho::network::privacy::rate_limit::TokenBucket;
///
/// // 10 tokens/sec, burst capacity of 20
/// let mut bucket = TokenBucket::new(20.0, 10.0);
///
/// // Consume tokens
/// assert!(bucket.try_consume(5));
/// assert!(bucket.try_consume(5));
/// ```
#[derive(Debug, Clone)]
pub struct TokenBucket {
    /// Current token count.
    tokens: f64,

    /// Maximum token capacity (burst limit).
    capacity: f64,

    /// Tokens added per second.
    refill_rate: f64,

    /// Last time tokens were refilled.
    last_refill: Instant,
}

impl TokenBucket {
    /// Create a new token bucket.
    ///
    /// # Arguments
    ///
    /// * `capacity` - Maximum tokens (burst limit)
    /// * `refill_rate` - Tokens added per second
    pub fn new(capacity: f64, refill_rate: f64) -> Self {
        Self {
            tokens: capacity, // Start full
            capacity,
            refill_rate,
            last_refill: Instant::now(),
        }
    }

    /// Try to consume tokens from the bucket.
    ///
    /// Refills tokens based on elapsed time, then attempts to consume
    /// the requested amount.
    ///
    /// # Returns
    ///
    /// `true` if tokens were consumed, `false` if insufficient tokens.
    pub fn try_consume(&mut self, amount: u32) -> bool {
        self.refill();

        let amount_f64 = amount as f64;
        if self.tokens >= amount_f64 {
            self.tokens -= amount_f64;
            true
        } else {
            false
        }
    }

    /// Refill tokens based on elapsed time.
    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_rate).min(self.capacity);
        self.last_refill = now;
    }

    /// Get the current token count.
    pub fn available_tokens(&self) -> f64 {
        self.tokens
    }

    /// Get the bucket capacity.
    pub fn capacity(&self) -> f64 {
        self.capacity
    }

    /// Get the refill rate (tokens per second).
    pub fn refill_rate(&self) -> f64 {
        self.refill_rate
    }
}

/// Bandwidth tracker using a sliding window.
///
/// Tracks bytes consumed within a time window to enforce bandwidth limits.
#[derive(Debug, Clone)]
pub struct BandwidthTracker {
    /// Timestamped byte counts.
    samples: Vec<(Instant, u64)>,

    /// Maximum bytes per second.
    max_bytes_per_sec: u64,

    /// Tracking window duration.
    window: Duration,
}

impl BandwidthTracker {
    /// Create a new bandwidth tracker.
    ///
    /// # Arguments
    ///
    /// * `max_bytes_per_sec` - Maximum allowed bandwidth
    pub fn new(max_bytes_per_sec: u64) -> Self {
        Self {
            samples: Vec::new(),
            max_bytes_per_sec,
            window: Duration::from_secs(1),
        }
    }

    /// Try to consume bandwidth.
    ///
    /// # Arguments
    ///
    /// * `bytes` - Number of bytes to consume
    ///
    /// # Returns
    ///
    /// `true` if bandwidth is available, `false` if over limit.
    pub fn try_consume(&mut self, bytes: usize) -> bool {
        let now = Instant::now();
        let window_start = now - self.window;

        // Clean up old samples
        self.samples.retain(|(t, _)| *t > window_start);

        // Calculate current bandwidth usage
        let current_usage: u64 = self.samples.iter().map(|(_, b)| b).sum();

        if current_usage + bytes as u64 > self.max_bytes_per_sec {
            return false;
        }

        // Record this consumption
        self.samples.push((now, bytes as u64));
        true
    }

    /// Get current bandwidth usage in bytes per second.
    pub fn current_usage(&self) -> u64 {
        let now = Instant::now();
        let window_start = now - self.window;
        self.samples
            .iter()
            .filter(|(t, _)| *t > window_start)
            .map(|(_, b)| b)
            .sum()
    }

    /// Reset the tracker.
    pub fn reset(&mut self) {
        self.samples.clear();
    }
}

/// Per-peer rate limiter combining token buckets and bandwidth tracking.
///
/// This struct tracks rate limiting state for a single peer, using:
/// - Token bucket for circuit CREATE requests
/// - Token bucket for relay messages
/// - Bandwidth tracker for total bytes
/// - Violation counter for disconnect decisions
#[derive(Debug)]
pub struct PeerRelayLimiter {
    /// Token bucket for circuit CREATE requests.
    circuit_bucket: TokenBucket,

    /// Token bucket for relay messages.
    relay_bucket: TokenBucket,

    /// Bandwidth tracker.
    bandwidth_tracker: BandwidthTracker,

    /// Violation count.
    violations: u32,

    /// Last violation time for potential cooldown.
    last_violation: Option<Instant>,
}

impl PeerRelayLimiter {
    /// Create a new peer relay limiter with the given limits.
    pub fn new(limits: &RelayRateLimits) -> Self {
        // Circuit creates: X per minute = X/60 per second
        // Burst capacity = 2x the per-minute limit for reasonable bursts
        let circuit_refill_rate = limits.circuit_creates_per_min as f64 / 60.0;
        let circuit_capacity = (limits.circuit_creates_per_min * 2) as f64;

        // Relay messages: X per second
        // Burst capacity = 2x the per-second limit
        let relay_capacity = (limits.relay_msgs_per_sec * 2) as f64;
        let relay_refill_rate = limits.relay_msgs_per_sec as f64;

        Self {
            circuit_bucket: TokenBucket::new(circuit_capacity, circuit_refill_rate),
            relay_bucket: TokenBucket::new(relay_capacity, relay_refill_rate),
            bandwidth_tracker: BandwidthTracker::new(limits.relay_bandwidth_per_peer),
            violations: 0,
            last_violation: None,
        }
    }

    /// Check if a circuit CREATE request is allowed.
    ///
    /// # Returns
    ///
    /// `true` if allowed, `false` if rate limited (violation recorded).
    pub fn check_circuit_create(&mut self) -> bool {
        if self.circuit_bucket.try_consume(1) {
            true
        } else {
            self.record_violation();
            false
        }
    }

    /// Check if a relay message is allowed.
    ///
    /// Checks both message rate and bandwidth limits.
    ///
    /// # Arguments
    ///
    /// * `size` - Size of the relay message in bytes
    ///
    /// # Returns
    ///
    /// `true` if allowed, `false` if rate limited (violation recorded).
    pub fn check_relay(&mut self, size: usize) -> bool {
        let msg_ok = self.relay_bucket.try_consume(1);
        let bw_ok = self.bandwidth_tracker.try_consume(size);

        if msg_ok && bw_ok {
            true
        } else {
            self.record_violation();
            false
        }
    }

    /// Record a rate limit violation.
    fn record_violation(&mut self) {
        self.violations = self.violations.saturating_add(1);
        self.last_violation = Some(Instant::now());
    }

    /// Check if the peer should be disconnected.
    pub fn should_disconnect(&self, threshold: u32) -> bool {
        self.violations >= threshold
    }

    /// Get the current violation count.
    pub fn violations(&self) -> u32 {
        self.violations
    }

    /// Reset violations (e.g., after cooldown period).
    pub fn reset_violations(&mut self) {
        self.violations = 0;
        self.last_violation = None;
    }

    /// Get time since last violation.
    pub fn time_since_violation(&self) -> Option<Duration> {
        self.last_violation.map(|t| t.elapsed())
    }

    /// Get current relay message rate info.
    pub fn relay_bucket_info(&self) -> (f64, f64) {
        (
            self.relay_bucket.available_tokens(),
            self.relay_bucket.capacity(),
        )
    }

    /// Get current circuit create rate info.
    pub fn circuit_bucket_info(&self) -> (f64, f64) {
        (
            self.circuit_bucket.available_tokens(),
            self.circuit_bucket.capacity(),
        )
    }

    /// Get current bandwidth usage.
    pub fn bandwidth_usage(&self) -> u64 {
        self.bandwidth_tracker.current_usage()
    }
}

/// Manages rate limiting for all peers.
///
/// This is the main entry point for relay rate limiting. It maintains
/// per-peer limiters and provides methods for checking limits and
/// getting statistics.
#[derive(Debug)]
pub struct RelayRateLimiter {
    /// Configuration.
    limits: RelayRateLimits,

    /// Per-peer limiters.
    peers: HashMap<PeerId, PeerRelayLimiter>,

    /// Peers flagged for disconnection.
    flagged_peers: Vec<PeerId>,

    /// Metrics.
    metrics: RelayRateLimitMetrics,
}

impl RelayRateLimiter {
    /// Create a new relay rate limiter.
    pub fn new(limits: RelayRateLimits) -> Self {
        Self {
            limits,
            peers: HashMap::new(),
            flagged_peers: Vec::new(),
            metrics: RelayRateLimitMetrics::new(),
        }
    }

    /// Check if rate limiting is enabled.
    pub fn is_enabled(&self) -> bool {
        self.limits.enabled
    }

    /// Get the rate limit configuration.
    pub fn limits(&self) -> &RelayRateLimits {
        &self.limits
    }

    /// Check if a circuit CREATE request from a peer is allowed.
    ///
    /// # Returns
    ///
    /// `RateLimitResult` indicating whether the request is allowed.
    pub fn check_circuit_create(&mut self, peer: &PeerId) -> RateLimitResult {
        self.metrics.inc_circuit_creates();

        if !self.limits.enabled {
            return RateLimitResult::Allowed;
        }

        // Get or create the limiter and check in one scope
        let limiter = self
            .peers
            .entry(*peer)
            .or_insert_with(|| PeerRelayLimiter::new(&self.limits));

        if limiter.check_circuit_create() {
            RateLimitResult::Allowed
        } else {
            let violations = limiter.violations();
            let should_disconnect = limiter.should_disconnect(self.limits.violation_threshold);
            let remaining = self.limits.violation_threshold.saturating_sub(violations);

            self.metrics.inc_circuit_rate_limited();

            if should_disconnect {
                self.flagged_peers.push(*peer);
                self.metrics.inc_disconnects();
                RateLimitResult::Disconnect
            } else {
                RateLimitResult::RateLimited {
                    violations,
                    remaining,
                }
            }
        }
    }

    /// Check if a relay message from a peer is allowed.
    ///
    /// # Arguments
    ///
    /// * `peer` - The sending peer
    /// * `size` - Size of the message in bytes
    ///
    /// # Returns
    ///
    /// `RateLimitResult` indicating whether the message is allowed.
    pub fn check_relay(&mut self, peer: &PeerId, size: usize) -> RateLimitResult {
        self.metrics.inc_relay_messages();

        if !self.limits.enabled {
            return RateLimitResult::Allowed;
        }

        // Get or create the limiter and check in one scope
        let limiter = self
            .peers
            .entry(*peer)
            .or_insert_with(|| PeerRelayLimiter::new(&self.limits));

        if limiter.check_relay(size) {
            RateLimitResult::Allowed
        } else {
            let violations = limiter.violations();
            let should_disconnect = limiter.should_disconnect(self.limits.violation_threshold);
            let remaining = self.limits.violation_threshold.saturating_sub(violations);

            self.metrics.inc_relay_rate_limited();

            if should_disconnect {
                self.flagged_peers.push(*peer);
                self.metrics.inc_disconnects();
                RateLimitResult::Disconnect
            } else {
                RateLimitResult::RateLimited {
                    violations,
                    remaining,
                }
            }
        }
    }

    /// Take the list of peers flagged for disconnection.
    pub fn take_flagged_peers(&mut self) -> Vec<PeerId> {
        std::mem::take(&mut self.flagged_peers)
    }

    /// Remove a peer from tracking (e.g., when disconnected).
    pub fn remove_peer(&mut self, peer: &PeerId) {
        self.peers.remove(peer);
    }

    /// Get statistics for a peer.
    pub fn get_peer_stats(&self, peer: &PeerId) -> Option<PeerRateLimitStats> {
        self.peers.get(peer).map(|limiter| {
            let (relay_tokens, relay_capacity) = limiter.relay_bucket_info();
            let (circuit_tokens, circuit_capacity) = limiter.circuit_bucket_info();
            PeerRateLimitStats {
                violations: limiter.violations(),
                should_disconnect: limiter.should_disconnect(self.limits.violation_threshold),
                relay_tokens_available: relay_tokens,
                relay_tokens_capacity: relay_capacity,
                circuit_tokens_available: circuit_tokens,
                circuit_tokens_capacity: circuit_capacity,
                bandwidth_usage: limiter.bandwidth_usage(),
            }
        })
    }

    /// Clean up stale peer entries.
    ///
    /// Removes peers with no violations and full token buckets.
    pub fn cleanup_stale_peers(&mut self) {
        // Keep peers that have violations or have used tokens recently
        self.peers.retain(|_, limiter| {
            limiter.violations() > 0
                || limiter.relay_bucket_info().0 < limiter.relay_bucket_info().1
                || limiter.circuit_bucket_info().0 < limiter.circuit_bucket_info().1
        });
    }

    /// Get the number of tracked peers.
    pub fn tracked_peer_count(&self) -> usize {
        self.peers.len()
    }

    /// Get rate limiting metrics.
    pub fn metrics(&self) -> &RelayRateLimitMetrics {
        &self.metrics
    }

    /// Get a snapshot of metrics.
    pub fn metrics_snapshot(&self) -> RelayRateLimitMetricsSnapshot {
        self.metrics.snapshot()
    }
}

/// Result of a rate limit check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RateLimitResult {
    /// Request is allowed.
    Allowed,
    /// Request is rate limited.
    RateLimited {
        /// Current violation count.
        violations: u32,
        /// Remaining violations before disconnect.
        remaining: u32,
    },
    /// Peer should be disconnected.
    Disconnect,
}

impl RateLimitResult {
    /// Check if the request is allowed.
    pub fn is_allowed(&self) -> bool {
        matches!(self, RateLimitResult::Allowed)
    }

    /// Check if the peer should be disconnected.
    pub fn should_disconnect(&self) -> bool {
        matches!(self, RateLimitResult::Disconnect)
    }
}

/// Statistics for a peer's rate limiting state.
#[derive(Debug, Clone)]
pub struct PeerRateLimitStats {
    /// Number of violations.
    pub violations: u32,
    /// Whether the peer should be disconnected.
    pub should_disconnect: bool,
    /// Available relay tokens.
    pub relay_tokens_available: f64,
    /// Total relay token capacity.
    pub relay_tokens_capacity: f64,
    /// Available circuit create tokens.
    pub circuit_tokens_available: f64,
    /// Total circuit create token capacity.
    pub circuit_tokens_capacity: f64,
    /// Current bandwidth usage (bytes/sec).
    pub bandwidth_usage: u64,
}

/// Metrics for relay rate limiting.
#[derive(Debug, Default)]
pub struct RelayRateLimitMetrics {
    /// Total circuit CREATE requests.
    circuit_creates_total: AtomicU64,
    /// Circuit CREATEs that were rate limited.
    circuit_rate_limited: AtomicU64,
    /// Total relay messages.
    relay_messages_total: AtomicU64,
    /// Relay messages that were rate limited.
    relay_rate_limited: AtomicU64,
    /// Peers disconnected for rate limit violations.
    disconnects: AtomicU64,
}

impl RelayRateLimitMetrics {
    /// Create new metrics.
    pub fn new() -> Self {
        Self::default()
    }

    /// Increment circuit creates counter.
    pub fn inc_circuit_creates(&self) {
        self.circuit_creates_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment circuit rate limited counter.
    pub fn inc_circuit_rate_limited(&self) {
        self.circuit_rate_limited.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment relay messages counter.
    pub fn inc_relay_messages(&self) {
        self.relay_messages_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment relay rate limited counter.
    pub fn inc_relay_rate_limited(&self) {
        self.relay_rate_limited.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment disconnects counter.
    pub fn inc_disconnects(&self) {
        self.disconnects.fetch_add(1, Ordering::Relaxed);
    }

    /// Get a snapshot of metrics.
    pub fn snapshot(&self) -> RelayRateLimitMetricsSnapshot {
        RelayRateLimitMetricsSnapshot {
            circuit_creates_total: self.circuit_creates_total.load(Ordering::Relaxed),
            circuit_rate_limited: self.circuit_rate_limited.load(Ordering::Relaxed),
            relay_messages_total: self.relay_messages_total.load(Ordering::Relaxed),
            relay_rate_limited: self.relay_rate_limited.load(Ordering::Relaxed),
            disconnects: self.disconnects.load(Ordering::Relaxed),
        }
    }
}

/// Snapshot of rate limiting metrics.
#[derive(Debug, Clone, Default)]
pub struct RelayRateLimitMetricsSnapshot {
    /// Total circuit CREATE requests.
    pub circuit_creates_total: u64,
    /// Circuit CREATEs that were rate limited.
    pub circuit_rate_limited: u64,
    /// Total relay messages.
    pub relay_messages_total: u64,
    /// Relay messages that were rate limited.
    pub relay_rate_limited: u64,
    /// Peers disconnected for rate limit violations.
    pub disconnects: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // TokenBucket Tests
    // ========================================================================

    #[test]
    fn test_token_bucket_initial_capacity() {
        let bucket = TokenBucket::new(10.0, 1.0);
        assert_eq!(bucket.available_tokens(), 10.0);
        assert_eq!(bucket.capacity(), 10.0);
    }

    #[test]
    fn test_token_bucket_consume() {
        let mut bucket = TokenBucket::new(10.0, 1.0);

        assert!(bucket.try_consume(5));
        // Due to timing, bucket may have refilled slightly, so check range
        assert!(bucket.available_tokens() <= 5.1);
        assert!(bucket.available_tokens() >= 4.9);

        assert!(bucket.try_consume(5));
        // After consuming 5 more, should be near 0 (possibly slightly above due to
        // refill)
        assert!(bucket.available_tokens() < 0.5);

        // Should fail - no tokens left (may have refilled a tiny amount)
        assert!(!bucket.try_consume(1));
    }

    #[test]
    fn test_token_bucket_refill() {
        let mut bucket = TokenBucket::new(10.0, 100.0); // 100 tokens/sec

        // Consume all tokens
        assert!(bucket.try_consume(10));
        assert_eq!(bucket.available_tokens(), 0.0);

        // Wait a bit for refill
        std::thread::sleep(Duration::from_millis(50));

        // Should have some tokens now (approximately 5 at 100/sec after 50ms)
        bucket.refill();
        assert!(bucket.available_tokens() > 0.0);
    }

    #[test]
    fn test_token_bucket_capacity_limit() {
        let mut bucket = TokenBucket::new(10.0, 1000.0); // High refill rate

        // Wait to accumulate tokens
        std::thread::sleep(Duration::from_millis(50));

        // Should not exceed capacity
        bucket.refill();
        assert!(bucket.available_tokens() <= bucket.capacity());
    }

    // ========================================================================
    // BandwidthTracker Tests
    // ========================================================================

    #[test]
    fn test_bandwidth_tracker_allows_under_limit() {
        let mut tracker = BandwidthTracker::new(1000);

        assert!(tracker.try_consume(500));
        assert!(tracker.try_consume(400));
        assert_eq!(tracker.current_usage(), 900);
    }

    #[test]
    fn test_bandwidth_tracker_blocks_over_limit() {
        let mut tracker = BandwidthTracker::new(1000);

        assert!(tracker.try_consume(800));
        assert!(!tracker.try_consume(300)); // Would exceed limit
        assert_eq!(tracker.current_usage(), 800);
    }

    #[test]
    fn test_bandwidth_tracker_reset() {
        let mut tracker = BandwidthTracker::new(1000);

        assert!(tracker.try_consume(500));
        assert_eq!(tracker.current_usage(), 500);

        tracker.reset();
        assert_eq!(tracker.current_usage(), 0);
    }

    // ========================================================================
    // PeerRelayLimiter Tests
    // ========================================================================

    #[test]
    fn test_peer_limiter_circuit_create() {
        let limits = RelayRateLimits {
            circuit_creates_per_min: 2,
            ..Default::default()
        };
        let mut limiter = PeerRelayLimiter::new(&limits);

        // Bucket capacity is 2x the limit, so 4 tokens
        assert!(limiter.check_circuit_create());
        assert!(limiter.check_circuit_create());
        assert!(limiter.check_circuit_create());
        assert!(limiter.check_circuit_create());

        // 5th should fail
        assert!(!limiter.check_circuit_create());
        assert_eq!(limiter.violations(), 1);
    }

    #[test]
    fn test_peer_limiter_relay_message() {
        let limits = RelayRateLimits {
            relay_msgs_per_sec: 2,
            relay_bandwidth_per_peer: 1000,
            ..Default::default()
        };
        let mut limiter = PeerRelayLimiter::new(&limits);

        // Bucket capacity is 2x = 4 tokens
        assert!(limiter.check_relay(100));
        assert!(limiter.check_relay(100));
        assert!(limiter.check_relay(100));
        assert!(limiter.check_relay(100));

        // 5th should fail (out of tokens)
        assert!(!limiter.check_relay(100));
        assert_eq!(limiter.violations(), 1);
    }

    #[test]
    fn test_peer_limiter_bandwidth_limit() {
        let limits = RelayRateLimits {
            relay_msgs_per_sec: 100,
            relay_bandwidth_per_peer: 500,
            ..Default::default()
        };
        let mut limiter = PeerRelayLimiter::new(&limits);

        // Should allow up to bandwidth limit
        assert!(limiter.check_relay(300));
        assert!(limiter.check_relay(100));

        // Should fail - would exceed bandwidth
        assert!(!limiter.check_relay(200));
        assert_eq!(limiter.violations(), 1);
    }

    #[test]
    fn test_peer_limiter_violation_tracking() {
        let limits = RelayRateLimits {
            circuit_creates_per_min: 1,
            violation_threshold: 3,
            ..Default::default()
        };
        let mut limiter = PeerRelayLimiter::new(&limits);

        // Use up tokens (capacity is 2)
        limiter.check_circuit_create();
        limiter.check_circuit_create();

        // Trigger violations
        limiter.check_circuit_create(); // violation 1
        limiter.check_circuit_create(); // violation 2

        assert_eq!(limiter.violations(), 2);
        assert!(!limiter.should_disconnect(3));

        limiter.check_circuit_create(); // violation 3
        assert!(limiter.should_disconnect(3));
    }

    #[test]
    fn test_peer_limiter_reset_violations() {
        let limits = RelayRateLimits {
            circuit_creates_per_min: 1,
            ..Default::default()
        };
        let mut limiter = PeerRelayLimiter::new(&limits);

        // Use up tokens and trigger violation
        limiter.check_circuit_create();
        limiter.check_circuit_create();
        limiter.check_circuit_create();
        assert!(limiter.violations() > 0);

        limiter.reset_violations();
        assert_eq!(limiter.violations(), 0);
    }

    // ========================================================================
    // RelayRateLimiter Tests
    // ========================================================================

    #[test]
    fn test_rate_limiter_allowed() {
        let limits = RelayRateLimits::default();
        let mut limiter = RelayRateLimiter::new(limits);
        let peer = PeerId::random();

        let result = limiter.check_relay(&peer, 100);
        assert!(result.is_allowed());
    }

    #[test]
    fn test_rate_limiter_disabled() {
        let limits = RelayRateLimits {
            enabled: false,
            circuit_creates_per_min: 0,
            relay_msgs_per_sec: 0,
            ..Default::default()
        };
        let mut limiter = RelayRateLimiter::new(limits);
        let peer = PeerId::random();

        // Even with zero limits, should allow when disabled
        let result = limiter.check_relay(&peer, 1_000_000);
        assert!(result.is_allowed());
    }

    #[test]
    fn test_rate_limiter_disconnect() {
        let limits = RelayRateLimits {
            circuit_creates_per_min: 1,
            violation_threshold: 2,
            ..Default::default()
        };
        let mut limiter = RelayRateLimiter::new(limits);
        let peer = PeerId::random();

        // Use up tokens (capacity is 2)
        limiter.check_circuit_create(&peer);
        limiter.check_circuit_create(&peer);

        // Trigger violations until disconnect
        limiter.check_circuit_create(&peer); // violation 1
        let result = limiter.check_circuit_create(&peer); // violation 2

        assert!(result.should_disconnect());

        let flagged = limiter.take_flagged_peers();
        assert_eq!(flagged.len(), 1);
        assert_eq!(flagged[0], peer);
    }

    #[test]
    fn test_rate_limiter_multiple_peers() {
        let limits = RelayRateLimits {
            relay_msgs_per_sec: 2,
            relay_bandwidth_per_peer: 10000,
            ..Default::default()
        };
        let mut limiter = RelayRateLimiter::new(limits);

        let peer1 = PeerId::random();
        let peer2 = PeerId::random();

        // Each peer gets their own bucket
        assert!(limiter.check_relay(&peer1, 100).is_allowed());
        assert!(limiter.check_relay(&peer2, 100).is_allowed());

        // Use up peer1's tokens
        limiter.check_relay(&peer1, 100);
        limiter.check_relay(&peer1, 100);
        limiter.check_relay(&peer1, 100);

        // peer1 should be rate limited
        let result = limiter.check_relay(&peer1, 100);
        assert!(!result.is_allowed());

        // peer2 should still be fine
        assert!(limiter.check_relay(&peer2, 100).is_allowed());
    }

    #[test]
    fn test_rate_limiter_remove_peer() {
        let limits = RelayRateLimits::default();
        let mut limiter = RelayRateLimiter::new(limits);
        let peer = PeerId::random();

        limiter.check_relay(&peer, 100);
        assert!(limiter.get_peer_stats(&peer).is_some());

        limiter.remove_peer(&peer);
        assert!(limiter.get_peer_stats(&peer).is_none());
    }

    #[test]
    fn test_rate_limiter_metrics() {
        let limits = RelayRateLimits {
            relay_msgs_per_sec: 1,
            relay_bandwidth_per_peer: 10000,
            ..Default::default()
        };
        let mut limiter = RelayRateLimiter::new(limits);
        let peer = PeerId::random();

        // Record some activity
        limiter.check_relay(&peer, 100);
        limiter.check_relay(&peer, 100);
        // 3rd should be rate limited (capacity is 2)
        limiter.check_relay(&peer, 100);

        let snapshot = limiter.metrics_snapshot();
        assert_eq!(snapshot.relay_messages_total, 3);
        assert!(snapshot.relay_rate_limited >= 1);
    }

    #[test]
    fn test_rate_limiter_peer_stats() {
        let limits = RelayRateLimits::default();
        let mut limiter = RelayRateLimiter::new(limits);
        let peer = PeerId::random();

        limiter.check_relay(&peer, 100);

        let stats = limiter.get_peer_stats(&peer).unwrap();
        assert_eq!(stats.violations, 0);
        assert!(!stats.should_disconnect);
        assert!(stats.relay_tokens_available < stats.relay_tokens_capacity);
    }

    #[test]
    fn test_rate_limit_result_methods() {
        let allowed = RateLimitResult::Allowed;
        assert!(allowed.is_allowed());
        assert!(!allowed.should_disconnect());

        let limited = RateLimitResult::RateLimited {
            violations: 1,
            remaining: 4,
        };
        assert!(!limited.is_allowed());
        assert!(!limited.should_disconnect());

        let disconnect = RateLimitResult::Disconnect;
        assert!(!disconnect.is_allowed());
        assert!(disconnect.should_disconnect());
    }
}
