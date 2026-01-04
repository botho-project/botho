// Copyright (c) 2024 Botho Foundation

//! Relay state management for onion gossip.
//!
//! This module provides data structures for managing relay operations:
//! - [`CircuitHopKey`]: Key material for a single circuit hop
//! - [`RelayState`]: Manages circuits where this node acts as a relay
//! - [`RateLimiter`]: Per-peer rate limiting for relay traffic
//!
//! # Relay Architecture
//!
//! Every node in the network can act as a relay hop. When a node receives
//! an onion-wrapped message, it:
//! 1. Looks up the circuit key in [`RelayState`]
//! 2. Decrypts one layer of the onion
//! 3. Forwards to the next hop (or broadcasts if exit)

use libp2p::PeerId;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use super::{
    circuit::OutboundCircuit,
    types::{CircuitId, SymmetricKey},
};

/// Default rate limit window duration.
pub const DEFAULT_RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);

/// Default maximum relay messages per peer per window.
pub const DEFAULT_MAX_RELAY_PER_WINDOW: u32 = 100;

/// Default circuit key expiration time.
pub const DEFAULT_CIRCUIT_KEY_LIFETIME: Duration = Duration::from_secs(900);

/// Key material for one hop of a circuit.
///
/// When this node is a relay hop in someone else's circuit, we store
/// the information needed to process messages for that circuit.
///
/// # Fields
///
/// - `key`: Symmetric key for decrypting messages on this circuit
/// - `next_hop`: Where to forward after decryption (`None` if we're the exit)
/// - `is_exit`: Whether we broadcast the decrypted message (exit hop)
/// - `created_at`: When this circuit hop was established
#[derive(Debug)]
pub struct CircuitHopKey {
    /// Symmetric key for decrypting this hop's layer.
    key: SymmetricKey,

    /// Next hop to forward to after decryption.
    /// `None` if this is the exit hop.
    next_hop: Option<PeerId>,

    /// When this circuit hop was created.
    created_at: Instant,

    /// Whether this is an exit hop (we broadcast the decrypted message).
    is_exit: bool,
}

impl CircuitHopKey {
    /// Create a new circuit hop key for a forward (non-exit) hop.
    pub fn new_forward(key: SymmetricKey, next_hop: PeerId) -> Self {
        Self {
            key,
            next_hop: Some(next_hop),
            created_at: Instant::now(),
            is_exit: false,
        }
    }

    /// Create a new circuit hop key for an exit hop.
    pub fn new_exit(key: SymmetricKey) -> Self {
        Self {
            key,
            next_hop: None,
            created_at: Instant::now(),
            is_exit: true,
        }
    }

    /// Get the symmetric key for this hop.
    pub fn key(&self) -> &SymmetricKey {
        &self.key
    }

    /// Get the next hop peer, if any.
    pub fn next_hop(&self) -> Option<&PeerId> {
        self.next_hop.as_ref()
    }

    /// Check if this is an exit hop.
    pub fn is_exit(&self) -> bool {
        self.is_exit
    }

    /// Get when this circuit hop was created.
    pub fn created_at(&self) -> Instant {
        self.created_at
    }

    /// Get the age of this circuit hop.
    pub fn age(&self) -> Duration {
        Instant::now().duration_since(self.created_at)
    }

    /// Check if this circuit hop key has expired.
    pub fn is_expired(&self, lifetime: Duration) -> bool {
        self.age() > lifetime
    }
}

/// Per-peer rate limiter for relay traffic.
///
/// Prevents any single peer from flooding the relay with messages.
/// Uses a sliding window algorithm to track request counts.
#[derive(Debug)]
pub struct RateLimiter {
    /// Timestamps of recent relay requests.
    request_times: Vec<Instant>,

    /// Maximum requests allowed per window.
    max_requests: u32,

    /// Window duration for rate limiting.
    window: Duration,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_RELAY_PER_WINDOW, DEFAULT_RATE_LIMIT_WINDOW)
    }
}

