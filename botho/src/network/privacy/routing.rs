// Copyright (c) 2024 Botho Foundation

//! Dual-path routing for privacy-sensitive message broadcast.
//!
//! This module implements path selection for network messages:
//!
//! - **Fast Path**: Direct gossipsub for latency-critical consensus messages
//! - **Private Path**: Onion gossip for privacy-sensitive transactions
//!
//! # Rationale
//!
//! ## Why SCP uses Fast Path
//!
//! SCP consensus messages don't reveal transaction origin:
//! - `ScpNominate`: Contains transaction hashes, not origins
//! - `ScpStatement`: Contains ballot info, not user identity
//! - Block headers/bodies: Public information
//!
//! The sender of an SCP message is a validator, not necessarily the
//! transaction originator.
//!
//! ## Why Transactions use Private Path
//!
//! Broadcasting a transaction reveals:
//! - The broadcaster's IP (likely the sender or their wallet)
//! - Timing of transaction creation
//! - Activity patterns
//!
//! # Configuration
//!
//! Users can override path selection:
//! - `force_private`: Route all messages through onion circuits
//! - `allow_fallback`: Allow fast path fallback when no circuits available
//!
//! # Example
//!
//! ```
//! use botho::network::privacy::routing::{
//!     MessageType, MessagePath, PrivacyRouter, PrivacyRoutingConfig,
//! };
//!
//! let config = PrivacyRoutingConfig::default();
//! let router = PrivacyRouter::new(config);
//!
//! // Transactions go private
//! assert_eq!(router.select_path(MessageType::Transaction), MessagePath::Private);
//!
//! // SCP messages go fast
//! assert_eq!(router.select_path(MessageType::ScpStatement), MessagePath::Fast);
//! ```

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use serde::{Deserialize, Serialize};

/// The routing path for a message.
///
/// Determines whether a message is sent via direct gossipsub (fast)
/// or through an onion circuit (private).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MessagePath {
    /// Direct gossipsub broadcast.
    ///
    /// Used for latency-critical consensus messages where privacy
    /// is not a concern (SCP statements, blocks, etc.).
    ///
    /// Latency overhead: ~0ms
    Fast,

    /// Onion gossip broadcast through 3-hop circuit.
    ///
    /// Used for privacy-sensitive messages where origin hiding
    /// is important (transactions, sync requests, etc.).
    ///
    /// Latency overhead: ~100-200ms
    Private,
}

impl std::fmt::Display for MessagePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessagePath::Fast => write!(f, "fast"),
            MessagePath::Private => write!(f, "private"),
        }
    }
}

/// Types of messages that can be routed through the network.
///
/// Each message type has a default routing path based on its
/// privacy requirements and latency sensitivity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MessageType {
    // ========================================
    // FAST PATH: Time-critical, doesn't reveal tx origin
    // ========================================
    /// SCP nomination message (contains tx hashes, not origins).
    ScpNominate,

    /// SCP statement message (ballot info, not user identity).
    ScpStatement,

    /// Block header broadcast.
    BlockHeader,

    /// Block body broadcast.
    BlockBody,

    /// Peer announcement message.
    PeerAnnouncement,

    /// Peer exchange message.
    PexMessage,

    // ========================================
    // PRIVATE PATH: Reveals sender activity
    // ========================================
    /// Transaction broadcast (reveals broadcaster's IP).
    Transaction,

    /// Chain sync request (reveals interest in specific blocks).
    SyncRequest,

    /// Wallet query (reveals account interest).
    WalletQuery,
}

impl MessageType {
    /// Get the default routing path for this message type.
    ///
    /// This is the path used when no configuration overrides are set.
    pub fn default_path(&self) -> MessagePath {
        match self {
            // Fast path: consensus and infrastructure messages
            MessageType::ScpNominate => MessagePath::Fast,
            MessageType::ScpStatement => MessagePath::Fast,
            MessageType::BlockHeader => MessagePath::Fast,
            MessageType::BlockBody => MessagePath::Fast,
            MessageType::PeerAnnouncement => MessagePath::Fast,
            MessageType::PexMessage => MessagePath::Fast,

            // Private path: user activity messages
            MessageType::Transaction => MessagePath::Private,
            MessageType::SyncRequest => MessagePath::Private,
            MessageType::WalletQuery => MessagePath::Private,
        }
    }

