// Copyright (c) 2024 Botho Foundation

//! Timing jitter for traffic analysis resistance.
//!
//! This module implements Phase 2.6 of the traffic privacy roadmap: adding
//! random timing delays to messages to prevent timing correlation attacks.
//!
//! # Design
//!
//! Before sending any private-path message, a random delay is applied within
//! a configurable range. This prevents attackers from correlating message
//! timing between network segments.
//!
//! # Security Properties
//!
//! - Delays are uniformly distributed within the configured range
//! - Applied only to private-path messages (not consensus-critical fast path)
//! - Combined with onion routing, timing jitter breaks correlation attacks
//!
//! # Example
//!
//! ```
//! use botho::network::privacy::timing::{TimingJitter, TimingJitterConfig};
//! use std::time::Duration;
//!
//! // Create jitter with default range (50-200ms)
//! let jitter = TimingJitter::default();
//!
//! // Get a random delay
//! let delay = jitter.delay();
//! assert!(delay >= Duration::from_millis(50));
//! assert!(delay <= Duration::from_millis(200));
//!
//! // Custom range
//! let config = TimingJitterConfig {
//!     min_delay_ms: 100,
//!     max_delay_ms: 500,
//! };
//! let custom_jitter = TimingJitter::new(config);
//! ```

use rand::Rng;
use std::time::Duration;

/// Default minimum delay in milliseconds.
pub const DEFAULT_MIN_DELAY_MS: u64 = 50;

/// Default maximum delay in milliseconds.
pub const DEFAULT_MAX_DELAY_MS: u64 = 200;

/// Configuration for timing jitter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimingJitterConfig {
    /// Minimum delay in milliseconds.
    pub min_delay_ms: u64,
    /// Maximum delay in milliseconds (inclusive).
    pub max_delay_ms: u64,
}

impl Default for TimingJitterConfig {
    fn default() -> Self {
        Self {
            min_delay_ms: DEFAULT_MIN_DELAY_MS,
            max_delay_ms: DEFAULT_MAX_DELAY_MS,
        }
    }
}

impl TimingJitterConfig {
    /// Create a new configuration with the specified range.
    ///
    /// # Panics
    ///
    /// Panics if `min_delay_ms > max_delay_ms`.
    pub fn new(min_delay_ms: u64, max_delay_ms: u64) -> Self {
        assert!(
            min_delay_ms <= max_delay_ms,
            "min_delay_ms ({}) must be <= max_delay_ms ({})",
            min_delay_ms,
            max_delay_ms
        );
        Self {
            min_delay_ms,
            max_delay_ms,
        }
    }

    /// Create configuration with no jitter (zero delay).
    pub fn disabled() -> Self {
        Self {
            min_delay_ms: 0,
            max_delay_ms: 0,
        }
    }

    /// Check if jitter is effectively disabled.
    pub fn is_disabled(&self) -> bool {
        self.min_delay_ms == 0 && self.max_delay_ms == 0
    }
}

/// Timing jitter generator for message delays.
///
/// Generates random delays within a configured range to prevent timing
/// correlation attacks on private-path messages.
#[derive(Debug, Clone)]
pub struct TimingJitter {
    config: TimingJitterConfig,
}

impl Default for TimingJitter {
    fn default() -> Self {
        Self::new(TimingJitterConfig::default())
    }
}

impl TimingJitter {
    /// Create a new timing jitter generator with the given configuration.
    pub fn new(config: TimingJitterConfig) -> Self {
        Self { config }
    }

    /// Create timing jitter with a custom range.
    ///
    /// # Panics
    ///
    /// Panics if `min_ms > max_ms`.
    pub fn with_range(min_ms: u64, max_ms: u64) -> Self {
        Self::new(TimingJitterConfig::new(min_ms, max_ms))
    }

    /// Create timing jitter that is disabled (zero delay).
    pub fn disabled() -> Self {
        Self::new(TimingJitterConfig::disabled())
    }

    /// Get the configuration.
    pub fn config(&self) -> &TimingJitterConfig {
        &self.config
    }

    /// Check if jitter is disabled.
    pub fn is_disabled(&self) -> bool {
        self.config.is_disabled()
    }

    /// Generate a random delay duration.
    ///
    /// Returns a duration uniformly distributed between `min_delay_ms` and
    /// `max_delay_ms` (inclusive).
    ///
    /// If jitter is disabled (both values are 0), returns zero duration.
    pub fn delay(&self) -> Duration {
        if self.config.is_disabled() {
            return Duration::ZERO;
        }

        let mut rng = rand::thread_rng();
        let ms = rng.gen_range(self.config.min_delay_ms..=self.config.max_delay_ms);
        Duration::from_millis(ms)
    }

    /// Generate a random delay using a provided RNG.
    ///
    /// Useful for deterministic testing.
    pub fn delay_with_rng<R: Rng>(&self, rng: &mut R) -> Duration {
        if self.config.is_disabled() {
            return Duration::ZERO;
        }

        let ms = rng.gen_range(self.config.min_delay_ms..=self.config.max_delay_ms);
        Duration::from_millis(ms)
    }
}

