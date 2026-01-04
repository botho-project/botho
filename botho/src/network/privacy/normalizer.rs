// Copyright (c) 2024 Botho Foundation

//! Traffic normalization integration for onion gossip.
//!
//! This module integrates all Phase 2 traffic normalization components:
//!
//! - **Message Padding**: Applied before onion encryption
//! - **Timing Jitter**: Applied before sending to first hop
//! - **Cover Traffic**: Generated and sent through circuits
//! - **Privacy Config**: Controls which features are active
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                    TRAFFIC NORMALIZER                           │
//! │                                                                 │
//! │  User submits transaction                                       │
//! │         │                                                       │
//! │         ▼                                                       │
//! │  ┌─────────────┐                                               │
//! │  │ Privacy     │ ─── Check privacy level                       │
//! │  │ Config      │                                               │
//! │  └─────────────┘                                               │
//! │         │                                                       │
//! │         ▼                                                       │
//! │  ┌─────────────┐                                               │
//! │  │ Padding     │ ─── Pad to fixed bucket size (if enabled)     │
//! │  └─────────────┘                                               │
//! │         │                                                       │
//! │         ▼                                                       │
//! │  ┌─────────────┐                                               │
//! │  │ Onion Wrap  │ ─── 3-layer encryption                        │
//! │  └─────────────┘                                               │
//! │         │                                                       │
//! │         ▼                                                       │
//! │  ┌─────────────┐                                               │
//! │  │ Timing      │ ─── Add jitter delay (if enabled)             │
//! │  │ Jitter      │                                               │
//! │  └─────────────┘                                               │
//! │         │                                                       │
//! │         ▼                                                       │
//! │  ┌─────────────┐                                               │
//! │  │ Send to     │ ─── First hop of circuit                      │
//! │  │ Circuit     │                                               │
//! │  └─────────────┘                                               │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Example
//!
//! ```ignore
//! use botho::network::privacy::normalizer::{TrafficNormalizer, NormalizerConfig};
//! use botho::network::privacy::PrivacyLevel;
//!
//! // Create normalizer with Enhanced privacy level
//! let config = NormalizerConfig::from_privacy_level(PrivacyLevel::Enhanced);
//! let normalizer = TrafficNormalizer::new(config);
//!
//! // Prepare a message for sending
//! let prepared = normalizer.prepare_message(payload)?;
//!
//! // Apply jitter before sending (async)
//! normalizer.apply_jitter().await;
//! ```

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Default padding bucket sizes in bytes.
/// Matches typical transaction size distribution.
pub const PADDING_BUCKETS: [usize; 5] = [512, 2048, 8192, 32768, 131072];

/// Default minimum jitter delay in milliseconds.
pub const DEFAULT_JITTER_MIN_MS: u64 = 50;

/// Default maximum jitter delay in milliseconds.
pub const DEFAULT_JITTER_MAX_MS: u64 = 200;

/// Configuration for the traffic normalizer.
///
/// Controls which Phase 2 features are enabled and their parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormalizerConfig {
    /// Enable message padding to fixed bucket sizes.
    pub padding_enabled: bool,

    /// Enable timing jitter before sending.
    pub jitter_enabled: bool,

    /// Minimum jitter delay in milliseconds.
    pub jitter_min_ms: u64,

    /// Maximum jitter delay in milliseconds.
    pub jitter_max_ms: u64,

    /// Enable cover traffic generation.
    pub cover_traffic_enabled: bool,

    /// Cover traffic rate (messages per minute) when idle.
    pub cover_rate_per_min: u32,
}

impl Default for NormalizerConfig {
    fn default() -> Self {
        Self {
            padding_enabled: false,
            jitter_enabled: false,
            jitter_min_ms: DEFAULT_JITTER_MIN_MS,
            jitter_max_ms: DEFAULT_JITTER_MAX_MS,
            cover_traffic_enabled: false,
            cover_rate_per_min: 2,
        }
    }
}

impl NormalizerConfig {
    /// Create config for Standard privacy level (no normalization).
    pub fn standard() -> Self {
        Self::default()
    }

    /// Create config from a privacy level.
    pub fn from_privacy_level(level: crate::network::privacy::PrivacyLevel) -> Self {
        use crate::network::privacy::PrivacyLevel;
        match level {
            PrivacyLevel::Standard => Self::standard(),
            PrivacyLevel::Enhanced => Self::enhanced(),
            PrivacyLevel::Maximum => Self::maximum(),
        }
    }

