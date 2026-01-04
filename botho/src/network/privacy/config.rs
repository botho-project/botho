// Copyright (c) 2024 Botho Foundation

//! Privacy level configuration for traffic normalization.
//!
//! This module implements Phase 2.7 of the traffic privacy roadmap:
//! user-configurable privacy levels that control which traffic normalization
//! features are enabled.
//!
//! # Privacy Levels
//!
//! Three privacy levels are available, balancing privacy against performance:
//!
//! | Level | Features | Latency | Bandwidth |
//! |-------|----------|---------|-----------|
//! | Standard | Onion routing | ~100ms | ~30% |
//! | Enhanced | + padding + jitter | ~200-400ms | ~50% |
//! | Maximum | + constant-rate + cover | Variable | ~2x |
//!
//! # Example
//!
//! ```
//! use botho::network::privacy::config::{PrivacyLevel, PrivacyConfig};
//!
//! // Get config for a privacy level
//! let config = PrivacyLevel::Enhanced.to_config();
//! assert!(config.padding);
//! assert!(config.timing_jitter);
//! assert!(!config.constant_rate);
//!
//! // Create custom config
//! let custom = PrivacyConfig::builder()
//!     .padding(true)
//!     .timing_jitter(true)
//!     .build();
//! ```

use serde::{Deserialize, Serialize};
use std::fmt;

/// Privacy level presets for traffic normalization.
///
/// Each level represents a different balance between privacy and performance.
/// Higher privacy levels add more protection but increase latency and bandwidth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PrivacyLevel {
    /// Standard privacy: onion routing only.
    ///
    /// - Latency: ~100ms added
    /// - Bandwidth: ~30% overhead
    /// - Protection: Transaction origin hidden via 3-hop circuits
    #[default]
    Standard,

    /// Enhanced privacy: onion routing + padding + timing jitter.
    ///
    /// - Latency: ~200-400ms added
    /// - Bandwidth: ~50% overhead
    /// - Protection: Standard + size and timing analysis resistance
    Enhanced,

    /// Maximum privacy: all features enabled.
    ///
    /// - Latency: Variable (queue-based)
    /// - Bandwidth: ~2x (cover traffic)
    /// - Protection: Enhanced + constant traffic pattern
    Maximum,
}

/// Default minimum jitter delay in milliseconds.
pub const DEFAULT_JITTER_MIN_MS: u64 = 50;

/// Default maximum jitter delay in milliseconds.
pub const DEFAULT_JITTER_MAX_MS: u64 = 200;

impl PrivacyLevel {
    /// Convert this privacy level to a full configuration.
    pub fn to_config(self) -> PrivacyConfig {
        match self {
            PrivacyLevel::Standard => PrivacyConfig {
                onion_routing: true,
                padding: false,
                timing_jitter: false,
                constant_rate: false,
                cover_traffic: false,
                jitter_min_ms: 0,
                jitter_max_ms: 0,
            },
            PrivacyLevel::Enhanced => PrivacyConfig {
                onion_routing: true,
                padding: true,
                timing_jitter: true,
                constant_rate: false,
                cover_traffic: false,
                jitter_min_ms: DEFAULT_JITTER_MIN_MS,
                jitter_max_ms: DEFAULT_JITTER_MAX_MS,
            },
            PrivacyLevel::Maximum => PrivacyConfig {
                onion_routing: true,
                padding: true,
                timing_jitter: true,
                constant_rate: true,
                cover_traffic: true,
                jitter_min_ms: 100, // Wider range for maximum privacy
                jitter_max_ms: 300,
            },
        }
    }

    /// Get all available privacy levels.
    pub fn all() -> &'static [PrivacyLevel] {
        &[
            PrivacyLevel::Standard,
            PrivacyLevel::Enhanced,
            PrivacyLevel::Maximum,
        ]
    }

    /// Get a human-readable description of this level.
    pub fn description(&self) -> &'static str {
        match self {
            PrivacyLevel::Standard => "Onion routing only (~100ms latency, ~30% bandwidth)",
            PrivacyLevel::Enhanced => {
                "Onion + padding + jitter (~200-400ms latency, ~50% bandwidth)"
            }
            PrivacyLevel::Maximum => {
                "All features: constant-rate + cover traffic (variable latency, ~2x bandwidth)"
            }
        }
    }
}

impl fmt::Display for PrivacyLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PrivacyLevel::Standard => write!(f, "standard"),
            PrivacyLevel::Enhanced => write!(f, "enhanced"),
            PrivacyLevel::Maximum => write!(f, "maximum"),
        }
    }
}

