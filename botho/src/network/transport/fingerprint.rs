// Copyright (c) 2024 Botho Foundation

//! Fingerprinting resistance testing utilities.
//!
//! This module provides tools for verifying that botho WebRTC traffic is
//! statistically indistinguishable from legitimate WebRTC traffic (video calls).
//!
//! # Overview
//!
//! The goal of protocol obfuscation is to make botho traffic look like normal
//! WebRTC traffic. This module provides rigorous statistical tests to verify:
//! - Packet size distributions
//! - Inter-arrival time patterns
//! - Flow characteristics
//!
//! # Statistical Methodology
//!
//! We use the Kolmogorov-Smirnov (K-S) test to compare traffic distributions.
//! The null hypothesis is that two samples come from the same distribution.
//! A p-value > 0.05 indicates the samples are statistically indistinguishable.
//!
//! # Success Criteria
//!
//! From the design document:
//! - Protocol detection rate **<5%** by commercial DPI
//! - K-S test **p > 0.05** for all traffic characteristics
//!
//! # References
//!
//! - Design: `docs/design/traffic-privacy-roadmap.md` (Section 3.9)
//! - K-S Test: <https://en.wikipedia.org/wiki/Kolmogorov-Smirnov_test>
//! - Issue: #210

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Captured traffic pattern for statistical analysis.
///
/// This struct captures the essential characteristics of network traffic
/// that can be used for fingerprinting analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrafficPattern {
    /// Packet sizes in bytes (ordered by capture time)
    pub packet_sizes: Vec<usize>,

    /// Inter-arrival times between consecutive packets
    pub inter_arrival_times: Vec<Duration>,

    /// Total flow duration
    pub flow_duration: Duration,

    /// Total bytes sent (outbound)
    pub bytes_sent: usize,

    /// Total bytes received (inbound)
    pub bytes_received: usize,

    /// Direction of each packet (true = outbound, false = inbound)
    pub packet_directions: Vec<bool>,
}

impl TrafficPattern {
    /// Create a new empty traffic pattern.
    pub fn new() -> Self {
        Self {
            packet_sizes: Vec::new(),
            inter_arrival_times: Vec::new(),
            flow_duration: Duration::ZERO,
            bytes_sent: 0,
            bytes_received: 0,
            packet_directions: Vec::new(),
        }
    }

    /// Record an outbound packet.
    pub fn record_outbound(&mut self, size: usize, inter_arrival: Option<Duration>) {
        self.packet_sizes.push(size);
        self.packet_directions.push(true);
        self.bytes_sent += size;
        if let Some(iat) = inter_arrival {
            self.inter_arrival_times.push(iat);
        }
    }

    /// Record an inbound packet.
    pub fn record_inbound(&mut self, size: usize, inter_arrival: Option<Duration>) {
        self.packet_sizes.push(size);
        self.packet_directions.push(false);
        self.bytes_received += size;
        if let Some(iat) = inter_arrival {
            self.inter_arrival_times.push(iat);
        }
    }

    /// Set the total flow duration.
    pub fn set_duration(&mut self, duration: Duration) {
        self.flow_duration = duration;
    }

    /// Get the total number of packets.
    pub fn packet_count(&self) -> usize {
        self.packet_sizes.len()
    }

    /// Get the total bytes transferred.
    pub fn total_bytes(&self) -> usize {
        self.bytes_sent + self.bytes_received
    }

    /// Calculate average packet size.
    pub fn average_packet_size(&self) -> f64 {
        if self.packet_sizes.is_empty() {
            return 0.0;
        }
        self.packet_sizes.iter().sum::<usize>() as f64 / self.packet_sizes.len() as f64
    }

    /// Calculate average inter-arrival time in microseconds.
    pub fn average_inter_arrival_us(&self) -> f64 {
        if self.inter_arrival_times.is_empty() {
            return 0.0;
        }
        let total_us: u128 = self
            .inter_arrival_times
            .iter()
            .map(|d| d.as_micros())
            .sum();
        total_us as f64 / self.inter_arrival_times.len() as f64
    }

    /// Calculate bytes per second throughput.
    pub fn throughput_bps(&self) -> f64 {
        if self.flow_duration.is_zero() {
            return 0.0;
        }
        self.total_bytes() as f64 / self.flow_duration.as_secs_f64()
    }

