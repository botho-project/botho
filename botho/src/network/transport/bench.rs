// Copyright (c) 2024 Botho Foundation

//! Benchmark utilities for transport performance testing.
//!
//! This module provides types and utilities for benchmarking transport
//! implementations across different scenarios and network conditions.
//!
//! # Overview
//!
//! The benchmark framework measures:
//! - Connection establishment time
//! - First byte latency
//! - Throughput (bytes/second)
//! - Latency percentiles (p50, p99)
//! - Resource usage (CPU, memory)
//!
//! # Target Metrics (from design doc)
//!
//! | Privacy Level | Latency Overhead |
//! |--------------|------------------|
//! | Standard     | < 200ms p99      |
//! | Maximum      | < 1s p99         |
//!
//! # Usage
//!
//! ```ignore
//! use botho::network::transport::bench::{
//!     BenchmarkScenario, NetworkConditions, TransportBenchmark,
//! };
//!
//! // Create a scenario
//! let scenario = BenchmarkScenario::SmallMessage { size: 512 };
//!
//! // Run with network conditions
//! let conditions = NetworkConditions::lan();
//! let results = scenario.run_with_conditions(&transport, conditions).await;
//! ```
//!
//! # References
//!
//! - Design: `docs/design/traffic-privacy-roadmap.md` (Section 3.10)
//! - Issue: #211 (Performance benchmarks across transports)

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::TransportType;

/// Transport benchmark results.
///
/// Contains all metrics collected during a benchmark run for a specific
/// transport implementation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportBenchmark {
    /// Transport type tested.
    pub transport: TransportType,

    /// Connection establishment time.
    ///
    /// Time from connection initiation to ready-to-send state.
    /// Includes handshake, encryption setup, etc.
    pub connection_time: Duration,

    /// First byte latency.
    ///
    /// Time from sending first byte to receiving acknowledgment
    /// or response.
    pub first_byte_latency: Duration,

    /// Throughput in bytes per second.
    ///
    /// Sustained transfer rate during bulk transfers.
    pub throughput: f64,

    /// Median (p50) latency.
    pub latency_p50: Duration,

    /// 99th percentile latency.
    pub latency_p99: Duration,

    /// CPU usage during benchmark (0.0-1.0 fraction).
    pub cpu_usage: f64,

    /// Memory usage in bytes.
    pub memory_bytes: usize,

    /// Number of samples collected.
    pub sample_count: usize,
}

impl TransportBenchmark {
    /// Create a new benchmark result.
    pub fn new(transport: TransportType) -> Self {
        Self {
            transport,
            connection_time: Duration::ZERO,
            first_byte_latency: Duration::ZERO,
            throughput: 0.0,
            latency_p50: Duration::ZERO,
            latency_p99: Duration::ZERO,
            cpu_usage: 0.0,
            memory_bytes: 0,
            sample_count: 0,
        }
    }

    /// Check if latency meets target for standard privacy level.
    ///
    /// Target: < 200ms p99 latency overhead.
    pub fn meets_standard_target(&self) -> bool {
        self.latency_p99 < Duration::from_millis(200)
    }

    /// Check if latency meets target for maximum privacy level.
    ///
    /// Target: < 1s p99 latency overhead.
    pub fn meets_maximum_target(&self) -> bool {
        self.latency_p99 < Duration::from_secs(1)
    }

    /// Calculate overhead compared to a baseline.
    pub fn overhead_vs(&self, baseline: &TransportBenchmark) -> TransportOverhead {
        let latency_overhead = if baseline.latency_p99.as_nanos() > 0 {
            self.latency_p99.as_nanos() as f64 / baseline.latency_p99.as_nanos() as f64
        } else {
            1.0
        };

        let throughput_ratio = if baseline.throughput > 0.0 {
            self.throughput / baseline.throughput
        } else {
            1.0
        };

        let connection_overhead = if baseline.connection_time.as_nanos() > 0 {
            self.connection_time.as_nanos() as f64 / baseline.connection_time.as_nanos() as f64
        } else {
            1.0
        };

        TransportOverhead {
            latency_overhead,
            throughput_ratio,
            connection_overhead,
        }
    }
}