impl std::str::FromStr for PrivacyLevel {
    type Err = PrivacyLevelParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "standard" | "std" | "normal" => Ok(PrivacyLevel::Standard),
            "enhanced" | "high" => Ok(PrivacyLevel::Enhanced),
            "maximum" | "max" | "paranoid" => Ok(PrivacyLevel::Maximum),
            _ => Err(PrivacyLevelParseError(s.to_string())),
        }
    }
}

/// Error when parsing a privacy level from string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivacyLevelParseError(String);

impl fmt::Display for PrivacyLevelParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid privacy level '{}': expected 'standard', 'enhanced', or 'maximum'",
            self.0
        )
    }
}

impl std::error::Error for PrivacyLevelParseError {}

/// Configuration for privacy features.
///
/// Controls which traffic normalization features are enabled. Can be created
/// from a [`PrivacyLevel`] preset or built manually for custom configurations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivacyConfig {
    /// Enable onion routing for private-path messages.
    /// This is the foundation of privacy and should always be true.
    #[serde(default = "default_true")]
    pub onion_routing: bool,

    /// Pad messages to fixed bucket sizes.
    /// Prevents size-based traffic analysis.
    #[serde(default)]
    pub padding: bool,

    /// Add random timing delays before sending.
    /// Prevents timing correlation attacks.
    #[serde(default)]
    pub timing_jitter: bool,

    /// Send messages at a constant rate (queue-based).
    /// Prevents traffic volume analysis.
    #[serde(default)]
    pub constant_rate: bool,

    /// Generate cover traffic when idle.
    /// Makes traffic patterns indistinguishable.
    #[serde(default)]
    pub cover_traffic: bool,

    /// Minimum timing jitter delay in milliseconds.
    #[serde(default)]
    pub jitter_min_ms: u64,

    /// Maximum timing jitter delay in milliseconds.
    #[serde(default)]
    pub jitter_max_ms: u64,
}

fn default_true() -> bool {
    true
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        PrivacyLevel::default().to_config()
    }
}

impl PrivacyConfig {
    /// Create a new builder for custom configuration.
    pub fn builder() -> PrivacyConfigBuilder {
        PrivacyConfigBuilder::new()
    }

    /// Create configuration from a privacy level.
    pub fn from_level(level: PrivacyLevel) -> Self {
        level.to_config()
    }

    /// Check if any privacy features beyond onion routing are enabled.
    pub fn has_traffic_normalization(&self) -> bool {
        self.padding || self.timing_jitter || self.constant_rate || self.cover_traffic
    }

    /// Get the approximate privacy level this config represents.
    ///
    /// Returns `None` if the config doesn't match any preset level.
    pub fn to_level(&self) -> Option<PrivacyLevel> {
        for level in PrivacyLevel::all() {
            if self.matches_level(*level) {
                return Some(*level);
            }
        }
        None
    }

    /// Check if this config matches a specific level.
    fn matches_level(&self, level: PrivacyLevel) -> bool {
        let expected = level.to_config();
        self.onion_routing == expected.onion_routing
            && self.padding == expected.padding
            && self.timing_jitter == expected.timing_jitter
            && self.constant_rate == expected.constant_rate
            && self.cover_traffic == expected.cover_traffic
    }

    /// Validate the configuration.
    ///
    /// Returns an error if the configuration is invalid (e.g., cover traffic
    /// without constant rate).
    pub fn validate(&self) -> Result<(), PrivacyConfigError> {
        // Cover traffic requires constant rate mode
        if self.cover_traffic && !self.constant_rate {
            return Err(PrivacyConfigError::CoverTrafficRequiresConstantRate);
        }

        // Onion routing should always be enabled for privacy
        if !self.onion_routing {
            return Err(PrivacyConfigError::OnionRoutingDisabled);
        }

        Ok(())
    }
}

/// Errors in privacy configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrivacyConfigError {
    /// Cover traffic is enabled but constant rate is not.
    CoverTrafficRequiresConstantRate,
    /// Onion routing is disabled (not recommended).
    OnionRoutingDisabled,
}

impl fmt::Display for PrivacyConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PrivacyConfigError::CoverTrafficRequiresConstantRate => {
                write!(f, "cover traffic requires constant rate mode to be enabled")
            }
            PrivacyConfigError::OnionRoutingDisabled => {
                write!(
                    f,
                    "onion routing is disabled; this removes privacy protection"
                )
            }
        }
    }
}

impl std::error::Error for PrivacyConfigError {}

/// Builder for creating custom privacy configurations.
#[derive(Debug, Clone)]
pub struct PrivacyConfigBuilder {
    config: PrivacyConfig,
}

