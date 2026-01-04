// Copyright (c) 2024 Botho Foundation

//! Prometheus metrics for onion gossip privacy layer.
//!
//! This module provides Prometheus-compatible metrics for monitoring the health
//! and performance of the privacy layer. Metrics are organized into categories:
//!
//! - **Circuit Metrics**: Circuit pool health and build statistics
//! - **Relay Metrics**: Relay traffic and rate limiting
//! - **Path Metrics**: Routing decisions (private vs fast path)
//! - **Handshake Metrics**: Circuit handshake performance
//!
//! # Integration
//!
//! Register all privacy metrics with the global Prometheus registry:
//!
//! ```ignore
//! use botho::network::privacy::metrics::register_privacy_metrics;
//! use botho::rpc::metrics::REGISTRY;
//!
//! register_privacy_metrics(&REGISTRY);
//! ```
//!
//! # Dashboard Queries
//!
//! Example Prometheus queries for monitoring:
//!
//! ```promql
//! # Circuit pool size
//! botho_privacy_circuits_active
//!
//! # Circuit build success rate
//! rate(botho_privacy_circuits_built_total[5m]) /
//! (rate(botho_privacy_circuits_built_total[5m]) +
//!  rate(botho_privacy_circuit_build_failures_total[5m]))
//!
//! # Relay throughput
//! rate(botho_privacy_relay_forwarded_total[1m])
//!
//! # Private path usage ratio
//! rate(botho_privacy_tx_private_total[5m]) /
//! (rate(botho_privacy_tx_private_total[5m]) +
//!  rate(botho_privacy_tx_fast_fallback_total[5m]))
//! ```

use lazy_static::lazy_static;
use prometheus::{Gauge, Histogram, HistogramOpts, IntCounter, IntGauge, Registry};
use tracing::info;

// ============================================================================
// Circuit Metrics
// ============================================================================

lazy_static! {
    /// Number of active circuits in the pool.
    pub static ref CIRCUITS_ACTIVE: IntGauge = IntGauge::new(
        "botho_privacy_circuits_active",
        "Number of active outbound circuits in the pool"
    ).expect("Failed to create circuits_active metric");

    /// Total circuits successfully built.
    pub static ref CIRCUITS_BUILT: IntCounter = IntCounter::new(
        "botho_privacy_circuits_built_total",
        "Total number of circuits successfully built"
    ).expect("Failed to create circuits_built metric");

    /// Total circuit build failures.
    pub static ref CIRCUIT_BUILD_FAILURES: IntCounter = IntCounter::new(
        "botho_privacy_circuit_build_failures_total",
        "Total number of circuit build failures"
    ).expect("Failed to create circuit_build_failures metric");

    /// Circuit build latency histogram (milliseconds).
    pub static ref CIRCUIT_BUILD_LATENCY: Histogram = Histogram::with_opts(
        HistogramOpts::new(
            "botho_privacy_circuit_build_latency_ms",
            "Circuit build latency in milliseconds"
        ).buckets(vec![50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0])
    ).expect("Failed to create circuit_build_latency metric");

    /// Total circuits rotated/expired.
    pub static ref CIRCUITS_ROTATED: IntCounter = IntCounter::new(
        "botho_privacy_circuits_rotated_total",
        "Total number of circuits rotated due to expiration"
    ).expect("Failed to create circuits_rotated metric");
}

// ============================================================================
// Relay Metrics
// ============================================================================

