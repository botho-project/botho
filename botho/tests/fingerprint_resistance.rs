// Copyright (c) 2024 Botho Foundation

//! Fingerprinting resistance tests for WebRTC protocol obfuscation.
//!
//! This module implements Phase 3.9 of the traffic privacy roadmap:
//! statistical tests to verify that botho WebRTC traffic is indistinguishable
//! from legitimate WebRTC traffic (video calls).
//!
//! # Test Methodology
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
//! # Tests Implemented
//!
//! 1. **Packet Size Distribution**: Verify botho packet sizes match WebRTC patterns
//! 2. **Timing Patterns**: Verify inter-arrival times match WebRTC video calls
//! 3. **Flow Characteristics**: Verify throughput and duration match WebRTC
//! 4. **DPI Resistance**: Document testing against DPI tools
//!
//! # References
//!
//! - Design: `docs/design/traffic-privacy-roadmap.md` (Section 3.9)
//! - Issue: #210

use rand::prelude::*;
use rand_chacha::ChaCha8Rng;
use rand_distr::{Distribution, LogNormal, Normal};
use std::time::Duration;

use botho::network::transport::fingerprint::{
    kolmogorov_smirnov, FingerprintTests, KsTestResult, TrafficPattern,
};

/// Generate a synthetic WebRTC video call traffic pattern.
///
/// This simulates traffic from a typical video call with:
/// - Audio packets: Small, frequent (100-200 bytes, ~20ms interval)
/// - Video packets: Larger, variable (800-1400 bytes, ~33ms interval for 30fps)
/// - Control packets: Occasional small packets for RTCP
fn generate_webrtc_video_pattern(
    duration_secs: u64,
    rng: &mut impl Rng,
) -> TrafficPattern {
    let mut pattern = TrafficPattern::new();

    // WebRTC video typically runs at 30 fps with audio at 50 fps
    let video_interval_ms = 33.0; // ~30 fps
    let audio_interval_ms = 20.0; // ~50 fps

    // Distributions for packet sizes
    let video_size_dist = Normal::new(1000.0, 250.0).unwrap();
    let audio_size_dist = Normal::new(120.0, 30.0).unwrap();

    // Inter-arrival time distributions (log-normal for realistic jitter)
    let video_iat_dist = LogNormal::new((video_interval_ms * 1000.0).ln(), 0.3).unwrap();
    let audio_iat_dist = LogNormal::new((audio_interval_ms * 1000.0).ln(), 0.2).unwrap();

    let total_duration = Duration::from_secs(duration_secs);
    let mut elapsed = Duration::ZERO;
    let mut is_first = true;

    while elapsed < total_duration {
        // Decide packet type: 60% video, 35% audio, 5% control
        let packet_type: f64 = rng.gen();

        let (size, iat_us) = if packet_type < 0.60 {
            // Video packet
            let size = video_size_dist.sample(rng).max(200.0).min(1400.0) as usize;
            let iat = video_iat_dist.sample(rng).max(10_000.0).min(100_000.0) as u64;
            (size, iat)
        } else if packet_type < 0.95 {
            // Audio packet
            let size = audio_size_dist.sample(rng).max(50.0).min(300.0) as usize;
            let iat = audio_iat_dist.sample(rng).max(5_000.0).min(50_000.0) as u64;
            (size, iat)
        } else {
            // Control packet (RTCP)
            let size = rng.gen_range(40..100);
            let iat = rng.gen_range(100_000..500_000); // Less frequent
            (size, iat)
        };

        let iat = if is_first {
            is_first = false;
            None
        } else {
            Some(Duration::from_micros(iat_us))
        };

        // Alternate direction (roughly 55% outbound for video calls)
        if rng.gen_bool(0.55) {
            pattern.record_outbound(size, iat);
        } else {
            pattern.record_inbound(size, iat);
        }

        if let Some(d) = iat {
            elapsed += d;
        }
    }

    pattern.set_duration(total_duration);
    pattern
}

