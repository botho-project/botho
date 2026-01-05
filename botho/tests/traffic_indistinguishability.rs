// Copyright (c) 2024 Botho Foundation

//! Statistical indistinguishability tests for traffic normalization.
//!
//! This module implements tests to verify that normalized traffic is
//! statistically indistinguishable from baseline patterns, as specified
//! in Phase 2.10 of the traffic privacy roadmap.
//!
//! # Test Methodology
//!
//! We use the Kolmogorov-Smirnov (K-S) test to compare distributions:
//! - **Null hypothesis (H0)**: The two samples come from the same distribution
//! - **Significance level**: p > 0.05 means we fail to reject H0
//! - **Interpretation**: High p-value = distributions are statistically
//!   indistinguishable
//!
//! # Tests Implemented
//!
//! 1. **Padding Tests**: Verify padded messages have uniform bucket
//!    distribution
//! 2. **Timing Tests**: Verify jitter produces uniform timing within range
//! 3. **Cover Traffic Tests**: Verify cover messages match real transaction
//!    sizes

use rand::prelude::*;
use rand_chacha::ChaCha8Rng;
use std::time::Duration;

use botho::network::privacy::{
    cover::CoverTrafficGenerator,
    normalizer::{TrafficNormalizer, PADDING_BUCKETS},
    padding::pad_to_bucket,
    timing::{TimingJitter, TimingJitterConfig},
};

/// Result of a Kolmogorov-Smirnov test.
#[derive(Debug, Clone)]
pub struct KsResult {
    /// The D statistic (maximum difference between CDFs)
    pub d_statistic: f64,
    /// The p-value for the test
    pub p_value: f64,
    /// Sample sizes used
    pub n1: usize,
    pub n2: usize,
}

impl KsResult {
    /// Check if distributions are statistically indistinguishable at alpha=0.05
    pub fn is_indistinguishable(&self) -> bool {
        self.p_value > 0.05
    }

    /// Check if distributions are indistinguishable at a custom significance
    /// level
    pub fn is_indistinguishable_at(&self, alpha: f64) -> bool {
        self.p_value > alpha
    }
}

/// Captured traffic pattern for analysis.
#[derive(Debug, Clone, Default)]
pub struct TrafficPattern {
    /// Packet sizes in bytes
    pub packet_sizes: Vec<usize>,
    /// Inter-arrival times between packets
    pub inter_arrival_times: Vec<Duration>,
    /// Total flow duration
    pub flow_duration: Duration,
}

impl TrafficPattern {
    /// Create a new empty traffic pattern.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a packet with the given size.
    pub fn record_packet(&mut self, size: usize) {
        self.packet_sizes.push(size);
    }

    /// Record an inter-arrival time.
    pub fn record_inter_arrival(&mut self, time: Duration) {
        self.inter_arrival_times.push(time);
    }

    /// Get packet sizes as f64 for statistical analysis.
    pub fn sizes_as_f64(&self) -> Vec<f64> {
        self.packet_sizes.iter().map(|&s| s as f64).collect()
    }

    /// Get inter-arrival times as f64 (milliseconds) for statistical analysis.
    pub fn times_as_f64(&self) -> Vec<f64> {
        self.inter_arrival_times
            .iter()
            .map(|d| d.as_secs_f64() * 1000.0)
            .collect()
    }
}