lazy_static! {
    /// Messages relayed (forwarded to next hop).
    pub static ref RELAY_FORWARDED: IntCounter = IntCounter::new(
        "botho_privacy_relay_forwarded_total",
        "Total number of onion messages forwarded to next hop"
    ).expect("Failed to create relay_forwarded metric");

    /// Messages exited (broadcast to gossipsub).
    pub static ref RELAY_EXITED: IntCounter = IntCounter::new(
        "botho_privacy_relay_exited_total",
        "Total number of onion messages exited to gossipsub"
    ).expect("Failed to create relay_exited metric");

    /// Relay messages rate limited.
    pub static ref RELAY_RATE_LIMITED: IntCounter = IntCounter::new(
        "botho_privacy_relay_rate_limited_total",
        "Total number of relay messages dropped due to rate limiting"
    ).expect("Failed to create relay_rate_limited metric");

    /// Peers disconnected for relay abuse.
    pub static ref RELAY_DISCONNECTS: IntCounter = IntCounter::new(
        "botho_privacy_relay_disconnects_total",
        "Total number of peers disconnected for relay abuse"
    ).expect("Failed to create relay_disconnects metric");

    /// Bytes relayed total.
    pub static ref RELAY_BYTES: IntCounter = IntCounter::new(
        "botho_privacy_relay_bytes_total",
        "Total bytes relayed through onion circuits"
    ).expect("Failed to create relay_bytes metric");

    /// Current relay load (0.0-1.0).
    pub static ref RELAY_LOAD: Gauge = Gauge::new(
        "botho_privacy_relay_load",
        "Current relay load as a fraction (0.0-1.0)"
    ).expect("Failed to create relay_load metric");

    /// Unknown circuit messages dropped.
    pub static ref RELAY_UNKNOWN_CIRCUITS: IntCounter = IntCounter::new(
        "botho_privacy_relay_unknown_circuits_total",
        "Total messages dropped due to unknown circuit ID"
    ).expect("Failed to create relay_unknown_circuits metric");

    /// Decryption failures.
    pub static ref RELAY_DECRYPTION_FAILURES: IntCounter = IntCounter::new(
        "botho_privacy_relay_decryption_failures_total",
        "Total messages dropped due to decryption failure"
    ).expect("Failed to create relay_decryption_failures metric");
}

// ============================================================================
// Privacy Path Metrics
// ============================================================================

lazy_static! {
    /// Transactions sent via private path (onion circuit).
    pub static ref TX_PRIVATE: IntCounter = IntCounter::new(
        "botho_privacy_tx_private_total",
        "Total transactions broadcast via private onion path"
    ).expect("Failed to create tx_private metric");

    /// Transactions sent via fast path (fallback).
    pub static ref TX_FAST_FALLBACK: IntCounter = IntCounter::new(
        "botho_privacy_tx_fast_fallback_total",
        "Total transactions broadcast via fast path fallback"
    ).expect("Failed to create tx_fast_fallback metric");

    /// Private path latency histogram (milliseconds).
    pub static ref PRIVATE_LATENCY: Histogram = Histogram::with_opts(
        HistogramOpts::new(
            "botho_privacy_private_latency_ms",
            "Private path broadcast latency in milliseconds"
        ).buckets(vec![10.0, 25.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0])
    ).expect("Failed to create private_latency metric");

    /// Cover traffic generated.
    pub static ref COVER_GENERATED: IntCounter = IntCounter::new(
        "botho_privacy_cover_generated_total",
        "Total cover traffic messages generated"
    ).expect("Failed to create cover_generated metric");

    /// Cover traffic received (and dropped).
    pub static ref COVER_RECEIVED: IntCounter = IntCounter::new(
        "botho_privacy_cover_received_total",
        "Total cover traffic messages received and dropped"
    ).expect("Failed to create cover_received metric");

    /// Messages queued waiting for circuit.
    pub static ref TX_QUEUED: IntCounter = IntCounter::new(
        "botho_privacy_tx_queued_total",
        "Total transactions queued waiting for circuit"
    ).expect("Failed to create tx_queued metric");

    /// Messages dropped (no circuit, no fallback).
    pub static ref TX_DROPPED: IntCounter = IntCounter::new(
        "botho_privacy_tx_dropped_total",
        "Total transactions dropped due to no circuit"
    ).expect("Failed to create tx_dropped metric");
}

// ============================================================================
// Handshake Metrics
// ============================================================================