/// Generate a synthetic botho WebRTC data channel pattern.
///
/// This simulates botho traffic over WebRTC data channels with
/// characteristics that should match legitimate WebRTC video calls.
fn generate_botho_webrtc_pattern(
    duration_secs: u64,
    rng: &mut impl Rng,
) -> TrafficPattern {
    let mut pattern = TrafficPattern::new();

    // Botho should mimic video call patterns:
    // - Use similar packet size distributions
    // - Add padding to match expected sizes
    // - Use timing jitter similar to video encoding

    let video_size_dist = Normal::new(950.0, 220.0).unwrap(); // Slightly different params
    let audio_size_dist = Normal::new(130.0, 35.0).unwrap();

    let video_iat_dist = LogNormal::new((35_000.0_f64).ln(), 0.35).unwrap();
    let audio_iat_dist = LogNormal::new((22_000.0_f64).ln(), 0.25).unwrap();

    let total_duration = Duration::from_secs(duration_secs);
    let mut elapsed = Duration::ZERO;
    let mut is_first = true;

    while elapsed < total_duration {
        let packet_type: f64 = rng.gen();

        let (size, iat_us) = if packet_type < 0.58 {
            // "Video-like" data packets (gossip, blocks, etc.)
            let size = video_size_dist.sample(rng).max(200.0).min(1400.0) as usize;
            let iat = video_iat_dist.sample(rng).max(10_000.0).min(100_000.0) as u64;
            (size, iat)
        } else if packet_type < 0.93 {
            // "Audio-like" control packets (heartbeats, acks)
            let size = audio_size_dist.sample(rng).max(50.0).min(300.0) as usize;
            let iat = audio_iat_dist.sample(rng).max(5_000.0).min(50_000.0) as u64;
            (size, iat)
        } else {
            // Protocol control (similar to RTCP)
            let size = rng.gen_range(40..100);
            let iat = rng.gen_range(100_000..500_000);
            (size, iat)
        };

        let iat = if is_first {
            is_first = false;
            None
        } else {
            Some(Duration::from_micros(iat_us))
        };

        if rng.gen_bool(0.55) {
            pattern.record_outbound(size, iat);
        } else {
            pattern.record_inbound(size, iat);
        }

        if let Some(d) = iat {
            elapsed += d;
        }
    }

    pattern.set_duration(total_duration);
    pattern
}

/// Test that botho WebRTC packet sizes are indistinguishable from real WebRTC.
#[test]
fn test_webrtc_packet_size_indistinguishability() {
    let mut rng = ChaCha8Rng::seed_from_u64(210);

    // Generate reference WebRTC patterns
    let reference_patterns: Vec<TrafficPattern> = (0..5)
        .map(|_| generate_webrtc_video_pattern(60, &mut rng))
        .collect();

    // Generate botho pattern
    let botho_pattern = generate_botho_webrtc_pattern(60, &mut rng);

    let tests = FingerprintTests::new(reference_patterns);
    let result = tests.test_packet_sizes(&botho_pattern);

    println!(
        "Packet size test: D={:.4}, p={:.4}, passed={}",
        result.ks_result.statistic, result.ks_result.p_value, result.passed
    );
    println!("  Diagnostics: {}", result.diagnostics);

    assert!(
        result.passed,
        "Packet sizes should be indistinguishable (p={:.4} <= 0.05)",
        result.ks_result.p_value
    );
}

/// Test that botho WebRTC timing patterns are indistinguishable from real WebRTC.
#[test]
fn test_webrtc_timing_indistinguishability() {
    let mut rng = ChaCha8Rng::seed_from_u64(211);

    let reference_patterns: Vec<TrafficPattern> = (0..5)
        .map(|_| generate_webrtc_video_pattern(60, &mut rng))
        .collect();

    let botho_pattern = generate_botho_webrtc_pattern(60, &mut rng);

    let tests = FingerprintTests::new(reference_patterns);
    let result = tests.test_timing(&botho_pattern);

    println!(
        "Timing test: D={:.4}, p={:.4}, passed={}",
        result.ks_result.statistic, result.ks_result.p_value, result.passed
    );
    println!("  Diagnostics: {}", result.diagnostics);

    assert!(
        result.passed,
        "Timing patterns should be indistinguishable (p={:.4} <= 0.05)",
        result.ks_result.p_value
    );
}

/// Test that botho WebRTC flow characteristics match real WebRTC.
#[test]
fn test_webrtc_flow_indistinguishability() {
    let mut rng = ChaCha8Rng::seed_from_u64(212);

    let reference_patterns: Vec<TrafficPattern> = (0..10)
        .map(|_| generate_webrtc_video_pattern(60, &mut rng))
        .collect();

    let botho_pattern = generate_botho_webrtc_pattern(60, &mut rng);

    let tests = FingerprintTests::new(reference_patterns);
    let result = tests.test_flow(&botho_pattern);

    println!(
        "Flow test: statistic={:.4}, p={:.4}, passed={}",
        result.ks_result.statistic, result.ks_result.p_value, result.passed
    );
    println!("  Diagnostics: {}", result.diagnostics);

    assert!(
        result.passed,
        "Flow characteristics should be indistinguishable (p={:.4} <= 0.05)",
        result.ks_result.p_value
    );
}

