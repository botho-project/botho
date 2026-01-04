// Copyright (c) 2024 Botho Foundation

//! Circuit management for onion gossip routing.
//!
//! This module provides data structures for managing outbound circuits:
//! - [`OutboundCircuit`]: A pre-built circuit through 3 relay hops
//! - [`CircuitPool`]: Pool of active circuits for quick message sending
//!
//! # Architecture
//!
//! Circuits are 3-hop paths through the relay network. Each hop has a
//! symmetric key established via a telescoping handshake. When sending
//! a message, the payload is wrapped in 3 layers of encryption (onion),
//! with each hop decrypting one layer.
//!
//! ```text
//! Alice -> Hop1 -> Hop2 -> Hop3 (exit) -> Gossipsub
//! ```

use libp2p::PeerId;
use std::time::{Duration, Instant};

use super::types::{CircuitId, SymmetricKey};

/// Number of hops in a standard circuit.
pub const CIRCUIT_HOPS: usize = 3;

/// Default minimum number of circuits to maintain in the pool.
pub const DEFAULT_MIN_CIRCUITS: usize = 3;

/// Default circuit rotation interval (10 minutes).
pub const DEFAULT_ROTATION_INTERVAL: Duration = Duration::from_secs(600);

/// Maximum jitter to add to circuit lifetime (3 minutes).
pub const MAX_LIFETIME_JITTER: Duration = Duration::from_secs(180);

/// An outbound circuit through 3 relay hops.
///
/// A circuit represents a pre-built path through the relay network that
/// can be used to send messages anonymously. The circuit consists of:
/// - A unique identifier
/// - Three relay hop peers
/// - Symmetric keys for each hop (for onion encryption)
/// - Timing information for expiration
///
/// # Example
///
/// ```
/// use botho::network::privacy::{OutboundCircuit, CircuitId, SymmetricKey};
/// use libp2p::PeerId;
/// use std::time::{Duration, Instant};
///
/// // Create circuit components
/// let mut rng = rand::thread_rng();
/// let circuit_id = CircuitId::random(&mut rng);
/// let hops = [PeerId::random(), PeerId::random(), PeerId::random()];
/// let keys = [
///     SymmetricKey::random(&mut rng),
///     SymmetricKey::random(&mut rng),
///     SymmetricKey::random(&mut rng),
/// ];
///
/// let circuit = OutboundCircuit::new(
///     circuit_id,
///     hops,
///     keys,
///     Duration::from_secs(600),
/// );
///
/// assert!(!circuit.is_expired());
/// ```
#[derive(Debug)]
pub struct OutboundCircuit {
    /// Unique circuit identifier.
    id: CircuitId,

    /// Ordered relay hops (first hop -> middle hop -> exit hop).
    hops: [PeerId; CIRCUIT_HOPS],

    /// Symmetric keys for each hop (for onion encryption).
    /// Each key is used to encrypt/decrypt the layer for that hop.
    hop_keys: [SymmetricKey; CIRCUIT_HOPS],

    /// When this circuit was built.
    created_at: Instant,

    /// When this circuit should be rotated (randomized around
    /// rotation_interval).
    expires_at: Instant,
}

impl OutboundCircuit {
    /// Create a new outbound circuit.
    ///
    /// # Arguments
    ///
    /// * `id` - Unique circuit identifier
    /// * `hops` - The three relay peers in order [entry, middle, exit]
    /// * `hop_keys` - Symmetric keys for each hop
    /// * `lifetime` - Base lifetime before expiration (jitter will be added)
    pub fn new(
        id: CircuitId,
        hops: [PeerId; CIRCUIT_HOPS],
        hop_keys: [SymmetricKey; CIRCUIT_HOPS],
        lifetime: Duration,
    ) -> Self {
        let now = Instant::now();
        // Add random jitter to lifetime to prevent timing correlation
        let jitter =
            Duration::from_millis(rand::random::<u64>() % MAX_LIFETIME_JITTER.as_millis() as u64);
        let expires_at = now + lifetime + jitter;

        Self {
            id,
            hops,
            hop_keys,
            created_at: now,
            expires_at,
        }
    }