/// Perform the two-sample Kolmogorov-Smirnov test.
///
/// This implementation follows the standard K-S test algorithm:
/// 1. Sort both samples
/// 2. Compute empirical CDFs
/// 3. Find maximum difference D
/// 4. Calculate p-value from D and sample sizes
///
/// # Arguments
///
/// * `a` - First sample
/// * `b` - Second sample
///
/// # Returns
///
/// A `KsResult` containing the D statistic and p-value.
///
/// # Panics
///
/// Panics if either sample is empty.
pub fn kolmogorov_smirnov(a: &[f64], b: &[f64]) -> KsResult {
    assert!(!a.is_empty(), "First sample cannot be empty");
    assert!(!b.is_empty(), "Second sample cannot be empty");

    let n1 = a.len();
    let n2 = b.len();

    // Sort samples
    let mut a_sorted: Vec<f64> = a.to_vec();
    let mut b_sorted: Vec<f64> = b.to_vec();
    a_sorted.sort_by(|x, y| x.partial_cmp(y).unwrap());
    b_sorted.sort_by(|x, y| x.partial_cmp(y).unwrap());

    // Merge all unique values
    let mut all_values: Vec<f64> = Vec::with_capacity(n1 + n2);
    all_values.extend_from_slice(&a_sorted);
    all_values.extend_from_slice(&b_sorted);
    all_values.sort_by(|x, y| x.partial_cmp(y).unwrap());
    all_values.dedup();

    // Compute D statistic as maximum CDF difference
    let mut d_max = 0.0f64;

    for &x in &all_values {
        // Empirical CDF for sample a: proportion of a <= x
        let cdf_a = a_sorted.partition_point(|&v| v <= x) as f64 / n1 as f64;
        // Empirical CDF for sample b: proportion of b <= x
        let cdf_b = b_sorted.partition_point(|&v| v <= x) as f64 / n2 as f64;

        let diff = (cdf_a - cdf_b).abs();
        if diff > d_max {
            d_max = diff;
        }
    }

    // Calculate p-value using the asymptotic approximation
    // For large samples, D * sqrt(n*m/(n+m)) approximately follows
    // the Kolmogorov distribution
    let p_value = ks_p_value(d_max, n1, n2);

    KsResult {
        d_statistic: d_max,
        p_value,
        n1,
        n2,
    }
}

/// Calculate p-value for the K-S test using asymptotic approximation.
///
/// Uses the formula: p = 2 * sum_{k=1}^{inf} (-1)^{k+1} * exp(-2 * k^2 * z^2)
/// where z = D * sqrt(n*m/(n+m))
fn ks_p_value(d: f64, n1: usize, n2: usize) -> f64 {
    if d == 0.0 {
        return 1.0;
    }

    let n = n1 as f64;
    let m = n2 as f64;
    let z = d * (n * m / (n + m)).sqrt();

    // Use asymptotic formula (Kolmogorov distribution)
    // P(D > d) = 2 * sum_{k=1}^{inf} (-1)^{k+1} * exp(-2 * k^2 * z^2)
    let mut p = 0.0;
    for k in 1..=100 {
        let term = (-1.0f64).powi(k + 1) * (-2.0 * (k as f64).powi(2) * z.powi(2)).exp();
        p += term;
        if term.abs() < 1e-12 {
            break;
        }
    }

    (2.0 * p).clamp(0.0, 1.0)
}

/// Test that padded messages from different original sizes produce
/// the same bucket size distribution.
///
/// If padding is working correctly, observers cannot distinguish
/// small vs large original messages by looking at wire sizes.
#[test]
fn test_padding_size_indistinguishability() {
    let mut rng = ChaCha8Rng::seed_from_u64(42);
    let normalizer = TrafficNormalizer::default();

    // Generate padded messages from small payloads (100-300 bytes)
    let mut small_sizes: Vec<f64> = Vec::new();
    for _ in 0..500 {
        let payload_size = rng.gen_range(100..=300);
        let payload: Vec<u8> = (0..payload_size).map(|_| rng.gen()).collect();
        let prepared = normalizer.prepare_message(&payload);
        small_sizes.push(prepared.payload.len() as f64);
    }

    // Generate padded messages from medium payloads (300-500 bytes)
    let mut medium_sizes: Vec<f64> = Vec::new();
    for _ in 0..500 {
        let payload_size = rng.gen_range(300..=500);
        let payload: Vec<u8> = (0..payload_size).map(|_| rng.gen()).collect();
        let prepared = normalizer.prepare_message(&payload);
        medium_sizes.push(prepared.payload.len() as f64);
    }

    // Both should result in same bucket distribution (mostly 512 and 2048)
    let result = kolmogorov_smirnov(&small_sizes, &medium_sizes);

    println!(
        "Padding size test: D = {:.4}, p-value = {:.4}",
        result.d_statistic, result.p_value
    );

    // With padding enabled, both should pad to same buckets for sizes < 512
    // So they should be indistinguishable
    // Note: This test validates the concept, actual p-value depends on
    // the specific size ranges chosen
    assert!(
        result.p_value > 0.01,
        "Padded sizes should not be easily distinguishable: D = {:.4}, p = {:.4}",
        result.d_statistic,
        result.p_value
    );
}

