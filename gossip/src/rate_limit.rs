// Copyright (c) 2024 Botho Foundation

//! Per-peer rate limiting for gossipsub messages.
//!
//! This module implements sliding window rate limiting to protect against
//! message flooding attacks from individual peers. It tracks message rates
//! per message type to enforce different limits for:
//!
//! - Transaction announcements: 100/min (frequent, lightweight)
//! - Block announcements: 10/min (infrequent, important)
//! - Consensus messages: 50/min (critical but bounded)
//! - Node announcements: 20/min (periodic discovery)
//!
//! # Security
//!
//! Rate limiting prevents DoS attacks where malicious peers flood the network
//! with messages. Peers that repeatedly exceed limits are flagged for
//! disconnection.

use crate::config::PeerRateLimitConfig;
use libp2p::PeerId;
use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

/// Types of gossipsub messages for rate limiting purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GossipMessageType {
    /// Transaction broadcasts
    Transaction,
    /// Block broadcasts
    Block,
    /// Consensus (SCP) messages
    Consensus,
    /// Node announcements for discovery
    Announcement,
    /// Peer exchange messages
    PeerExchange,
    /// Unknown/other message types
    Other,
}

impl GossipMessageType {
    /// Get the rate limit (messages per minute) for this message type.
    pub fn rate_limit(&self, config: &PeerRateLimitConfig) -> u32 {
        match self {
            GossipMessageType::Transaction => config.message_limits.transactions_per_minute,
            GossipMessageType::Block => config.message_limits.blocks_per_minute,
            GossipMessageType::Consensus => config.message_limits.consensus_per_minute,
            GossipMessageType::Announcement => config.message_limits.announcements_per_minute,
            GossipMessageType::PeerExchange => config.message_limits.announcements_per_minute,
            GossipMessageType::Other => config.max_messages_per_second * 60, // Use global limit
        }
    }
}

/// Tracks rate limiting state for a single peer.
#[derive(Debug, Clone)]
pub struct PeerRateState {
    /// Timestamps of recent messages by type (within 1-minute window)
    message_times_by_type: HashMap<GossipMessageType, Vec<Instant>>,
    /// Global message times for overall rate limiting
    global_message_times: Vec<Instant>,
    /// Number of rate limit violations
    violations: u32,
    /// Last time the peer was warned
    last_warning: Option<Instant>,
}

impl Default for PeerRateState {
    fn default() -> Self {
        Self::new()
    }
}

impl PeerRateState {
    /// Create a new rate state.
    pub fn new() -> Self {
        Self {
            message_times_by_type: HashMap::new(),
            global_message_times: Vec::with_capacity(100),
            violations: 0,
            last_warning: None,
        }
    }

    /// Get the number of violations.
    pub fn violations(&self) -> u32 {
        self.violations
    }

    /// Record a violation.
    pub fn record_violation(&mut self) {
        self.violations = self.violations.saturating_add(1);
        self.last_warning = Some(Instant::now());
    }

    /// Reset violations (e.g., after a cooldown period of good behavior).
    pub fn reset_violations(&mut self) {
        self.violations = 0;
    }

    /// Record a message and check if it exceeds rate limits.
    /// Returns true if the message should be allowed, false if rate limited.
    pub fn record_message(&mut self, config: &PeerRateLimitConfig) -> bool {
        self.record_message_typed(config, GossipMessageType::Other)
    }

    /// Record a typed message and check if it exceeds rate limits.
    /// Returns true if the message should be allowed, false if rate limited.
    pub fn record_message_typed(
        &mut self,
        config: &PeerRateLimitConfig,
        msg_type: GossipMessageType,
    ) -> bool {
        let now = Instant::now();
        let one_minute = Duration::from_secs(60);
        let burst_window = Duration::from_millis(config.burst_window_ms);

        // Clean up old global messages
        self.global_message_times
            .retain(|t| now.duration_since(*t) < burst_window);

        // Check global burst limit
        if self.global_message_times.len() >= config.burst_limit as usize {
            self.record_violation();
            return false;
        }

        // Check global per-second limit
        let one_second_ago = now - Duration::from_secs(1);
        let recent_global_count = self
            .global_message_times
            .iter()
            .filter(|t| **t > one_second_ago)
            .count();

        if recent_global_count >= config.max_messages_per_second as usize {
            self.record_violation();
            return false;
        }

        // Check per-message-type limit (messages per minute)
        let type_times = self.message_times_by_type.entry(msg_type).or_default();
        type_times.retain(|t| now.duration_since(*t) < one_minute);

        let type_limit = msg_type.rate_limit(config) as usize;
        if type_times.len() >= type_limit {
            self.record_violation();
            return false;
        }

        // Message is allowed - record it
        type_times.push(now);
        self.global_message_times.push(now);
        true
    }