    /// Create a new outbound circuit with exact expiration time (for testing).
    ///
    /// Unlike `new()`, this method does not add jitter to the lifetime.
    /// This is primarily useful for testing expiration behavior.
    #[cfg(test)]
    pub fn new_exact_lifetime(
        id: CircuitId,
        hops: [PeerId; CIRCUIT_HOPS],
        hop_keys: [SymmetricKey; CIRCUIT_HOPS],
        lifetime: Duration,
    ) -> Self {
        let now = Instant::now();
        Self {
            id,
            hops,
            hop_keys,
            created_at: now,
            expires_at: now + lifetime,
        }
    }

    /// Get the circuit's unique identifier.
    #[inline]
    pub fn id(&self) -> &CircuitId {
        &self.id
    }

    /// Get the relay hops in order [entry, middle, exit].
    #[inline]
    pub fn hops(&self) -> &[PeerId; CIRCUIT_HOPS] {
        &self.hops
    }

    /// Get the first (entry) hop peer.
    #[inline]
    pub fn entry_hop(&self) -> &PeerId {
        &self.hops[0]
    }

    /// Get the middle hop peer.
    #[inline]
    pub fn middle_hop(&self) -> &PeerId {
        &self.hops[1]
    }

    /// Get the exit hop peer.
    #[inline]
    pub fn exit_hop(&self) -> &PeerId {
        &self.hops[2]
    }

    /// Get the symmetric key for a specific hop (0 = entry, 1 = middle, 2 =
    /// exit).
    ///
    /// # Panics
    ///
    /// Panics if `hop_index >= CIRCUIT_HOPS`.
    #[inline]
    pub fn hop_key(&self, hop_index: usize) -> &SymmetricKey {
        &self.hop_keys[hop_index]
    }

    /// Get when this circuit was created.
    #[inline]
    pub fn created_at(&self) -> Instant {
        self.created_at
    }

    /// Get when this circuit expires.
    #[inline]
    pub fn expires_at(&self) -> Instant {
        self.expires_at
    }

    /// Check if this circuit has expired and should be rotated.
    pub fn is_expired(&self) -> bool {
        Instant::now() >= self.expires_at
    }

    /// Get the age of this circuit.
    pub fn age(&self) -> Duration {
        Instant::now().duration_since(self.created_at)
    }

    /// Get remaining time until expiration.
    ///
    /// Returns `Duration::ZERO` if already expired.
    pub fn time_remaining(&self) -> Duration {
        let now = Instant::now();
        if now >= self.expires_at {
            Duration::ZERO
        } else {
            self.expires_at - now
        }
    }
}

/// Configuration for the circuit pool.
#[derive(Debug, Clone)]
pub struct CircuitPoolConfig {
    /// Minimum number of circuits to maintain.
    pub min_circuits: usize,

    /// Circuit rotation interval (before jitter).
    pub rotation_interval: Duration,
}

impl Default for CircuitPoolConfig {
    fn default() -> Self {
        Self {
            min_circuits: DEFAULT_MIN_CIRCUITS,
            rotation_interval: DEFAULT_ROTATION_INTERVAL,
        }
    }
}

/// Pool of pre-built circuits for quick message sending.
///
/// The pool maintains a set of active circuits that can be used immediately
/// for sending messages. Circuits are rotated based on their expiration time,
/// and new circuits are built in the background to maintain the minimum count.
///
/// # Example
///
/// ```
/// use botho::network::privacy::{CircuitPool, CircuitPoolConfig};
///
/// // Create with default configuration
/// let pool = CircuitPool::new(CircuitPoolConfig::default());
///
/// // Check pool status
/// assert_eq!(pool.active_count(), 0);
/// assert!(pool.needs_more_circuits());
/// ```
#[derive(Debug)]
pub struct CircuitPool {
    /// Active circuits ready for use.
    active: Vec<OutboundCircuit>,

    /// Pool configuration.
    config: CircuitPoolConfig,
}