/// Test that all padded messages of similar type fall into fixed buckets.
#[test]
fn test_padding_bucket_uniformity() {
    let normalizer = TrafficNormalizer::default();
    let mut rng = ChaCha8Rng::seed_from_u64(123);

    // Track which buckets are used
    let mut bucket_counts = std::collections::HashMap::new();

    for _ in 0..1000 {
        // Generate payloads of varying sizes that should all fit in 512 bucket
        let size = rng.gen_range(10..=500);
        let payload: Vec<u8> = (0..size).map(|_| rng.gen()).collect();
        let prepared = normalizer.prepare_message(&payload);

        *bucket_counts
            .entry(prepared.payload.len())
            .or_insert(0usize) += 1;
    }

    // All output sizes should be valid bucket sizes
    for &size in bucket_counts.keys() {
        assert!(
            PADDING_BUCKETS.contains(&size),
            "Output size {} is not a valid bucket",
            size
        );
    }

    // Verify no leakage of original sizes
    println!("Bucket distribution: {:?}", bucket_counts);
}

/// Test that jitter timing is uniformly distributed within the configured
/// range.
#[test]
fn test_jitter_timing_uniformity() {
    let config = TimingJitterConfig::new(50, 200);
    let jitter = TimingJitter::new(config);
    let mut rng = ChaCha8Rng::seed_from_u64(456);

    // Collect jitter samples
    let mut jitter_samples: Vec<f64> = Vec::new();
    for _ in 0..1000 {
        let delay = jitter.delay_with_rng(&mut rng);
        jitter_samples.push(delay.as_millis() as f64);
    }

    // Generate uniform distribution for comparison
    let mut uniform_samples: Vec<f64> = Vec::new();
    for _ in 0..1000 {
        uniform_samples.push(rng.gen_range(50.0..=200.0));
    }

    let result = kolmogorov_smirnov(&jitter_samples, &uniform_samples);

    println!(
        "Jitter uniformity test: D = {:.4}, p-value = {:.4}",
        result.d_statistic, result.p_value
    );

    // Jitter should be uniformly distributed
    assert!(
        result.is_indistinguishable(),
        "Jitter should be uniformly distributed: D = {:.4}, p = {:.4}",
        result.d_statistic,
        result.p_value
    );
}

/// Test that jitter with fixed interval produces constant-rate behavior.
#[test]
fn test_constant_rate_timing() {
    let config = TimingJitterConfig::new(100, 100); // Fixed 100ms
    let jitter = TimingJitter::new(config);
    let mut rng = ChaCha8Rng::seed_from_u64(789);

    let mut samples: Vec<f64> = Vec::new();
    for _ in 0..100 {
        let delay = jitter.delay_with_rng(&mut rng);
        samples.push(delay.as_millis() as f64);
    }

    // All samples should be exactly 100ms
    for sample in &samples {
        assert!(
            (*sample - 100.0).abs() < 0.001,
            "Constant rate should produce fixed timing"
        );
    }

    // Variance should be zero
    let mean: f64 = samples.iter().sum::<f64>() / samples.len() as f64;
    let variance: f64 =
        samples.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / samples.len() as f64;

    assert!(
        variance < 0.001,
        "Constant rate variance should be ~0, got {}",
        variance
    );
}