    /// Get packet sizes as f64 for statistical analysis.
    pub fn packet_sizes_f64(&self) -> Vec<f64> {
        self.packet_sizes.iter().map(|&s| s as f64).collect()
    }

    /// Get inter-arrival times in microseconds as f64 for statistical analysis.
    pub fn inter_arrival_times_us(&self) -> Vec<f64> {
        self.inter_arrival_times
            .iter()
            .map(|d| d.as_micros() as f64)
            .collect()
    }
}

impl Default for TrafficPattern {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of a Kolmogorov-Smirnov test.
#[derive(Debug, Clone, Copy)]
pub struct KsTestResult {
    /// The K-S statistic (maximum absolute difference between CDFs)
    pub statistic: f64,

    /// The p-value (probability of observing this statistic under H0)
    pub p_value: f64,

    /// Sample size of the first sample
    pub n1: usize,

    /// Sample size of the second sample
    pub n2: usize,
}

impl KsTestResult {
    /// Check if the samples are indistinguishable at the given significance level.
    ///
    /// Returns true if p_value > significance_level, meaning we cannot reject
    /// the null hypothesis that the samples come from the same distribution.
    pub fn is_indistinguishable(&self, significance_level: f64) -> bool {
        self.p_value > significance_level
    }

    /// Check if samples are indistinguishable at the standard 0.05 level.
    pub fn is_indistinguishable_at_05(&self) -> bool {
        self.is_indistinguishable(0.05)
    }
}

/// Perform a two-sample Kolmogorov-Smirnov test.
///
/// This function compares two samples to determine if they come from
/// the same distribution. The null hypothesis is that they do.
///
/// # Arguments
///
/// * `sample1` - First sample of observations
/// * `sample2` - Second sample of observations
///
/// # Returns
///
/// A `KsTestResult` containing the K-S statistic and p-value.
///
/// # Example
///
/// ```rust
/// use botho::network::transport::fingerprint::kolmogorov_smirnov;
///
/// let botho_sizes = vec![100.0, 200.0, 150.0, 180.0, 220.0];
/// let webrtc_sizes = vec![110.0, 190.0, 160.0, 175.0, 210.0];
///
/// let result = kolmogorov_smirnov(&botho_sizes, &webrtc_sizes);
/// println!("K-S statistic: {}, p-value: {}", result.statistic, result.p_value);
///
/// if result.is_indistinguishable_at_05() {
///     println!("Samples are statistically indistinguishable");
/// }
/// ```
pub fn kolmogorov_smirnov(sample1: &[f64], sample2: &[f64]) -> KsTestResult {
    let n1 = sample1.len();
    let n2 = sample2.len();

    if n1 == 0 || n2 == 0 {
        return KsTestResult {
            statistic: 0.0,
            p_value: 1.0,
            n1,
            n2,
        };
    }

    // Sort both samples
    let mut sorted1: Vec<f64> = sample1.to_vec();
    let mut sorted2: Vec<f64> = sample2.to_vec();
    sorted1.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    sorted2.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    // Compute the K-S statistic (maximum absolute difference between CDFs)
    let mut d_max = 0.0f64;

    // Merge and walk through both distributions
    let mut i = 0usize;
    let mut j = 0usize;
    let n1f = n1 as f64;
    let n2f = n2 as f64;

    while i < n1 || j < n2 {
        let cdf1 = i as f64 / n1f;
        let cdf2 = j as f64 / n2f;
        let d = (cdf1 - cdf2).abs();
        d_max = d_max.max(d);

        // Advance the pointer with the smaller value
        if i < n1 && (j >= n2 || sorted1[i] <= sorted2[j]) {
            i += 1;
        } else {
            j += 1;
        }

        // Also check after advancing
        let cdf1_new = i as f64 / n1f;
        let cdf2_new = j as f64 / n2f;
        let d_new = (cdf1_new - cdf2_new).abs();
        d_max = d_max.max(d_new);
    }

    // Calculate p-value using asymptotic approximation
    // For large samples, use the asymptotic formula
    let en = (n1f * n2f / (n1f + n2f)).sqrt();
    let lambda = (en + 0.12 + 0.11 / en) * d_max;

    // Kolmogorov distribution approximation
    let p_value = kolmogorov_p_value(lambda);

    KsTestResult {
        statistic: d_max,
        p_value,
        n1,
        n2,
    }
}

/// Calculate the p-value from the Kolmogorov distribution.
///
/// Uses the asymptotic formula for the complementary CDF of the
/// Kolmogorov distribution.
fn kolmogorov_p_value(lambda: f64) -> f64 {
    if lambda < 0.0 {
        return 1.0;
    }
    if lambda == 0.0 {
        return 1.0;
    }
    if lambda >= 10.0 {
        return 0.0;
    }

    // Asymptotic series expansion
    // P(K > lambda) = 2 * sum_{j=1}^{inf} (-1)^{j-1} * exp(-2 * j^2 * lambda^2)
    let lambda_sq = lambda * lambda;
    let mut sum = 0.0;
    let mut sign = 1.0;

    for j in 1..=100 {
        let jf = j as f64;
        let term = sign * (-2.0 * jf * jf * lambda_sq).exp();
        sum += term;
        sign = -sign;

        // Convergence check
        if term.abs() < 1e-15 {
            break;
        }
    }

    (2.0 * sum).clamp(0.0, 1.0)
}

/// Result of a single fingerprinting test.
#[derive(Debug, Clone)]
pub struct TestResult {
    /// Name of the test
    pub name: String,