    /// Check if this message type is latency-sensitive.
    ///
    /// Latency-sensitive messages should use the fast path when possible.
    pub fn is_latency_sensitive(&self) -> bool {
        matches!(
            self,
            MessageType::ScpNominate
                | MessageType::ScpStatement
                | MessageType::BlockHeader
                | MessageType::BlockBody
        )
    }

    /// Check if this message type reveals user activity.
    ///
    /// Messages that reveal user activity should use the private path
    /// to hide the origin.
    pub fn reveals_user_activity(&self) -> bool {
        matches!(
            self,
            MessageType::Transaction | MessageType::SyncRequest | MessageType::WalletQuery
        )
    }
}

impl std::fmt::Display for MessageType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessageType::ScpNominate => write!(f, "scp_nominate"),
            MessageType::ScpStatement => write!(f, "scp_statement"),
            MessageType::BlockHeader => write!(f, "block_header"),
            MessageType::BlockBody => write!(f, "block_body"),
            MessageType::PeerAnnouncement => write!(f, "peer_announcement"),
            MessageType::PexMessage => write!(f, "pex_message"),
            MessageType::Transaction => write!(f, "transaction"),
            MessageType::SyncRequest => write!(f, "sync_request"),
            MessageType::WalletQuery => write!(f, "wallet_query"),
        }
    }
}

/// Configuration for privacy-aware message routing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrivacyRoutingConfig {
    /// Force all messages through private path (onion circuits).
    ///
    /// When enabled, even consensus messages will be routed through
    /// circuits. This provides maximum privacy at the cost of latency.
    ///
    /// Default: false
    pub force_private: bool,

    /// Allow fast path fallback when no circuits are available.
    ///
    /// When enabled, messages that would normally use the private path
    /// can fall back to direct gossipsub if no circuits are available.
    /// This prioritizes availability over privacy.
    ///
    /// When disabled, messages will be queued until a circuit becomes
    /// available, which may cause delays or message drops.
    ///
    /// Default: false (privacy over availability)
    pub allow_fallback: bool,

    /// Log when fallback occurs.
    ///
    /// When enabled, a warning is logged each time a message falls back
    /// to the fast path due to circuit unavailability.
    ///
    /// Default: true
    pub log_fallback: bool,
}

impl Default for PrivacyRoutingConfig {
    fn default() -> Self {
        Self {
            force_private: false,
            allow_fallback: false,
            log_fallback: true,
        }
    }
}

impl PrivacyRoutingConfig {
    /// Create a new configuration with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a maximum privacy configuration.
    ///
    /// All messages use private path, no fallback allowed.
    pub fn max_privacy() -> Self {
        Self {
            force_private: true,
            allow_fallback: false,
            log_fallback: true,
        }
    }

    /// Create a configuration optimized for availability.
    ///
    /// Uses default paths but allows fallback to fast path.
    pub fn prioritize_availability() -> Self {
        Self {
            force_private: false,
            allow_fallback: true,
            log_fallback: true,
        }
    }
}

/// Metrics for routing decisions.
#[derive(Debug, Default)]
pub struct RoutingMetrics {
    /// Messages sent via fast path.
    pub fast_path_count: AtomicU64,

    /// Messages sent via private path.
    pub private_path_count: AtomicU64,

    /// Messages that fell back to fast path (no circuit available).
    pub fallback_count: AtomicU64,

    /// Messages queued waiting for circuit.
    pub queued_count: AtomicU64,

    /// Messages dropped due to no circuit and no fallback allowed.
    pub dropped_count: AtomicU64,
}