impl CircuitPool {
    /// Create a new circuit pool with the given configuration.
    pub fn new(config: CircuitPoolConfig) -> Self {
        Self {
            active: Vec::with_capacity(config.min_circuits),
            config,
        }
    }

    /// Get the number of active (non-expired) circuits.
    pub fn active_count(&self) -> usize {
        self.active.iter().filter(|c| !c.is_expired()).count()
    }

    /// Get the total number of circuits (including expired).
    pub fn total_count(&self) -> usize {
        self.active.len()
    }

    /// Check if the pool needs more circuits to meet the minimum.
    pub fn needs_more_circuits(&self) -> bool {
        self.active_count() < self.config.min_circuits
    }

    /// Get the pool configuration.
    pub fn config(&self) -> &CircuitPoolConfig {
        &self.config
    }

    /// Add a circuit to the pool.
    pub fn add_circuit(&mut self, circuit: OutboundCircuit) {
        self.active.push(circuit);
    }

    /// Get a random active circuit for sending a message.
    ///
    /// Returns `None` if no active (non-expired) circuits are available.
    pub fn get_circuit(&self) -> Option<&OutboundCircuit> {
        let active_circuits: Vec<_> = self.active.iter().filter(|c| !c.is_expired()).collect();

        if active_circuits.is_empty() {
            return None;
        }

        // Use random selection for unlinkability
        let index = rand::random::<usize>() % active_circuits.len();
        Some(active_circuits[index])
    }

    /// Remove expired circuits from the pool.
    ///
    /// Returns the number of circuits removed.
    pub fn remove_expired(&mut self) -> usize {
        let before = self.active.len();
        self.active.retain(|c| !c.is_expired());
        before - self.active.len()
    }

    /// Remove a specific circuit by ID.
    ///
    /// Returns `true` if the circuit was found and removed.
    pub fn remove_circuit(&mut self, circuit_id: &CircuitId) -> bool {
        let before = self.active.len();
        self.active.retain(|c| c.id() != circuit_id);
        self.active.len() < before
    }

    /// Clear all circuits from the pool.
    pub fn clear(&mut self) {
        self.active.clear();
    }