/// Apply jitter delay before executing an async operation.
///
/// This is a convenience function for applying timing jitter in async contexts.
///
/// # Example
///
/// ```ignore
/// use botho::network::privacy::timing::{TimingJitter, apply_jitter};
///
/// async fn send_message(msg: Message) {
///     let jitter = TimingJitter::default();
///     apply_jitter(&jitter).await;
///     // Now send the message
///     do_send(msg).await;
/// }
/// ```
pub async fn apply_jitter(jitter: &TimingJitter) {
    let delay = jitter.delay();
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }
}

/// Apply jitter and then execute a future.
///
/// Convenience function that applies jitter before running the provided future.
///
/// # Example
///
/// ```ignore
/// use botho::network::privacy::timing::{TimingJitter, with_jitter};
///
/// async fn example() {
///     let jitter = TimingJitter::default();
///     with_jitter(&jitter, async {
///         // This runs after the jitter delay
///         send_message().await
///     }).await;
/// }
/// ```
pub async fn with_jitter<F, T>(jitter: &TimingJitter, future: F) -> T
where
    F: std::future::Future<Output = T>,
{
    apply_jitter(jitter).await;
    future.await
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;

    #[test]
    fn test_default_config() {
        let config = TimingJitterConfig::default();
        assert_eq!(config.min_delay_ms, DEFAULT_MIN_DELAY_MS);
        assert_eq!(config.max_delay_ms, DEFAULT_MAX_DELAY_MS);
    }

    #[test]
    fn test_custom_config() {
        let config = TimingJitterConfig::new(100, 500);
        assert_eq!(config.min_delay_ms, 100);
        assert_eq!(config.max_delay_ms, 500);
    }

    #[test]
    #[should_panic(expected = "min_delay_ms")]
    fn test_invalid_config() {
        TimingJitterConfig::new(500, 100); // min > max
    }

    #[test]
    fn test_disabled_config() {
        let config = TimingJitterConfig::disabled();
        assert!(config.is_disabled());
        assert_eq!(config.min_delay_ms, 0);
        assert_eq!(config.max_delay_ms, 0);
    }

    #[test]
    fn test_delay_within_range() {
        let jitter = TimingJitter::default();

        for _ in 0..100 {
            let delay = jitter.delay();
            assert!(delay >= Duration::from_millis(DEFAULT_MIN_DELAY_MS));
            assert!(delay <= Duration::from_millis(DEFAULT_MAX_DELAY_MS));
        }
    }

    #[test]
    fn test_custom_range() {
        let jitter = TimingJitter::with_range(100, 200);

        for _ in 0..100 {
            let delay = jitter.delay();
            assert!(delay >= Duration::from_millis(100));
            assert!(delay <= Duration::from_millis(200));
        }
    }

    #[test]
    fn test_disabled_jitter() {
        let jitter = TimingJitter::disabled();
        assert!(jitter.is_disabled());

        for _ in 0..10 {
            let delay = jitter.delay();
            assert_eq!(delay, Duration::ZERO);
        }
    }

    #[test]
    fn test_deterministic_with_rng() {
        let jitter = TimingJitter::with_range(50, 200);

        // Same seed should produce same sequence
        let mut rng1 = ChaCha8Rng::seed_from_u64(12345);
        let mut rng2 = ChaCha8Rng::seed_from_u64(12345);

        for _ in 0..10 {
            let delay1 = jitter.delay_with_rng(&mut rng1);
            let delay2 = jitter.delay_with_rng(&mut rng2);
            assert_eq!(delay1, delay2);
        }
    }

    #[test]
    fn test_delay_distribution() {
        // Verify delays are reasonably distributed (not all the same)
        let jitter = TimingJitter::with_range(0, 100);
        let mut delays: Vec<u64> = Vec::new();

        for _ in 0..100 {
            delays.push(jitter.delay().as_millis() as u64);
        }

        // Should have some variance
        let min = *delays.iter().min().unwrap();
        let max = *delays.iter().max().unwrap();
        assert!(max > min, "delays should have variance");
    }

    #[test]
    fn test_single_value_range() {
        // When min == max, should always return that value
        let jitter = TimingJitter::with_range(100, 100);

        for _ in 0..10 {
            let delay = jitter.delay();
            assert_eq!(delay, Duration::from_millis(100));
        }
    }

    #[tokio::test]
    async fn test_apply_jitter_disabled() {
        let jitter = TimingJitter::disabled();
        let start = std::time::Instant::now();
        apply_jitter(&jitter).await;
        let elapsed = start.elapsed();

        // Should be nearly instant
        assert!(elapsed < Duration::from_millis(10));
    }

    #[tokio::test]
    async fn test_with_jitter() {
        let jitter = TimingJitter::disabled();
        let result = with_jitter(&jitter, async { 42 }).await;
        assert_eq!(result, 42);
    }
}