lazy_static! {
    /// CREATE messages sent.
    pub static ref HANDSHAKE_CREATES_SENT: IntCounter = IntCounter::new(
        "botho_privacy_handshake_creates_sent_total",
        "Total CREATE handshake messages sent"
    ).expect("Failed to create handshake_creates_sent metric");

    /// CREATED responses received.
    pub static ref HANDSHAKE_CREATED_RECEIVED: IntCounter = IntCounter::new(
        "botho_privacy_handshake_created_received_total",
        "Total CREATED handshake responses received"
    ).expect("Failed to create handshake_created_received metric");

    /// EXTEND messages sent.
    pub static ref HANDSHAKE_EXTENDS_SENT: IntCounter = IntCounter::new(
        "botho_privacy_handshake_extends_sent_total",
        "Total EXTEND handshake messages sent"
    ).expect("Failed to create handshake_extends_sent metric");

    /// EXTENDED responses received.
    pub static ref HANDSHAKE_EXTENDED_RECEIVED: IntCounter = IntCounter::new(
        "botho_privacy_handshake_extended_received_total",
        "Total EXTENDED handshake responses received"
    ).expect("Failed to create handshake_extended_received metric");

    /// Handshake timeouts.
    pub static ref HANDSHAKE_TIMEOUTS: IntCounter = IntCounter::new(
        "botho_privacy_handshake_timeouts_total",
        "Total handshake timeouts"
    ).expect("Failed to create handshake_timeouts metric");

    /// Handshake latency per hop (milliseconds).
    pub static ref HANDSHAKE_LATENCY: Histogram = Histogram::with_opts(
        HistogramOpts::new(
            "botho_privacy_handshake_latency_ms",
            "Handshake latency per hop in milliseconds"
        ).buckets(vec![10.0, 25.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0])
    ).expect("Failed to create handshake_latency metric");
}

// ============================================================================
// Registration
// ============================================================================

/// Register all privacy metrics with a Prometheus registry.
///
/// This function should be called once during node startup, after
/// `init_metrics()` from the RPC module.
///
/// # Example
///
/// ```ignore
/// use botho::network::privacy::metrics::register_privacy_metrics;
/// use botho::rpc::metrics::REGISTRY;
///
/// register_privacy_metrics(&REGISTRY);
/// ```
pub fn register_privacy_metrics(registry: &Registry) {
    // Circuit metrics
    registry
        .register(Box::new(CIRCUITS_ACTIVE.clone()))
        .expect("Failed to register circuits_active");
    registry
        .register(Box::new(CIRCUITS_BUILT.clone()))
        .expect("Failed to register circuits_built");
    registry
        .register(Box::new(CIRCUIT_BUILD_FAILURES.clone()))
        .expect("Failed to register circuit_build_failures");
    registry
        .register(Box::new(CIRCUIT_BUILD_LATENCY.clone()))
        .expect("Failed to register circuit_build_latency");
    registry
        .register(Box::new(CIRCUITS_ROTATED.clone()))
        .expect("Failed to register circuits_rotated");

    // Relay metrics
    registry
        .register(Box::new(RELAY_FORWARDED.clone()))
        .expect("Failed to register relay_forwarded");
    registry
        .register(Box::new(RELAY_EXITED.clone()))
        .expect("Failed to register relay_exited");
    registry
        .register(Box::new(RELAY_RATE_LIMITED.clone()))
        .expect("Failed to register relay_rate_limited");
    registry
        .register(Box::new(RELAY_DISCONNECTS.clone()))
        .expect("Failed to register relay_disconnects");
    registry
        .register(Box::new(RELAY_BYTES.clone()))
        .expect("Failed to register relay_bytes");
    registry
        .register(Box::new(RELAY_LOAD.clone()))
        .expect("Failed to register relay_load");
    registry
        .register(Box::new(RELAY_UNKNOWN_CIRCUITS.clone()))
        .expect("Failed to register relay_unknown_circuits");
    registry
        .register(Box::new(RELAY_DECRYPTION_FAILURES.clone()))
        .expect("Failed to register relay_decryption_failures");

    // Path metrics
    registry
        .register(Box::new(TX_PRIVATE.clone()))
        .expect("Failed to register tx_private");
    registry
        .register(Box::new(TX_FAST_FALLBACK.clone()))
        .expect("Failed to register tx_fast_fallback");
    registry
        .register(Box::new(PRIVATE_LATENCY.clone()))
        .expect("Failed to register private_latency");
    registry
        .register(Box::new(COVER_GENERATED.clone()))
        .expect("Failed to register cover_generated");
    registry
        .register(Box::new(COVER_RECEIVED.clone()))
        .expect("Failed to register cover_received");
    registry
        .register(Box::new(TX_QUEUED.clone()))
        .expect("Failed to register tx_queued");
    registry
        .register(Box::new(TX_DROPPED.clone()))
        .expect("Failed to register tx_dropped");

    // Handshake metrics
    registry
        .register(Box::new(HANDSHAKE_CREATES_SENT.clone()))
        .expect("Failed to register handshake_creates_sent");
    registry
        .register(Box::new(HANDSHAKE_CREATED_RECEIVED.clone()))
        .expect("Failed to register handshake_created_received");
    registry
        .register(Box::new(HANDSHAKE_EXTENDS_SENT.clone()))
        .expect("Failed to register handshake_extends_sent");
    registry
        .register(Box::new(HANDSHAKE_EXTENDED_RECEIVED.clone()))
        .expect("Failed to register handshake_extended_received");
    registry
        .register(Box::new(HANDSHAKE_TIMEOUTS.clone()))
        .expect("Failed to register handshake_timeouts");
    registry
        .register(Box::new(HANDSHAKE_LATENCY.clone()))
        .expect("Failed to register handshake_latency");

    info!("Privacy metrics registered with Prometheus");
}

