// Copyright (c) 2024 Botho Foundation

//! Cover traffic generation for traffic analysis resistance.
//!
//! This module implements Phase 2.4 of the traffic privacy roadmap: generating
//! dummy messages that are indistinguishable from real transactions after
//! onion encryption.
//!
//! # Design
//!
//! Cover messages match the size distribution of real transactions to prevent
//! statistical analysis. After onion encryption, cover traffic is
//! indistinguishable from real transaction traffic.
//!
//! # Size Distribution
//!
//! Real transactions typically fall into these size ranges:
//! - Small (200-300 bytes): Simple transfers
//! - Medium (300-450 bytes): Standard transactions with change
//! - Large (450-600 bytes): Multi-input transactions
//!
//! Cover traffic uses weighted random selection to match this distribution.
//!
//! # Example
//!
//! ```
//! use botho::network::privacy::cover::{CoverMessage, CoverTrafficGenerator};
//!
//! // Generate a single cover message
//! let cover = CoverMessage::generate();
//! assert!(cover.payload.len() >= 200);
//! assert!(cover.payload.len() <= 600);
//!
//! // Use a generator for configurable cover traffic
//! let generator = CoverTrafficGenerator::default();
//! let cover = generator.generate();
//! ```

use rand::{
    distributions::{Distribution, WeightedIndex},
    Rng,
};
use serde::{Deserialize, Serialize};

/// Minimum cover message size in bytes.
pub const MIN_COVER_SIZE: usize = 200;

/// Maximum cover message size in bytes.
pub const MAX_COVER_SIZE: usize = 600;

/// Default size distribution weights.
/// Index 0 = small (200-300), 1 = medium (300-450), 2 = large (450-600)
pub const DEFAULT_SIZE_WEIGHTS: [u32; 3] = [30, 50, 20];

/// Message type marker for cover traffic.
///
/// This is only visible after decryption at the exit hop.
/// The exit hop silently discards cover messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum CoverMessageType {
    /// Cover traffic message (should be discarded by exit hop)
    Cover = 0xFF,
}

impl Default for CoverMessageType {
    fn default() -> Self {
        Self::Cover
    }
}

/// A cover message for traffic analysis resistance.
///
/// Cover messages are dummy messages that, after onion encryption, are
/// indistinguishable from real transaction traffic. They are identified
/// by a type marker that is only visible after final decryption.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverMessage {
    /// Message type marker (only visible after decryption)
    pub msg_type: CoverMessageType,
    /// Random data matching typical transaction size
    pub payload: Vec<u8>,
}

impl CoverMessage {
    /// Generate a new cover message with random size from default distribution.
    ///
    /// The size is chosen using weighted random selection to match the
    /// typical distribution of real transaction sizes.
    pub fn generate() -> Self {
        Self::generate_with_rng(&mut rand::thread_rng())
    }

    /// Generate a cover message with a specific size.
    ///
    /// # Arguments
    ///
    /// * `size` - The payload size in bytes (clamped to valid range)
    pub fn with_size(size: usize) -> Self {
        Self::with_size_and_rng(size, &mut rand::thread_rng())
    }

    /// Generate a cover message using a provided RNG.
    ///
    /// Useful for deterministic testing.
    pub fn generate_with_rng<R: Rng>(rng: &mut R) -> Self {
        let size = generate_cover_size(rng);
        Self::with_size_and_rng(size, rng)
    }

    /// Generate a cover message with specific size using provided RNG.
    pub fn with_size_and_rng<R: Rng>(size: usize, rng: &mut R) -> Self {
        let clamped_size = size.clamp(MIN_COVER_SIZE, MAX_COVER_SIZE);
        let mut payload = vec![0u8; clamped_size];
        rng.fill(&mut payload[..]);

        Self {
            msg_type: CoverMessageType::Cover,
            payload,
        }
    }

    /// Check if this message is a cover message.
    ///
    /// Always returns true for `CoverMessage` instances.
    pub fn is_cover(&self) -> bool {
        matches!(self.msg_type, CoverMessageType::Cover)
    }

    /// Get the total serialized size of this message.
    ///
    /// This includes the message type marker and payload.
    pub fn serialized_size(&self) -> usize {
        // 1 byte for msg_type + payload length
        1 + self.payload.len()
    }