    /// Check if this peer should be disconnected.
    pub fn should_disconnect(&self, threshold: u32) -> bool {
        self.violations >= threshold
    }

    /// Get message count in the current window.
    pub fn current_message_count(&self, window: Duration) -> usize {
        let now = Instant::now();
        self.global_message_times
            .iter()
            .filter(|t| now.duration_since(**t) < window)
            .count()
    }

    /// Get message count by type in the last minute.
    pub fn message_count_by_type(&self, msg_type: GossipMessageType) -> usize {
        let now = Instant::now();
        let one_minute = Duration::from_secs(60);
        self.message_times_by_type
            .get(&msg_type)
            .map(|times| {
                times
                    .iter()
                    .filter(|t| now.duration_since(**t) < one_minute)
                    .count()
            })
            .unwrap_or(0)
    }
}

/// Per-peer rate limiter for gossipsub messages.
#[derive(Debug)]
pub struct PeerRateLimiter {
    /// Configuration
    config: PeerRateLimitConfig,
    /// Per-peer state
    peers: HashMap<PeerId, PeerRateState>,
    /// Peers that have been flagged for disconnection
    flagged_peers: Vec<PeerId>,
    /// Metrics for monitoring
    metrics: RateLimitMetrics,
}

impl PeerRateLimiter {
    /// Create a new rate limiter with the given configuration.
    pub fn new(config: PeerRateLimitConfig) -> Self {
        Self {
            config,
            peers: HashMap::new(),
            flagged_peers: Vec::new(),
            metrics: RateLimitMetrics::new(),
        }
    }

    /// Check if rate limiting is enabled.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Record a message from a peer (untyped, uses Other type).
    /// Returns RateLimitResult indicating if the message should be processed.
    pub fn record_message(&mut self, peer: &PeerId) -> RateLimitResult {
        self.record_message_typed(peer, GossipMessageType::Other)
    }

    /// Record a typed message from a peer.
    /// Returns RateLimitResult indicating if the message should be processed.
    pub fn record_message_typed(
        &mut self,
        peer: &PeerId,
        msg_type: GossipMessageType,
    ) -> RateLimitResult {
        // Always count messages for metrics
        self.metrics.record_message(msg_type);

        if !self.config.enabled {
            return RateLimitResult::Allowed;
        }

        let state = self.peers.entry(*peer).or_default();
        let allowed = state.record_message_typed(&self.config, msg_type);

        if !allowed {
            // Record rate limit hit
            self.metrics.record_rate_limit_hit(msg_type);

            if state.should_disconnect(self.config.disconnect_threshold) {
                self.flagged_peers.push(*peer);
                self.metrics.record_peer_ban();
                RateLimitResult::Disconnect
            } else {
                RateLimitResult::RateLimited {
                    violations: state.violations(),
                    remaining: self.config.disconnect_threshold - state.violations(),
                    message_type: msg_type,
                }
            }
        } else {
            RateLimitResult::Allowed
        }
    }

    /// Get peers flagged for disconnection and clear the list.
    pub fn take_flagged_peers(&mut self) -> Vec<PeerId> {
        std::mem::take(&mut self.flagged_peers)
    }

    /// Remove a peer from tracking (e.g., when disconnected).
    pub fn remove_peer(&mut self, peer: &PeerId) {
        self.peers.remove(peer);
    }

    /// Get statistics for a peer.
    pub fn get_peer_stats(&self, peer: &PeerId) -> Option<PeerRateStats> {
        self.peers.get(peer).map(|state| {
            let window = Duration::from_millis(self.config.burst_window_ms);
            PeerRateStats {
                violations: state.violations(),
                messages_in_window: state.current_message_count(window),
                should_disconnect: state.should_disconnect(self.config.disconnect_threshold),
                transactions_per_minute: state
                    .message_count_by_type(GossipMessageType::Transaction),
                blocks_per_minute: state.message_count_by_type(GossipMessageType::Block),
                consensus_per_minute: state.message_count_by_type(GossipMessageType::Consensus),
            }
        })
    }