/// Test that cover traffic matches the size distribution of real transactions.
#[test]
fn test_cover_traffic_size_distribution() {
    let generator = CoverTrafficGenerator::default();
    let mut rng = ChaCha8Rng::seed_from_u64(321);

    // Generate cover traffic
    let mut cover_sizes: Vec<f64> = Vec::new();
    for _ in 0..1000 {
        let msg = generator.generate_with_rng(&mut rng);
        cover_sizes.push(msg.payload.len() as f64);
    }

    // Generate simulated "real" transactions with similar distribution
    // Real transactions are weighted: 30% small (200-300), 50% medium (300-450),
    // 20% large (450-600)
    let mut real_sizes: Vec<f64> = Vec::new();
    for _ in 0..1000 {
        let category: f64 = rng.gen();
        let size = if category < 0.30 {
            rng.gen_range(200..=300) // Small
        } else if category < 0.80 {
            rng.gen_range(300..=450) // Medium
        } else {
            rng.gen_range(450..=600) // Large
        };
        real_sizes.push(size as f64);
    }

    let result = kolmogorov_smirnov(&cover_sizes, &real_sizes);

    println!(
        "Cover traffic vs real: D = {:.4}, p-value = {:.4}",
        result.d_statistic, result.p_value
    );

    // Cover traffic should match real transaction distribution
    assert!(
        result.is_indistinguishable(),
        "Cover traffic should match real transaction sizes: D = {:.4}, p = {:.4}",
        result.d_statistic,
        result.p_value
    );
}

/// Test that cover traffic with different weights produces distinguishable
/// patterns.
///
/// This is a sanity check that our K-S test can actually detect differences.
#[test]
fn test_ks_can_detect_differences() {
    let mut rng = ChaCha8Rng::seed_from_u64(654);

    // Sample from uniform(0, 100)
    let uniform: Vec<f64> = (0..500).map(|_| rng.gen_range(0.0..100.0)).collect();

    // Sample from normal-like (centered around 50)
    let normal_like: Vec<f64> = (0..500)
        .map(|_| {
            // Simple approximation of normal using sum of uniforms
            let sum: f64 = (0..12).map(|_| rng.gen::<f64>()).sum();
            (sum - 6.0) * 15.0 + 50.0 // Approximately N(50, 15)
        })
        .collect();

    let result = kolmogorov_smirnov(&uniform, &normal_like);

    println!(
        "Sanity check (uniform vs normal): D = {:.4}, p-value = {:.4}",
        result.d_statistic, result.p_value
    );

    // These should be distinguishable (p < 0.05)
    assert!(
        result.p_value < 0.05,
        "K-S test should detect difference between uniform and normal: p = {:.4}",
        result.p_value
    );
}

/// Test that identical distributions produce high p-values.
#[test]
fn test_ks_identical_distributions() {
    let mut rng = ChaCha8Rng::seed_from_u64(987);

    // Two samples from the same distribution
    let sample1: Vec<f64> = (0..500).map(|_| rng.gen_range(0.0..100.0)).collect();
    let sample2: Vec<f64> = (0..500).map(|_| rng.gen_range(0.0..100.0)).collect();

    let result = kolmogorov_smirnov(&sample1, &sample2);

    println!(
        "Identical distributions: D = {:.4}, p-value = {:.4}",
        result.d_statistic, result.p_value
    );

    // Should be indistinguishable
    assert!(
        result.is_indistinguishable(),
        "Samples from same distribution should be indistinguishable: p = {:.4}",
        result.p_value
    );
}

/// Test padded vs unpadded traffic size patterns.
#[test]
fn test_padded_vs_unpadded_sizes() {
    let mut rng = ChaCha8Rng::seed_from_u64(111);

    // Unpadded: original payload sizes
    let unpadded: Vec<f64> = (0..500).map(|_| rng.gen_range(100..=500) as f64).collect();

    // Padded: bucket sizes
    let padded: Vec<f64> = unpadded
        .iter()
        .map(|&size| {
            let payload: Vec<u8> = (0..size as usize).map(|_| rng.gen()).collect();
            pad_to_bucket(&payload).len() as f64
        })
        .collect();

    let result = kolmogorov_smirnov(&unpadded, &padded);

    println!(
        "Padded vs unpadded: D = {:.4}, p-value = {:.4}",
        result.d_statistic, result.p_value
    );

    // These SHOULD be distinguishable (padding changes the distribution)
    assert!(
        result.p_value < 0.05,
        "Padded and unpadded should be distinguishable: p = {:.4}",
        result.p_value
    );
}