    /// Create config for Enhanced privacy level (padding + jitter).
    pub fn enhanced() -> Self {
        Self {
            padding_enabled: true,
            jitter_enabled: true,
            jitter_min_ms: DEFAULT_JITTER_MIN_MS,
            jitter_max_ms: DEFAULT_JITTER_MAX_MS,
            cover_traffic_enabled: false,
            cover_rate_per_min: 0,
        }
    }

    /// Create config for Maximum privacy level (all features).
    pub fn maximum() -> Self {
        Self {
            padding_enabled: true,
            jitter_enabled: true,
            jitter_min_ms: 100,
            jitter_max_ms: 300,
            cover_traffic_enabled: true,
            cover_rate_per_min: 4,
        }
    }

    /// Check if any normalization features are enabled.
    pub fn has_normalization(&self) -> bool {
        self.padding_enabled || self.jitter_enabled || self.cover_traffic_enabled
    }
}

/// Result of preparing a message for normalized transmission.
#[derive(Debug, Clone)]
pub struct PreparedMessage {
    /// The prepared payload (possibly padded).
    pub payload: Vec<u8>,

    /// Original payload size before padding.
    pub original_size: usize,

    /// Whether padding was applied.
    pub was_padded: bool,

    /// The bucket size used for padding (if padded).
    pub bucket_size: Option<usize>,
}

impl PreparedMessage {
    /// Create a prepared message without padding.
    pub fn unpadded(payload: Vec<u8>) -> Self {
        let size = payload.len();
        Self {
            payload,
            original_size: size,
            was_padded: false,
            bucket_size: None,
        }
    }

    /// Create a prepared message with padding.
    pub fn padded(payload: Vec<u8>, original_size: usize, bucket_size: usize) -> Self {
        Self {
            payload,
            original_size,
            was_padded: true,
            bucket_size: Some(bucket_size),
        }
    }

    /// Get the padding overhead in bytes.
    pub fn padding_overhead(&self) -> usize {
        self.payload.len().saturating_sub(self.original_size)
    }
}

/// Metrics for traffic normalization operations.
#[derive(Debug, Default)]
pub struct NormalizerMetrics {
    /// Messages processed through normalizer.
    pub messages_processed: AtomicU64,

    /// Messages that were padded.
    pub messages_padded: AtomicU64,

    /// Total padding bytes added.
    pub padding_bytes_added: AtomicU64,

    /// Messages with jitter applied.
    pub jitter_applied: AtomicU64,

    /// Total jitter delay in milliseconds.
    pub total_jitter_ms: AtomicU64,

    /// Cover messages generated.
    pub cover_messages_generated: AtomicU64,
}

impl NormalizerMetrics {
    /// Create new metrics.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a processed message.
    pub fn record_processed(&self) {
        self.messages_processed.fetch_add(1, Ordering::Relaxed);
    }

    /// Record padding applied.
    pub fn record_padded(&self, padding_bytes: usize) {
        self.messages_padded.fetch_add(1, Ordering::Relaxed);
        self.padding_bytes_added
            .fetch_add(padding_bytes as u64, Ordering::Relaxed);
    }

    /// Record jitter applied.
    pub fn record_jitter(&self, jitter_ms: u64) {
        self.jitter_applied.fetch_add(1, Ordering::Relaxed);
        self.total_jitter_ms.fetch_add(jitter_ms, Ordering::Relaxed);
    }

    /// Record cover message generated.
    pub fn record_cover(&self) {
        self.cover_messages_generated.fetch_add(1, Ordering::Relaxed);
    }

    /// Get a snapshot of metrics.
    pub fn snapshot(&self) -> NormalizerMetricsSnapshot {
        NormalizerMetricsSnapshot {
            messages_processed: self.messages_processed.load(Ordering::Relaxed),
            messages_padded: self.messages_padded.load(Ordering::Relaxed),
            padding_bytes_added: self.padding_bytes_added.load(Ordering::Relaxed),
            jitter_applied: self.jitter_applied.load(Ordering::Relaxed),
            total_jitter_ms: self.total_jitter_ms.load(Ordering::Relaxed),
            cover_messages_generated: self.cover_messages_generated.load(Ordering::Relaxed),
        }
    }
}

