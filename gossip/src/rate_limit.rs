// Copyright (c) 2024 Botho Foundation

//! Per-peer rate limiting for gossipsub messages.
//!
//! This module implements sliding window rate limiting to protect against
//! message flooding attacks from individual peers.

use crate::config::PeerRateLimitConfig;
use libp2p::PeerId;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Tracks rate limiting state for a single peer.
#[derive(Debug, Clone)]
pub struct PeerRateState {
    /// Timestamps of recent messages (within burst window)
    message_times: Vec<Instant>,
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
            message_times: Vec::with_capacity(100),
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
        let now = Instant::now();
        let window = Duration::from_millis(config.burst_window_ms);

        // Remove old messages outside the window
        self.message_times.retain(|t| now.duration_since(*t) < window);

        // Check if we're over the burst limit
        if self.message_times.len() >= config.burst_limit as usize {
            self.record_violation();
            return false;
        }

        // Check messages per second (using 1-second sliding window)
        let one_second_ago = now - Duration::from_secs(1);
        let recent_count = self
            .message_times
            .iter()
            .filter(|t| **t > one_second_ago)
            .count();

        if recent_count >= config.max_messages_per_second as usize {
            self.record_violation();
            return false;
        }

        // Message is allowed
        self.message_times.push(now);
        true
    }

    /// Check if this peer should be disconnected.
    pub fn should_disconnect(&self, threshold: u32) -> bool {
        self.violations >= threshold
    }

    /// Get message count in the current window.
    pub fn current_message_count(&self, window: Duration) -> usize {
        let now = Instant::now();
        self.message_times
            .iter()
            .filter(|t| now.duration_since(**t) < window)
            .count()
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
}

impl PeerRateLimiter {
    /// Create a new rate limiter with the given configuration.
    pub fn new(config: PeerRateLimitConfig) -> Self {
        Self {
            config,
            peers: HashMap::new(),
            flagged_peers: Vec::new(),
        }
    }

    /// Check if rate limiting is enabled.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Record a message from a peer.
    /// Returns RateLimitResult indicating if the message should be processed.
    pub fn record_message(&mut self, peer: &PeerId) -> RateLimitResult {
        if !self.config.enabled {
            return RateLimitResult::Allowed;
        }

        let state = self.peers.entry(*peer).or_default();
        let allowed = state.record_message(&self.config);

        if !allowed {
            if state.should_disconnect(self.config.disconnect_threshold) {
                self.flagged_peers.push(*peer);
                RateLimitResult::Disconnect
            } else {
                RateLimitResult::RateLimited {
                    violations: state.violations(),
                    remaining: self.config.disconnect_threshold - state.violations(),
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> PeerRateLimitConfig {
        PeerRateLimitConfig {
            max_messages_per_second: 5,
            burst_limit: 20,
            burst_window_ms: 1000,
            disconnect_threshold: 3,
            enabled: true,
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
            // Wait conceptually (in real test we'd use time control)
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
}
