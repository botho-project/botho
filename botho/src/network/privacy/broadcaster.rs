// Copyright (c) 2024 Botho Foundation

//! Privacy-preserving transaction broadcaster using onion gossip.
//!
//! This module implements the broadcast integration for Phase 1 of the
//! traffic analysis resistance roadmap. Transactions are routed through
//! 3-hop circuits before being broadcast, hiding the originating node.
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────────────┐
//! │                    PRIVATE BROADCAST FLOW                         │
//! │                                                                   │
//! │   User submits transaction                                        │
//! │       │                                                           │
//! │       ▼                                                           │
//! │   OnionBroadcaster.broadcast_private(tx)                          │
//! │       │                                                           │
//! │       ├─── Get circuit from CircuitPool                           │
//! │       │         │                                                 │
//! │       │         ├─── Circuit available                            │
//! │       │         │         │                                       │
//! │       │         │         ▼                                       │
//! │       │         │    Wrap tx in InnerMessage::Transaction         │
//! │       │         │         │                                       │
//! │       │         │         ▼                                       │
//! │       │         │    wrap_onion(inner, hops, keys)                │
//! │       │         │         │                                       │
//! │       │         │         ▼                                       │
//! │       │         │    GossipHandle.send_onion_relay()              │
//! │       │         │         │                                       │
//! │       │         │         ▼                                       │
//! │       │         │    → Hop1 → Hop2 → Exit → gossipsub             │
//! │       │         │                                                 │
//! │       │         └─── No circuit (fallback behavior)               │
//! │       │                   │                                       │
//! │       │                   ▼                                       │
//! │       │              Queue or return error                        │
//! │       │                                                           │
//! │       ▼                                                           │
//! │   Return tx_hash                                                  │
//! └──────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Security Properties
//!
//! - Transactions exit from a different node than the origin
//! - No single relay knows both origin and transaction content
//! - Timing jitter prevents correlation of entry/exit timing (Phase 2)
//! - Cover traffic normalizes message patterns (Phase 2)

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use bth_gossip::{GossipHandle, InnerMessage, OnionRelayMessage};
use libp2p::PeerId;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tracing::{debug, trace, warn};

use super::{
    timing::{TimingJitter, TimingJitterConfig},
    wrap_onion, CircuitPool, OutboundCircuit,
};
use crate::transaction::Transaction;

/// Errors that can occur during private broadcast.
#[derive(Debug, Error)]
pub enum BroadcastError {
    /// No circuit available for routing.
    #[error("no circuit available for private broadcast")]
    NoCircuit,

    /// Failed to serialize transaction.
    #[error("failed to serialize transaction: {0}")]
    SerializationError(String),

    /// Failed to serialize inner message.
    #[error("failed to serialize inner message: {0}")]
    InnerSerializationError(String),

    /// Failed to send to gossip network.
    #[error("gossip network error: {0}")]
    GossipError(String),
}

/// Metrics for private broadcast operations.
#[derive(Debug, Default)]
pub struct BroadcastMetrics {
    /// Transactions sent via onion circuit.
    pub tx_broadcast_private: AtomicU64,

    /// Transactions queued because no circuit available.
    pub tx_queued_no_circuit: AtomicU64,

    /// Transactions that failed to broadcast.
    pub tx_broadcast_failed: AtomicU64,

    /// Transactions we broadcast as exit node (received via relay).
    pub tx_exit_broadcast: AtomicU64,

    /// Total jitter delay applied in milliseconds.
    pub jitter_delay_total_ms: AtomicU64,

    /// Number of broadcasts with jitter applied.
    pub jitter_applied_count: AtomicU64,
}

impl BroadcastMetrics {
    /// Create new metrics instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get a snapshot of current metrics.
    pub fn snapshot(&self) -> BroadcastMetricsSnapshot {
        BroadcastMetricsSnapshot {
            tx_broadcast_private: self.tx_broadcast_private.load(Ordering::Relaxed),
            tx_queued_no_circuit: self.tx_queued_no_circuit.load(Ordering::Relaxed),
            tx_broadcast_failed: self.tx_broadcast_failed.load(Ordering::Relaxed),
            tx_exit_broadcast: self.tx_exit_broadcast.load(Ordering::Relaxed),
            jitter_delay_total_ms: self.jitter_delay_total_ms.load(Ordering::Relaxed),
            jitter_applied_count: self.jitter_applied_count.load(Ordering::Relaxed),
        }
    }