/// Snapshot of normalizer metrics.
#[derive(Debug, Clone, Default)]
pub struct NormalizerMetricsSnapshot {
    /// Messages processed through normalizer.
    pub messages_processed: u64,
    /// Messages that were padded.
    pub messages_padded: u64,
    /// Total padding bytes added.
    pub padding_bytes_added: u64,
    /// Messages with jitter applied.
    pub jitter_applied: u64,
    /// Total jitter delay in milliseconds.
    pub total_jitter_ms: u64,
    /// Cover messages generated.
    pub cover_messages_generated: u64,
}

impl NormalizerMetricsSnapshot {
    /// Calculate average padding per message.
    pub fn avg_padding_bytes(&self) -> f64 {
        if self.messages_padded == 0 {
            0.0
        } else {
            self.padding_bytes_added as f64 / self.messages_padded as f64
        }
    }

    /// Calculate average jitter in milliseconds.
    pub fn avg_jitter_ms(&self) -> f64 {
        if self.jitter_applied == 0 {
            0.0
        } else {
            self.total_jitter_ms as f64 / self.jitter_applied as f64
        }
    }

    /// Calculate padding ratio (padded / total).
    pub fn padding_ratio(&self) -> f64 {
        if self.messages_processed == 0 {
            0.0
        } else {
            self.messages_padded as f64 / self.messages_processed as f64
        }
    }
}

/// Traffic normalizer that integrates all Phase 2 components.
///
/// The normalizer applies traffic normalization features based on
/// the configured privacy level:
///
/// - **Standard**: No normalization (fastest)
/// - **Enhanced**: Padding + jitter (balanced)
/// - **Maximum**: All features including cover traffic (most private)
#[derive(Debug)]
pub struct TrafficNormalizer {
    /// Configuration controlling which features are active.
    config: NormalizerConfig,

    /// Metrics for monitoring normalization.
    metrics: NormalizerMetrics,
}

impl TrafficNormalizer {
    /// Create a new traffic normalizer with the given configuration.
    pub fn new(config: NormalizerConfig) -> Self {
        Self {
            config,
            metrics: NormalizerMetrics::new(),
        }
    }

    /// Create a normalizer with Standard privacy (no normalization).
    pub fn standard() -> Self {
        Self::new(NormalizerConfig::standard())
    }

    /// Create a normalizer with Enhanced privacy (padding + jitter).
    pub fn enhanced() -> Self {
        Self::new(NormalizerConfig::enhanced())
    }

    /// Create a normalizer with Maximum privacy (all features).
    pub fn maximum() -> Self {
        Self::new(NormalizerConfig::maximum())
    }

    /// Get the configuration.
    pub fn config(&self) -> &NormalizerConfig {
        &self.config
    }

    /// Get the metrics.
    pub fn metrics(&self) -> &NormalizerMetrics {
        &self.metrics
    }

    /// Prepare a message for normalized transmission.
    ///
    /// This applies padding if enabled based on the privacy configuration.
    pub fn prepare_message(&self, payload: &[u8]) -> PreparedMessage {
        self.metrics.record_processed();

        if !self.config.padding_enabled {
            return PreparedMessage::unpadded(payload.to_vec());
        }

        // Find appropriate bucket size
        let bucket = self.select_bucket(payload.len());

        // Apply padding
        let padded = self.pad_to_bucket(payload, bucket);
        let overhead = padded.len() - payload.len();

        self.metrics.record_padded(overhead);

        PreparedMessage::padded(padded, payload.len(), bucket)
    }

    /// Select the appropriate bucket size for a payload.
    fn select_bucket(&self, payload_len: usize) -> usize {
        for &bucket in &PADDING_BUCKETS {
            if payload_len <= bucket {
                return bucket;
            }
        }
        // Use largest bucket for oversized payloads
        *PADDING_BUCKETS.last().unwrap()
    }

    /// Pad a payload to the specified bucket size.
    fn pad_to_bucket(&self, payload: &[u8], bucket_size: usize) -> Vec<u8> {
        let mut padded = Vec::with_capacity(bucket_size);

        // Length header (2 bytes, little-endian)
        let len = (payload.len() as u16).to_le_bytes();
        padded.extend_from_slice(&len);

        // Original payload
        padded.extend_from_slice(payload);

        // Random padding to fill bucket
        let padding_needed = bucket_size.saturating_sub(padded.len());
        if padding_needed > 0 {
            let mut padding = vec![0u8; padding_needed];
            rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut padding);
            padded.extend_from_slice(&padding);
        }