    /// K-S test result
    pub ks_result: KsTestResult,

    /// Whether the test passed (samples indistinguishable at alpha = 0.05)
    pub passed: bool,

    /// Additional diagnostic information
    pub diagnostics: String,
}

/// Complete fingerprinting test suite results.
#[derive(Debug, Clone)]
pub struct FullTestResult {
    /// Packet size distribution test
    pub packet_sizes: TestResult,

    /// Inter-arrival timing test
    pub timing: TestResult,

    /// Flow characteristics test
    pub flow: TestResult,

    /// Overall pass/fail (all tests must pass)
    pub all_passed: bool,
}

/// Test suite for fingerprinting resistance.
///
/// This suite compares botho traffic patterns against reference WebRTC
/// traffic to verify statistical indistinguishability.
pub struct FingerprintTests {
    /// Reference WebRTC traffic patterns for comparison
    reference_patterns: Vec<TrafficPattern>,

    /// Significance level for statistical tests (default: 0.05)
    significance_level: f64,
}

impl FingerprintTests {
    /// Create a new fingerprint test suite with reference patterns.
    pub fn new(reference_patterns: Vec<TrafficPattern>) -> Self {
        Self {
            reference_patterns,
            significance_level: 0.05,
        }
    }

    /// Set a custom significance level for tests.
    pub fn with_significance_level(mut self, level: f64) -> Self {
        self.significance_level = level;
        self
    }

    /// Test packet size distribution.
    ///
    /// Compares the packet size distribution of botho traffic against
    /// reference WebRTC traffic using the K-S test.
    pub fn test_packet_sizes(&self, botho: &TrafficPattern) -> TestResult {
        // Aggregate reference packet sizes
        let reference_sizes: Vec<f64> = self
            .reference_patterns
            .iter()
            .flat_map(|p| p.packet_sizes_f64())
            .collect();

        let botho_sizes = botho.packet_sizes_f64();

        let ks_result = kolmogorov_smirnov(&botho_sizes, &reference_sizes);
        let passed = ks_result.is_indistinguishable(self.significance_level);

        let diagnostics = format!(
            "Botho avg: {:.1} bytes, Reference avg: {:.1} bytes, Botho n={}, Reference n={}",
            botho.average_packet_size(),
            if reference_sizes.is_empty() {
                0.0
            } else {
                reference_sizes.iter().sum::<f64>() / reference_sizes.len() as f64
            },
            botho_sizes.len(),
            reference_sizes.len()
        );

        TestResult {
            name: "Packet Size Distribution".to_string(),
            ks_result,
            passed,
            diagnostics,
        }
    }