    /// Serialize to bytes for transmission.
    ///
    /// Format: [msg_type: u8][payload: variable]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.serialized_size());
        bytes.push(self.msg_type as u8);
        bytes.extend_from_slice(&self.payload);
        bytes
    }

    /// Deserialize from bytes.
    ///
    /// Returns `None` if the data is too short or has invalid type marker.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.is_empty() {
            return None;
        }

        // Check message type
        if bytes[0] != CoverMessageType::Cover as u8 {
            return None;
        }

        Some(Self {
            msg_type: CoverMessageType::Cover,
            payload: bytes[1..].to_vec(),
        })
    }
}

/// Size category for cover traffic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeCategory {
    /// Small transactions (200-300 bytes)
    Small,
    /// Medium transactions (300-450 bytes)
    Medium,
    /// Large transactions (450-600 bytes)
    Large,
}

impl SizeCategory {
    /// Get the size range for this category.
    pub fn range(&self) -> (usize, usize) {
        match self {
            SizeCategory::Small => (200, 300),
            SizeCategory::Medium => (300, 450),
            SizeCategory::Large => (450, 600),
        }
    }

    /// Generate a random size within this category.
    pub fn random_size<R: Rng>(&self, rng: &mut R) -> usize {
        let (min, max) = self.range();
        rng.gen_range(min..=max)
    }
}

/// Configuration for cover traffic generation.
#[derive(Debug, Clone)]
pub struct CoverTrafficConfig {
    /// Weights for size categories [small, medium, large]
    pub size_weights: [u32; 3],
    /// Minimum message size override (default: MIN_COVER_SIZE)
    pub min_size: usize,
    /// Maximum message size override (default: MAX_COVER_SIZE)
    pub max_size: usize,
}

impl Default for CoverTrafficConfig {
    fn default() -> Self {
        Self {
            size_weights: DEFAULT_SIZE_WEIGHTS,
            min_size: MIN_COVER_SIZE,
            max_size: MAX_COVER_SIZE,
        }
    }
}

impl CoverTrafficConfig {
    /// Create config with custom size weights.
    ///
    /// Weights determine the probability of each size category:
    /// - Index 0: Small (200-300 bytes)
    /// - Index 1: Medium (300-450 bytes)
    /// - Index 2: Large (450-600 bytes)
    pub fn with_weights(weights: [u32; 3]) -> Self {
        Self {
            size_weights: weights,
            ..Default::default()
        }
    }

    /// Create config with uniform size distribution.
    pub fn uniform() -> Self {
        Self {
            size_weights: [1, 1, 1],
            ..Default::default()
        }
    }
}

/// Generator for cover traffic with configurable size distribution.
#[derive(Debug, Clone)]
pub struct CoverTrafficGenerator {
    config: CoverTrafficConfig,
}

impl Default for CoverTrafficGenerator {
    fn default() -> Self {
        Self::new(CoverTrafficConfig::default())
    }
}

impl CoverTrafficGenerator {
    /// Create a new generator with the given configuration.
    pub fn new(config: CoverTrafficConfig) -> Self {
        Self { config }
    }

    /// Create a generator with custom size weights.
    pub fn with_weights(weights: [u32; 3]) -> Self {
        Self::new(CoverTrafficConfig::with_weights(weights))
    }

    /// Create a generator with uniform size distribution.
    pub fn uniform() -> Self {
        Self::new(CoverTrafficConfig::uniform())
    }

    /// Get the configuration.
    pub fn config(&self) -> &CoverTrafficConfig {
        &self.config
    }

    /// Generate a cover message using the configured distribution.
    pub fn generate(&self) -> CoverMessage {
        self.generate_with_rng(&mut rand::thread_rng())
    }

    /// Generate a cover message using a provided RNG.
    pub fn generate_with_rng<R: Rng>(&self, rng: &mut R) -> CoverMessage {
        let size = self.generate_size(rng);
        CoverMessage::with_size_and_rng(size, rng)
    }

    /// Generate just a size value using the configured distribution.
    pub fn generate_size<R: Rng>(&self, rng: &mut R) -> usize {
        let category = self.select_category(rng);
        let (min, max) = category.range();

        // Clamp to configured bounds
        let min = min.max(self.config.min_size);
        let max = max.min(self.config.max_size);

        rng.gen_range(min..=max)
    }

    /// Select a size category based on weights.
    fn select_category<R: Rng>(&self, rng: &mut R) -> SizeCategory {
        let dist = WeightedIndex::new(&self.config.size_weights).expect("weights must be non-zero");

        match dist.sample(rng) {
            0 => SizeCategory::Small,
            1 => SizeCategory::Medium,
            _ => SizeCategory::Large,
        }
    }