impl Default for TransportBenchmark {
    fn default() -> Self {
        Self::new(TransportType::Plain)
    }
}

/// Overhead metrics comparing transports.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TransportOverhead {
    /// Latency overhead multiplier (1.0 = same, 2.0 = twice as slow).
    pub latency_overhead: f64,

    /// Throughput ratio (1.0 = same, 0.5 = half the throughput).
    pub throughput_ratio: f64,

    /// Connection establishment overhead multiplier.
    pub connection_overhead: f64,
}

/// Benchmark scenarios for testing transports.
///
/// Each scenario represents a different workload pattern that
/// exercises transports in different ways.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BenchmarkScenario {
    /// Single small message (transaction-like).
    ///
    /// Tests latency for typical transaction messages.
    SmallMessage {
        /// Message size in bytes.
        size: usize,
    },

    /// Continuous stream of small messages.
    ///
    /// Tests sustained transaction throughput.
    TransactionStream {
        /// Target messages per second.
        rate: f64,
        /// Duration of the test.
        duration: Duration,
    },

    /// Large bulk transfer (block sync).
    ///
    /// Tests throughput for block synchronization.
    BulkTransfer {
        /// Total transfer size in bytes.
        size: usize,
    },

    /// Mixed workload combining small and large messages.
    ///
    /// Tests realistic network conditions.
    MixedWorkload {
        /// Ratio of small messages (0.0-1.0).
        small_ratio: f64,
        /// Total messages to send.
        message_count: usize,
    },
}

impl BenchmarkScenario {
    /// Create a small message scenario with typical transaction size.
    pub fn typical_transaction() -> Self {
        Self::SmallMessage { size: 512 }
    }

    /// Create a bulk transfer scenario for block sync.
    pub fn block_sync() -> Self {
        Self::BulkTransfer {
            size: 1024 * 1024, // 1 MB
        }
    }

    /// Create a mixed workload scenario.
    pub fn realistic() -> Self {
        Self::MixedWorkload {
            small_ratio: 0.8,
            message_count: 1000,
        }
    }

    /// Get a human-readable name for this scenario.
    pub fn name(&self) -> &'static str {
        match self {
            Self::SmallMessage { .. } => "small_message",
            Self::TransactionStream { .. } => "transaction_stream",
            Self::BulkTransfer { .. } => "bulk_transfer",
            Self::MixedWorkload { .. } => "mixed_workload",
        }
    }

    /// Get the expected message sizes for this scenario.
    pub fn message_sizes(&self) -> Vec<usize> {
        match self {
            Self::SmallMessage { size } => vec![*size],
            Self::TransactionStream { .. } => vec![512], // Typical transaction
            Self::BulkTransfer { size } => vec![*size],
            Self::MixedWorkload { small_ratio, .. } => {
                if *small_ratio > 0.5 {
                    vec![512, 8192] // Mix of small and medium
                } else {
                    vec![8192, 65536] // Mix of medium and large
                }
            }
        }
    }
}

/// Network conditions for simulating different environments.
///
/// Used to test transport behavior under various network conditions.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct NetworkConditions {
    /// One-way latency.
    pub latency: Duration,

    /// Latency jitter (variation).
    pub jitter: Duration,

    /// Packet loss probability (0.0-1.0).
    pub packet_loss: f64,

    /// Bandwidth limit in bytes/second (None = unlimited).
    pub bandwidth_limit: Option<usize>,
}

impl NetworkConditions {
    /// Create LAN conditions (low latency, no loss).
    pub fn lan() -> Self {
        Self {
            latency: Duration::from_micros(100),
            jitter: Duration::from_micros(10),
            packet_loss: 0.0,
            bandwidth_limit: None,
        }
    }

    /// Create WAN conditions (moderate latency).
    pub fn wan() -> Self {
        Self {
            latency: Duration::from_millis(50),
            jitter: Duration::from_millis(10),
            packet_loss: 0.001, // 0.1% loss
            bandwidth_limit: None,
        }
    }

