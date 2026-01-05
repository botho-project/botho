// Copyright (c) 2024 Botho Foundation

//! Privacy configuration for traffic normalization.
//!
//! This module implements Phase 2.7 of the traffic privacy roadmap.
//! All clients use maximum privacy by default, with all traffic normalization
//! features enabled to ensure consistent, strong privacy guarantees across
//! the network.
//!
//! # Privacy Features
//!
//! All clients have the following features enabled by default:
//!
//! | Feature | Description |
//! |---------|-------------|
//! | Onion routing | 3-hop circuits hide transaction origin |
//! | Message padding | Fixed bucket sizes prevent size analysis |
//! | Timing jitter | Random delays prevent timing correlation |
//! | Constant-rate | Queue-based transmission prevents volume analysis |
//! | Cover traffic | Dummy messages when idle maintain uniform patterns |
//!
//! # Example
//!
//! ```
//! use botho::network::privacy::config::PrivacyConfig;
//!
//! // Default config has all features enabled
//! let config = PrivacyConfig::default();
//! assert!(config.padding);
//! assert!(config.timing_jitter);
//! assert!(config.constant_rate);
//! assert!(config.cover_traffic);
//!
//! // Create custom config for testing
//! let custom = PrivacyConfig::builder()
//!     .padding(true)
//!     .timing_jitter(true)
//!     .build();
//! ```

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::network::transport::config::{
    TlsTransportConfig, TransportConfig, TransportPreference, WebRtcTransportConfig,
};

/// Default minimum jitter delay in milliseconds.
pub const DEFAULT_JITTER_MIN_MS: u64 = 100;

/// Default maximum jitter delay in milliseconds.
pub const DEFAULT_JITTER_MAX_MS: u64 = 300;

/// Configuration for privacy features.
///
/// Controls which traffic normalization features are enabled. By default,
/// all features are enabled to provide maximum privacy. The builder can
/// be used to create custom configurations for testing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivacyConfig {
    /// Enable onion routing for private-path messages.
    /// This is the foundation of privacy and should always be true.
    #[serde(default = "default_true")]
    pub onion_routing: bool,

    /// Pad messages to fixed bucket sizes.
    /// Prevents size-based traffic analysis.
    #[serde(default = "default_true")]
    pub padding: bool,

    /// Add random timing delays before sending.
    /// Prevents timing correlation attacks.
    #[serde(default = "default_true")]
    pub timing_jitter: bool,

    /// Send messages at a constant rate (queue-based).
    /// Prevents traffic volume analysis.
    #[serde(default = "default_true")]
    pub constant_rate: bool,

    /// Generate cover traffic when idle.
    /// Makes traffic patterns indistinguishable.
    #[serde(default = "default_true")]
    pub cover_traffic: bool,

    /// Minimum timing jitter delay in milliseconds.
    #[serde(default = "default_jitter_min")]
    pub jitter_min_ms: u64,

    /// Maximum timing jitter delay in milliseconds.
    #[serde(default = "default_jitter_max")]
    pub jitter_max_ms: u64,
}

fn default_true() -> bool {
    true
}

fn default_jitter_min() -> u64 {
    DEFAULT_JITTER_MIN_MS
}

fn default_jitter_max() -> u64 {
    DEFAULT_JITTER_MAX_MS
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            onion_routing: true,
            padding: true,
            timing_jitter: true,
            constant_rate: true,
            cover_traffic: true,
            jitter_min_ms: DEFAULT_JITTER_MIN_MS,
            jitter_max_ms: DEFAULT_JITTER_MAX_MS,
        }
    }
}

impl PrivacyConfig {
    /// Create a new builder for custom configuration.
    pub fn builder() -> PrivacyConfigBuilder {
        PrivacyConfigBuilder::new()
    }