// ============================================================================
// Metrics Updater (convenience wrapper)
// ============================================================================

/// Convenience wrapper for updating privacy metrics.
///
/// This struct provides a cleaner API for updating metrics from
/// various privacy layer components.
#[derive(Clone, Default)]
pub struct PrivacyMetricsUpdater;

impl PrivacyMetricsUpdater {
    /// Create a new metrics updater.
    pub fn new() -> Self {
        Self
    }

    // --- Circuit Metrics ---

    /// Set the number of active circuits.
    pub fn set_active_circuits(&self, count: usize) {
        CIRCUITS_ACTIVE.set(count as i64);
    }

    /// Record a successful circuit build.
    pub fn record_circuit_built(&self) {
        CIRCUITS_BUILT.inc();
    }

    /// Record a circuit build failure.
    pub fn record_circuit_build_failure(&self) {
        CIRCUIT_BUILD_FAILURES.inc();
    }

    /// Record circuit build latency in milliseconds.
    pub fn observe_circuit_build_latency(&self, ms: f64) {
        CIRCUIT_BUILD_LATENCY.observe(ms);
    }

    /// Record a circuit rotation/expiration.
    pub fn record_circuit_rotated(&self) {
        CIRCUITS_ROTATED.inc();
    }

    /// Record multiple circuit rotations.
    pub fn record_circuits_rotated(&self, count: u64) {
        for _ in 0..count {
            CIRCUITS_ROTATED.inc();
        }
    }

    // --- Relay Metrics ---

    /// Record a message forwarded to next hop.
    pub fn record_relay_forwarded(&self) {
        RELAY_FORWARDED.inc();
    }

    /// Record a message exited to gossipsub.
    pub fn record_relay_exited(&self) {
        RELAY_EXITED.inc();
    }

    /// Record a rate-limited message.
    pub fn record_relay_rate_limited(&self) {
        RELAY_RATE_LIMITED.inc();
    }

    /// Record a peer disconnect for abuse.
    pub fn record_relay_disconnect(&self) {
        RELAY_DISCONNECTS.inc();
    }

    /// Add bytes to relay counter.
    pub fn add_relay_bytes(&self, bytes: u64) {
        RELAY_BYTES.inc_by(bytes);
    }

    /// Set the current relay load (0.0-1.0).
    pub fn set_relay_load(&self, load: f64) {
        RELAY_LOAD.set(load);
    }

    /// Record an unknown circuit message.
    pub fn record_unknown_circuit(&self) {
        RELAY_UNKNOWN_CIRCUITS.inc();
    }