impl RateLimiter {
    /// Create a new rate limiter with the specified limits.
    pub fn new(max_requests: u32, window: Duration) -> Self {
        Self {
            request_times: Vec::new(),
            max_requests,
            window,
        }
    }

    /// Check if a request should be allowed and record it if so.
    ///
    /// Returns `true` if the request is allowed, `false` if rate limited.
    pub fn check(&mut self) -> bool {
        let now = Instant::now();
        let window_start = now - self.window;

        // Remove requests outside the window
        self.request_times.retain(|&t| t > window_start);

        // Check if under limit
        if self.request_times.len() >= self.max_requests as usize {
            return false;
        }

        // Record this request
        self.request_times.push(now);
        true
    }

    /// Get the current request count within the window.
    pub fn current_count(&self) -> usize {
        let now = Instant::now();
        let window_start = now - self.window;
        self.request_times
            .iter()
            .filter(|&&t| t > window_start)
            .count()
    }

    /// Reset the rate limiter.
    pub fn reset(&mut self) {
        self.request_times.clear();
    }
}

/// Manages relay state for this node.
///
/// Tracks:
/// - Circuit keys for circuits where we're a relay hop
/// - Circuits we've created (we're the origin)
/// - Rate limiting per peer
///
/// # Example
///
/// ```
/// use botho::network::privacy::{RelayState, RelayStateConfig};
///
/// let state = RelayState::new(RelayStateConfig::default());
/// assert_eq!(state.circuit_count(), 0);
/// ```
#[derive(Debug)]
pub struct RelayState {
    /// Circuit keys for decryption (we're a relay hop in these circuits).
    circuit_keys: HashMap<CircuitId, CircuitHopKey>,

    /// Circuits we've created (we're the origin).
    our_circuits: HashMap<CircuitId, OutboundCircuit>,

    /// Rate limiting per peer.
    relay_limits: HashMap<PeerId, RateLimiter>,

    /// Configuration.
    config: RelayStateConfig,
}

/// Configuration for relay state.
#[derive(Debug, Clone)]
pub struct RelayStateConfig {
    /// Maximum relay messages per peer per window.
    pub max_relay_per_window: u32,

    /// Rate limit window duration.
    pub rate_limit_window: Duration,

    /// How long to keep circuit keys.
    pub circuit_key_lifetime: Duration,
}

impl Default for RelayStateConfig {
    fn default() -> Self {
        Self {
            max_relay_per_window: DEFAULT_MAX_RELAY_PER_WINDOW,
            rate_limit_window: DEFAULT_RATE_LIMIT_WINDOW,
            circuit_key_lifetime: DEFAULT_CIRCUIT_KEY_LIFETIME,
        }
    }
}

impl RelayState {
    /// Create a new relay state with the given configuration.
    pub fn new(config: RelayStateConfig) -> Self {
        Self {
            circuit_keys: HashMap::new(),
            our_circuits: HashMap::new(),
            relay_limits: HashMap::new(),
            config,
        }
    }

    /// Get the configuration.
    pub fn config(&self) -> &RelayStateConfig {
        &self.config
    }

    // ========================================================================
    // Circuit Key Management (for relaying others' traffic)
    // ========================================================================

    /// Add a circuit key for relaying.
    ///
    /// Called when we accept being a hop in someone else's circuit.
    pub fn add_circuit_key(&mut self, circuit_id: CircuitId, hop_key: CircuitHopKey) {
        self.circuit_keys.insert(circuit_id, hop_key);
    }

    /// Get a circuit key by ID.
    pub fn get_circuit_key(&self, circuit_id: &CircuitId) -> Option<&CircuitHopKey> {
        self.circuit_keys.get(circuit_id)
    }

    /// Remove a circuit key.
    pub fn remove_circuit_key(&mut self, circuit_id: &CircuitId) -> Option<CircuitHopKey> {
        self.circuit_keys.remove(circuit_id)
    }

