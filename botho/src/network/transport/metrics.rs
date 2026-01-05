// Copyright (c) 2024 Botho Foundation

//! Transport metrics for intelligent selection.
//!
//! This module tracks transport performance metrics to enable metrics-based
//! transport selection. It records success rates, latencies, and failure
//! information to improve transport selection over time.
//!
//! # Example
//!
//! ```
//! use botho::network::transport::metrics::{TransportMetrics, ConnectResult};
//! use botho::network::transport::TransportType;
//! use std::time::Duration;
//!
//! let mut metrics = TransportMetrics::new();
//!
//! // Record successful connection
//! metrics.record(TransportType::WebRTC, ConnectResult::Success {
//!     latency: Duration::from_millis(150),
//! });
//!
//! // Get success rate
//! let rate = metrics.success_rate(TransportType::WebRTC);
//! assert_eq!(rate, 1.0);
//! ```

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use super::types::TransportType;

/// Maximum number of recent failures to track per transport.
const MAX_RECENT_FAILURES: usize = 10;

/// Time window for considering failures as "recent" (5 minutes).
const RECENT_FAILURE_WINDOW: Duration = Duration::from_secs(300);

/// Minimum number of attempts before metrics are considered reliable.
const MIN_ATTEMPTS_FOR_RELIABILITY: u32 = 5;

/// Result of a connection attempt.
#[derive(Debug, Clone)]
pub enum ConnectResult {
    /// Connection succeeded.
    Success {
        /// Time taken to establish the connection.
        latency: Duration,
    },
    /// Connection failed.
    Failure {
        /// Error message describing the failure.
        error: String,
        /// Whether the failure was due to timeout.
        is_timeout: bool,
    },
}

impl ConnectResult {
    /// Create a success result with the given latency.
    pub fn success(latency: Duration) -> Self {
        Self::Success { latency }
    }

    /// Create a failure result.
    pub fn failure(error: impl Into<String>) -> Self {
        Self::Failure {
            error: error.into(),
            is_timeout: false,
        }
    }

    /// Create a timeout failure result.
    pub fn timeout() -> Self {
        Self::Failure {
            error: "connection timed out".to_string(),
            is_timeout: true,
        }
    }

    /// Check if this result represents a success.
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success { .. })
    }
}

/// Metrics for a single transport type.
#[derive(Debug, Clone, Default)]
pub struct TransportStats {
    /// Total number of connection attempts.
    pub total_attempts: u32,
    /// Number of successful connections.
    pub successes: u32,
    /// Number of failed connections.
    pub failures: u32,
    /// Number of timeout failures.
    pub timeouts: u32,
    /// Sum of all successful connection latencies.
    pub total_latency: Duration,
    /// Recent failure timestamps for calculating recent failure rate.
    pub recent_failures: Vec<Instant>,
    /// Last successful connection time.
    pub last_success: Option<Instant>,
    /// Last failure time.
    pub last_failure: Option<Instant>,
}

impl TransportStats {
    /// Create new transport stats.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a connection result.
    pub fn record(&mut self, result: &ConnectResult) {
        self.total_attempts += 1;

        match result {
            ConnectResult::Success { latency } => {
                self.successes += 1;
                self.total_latency += *latency;
                self.last_success = Some(Instant::now());
            }
            ConnectResult::Failure { is_timeout, .. } => {
                self.failures += 1;
                if *is_timeout {
                    self.timeouts += 1;
                }
                self.last_failure = Some(Instant::now());
                self.recent_failures.push(Instant::now());

                // Trim old failures
                self.prune_old_failures();
            }
        }
    }

    /// Remove failures older than the recent window.
    fn prune_old_failures(&mut self) {
        let cutoff = Instant::now() - RECENT_FAILURE_WINDOW;
        self.recent_failures.retain(|&t| t > cutoff);

        // Also limit the number of tracked failures
        while self.recent_failures.len() > MAX_RECENT_FAILURES {
            self.recent_failures.remove(0);
        }
    }

    /// Get the success rate (0.0 to 1.0).
    pub fn success_rate(&self) -> f64 {
        if self.total_attempts == 0 {
            // No data, assume success
            1.0
        } else {
            self.successes as f64 / self.total_attempts as f64
        }
    }

    /// Get the average latency for successful connections.
    pub fn average_latency(&self) -> Option<Duration> {
        if self.successes == 0 {
            None
        } else {
            Some(self.total_latency / self.successes)
        }
    }

    /// Get the number of recent failures (within the time window).
    pub fn recent_failure_count(&self) -> usize {
        let cutoff = Instant::now() - RECENT_FAILURE_WINDOW;
        self.recent_failures.iter().filter(|&&t| t > cutoff).count()
    }

    /// Check if this transport should be avoided due to recent failures.
    pub fn should_avoid(&self) -> bool {
        // Avoid if there are many recent failures
        self.recent_failure_count() >= 3
    }

    /// Check if the metrics are reliable (enough data points).
    pub fn is_reliable(&self) -> bool {
        self.total_attempts >= MIN_ATTEMPTS_FOR_RELIABILITY
    }
}