impl PrivacyConfigBuilder {
    /// Create a new builder with default settings (Standard level).
    pub fn new() -> Self {
        Self {
            config: PrivacyConfig {
                onion_routing: true,
                padding: false,
                timing_jitter: false,
                constant_rate: false,
                cover_traffic: false,
                jitter_min_ms: 0,
                jitter_max_ms: 0,
            },
        }
    }

    /// Start from a specific privacy level.
    pub fn from_level(level: PrivacyLevel) -> Self {
        Self {
            config: level.to_config(),
        }
    }

    /// Enable or disable message padding.
    pub fn padding(mut self, enabled: bool) -> Self {
        self.config.padding = enabled;
        self
    }

    /// Enable or disable timing jitter.
    pub fn timing_jitter(mut self, enabled: bool) -> Self {
        self.config.timing_jitter = enabled;
        if enabled && self.config.jitter_min_ms == 0 && self.config.jitter_max_ms == 0 {
            self.config.jitter_min_ms = DEFAULT_JITTER_MIN_MS;
            self.config.jitter_max_ms = DEFAULT_JITTER_MAX_MS;
        }
        self
    }

    /// Set custom timing jitter range in milliseconds.
    pub fn jitter_range(mut self, min_ms: u64, max_ms: u64) -> Self {
        self.config.jitter_min_ms = min_ms;
        self.config.jitter_max_ms = max_ms;
        self.config.timing_jitter = max_ms > 0;
        self
    }

    /// Enable or disable constant rate transmission.
    pub fn constant_rate(mut self, enabled: bool) -> Self {
        self.config.constant_rate = enabled;
        self
    }

    /// Enable or disable cover traffic.
    ///
    /// Note: This also enables constant rate if not already enabled.
    pub fn cover_traffic(mut self, enabled: bool) -> Self {
        self.config.cover_traffic = enabled;
        if enabled {
            self.config.constant_rate = true;
        }
        self
    }

    /// Build the configuration.
    ///
    /// # Panics
    ///
    /// Panics if the configuration is invalid. Use [`build_validated`] for
    /// fallible construction.
    pub fn build(self) -> PrivacyConfig {
        self.build_validated()
            .expect("invalid privacy configuration")
    }

    /// Build and validate the configuration.
    pub fn build_validated(self) -> Result<PrivacyConfig, PrivacyConfigError> {
        self.config.validate()?;
        Ok(self.config)
    }
}

