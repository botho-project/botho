// Copyright (c) 2024 Botho Foundation

//! Transport configuration for user preferences and selection.
//!
//! This module provides configuration structures for transport selection,
//! allowing users to specify their preferences for which transports to use
//! and how they should be selected.
//!
//! # Example
//!
//! ```
//! use botho::network::transport::config::{TransportConfig, TransportPreference};
//! use botho::network::transport::TransportType;
//!
//! // Create a config preferring obfuscated transports
//! let config = TransportConfig::builder()
//!     .preferred(TransportType::WebRTC)
//!     .enable_webrtc(true)
//!     .enable_tls_tunnel(true)
//!     .build();
//!
//! assert!(config.enable_webrtc);
//! assert!(config.enable_tls_tunnel);
//! ```

use serde::{Deserialize, Serialize};

use super::types::TransportType;

/// User's preference for transport selection.
///
/// This affects how the transport manager chooses between available transports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportPreference {
    /// Prefer performance (lower latency, higher throughput).
    /// Will use Plain transport when available.
    Performance,

    /// Prefer privacy (obfuscated transports).
    /// Will prefer WebRTC or TLS tunnel over plain.
    #[default]
    Privacy,

    /// Prefer compatibility (widest NAT traversal).
    /// Will prefer transports that work through restrictive NATs.
    Compatibility,

    /// Use a specific transport exclusively.
    Specific(TransportType),
}

impl TransportPreference {
    /// Get the preference score for a transport type.
    ///
    /// Higher scores are preferred. This score is added to the base transport
    /// preference score during selection.
    pub fn score_for(&self, transport: TransportType) -> i32 {
        match self {
            TransportPreference::Performance => match transport {
                TransportType::Plain => 50,
                TransportType::TlsTunnel => 20,
                TransportType::WebRTC => 10,
            },
            TransportPreference::Privacy => match transport {
                TransportType::WebRTC => 50,
                TransportType::TlsTunnel => 40,
                TransportType::Plain => 0,
            },
            TransportPreference::Compatibility => match transport {
                TransportType::WebRTC => 40, // Good NAT traversal
                TransportType::TlsTunnel => 30,
                TransportType::Plain => 20,
            },
            TransportPreference::Specific(specific) => {
                if transport == *specific {
                    100
                } else {
                    -100
                }
            }
        }
    }
}

/// Configuration for transport selection.
///
/// This structure controls which transports are enabled and how they
/// should be selected based on user preferences.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportConfig {
    /// User's preferred transport selection strategy.
    #[serde(default)]
    pub preference: TransportPreference,

    /// Enable WebRTC transport.
    #[serde(default)]
    pub enable_webrtc: bool,

    /// Enable TLS tunnel transport.
    #[serde(default)]
    pub enable_tls_tunnel: bool,

    /// WebRTC configuration (if enabled).
    #[serde(default)]
    pub webrtc_config: Option<WebRtcTransportConfig>,

    /// TLS configuration (if enabled).
    #[serde(default)]
    pub tls_config: Option<TlsTransportConfig>,

    /// Enable metrics-based transport selection.
    ///
    /// When enabled, the transport manager will track success rates and
    /// latencies to improve selection over time.
    #[serde(default = "default_true")]
    pub enable_metrics: bool,

    /// Enable automatic fallback on connection failures.
    ///
    /// When a preferred transport fails, try the next best transport.
    #[serde(default = "default_true")]
    pub enable_fallback: bool,

    /// Maximum fallback attempts before giving up.
    #[serde(default = "default_fallback_attempts")]
    pub max_fallback_attempts: u32,

    /// Connection timeout in seconds for transport connections.
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_secs: u64,
}

fn default_true() -> bool {
    true
}

fn default_fallback_attempts() -> u32 {
    3
}