    /// Iterate over all active circuits.
    pub fn iter(&self) -> impl Iterator<Item = &OutboundCircuit> {
        self.active.iter().filter(|c| !c.is_expired())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_circuit(lifetime: Duration) -> OutboundCircuit {
        let mut rng = rand::thread_rng();
        OutboundCircuit::new(
            CircuitId::random(&mut rng),
            [PeerId::random(), PeerId::random(), PeerId::random()],
            [
                SymmetricKey::random(&mut rng),
                SymmetricKey::random(&mut rng),
                SymmetricKey::random(&mut rng),
            ],
            lifetime,
        )
    }

    #[test]
    fn test_circuit_creation() {
        let circuit = make_test_circuit(Duration::from_secs(600));

        assert!(!circuit.is_expired());
        assert!(circuit.time_remaining() > Duration::ZERO);
        assert!(circuit.age() < Duration::from_secs(1));
    }

    #[test]
    fn test_circuit_hops() {
        let mut rng = rand::thread_rng();
        let hops = [PeerId::random(), PeerId::random(), PeerId::random()];
        let circuit = OutboundCircuit::new(
            CircuitId::random(&mut rng),
            hops.clone(),
            [
                SymmetricKey::random(&mut rng),
                SymmetricKey::random(&mut rng),
                SymmetricKey::random(&mut rng),
            ],
            Duration::from_secs(600),
        );

        assert_eq!(circuit.entry_hop(), &hops[0]);
        assert_eq!(circuit.middle_hop(), &hops[1]);
        assert_eq!(circuit.exit_hop(), &hops[2]);
    }

    #[test]
    fn test_circuit_expiry() {
        let mut rng = rand::thread_rng();
        // Create circuit with very short lifetime (using exact lifetime for testing)
        let circuit = OutboundCircuit::new_exact_lifetime(
            CircuitId::random(&mut rng),
            [PeerId::random(), PeerId::random(), PeerId::random()],
            [
                SymmetricKey::random(&mut rng),
                SymmetricKey::random(&mut rng),
                SymmetricKey::random(&mut rng),
            ],
            Duration::from_millis(1),
        );

        // Wait for expiration
        std::thread::sleep(Duration::from_millis(10));

        assert!(circuit.is_expired());
        assert_eq!(circuit.time_remaining(), Duration::ZERO);
    }

    #[test]
    fn test_pool_creation() {
        let pool = CircuitPool::new(CircuitPoolConfig::default());

        assert_eq!(pool.active_count(), 0);
        assert_eq!(pool.total_count(), 0);
        assert!(pool.needs_more_circuits());
    }

    #[test]
    fn test_pool_add_circuit() {
        let mut pool = CircuitPool::new(CircuitPoolConfig::default());
        let circuit = make_test_circuit(Duration::from_secs(600));

        pool.add_circuit(circuit);

        assert_eq!(pool.active_count(), 1);
        assert!(pool.needs_more_circuits()); // Default min is 3
    }

    #[test]
    fn test_pool_get_circuit() {
        let mut pool = CircuitPool::new(CircuitPoolConfig::default());

        // Empty pool returns None
        assert!(pool.get_circuit().is_none());

        // Add a circuit
        pool.add_circuit(make_test_circuit(Duration::from_secs(600)));

        // Should get a circuit now
        assert!(pool.get_circuit().is_some());
    }

    #[test]
    fn test_pool_remove_expired() {
        let mut pool = CircuitPool::new(CircuitPoolConfig::default());
        let mut rng = rand::thread_rng();

        // Add short-lived circuit (exact lifetime for testing)
        let short_circuit = OutboundCircuit::new_exact_lifetime(
            CircuitId::random(&mut rng),
            [PeerId::random(), PeerId::random(), PeerId::random()],
            [
                SymmetricKey::random(&mut rng),
                SymmetricKey::random(&mut rng),
                SymmetricKey::random(&mut rng),
            ],
            Duration::from_millis(1),
        );
        pool.add_circuit(short_circuit);

        // Add long-lived circuit
        pool.add_circuit(make_test_circuit(Duration::from_secs(600)));

        assert_eq!(pool.total_count(), 2);

        // Wait for short-lived circuit to expire
        std::thread::sleep(Duration::from_millis(10));

        // Remove expired
        let removed = pool.remove_expired();
        assert_eq!(removed, 1);
        assert_eq!(pool.total_count(), 1);
    }

    #[test]
    fn test_pool_remove_by_id() {
        let mut pool = CircuitPool::new(CircuitPoolConfig::default());
        let circuit = make_test_circuit(Duration::from_secs(600));
        let circuit_id = *circuit.id();

        pool.add_circuit(circuit);
        assert_eq!(pool.active_count(), 1);

        assert!(pool.remove_circuit(&circuit_id));
        assert_eq!(pool.active_count(), 0);
    }

    #[test]
    fn test_pool_min_circuits_threshold() {
        let config = CircuitPoolConfig {
            min_circuits: 2,
            ..Default::default()
        };
        let mut pool = CircuitPool::new(config);

        // Below threshold
        pool.add_circuit(make_test_circuit(Duration::from_secs(600)));
        assert!(pool.needs_more_circuits());

        // At threshold
        pool.add_circuit(make_test_circuit(Duration::from_secs(600)));
        assert!(!pool.needs_more_circuits());

        // Above threshold
        pool.add_circuit(make_test_circuit(Duration::from_secs(600)));
        assert!(!pool.needs_more_circuits());
    }

    #[test]
    fn test_pool_iter() {
        let mut pool = CircuitPool::new(CircuitPoolConfig::default());

        for _ in 0..5 {
            pool.add_circuit(make_test_circuit(Duration::from_secs(600)));
        }

        assert_eq!(pool.iter().count(), 5);
    }

    #[test]
    fn test_pool_clear() {
        let mut pool = CircuitPool::new(CircuitPoolConfig::default());

        for _ in 0..3 {
            pool.add_circuit(make_test_circuit(Duration::from_secs(600)));
        }

        pool.clear();
        assert_eq!(pool.active_count(), 0);
    }
}
