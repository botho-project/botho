// Copyright (c) 2024 Botho Foundation

//! Relay message handler for onion gossip.
//!
//! This module implements the relay-side logic for receiving, decrypting,
//! and forwarding onion messages. It integrates with the gossipsub layer
//! for broadcasting exit-hop messages.
//!
//! # Message Flow
//!
//! ```text
//! Receive OnionRelayMessage
//!     │
//!     ▼
//! Rate limit check (per source peer)
//!     │
//!     ├── Rate limited → Log warning, ignore
//!     │
//!     ▼
//! Look up circuit_id in RelayState
//!     │
//!     ├── Not found → Ignore (stale/invalid circuit)
//!     │
//!     ▼
//! Decrypt one layer
//!     │
//!     ├── Decryption fails → Log warning, ignore
//!     │
//!     ▼
//! Parse layer type
//!     │
//!     ├── Forward → Extract next_hop, return RelayAction::Forward
//!     │
//!     └── Exit → Deserialize InnerMessage, return RelayAction::Exit
//! ```
//!
//! # Security Considerations
//!
//! - Never log decrypted payload contents
//! - Rate limit to prevent relay abuse
//! - Validate transactions before broadcasting (DoS prevention)
//! - Unknown circuits are silently ignored (no error response)

use libp2p::PeerId;
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;
use tracing::warn;

use super::{decrypt_layer, rate_limit::RateLimitResult, CryptoError, DecryptedLayer, RelayState};
use bth_gossip::{InnerMessage, OnionRelayMessage};

/// Errors that can occur during relay message handling.
#[derive(Debug, Error)]
pub enum RelayHandlerError {
    /// Source peer is rate limited.
    #[error("rate limited: peer {0} exceeded relay quota")]
    RateLimited(PeerId),

    /// Circuit not found in relay state.
    #[error("unknown circuit: {0}")]
    UnknownCircuit(String),

    /// Decryption of onion layer failed.
    #[error("decryption failed: {0}")]
    DecryptionFailed(#[from] CryptoError),

    /// Failed to deserialize inner message.
    #[error("invalid inner message: {0}")]
    InvalidInnerMessage(String),

    /// Next hop peer ID is invalid.
    #[error("invalid next hop peer ID: {0}")]
    InvalidNextHop(String),
}

/// Result of handling a relay message.
#[derive(Debug)]
pub enum RelayAction {
    /// Forward the decrypted payload to the next hop.
    Forward {
        /// The peer to forward to.
        next_hop: PeerId,
        /// The forwarded onion message (circuit_id + remaining payload).
        message: OnionRelayMessage,
    },
    /// Exit hop: broadcast the inner message via gossipsub.
    Exit {
        /// The decrypted inner message.
        inner: InnerMessage,
    },
    /// Message was dropped (rate limited, unknown circuit, etc.).
    Dropped {
        /// Reason for dropping.
        reason: String,
    },
}

/// Metrics for relay traffic.
///
/// These metrics help monitor relay health and detect abuse.
#[derive(Debug, Default)]
pub struct RelayMetrics {
    /// Total messages received for relay.
    pub messages_received: AtomicU64,
    /// Messages successfully forwarded.
    pub messages_forwarded: AtomicU64,
    /// Messages successfully exited (broadcast).
    pub messages_exited: AtomicU64,
    /// Messages dropped due to rate limiting.
    pub rate_limited: AtomicU64,
    /// Messages dropped due to unknown circuit.
    pub unknown_circuits: AtomicU64,
    /// Messages dropped due to decryption failure.
    pub decryption_failures: AtomicU64,
    /// Cover traffic received (and dropped).
    pub cover_traffic_received: AtomicU64,
    /// Peers flagged for disconnection due to rate limit abuse.
    pub peers_flagged_for_disconnect: AtomicU64,
}

impl RelayMetrics {
    /// Create new relay metrics.
    pub fn new() -> Self {
        Self::default()
    }