impl Default for PrivacyConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_privacy_level_default() {
        assert_eq!(PrivacyLevel::default(), PrivacyLevel::Standard);
    }

    #[test]
    fn test_standard_config() {
        let config = PrivacyLevel::Standard.to_config();
        assert!(config.onion_routing);
        assert!(!config.padding);
        assert!(!config.timing_jitter);
        assert!(!config.constant_rate);
        assert!(!config.cover_traffic);
    }

    #[test]
    fn test_enhanced_config() {
        let config = PrivacyLevel::Enhanced.to_config();
        assert!(config.onion_routing);
        assert!(config.padding);
        assert!(config.timing_jitter);
        assert!(!config.constant_rate);
        assert!(!config.cover_traffic);
    }

    #[test]
    fn test_maximum_config() {
        let config = PrivacyLevel::Maximum.to_config();
        assert!(config.onion_routing);
        assert!(config.padding);
        assert!(config.timing_jitter);
        assert!(config.constant_rate);
        assert!(config.cover_traffic);
    }

    #[test]
    fn test_level_from_str() {
        assert_eq!("standard".parse::<PrivacyLevel>().unwrap(), PrivacyLevel::Standard);
        assert_eq!("enhanced".parse::<PrivacyLevel>().unwrap(), PrivacyLevel::Enhanced);
        assert_eq!("maximum".parse::<PrivacyLevel>().unwrap(), PrivacyLevel::Maximum);

        // Aliases
        assert_eq!("std".parse::<PrivacyLevel>().unwrap(), PrivacyLevel::Standard);
        assert_eq!("high".parse::<PrivacyLevel>().unwrap(), PrivacyLevel::Enhanced);
        assert_eq!("max".parse::<PrivacyLevel>().unwrap(), PrivacyLevel::Maximum);
        assert_eq!("paranoid".parse::<PrivacyLevel>().unwrap(), PrivacyLevel::Maximum);
    }

    #[test]
    fn test_level_from_str_case_insensitive() {
        assert_eq!("STANDARD".parse::<PrivacyLevel>().unwrap(), PrivacyLevel::Standard);
        assert_eq!("Enhanced".parse::<PrivacyLevel>().unwrap(), PrivacyLevel::Enhanced);
        assert_eq!("MAXIMUM".parse::<PrivacyLevel>().unwrap(), PrivacyLevel::Maximum);
    }

    #[test]
    fn test_level_from_str_invalid() {
        assert!("invalid".parse::<PrivacyLevel>().is_err());
        assert!("".parse::<PrivacyLevel>().is_err());
    }

    #[test]
    fn test_level_display() {
        assert_eq!(PrivacyLevel::Standard.to_string(), "standard");
        assert_eq!(PrivacyLevel::Enhanced.to_string(), "enhanced");
        assert_eq!(PrivacyLevel::Maximum.to_string(), "maximum");
    }

    #[test]
    fn test_config_to_level() {
        assert_eq!(
            PrivacyLevel::Standard.to_config().to_level(),
            Some(PrivacyLevel::Standard)
        );
        assert_eq!(
            PrivacyLevel::Enhanced.to_config().to_level(),
            Some(PrivacyLevel::Enhanced)
        );
        assert_eq!(
            PrivacyLevel::Maximum.to_config().to_level(),
            Some(PrivacyLevel::Maximum)
        );
    }

    #[test]
    fn test_custom_config_no_level() {
        let config = PrivacyConfig::builder()
            .padding(true)
            .build();
        assert_eq!(config.to_level(), None);
    }

    #[test]
    fn test_builder_default() {
        let config = PrivacyConfigBuilder::new().build();
        assert!(config.onion_routing);
        assert!(!config.padding);
    }

    #[test]
    fn test_builder_from_level() {
        let config = PrivacyConfigBuilder::from_level(PrivacyLevel::Enhanced).build();
        assert!(config.padding);
        assert!(config.timing_jitter);
    }

    #[test]
    fn test_builder_custom() {
        let config = PrivacyConfig::builder()
            .padding(true)
            .timing_jitter(true)
            .build();

        assert!(config.padding);
        assert!(config.timing_jitter);
        assert!(!config.constant_rate);
    }

    #[test]
    fn test_builder_cover_enables_constant_rate() {
        let config = PrivacyConfig::builder()
            .cover_traffic(true)
            .build();

        assert!(config.cover_traffic);
        assert!(config.constant_rate); // Auto-enabled
    }

    #[test]
    fn test_config_validation_ok() {
        assert!(PrivacyLevel::Standard.to_config().validate().is_ok());
        assert!(PrivacyLevel::Enhanced.to_config().validate().is_ok());
        assert!(PrivacyLevel::Maximum.to_config().validate().is_ok());
    }

    #[test]
    fn test_config_validation_cover_without_constant() {
        let mut config = PrivacyConfig::default();
        config.cover_traffic = true;
        config.constant_rate = false;

        assert_eq!(
            config.validate(),
            Err(PrivacyConfigError::CoverTrafficRequiresConstantRate)
        );
    }

    #[test]
    fn test_config_validation_no_onion() {
        let mut config = PrivacyConfig::default();
        config.onion_routing = false;

        assert_eq!(
            config.validate(),
            Err(PrivacyConfigError::OnionRoutingDisabled)
        );
    }

    #[test]
    fn test_has_traffic_normalization() {
        assert!(!PrivacyLevel::Standard.to_config().has_traffic_normalization());
        assert!(PrivacyLevel::Enhanced.to_config().has_traffic_normalization());
        assert!(PrivacyLevel::Maximum.to_config().has_traffic_normalization());
    }

    #[test]
    fn test_config_serialization() {
        let config = PrivacyLevel::Enhanced.to_config();
        let json = serde_json::to_string(&config).unwrap();
        let parsed: PrivacyConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, parsed);
    }

    #[test]
    fn test_level_serialization() {
        let level = PrivacyLevel::Enhanced;
        let json = serde_json::to_string(&level).unwrap();
        assert_eq!(json, "\"enhanced\"");

        let parsed: PrivacyLevel = serde_json::from_str(&json).unwrap();
        assert_eq!(level, parsed);
    }

    #[test]
    fn test_all_levels() {
        let levels = PrivacyLevel::all();
        assert_eq!(levels.len(), 3);
        assert!(levels.contains(&PrivacyLevel::Standard));
        assert!(levels.contains(&PrivacyLevel::Enhanced));
        assert!(levels.contains(&PrivacyLevel::Maximum));
    }

    #[test]
    fn test_level_description() {
        assert!(!PrivacyLevel::Standard.description().is_empty());
        assert!(!PrivacyLevel::Enhanced.description().is_empty());
        assert!(!PrivacyLevel::Maximum.description().is_empty());
    }
}