    /// Create mobile network conditions.
    pub fn mobile() -> Self {
        Self {
            latency: Duration::from_millis(100),
            jitter: Duration::from_millis(30),
            packet_loss: 0.01,                       // 1% loss
            bandwidth_limit: Some(10 * 1024 * 1024), // 10 Mbps
        }
    }

    /// Create lossy network conditions for stress testing.
    pub fn lossy() -> Self {
        Self {
            latency: Duration::from_millis(20),
            jitter: Duration::from_millis(5),
            packet_loss: 0.05, // 5% loss
            bandwidth_limit: None,
        }
    }

    /// Create high-latency satellite conditions.
    pub fn satellite() -> Self {
        Self {
            latency: Duration::from_millis(300),
            jitter: Duration::from_millis(50),
            packet_loss: 0.02,                      // 2% loss
            bandwidth_limit: Some(5 * 1024 * 1024), // 5 Mbps
        }
    }

    /// Get a human-readable name for these conditions.
    pub fn name(&self) -> &'static str {
        if self.latency < Duration::from_millis(1) {
            "lan"
        } else if self.latency < Duration::from_millis(100) {
            "wan"
        } else if self.latency >= Duration::from_millis(200) {
            "satellite"
        } else if self.packet_loss >= 0.05 {
            "lossy"
        } else {
            "mobile"
        }
    }

    /// Calculate the round-trip time.
    pub fn rtt(&self) -> Duration {
        self.latency * 2
    }
}

impl Default for NetworkConditions {
    fn default() -> Self {
        Self::lan()
    }
}

/// Benchmark report comparing multiple transports.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkReport {
    /// Scenario used for benchmarking.
    pub scenario: String,

    /// Network conditions used.
    pub conditions: NetworkConditions,

    /// Results for each transport.
    pub results: Vec<TransportBenchmark>,

    /// Timestamp when benchmark was run.
    pub timestamp: String,

    /// Duration of the benchmark run.
    pub duration: Duration,
}

impl BenchmarkReport {
    /// Create a new benchmark report.
    pub fn new(scenario: &str, conditions: NetworkConditions) -> Self {
        Self {
            scenario: scenario.to_string(),
            conditions,
            results: Vec::new(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            duration: Duration::ZERO,
        }
    }

    /// Add a benchmark result.
    pub fn add_result(&mut self, result: TransportBenchmark) {
        self.results.push(result);
    }

    /// Get the baseline (plain transport) result.
    pub fn baseline(&self) -> Option<&TransportBenchmark> {
        self.results
            .iter()
            .find(|r| r.transport == TransportType::Plain)
    }

    /// Generate a markdown table comparing results.
    pub fn to_markdown_table(&self) -> String {
        let mut table = String::new();

        table.push_str("| Transport | Connection | p50 Latency | p99 Latency | Throughput |\n");
        table.push_str("|-----------|------------|-------------|-------------|------------|\n");

        for result in &self.results {
            table.push_str(&format!(
                "| {} | {:?} | {:?} | {:?} | {:.2} MB/s |\n",
                result.transport.name(),
                result.connection_time,
                result.latency_p50,
                result.latency_p99,
                result.throughput / (1024.0 * 1024.0)
            ));
        }

        table
    }

    /// Check if all transports meet the standard privacy target.
    pub fn all_meet_standard_target(&self) -> bool {
        self.results.iter().all(|r| r.meets_standard_target())
    }

    /// Check if all transports meet the maximum privacy target.
    pub fn all_meet_maximum_target(&self) -> bool {
        self.results.iter().all(|r| r.meets_maximum_target())
    }
}

/// Helper for collecting latency samples and computing percentiles.
#[derive(Debug, Clone, Default)]
pub struct LatencyCollector {
    samples: Vec<Duration>,
}

impl LatencyCollector {
    /// Create a new latency collector.
    pub fn new() -> Self {
        Self {
            samples: Vec::new(),
        }
    }