    /// Increment received message count.
    pub fn inc_received(&self) {
        self.messages_received.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment forwarded message count.
    pub fn inc_forwarded(&self) {
        self.messages_forwarded.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment exited message count.
    pub fn inc_exited(&self) {
        self.messages_exited.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment rate limited count.
    pub fn inc_rate_limited(&self) {
        self.rate_limited.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment unknown circuit count.
    pub fn inc_unknown_circuit(&self) {
        self.unknown_circuits.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment decryption failure count.
    pub fn inc_decryption_failure(&self) {
        self.decryption_failures.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment cover traffic count.
    pub fn inc_cover_traffic(&self) {
        self.cover_traffic_received.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment peers flagged for disconnect count.
    pub fn inc_flagged_for_disconnect(&self) {
        self.peers_flagged_for_disconnect
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Get a snapshot of all metrics.
    pub fn snapshot(&self) -> RelayMetricsSnapshot {
        RelayMetricsSnapshot {
            messages_received: self.messages_received.load(Ordering::Relaxed),
            messages_forwarded: self.messages_forwarded.load(Ordering::Relaxed),
            messages_exited: self.messages_exited.load(Ordering::Relaxed),
            rate_limited: self.rate_limited.load(Ordering::Relaxed),
            unknown_circuits: self.unknown_circuits.load(Ordering::Relaxed),
            decryption_failures: self.decryption_failures.load(Ordering::Relaxed),
            cover_traffic_received: self.cover_traffic_received.load(Ordering::Relaxed),
            peers_flagged_for_disconnect: self.peers_flagged_for_disconnect.load(Ordering::Relaxed),
        }
    }
}

/// Snapshot of relay metrics for reporting.
#[derive(Debug, Clone, Default)]
pub struct RelayMetricsSnapshot {
    /// Total messages received for relay.
    pub messages_received: u64,
    /// Messages successfully forwarded.
    pub messages_forwarded: u64,
    /// Messages successfully exited (broadcast).
    pub messages_exited: u64,
    /// Messages dropped due to rate limiting.
    pub rate_limited: u64,
    /// Messages dropped due to unknown circuit.
    pub unknown_circuits: u64,
    /// Messages dropped due to decryption failure.
    pub decryption_failures: u64,
    /// Cover traffic received (and dropped).
    pub cover_traffic_received: u64,
    /// Peers flagged for disconnection due to rate limit abuse.
    pub peers_flagged_for_disconnect: u64,
}

/// Handler for relay messages.
///
/// This struct encapsulates the logic for processing onion relay messages.
/// It maintains a reference to the relay state and metrics.
///
/// # Example
///
/// ```ignore
/// use botho::network::privacy::{RelayHandler, RelayState, RelayStateConfig};
///
/// let relay_state = RelayState::new(RelayStateConfig::default());
/// let handler = RelayHandler::new();
///
/// // Handle an incoming message
/// let action = handler.handle_message(&mut relay_state, from_peer, message)?;
/// match action {
///     RelayAction::Forward { next_hop, message } => {
///         // Forward to next_hop
///     }
///     RelayAction::Exit { inner } => {
///         // Broadcast via gossipsub
///     }
///     RelayAction::Dropped { reason } => {
///         // Message was dropped
///     }
/// }
/// ```
pub struct RelayHandler {
    /// Metrics for relay operations.
    metrics: RelayMetrics,
}

impl Default for RelayHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl RelayHandler {
    /// Create a new relay handler.
    pub fn new() -> Self {
        Self {
            metrics: RelayMetrics::new(),
        }
    }

    /// Get the relay metrics.
    pub fn metrics(&self) -> &RelayMetrics {
        &self.metrics
    }

    /// Handle an incoming onion relay message.
    ///
    /// This method:
    /// 1. Checks rate limiting for the source peer (with message size)
    /// 2. Looks up the circuit key
    /// 3. Decrypts one layer
    /// 4. Returns the appropriate action (forward, exit, or drop)
    ///
    /// # Arguments
    ///
    /// * `relay_state` - The relay state containing circuit keys and rate
    ///   limits
    /// * `from` - The peer that sent this message
    /// * `msg` - The onion relay message to handle
    ///
    /// # Returns
    ///
    /// A `RelayAction` indicating what to do with the message.
    pub fn handle_message(
        &self,
        relay_state: &mut RelayState,
        from: &PeerId,
        msg: OnionRelayMessage,
    ) -> RelayAction {
        self.metrics.inc_received();

        // Step 1: Enhanced rate limiting with message size tracking
        let rate_result = relay_state.check_relay_enhanced(from, msg.payload.len());
        match rate_result {
            RateLimitResult::Allowed => {
                // Continue processing
            }
            RateLimitResult::RateLimited {
                violations,
                remaining,
            } => {
                self.metrics.inc_rate_limited();
                warn!(
                    "Rate limited relay from {} (violations: {}, remaining: {})",
                    from, violations, remaining
                );
                return RelayAction::Dropped {
                    reason: format!("rate limited: {} (violations: {})", from, violations),
                };
            }
            RateLimitResult::Disconnect => {
                self.metrics.inc_rate_limited();
                self.metrics.inc_flagged_for_disconnect();
                warn!(
                    "Peer {} exceeded violation threshold, flagged for disconnect",
                    from
                );
                return RelayAction::Dropped {
                    reason: format!("disconnect: {} exceeded violation threshold", from),
                };
            }
        }

        // Step 2: Look up circuit
        let circuit_id_display = hex::encode(msg.circuit_id.as_bytes());
        let hop_key = match relay_state.get_circuit_key(
            &crate::network::privacy::CircuitId::from_bytes(msg.circuit_id.as_ref()).unwrap(),
        ) {
            Some(key) => key,
            None => {
                self.metrics.inc_unknown_circuit();
                // Silently ignore unknown circuits (don't log to avoid info leakage)
                return RelayAction::Dropped {
                    reason: format!("unknown circuit: {}", circuit_id_display),
                };
            }
        };

        // Step 3: Decrypt one layer
        let decrypted = match decrypt_layer(hop_key.key(), &msg.payload) {
            Ok(d) => d,
            Err(e) => {
                self.metrics.inc_decryption_failure();
                warn!("Decrypt failed for circuit {}: {}", circuit_id_display, e);
                return RelayAction::Dropped {
                    reason: format!("decryption failed: {}", e),
                };
            }
        };

        // Step 4: Handle based on layer type
        match decrypted {
            DecryptedLayer::Forward { next_hop, inner } => {
                self.metrics.inc_forwarded();
                let forwarded = OnionRelayMessage {
                    circuit_id: msg.circuit_id,
                    payload: inner,
                };
                RelayAction::Forward {
                    next_hop,
                    message: forwarded,
                }
            }
            DecryptedLayer::Exit { payload } => {
                // Deserialize the inner message
                match bth_util_serial::deserialize::<InnerMessage>(&payload) {
                    Ok(inner) => {
                        // Handle cover traffic specially
                        if matches!(inner, InnerMessage::Cover) {
                            self.metrics.inc_cover_traffic();
                            return RelayAction::Dropped {
                                reason: "cover traffic".to_string(),
                            };
                        }

                        self.metrics.inc_exited();
                        RelayAction::Exit { inner }
                    }
                    Err(e) => {
                        self.metrics.inc_decryption_failure();
                        warn!("Failed to deserialize inner message: {}", e);
                        RelayAction::Dropped {
                            reason: format!("invalid inner message: {}", e),
                        }
                    }
                }
            }
        }
    }

    /// Check if a transaction should be broadcast.
    ///
    /// This performs basic validation before allowing the transaction
    /// to be broadcast via gossipsub. The actual transaction validation
    /// happens in the mempool, but we do preliminary checks here.
    ///
    /// # Arguments
    ///
    /// * `tx_data` - Serialized transaction data
    /// * `tx_hash` - Expected transaction hash
    ///
    /// # Returns
    ///
    /// `true` if the transaction should be broadcast, `false` otherwise.
    pub fn should_broadcast_transaction(tx_data: &[u8], tx_hash: &[u8; 32]) -> bool {
        // Basic size check
        if tx_data.is_empty() || tx_data.len() > 1_000_000 {
            warn!("Transaction size out of bounds: {} bytes", tx_data.len());
            return false;
        }

        // Verify hash matches (prevents malformed tx_hash)
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(tx_data);
        let computed_hash = hasher.finalize();
        if computed_hash.as_slice() != tx_hash {
            warn!("Transaction hash mismatch");
            return false;
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::privacy::{
        encrypt_exit_layer, encrypt_forward_layer, CircuitHopKey, CircuitId, RelayStateConfig,
        SymmetricKey,
    };

    fn random_symmetric_key() -> SymmetricKey {
        SymmetricKey::random(&mut rand::thread_rng())
    }

    fn random_circuit_id() -> CircuitId {
        CircuitId::random(&mut rand::thread_rng())
    }

    fn to_gossip_circuit_id(id: &CircuitId) -> bth_gossip::CircuitId {
        bth_gossip::CircuitId(*id.as_bytes())
    }

    #[test]
    fn test_relay_handler_forward() {
        let mut relay_state = RelayState::new(RelayStateConfig::default());
        let handler = RelayHandler::new();

        // Set up circuit
        let circuit_id = random_circuit_id();
        let key = random_symmetric_key();
        let next_hop = PeerId::random();
        let hop_key = CircuitHopKey::new_forward(key.duplicate(), next_hop.clone());
        relay_state.add_circuit_key(circuit_id, hop_key);

        // Create forward layer
        let inner_data = b"inner payload";
        let encrypted = encrypt_forward_layer(&key, &next_hop, inner_data);

        let msg = OnionRelayMessage {
            circuit_id: to_gossip_circuit_id(&circuit_id),
            payload: encrypted,
        };

        let from = PeerId::random();
        let action = handler.handle_message(&mut relay_state, &from, msg);

        match action {
            RelayAction::Forward {
                next_hop: nh,
                message,
            } => {
                assert_eq!(nh, next_hop);
                assert_eq!(message.payload, inner_data);
            }
            _ => panic!("Expected Forward action"),
        }

        // Check metrics
        let snapshot = handler.metrics().snapshot();
        assert_eq!(snapshot.messages_received, 1);
        assert_eq!(snapshot.messages_forwarded, 1);
    }

    #[test]
    fn test_relay_handler_exit_transaction() {
        let mut relay_state = RelayState::new(RelayStateConfig::default());
        let handler = RelayHandler::new();

        // Set up circuit (exit hop)
        let circuit_id = random_circuit_id();
        let key = random_symmetric_key();
        let hop_key = CircuitHopKey::new_exit(key.duplicate());
        relay_state.add_circuit_key(circuit_id, hop_key);

        // Create inner message
        let tx_data = b"transaction data".to_vec();
        let tx_hash = [42u8; 32];
        let inner = InnerMessage::Transaction {
            tx_data: tx_data.clone(),
            tx_hash,
        };
        let inner_bytes = bth_util_serial::serialize(&inner).unwrap();

        // Create exit layer
        let encrypted = encrypt_exit_layer(&key, &inner_bytes);

        let msg = OnionRelayMessage {
            circuit_id: to_gossip_circuit_id(&circuit_id),
            payload: encrypted,
        };

        let from = PeerId::random();
        let action = handler.handle_message(&mut relay_state, &from, msg);

        match action {
            RelayAction::Exit { inner } => match inner {
                InnerMessage::Transaction {
                    tx_data: td,
                    tx_hash: th,
                } => {
                    assert_eq!(td, tx_data);
                    assert_eq!(th, tx_hash);
                }
                _ => panic!("Expected Transaction inner message"),
            },
            _ => panic!("Expected Exit action"),
        }

        // Check metrics
        let snapshot = handler.metrics().snapshot();
        assert_eq!(snapshot.messages_received, 1);
        assert_eq!(snapshot.messages_exited, 1);
    }

    #[test]
    fn test_relay_handler_cover_traffic() {
        let mut relay_state = RelayState::new(RelayStateConfig::default());
        let handler = RelayHandler::new();

        // Set up circuit (exit hop)
        let circuit_id = random_circuit_id();
        let key = random_symmetric_key();
        let hop_key = CircuitHopKey::new_exit(key.duplicate());
        relay_state.add_circuit_key(circuit_id, hop_key);

        // Create cover traffic inner message
        let inner = InnerMessage::Cover;
        let inner_bytes = bth_util_serial::serialize(&inner).unwrap();

        // Create exit layer
        let encrypted = encrypt_exit_layer(&key, &inner_bytes);

        let msg = OnionRelayMessage {
            circuit_id: to_gossip_circuit_id(&circuit_id),
            payload: encrypted,
        };

        let from = PeerId::random();
        let action = handler.handle_message(&mut relay_state, &from, msg);

        // Cover traffic should be dropped
        match action {
            RelayAction::Dropped { reason } => {
                assert!(reason.contains("cover"));
            }
            _ => panic!("Expected Dropped action for cover traffic"),
        }

        // Check metrics
        let snapshot = handler.metrics().snapshot();
        assert_eq!(snapshot.cover_traffic_received, 1);
    }

    #[test]
    fn test_relay_handler_unknown_circuit() {
        let mut relay_state = RelayState::new(RelayStateConfig::default());
        let handler = RelayHandler::new();

        // Don't add any circuit keys

        let msg = OnionRelayMessage {
            circuit_id: bth_gossip::CircuitId::random(),
            payload: vec![1, 2, 3, 4],
        };

        let from = PeerId::random();
        let action = handler.handle_message(&mut relay_state, &from, msg);

        match action {
            RelayAction::Dropped { reason } => {
                assert!(reason.contains("unknown circuit"));
            }
            _ => panic!("Expected Dropped action for unknown circuit"),
        }

        // Check metrics
        let snapshot = handler.metrics().snapshot();
        assert_eq!(snapshot.unknown_circuits, 1);
    }

    #[test]
    fn test_relay_handler_rate_limited() {
        use crate::network::privacy::rate_limit::RelayRateLimits;

        // Configure with very low relay message limit (1 msg/sec, capacity 2)
        let config = RelayStateConfig {
            max_relay_per_window: 1,
            rate_limits: RelayRateLimits {
                relay_msgs_per_sec: 1, // Very low limit, capacity = 2
                relay_bandwidth_per_peer: 10_000,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut relay_state = RelayState::new(config);
        let handler = RelayHandler::new();

        // Set up circuit
        let circuit_id = random_circuit_id();
        let key = random_symmetric_key();
        let hop_key = CircuitHopKey::new_exit(key.duplicate());
        relay_state.add_circuit_key(circuit_id, hop_key);

        let inner = InnerMessage::Cover;
        let inner_bytes = bth_util_serial::serialize(&inner).unwrap();
        let encrypted = encrypt_exit_layer(&key, &inner_bytes);

        let msg = OnionRelayMessage {
            circuit_id: to_gossip_circuit_id(&circuit_id),
            payload: encrypted.clone(),
        };

        let from = PeerId::random();

        // First and second messages should succeed (capacity is 2)
        let _ = handler.handle_message(&mut relay_state, &from, msg.clone());
        let _ = handler.handle_message(&mut relay_state, &from, msg.clone());

        // Third message from same peer should be rate limited
        let action = handler.handle_message(&mut relay_state, &from, msg);

        match action {
            RelayAction::Dropped { reason } => {
                assert!(reason.contains("rate limited"));
            }
            _ => panic!("Expected Dropped action for rate limited"),
        }

        // Check metrics
        let snapshot = handler.metrics().snapshot();
        assert!(snapshot.rate_limited >= 1);
    }

    #[test]
    fn test_relay_handler_decryption_failure() {
        let mut relay_state = RelayState::new(RelayStateConfig::default());
        let handler = RelayHandler::new();

        // Set up circuit
        let circuit_id = random_circuit_id();
        let key = random_symmetric_key();
        let hop_key = CircuitHopKey::new_exit(key.duplicate());
        relay_state.add_circuit_key(circuit_id, hop_key);

        // Create message with garbage payload (wrong key or tampered)
        let msg = OnionRelayMessage {
            circuit_id: to_gossip_circuit_id(&circuit_id),
            payload: vec![0u8; 100], // Invalid encrypted data
        };

        let from = PeerId::random();
        let action = handler.handle_message(&mut relay_state, &from, msg);

        match action {
            RelayAction::Dropped { reason } => {
                assert!(reason.contains("decryption failed"));
            }
            _ => panic!("Expected Dropped action for decryption failure"),
        }

        // Check metrics
        let snapshot = handler.metrics().snapshot();
        assert_eq!(snapshot.decryption_failures, 1);
    }

    #[test]
    fn test_should_broadcast_transaction_valid() {
        let tx_data = b"test transaction data";

        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(tx_data);
        let hash = hasher.finalize();
        let mut tx_hash = [0u8; 32];
        tx_hash.copy_from_slice(&hash);

        assert!(RelayHandler::should_broadcast_transaction(
            tx_data, &tx_hash
        ));
    }

    #[test]
    fn test_should_broadcast_transaction_hash_mismatch() {
        let tx_data = b"test transaction data";
        let wrong_hash = [0u8; 32]; // Wrong hash

        assert!(!RelayHandler::should_broadcast_transaction(
            tx_data,
            &wrong_hash
        ));
    }

    #[test]
    fn test_should_broadcast_transaction_empty() {
        let tx_data = b"";
        let tx_hash = [0u8; 32];

        assert!(!RelayHandler::should_broadcast_transaction(
            tx_data, &tx_hash
        ));
    }

    #[test]
    fn test_metrics_snapshot() {
        let metrics = RelayMetrics::new();

        metrics.inc_received();
        metrics.inc_received();
        metrics.inc_forwarded();
        metrics.inc_exited();
        metrics.inc_rate_limited();
        metrics.inc_unknown_circuit();
        metrics.inc_decryption_failure();
        metrics.inc_cover_traffic();
        metrics.inc_flagged_for_disconnect();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.messages_received, 2);
        assert_eq!(snapshot.messages_forwarded, 1);
        assert_eq!(snapshot.messages_exited, 1);
        assert_eq!(snapshot.rate_limited, 1);
        assert_eq!(snapshot.unknown_circuits, 1);
        assert_eq!(snapshot.decryption_failures, 1);
        assert_eq!(snapshot.cover_traffic_received, 1);
        assert_eq!(snapshot.peers_flagged_for_disconnect, 1);
    }

    #[test]
    fn test_relay_handler_abusive_peer_disconnect() {
        use crate::network::privacy::rate_limit::RelayRateLimits;

        // Configure with very low limits and low violation threshold
        let config = RelayStateConfig {
            rate_limits: RelayRateLimits {
                relay_msgs_per_sec: 1, // Capacity = 2
                relay_bandwidth_per_peer: 10_000,
                violation_threshold: 2, // Disconnect after 2 violations
                ..Default::default()
            },
            ..Default::default()
        };
        let mut relay_state = RelayState::new(config);
        let handler = RelayHandler::new();

        // Set up circuit
        let circuit_id = random_circuit_id();
        let key = random_symmetric_key();
        let hop_key = CircuitHopKey::new_exit(key.duplicate());
        relay_state.add_circuit_key(circuit_id, hop_key);

        let inner = InnerMessage::Cover;
        let inner_bytes = bth_util_serial::serialize(&inner).unwrap();
        let encrypted = encrypt_exit_layer(&key, &inner_bytes);

        let msg = OnionRelayMessage {
            circuit_id: to_gossip_circuit_id(&circuit_id),
            payload: encrypted.clone(),
        };

        let from = PeerId::random();

        // Use up token bucket capacity (2 tokens)
        handler.handle_message(&mut relay_state, &from, msg.clone());
        handler.handle_message(&mut relay_state, &from, msg.clone());

        // Next messages trigger violations
        handler.handle_message(&mut relay_state, &from, msg.clone()); // violation 1
        let action = handler.handle_message(&mut relay_state, &from, msg.clone()); // violation 2 -> disconnect

        // Should be flagged for disconnect
        match action {
            RelayAction::Dropped { reason } => {
                assert!(
                    reason.contains("disconnect") || reason.contains("rate limited"),
                    "Expected disconnect or rate limited, got: {}",
                    reason
                );
            }
            _ => panic!("Expected Dropped action for abusive peer"),
        }

        // Check that peer was flagged for disconnect
        let flagged = relay_state.take_flagged_peers();
        assert!(
            !flagged.is_empty() || handler.metrics().snapshot().peers_flagged_for_disconnect >= 1,
            "Peer should be flagged for disconnect"
        );
    }
}