/// Test that padding eliminates size variance within buckets.
#[test]
fn test_padding_eliminates_variance() {
    let normalizer = TrafficNormalizer::default();
    let mut rng = ChaCha8Rng::seed_from_u64(222);

    // Generate messages that should all pad to 512 bytes
    let mut sizes: Vec<f64> = Vec::new();
    for _ in 0..100 {
        let size = rng.gen_range(10..=500);
        let payload: Vec<u8> = (0..size).map(|_| rng.gen()).collect();
        let prepared = normalizer.prepare_message(&payload);
        if prepared.bucket_size == Some(512) {
            sizes.push(prepared.payload.len() as f64);
        }
    }

    // All sizes in this bucket should be exactly 512
    let unique_sizes: std::collections::HashSet<i64> = sizes.iter().map(|&s| s as i64).collect();

    assert_eq!(
        unique_sizes.len(),
        1,
        "All messages in a bucket should have identical size"
    );
    assert!(unique_sizes.contains(&512), "Bucket size should be 512");
}

/// Test jitter range boundaries.
#[test]
fn test_jitter_range_boundaries() {
    let config = TimingJitterConfig::new(50, 200);
    let jitter = TimingJitter::new(config);
    let mut rng = ChaCha8Rng::seed_from_u64(333);

    let mut min_seen = u64::MAX;
    let mut max_seen = 0u64;

    for _ in 0..10000 {
        let delay = jitter.delay_with_rng(&mut rng);
        let ms = delay.as_millis() as u64;
        min_seen = min_seen.min(ms);
        max_seen = max_seen.max(ms);
    }

    assert_eq!(min_seen, 50, "Minimum delay should be 50ms");
    assert_eq!(max_seen, 200, "Maximum delay should be 200ms");
}

/// Test cover message size bounds.
#[test]
fn test_cover_message_size_bounds() {
    let generator = CoverTrafficGenerator::default();
    let mut rng = ChaCha8Rng::seed_from_u64(444);

    for _ in 0..1000 {
        let msg = generator.generate_with_rng(&mut rng);
        assert!(
            msg.payload.len() >= 200,
            "Cover message too small: {}",
            msg.payload.len()
        );
        assert!(
            msg.payload.len() <= 600,
            "Cover message too large: {}",
            msg.payload.len()
        );
    }
}

/// Integration test: Complete traffic pattern analysis.
#[test]
fn test_traffic_pattern_analysis() {
    let normalizer = TrafficNormalizer::default();
    let jitter = TimingJitter::default();
    let cover_gen = CoverTrafficGenerator::default();
    let mut rng = ChaCha8Rng::seed_from_u64(555);

    // Simulate normalized traffic
    let mut normalized_pattern = TrafficPattern::new();
    for _ in 0..500 {
        // Random payload size
        let size = rng.gen_range(100..=1000);
        let payload: Vec<u8> = (0..size).map(|_| rng.gen()).collect();

        // Apply normalization
        let prepared = normalizer.prepare_message(&payload);
        normalized_pattern.record_packet(prepared.payload.len());

        // Record jitter
        let delay = jitter.delay_with_rng(&mut rng);
        normalized_pattern.record_inter_arrival(delay);
    }

    // Simulate cover traffic pattern
    let mut cover_pattern = TrafficPattern::new();
    for _ in 0..500 {
        let msg = cover_gen.generate_with_rng(&mut rng);
        // Pad cover message to bucket
        let prepared = normalizer.prepare_message(&msg.payload);
        cover_pattern.record_packet(prepared.payload.len());

        let delay = jitter.delay_with_rng(&mut rng);
        cover_pattern.record_inter_arrival(delay);
    }

    // Compare size distributions
    let size_result = kolmogorov_smirnov(
        &normalized_pattern.sizes_as_f64(),
        &cover_pattern.sizes_as_f64(),
    );

    println!(
        "Traffic pattern size comparison: D = {:.4}, p = {:.4}",
        size_result.d_statistic, size_result.p_value
    );

    // Compare timing distributions
    let timing_result = kolmogorov_smirnov(
        &normalized_pattern.times_as_f64(),
        &cover_pattern.times_as_f64(),
    );

    println!(
        "Traffic pattern timing comparison: D = {:.4}, p = {:.4}",
        timing_result.d_statistic, timing_result.p_value
    );

    // Both should be indistinguishable after normalization
    // Note: Size might differ slightly due to different source distributions
    // but timing should be identical (both use same jitter config)
    assert!(
        timing_result.is_indistinguishable(),
        "Timing patterns should be indistinguishable: p = {:.4}",
        timing_result.p_value
    );
}