    /// Clean up stale peer entries (peers with no recent messages).
    pub fn cleanup_stale_peers(&mut self) {
        let window = Duration::from_millis(self.config.burst_window_ms * 10);
        self.peers
            .retain(|_, state| state.current_message_count(window) > 0 || state.violations > 0);
    }

    /// Get total number of tracked peers.
    pub fn tracked_peer_count(&self) -> usize {
        self.peers.len()
    }

    /// Get rate limiting metrics for monitoring.
    pub fn metrics(&self) -> &RateLimitMetrics {
        &self.metrics
    }

    /// Get a snapshot of current metrics.
    pub fn metrics_snapshot(&self) -> RateLimitMetricsSnapshot {
        self.metrics.snapshot()
    }
}

/// Result of a rate limit check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RateLimitResult {
    /// Message is allowed through.
    Allowed,
    /// Message is rate limited (dropped).
    RateLimited {
        /// Current number of violations.
        violations: u32,
        /// Remaining violations before disconnect.
        remaining: u32,
        /// The message type that was rate limited.
        message_type: GossipMessageType,
    },
    /// Peer should be disconnected due to repeated violations.
    Disconnect,
}

impl RateLimitResult {
    /// Check if the message is allowed.
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
pub struct PeerRateStats {
    /// Number of rate limit violations.
    pub violations: u32,
    /// Messages in the current burst window.
    pub messages_in_window: usize,
    /// Whether the peer should be disconnected.
    pub should_disconnect: bool,
    /// Transaction messages in the last minute.
    pub transactions_per_minute: usize,
    /// Block messages in the last minute.
    pub blocks_per_minute: usize,
    /// Consensus messages in the last minute.
    pub consensus_per_minute: usize,
}

/// Metrics for rate limiting monitoring.
///
/// These metrics can be exposed to monitoring systems (Prometheus, etc.)
/// to track rate limiting effectiveness and detect potential attacks.
#[derive(Debug)]
pub struct RateLimitMetrics {
    /// Total messages received (by type)
    messages_total: HashMap<GossipMessageType, AtomicU64>,
    /// Rate limit hits (by type)
    rate_limit_hits: HashMap<GossipMessageType, AtomicU64>,
    /// Peers banned for rate limit violations
    peers_banned: AtomicU64,
}

impl RateLimitMetrics {
    /// Create new metrics tracker.
    pub fn new() -> Self {
        let mut messages_total = HashMap::new();
        let mut rate_limit_hits = HashMap::new();

        // Initialize counters for each message type
        for msg_type in [
            GossipMessageType::Transaction,
            GossipMessageType::Block,
            GossipMessageType::Consensus,
            GossipMessageType::Announcement,
            GossipMessageType::PeerExchange,
            GossipMessageType::Other,
        ] {
            messages_total.insert(msg_type, AtomicU64::new(0));
            rate_limit_hits.insert(msg_type, AtomicU64::new(0));
        }

        Self {
            messages_total,
            rate_limit_hits,
            peers_banned: AtomicU64::new(0),
        }
    }