        padded
    }

    /// Generate a random jitter delay based on configuration.
    ///
    /// Returns `Duration::ZERO` if jitter is disabled.
    pub fn generate_jitter(&self) -> Duration {
        if !self.config.jitter_enabled {
            return Duration::ZERO;
        }

        if self.config.jitter_max_ms == 0 {
            return Duration::ZERO;
        }

        let mut bytes = [0u8; 8];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut bytes);
        let random = u64::from_le_bytes(bytes);

        let range = self.config.jitter_max_ms - self.config.jitter_min_ms;
        let jitter_ms = self.config.jitter_min_ms + (random % (range + 1));

        self.metrics.record_jitter(jitter_ms);

        Duration::from_millis(jitter_ms)
    }

    /// Apply jitter delay asynchronously.
    ///
    /// This is the recommended way to apply jitter before sending.
    pub async fn apply_jitter(&self) {
        let delay = self.generate_jitter();
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }

    /// Check if cover traffic should be generated.
    pub fn should_generate_cover(&self) -> bool {
        self.config.cover_traffic_enabled && self.config.cover_rate_per_min > 0
    }

    /// Get the interval between cover messages.
    ///
    /// Returns `None` if cover traffic is disabled.
    pub fn cover_interval(&self) -> Option<Duration> {
        if !self.should_generate_cover() {
            return None;
        }

        let interval_secs = 60.0 / self.config.cover_rate_per_min as f64;
        Some(Duration::from_secs_f64(interval_secs))
    }

    /// Record that a cover message was generated.
    pub fn record_cover_generated(&self) {
        self.metrics.record_cover();
    }
}

impl Default for TrafficNormalizer {
    fn default() -> Self {
        Self::new(NormalizerConfig::default())
    }
}