fn default_connect_timeout() -> u64 {
    30
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            preference: TransportPreference::default(),
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

impl TransportConfig {
    /// Create a new builder for transport configuration.
    pub fn builder() -> TransportConfigBuilder {
        TransportConfigBuilder::default()
    }

    /// Create a minimal config with only plain transport.
    pub fn plain_only() -> Self {
        Self::default()
    }

    /// Create a config with all transports enabled.
    pub fn full() -> Self {
        Self {
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
    }

    /// Get the list of enabled transport types in preference order.
    pub fn enabled_transports(&self) -> Vec<TransportType> {
        let mut transports = Vec::new();

        // Add transports based on preference
        match self.preference {
            TransportPreference::Performance => {
                transports.push(TransportType::Plain);
                if self.enable_tls_tunnel {
                    transports.push(TransportType::TlsTunnel);
                }
                if self.enable_webrtc {
                    transports.push(TransportType::WebRTC);
                }
            }
            TransportPreference::Privacy | TransportPreference::Compatibility => {
                if self.enable_webrtc {
                    transports.push(TransportType::WebRTC);
                }
                if self.enable_tls_tunnel {
                    transports.push(TransportType::TlsTunnel);
                }
                transports.push(TransportType::Plain);
            }
            TransportPreference::Specific(specific) => {
                // Only include the specific transport if enabled
                match specific {
                    TransportType::Plain => transports.push(TransportType::Plain),
                    TransportType::WebRTC if self.enable_webrtc => {
                        transports.push(TransportType::WebRTC)
                    }
                    TransportType::TlsTunnel if self.enable_tls_tunnel => {
                        transports.push(TransportType::TlsTunnel)
                    }
                    _ => {
                        // Fallback to plain if specific transport isn't enabled
                        transports.push(TransportType::Plain);
                    }
                }
            }
        }

        transports
    }

    /// Check if a transport type is enabled.
    pub fn is_enabled(&self, transport: TransportType) -> bool {
        match transport {
            TransportType::Plain => true, // Always available
            TransportType::WebRTC => self.enable_webrtc,
            TransportType::TlsTunnel => self.enable_tls_tunnel,
        }
    }
}

/// Builder for transport configuration.
#[derive(Debug, Clone, Default)]
pub struct TransportConfigBuilder {
    config: TransportConfig,
}

impl TransportConfigBuilder {
    /// Set the transport preference.
    pub fn preference(mut self, preference: TransportPreference) -> Self {
        self.config.preference = preference;
        self
    }

    /// Set the preferred transport type.
    pub fn preferred(mut self, transport: TransportType) -> Self {
        self.config.preference = TransportPreference::Specific(transport);
        self
    }

    /// Enable or disable WebRTC transport.
    pub fn enable_webrtc(mut self, enable: bool) -> Self {
        self.config.enable_webrtc = enable;
        if enable && self.config.webrtc_config.is_none() {
            self.config.webrtc_config = Some(WebRtcTransportConfig::default());
        }
        self
    }

    /// Enable or disable TLS tunnel transport.
    pub fn enable_tls_tunnel(mut self, enable: bool) -> Self {
        self.config.enable_tls_tunnel = enable;
        if enable && self.config.tls_config.is_none() {
            self.config.tls_config = Some(TlsTransportConfig::default());
        }
        self
    }

    /// Set WebRTC configuration.
    pub fn webrtc_config(mut self, config: WebRtcTransportConfig) -> Self {
        self.config.webrtc_config = Some(config);
        self.config.enable_webrtc = true;
        self
    }

    /// Set TLS configuration.
    pub fn tls_config(mut self, config: TlsTransportConfig) -> Self {
        self.config.tls_config = Some(config);
        self.config.enable_tls_tunnel = true;
        self
    }

    /// Enable or disable metrics-based selection.
    pub fn enable_metrics(mut self, enable: bool) -> Self {
        self.config.enable_metrics = enable;
        self
    }

    /// Enable or disable automatic fallback.
    pub fn enable_fallback(mut self, enable: bool) -> Self {
        self.config.enable_fallback = enable;
        self
    }

    /// Set maximum fallback attempts.
    pub fn max_fallback_attempts(mut self, attempts: u32) -> Self {
        self.config.max_fallback_attempts = attempts;
        self
    }

    /// Set connection timeout.
    pub fn connect_timeout_secs(mut self, timeout: u64) -> Self {
        self.config.connect_timeout_secs = timeout;
        self
    }

    /// Build the configuration.
    pub fn build(self) -> TransportConfig {
        self.config
    }
}

/// WebRTC transport configuration.
///
/// This is a serializable configuration for WebRTC transport.
/// It will be converted to `IceConfig` when creating the transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebRtcTransportConfig {
    /// STUN server addresses for NAT traversal.
    #[serde(default = "default_stun_servers")]
    pub stun_servers: Vec<String>,

    /// ICE connection timeout in seconds.
    #[serde(default = "default_ice_timeout")]
    pub ice_timeout_secs: u64,

    /// Maximum number of ICE candidates to gather.
    #[serde(default = "default_max_candidates")]
    pub max_candidates: usize,
}

fn default_stun_servers() -> Vec<String> {
    vec![
        "stun:stun.l.google.com:19302".to_string(),
        "stun:stun1.l.google.com:19302".to_string(),
    ]
}

fn default_ice_timeout() -> u64 {
    30
}

fn default_max_candidates() -> usize {
    10
}

impl Default for WebRtcTransportConfig {
    fn default() -> Self {
        Self {
            stun_servers: default_stun_servers(),
            ice_timeout_secs: default_ice_timeout(),
            max_candidates: default_max_candidates(),
        }
    }
}

/// TLS transport configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsTransportConfig {
    /// Server name for SNI (Server Name Indication).
    /// If not set, uses a random common domain to blend with HTTPS traffic.
    pub server_name: Option<String>,

    /// Whether to verify server certificates.
    /// Default is true for production, false for testing.
    #[serde(default = "default_true")]
    pub verify_certificates: bool,

    /// Custom CA certificates (PEM format).
    pub custom_ca_certs: Option<Vec<String>>,
}