    /// Test timing patterns.
    ///
    /// Compares inter-arrival time distributions to detect timing-based
    /// fingerprinting.
    pub fn test_timing(&self, botho: &TrafficPattern) -> TestResult {
        // Aggregate reference inter-arrival times
        let reference_times: Vec<f64> = self
            .reference_patterns
            .iter()
            .flat_map(|p| p.inter_arrival_times_us())
            .collect();

        let botho_times = botho.inter_arrival_times_us();

        let ks_result = kolmogorov_smirnov(&botho_times, &reference_times);
        let passed = ks_result.is_indistinguishable(self.significance_level);

        let diagnostics = format!(
            "Botho avg IAT: {:.1} µs, Reference avg IAT: {:.1} µs",
            botho.average_inter_arrival_us(),
            if reference_times.is_empty() {
                0.0
            } else {
                reference_times.iter().sum::<f64>() / reference_times.len() as f64
            }
        );

        TestResult {
            name: "Timing Patterns".to_string(),
            ks_result,
            passed,
            diagnostics,
        }
    }

    /// Test flow characteristics.
    ///
    /// Compares overall flow characteristics including throughput
    /// and packet count ratios.
    pub fn test_flow(&self, botho: &TrafficPattern) -> TestResult {
        // For flow analysis, we compare normalized metrics across patterns
        // Create a synthetic distribution from flow characteristics
        let reference_throughputs: Vec<f64> = self
            .reference_patterns
            .iter()
            .map(|p| p.throughput_bps())
            .collect();

        // For a single botho pattern, we can't do a proper K-S test
        // Instead, check if botho throughput is within the reference range
        let botho_throughput = botho.throughput_bps();

        // Use a synthetic K-S test by comparing the botho throughput
        // against the reference distribution using a one-sample approach
        let (ks_result, passed) = if reference_throughputs.is_empty() {
            (
                KsTestResult {
                    statistic: 0.0,
                    p_value: 1.0,
                    n1: 1,
                    n2: 0,
                },
                true, // No reference = can't fail
            )
        } else {
            // Check if botho is within 2 standard deviations of reference mean
            let mean: f64 =
                reference_throughputs.iter().sum::<f64>() / reference_throughputs.len() as f64;
            let variance: f64 = reference_throughputs
                .iter()
                .map(|x| (x - mean).powi(2))
                .sum::<f64>()
                / reference_throughputs.len() as f64;
            let std_dev = variance.sqrt();

            let z_score = if std_dev > 0.0 {
                (botho_throughput - mean).abs() / std_dev
            } else {
                0.0
            };

            // Convert z-score to approximate p-value (two-tailed)
            let p_value = 2.0 * (1.0 - standard_normal_cdf(z_score));

            (
                KsTestResult {
                    statistic: z_score,
                    p_value,
                    n1: 1,
                    n2: reference_throughputs.len(),
                },
                p_value > self.significance_level,
            )
        };

        let diagnostics = format!(
            "Botho throughput: {:.1} B/s, Reference patterns: {}",
            botho_throughput,
            self.reference_patterns.len()
        );

        TestResult {
            name: "Flow Characteristics".to_string(),
            ks_result,
            passed,
            diagnostics,
        }
    }