    /// Record a decryption failure.
    pub fn record_decryption_failure(&self) {
        RELAY_DECRYPTION_FAILURES.inc();
    }

    // --- Path Metrics ---

    /// Record a transaction sent via private path.
    pub fn record_tx_private(&self) {
        TX_PRIVATE.inc();
    }

    /// Record a transaction sent via fast fallback.
    pub fn record_tx_fast_fallback(&self) {
        TX_FAST_FALLBACK.inc();
    }

    /// Record private path latency in milliseconds.
    pub fn observe_private_latency(&self, ms: f64) {
        PRIVATE_LATENCY.observe(ms);
    }

    /// Record cover traffic generated.
    pub fn record_cover_generated(&self) {
        COVER_GENERATED.inc();
    }

    /// Record cover traffic received.
    pub fn record_cover_received(&self) {
        COVER_RECEIVED.inc();
    }

    /// Record a queued transaction.
    pub fn record_tx_queued(&self) {
        TX_QUEUED.inc();
    }

    /// Record a dropped transaction.
    pub fn record_tx_dropped(&self) {
        TX_DROPPED.inc();
    }

    // --- Handshake Metrics ---

    /// Record a CREATE message sent.
    pub fn record_handshake_create_sent(&self) {
        HANDSHAKE_CREATES_SENT.inc();
    }

    /// Record a CREATED response received.
    pub fn record_handshake_created_received(&self) {
        HANDSHAKE_CREATED_RECEIVED.inc();
    }

    /// Record an EXTEND message sent.
    pub fn record_handshake_extend_sent(&self) {
        HANDSHAKE_EXTENDS_SENT.inc();
    }

    /// Record an EXTENDED response received.
    pub fn record_handshake_extended_received(&self) {
        HANDSHAKE_EXTENDED_RECEIVED.inc();
    }

    /// Record a handshake timeout.
    pub fn record_handshake_timeout(&self) {
        HANDSHAKE_TIMEOUTS.inc();
    }

    /// Record handshake latency in milliseconds.
    pub fn observe_handshake_latency(&self, ms: f64) {
        HANDSHAKE_LATENCY.observe(ms);
    }
}

// ============================================================================
// Snapshot Aggregator
// ============================================================================

/// Aggregated snapshot of all privacy metrics.
///
/// This struct provides a point-in-time view of all privacy metrics,
/// useful for RPC endpoints and monitoring dashboards.
#[derive(Debug, Clone, Default)]
pub struct PrivacyMetricsSnapshot {
    // Circuit metrics
    /// Number of active circuits.
    pub circuits_active: i64,
    /// Total circuits built.
    pub circuits_built: u64,
    /// Total build failures.
    pub circuit_build_failures: u64,
    /// Total circuits rotated.
    pub circuits_rotated: u64,

    // Relay metrics
    /// Messages forwarded.
    pub relay_forwarded: u64,
    /// Messages exited.
    pub relay_exited: u64,
    /// Messages rate limited.
    pub relay_rate_limited: u64,
    /// Bytes relayed.
    pub relay_bytes: u64,
    /// Current relay load.
    pub relay_load: f64,
    /// Unknown circuit drops.
    pub relay_unknown_circuits: u64,
    /// Decryption failures.
    pub relay_decryption_failures: u64,

    // Path metrics
    /// Transactions via private path.
    pub tx_private: u64,
    /// Transactions via fast fallback.
    pub tx_fast_fallback: u64,
    /// Cover traffic generated.
    pub cover_generated: u64,
    /// Cover traffic received.
    pub cover_received: u64,
    /// Transactions queued.
    pub tx_queued: u64,
    /// Transactions dropped.
    pub tx_dropped: u64,

    // Handshake metrics
    /// CREATE messages sent.
    pub handshake_creates_sent: u64,
    /// CREATED responses received.
    pub handshake_created_received: u64,
    /// EXTEND messages sent.
    pub handshake_extends_sent: u64,
    /// EXTENDED responses received.
    pub handshake_extended_received: u64,
    /// Handshake timeouts.
    pub handshake_timeouts: u64,
}