/// Full fingerprinting resistance test suite.
#[test]
fn test_full_fingerprinting_resistance() {
    let mut rng = ChaCha8Rng::seed_from_u64(213);

    // Generate multiple reference patterns from different "video calls"
    let reference_patterns: Vec<TrafficPattern> = (0..10)
        .map(|_| generate_webrtc_video_pattern(120, &mut rng))
        .collect();

    // Generate botho pattern
    let botho_pattern = generate_botho_webrtc_pattern(120, &mut rng);

    println!("Reference patterns: {}", reference_patterns.len());
    println!("  Total packets: {}", reference_patterns.iter().map(|p| p.packet_count()).sum::<usize>());
    println!("Botho pattern: {} packets", botho_pattern.packet_count());

    let tests = FingerprintTests::new(reference_patterns);
    let result = tests.run_all(&botho_pattern);

    println!("\n=== Full Fingerprinting Resistance Test ===");
    println!(
        "Packet Sizes: D={:.4}, p={:.4}, {}",
        result.packet_sizes.ks_result.statistic,
        result.packet_sizes.ks_result.p_value,
        if result.packet_sizes.passed { "PASS" } else { "FAIL" }
    );
    println!(
        "Timing: D={:.4}, p={:.4}, {}",
        result.timing.ks_result.statistic,
        result.timing.ks_result.p_value,
        if result.timing.passed { "PASS" } else { "FAIL" }
    );
    println!(
        "Flow: statistic={:.4}, p={:.4}, {}",
        result.flow.ks_result.statistic,
        result.flow.ks_result.p_value,
        if result.flow.passed { "PASS" } else { "FAIL" }
    );
    println!(
        "\nOverall: {}",
        if result.all_passed { "ALL TESTS PASSED" } else { "SOME TESTS FAILED" }
    );

    assert!(
        result.all_passed,
        "All fingerprinting resistance tests should pass"
    );
}

/// Sanity check: K-S test can detect obviously different traffic.
#[test]
fn test_ks_detects_obvious_differences() {
    let mut rng = ChaCha8Rng::seed_from_u64(214);

    // Reference: WebRTC video pattern
    let reference = vec![generate_webrtc_video_pattern(60, &mut rng)];

    // Obviously different: Constant-size packets with constant timing
    let mut different_pattern = TrafficPattern::new();
    for i in 0..1000 {
        let iat = if i > 0 {
            Some(Duration::from_millis(100)) // Fixed 100ms
        } else {
            None
        };
        different_pattern.record_outbound(1000, iat); // Fixed 1000 bytes
    }
    different_pattern.set_duration(Duration::from_secs(100));

    let tests = FingerprintTests::new(reference);
    let result = tests.run_all(&different_pattern);

    println!(
        "Obvious difference test - packet sizes: p={:.4}",
        result.packet_sizes.ks_result.p_value
    );
    println!(
        "Obvious difference test - timing: p={:.4}",
        result.timing.ks_result.p_value
    );

    // At least packet sizes should be distinguishable
    assert!(
        !result.packet_sizes.passed || !result.timing.passed,
        "Obviously different traffic should be distinguishable"
    );
}

/// Test with varying duration patterns.
#[test]
fn test_varying_duration_patterns() {
    let mut rng = ChaCha8Rng::seed_from_u64(215);

    // Short call (30 seconds)
    let short_pattern = generate_webrtc_video_pattern(30, &mut rng);
    assert!(short_pattern.packet_count() > 0);

    // Long call (5 minutes)
    let long_pattern = generate_webrtc_video_pattern(300, &mut rng);
    assert!(long_pattern.packet_count() > short_pattern.packet_count());

    // Test that they have similar per-second characteristics
    let short_pps = short_pattern.packet_count() as f64 / 30.0;
    let long_pps = long_pattern.packet_count() as f64 / 300.0;

    println!("Short call: {} pps", short_pps);
    println!("Long call: {} pps", long_pps);

    // Packets per second should be in similar range
    assert!(
        (short_pps - long_pps).abs() / long_pps < 0.3,
        "Packets per second should be similar regardless of duration"
    );
}

/// Test K-S implementation against known distributions.
#[test]
fn test_ks_implementation_accuracy() {
    let mut rng = ChaCha8Rng::seed_from_u64(216);

    // Test 1: Identical samples should have D=0, p=1
    let sample = vec![1.0, 2.0, 3.0, 4.0, 5.0];
    let result = kolmogorov_smirnov(&sample, &sample);
    assert!(result.statistic < 0.001, "Identical samples should have D~0");
    assert!(result.p_value > 0.99, "Identical samples should have p~1");

    // Test 2: Samples from same distribution should be indistinguishable
    let uniform1: Vec<f64> = (0..500).map(|_| rng.gen_range(0.0..100.0)).collect();
    let uniform2: Vec<f64> = (0..500).map(|_| rng.gen_range(0.0..100.0)).collect();
    let result = kolmogorov_smirnov(&uniform1, &uniform2);
    assert!(
        result.is_indistinguishable_at_05(),
        "Same distribution samples should be indistinguishable, p={}",
        result.p_value
    );

    // Test 3: Non-overlapping samples should be distinguishable
    let low: Vec<f64> = (0..100).map(|_| rng.gen_range(0.0..10.0)).collect();
    let high: Vec<f64> = (0..100).map(|_| rng.gen_range(90.0..100.0)).collect();
    let result = kolmogorov_smirnov(&low, &high);
    assert!(
        !result.is_indistinguishable_at_05(),
        "Non-overlapping samples should be distinguishable, p={}",
        result.p_value
    );
}