    /// Get the number of circuit keys we're holding.
    pub fn circuit_count(&self) -> usize {
        self.circuit_keys.len()
    }

    /// Remove expired circuit keys.
    ///
    /// Returns the number of keys removed.
    pub fn cleanup_expired_keys(&mut self) -> usize {
        let lifetime = self.config.circuit_key_lifetime;
        let before = self.circuit_keys.len();
        self.circuit_keys.retain(|_, key| !key.is_expired(lifetime));
        before - self.circuit_keys.len()
    }

    // ========================================================================
    // Our Circuit Management (for our outbound traffic)
    // ========================================================================

    /// Add a circuit we've created.
    pub fn add_our_circuit(&mut self, circuit: OutboundCircuit) {
        self.our_circuits.insert(*circuit.id(), circuit);
    }

    /// Get one of our circuits by ID.
    pub fn get_our_circuit(&self, circuit_id: &CircuitId) -> Option<&OutboundCircuit> {
        self.our_circuits.get(circuit_id)
    }

    /// Remove one of our circuits.
    pub fn remove_our_circuit(&mut self, circuit_id: &CircuitId) -> Option<OutboundCircuit> {
        self.our_circuits.remove(circuit_id)
    }

    /// Get the number of circuits we've created.
    pub fn our_circuit_count(&self) -> usize {
        self.our_circuits.len()
    }

    /// Remove expired circuits we've created.
    ///
    /// Returns the number of circuits removed.
    pub fn cleanup_expired_circuits(&mut self) -> usize {
        let before = self.our_circuits.len();
        self.our_circuits.retain(|_, c| !c.is_expired());
        before - self.our_circuits.len()
    }

    // ========================================================================
    // Rate Limiting
    // ========================================================================

    /// Check if a relay request from a peer should be allowed.
    ///
    /// Returns `true` if allowed, `false` if rate limited.
    pub fn check_rate_limit(&mut self, peer: &PeerId) -> bool {
        self.relay_limits
            .entry(*peer)
            .or_insert_with(|| {
                RateLimiter::new(
                    self.config.max_relay_per_window,
                    self.config.rate_limit_window,
                )
            })
            .check()
    }

    /// Get the current relay count for a peer.
    pub fn peer_relay_count(&self, peer: &PeerId) -> usize {
        self.relay_limits
            .get(peer)
            .map(|r| r.current_count())
            .unwrap_or(0)
    }

    /// Clean up rate limiters for peers with no recent activity.
    pub fn cleanup_rate_limiters(&mut self) {
        self.relay_limits
            .retain(|_, limiter| limiter.current_count() > 0);
    }

    // ========================================================================
    // Combined Cleanup
    // ========================================================================