    /// Record a message received.
    pub fn record_message(&self, msg_type: GossipMessageType) {
        if let Some(counter) = self.messages_total.get(&msg_type) {
            counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Record a rate limit hit.
    pub fn record_rate_limit_hit(&self, msg_type: GossipMessageType) {
        if let Some(counter) = self.rate_limit_hits.get(&msg_type) {
            counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Record a peer ban.
    pub fn record_peer_ban(&self) {
        self.peers_banned.fetch_add(1, Ordering::Relaxed);
    }

    /// Get total messages received for a type.
    pub fn messages_total(&self, msg_type: GossipMessageType) -> u64 {
        self.messages_total
            .get(&msg_type)
            .map(|c| c.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// Get rate limit hits for a type.
    pub fn rate_limit_hits(&self, msg_type: GossipMessageType) -> u64 {
        self.rate_limit_hits
            .get(&msg_type)
            .map(|c| c.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// Get total peers banned.
    pub fn peers_banned(&self) -> u64 {
        self.peers_banned.load(Ordering::Relaxed)
    }

    /// Get a snapshot of all metrics.
    pub fn snapshot(&self) -> RateLimitMetricsSnapshot {
        RateLimitMetricsSnapshot {
            transactions_total: self.messages_total(GossipMessageType::Transaction),
            transactions_rate_limited: self.rate_limit_hits(GossipMessageType::Transaction),
            blocks_total: self.messages_total(GossipMessageType::Block),
            blocks_rate_limited: self.rate_limit_hits(GossipMessageType::Block),
            consensus_total: self.messages_total(GossipMessageType::Consensus),
            consensus_rate_limited: self.rate_limit_hits(GossipMessageType::Consensus),
            announcements_total: self.messages_total(GossipMessageType::Announcement),
            announcements_rate_limited: self.rate_limit_hits(GossipMessageType::Announcement),
            peers_banned: self.peers_banned(),
        }
    }
}

impl Default for RateLimitMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Snapshot of rate limiting metrics for export.
#[derive(Debug, Clone, Default)]
pub struct RateLimitMetricsSnapshot {
    /// Total transaction messages received.
    pub transactions_total: u64,
    /// Transaction messages rate limited.
    pub transactions_rate_limited: u64,
    /// Total block messages received.
    pub blocks_total: u64,
    /// Block messages rate limited.
    pub blocks_rate_limited: u64,
    /// Total consensus messages received.
    pub consensus_total: u64,
    /// Consensus messages rate limited.
    pub consensus_rate_limited: u64,
    /// Total announcement messages received.
    pub announcements_total: u64,
    /// Announcement messages rate limited.
    pub announcements_rate_limited: u64,
    /// Total peers banned for rate limit violations.
    pub peers_banned: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MessageTypeLimits;

    fn test_config() -> PeerRateLimitConfig {
        PeerRateLimitConfig {
            max_messages_per_second: 5,
            burst_limit: 20,
            burst_window_ms: 1000,
            disconnect_threshold: 3,
            enabled: true,
            message_limits: MessageTypeLimits {
                transactions_per_minute: 10,
                blocks_per_minute: 5,
                consensus_per_minute: 8,
                announcements_per_minute: 5,
            },
        }
    }

    #[test]
    fn test_peer_rate_state_allows_normal_traffic() {
        let config = test_config();
        let mut state = PeerRateState::new();

        // Should allow up to max_messages_per_second
        for _ in 0..5 {
            assert!(state.record_message(&config));
        }

        // 6th message should be rate limited
        assert!(!state.record_message(&config));
        assert_eq!(state.violations(), 1);
    }

    #[test]
    fn test_peer_rate_state_burst_limit() {
        let mut config = test_config();
        config.max_messages_per_second = 100; // High per-second limit
        config.burst_limit = 10;
        config.message_limits.transactions_per_minute = 100; // High type limit

        let mut state = PeerRateState::new();

        // Should allow up to burst_limit
        for _ in 0..10 {
            assert!(state.record_message(&config));
        }

        // Next message should be rate limited (burst limit reached)
        assert!(!state.record_message(&config));
        assert_eq!(state.violations(), 1);
    }

    #[test]
    fn test_per_type_rate_limiting() {
        let mut config = test_config();
        config.max_messages_per_second = 100; // High global limit
        config.burst_limit = 100;
        config.message_limits.transactions_per_minute = 3; // Low transaction limit

        let mut state = PeerRateState::new();

        // Should allow 3 transaction messages
        for _ in 0..3 {
            assert!(state.record_message_typed(&config, GossipMessageType::Transaction));
        }

        // 4th transaction should be rate limited
        assert!(!state.record_message_typed(&config, GossipMessageType::Transaction));
        assert_eq!(state.violations(), 1);

        // But block messages should still be allowed (different type)
        assert!(state.record_message_typed(&config, GossipMessageType::Block));
    }

    #[test]
    fn test_peer_rate_limiter_disconnect() {
        let config = test_config();
        let mut limiter = PeerRateLimiter::new(config);
        let peer = PeerId::random();

        // Trigger violations up to disconnect threshold
        for _ in 0..3 {
            // Send 6 messages to trigger rate limit each time
            for _ in 0..5 {
                limiter.record_message(&peer);
            }
            // This one triggers violation
            limiter.record_message(&peer);
        }

        // After 3 violations, peer should be flagged for disconnect
        let result = limiter.record_message(&peer);
        assert!(
            result.should_disconnect() || matches!(result, RateLimitResult::RateLimited { .. })
        );
    }

    #[test]
    fn test_rate_limiter_disabled() {
        let mut config = test_config();
        config.enabled = false;

        let mut limiter = PeerRateLimiter::new(config);
        let peer = PeerId::random();

        // Should always allow when disabled
        for _ in 0..100 {
            assert!(limiter.record_message(&peer).is_allowed());
        }
    }

    #[test]
    fn test_rate_limiter_remove_peer() {
        let config = test_config();
        let mut limiter = PeerRateLimiter::new(config);
        let peer = PeerId::random();

        limiter.record_message(&peer);
        assert!(limiter.get_peer_stats(&peer).is_some());

        limiter.remove_peer(&peer);
        assert!(limiter.get_peer_stats(&peer).is_none());
    }

    #[test]
    fn test_flagged_peers() {
        let mut config = test_config();
        config.max_messages_per_second = 1;
        config.disconnect_threshold = 1;

        let mut limiter = PeerRateLimiter::new(config);
        let peer = PeerId::random();

        // First message allowed
        assert!(limiter.record_message(&peer).is_allowed());
        // Second triggers violation + disconnect
        let result = limiter.record_message(&peer);
        assert!(result.should_disconnect());

        let flagged = limiter.take_flagged_peers();
        assert_eq!(flagged.len(), 1);
        assert_eq!(flagged[0], peer);

        // List should be cleared
        assert!(limiter.take_flagged_peers().is_empty());
    }

    #[test]
    fn test_typed_rate_limiter() {
        let mut config = test_config();
        config.max_messages_per_second = 100;
        config.burst_limit = 100;
        config.message_limits.blocks_per_minute = 2;

        let mut limiter = PeerRateLimiter::new(config);
        let peer = PeerId::random();

        // Allow 2 block messages
        assert!(limiter
            .record_message_typed(&peer, GossipMessageType::Block)
            .is_allowed());
        assert!(limiter
            .record_message_typed(&peer, GossipMessageType::Block)
            .is_allowed());

        // 3rd block should be rate limited
        let result = limiter.record_message_typed(&peer, GossipMessageType::Block);
        assert!(!result.is_allowed());
        assert!(matches!(
            result,
            RateLimitResult::RateLimited {
                message_type: GossipMessageType::Block,
                ..
            }
        ));
    }

    #[test]
    fn test_metrics_tracking() {
        let config = test_config();
        let mut limiter = PeerRateLimiter::new(config);
        let peer = PeerId::random();

        // Record some messages
        limiter.record_message_typed(&peer, GossipMessageType::Transaction);
        limiter.record_message_typed(&peer, GossipMessageType::Transaction);
        limiter.record_message_typed(&peer, GossipMessageType::Block);

        let snapshot = limiter.metrics_snapshot();
        assert_eq!(snapshot.transactions_total, 2);
        assert_eq!(snapshot.blocks_total, 1);
        assert_eq!(snapshot.transactions_rate_limited, 0);
    }

    #[test]
    fn test_metrics_rate_limit_hits() {
        let mut config = test_config();
        config.max_messages_per_second = 100;
        config.burst_limit = 100;
        config.message_limits.transactions_per_minute = 1;

        let mut limiter = PeerRateLimiter::new(config);
        let peer = PeerId::random();

        // First allowed
        limiter.record_message_typed(&peer, GossipMessageType::Transaction);
        // Second rate limited
        limiter.record_message_typed(&peer, GossipMessageType::Transaction);

        let snapshot = limiter.metrics_snapshot();
        assert_eq!(snapshot.transactions_total, 2);
        assert_eq!(snapshot.transactions_rate_limited, 1);
    }

    #[test]
    fn test_peer_stats_by_type() {
        let mut config = test_config();
        config.max_messages_per_second = 100;
        config.burst_limit = 100;

        let mut limiter = PeerRateLimiter::new(config);
        let peer = PeerId::random();

        // Send different message types
        for _ in 0..3 {
            limiter.record_message_typed(&peer, GossipMessageType::Transaction);
        }
        for _ in 0..2 {
            limiter.record_message_typed(&peer, GossipMessageType::Block);
        }

        let stats = limiter.get_peer_stats(&peer).unwrap();
        assert_eq!(stats.transactions_per_minute, 3);
        assert_eq!(stats.blocks_per_minute, 2);
    }

    #[test]
    fn test_message_type_rate_limits() {
        let config = PeerRateLimitConfig::default();

        // Verify default rate limits from audit requirements
        assert_eq!(
            GossipMessageType::Transaction.rate_limit(&config),
            100 // 100/min for transactions
        );
        assert_eq!(
            GossipMessageType::Block.rate_limit(&config),
            10 // 10/min for blocks
        );
        assert_eq!(
            GossipMessageType::Consensus.rate_limit(&config),
            50 // 50/min for consensus
        );
    }
}