impl PrivacyMetricsSnapshot {
    /// Capture current snapshot of all privacy metrics.
    pub fn capture() -> Self {
        Self {
            // Circuit metrics
            circuits_active: CIRCUITS_ACTIVE.get(),
            circuits_built: CIRCUITS_BUILT.get(),
            circuit_build_failures: CIRCUIT_BUILD_FAILURES.get(),
            circuits_rotated: CIRCUITS_ROTATED.get(),

            // Relay metrics
            relay_forwarded: RELAY_FORWARDED.get(),
            relay_exited: RELAY_EXITED.get(),
            relay_rate_limited: RELAY_RATE_LIMITED.get(),
            relay_bytes: RELAY_BYTES.get(),
            relay_load: RELAY_LOAD.get(),
            relay_unknown_circuits: RELAY_UNKNOWN_CIRCUITS.get(),
            relay_decryption_failures: RELAY_DECRYPTION_FAILURES.get(),

            // Path metrics
            tx_private: TX_PRIVATE.get(),
            tx_fast_fallback: TX_FAST_FALLBACK.get(),
            cover_generated: COVER_GENERATED.get(),
            cover_received: COVER_RECEIVED.get(),
            tx_queued: TX_QUEUED.get(),
            tx_dropped: TX_DROPPED.get(),

            // Handshake metrics
            handshake_creates_sent: HANDSHAKE_CREATES_SENT.get(),
            handshake_created_received: HANDSHAKE_CREATED_RECEIVED.get(),
            handshake_extends_sent: HANDSHAKE_EXTENDS_SENT.get(),
            handshake_extended_received: HANDSHAKE_EXTENDED_RECEIVED.get(),
            handshake_timeouts: HANDSHAKE_TIMEOUTS.get(),
        }
    }

    /// Calculate circuit build success rate.
    ///
    /// Returns 1.0 if no circuits have been attempted.
    pub fn circuit_build_success_rate(&self) -> f64 {
        let total = self.circuits_built + self.circuit_build_failures;
        if total == 0 {
            1.0
        } else {
            self.circuits_built as f64 / total as f64
        }
    }

    /// Calculate private path usage ratio.
    ///
    /// Returns the fraction of transactions that used the private path
    /// out of all transactions that should have used it.
    pub fn private_path_ratio(&self) -> f64 {
        let total_private_intended = self.tx_private + self.tx_fast_fallback;
        if total_private_intended == 0 {
            1.0
        } else {
            self.tx_private as f64 / total_private_intended as f64
        }
    }

    /// Calculate handshake success rate.
    ///
    /// Returns 1.0 if no handshakes have been attempted.
    pub fn handshake_success_rate(&self) -> f64 {
        let attempted = self.handshake_creates_sent;
        let completed = self.handshake_created_received;
        if attempted == 0 {
            1.0
        } else {
            completed as f64 / attempted as f64
        }
    }
}

// ============================================================================
// Alerting Thresholds
// ============================================================================

/// Alerting thresholds for privacy metrics.
///
/// These are recommended thresholds for alerting rules. Operators
/// should tune these based on their network conditions.
pub struct AlertingThresholds;

impl AlertingThresholds {
    /// Minimum number of active circuits before alerting.
    ///
    /// Alert when: `botho_privacy_circuits_active < 2`
    pub const MIN_CIRCUITS: i64 = 2;

    /// Maximum circuit build failure rate before alerting.
    ///
    /// Alert when: failure rate > 20%
    pub const MAX_BUILD_FAILURE_RATE: f64 = 0.20;

    /// Maximum rate limiting ratio before alerting.
    ///
    /// Alert when: rate_limited / (forwarded + exited) > 10%
    pub const MAX_RATE_LIMIT_RATIO: f64 = 0.10;

    /// Minimum private path ratio before alerting.
    ///
    /// Alert when: private / (private + fallback) < 90%
    pub const MIN_PRIVATE_PATH_RATIO: f64 = 0.90;