    /// Run all cleanup operations.
    ///
    /// Returns (expired_keys, expired_circuits, cleaned_limiters).
    pub fn cleanup_all(&mut self) -> (usize, usize, usize) {
        let expired_keys = self.cleanup_expired_keys();
        let expired_circuits = self.cleanup_expired_circuits();
        let limiter_count_before = self.relay_limits.len();
        self.cleanup_rate_limiters();
        let cleaned_limiters = limiter_count_before - self.relay_limits.len();

        (expired_keys, expired_circuits, cleaned_limiters)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::privacy::{CircuitId, SymmetricKey};

    fn random_symmetric_key() -> SymmetricKey {
        SymmetricKey::random(&mut rand::thread_rng())
    }

    fn random_circuit_id() -> CircuitId {
        CircuitId::random(&mut rand::thread_rng())
    }

    #[test]
    fn test_circuit_hop_key_forward() {
        let key = random_symmetric_key();
        let next_hop = PeerId::random();
        let hop_key = CircuitHopKey::new_forward(key, next_hop.clone());

        assert!(!hop_key.is_exit());
        assert_eq!(hop_key.next_hop(), Some(&next_hop));
        assert!(hop_key.age() < Duration::from_secs(1));
    }

    #[test]
    fn test_circuit_hop_key_exit() {
        let key = random_symmetric_key();
        let hop_key = CircuitHopKey::new_exit(key);

        assert!(hop_key.is_exit());
        assert!(hop_key.next_hop().is_none());
    }

    #[test]
    fn test_circuit_hop_key_expiry() {
        let key = random_symmetric_key();
        let hop_key = CircuitHopKey::new_exit(key);

        // Not expired with long lifetime
        assert!(!hop_key.is_expired(Duration::from_secs(600)));

        // Expired with very short lifetime
        std::thread::sleep(Duration::from_millis(10));
        assert!(hop_key.is_expired(Duration::from_millis(1)));
    }

    #[test]
    fn test_rate_limiter_allows_under_limit() {
        let mut limiter = RateLimiter::new(10, Duration::from_secs(60));

        for _ in 0..10 {
            assert!(limiter.check());
        }
    }

    #[test]
    fn test_rate_limiter_blocks_over_limit() {
        let mut limiter = RateLimiter::new(3, Duration::from_secs(60));

        assert!(limiter.check());
        assert!(limiter.check());
        assert!(limiter.check());
        assert!(!limiter.check()); // Should be blocked
    }

    #[test]
    fn test_rate_limiter_reset() {
        let mut limiter = RateLimiter::new(2, Duration::from_secs(60));

        assert!(limiter.check());
        assert!(limiter.check());
        assert!(!limiter.check());

        limiter.reset();
        assert!(limiter.check()); // Should work after reset
    }

    #[test]
    fn test_relay_state_creation() {
        let state = RelayState::new(RelayStateConfig::default());
        assert_eq!(state.circuit_count(), 0);
        assert_eq!(state.our_circuit_count(), 0);
    }

    #[test]
    fn test_relay_state_circuit_keys() {
        let mut state = RelayState::new(RelayStateConfig::default());

        let circuit_id = random_circuit_id();
        let hop_key = CircuitHopKey::new_exit(random_symmetric_key());

        state.add_circuit_key(circuit_id, hop_key);
        assert_eq!(state.circuit_count(), 1);
        assert!(state.get_circuit_key(&circuit_id).is_some());

        state.remove_circuit_key(&circuit_id);
        assert_eq!(state.circuit_count(), 0);
    }

    #[test]
    fn test_relay_state_rate_limiting() {
        let config = RelayStateConfig {
            max_relay_per_window: 2,
            ..Default::default()
        };
        let mut state = RelayState::new(config);
        let peer = PeerId::random();

        assert!(state.check_rate_limit(&peer));
        assert!(state.check_rate_limit(&peer));
        assert!(!state.check_rate_limit(&peer)); // Should be rate limited
    }

    #[test]
    fn test_relay_state_cleanup_expired_keys() {
        let config = RelayStateConfig {
            circuit_key_lifetime: Duration::from_millis(1),
            ..Default::default()
        };
        let mut state = RelayState::new(config);

        // Add a circuit key
        state.add_circuit_key(
            random_circuit_id(),
            CircuitHopKey::new_exit(random_symmetric_key()),
        );
        assert_eq!(state.circuit_count(), 1);

        // Wait for expiration
        std::thread::sleep(Duration::from_millis(10));

        // Cleanup should remove expired key
        let removed = state.cleanup_expired_keys();
        assert_eq!(removed, 1);
        assert_eq!(state.circuit_count(), 0);
    }

    #[test]
    fn test_relay_state_multiple_peers_rate_limits() {
        let config = RelayStateConfig {
            max_relay_per_window: 1,
            ..Default::default()
        };
        let mut state = RelayState::new(config);

        let peer1 = PeerId::random();
        let peer2 = PeerId::random();

        // Each peer gets their own limit
        assert!(state.check_rate_limit(&peer1));
        assert!(state.check_rate_limit(&peer2));

        // Both should be rate limited now
        assert!(!state.check_rate_limit(&peer1));
        assert!(!state.check_rate_limit(&peer2));
    }
}