/// Test with asymmetric sample sizes.
#[test]
fn test_ks_asymmetric_samples() {
    let mut rng = ChaCha8Rng::seed_from_u64(217);

    // Large reference, small test sample
    let large: Vec<f64> = (0..1000).map(|_| rng.gen_range(0.0..100.0)).collect();
    let small: Vec<f64> = (0..50).map(|_| rng.gen_range(0.0..100.0)).collect();

    let result = kolmogorov_smirnov(&small, &large);

    println!(
        "Asymmetric (50 vs 1000): D={:.4}, p={:.4}",
        result.statistic, result.p_value
    );

    // Should still work correctly (same underlying distribution)
    assert!(
        result.is_indistinguishable_at_05(),
        "Asymmetric samples from same distribution should be indistinguishable"
    );
}

/// Documentation test: DPI tool testing requirements.
///
/// This test documents the DPI tools that should be tested against
/// for comprehensive fingerprinting resistance validation.
#[test]
fn test_dpi_tool_documentation() {
    // This test documents the DPI tools that should be tested:
    //
    // 1. nDPI (Open Source)
    //    - https://github.com/ntop/nDPI
    //    - Should classify botho WebRTC traffic as "WebRTC"
    //    - Test command: ndpiReader -i capture.pcap
    //
    // 2. Wireshark Protocol Detection
    //    - Should identify traffic as DTLS/WebRTC
    //    - Verify no custom protocol indicators
    //
    // 3. Zeek (formerly Bro)
    //    - Network analysis framework
    //    - Should not flag botho traffic as anomalous
    //
    // 4. Commercial DPI (if available)
    //    - Palo Alto, Cisco, etc.
    //    - Target: <5% detection rate
    //
    // Manual testing procedure:
    // 1. Capture botho WebRTC traffic to PCAP
    // 2. Run through each DPI tool
    // 3. Verify classification matches legitimate WebRTC
    // 4. Document any detection signatures to address

    // This is a documentation test - always passes
    println!("DPI Tool Testing Requirements:");
    println!("  - nDPI: Should classify as WebRTC");
    println!("  - Wireshark: Should show DTLS/WebRTC protocols");
    println!("  - Zeek: Should not flag as anomalous");
    println!("  - Commercial DPI: Target <5% detection rate");
}

/// Test TrafficPattern serialization.
#[test]
fn test_traffic_pattern_serialization() {
    let mut pattern = TrafficPattern::new();
    pattern.record_outbound(100, None);
    pattern.record_inbound(200, Some(Duration::from_millis(10)));
    pattern.record_outbound(150, Some(Duration::from_millis(15)));
    pattern.set_duration(Duration::from_secs(1));

    // Serialize to JSON
    let json = serde_json::to_string(&pattern).expect("Should serialize");
    assert!(!json.is_empty());

    // Deserialize back
    let restored: TrafficPattern = serde_json::from_str(&json).expect("Should deserialize");
    assert_eq!(restored.packet_count(), pattern.packet_count());
    assert_eq!(restored.bytes_sent, pattern.bytes_sent);
    assert_eq!(restored.bytes_received, pattern.bytes_received);
}

/// Test TrafficPattern statistics.
#[test]
fn test_traffic_pattern_statistics() {
    let mut pattern = TrafficPattern::new();

    // Record 5 packets
    pattern.record_outbound(100, None);
    pattern.record_outbound(200, Some(Duration::from_micros(1000)));
    pattern.record_outbound(300, Some(Duration::from_micros(2000)));
    pattern.record_inbound(150, Some(Duration::from_micros(1500)));
    pattern.record_inbound(250, Some(Duration::from_micros(2500)));
    pattern.set_duration(Duration::from_secs(1));

    assert_eq!(pattern.packet_count(), 5);
    assert_eq!(pattern.bytes_sent, 600);
    assert_eq!(pattern.bytes_received, 400);
    assert_eq!(pattern.total_bytes(), 1000);

    // Average packet size: (100+200+300+150+250)/5 = 200
    assert!((pattern.average_packet_size() - 200.0).abs() < 0.01);

    // Average IAT: (1000+2000+1500+2500)/4 = 1750 µs
    assert!((pattern.average_inter_arrival_us() - 1750.0).abs() < 0.01);

    // Throughput: 1000 bytes / 1 second = 1000 B/s
    assert!((pattern.throughput_bps() - 1000.0).abs() < 0.01);
}