    /// Create a collector with pre-allocated capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            samples: Vec::with_capacity(capacity),
        }
    }

    /// Add a latency sample.
    pub fn add(&mut self, latency: Duration) {
        self.samples.push(latency);
    }

    /// Get the number of samples.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Calculate the median (p50) latency.
    pub fn p50(&self) -> Duration {
        self.percentile(50)
    }

    /// Calculate the 99th percentile latency.
    pub fn p99(&self) -> Duration {
        self.percentile(99)
    }

    /// Calculate a specific percentile.
    pub fn percentile(&self, p: usize) -> Duration {
        if self.samples.is_empty() {
            return Duration::ZERO;
        }

        let mut sorted = self.samples.clone();
        sorted.sort();

        let idx = (sorted.len() * p / 100).min(sorted.len() - 1);
        sorted[idx]
    }

    /// Calculate the mean latency.
    pub fn mean(&self) -> Duration {
        if self.samples.is_empty() {
            return Duration::ZERO;
        }

        let total: Duration = self.samples.iter().sum();
        total / self.samples.len() as u32
    }

    /// Calculate the minimum latency.
    pub fn min(&self) -> Duration {
        self.samples.iter().copied().min().unwrap_or(Duration::ZERO)
    }

    /// Calculate the maximum latency.
    pub fn max(&self) -> Duration {
        self.samples.iter().copied().max().unwrap_or(Duration::ZERO)
    }

    /// Get all samples.
    pub fn samples(&self) -> &[Duration] {
        &self.samples
    }
}

/// Helper for measuring throughput.
#[derive(Debug, Clone)]
pub struct ThroughputMeasurer {
    start_time: Option<std::time::Instant>,
    total_bytes: usize,
}

impl ThroughputMeasurer {
    /// Create a new throughput measurer.
    pub fn new() -> Self {
        Self {
            start_time: None,
            total_bytes: 0,
        }
    }

    /// Start measuring.
    pub fn start(&mut self) {
        self.start_time = Some(std::time::Instant::now());
        self.total_bytes = 0;
    }

    /// Record bytes transferred.
    pub fn record(&mut self, bytes: usize) {
        self.total_bytes += bytes;
    }

    /// Calculate throughput in bytes per second.
    pub fn throughput(&self) -> f64 {
        match self.start_time {
            Some(start) => {
                let elapsed = start.elapsed().as_secs_f64();
                if elapsed > 0.0 {
                    self.total_bytes as f64 / elapsed
                } else {
                    0.0
                }
            }
            None => 0.0,
        }
    }

    /// Get elapsed time since start.
    pub fn elapsed(&self) -> Duration {
        self.start_time
            .map(|s| s.elapsed())
            .unwrap_or(Duration::ZERO)
    }

    /// Get total bytes transferred.
    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }
}

impl Default for ThroughputMeasurer {
    fn default() -> Self {
        Self::new()
    }
}

/// Standard message sizes for benchmarking.
pub const BENCHMARK_MESSAGE_SIZES: [usize; 5] = [64, 512, 2048, 8192, 65536];

/// Number of iterations for latency measurements.
pub const DEFAULT_LATENCY_ITERATIONS: usize = 1000;