    /// Check if any privacy features beyond onion routing are enabled.
    pub fn has_traffic_normalization(&self) -> bool {
        self.padding || self.timing_jitter || self.constant_rate || self.cover_traffic
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

    /// Get recommended transport configuration for this privacy configuration.
    ///
    /// Returns a `TransportConfig` optimized for the privacy settings:
    /// - Full privacy features: Enables all obfuscated transports (WebRTC, TLS)
    /// - Traffic normalization enabled: Prefers privacy over performance
    /// - Minimal features: Basic transport config
    ///
    /// # Example
    ///
    /// ```
    /// use botho::network::privacy::config::PrivacyConfig;
    ///
    /// // Default config has all features enabled, recommends obfuscated transports
    /// let privacy_config = PrivacyConfig::default();
    /// let transport_config = privacy_config.transport_config();
    ///
    /// assert!(transport_config.enable_webrtc);
    /// assert!(transport_config.enable_tls_tunnel);
    /// ```
    pub fn transport_config(&self) -> TransportConfig {
        // Default privacy config has all features enabled (maximum privacy)
        // This means we should use obfuscated transports
        if self.has_traffic_normalization() {
            // Full privacy - enable all obfuscated transports
            TransportConfig {
                preference: TransportPreference::Privacy,
                enable_webrtc: true,
                enable_tls_tunnel: true,
                webrtc_config: Some(WebRtcTransportConfig::default()),
                tls_config: Some(TlsTransportConfig::default()),
                enable_metrics: true,
                enable_fallback: true,
                max_fallback_attempts: 3,
                connect_timeout_secs: 30,
            }
        } else {
            // Minimal privacy - just onion routing, plain transport is fine
            TransportConfig {
                preference: TransportPreference::Performance,
                enable_webrtc: false,
                enable_tls_tunnel: false,
                webrtc_config: None,
                tls_config: None,
                enable_metrics: true,
                enable_fallback: true,
                max_fallback_attempts: 3,
                connect_timeout_secs: 30,
            }
        }
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
///
/// Starts with a minimal configuration (onion routing only) so tests can
/// enable specific features as needed. For production use, prefer
/// `PrivacyConfig::default()` which has all features enabled.
#[derive(Debug, Clone)]
pub struct PrivacyConfigBuilder {
    config: PrivacyConfig,
}

impl PrivacyConfigBuilder {
    /// Create a new builder with minimal settings (onion routing only).
    ///
    /// This is useful for testing specific feature combinations.
    /// For production, use `PrivacyConfig::default()` instead.
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
    fn test_default_config_has_all_features() {
        let config = PrivacyConfig::default();
        assert!(config.onion_routing);
        assert!(config.padding);
        assert!(config.timing_jitter);
        assert!(config.constant_rate);
        assert!(config.cover_traffic);
        assert_eq!(config.jitter_min_ms, DEFAULT_JITTER_MIN_MS);
        assert_eq!(config.jitter_max_ms, DEFAULT_JITTER_MAX_MS);
    }

    #[test]
    fn test_builder_starts_minimal() {
        let config = PrivacyConfigBuilder::new().build();
        assert!(config.onion_routing);
        assert!(!config.padding);
        assert!(!config.timing_jitter);
        assert!(!config.constant_rate);
        assert!(!config.cover_traffic);
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
        let config = PrivacyConfig::builder().cover_traffic(true).build();

        assert!(config.cover_traffic);
        assert!(config.constant_rate); // Auto-enabled
    }

    #[test]
    fn test_config_validation_ok() {
        assert!(PrivacyConfig::default().validate().is_ok());

        // Builder with valid config
        let config = PrivacyConfig::builder()
            .padding(true)
            .timing_jitter(true)
            .build();
        assert!(config.validate().is_ok());
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
        // Default config has all normalization features
        assert!(PrivacyConfig::default().has_traffic_normalization());

        // Minimal config from builder has no normalization
        let minimal = PrivacyConfigBuilder::new().build();
        assert!(!minimal.has_traffic_normalization());

        // Adding any feature enables normalization
        let with_padding = PrivacyConfig::builder().padding(true).build();
        assert!(with_padding.has_traffic_normalization());
    }

    #[test]
    fn test_config_serialization() {
        let config = PrivacyConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let parsed: PrivacyConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, parsed);
    }

    #[test]
    fn test_config_deserialization_with_defaults() {
        // Empty JSON should deserialize with all features enabled
        let json = "{}";
        let config: PrivacyConfig = serde_json::from_str(json).unwrap();
        assert!(config.onion_routing);
        assert!(config.padding);
        assert!(config.timing_jitter);
        assert!(config.constant_rate);
        assert!(config.cover_traffic);
    }

    #[test]
    fn test_jitter_range_builder() {
        let config = PrivacyConfig::builder().jitter_range(50, 150).build();

        assert!(config.timing_jitter);
        assert_eq!(config.jitter_min_ms, 50);
        assert_eq!(config.jitter_max_ms, 150);
    }

    #[test]
    fn test_builder_default_impl() {
        let config = PrivacyConfigBuilder::default().build();
        assert!(config.onion_routing);
        assert!(!config.padding);
    }

    #[test]
    fn test_transport_config_full_privacy() {
        // Default config has all features enabled (maximum privacy)
        let privacy_config = PrivacyConfig::default();
        let transport_config = privacy_config.transport_config();

        assert!(transport_config.enable_webrtc);
        assert!(transport_config.enable_tls_tunnel);
        assert_eq!(transport_config.preference, TransportPreference::Privacy);
    }

    #[test]
    fn test_transport_config_minimal_privacy() {
        // Minimal config from builder has no normalization
        let privacy_config = PrivacyConfigBuilder::new().build();
        let transport_config = privacy_config.transport_config();

        assert!(!transport_config.enable_webrtc);
        assert!(!transport_config.enable_tls_tunnel);
        assert_eq!(
            transport_config.preference,
            TransportPreference::Performance
        );
    }

    #[test]
    fn test_transport_config_with_normalization() {
        // Adding any normalization feature enables obfuscated transports
        let privacy_config = PrivacyConfig::builder().padding(true).build();
        let transport_config = privacy_config.transport_config();

        assert!(transport_config.enable_webrtc);
        assert!(transport_config.enable_tls_tunnel);
    }
}