    /// Run all fingerprinting tests.
    ///
    /// Returns a comprehensive result including all individual tests
    /// and an overall pass/fail status.
    pub fn run_all(&self, botho: &TrafficPattern) -> FullTestResult {
        let packet_sizes = self.test_packet_sizes(botho);
        let timing = self.test_timing(botho);
        let flow = self.test_flow(botho);

        let all_passed = packet_sizes.passed && timing.passed && flow.passed;

        FullTestResult {
            packet_sizes,
            timing,
            flow,
            all_passed,
        }
    }
}

/// Standard normal cumulative distribution function.
///
/// Approximation using the error function.
fn standard_normal_cdf(x: f64) -> f64 {
    0.5 * (1.0 + erf(x / std::f64::consts::SQRT_2))
}

/// Error function approximation.
///
/// Uses Abramowitz and Stegun approximation 7.1.26.
fn erf(x: f64) -> f64 {
    // Constants
    const A1: f64 = 0.254829592;
    const A2: f64 = -0.284496736;
    const A3: f64 = 1.421413741;
    const A4: f64 = -1.453152027;
    const A5: f64 = 1.061405429;
    const P: f64 = 0.3275911;

    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let x = x.abs();

    let t = 1.0 / (1.0 + P * x);
    let y = 1.0 - (((((A5 * t + A4) * t) + A3) * t + A2) * t + A1) * t * (-x * x).exp();

    sign * y
}

/// Generate synthetic WebRTC-like reference traffic pattern.
///
/// This creates a pattern that mimics typical WebRTC video call characteristics
/// for testing purposes when real capture data is not available.
#[cfg(test)]
pub fn generate_synthetic_webrtc_pattern(packet_count: usize) -> TrafficPattern {
    use rand::Rng;
    use rand_distr::{Distribution, LogNormal, Normal};

    let mut rng = rand::thread_rng();
    let mut pattern = TrafficPattern::new();

    // WebRTC video typically has bimodal packet sizes:
    // - Small packets (~100-200 bytes) for audio and control
    // - Larger packets (~800-1200 bytes) for video keyframes and data

    let small_size_dist = Normal::new(150.0, 30.0).unwrap();
    let large_size_dist = Normal::new(1000.0, 200.0).unwrap();

    // Inter-arrival times follow a log-normal distribution
    // Mean ~20ms for 50 fps video, with some variation
    let iat_dist = LogNormal::new(10.0, 1.0).unwrap(); // ~20ms mean

    for i in 0..packet_count {
        // 30% small packets (audio/control), 70% larger (video)
        let size = if rng.gen_bool(0.3) {
            let sample: f64 = small_size_dist.sample(&mut rng);
            sample.max(50.0) as usize
        } else {
            let sample: f64 = large_size_dist.sample(&mut rng);
            sample.max(200.0).min(1400.0) as usize
        };

        let iat = if i > 0 {
            let iat_us: f64 = iat_dist.sample(&mut rng);
            Some(Duration::from_micros(iat_us.max(1000.0).min(100_000.0) as u64))
        } else {
            None
        };

        // Alternate direction roughly 60% outbound
        if rng.gen_bool(0.6) {
            pattern.record_outbound(size, iat);
        } else {
            pattern.record_inbound(size, iat);
        }
    }

    // Set a realistic duration based on packet count and average IAT
    let avg_iat_ms = 20.0;
    let duration_ms = packet_count as f64 * avg_iat_ms;
    pattern.set_duration(Duration::from_millis(duration_ms as u64));

    pattern
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_traffic_pattern_recording() {
        let mut pattern = TrafficPattern::new();

        pattern.record_outbound(100, None);
        pattern.record_inbound(200, Some(Duration::from_millis(10)));
        pattern.record_outbound(150, Some(Duration::from_millis(15)));

        assert_eq!(pattern.packet_count(), 3);
        assert_eq!(pattern.bytes_sent, 250);
        assert_eq!(pattern.bytes_received, 200);
        assert_eq!(pattern.total_bytes(), 450);
        assert_eq!(pattern.inter_arrival_times.len(), 2);
    }

    #[test]
    fn test_traffic_pattern_averages() {
        let mut pattern = TrafficPattern::new();

        pattern.record_outbound(100, None);
        pattern.record_outbound(200, Some(Duration::from_micros(1000)));
        pattern.record_outbound(300, Some(Duration::from_micros(2000)));

        assert!((pattern.average_packet_size() - 200.0).abs() < 0.01);
        assert!((pattern.average_inter_arrival_us() - 1500.0).abs() < 0.01);
    }

    #[test]
    fn test_ks_identical_samples() {
        let sample1 = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let sample2 = vec![1.0, 2.0, 3.0, 4.0, 5.0];

        let result = kolmogorov_smirnov(&sample1, &sample2);

        assert_eq!(result.statistic, 0.0);
        assert!(result.p_value >= 0.99); // Should be 1.0 or very close
        assert!(result.is_indistinguishable_at_05());
    }

    #[test]
    fn test_ks_different_samples() {
        // Clearly different distributions
        let sample1 = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
        let sample2 = vec![
            100.0, 110.0, 120.0, 130.0, 140.0, 150.0, 160.0, 170.0, 180.0, 190.0,
        ];

        let result = kolmogorov_smirnov(&sample1, &sample2);

        assert!(result.statistic > 0.5);
        assert!(result.p_value < 0.05);
        assert!(!result.is_indistinguishable_at_05());
    }

    #[test]
    fn test_ks_similar_samples() {
        // Similar distributions with some noise
        let sample1: Vec<f64> = (0..50).map(|i| i as f64 * 2.0 + 1.0).collect();
        let sample2: Vec<f64> = (0..50).map(|i| i as f64 * 2.0 + 1.5).collect();

        let result = kolmogorov_smirnov(&sample1, &sample2);

        // With similar distributions, should be indistinguishable
        assert!(result.is_indistinguishable_at_05());
    }

    #[test]
    fn test_ks_empty_samples() {
        let sample1: Vec<f64> = vec![];
        let sample2 = vec![1.0, 2.0, 3.0];

        let result = kolmogorov_smirnov(&sample1, &sample2);

        assert_eq!(result.p_value, 1.0);
        assert!(result.is_indistinguishable_at_05());
    }

    #[test]
    fn test_fingerprint_tests_packet_sizes() {
        // Generate reference patterns
        let reference = vec![
            generate_synthetic_webrtc_pattern(100),
            generate_synthetic_webrtc_pattern(100),
        ];

        // Generate a similar botho pattern
        let botho = generate_synthetic_webrtc_pattern(100);

        let tests = FingerprintTests::new(reference);
        let result = tests.test_packet_sizes(&botho);

        // Synthetic patterns should be indistinguishable from each other
        // (they use the same generation function)
        println!(
            "Packet sizes: statistic={}, p_value={}, passed={}",
            result.ks_result.statistic, result.ks_result.p_value, result.passed
        );
        println!("Diagnostics: {}", result.diagnostics);
    }

    #[test]
    fn test_fingerprint_tests_timing() {
        let reference = vec![generate_synthetic_webrtc_pattern(100)];
        let botho = generate_synthetic_webrtc_pattern(100);

        let tests = FingerprintTests::new(reference);
        let result = tests.test_timing(&botho);

        println!(
            "Timing: statistic={}, p_value={}, passed={}",
            result.ks_result.statistic, result.ks_result.p_value, result.passed
        );
    }

    #[test]
    fn test_fingerprint_tests_full_suite() {
        let reference = vec![
            generate_synthetic_webrtc_pattern(200),
            generate_synthetic_webrtc_pattern(200),
            generate_synthetic_webrtc_pattern(200),
        ];
        let botho = generate_synthetic_webrtc_pattern(200);

        let tests = FingerprintTests::new(reference);
        let result = tests.run_all(&botho);

        println!("Full test results:");
        println!(
            "  Packet sizes: p={}, passed={}",
            result.packet_sizes.ks_result.p_value, result.packet_sizes.passed
        );
        println!(
            "  Timing: p={}, passed={}",
            result.timing.ks_result.p_value, result.timing.passed
        );
        println!(
            "  Flow: p={}, passed={}",
            result.flow.ks_result.p_value, result.flow.passed
        );
        println!("  All passed: {}", result.all_passed);
    }

    #[test]
    fn test_generate_synthetic_pattern() {
        let pattern = generate_synthetic_webrtc_pattern(100);

        assert_eq!(pattern.packet_count(), 100);
        assert!(pattern.bytes_sent > 0);
        assert!(pattern.bytes_received > 0);
        assert_eq!(pattern.inter_arrival_times.len(), 99); // n-1 inter-arrival times

        // Check that sizes are reasonable (WebRTC-like)
        let avg_size = pattern.average_packet_size();
        assert!(avg_size > 100.0 && avg_size < 1500.0);
    }

    #[test]
    fn test_kolmogorov_p_value_bounds() {
        // Lambda = 0 should give p = 1
        assert!((kolmogorov_p_value(0.0) - 1.0).abs() < 0.001);

        // Very large lambda should give p close to 0
        assert!(kolmogorov_p_value(5.0) < 0.01);

        // p-value should always be in [0, 1]
        for lambda in [0.1, 0.5, 1.0, 1.5, 2.0, 3.0] {
            let p = kolmogorov_p_value(lambda);
            assert!(p >= 0.0 && p <= 1.0, "p={} for lambda={}", p, lambda);
        }
    }

    #[test]
    fn test_erf_accuracy() {
        // Test against known values
        assert!((erf(0.0)).abs() < 0.001);
        assert!((erf(1.0) - 0.8427).abs() < 0.001);
        assert!((erf(-1.0) + 0.8427).abs() < 0.001);
    }

    #[test]
    fn test_standard_normal_cdf() {
        // CDF(0) should be 0.5
        assert!((standard_normal_cdf(0.0) - 0.5).abs() < 0.001);

        // CDF should be monotonically increasing
        assert!(standard_normal_cdf(-1.0) < standard_normal_cdf(0.0));
        assert!(standard_normal_cdf(0.0) < standard_normal_cdf(1.0));
    }
}