/// Transport metrics for all transport types.
///
/// Tracks performance metrics for each transport type to enable
/// intelligent transport selection based on historical performance.
#[derive(Debug, Clone, Default)]
pub struct TransportMetrics {
    /// Per-transport statistics.
    stats: HashMap<TransportType, TransportStats>,
}

impl TransportMetrics {
    /// Create a new metrics tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a connection attempt result.
    pub fn record(&mut self, transport: TransportType, result: ConnectResult) {
        self.stats
            .entry(transport)
            .or_insert_with(TransportStats::new)
            .record(&result);
    }

    /// Get the success rate for a transport type (0.0 to 1.0).
    pub fn success_rate(&self, transport: TransportType) -> f64 {
        self.stats
            .get(&transport)
            .map(|s| s.success_rate())
            .unwrap_or(1.0)
    }

    /// Get the average latency for a transport type.
    pub fn average_latency(&self, transport: TransportType) -> Option<Duration> {
        self.stats.get(&transport).and_then(|s| s.average_latency())
    }

    /// Get the statistics for a transport type.
    pub fn get_stats(&self, transport: TransportType) -> Option<&TransportStats> {
        self.stats.get(&transport)
    }

    /// Get a recommendation for the best transport based on metrics.
    ///
    /// Returns the transport with the best score considering:
    /// - Success rate (higher is better)
    /// - Average latency (lower is better)
    /// - Recent failures (fewer is better)
    pub fn recommend(&self, available: &[TransportType]) -> Option<TransportType> {
        if available.is_empty() {
            return None;
        }

        let mut best: Option<(TransportType, f64)> = None;

        for &transport in available {
            let score = self.calculate_score(transport);

            match &best {
                Some((_, best_score)) if score > *best_score => {
                    best = Some((transport, score));
                }
                None => {
                    best = Some((transport, score));
                }
                _ => {}
            }
        }

        best.map(|(t, _)| t)
    }

    /// Calculate a score for transport selection.
    ///
    /// The score combines multiple factors:
    /// - Success rate contributes positively
    /// - Lower latency contributes positively
    /// - Recent failures contribute negatively
    /// - Unreliable metrics are penalized slightly
    fn calculate_score(&self, transport: TransportType) -> f64 {
        let stats = match self.stats.get(&transport) {
            Some(s) => s,
            None => return 50.0, // Default score for unknown transports
        };

        let mut score = 0.0;

        // Success rate: 0-50 points
        score += stats.success_rate() * 50.0;

        // Latency: 0-25 points (lower is better)
        if let Some(latency) = stats.average_latency() {
            let latency_ms = latency.as_millis() as f64;
            // Scale: 0ms = 25 points, 500ms+ = 0 points
            score += (25.0 - (latency_ms / 20.0).min(25.0)).max(0.0);
        } else {
            score += 12.5; // Default mid-range for unknown latency
        }

        // Recent failures: -25 to 0 points
        let recent_failures = stats.recent_failure_count();
        score -= (recent_failures as f64) * 8.0;

        // Reliability bonus: +10 if we have enough data
        if stats.is_reliable() {
            score += 10.0;
        }

        // Avoid penalty if transport should be avoided
        if stats.should_avoid() {
            score -= 20.0;
        }

        score
    }

    /// Reset all metrics.
    pub fn reset(&mut self) {
        self.stats.clear();
    }

    /// Reset metrics for a specific transport.
    pub fn reset_transport(&mut self, transport: TransportType) {
        self.stats.remove(&transport);
    }

    /// Get a snapshot of metrics for serialization/logging.
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            transports: self
                .stats
                .iter()
                .map(|(&t, s)| {
                    (
                        t,
                        TransportMetricsSummary {
                            total_attempts: s.total_attempts,
                            successes: s.successes,
                            failures: s.failures,
                            timeouts: s.timeouts,
                            success_rate: s.success_rate(),
                            average_latency_ms: s.average_latency().map(|d| d.as_millis() as u64),
                            recent_failures: s.recent_failure_count() as u32,
                        },
                    )
                })
                .collect(),
        }
    }
}

/// A serializable snapshot of transport metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    /// Metrics for each transport type.
    pub transports: HashMap<TransportType, TransportMetricsSummary>,
}