/// Unpad a message that was padded by the normalizer.
///
/// Returns `None` if the message is invalid or too short.
pub fn unpad_message(padded: &[u8]) -> Option<Vec<u8>> {
    if padded.len() < 2 {
        return None;
    }

    // Read length header
    let len = u16::from_le_bytes([padded[0], padded[1]]) as usize;

    // Validate length
    if len + 2 > padded.len() {
        return None;
    }

    // Extract original payload
    Some(padded[2..2 + len].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_standard() {
        let config = NormalizerConfig::standard();
        assert!(!config.padding_enabled);
        assert!(!config.jitter_enabled);
        assert!(!config.cover_traffic_enabled);
        assert!(!config.has_normalization());
    }

    #[test]
    fn test_config_enhanced() {
        let config = NormalizerConfig::enhanced();
        assert!(config.padding_enabled);
        assert!(config.jitter_enabled);
        assert!(!config.cover_traffic_enabled);
        assert!(config.has_normalization());
    }

    #[test]
    fn test_config_maximum() {
        let config = NormalizerConfig::maximum();
        assert!(config.padding_enabled);
        assert!(config.jitter_enabled);
        assert!(config.cover_traffic_enabled);
        assert!(config.has_normalization());
    }

    #[test]
    fn test_prepare_message_no_padding() {
        let normalizer = TrafficNormalizer::standard();
        let payload = b"test payload";

        let prepared = normalizer.prepare_message(payload);

        assert!(!prepared.was_padded);
        assert_eq!(prepared.payload, payload);
        assert_eq!(prepared.original_size, payload.len());
        assert_eq!(prepared.bucket_size, None);
        assert_eq!(prepared.padding_overhead(), 0);
    }

    #[test]
    fn test_prepare_message_with_padding() {
        let normalizer = TrafficNormalizer::enhanced();
        let payload = b"test payload";

        let prepared = normalizer.prepare_message(payload);

        assert!(prepared.was_padded);
        assert!(prepared.payload.len() >= payload.len());
        assert_eq!(prepared.original_size, payload.len());
        assert!(prepared.bucket_size.is_some());
        assert!(prepared.padding_overhead() > 0);
    }

    #[test]
    fn test_padding_bucket_selection() {
        let normalizer = TrafficNormalizer::enhanced();

        // Small payload -> smallest bucket
        let prepared = normalizer.prepare_message(&[0u8; 100]);
        assert_eq!(prepared.bucket_size, Some(512));

        // Medium payload -> appropriate bucket
        let prepared = normalizer.prepare_message(&[0u8; 1000]);
        assert_eq!(prepared.bucket_size, Some(2048));

        // Large payload -> larger bucket
        let prepared = normalizer.prepare_message(&[0u8; 5000]);
        assert_eq!(prepared.bucket_size, Some(8192));
    }

    #[test]
    fn test_unpad_message() {
        let normalizer = TrafficNormalizer::enhanced();
        let original = b"original payload data";

        let prepared = normalizer.prepare_message(original);
        let unpadded = unpad_message(&prepared.payload).unwrap();

        assert_eq!(unpadded, original);
    }

    #[test]
    fn test_unpad_invalid_message() {
        // Too short
        assert!(unpad_message(&[]).is_none());
        assert!(unpad_message(&[0]).is_none());

        // Invalid length header
        let invalid = [0xFF, 0xFF, 0x01, 0x02]; // Length claims 65535 bytes
        assert!(unpad_message(&invalid).is_none());
    }

    #[test]
    fn test_jitter_disabled() {
        let normalizer = TrafficNormalizer::standard();
        let jitter = normalizer.generate_jitter();
        assert_eq!(jitter, Duration::ZERO);
    }

    #[test]
    fn test_jitter_enabled() {
        let normalizer = TrafficNormalizer::enhanced();

        // Generate multiple jitters and verify they're in range
        for _ in 0..10 {
            let jitter = normalizer.generate_jitter();
            assert!(jitter >= Duration::from_millis(DEFAULT_JITTER_MIN_MS));
            assert!(jitter <= Duration::from_millis(DEFAULT_JITTER_MAX_MS));
        }
    }

    #[test]
    fn test_cover_traffic_disabled() {
        let normalizer = TrafficNormalizer::enhanced();
        assert!(!normalizer.should_generate_cover());
        assert!(normalizer.cover_interval().is_none());
    }

    #[test]
    fn test_cover_traffic_enabled() {
        let normalizer = TrafficNormalizer::maximum();
        assert!(normalizer.should_generate_cover());

        let interval = normalizer.cover_interval().unwrap();
        // 4 per minute = 15 second intervals
        assert!(interval > Duration::from_secs(10));
        assert!(interval < Duration::from_secs(20));
    }

    #[test]
    fn test_metrics_tracking() {
        let normalizer = TrafficNormalizer::enhanced();

        // Process some messages
        normalizer.prepare_message(b"message 1");
        normalizer.prepare_message(b"message 2");
        normalizer.generate_jitter();

        let snapshot = normalizer.metrics().snapshot();

        assert_eq!(snapshot.messages_processed, 2);
        assert_eq!(snapshot.messages_padded, 2);
        assert!(snapshot.padding_bytes_added > 0);
        assert_eq!(snapshot.jitter_applied, 1);
    }

    #[test]
    fn test_metrics_calculations() {
        let snapshot = NormalizerMetricsSnapshot {
            messages_processed: 100,
            messages_padded: 80,
            padding_bytes_added: 40000,
            jitter_applied: 50,
            total_jitter_ms: 5000,
            cover_messages_generated: 10,
        };

        assert!((snapshot.avg_padding_bytes() - 500.0).abs() < 0.1);
        assert!((snapshot.avg_jitter_ms() - 100.0).abs() < 0.1);
        assert!((snapshot.padding_ratio() - 0.8).abs() < 0.01);
    }

    #[test]
    fn test_metrics_edge_cases() {
        let snapshot = NormalizerMetricsSnapshot::default();

        // Should not panic on division by zero
        assert_eq!(snapshot.avg_padding_bytes(), 0.0);
        assert_eq!(snapshot.avg_jitter_ms(), 0.0);
        assert_eq!(snapshot.padding_ratio(), 0.0);
    }

    #[tokio::test]
    async fn test_apply_jitter_async() {
        let normalizer = TrafficNormalizer::standard();

        // Should return immediately when disabled
        let start = std::time::Instant::now();
        normalizer.apply_jitter().await;
        let elapsed = start.elapsed();

        assert!(elapsed < Duration::from_millis(10));
    }

    #[test]
    fn test_prepared_message_unpadded() {
        let payload = vec![1, 2, 3, 4, 5];
        let prepared = PreparedMessage::unpadded(payload.clone());

        assert_eq!(prepared.payload, payload);
        assert_eq!(prepared.original_size, 5);
        assert!(!prepared.was_padded);
        assert!(prepared.bucket_size.is_none());
    }

    #[test]
    fn test_prepared_message_padded() {
        let payload = vec![0u8; 512];
        let prepared = PreparedMessage::padded(payload, 100, 512);

        assert_eq!(prepared.original_size, 100);
        assert!(prepared.was_padded);
        assert_eq!(prepared.bucket_size, Some(512));
        assert_eq!(prepared.padding_overhead(), 412);
    }
}
