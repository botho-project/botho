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
use std::{
    future::Future,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::RwLock;
use tracing::{debug, warn};

use super::types::{CircuitId, SymmetricKey};

/// Number of hops in a standard circuit.
pub const CIRCUIT_HOPS: usize = 3;

/// Default minimum number of circuits to maintain in the pool.
pub const DEFAULT_MIN_CIRCUITS: usize = 3;

/// Default circuit rotation interval (10 minutes).
pub const DEFAULT_ROTATION_INTERVAL: Duration = Duration::from_secs(600);

/// Maximum jitter to add to circuit lifetime (3 minutes).
pub const MAX_LIFETIME_JITTER: Duration = Duration::from_secs(180);

/// Default maintenance loop interval (30 seconds).
pub const DEFAULT_MAINTENANCE_INTERVAL: Duration = Duration::from_secs(30);

/// Default threshold for pre-emptive circuit rebuild (2 minutes before expiry).
pub const DEFAULT_REBUILD_THRESHOLD: Duration = Duration::from_secs(120);

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

    /// How often to run maintenance (remove expired, build new).
    pub maintenance_interval: Duration,

    /// Pre-emptively rebuild circuits expiring within this duration.
    pub rebuild_threshold: Duration,
}

impl Default for CircuitPoolConfig {
    fn default() -> Self {
        Self {
            min_circuits: DEFAULT_MIN_CIRCUITS,
            rotation_interval: DEFAULT_ROTATION_INTERVAL,
            maintenance_interval: DEFAULT_MAINTENANCE_INTERVAL,
            rebuild_threshold: DEFAULT_REBUILD_THRESHOLD,
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

    /// Count circuits that will expire within the given duration.
    ///
    /// This is used to pre-emptively rebuild circuits before they expire,
    /// ensuring we always have enough ready circuits.
    pub fn expiring_within(&self, threshold: Duration) -> usize {
        self.active
            .iter()
            .filter(|c| !c.is_expired() && c.time_remaining() < threshold)
            .count()
    }

    /// Calculate how many new circuits need to be built.
    ///
    /// This accounts for:
    /// - The minimum circuit threshold
    /// - Circuits that will expire soon (within rebuild_threshold)
    pub fn circuits_needed(&self) -> usize {
        let active = self.active_count();
        let expiring_soon = self.expiring_within(self.config.rebuild_threshold);

        // Need to replace expired circuits and pre-emptively build for expiring ones
        let target = self.config.min_circuits + expiring_soon;
        target.saturating_sub(active)
    }

    /// Run maintenance: remove expired circuits.
    ///
    /// Returns the number of circuits removed.
    /// Note: Building new circuits is handled separately by the maintainer.
    pub fn maintain(&mut self) -> MaintenanceResult {
        let removed = self.remove_expired();
        let active = self.active_count();
        let expiring_soon = self.expiring_within(self.config.rebuild_threshold);
        let needed = self.circuits_needed();

        MaintenanceResult {
            removed,
            active,
            expiring_soon,
            circuits_needed: needed,
        }
    }
}

/// Result of a maintenance operation.
#[derive(Debug, Clone, Copy)]
pub struct MaintenanceResult {
    /// Number of expired circuits removed.
    pub removed: usize,
    /// Number of active circuits remaining.
    pub active: usize,
    /// Number of circuits expiring soon.
    pub expiring_soon: usize,
    /// Number of new circuits needed.
    pub circuits_needed: usize,
}

/// Metrics for circuit pool operations.
///
/// All counters are atomic and can be read from any thread.
#[derive(Debug, Default)]
pub struct CircuitPoolMetrics {
    /// Total number of circuits successfully built.
    pub circuits_built: AtomicU64,
    /// Total number of circuit build failures.
    pub build_failures: AtomicU64,
    /// Total number of circuits removed due to expiration.
    pub circuits_expired: AtomicU64,
    /// Total number of maintenance cycles completed.
    pub maintenance_cycles: AtomicU64,
}

impl CircuitPoolMetrics {
    /// Create new metrics with all counters at zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// Increment the circuits built counter.
    pub fn record_circuit_built(&self) {
        self.circuits_built.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the build failures counter.
    pub fn record_build_failure(&self) {
        self.build_failures.fetch_add(1, Ordering::Relaxed);
    }

    /// Add to the circuits expired counter.
    pub fn record_circuits_expired(&self, count: u64) {
        self.circuits_expired.fetch_add(count, Ordering::Relaxed);
    }

    /// Increment the maintenance cycles counter.
    pub fn record_maintenance_cycle(&self) {
        self.maintenance_cycles.fetch_add(1, Ordering::Relaxed);
    }

    /// Get the current count of circuits built.
    pub fn get_circuits_built(&self) -> u64 {
        self.circuits_built.load(Ordering::Relaxed)
    }

    /// Get the current count of build failures.
    pub fn get_build_failures(&self) -> u64 {
        self.build_failures.load(Ordering::Relaxed)
    }

    /// Get the current count of expired circuits.
    pub fn get_circuits_expired(&self) -> u64 {
        self.circuits_expired.load(Ordering::Relaxed)
    }

    /// Get the current count of maintenance cycles.
    pub fn get_maintenance_cycles(&self) -> u64 {
        self.maintenance_cycles.load(Ordering::Relaxed)
    }
}

/// Thread-safe circuit pool wrapper for async maintenance.
pub type SharedCircuitPool = Arc<RwLock<CircuitPool>>;

/// Creates a new shared circuit pool.
pub fn new_shared_pool(config: CircuitPoolConfig) -> SharedCircuitPool {
    Arc::new(RwLock::new(CircuitPool::new(config)))
}

/// Background circuit pool maintainer.
///
/// Handles periodic maintenance tasks:
/// - Removing expired circuits
/// - Building new circuits to maintain minimum count
/// - Pre-emptively rebuilding circuits nearing expiry
///
/// # Example
///
/// ```ignore
/// use botho::network::privacy::circuit::{
///     CircuitPoolMaintainer, CircuitPoolConfig, new_shared_pool,
/// };
///
/// let pool = new_shared_pool(CircuitPoolConfig::default());
/// let metrics = Arc::new(CircuitPoolMetrics::new());
///
/// // The builder function creates new circuits
/// let builder = |pool: &SharedCircuitPool| async move {
///     // Build circuit via handshake protocol...
///     Ok(new_circuit)
/// };
///
/// let maintainer = CircuitPoolMaintainer::new(pool, metrics);
/// let handle = maintainer.spawn(builder);
/// ```
pub struct CircuitPoolMaintainer {
    pool: SharedCircuitPool,
    metrics: Arc<CircuitPoolMetrics>,
    config: CircuitPoolConfig,
}

impl CircuitPoolMaintainer {
    /// Create a new circuit pool maintainer.
    pub fn new(pool: SharedCircuitPool, metrics: Arc<CircuitPoolMetrics>) -> Self {
        // Clone config from pool
        let config = {
            let pool_guard =
                futures::executor::block_on(async { pool.read().await.config().clone() });
            pool_guard
        };

        Self {
            pool,
            metrics,
            config,
        }
    }

    /// Create a new maintainer with explicit config.
    pub fn with_config(
        pool: SharedCircuitPool,
        metrics: Arc<CircuitPoolMetrics>,
        config: CircuitPoolConfig,
    ) -> Self {
        Self {
            pool,
            metrics,
            config,
        }
    }

    /// Run a single maintenance cycle.
    ///
    /// Returns the maintenance result after removing expired circuits.
    /// The caller should use the result to determine how many circuits to
    /// build.
    pub async fn run_maintenance(&self) -> MaintenanceResult {
        let mut pool = self.pool.write().await;
        let result = pool.maintain();

        // Record metrics
        self.metrics.record_circuits_expired(result.removed as u64);
        self.metrics.record_maintenance_cycle();

        debug!(
            removed = result.removed,
            active = result.active,
            expiring_soon = result.expiring_soon,
            needed = result.circuits_needed,
            "Circuit pool maintenance completed"
        );

        result
    }

    /// Add a newly built circuit to the pool.
    pub async fn add_circuit(&self, circuit: OutboundCircuit) {
        let mut pool = self.pool.write().await;
        pool.add_circuit(circuit);
        self.metrics.record_circuit_built();
    }

    /// Record a circuit build failure.
    pub fn record_build_failure(&self) {
        self.metrics.record_build_failure();
    }

    /// Get the shared pool reference.
    pub fn pool(&self) -> &SharedCircuitPool {
        &self.pool
    }

    /// Get the metrics reference.
    pub fn metrics(&self) -> &Arc<CircuitPoolMetrics> {
        &self.metrics
    }

    /// Spawn the background maintenance loop.
    ///
    /// The `build_circuit` function is called to create new circuits when
    /// needed. It should handle all the network communication for circuit
    /// establishment.
    ///
    /// Returns a handle to the spawned task.
    pub fn spawn<F, Fut>(self, mut build_circuit: F) -> tokio::task::JoinHandle<()>
    where
        F: FnMut() -> Fut + Send + 'static,
        Fut: Future<Output = Result<OutboundCircuit, Box<dyn std::error::Error + Send + Sync>>>
            + Send,
    {
        let interval = self.config.maintenance_interval;

        tokio::spawn(async move {
            loop {
                // Run maintenance
                let result = self.run_maintenance().await;

                // Build needed circuits concurrently
                if result.circuits_needed > 0 {
                    debug!(
                        needed = result.circuits_needed,
                        "Building new circuits to maintain pool"
                    );

                    // Build circuits one at a time to avoid overwhelming peers
                    // Future improvement: concurrent builds with rate limiting
                    for _ in 0..result.circuits_needed {
                        match build_circuit().await {
                            Ok(circuit) => {
                                self.add_circuit(circuit).await;
                            }
                            Err(e) => {
                                warn!(error = %e, "Failed to build circuit");
                                self.record_build_failure();
                            }
                        }
                    }
                }

                // Wait for next maintenance interval
                tokio::time::sleep(interval).await;
            }
        })
    }

    /// Spawn maintenance with a circuit builder that has access to the pool.
    ///
    /// This variant passes a reference to the shared pool to the builder
    /// function, which can be useful for checking pool state during circuit
    /// building.
    pub fn spawn_with_pool<F, Fut>(self, mut build_circuit: F) -> tokio::task::JoinHandle<()>
    where
        F: FnMut(SharedCircuitPool) -> Fut + Send + 'static,
        Fut: Future<Output = Result<OutboundCircuit, Box<dyn std::error::Error + Send + Sync>>>
            + Send,
    {
        let interval = self.config.maintenance_interval;
        let pool_clone = Arc::clone(&self.pool);

        tokio::spawn(async move {
            loop {
                let result = self.run_maintenance().await;

                if result.circuits_needed > 0 {
                    debug!(
                        needed = result.circuits_needed,
                        "Building new circuits to maintain pool"
                    );

                    for _ in 0..result.circuits_needed {
                        match build_circuit(Arc::clone(&pool_clone)).await {
                            Ok(circuit) => {
                                self.add_circuit(circuit).await;
                            }
                            Err(e) => {
                                warn!(error = %e, "Failed to build circuit");
                                self.record_build_failure();
                            }
                        }
                    }
                }

                tokio::time::sleep(interval).await;
            }
        })
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

    #[test]
    fn test_expiring_within() {
        let config = CircuitPoolConfig {
            rebuild_threshold: Duration::from_secs(120),
            ..Default::default()
        };
        let mut pool = CircuitPool::new(config);
        let mut rng = rand::thread_rng();

        // Add circuit expiring in 60 seconds (within threshold)
        let expiring_soon = OutboundCircuit::new_exact_lifetime(
            CircuitId::random(&mut rng),
            [PeerId::random(), PeerId::random(), PeerId::random()],
            [
                SymmetricKey::random(&mut rng),
                SymmetricKey::random(&mut rng),
                SymmetricKey::random(&mut rng),
            ],
            Duration::from_secs(60),
        );
        pool.add_circuit(expiring_soon);

        // Add circuit expiring in 300 seconds (outside threshold)
        let not_expiring = OutboundCircuit::new_exact_lifetime(
            CircuitId::random(&mut rng),
            [PeerId::random(), PeerId::random(), PeerId::random()],
            [
                SymmetricKey::random(&mut rng),
                SymmetricKey::random(&mut rng),
                SymmetricKey::random(&mut rng),
            ],
            Duration::from_secs(300),
        );
        pool.add_circuit(not_expiring);

        // One circuit expiring within 120 seconds
        assert_eq!(pool.expiring_within(Duration::from_secs(120)), 1);

        // Both within 600 seconds
        assert_eq!(pool.expiring_within(Duration::from_secs(600)), 2);

        // None within 30 seconds
        assert_eq!(pool.expiring_within(Duration::from_secs(30)), 0);
    }

    #[test]
    fn test_circuits_needed() {
        let config = CircuitPoolConfig {
            min_circuits: 3,
            rebuild_threshold: Duration::from_secs(120),
            ..Default::default()
        };
        let mut pool = CircuitPool::new(config);
        let mut rng = rand::thread_rng();

        // Empty pool needs 3 circuits
        assert_eq!(pool.circuits_needed(), 3);

        // Add 2 long-lived circuits
        for _ in 0..2 {
            pool.add_circuit(OutboundCircuit::new_exact_lifetime(
                CircuitId::random(&mut rng),
                [PeerId::random(), PeerId::random(), PeerId::random()],
                [
                    SymmetricKey::random(&mut rng),
                    SymmetricKey::random(&mut rng),
                    SymmetricKey::random(&mut rng),
                ],
                Duration::from_secs(600),
            ));
        }
        // Need 1 more to reach minimum
        assert_eq!(pool.circuits_needed(), 1);

        // Add 1 more long-lived circuit (now at min)
        pool.add_circuit(OutboundCircuit::new_exact_lifetime(
            CircuitId::random(&mut rng),
            [PeerId::random(), PeerId::random(), PeerId::random()],
            [
                SymmetricKey::random(&mut rng),
                SymmetricKey::random(&mut rng),
                SymmetricKey::random(&mut rng),
            ],
            Duration::from_secs(600),
        ));
        assert_eq!(pool.circuits_needed(), 0);

        // Add circuit expiring soon - should need to pre-build replacement
        pool.add_circuit(OutboundCircuit::new_exact_lifetime(
            CircuitId::random(&mut rng),
            [PeerId::random(), PeerId::random(), PeerId::random()],
            [
                SymmetricKey::random(&mut rng),
                SymmetricKey::random(&mut rng),
                SymmetricKey::random(&mut rng),
            ],
            Duration::from_secs(60), // Expires within rebuild_threshold
        ));
        // min=3, have 4, but 1 expiring soon, so need (3+1)-4 = 0
        // Actually: target = 3 + 1 = 4, active = 4, so need 0
        assert_eq!(pool.circuits_needed(), 0);
    }

    #[test]
    fn test_maintain() {
        let config = CircuitPoolConfig {
            min_circuits: 2,
            rebuild_threshold: Duration::from_secs(120),
            ..Default::default()
        };
        let mut pool = CircuitPool::new(config);
        let mut rng = rand::thread_rng();

        // Add expired circuit
        let expired = OutboundCircuit::new_exact_lifetime(
            CircuitId::random(&mut rng),
            [PeerId::random(), PeerId::random(), PeerId::random()],
            [
                SymmetricKey::random(&mut rng),
                SymmetricKey::random(&mut rng),
                SymmetricKey::random(&mut rng),
            ],
            Duration::from_millis(1),
        );
        pool.add_circuit(expired);

        // Add active circuit
        pool.add_circuit(make_test_circuit(Duration::from_secs(600)));

        // Wait for first to expire
        std::thread::sleep(Duration::from_millis(10));

        // Run maintenance
        let result = pool.maintain();

        assert_eq!(result.removed, 1);
        assert_eq!(result.active, 1);
        assert_eq!(result.circuits_needed, 1); // Need 1 more to reach min of 2
    }

    #[test]
    fn test_metrics() {
        let metrics = CircuitPoolMetrics::new();

        assert_eq!(metrics.get_circuits_built(), 0);
        assert_eq!(metrics.get_build_failures(), 0);
        assert_eq!(metrics.get_circuits_expired(), 0);
        assert_eq!(metrics.get_maintenance_cycles(), 0);

        metrics.record_circuit_built();
        metrics.record_circuit_built();
        assert_eq!(metrics.get_circuits_built(), 2);

        metrics.record_build_failure();
        assert_eq!(metrics.get_build_failures(), 1);

        metrics.record_circuits_expired(5);
        assert_eq!(metrics.get_circuits_expired(), 5);

        metrics.record_maintenance_cycle();
        metrics.record_maintenance_cycle();
        metrics.record_maintenance_cycle();
        assert_eq!(metrics.get_maintenance_cycles(), 3);
    }

    #[tokio::test]
    async fn test_maintainer_run_maintenance() {
        let pool = new_shared_pool(CircuitPoolConfig {
            min_circuits: 2,
            ..Default::default()
        });
        let metrics = Arc::new(CircuitPoolMetrics::new());

        // Add circuits directly to pool
        {
            let mut pool_guard = pool.write().await;
            pool_guard.add_circuit(make_test_circuit(Duration::from_secs(600)));
        }

        let maintainer = CircuitPoolMaintainer::with_config(
            Arc::clone(&pool),
            Arc::clone(&metrics),
            CircuitPoolConfig::default(),
        );

        let result = maintainer.run_maintenance().await;

        assert_eq!(result.removed, 0);
        assert_eq!(result.active, 1);
        assert!(result.circuits_needed > 0);
        assert_eq!(metrics.get_maintenance_cycles(), 1);
    }

    #[tokio::test]
    async fn test_maintainer_add_circuit() {
        let pool = new_shared_pool(CircuitPoolConfig::default());
        let metrics = Arc::new(CircuitPoolMetrics::new());

        let maintainer = CircuitPoolMaintainer::with_config(
            Arc::clone(&pool),
            Arc::clone(&metrics),
            CircuitPoolConfig::default(),
        );

        // Pool starts empty
        assert_eq!(pool.read().await.active_count(), 0);

        // Add circuit through maintainer
        maintainer
            .add_circuit(make_test_circuit(Duration::from_secs(600)))
            .await;

        assert_eq!(pool.read().await.active_count(), 1);
        assert_eq!(metrics.get_circuits_built(), 1);
    }

    #[tokio::test]
    async fn test_pool_recovery_after_expiration() {
        // Test that pool can recover after all circuits expire
        let config = CircuitPoolConfig {
            min_circuits: 2,
            rebuild_threshold: Duration::from_secs(10),
            ..Default::default()
        };
        let pool = new_shared_pool(config.clone());
        let metrics = Arc::new(CircuitPoolMetrics::new());
        let mut rng = rand::thread_rng();

        // Add short-lived circuits
        {
            let mut pool_guard = pool.write().await;
            for _ in 0..2 {
                pool_guard.add_circuit(OutboundCircuit::new_exact_lifetime(
                    CircuitId::random(&mut rng),
                    [PeerId::random(), PeerId::random(), PeerId::random()],
                    [
                        SymmetricKey::random(&mut rng),
                        SymmetricKey::random(&mut rng),
                        SymmetricKey::random(&mut rng),
                    ],
                    Duration::from_millis(5),
                ));
            }
        }

        // Wait for circuits to expire
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Run maintenance - should remove expired and report need for new circuits
        let maintainer =
            CircuitPoolMaintainer::with_config(Arc::clone(&pool), Arc::clone(&metrics), config);

        let result = maintainer.run_maintenance().await;

        assert_eq!(result.removed, 2);
        assert_eq!(result.active, 0);
        assert_eq!(result.circuits_needed, 2); // Need to rebuild to min

        // Simulate building replacement circuits
        for _ in 0..result.circuits_needed {
            maintainer
                .add_circuit(make_test_circuit(Duration::from_secs(600)))
                .await;
        }

        // Pool should be recovered
        assert_eq!(pool.read().await.active_count(), 2);
        assert_eq!(metrics.get_circuits_built(), 2);
        assert_eq!(metrics.get_circuits_expired(), 2);
    }
}