/// Summary metrics for a single transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportMetricsSummary {
    /// Total connection attempts.
    pub total_attempts: u32,
    /// Successful connections.
    pub successes: u32,
    /// Failed connections.
    pub failures: u32,
    /// Timeout failures.
    pub timeouts: u32,
    /// Current success rate (0.0 to 1.0).
    pub success_rate: f64,
    /// Average latency in milliseconds.
    pub average_latency_ms: Option<u64>,
    /// Number of recent failures.
    pub recent_failures: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connect_result() {
        let success = ConnectResult::success(Duration::from_millis(100));
        assert!(success.is_success());

        let failure = ConnectResult::failure("test error");
        assert!(!failure.is_success());

        let timeout = ConnectResult::timeout();
        assert!(!timeout.is_success());
        if let ConnectResult::Failure { is_timeout, .. } = timeout {
            assert!(is_timeout);
        }
    }

    #[test]
    fn test_transport_stats_new() {
        let stats = TransportStats::new();
        assert_eq!(stats.total_attempts, 0);
        assert_eq!(stats.successes, 0);
        assert_eq!(stats.success_rate(), 1.0); // Default for no data
    }

    #[test]
    fn test_transport_stats_record_success() {
        let mut stats = TransportStats::new();
        stats.record(&ConnectResult::success(Duration::from_millis(100)));

        assert_eq!(stats.total_attempts, 1);
        assert_eq!(stats.successes, 1);
        assert_eq!(stats.failures, 0);
        assert_eq!(stats.success_rate(), 1.0);
        assert!(stats.last_success.is_some());
    }

    #[test]
    fn test_transport_stats_record_failure() {
        let mut stats = TransportStats::new();
        stats.record(&ConnectResult::failure("test error"));

        assert_eq!(stats.total_attempts, 1);
        assert_eq!(stats.successes, 0);
        assert_eq!(stats.failures, 1);
        assert_eq!(stats.success_rate(), 0.0);
        assert!(stats.last_failure.is_some());
    }

    #[test]
    fn test_transport_stats_average_latency() {
        let mut stats = TransportStats::new();
        stats.record(&ConnectResult::success(Duration::from_millis(100)));
        stats.record(&ConnectResult::success(Duration::from_millis(200)));

        let avg = stats.average_latency().unwrap();
        assert_eq!(avg, Duration::from_millis(150));
    }

    #[test]
    fn test_transport_metrics_new() {
        let metrics = TransportMetrics::new();
        assert_eq!(metrics.success_rate(TransportType::Plain), 1.0);
        assert!(metrics.average_latency(TransportType::Plain).is_none());
    }

    #[test]
    fn test_transport_metrics_record() {
        let mut metrics = TransportMetrics::new();

        metrics.record(
            TransportType::WebRTC,
            ConnectResult::success(Duration::from_millis(150)),
        );
        metrics.record(TransportType::WebRTC, ConnectResult::failure("error"));

        assert_eq!(metrics.success_rate(TransportType::WebRTC), 0.5);
        assert_eq!(
            metrics.average_latency(TransportType::WebRTC),
            Some(Duration::from_millis(150))
        );
    }

    #[test]
    fn test_transport_metrics_recommend() {
        let mut metrics = TransportMetrics::new();

        // Record good performance for WebRTC
        for _ in 0..5 {
            metrics.record(
                TransportType::WebRTC,
                ConnectResult::success(Duration::from_millis(50)),
            );
        }

        // Record poor performance for Plain
        for _ in 0..5 {
            metrics.record(TransportType::Plain, ConnectResult::failure("error"));
        }

        let available = vec![TransportType::Plain, TransportType::WebRTC];
        let recommended = metrics.recommend(&available);

        assert_eq!(recommended, Some(TransportType::WebRTC));
    }

    #[test]
    fn test_transport_metrics_recommend_empty() {
        let metrics = TransportMetrics::new();
        assert!(metrics.recommend(&[]).is_none());
    }

    #[test]
    fn test_transport_metrics_reset() {
        let mut metrics = TransportMetrics::new();
        metrics.record(
            TransportType::Plain,
            ConnectResult::success(Duration::from_millis(100)),
        );

        assert!(metrics.get_stats(TransportType::Plain).is_some());

        metrics.reset();
        assert!(metrics.get_stats(TransportType::Plain).is_none());
    }

    #[test]
    fn test_transport_metrics_snapshot() {
        let mut metrics = TransportMetrics::new();
        metrics.record(
            TransportType::WebRTC,
            ConnectResult::success(Duration::from_millis(100)),
        );

        let snapshot = metrics.snapshot();
        assert!(snapshot.transports.contains_key(&TransportType::WebRTC));

        let summary = snapshot.transports.get(&TransportType::WebRTC).unwrap();
        assert_eq!(summary.total_attempts, 1);
        assert_eq!(summary.successes, 1);
        assert_eq!(summary.success_rate, 1.0);
        assert_eq!(summary.average_latency_ms, Some(100));
    }

    #[test]
    fn test_should_avoid() {
        let mut stats = TransportStats::new();

        // Not enough failures to avoid
        stats.record(&ConnectResult::failure("error1"));
        stats.record(&ConnectResult::failure("error2"));
        assert!(!stats.should_avoid());

        // Now enough failures
        stats.record(&ConnectResult::failure("error3"));
        assert!(stats.should_avoid());
    }

    #[test]
    fn test_is_reliable() {
        let mut stats = TransportStats::new();

        for i in 0..4 {
            stats.record(&ConnectResult::success(Duration::from_millis(i * 100)));
        }
        assert!(!stats.is_reliable());

        stats.record(&ConnectResult::success(Duration::from_millis(500)));
        assert!(stats.is_reliable());
    }
}