impl RoutingMetrics {
    /// Create new metrics instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a fast path message.
    pub fn record_fast(&self) {
        self.fast_path_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a private path message.
    pub fn record_private(&self) {
        self.private_path_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a fallback to fast path.
    pub fn record_fallback(&self) {
        self.fallback_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a queued message.
    pub fn record_queued(&self) {
        self.queued_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a dropped message.
    pub fn record_dropped(&self) {
        self.dropped_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Get a snapshot of current metrics.
    pub fn snapshot(&self) -> RoutingMetricsSnapshot {
        RoutingMetricsSnapshot {
            fast_path_count: self.fast_path_count.load(Ordering::Relaxed),
            private_path_count: self.private_path_count.load(Ordering::Relaxed),
            fallback_count: self.fallback_count.load(Ordering::Relaxed),
            queued_count: self.queued_count.load(Ordering::Relaxed),
            dropped_count: self.dropped_count.load(Ordering::Relaxed),
        }
    }
}

/// Snapshot of routing metrics (for RPC/monitoring).
#[derive(Debug, Clone, Copy, Default)]
pub struct RoutingMetricsSnapshot {
    /// Messages sent via fast path.
    pub fast_path_count: u64,
    /// Messages sent via private path.
    pub private_path_count: u64,
    /// Messages that fell back to fast path.
    pub fallback_count: u64,
    /// Messages queued waiting for circuit.
    pub queued_count: u64,
    /// Messages dropped due to no circuit.
    pub dropped_count: u64,
}

impl RoutingMetricsSnapshot {
    /// Calculate the private path ratio.
    ///
    /// Returns the fraction of messages that used the private path
    /// out of all messages that should have used it.
    pub fn private_path_ratio(&self) -> f64 {
        let total_private_intended = self.private_path_count + self.fallback_count;
        if total_private_intended == 0 {
            1.0
        } else {
            self.private_path_count as f64 / total_private_intended as f64
        }
    }

    /// Calculate the total messages routed.
    pub fn total_routed(&self) -> u64 {
        self.fast_path_count + self.private_path_count + self.fallback_count
    }
}

/// Result of a routing decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingDecision {
    /// Use fast path (direct gossipsub).
    UseFastPath,

    /// Use private path (onion circuit).
    UsePrivatePath,

    /// Fall back to fast path (no circuit available).
    FallbackToFast,

    /// Queue message until circuit available.
    QueueForCircuit,

    /// Drop message (no circuit, no fallback allowed, queue full).
    Drop,
}

impl RoutingDecision {
    /// Check if this decision results in immediate send.
    pub fn is_immediate(&self) -> bool {
        matches!(
            self,
            RoutingDecision::UseFastPath
                | RoutingDecision::UsePrivatePath
                | RoutingDecision::FallbackToFast
        )
    }

    /// Get the actual path used (if immediate).
    pub fn actual_path(&self) -> Option<MessagePath> {
        match self {
            RoutingDecision::UseFastPath | RoutingDecision::FallbackToFast => {
                Some(MessagePath::Fast)
            }
            RoutingDecision::UsePrivatePath => Some(MessagePath::Private),
            RoutingDecision::QueueForCircuit | RoutingDecision::Drop => None,
        }
    }
}

/// Privacy-aware message router.
///
/// Determines the routing path for network messages based on their
/// type and the configured privacy settings.
#[derive(Debug)]
pub struct PrivacyRouter {
    /// Routing configuration.
    config: PrivacyRoutingConfig,

    /// Routing metrics.
    metrics: Arc<RoutingMetrics>,
}

impl PrivacyRouter {
    /// Create a new router with the given configuration.
    pub fn new(config: PrivacyRoutingConfig) -> Self {
        Self {
            config,
            metrics: Arc::new(RoutingMetrics::new()),
        }
    }

    /// Create a new router with shared metrics.
    pub fn with_metrics(config: PrivacyRoutingConfig, metrics: Arc<RoutingMetrics>) -> Self {
        Self { config, metrics }
    }

    /// Get the router's configuration.
    pub fn config(&self) -> &PrivacyRoutingConfig {
        &self.config
    }

    /// Get the router's metrics.
    pub fn metrics(&self) -> &Arc<RoutingMetrics> {
        &self.metrics
    }

    /// Select the routing path for a message type.
    ///
    /// Returns the intended path based on message type and configuration.
    /// This does not consider circuit availability.
    pub fn select_path(&self, msg_type: MessageType) -> MessagePath {
        if self.config.force_private {
            MessagePath::Private
        } else {
            msg_type.default_path()
        }
    }

    /// Make a routing decision considering circuit availability.
    ///
    /// This is the main entry point for routing decisions. It considers:
    /// - Message type default path
    /// - Configuration overrides
    /// - Circuit availability
    /// - Fallback policy
    ///
    /// # Arguments
    ///
    /// * `msg_type` - The type of message to route
    /// * `circuit_available` - Whether an onion circuit is currently available
    ///
    /// # Returns
    ///
    /// A `RoutingDecision` indicating how to handle the message.
    pub fn decide(&self, msg_type: MessageType, circuit_available: bool) -> RoutingDecision {
        let intended_path = self.select_path(msg_type);

        match intended_path {
            MessagePath::Fast => {
                self.metrics.record_fast();
                RoutingDecision::UseFastPath
            }
            MessagePath::Private => {
                if circuit_available {
                    self.metrics.record_private();
                    RoutingDecision::UsePrivatePath
                } else if self.config.allow_fallback {
                    self.metrics.record_fallback();
                    if self.config.log_fallback {
                        tracing::warn!(
                            message_type = %msg_type,
                            "No circuit available, falling back to fast path"
                        );
                    }
                    RoutingDecision::FallbackToFast
                } else {
                    self.metrics.record_queued();
                    RoutingDecision::QueueForCircuit
                }
            }
        }
    }

    /// Check if a message type should use the private path.
    ///
    /// This is a convenience method for checking path selection
    /// without considering circuit availability.
    pub fn should_use_private(&self, msg_type: MessageType) -> bool {
        self.select_path(msg_type) == MessagePath::Private
    }
}

impl Default for PrivacyRouter {
    fn default() -> Self {
        Self::new(PrivacyRoutingConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_type_default_paths() {
        // Fast path messages
        assert_eq!(MessageType::ScpNominate.default_path(), MessagePath::Fast);
        assert_eq!(MessageType::ScpStatement.default_path(), MessagePath::Fast);
        assert_eq!(MessageType::BlockHeader.default_path(), MessagePath::Fast);
        assert_eq!(MessageType::BlockBody.default_path(), MessagePath::Fast);
        assert_eq!(
            MessageType::PeerAnnouncement.default_path(),
            MessagePath::Fast
        );
        assert_eq!(MessageType::PexMessage.default_path(), MessagePath::Fast);

        // Private path messages
        assert_eq!(
            MessageType::Transaction.default_path(),
            MessagePath::Private
        );
        assert_eq!(
            MessageType::SyncRequest.default_path(),
            MessagePath::Private
        );
        assert_eq!(
            MessageType::WalletQuery.default_path(),
            MessagePath::Private
        );
    }

    #[test]
    fn test_router_default_config() {
        let router = PrivacyRouter::default();

        // Check default path selection
        assert_eq!(
            router.select_path(MessageType::Transaction),
            MessagePath::Private
        );
        assert_eq!(
            router.select_path(MessageType::ScpStatement),
            MessagePath::Fast
        );
    }

    #[test]
    fn test_router_force_private() {
        let config = PrivacyRoutingConfig {
            force_private: true,
            ..Default::default()
        };
        let router = PrivacyRouter::new(config);

        // All messages should use private path
        assert_eq!(
            router.select_path(MessageType::ScpStatement),
            MessagePath::Private
        );
        assert_eq!(
            router.select_path(MessageType::BlockHeader),
            MessagePath::Private
        );
    }

    #[test]
    fn test_routing_decision_with_circuit() {
        let router = PrivacyRouter::default();

        // Transaction with circuit available
        let decision = router.decide(MessageType::Transaction, true);
        assert_eq!(decision, RoutingDecision::UsePrivatePath);
        assert_eq!(decision.actual_path(), Some(MessagePath::Private));

        // SCP message (always fast)
        let decision = router.decide(MessageType::ScpStatement, true);
        assert_eq!(decision, RoutingDecision::UseFastPath);
        assert_eq!(decision.actual_path(), Some(MessagePath::Fast));
    }

    #[test]
    fn test_routing_decision_no_circuit_no_fallback() {
        let config = PrivacyRoutingConfig {
            allow_fallback: false,
            ..Default::default()
        };
        let router = PrivacyRouter::new(config);

        // Transaction without circuit, no fallback
        let decision = router.decide(MessageType::Transaction, false);
        assert_eq!(decision, RoutingDecision::QueueForCircuit);
        assert!(!decision.is_immediate());
    }

    #[test]
    fn test_routing_decision_no_circuit_with_fallback() {
        let config = PrivacyRoutingConfig {
            allow_fallback: true,
            log_fallback: false, // Disable logging in tests
            ..Default::default()
        };
        let router = PrivacyRouter::new(config);

        // Transaction without circuit, with fallback
        let decision = router.decide(MessageType::Transaction, false);
        assert_eq!(decision, RoutingDecision::FallbackToFast);
        assert_eq!(decision.actual_path(), Some(MessagePath::Fast));
    }

    #[test]
    fn test_metrics_tracking() {
        let router = PrivacyRouter::default();

        // Generate some routing decisions
        router.decide(MessageType::ScpStatement, true);
        router.decide(MessageType::ScpNominate, true);
        router.decide(MessageType::Transaction, true);

        let snapshot = router.metrics().snapshot();
        assert_eq!(snapshot.fast_path_count, 2);
        assert_eq!(snapshot.private_path_count, 1);
    }

    #[test]
    fn test_metrics_fallback_tracking() {
        let config = PrivacyRoutingConfig {
            allow_fallback: true,
            log_fallback: false,
            ..Default::default()
        };
        let router = PrivacyRouter::new(config);

        // Fallback scenarios
        router.decide(MessageType::Transaction, false);
        router.decide(MessageType::SyncRequest, false);

        let snapshot = router.metrics().snapshot();
        assert_eq!(snapshot.fallback_count, 2);
    }

    #[test]
    fn test_private_path_ratio() {
        let snapshot = RoutingMetricsSnapshot {
            fast_path_count: 10,
            private_path_count: 8,
            fallback_count: 2,
            queued_count: 0,
            dropped_count: 0,
        };

        // 8 out of 10 private-intended messages used private path
        assert!((snapshot.private_path_ratio() - 0.8).abs() < 0.001);
    }

    #[test]
    fn test_latency_sensitive() {
        assert!(MessageType::ScpNominate.is_latency_sensitive());
        assert!(MessageType::ScpStatement.is_latency_sensitive());
        assert!(MessageType::BlockHeader.is_latency_sensitive());
        assert!(!MessageType::Transaction.is_latency_sensitive());
    }

    #[test]
    fn test_reveals_user_activity() {
        assert!(MessageType::Transaction.reveals_user_activity());
        assert!(MessageType::SyncRequest.reveals_user_activity());
        assert!(MessageType::WalletQuery.reveals_user_activity());
        assert!(!MessageType::ScpStatement.reveals_user_activity());
    }

    #[test]
    fn test_max_privacy_config() {
        let config = PrivacyRoutingConfig::max_privacy();
        let router = PrivacyRouter::new(config);

        // All messages should use private path
        assert!(router.should_use_private(MessageType::ScpStatement));
        assert!(router.should_use_private(MessageType::Transaction));

        // No fallback allowed
        let decision = router.decide(MessageType::Transaction, false);
        assert_eq!(decision, RoutingDecision::QueueForCircuit);
    }

    #[test]
    fn test_prioritize_availability_config() {
        let config = PrivacyRoutingConfig::prioritize_availability();
        let router = PrivacyRouter::new(config);

        // Default paths (not force_private)
        assert!(!router.should_use_private(MessageType::ScpStatement));
        assert!(router.should_use_private(MessageType::Transaction));

        // Fallback allowed
        let decision = router.decide(MessageType::Transaction, false);
        assert_eq!(decision, RoutingDecision::FallbackToFast);
    }
}