impl Default for TlsTransportConfig {
    fn default() -> Self {
        Self {
            server_name: None,
            verify_certificates: true,
            custom_ca_certs: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transport_preference_default() {
        assert_eq!(TransportPreference::default(), TransportPreference::Privacy);
    }

    #[test]
    fn test_transport_preference_scores() {
        // Privacy preference should favor WebRTC
        let privacy = TransportPreference::Privacy;
        assert!(privacy.score_for(TransportType::WebRTC) > privacy.score_for(TransportType::Plain));

        // Performance preference should favor Plain
        let perf = TransportPreference::Performance;
        assert!(perf.score_for(TransportType::Plain) > perf.score_for(TransportType::WebRTC));

        // Specific should heavily favor the specific transport
        let specific = TransportPreference::Specific(TransportType::TlsTunnel);
        assert_eq!(specific.score_for(TransportType::TlsTunnel), 100);
        assert_eq!(specific.score_for(TransportType::Plain), -100);
    }

    #[test]
    fn test_transport_config_default() {
        let config = TransportConfig::default();
        assert!(!config.enable_webrtc);
        assert!(!config.enable_tls_tunnel);
        assert!(config.enable_metrics);
        assert!(config.enable_fallback);
        assert_eq!(config.max_fallback_attempts, 3);
    }

    #[test]
    fn test_transport_config_full() {
        let config = TransportConfig::full();
        assert!(config.enable_webrtc);
        assert!(config.enable_tls_tunnel);
        assert!(config.webrtc_config.is_some());
        assert!(config.tls_config.is_some());
    }

    #[test]
    fn test_transport_config_builder() {
        let config = TransportConfig::builder()
            .preference(TransportPreference::Privacy)
            .enable_webrtc(true)
            .enable_tls_tunnel(true)
            .max_fallback_attempts(5)
            .build();

        assert_eq!(config.preference, TransportPreference::Privacy);
        assert!(config.enable_webrtc);
        assert!(config.enable_tls_tunnel);
        assert_eq!(config.max_fallback_attempts, 5);
    }

    #[test]
    fn test_enabled_transports_privacy() {
        let config = TransportConfig::builder()
            .preference(TransportPreference::Privacy)
            .enable_webrtc(true)
            .enable_tls_tunnel(true)
            .build();

        let transports = config.enabled_transports();
        assert_eq!(transports[0], TransportType::WebRTC);
        assert_eq!(transports[1], TransportType::TlsTunnel);
        assert_eq!(transports[2], TransportType::Plain);
    }

    #[test]
    fn test_enabled_transports_performance() {
        let config = TransportConfig::builder()
            .preference(TransportPreference::Performance)
            .enable_webrtc(true)
            .enable_tls_tunnel(true)
            .build();

        let transports = config.enabled_transports();
        assert_eq!(transports[0], TransportType::Plain);
    }

    #[test]
    fn test_enabled_transports_specific() {
        let config = TransportConfig::builder()
            .preferred(TransportType::WebRTC)
            .enable_webrtc(true)
            .build();

        let transports = config.enabled_transports();
        assert_eq!(transports.len(), 1);
        assert_eq!(transports[0], TransportType::WebRTC);
    }

    #[test]
    fn test_is_enabled() {
        let config = TransportConfig::builder().enable_webrtc(true).build();

        assert!(config.is_enabled(TransportType::Plain));
        assert!(config.is_enabled(TransportType::WebRTC));
        assert!(!config.is_enabled(TransportType::TlsTunnel));
    }

    #[test]
    fn test_config_serialization() {
        let config = TransportConfig::full();
        let json = serde_json::to_string(&config).unwrap();
        let parsed: TransportConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(config.enable_webrtc, parsed.enable_webrtc);
        assert_eq!(config.enable_tls_tunnel, parsed.enable_tls_tunnel);
    }
}