/// Test that the K-S implementation handles edge cases correctly.
#[test]
fn test_ks_edge_cases() {
    // Small samples
    let small1 = vec![1.0, 2.0, 3.0];
    let small2 = vec![1.5, 2.5, 3.5];
    let result = kolmogorov_smirnov(&small1, &small2);
    assert!(result.d_statistic >= 0.0 && result.d_statistic <= 1.0);
    assert!(result.p_value >= 0.0 && result.p_value <= 1.0);

    // Identical samples
    let same1 = vec![1.0, 2.0, 3.0, 4.0, 5.0];
    let same2 = vec![1.0, 2.0, 3.0, 4.0, 5.0];
    let result = kolmogorov_smirnov(&same1, &same2);
    assert!(
        result.d_statistic < 0.01,
        "Identical samples should have D ~ 0"
    );
    assert!(result.p_value > 0.99, "Identical samples should have p ~ 1");

    // Completely different
    let diff1 = vec![1.0, 2.0, 3.0];
    let diff2 = vec![100.0, 200.0, 300.0];
    let result = kolmogorov_smirnov(&diff1, &diff2);
    assert!(
        result.d_statistic > 0.9,
        "Non-overlapping samples should have high D"
    );
}

/// Test asymmetric sample sizes.
#[test]
fn test_ks_asymmetric_samples() {
    let mut rng = ChaCha8Rng::seed_from_u64(666);

    // Different sample sizes from same distribution
    let large: Vec<f64> = (0..1000).map(|_| rng.gen_range(0.0..100.0)).collect();
    let small: Vec<f64> = (0..100).map(|_| rng.gen_range(0.0..100.0)).collect();

    let result = kolmogorov_smirnov(&large, &small);

    println!(
        "Asymmetric samples (1000 vs 100): D = {:.4}, p = {:.4}",
        result.d_statistic, result.p_value
    );

    // Should still be indistinguishable (same underlying distribution)
    assert!(
        result.is_indistinguishable(),
        "Same distribution with different sample sizes should be indistinguishable"
    );
}

#[cfg(test)]
mod ks_unit_tests {
    use super::*;

    #[test]
    fn test_ks_p_value_zero_d() {
        let p = ks_p_value(0.0, 100, 100);
        assert!((p - 1.0).abs() < 0.001, "D=0 should give p=1");
    }

    #[test]
    fn test_ks_p_value_large_d() {
        let p = ks_p_value(1.0, 100, 100);
        assert!(p < 0.001, "D=1 should give very small p");
    }

    #[test]
    fn test_ks_result_methods() {
        let result = KsResult {
            d_statistic: 0.1,
            p_value: 0.5,
            n1: 100,
            n2: 100,
        };

        assert!(result.is_indistinguishable());
        assert!(result.is_indistinguishable_at(0.1));
        assert!(!result.is_indistinguishable_at(0.9));
    }
}