    /// Maximum handshake timeout rate before alerting.
    ///
    /// Alert when: timeouts / creates_sent > 5%
    pub const MAX_HANDSHAKE_TIMEOUT_RATE: f64 = 0.05;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_updater_circuits() {
        let updater = PrivacyMetricsUpdater::new();

        updater.set_active_circuits(5);
        assert_eq!(CIRCUITS_ACTIVE.get(), 5);

        let before = CIRCUITS_BUILT.get();
        updater.record_circuit_built();
        assert_eq!(CIRCUITS_BUILT.get(), before + 1);

        let before = CIRCUIT_BUILD_FAILURES.get();
        updater.record_circuit_build_failure();
        assert_eq!(CIRCUIT_BUILD_FAILURES.get(), before + 1);
    }

    #[test]
    fn test_metrics_updater_relay() {
        let updater = PrivacyMetricsUpdater::new();

        let before = RELAY_FORWARDED.get();
        updater.record_relay_forwarded();
        assert_eq!(RELAY_FORWARDED.get(), before + 1);

        let before = RELAY_EXITED.get();
        updater.record_relay_exited();
        assert_eq!(RELAY_EXITED.get(), before + 1);

        updater.set_relay_load(0.75);
        assert!((RELAY_LOAD.get() - 0.75).abs() < 0.001);
    }

    #[test]
    fn test_metrics_updater_path() {
        let updater = PrivacyMetricsUpdater::new();

        let before = TX_PRIVATE.get();
        updater.record_tx_private();
        assert_eq!(TX_PRIVATE.get(), before + 1);

        let before = TX_FAST_FALLBACK.get();
        updater.record_tx_fast_fallback();
        assert_eq!(TX_FAST_FALLBACK.get(), before + 1);
    }

    #[test]
    fn test_metrics_updater_handshake() {
        let updater = PrivacyMetricsUpdater::new();

        let before = HANDSHAKE_CREATES_SENT.get();
        updater.record_handshake_create_sent();
        assert_eq!(HANDSHAKE_CREATES_SENT.get(), before + 1);

        let before = HANDSHAKE_TIMEOUTS.get();
        updater.record_handshake_timeout();
        assert_eq!(HANDSHAKE_TIMEOUTS.get(), before + 1);
    }

    #[test]
    fn test_snapshot_capture() {
        // Reset some metrics to known state for testing
        CIRCUITS_ACTIVE.set(3);

        let snapshot = PrivacyMetricsSnapshot::capture();
        assert_eq!(snapshot.circuits_active, 3);
    }

    #[test]
    fn test_circuit_build_success_rate() {
        let snapshot = PrivacyMetricsSnapshot {
            circuits_built: 90,
            circuit_build_failures: 10,
            ..Default::default()
        };

        assert!((snapshot.circuit_build_success_rate() - 0.9).abs() < 0.001);
    }

    #[test]
    fn test_circuit_build_success_rate_no_attempts() {
        let snapshot = PrivacyMetricsSnapshot::default();
        assert!((snapshot.circuit_build_success_rate() - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_private_path_ratio() {
        let snapshot = PrivacyMetricsSnapshot {
            tx_private: 80,
            tx_fast_fallback: 20,
            ..Default::default()
        };

        assert!((snapshot.private_path_ratio() - 0.8).abs() < 0.001);
    }

    #[test]
    fn test_private_path_ratio_no_transactions() {
        let snapshot = PrivacyMetricsSnapshot::default();
        assert!((snapshot.private_path_ratio() - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_handshake_success_rate() {
        let snapshot = PrivacyMetricsSnapshot {
            handshake_creates_sent: 100,
            handshake_created_received: 95,
            ..Default::default()
        };

        assert!((snapshot.handshake_success_rate() - 0.95).abs() < 0.001);
    }

    #[test]
    fn test_alerting_thresholds() {
        assert_eq!(AlertingThresholds::MIN_CIRCUITS, 2);
        assert!((AlertingThresholds::MAX_BUILD_FAILURE_RATE - 0.20).abs() < 0.001);
        assert!((AlertingThresholds::MIN_PRIVATE_PATH_RATIO - 0.90).abs() < 0.001);
    }
}