/// Number of iterations for throughput measurements.
pub const DEFAULT_THROUGHPUT_ITERATIONS: usize = 100;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transport_benchmark_default() {
        let bench = TransportBenchmark::default();
        assert_eq!(bench.transport, TransportType::Plain);
        assert_eq!(bench.sample_count, 0);
    }

    #[test]
    fn test_transport_benchmark_meets_targets() {
        let mut bench = TransportBenchmark::new(TransportType::Plain);
        bench.latency_p99 = Duration::from_millis(100);
        assert!(bench.meets_standard_target());
        assert!(bench.meets_maximum_target());

        bench.latency_p99 = Duration::from_millis(500);
        assert!(!bench.meets_standard_target());
        assert!(bench.meets_maximum_target());

        bench.latency_p99 = Duration::from_secs(2);
        assert!(!bench.meets_standard_target());
        assert!(!bench.meets_maximum_target());
    }

    #[test]
    fn test_transport_overhead() {
        let baseline = TransportBenchmark {
            transport: TransportType::Plain,
            connection_time: Duration::from_millis(100),
            first_byte_latency: Duration::from_millis(10),
            throughput: 100_000_000.0,
            latency_p50: Duration::from_millis(5),
            latency_p99: Duration::from_millis(20),
            cpu_usage: 0.05,
            memory_bytes: 1024 * 1024,
            sample_count: 1000,
        };

        let webrtc = TransportBenchmark {
            transport: TransportType::WebRTC,
            connection_time: Duration::from_millis(300),
            first_byte_latency: Duration::from_millis(30),
            throughput: 80_000_000.0,
            latency_p50: Duration::from_millis(15),
            latency_p99: Duration::from_millis(60),
            cpu_usage: 0.08,
            memory_bytes: 2 * 1024 * 1024,
            sample_count: 1000,
        };

        let overhead = webrtc.overhead_vs(&baseline);
        assert_eq!(overhead.connection_overhead, 3.0);
        assert_eq!(overhead.latency_overhead, 3.0);
        assert_eq!(overhead.throughput_ratio, 0.8);
    }

    #[test]
    fn test_benchmark_scenario_names() {
        assert_eq!(
            BenchmarkScenario::typical_transaction().name(),
            "small_message"
        );
        assert_eq!(BenchmarkScenario::block_sync().name(), "bulk_transfer");
        assert_eq!(BenchmarkScenario::realistic().name(), "mixed_workload");
    }

    #[test]
    fn test_network_conditions_names() {
        assert_eq!(NetworkConditions::lan().name(), "lan");
        assert_eq!(NetworkConditions::wan().name(), "wan");
        assert_eq!(NetworkConditions::mobile().name(), "mobile");
        assert_eq!(NetworkConditions::lossy().name(), "lossy");
        assert_eq!(NetworkConditions::satellite().name(), "satellite");
    }

    #[test]
    fn test_network_conditions_rtt() {
        let wan = NetworkConditions::wan();
        assert_eq!(wan.rtt(), Duration::from_millis(100));
    }

    #[test]
    fn test_latency_collector() {
        let mut collector = LatencyCollector::new();

        for i in 1..=100 {
            collector.add(Duration::from_millis(i));
        }

        assert_eq!(collector.len(), 100);
        assert_eq!(collector.min(), Duration::from_millis(1));
        assert_eq!(collector.max(), Duration::from_millis(100));
        assert_eq!(collector.p50(), Duration::from_millis(50));
        assert_eq!(collector.p99(), Duration::from_millis(99));
    }

    #[test]
    fn test_latency_collector_empty() {
        let collector = LatencyCollector::new();
        assert!(collector.is_empty());
        assert_eq!(collector.p50(), Duration::ZERO);
        assert_eq!(collector.mean(), Duration::ZERO);
    }

    #[test]
    fn test_throughput_measurer() {
        let mut measurer = ThroughputMeasurer::new();
        measurer.start();
        measurer.record(1000);
        measurer.record(2000);

        assert_eq!(measurer.total_bytes(), 3000);
        assert!(measurer.elapsed() > Duration::ZERO);
        // Throughput depends on timing, just check it's positive
        assert!(measurer.throughput() > 0.0);
    }

    #[test]
    fn test_benchmark_report_markdown() {
        let mut report = BenchmarkReport::new("test", NetworkConditions::lan());

        let mut plain = TransportBenchmark::new(TransportType::Plain);
        plain.connection_time = Duration::from_millis(50);
        plain.latency_p50 = Duration::from_millis(5);
        plain.latency_p99 = Duration::from_millis(20);
        plain.throughput = 100_000_000.0;
        report.add_result(plain);

        let table = report.to_markdown_table();
        assert!(table.contains("plain"));
        assert!(table.contains("MB/s"));
    }
}