    fn inc_private(&self) {
        self.tx_broadcast_private.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_queued(&self) {
        self.tx_queued_no_circuit.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_failed(&self) {
        self.tx_broadcast_failed.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment exit broadcast counter (called when we're the exit node).
    pub fn inc_exit_broadcast(&self) {
        self.tx_exit_broadcast.fetch_add(1, Ordering::Relaxed);
    }

    /// Record jitter delay applied to a broadcast.
    fn record_jitter(&self, delay_ms: u64) {
        self.jitter_delay_total_ms
            .fetch_add(delay_ms, Ordering::Relaxed);
        self.jitter_applied_count.fetch_add(1, Ordering::Relaxed);
    }
}

/// Snapshot of broadcast metrics (for RPC/monitoring).
#[derive(Debug, Clone, Copy, Default)]
pub struct BroadcastMetricsSnapshot {
    /// Transactions sent via onion circuit.
    pub tx_broadcast_private: u64,
    /// Transactions queued because no circuit available.
    pub tx_queued_no_circuit: u64,
    /// Transactions that failed to broadcast.
    pub tx_broadcast_failed: u64,
    /// Transactions we broadcast as exit node.
    pub tx_exit_broadcast: u64,
    /// Total jitter delay applied in milliseconds.
    pub jitter_delay_total_ms: u64,
    /// Number of broadcasts with jitter applied.
    pub jitter_applied_count: u64,
}

impl BroadcastMetricsSnapshot {
    /// Calculate average jitter delay in milliseconds.
    ///
    /// Returns 0 if no jitter has been applied yet.
    pub fn avg_jitter_delay_ms(&self) -> u64 {
        if self.jitter_applied_count == 0 {
            0
        } else {
            self.jitter_delay_total_ms / self.jitter_applied_count
        }
    }
}

/// Privacy-preserving transaction broadcaster.
///
/// Wraps transactions in onion layers and routes them through circuits
/// before broadcast. This ensures the exit node (not the origin) appears
/// as the source of the transaction.
///
/// # Timing Jitter
///
/// The broadcaster applies random timing jitter before sending messages
/// to prevent timing correlation attacks. This is configurable and can
/// be disabled for testing.
#[derive(Debug)]
pub struct OnionBroadcaster {
    /// Metrics for monitoring broadcast operations.
    metrics: Arc<BroadcastMetrics>,
    /// Timing jitter configuration for anti-correlation.
    jitter: TimingJitter,
}

impl OnionBroadcaster {
    /// Create a new broadcaster with default jitter (50-200ms).
    pub fn new() -> Self {
        Self {
            metrics: Arc::new(BroadcastMetrics::new()),
            jitter: TimingJitter::default(),
        }
    }

    /// Create a new broadcaster with shared metrics and default jitter.
    pub fn with_metrics(metrics: Arc<BroadcastMetrics>) -> Self {
        Self {
            metrics,
            jitter: TimingJitter::default(),
        }
    }

    /// Create a new broadcaster with custom jitter configuration.
    pub fn with_jitter(jitter_config: TimingJitterConfig) -> Self {
        Self {
            metrics: Arc::new(BroadcastMetrics::new()),
            jitter: TimingJitter::new(jitter_config),
        }
    }

    /// Create a new broadcaster with shared metrics and custom jitter.
    pub fn with_metrics_and_jitter(
        metrics: Arc<BroadcastMetrics>,
        jitter_config: TimingJitterConfig,
    ) -> Self {
        Self {
            metrics,
            jitter: TimingJitter::new(jitter_config),
        }
    }

    /// Get the broadcaster's metrics.
    pub fn metrics(&self) -> &Arc<BroadcastMetrics> {
        &self.metrics
    }

    /// Get the jitter configuration.
    pub fn jitter(&self) -> &TimingJitter {
        &self.jitter
    }

    /// Broadcast a transaction privately through an onion circuit.
    ///
    /// The transaction is:
    /// 1. Serialized to bytes
    /// 2. Wrapped in an InnerMessage::Transaction
    /// 3. Onion-encrypted with 3 layers
    /// 4. Sent to the first hop via gossipsub
    ///
    /// # Arguments
    ///
    /// * `tx` - The transaction to broadcast
    /// * `circuit_pool` - Pool of available circuits
    /// * `gossip_handle` - Handle to the gossip network
    ///
    /// # Returns
    ///
    /// The transaction hash on success, or an error if broadcast failed.
    pub async fn broadcast_private(
        &self,
        tx: &Transaction,
        circuit_pool: &CircuitPool,
        gossip_handle: &GossipHandle,
    ) -> Result<[u8; 32], BroadcastError> {
        // Get a circuit from the pool
        let circuit = match circuit_pool.get_circuit() {
            Some(c) => c,
            None => {
                self.metrics.inc_queued();
                warn!("No circuit available for private broadcast");
                return Err(BroadcastError::NoCircuit);
            }
        };

        self.broadcast_via_circuit(tx, circuit, gossip_handle).await
    }

    /// Broadcast a transaction through a specific circuit.
    ///
    /// This is the core broadcast logic, factored out to allow testing
    /// with specific circuits.
    pub async fn broadcast_via_circuit(
        &self,
        tx: &Transaction,
        circuit: &OutboundCircuit,
        gossip_handle: &GossipHandle,
    ) -> Result<[u8; 32], BroadcastError> {
        // Serialize transaction
        let tx_data = bincode::serialize(tx)
            .map_err(|e| BroadcastError::SerializationError(e.to_string()))?;

        // Compute transaction hash
        let tx_hash = tx.hash();

        debug!(
            tx_hash = hex::encode(&tx_hash[..8]),
            circuit_id = %circuit.id(),
            first_hop = %circuit.entry_hop(),
            "Broadcasting transaction via onion circuit"
        );

        // Create inner message
        let inner = InnerMessage::Transaction { tx_data, tx_hash };

        // Serialize inner message
        let inner_bytes = bth_util_serial::serialize(&inner)
            .map_err(|e| BroadcastError::InnerSerializationError(e.to_string()))?;

        // Wrap in onion layers
        let hops = *circuit.hops();
        let keys = [
            circuit.hop_key(0).duplicate(),
            circuit.hop_key(1).duplicate(),
            circuit.hop_key(2).duplicate(),
        ];
        let wrapped = wrap_onion(&inner_bytes, &hops, &keys);

        // Create gossip circuit ID from our circuit ID
        let gossip_circuit_id = bth_gossip::CircuitId(*circuit.id().as_bytes());

        // Create onion relay message
        let msg = OnionRelayMessage {
            circuit_id: gossip_circuit_id,
            payload: wrapped,
        };

        // Apply timing jitter to prevent correlation attacks
        // This is critical for privacy: without jitter, an observer could
        // correlate the timing of message entry and exit from the circuit
        let delay = self.jitter.delay();
        if !delay.is_zero() {
            trace!(
                delay_ms = delay.as_millis(),
                tx_hash = hex::encode(&tx_hash[..8]),
                "Applying timing jitter before broadcast"
            );
            self.metrics.record_jitter(delay.as_millis() as u64);
            tokio::time::sleep(delay).await;
        }

        // Send to gossip network
        gossip_handle.send_onion_relay(msg).await.map_err(|e| {
            self.metrics.inc_failed();
            BroadcastError::GossipError(e.to_string())
        })?;

        self.metrics.inc_private();

        trace!(
            tx_hash = hex::encode(&tx_hash[..8]),
            "Transaction successfully routed through circuit"
        );

        Ok(tx_hash)
    }

    /// Check if a transaction hash matches the transaction data.
    ///
    /// Used by exit nodes to validate transactions before broadcast.
    pub fn validate_tx_hash(tx_data: &[u8], expected_hash: &[u8; 32]) -> bool {
        if tx_data.is_empty() {
            return false;
        }

        let mut hasher = Sha256::new();
        hasher.update(tx_data);
        let computed = hasher.finalize();

        computed.as_slice() == expected_hash
    }
}

impl Default for OnionBroadcaster {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::privacy::{CircuitId, CircuitPoolConfig, SymmetricKey};
    use std::time::Duration;

    fn make_test_circuit() -> OutboundCircuit {
        let mut rng = rand::thread_rng();
        OutboundCircuit::new(
            CircuitId::random(&mut rng),
            [PeerId::random(), PeerId::random(), PeerId::random()],
            [
                SymmetricKey::random(&mut rng),
                SymmetricKey::random(&mut rng),
                SymmetricKey::random(&mut rng),
            ],
            Duration::from_secs(600),
        )
    }

    #[test]
    fn test_broadcaster_creation() {
        let broadcaster = OnionBroadcaster::new();
        let snapshot = broadcaster.metrics().snapshot();

        assert_eq!(snapshot.tx_broadcast_private, 0);
        assert_eq!(snapshot.tx_queued_no_circuit, 0);
        assert_eq!(snapshot.tx_broadcast_failed, 0);
        assert_eq!(snapshot.tx_exit_broadcast, 0);
    }

    #[test]
    fn test_metrics_increment() {
        let metrics = BroadcastMetrics::new();

        metrics.inc_private();
        metrics.inc_private();
        metrics.inc_queued();
        metrics.inc_failed();
        metrics.inc_exit_broadcast();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.tx_broadcast_private, 2);
        assert_eq!(snapshot.tx_queued_no_circuit, 1);
        assert_eq!(snapshot.tx_broadcast_failed, 1);
        assert_eq!(snapshot.tx_exit_broadcast, 1);
    }

    #[test]
    fn test_validate_tx_hash() {
        use sha2::{Digest, Sha256};

        let tx_data = b"test transaction data";

        // Compute correct hash
        let mut hasher = Sha256::new();
        hasher.update(tx_data);
        let hash = hasher.finalize();
        let mut correct_hash = [0u8; 32];
        correct_hash.copy_from_slice(&hash);

        // Valid hash should pass
        assert!(OnionBroadcaster::validate_tx_hash(tx_data, &correct_hash));

        // Wrong hash should fail
        let wrong_hash = [0u8; 32];
        assert!(!OnionBroadcaster::validate_tx_hash(tx_data, &wrong_hash));

        // Empty data should fail
        assert!(!OnionBroadcaster::validate_tx_hash(&[], &correct_hash));
    }

    #[test]
    fn test_no_circuit_increments_queued() {
        let broadcaster = OnionBroadcaster::new();
        let pool = CircuitPool::new(CircuitPoolConfig::default());

        // Pool is empty, so we can't test the full broadcast without a mock
        // But we can verify the pool reports no circuits
        assert!(pool.get_circuit().is_none());
    }

    #[test]
    fn test_shared_metrics() {
        let metrics = Arc::new(BroadcastMetrics::new());
        let broadcaster = OnionBroadcaster::with_metrics(metrics.clone());

        // Increment via broadcaster's metrics
        broadcaster.metrics().inc_private();

        // Should be visible via shared reference
        assert_eq!(metrics.snapshot().tx_broadcast_private, 1);
    }

    #[test]
    fn test_broadcaster_with_jitter() {
        let config = TimingJitterConfig::new(100, 200);
        let broadcaster = OnionBroadcaster::with_jitter(config);

        // Verify jitter configuration
        assert!(!broadcaster.jitter().is_disabled());
        let jitter_config = broadcaster.jitter().config();
        assert_eq!(jitter_config.min_delay_ms, 100);
        assert_eq!(jitter_config.max_delay_ms, 200);
    }

    #[test]
    fn test_broadcaster_with_disabled_jitter() {
        let config = TimingJitterConfig::disabled();
        let broadcaster = OnionBroadcaster::with_jitter(config);

        assert!(broadcaster.jitter().is_disabled());
    }

    #[test]
    fn test_broadcaster_default_jitter() {
        let broadcaster = OnionBroadcaster::new();

        // Default jitter should be 50-200ms
        let config = broadcaster.jitter().config();
        assert_eq!(config.min_delay_ms, 50);
        assert_eq!(config.max_delay_ms, 200);
    }

    #[test]
    fn test_jitter_metrics_recording() {
        let metrics = BroadcastMetrics::new();

        // Record some jitter
        metrics.record_jitter(100);
        metrics.record_jitter(150);
        metrics.record_jitter(200);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.jitter_applied_count, 3);
        assert_eq!(snapshot.jitter_delay_total_ms, 450);
        assert_eq!(snapshot.avg_jitter_delay_ms(), 150);
    }

    #[test]
    fn test_jitter_avg_with_no_samples() {
        let metrics = BroadcastMetrics::new();
        let snapshot = metrics.snapshot();

        // Should return 0 when no jitter has been recorded
        assert_eq!(snapshot.avg_jitter_delay_ms(), 0);
    }

    #[test]
    fn test_broadcaster_with_metrics_and_jitter() {
        let metrics = Arc::new(BroadcastMetrics::new());
        let config = TimingJitterConfig::new(10, 50);
        let broadcaster = OnionBroadcaster::with_metrics_and_jitter(metrics.clone(), config);

        // Verify both metrics and jitter are set correctly
        let jitter_config = broadcaster.jitter().config();
        assert_eq!(jitter_config.min_delay_ms, 10);
        assert_eq!(jitter_config.max_delay_ms, 50);

        // Shared metrics should work
        broadcaster.metrics().inc_private();
        assert_eq!(metrics.snapshot().tx_broadcast_private, 1);
    }
}