    /// Generate multiple cover messages.
    pub fn generate_batch(&self, count: usize) -> Vec<CoverMessage> {
        let mut rng = rand::thread_rng();
        (0..count)
            .map(|_| self.generate_with_rng(&mut rng))
            .collect()
    }
}

/// Generate a cover message size using weighted random selection.
fn generate_cover_size<R: Rng>(rng: &mut R) -> usize {
    let dist = WeightedIndex::new(DEFAULT_SIZE_WEIGHTS).expect("default weights are valid");

    let category = match dist.sample(rng) {
        0 => SizeCategory::Small,
        1 => SizeCategory::Medium,
        _ => SizeCategory::Large,
    };

    category.random_size(rng)
}

/// Statistics about generated cover traffic.
#[derive(Debug, Clone, Default)]
pub struct CoverTrafficStats {
    /// Total messages generated
    pub total_messages: u64,
    /// Total bytes generated
    pub total_bytes: u64,
    /// Count by size category
    pub by_category: [u64; 3],
    /// Minimum size seen
    pub min_size: Option<usize>,
    /// Maximum size seen
    pub max_size: Option<usize>,
}

impl CoverTrafficStats {
    /// Record a generated message.
    pub fn record(&mut self, msg: &CoverMessage) {
        self.total_messages += 1;
        self.total_bytes += msg.payload.len() as u64;

        let size = msg.payload.len();
        let category_idx = if size <= 300 {
            0
        } else if size <= 450 {
            1
        } else {
            2
        };
        self.by_category[category_idx] += 1;

        self.min_size = Some(self.min_size.map_or(size, |m| m.min(size)));
        self.max_size = Some(self.max_size.map_or(size, |m| m.max(size)));
    }

    /// Get average message size.
    pub fn average_size(&self) -> Option<f64> {
        if self.total_messages > 0 {
            Some(self.total_bytes as f64 / self.total_messages as f64)
        } else {
            None
        }
    }

    /// Get distribution percentages by category.
    pub fn distribution(&self) -> [f64; 3] {
        if self.total_messages == 0 {
            return [0.0; 3];
        }

        let total = self.total_messages as f64;
        [
            self.by_category[0] as f64 / total * 100.0,
            self.by_category[1] as f64 / total * 100.0,
            self.by_category[2] as f64 / total * 100.0,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;

    #[test]
    fn test_cover_message_generate() {
        let msg = CoverMessage::generate();
        assert!(msg.is_cover());
        assert!(msg.payload.len() >= MIN_COVER_SIZE);
        assert!(msg.payload.len() <= MAX_COVER_SIZE);
    }

    #[test]
    fn test_cover_message_with_size() {
        let msg = CoverMessage::with_size(300);
        assert_eq!(msg.payload.len(), 300);
        assert!(msg.is_cover());
    }

    #[test]
    fn test_cover_message_size_clamped() {
        // Too small
        let msg = CoverMessage::with_size(50);
        assert_eq!(msg.payload.len(), MIN_COVER_SIZE);

        // Too large
        let msg = CoverMessage::with_size(1000);
        assert_eq!(msg.payload.len(), MAX_COVER_SIZE);
    }

    #[test]
    fn test_cover_message_serialization() {
        let msg = CoverMessage::with_size(300);
        let bytes = msg.to_bytes();

        assert_eq!(bytes[0], CoverMessageType::Cover as u8);
        assert_eq!(bytes.len(), 301); // 1 byte type + 300 payload

        let parsed = CoverMessage::from_bytes(&bytes).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn test_cover_message_from_bytes_invalid() {
        // Empty
        assert!(CoverMessage::from_bytes(&[]).is_none());

        // Wrong type marker
        assert!(CoverMessage::from_bytes(&[0x00, 0x01, 0x02]).is_none());
    }

    #[test]
    fn test_cover_message_deterministic() {
        let mut rng1 = ChaCha8Rng::seed_from_u64(12345);
        let mut rng2 = ChaCha8Rng::seed_from_u64(12345);

        let msg1 = CoverMessage::generate_with_rng(&mut rng1);
        let msg2 = CoverMessage::generate_with_rng(&mut rng2);

        assert_eq!(msg1, msg2);
    }

    #[test]
    fn test_generator_default() {
        let gen = CoverTrafficGenerator::default();
        let msg = gen.generate();
        assert!(msg.is_cover());
        assert!(msg.payload.len() >= MIN_COVER_SIZE);
        assert!(msg.payload.len() <= MAX_COVER_SIZE);
    }

    #[test]
    fn test_generator_with_weights() {
        // Heavy bias toward small messages
        let gen = CoverTrafficGenerator::with_weights([100, 1, 1]);
        let mut rng = ChaCha8Rng::seed_from_u64(42);

        let mut small_count = 0;
        for _ in 0..100 {
            let msg = gen.generate_with_rng(&mut rng);
            if msg.payload.len() <= 300 {
                small_count += 1;
            }
        }

        // Should be heavily biased toward small
        assert!(small_count > 80, "Expected >80 small, got {}", small_count);
    }

    #[test]
    fn test_generator_uniform() {
        let gen = CoverTrafficGenerator::uniform();
        let mut rng = ChaCha8Rng::seed_from_u64(42);

        let mut stats = CoverTrafficStats::default();
        for _ in 0..300 {
            let msg = gen.generate_with_rng(&mut rng);
            stats.record(&msg);
        }

        // Should be roughly uniform (each category ~33%)
        let dist = stats.distribution();
        for pct in dist.iter() {
            assert!(*pct > 20.0 && *pct < 50.0, "Expected ~33%, got {:.1}%", pct);
        }
    }

    #[test]
    fn test_generator_batch() {
        let gen = CoverTrafficGenerator::default();
        let batch = gen.generate_batch(10);

        assert_eq!(batch.len(), 10);
        for msg in batch {
            assert!(msg.is_cover());
        }
    }

    #[test]
    fn test_size_category_ranges() {
        assert_eq!(SizeCategory::Small.range(), (200, 300));
        assert_eq!(SizeCategory::Medium.range(), (300, 450));
        assert_eq!(SizeCategory::Large.range(), (450, 600));
    }

    #[test]
    fn test_size_distribution_matches_weights() {
        let mut rng = ChaCha8Rng::seed_from_u64(42);
        let mut stats = CoverTrafficStats::default();

        // Generate many messages
        for _ in 0..1000 {
            let msg = CoverMessage::generate_with_rng(&mut rng);
            stats.record(&msg);
        }

        // Default weights are [30, 50, 20]
        // So we expect roughly 30% small, 50% medium, 20% large
        let dist = stats.distribution();

        // Allow some variance (±10%)
        assert!(dist[0] > 20.0 && dist[0] < 40.0, "Small: {:.1}%", dist[0]);
        assert!(dist[1] > 40.0 && dist[1] < 60.0, "Medium: {:.1}%", dist[1]);
        assert!(dist[2] > 10.0 && dist[2] < 30.0, "Large: {:.1}%", dist[2]);
    }

    #[test]
    fn test_payload_is_random() {
        let msg1 = CoverMessage::with_size(300);
        let msg2 = CoverMessage::with_size(300);

        // Payloads should be different (random)
        assert_ne!(msg1.payload, msg2.payload);
    }

    #[test]
    fn test_stats_tracking() {
        let mut stats = CoverTrafficStats::default();

        stats.record(&CoverMessage::with_size(250)); // small
        stats.record(&CoverMessage::with_size(350)); // medium
        stats.record(&CoverMessage::with_size(500)); // large

        assert_eq!(stats.total_messages, 3);
        assert_eq!(stats.total_bytes, 250 + 350 + 500);
        assert_eq!(stats.by_category, [1, 1, 1]);
        assert_eq!(stats.min_size, Some(250));
        assert_eq!(stats.max_size, Some(500));

        let avg = stats.average_size().unwrap();
        assert!((avg - 366.67).abs() < 0.1);
    }

    #[test]
    fn test_stats_empty() {
        let stats = CoverTrafficStats::default();
        assert_eq!(stats.total_messages, 0);
        assert!(stats.average_size().is_none());
        assert_eq!(stats.distribution(), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn test_serialized_size() {
        let msg = CoverMessage::with_size(300);
        assert_eq!(msg.serialized_size(), 301);
        assert_eq!(msg.to_bytes().len(), msg.serialized_size());
    }

    #[test]
    fn test_cover_type_default() {
        let msg_type = CoverMessageType::default();
        assert_eq!(msg_type, CoverMessageType::Cover);
    }
}
