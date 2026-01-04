// Copyright (c) 2024 Botho Foundation

//! Transport type definitions for pluggable transports.
//!
//! This module defines the available transport types and their metadata.
//! Each transport type represents a different method of establishing
//! connections with peers, with different trade-offs for performance,
//! compatibility, and protocol obfuscation.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Available transport types for peer connections.
///
/// Different transport types offer different trade-offs:
/// - **Plain**: Standard TCP + Noise, best performance, no obfuscation
/// - **WebRTC**: Looks like video call traffic, good NAT traversal
/// - **TlsTunnel**: Looks like HTTPS traffic, good firewall traversal
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TransportType {
    /// Standard TCP + Noise transport (current default).
    ///
    /// This is the baseline transport with:
    /// - Lowest latency
    /// - Best throughput
    /// - No protocol obfuscation
    /// - May be blocked by DPI
    #[default]
    Plain,

    /// WebRTC data channel transport.
    ///
    /// Traffic looks like video calls:
    /// - Uses DTLS encryption
    /// - Built-in NAT traversal (ICE/STUN)
    /// - Indistinguishable from legitimate WebRTC
    /// - Higher connection setup latency
    WebRTC,

    /// TLS tunnel transport.
    ///
    /// Traffic looks like HTTPS:
    /// - Standard TLS 1.3
    /// - Good firewall compatibility
    /// - Alternative when WebRTC is blocked
    TlsTunnel,
}

impl TransportType {
    /// Get all available transport types.
    pub fn all() -> &'static [TransportType] {
        &[TransportType::Plain, TransportType::WebRTC, TransportType::TlsTunnel]
    }

    /// Get the human-readable name of this transport.
    pub fn name(&self) -> &'static str {
        match self {
            TransportType::Plain => "plain",
            TransportType::WebRTC => "webrtc",
            TransportType::TlsTunnel => "tls-tunnel",
        }
    }

    /// Get a description of this transport type.
    pub fn description(&self) -> &'static str {
        match self {
            TransportType::Plain => "Standard TCP + Noise (no obfuscation)",
            TransportType::WebRTC => "WebRTC data channels (looks like video calls)",
            TransportType::TlsTunnel => "TLS tunnel (looks like HTTPS)",
        }
    }

    /// Check if this transport provides protocol obfuscation.
    pub fn is_obfuscated(&self) -> bool {
        match self {
            TransportType::Plain => false,
            TransportType::WebRTC | TransportType::TlsTunnel => true,
        }
    }

    /// Get the expected connection setup overhead compared to plain TCP.
    ///
    /// Returns a multiplier (1.0 = same as plain, 2.0 = twice as long).
    pub fn setup_overhead(&self) -> f64 {
        match self {
            TransportType::Plain => 1.0,
            TransportType::WebRTC => 3.0, // ICE + DTLS handshake
            TransportType::TlsTunnel => 1.5, // TLS handshake
        }
    }
}

impl fmt::Display for TransportType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl std::str::FromStr for TransportType {
    type Err = TransportTypeParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "plain" | "tcp" | "noise" => Ok(TransportType::Plain),
            "webrtc" | "rtc" => Ok(TransportType::WebRTC),
            "tls-tunnel" | "tls" | "https" => Ok(TransportType::TlsTunnel),
            _ => Err(TransportTypeParseError(s.to_string())),
        }
    }
}

/// Error when parsing a transport type from string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportTypeParseError(String);

impl fmt::Display for TransportTypeParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid transport type '{}': expected 'plain', 'webrtc', or 'tls-tunnel'",
            self.0
        )
    }
}

impl std::error::Error for TransportTypeParseError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transport_type_default() {
        assert_eq!(TransportType::default(), TransportType::Plain);
    }

    #[test]
    fn test_transport_type_name() {
        assert_eq!(TransportType::Plain.name(), "plain");
        assert_eq!(TransportType::WebRTC.name(), "webrtc");
        assert_eq!(TransportType::TlsTunnel.name(), "tls-tunnel");
    }

    #[test]
    fn test_transport_type_display() {
        assert_eq!(TransportType::Plain.to_string(), "plain");
        assert_eq!(TransportType::WebRTC.to_string(), "webrtc");
        assert_eq!(TransportType::TlsTunnel.to_string(), "tls-tunnel");
    }

    #[test]
    fn test_transport_type_from_str() {
        assert_eq!("plain".parse::<TransportType>().unwrap(), TransportType::Plain);
        assert_eq!("webrtc".parse::<TransportType>().unwrap(), TransportType::WebRTC);
        assert_eq!("tls-tunnel".parse::<TransportType>().unwrap(), TransportType::TlsTunnel);

        // Aliases
        assert_eq!("tcp".parse::<TransportType>().unwrap(), TransportType::Plain);
        assert_eq!("rtc".parse::<TransportType>().unwrap(), TransportType::WebRTC);
        assert_eq!("tls".parse::<TransportType>().unwrap(), TransportType::TlsTunnel);
        assert_eq!("https".parse::<TransportType>().unwrap(), TransportType::TlsTunnel);
    }

    #[test]
    fn test_transport_type_from_str_case_insensitive() {
        assert_eq!("PLAIN".parse::<TransportType>().unwrap(), TransportType::Plain);
        assert_eq!("WebRTC".parse::<TransportType>().unwrap(), TransportType::WebRTC);
    }

    #[test]
    fn test_transport_type_from_str_invalid() {
        assert!("invalid".parse::<TransportType>().is_err());
        assert!("".parse::<TransportType>().is_err());
    }

    #[test]
    fn test_is_obfuscated() {
        assert!(!TransportType::Plain.is_obfuscated());
        assert!(TransportType::WebRTC.is_obfuscated());
        assert!(TransportType::TlsTunnel.is_obfuscated());
    }

    #[test]
    fn test_setup_overhead() {
        assert_eq!(TransportType::Plain.setup_overhead(), 1.0);
        assert!(TransportType::WebRTC.setup_overhead() > 1.0);
        assert!(TransportType::TlsTunnel.setup_overhead() > 1.0);
    }

    #[test]
    fn test_all_types() {
        let types = TransportType::all();
        assert_eq!(types.len(), 3);
        assert!(types.contains(&TransportType::Plain));
        assert!(types.contains(&TransportType::WebRTC));
        assert!(types.contains(&TransportType::TlsTunnel));
    }

    #[test]
    fn test_serialization() {
        let t = TransportType::WebRTC;
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(json, "\"webrtc\"");

        let parsed: TransportType = serde_json::from_str(&json).unwrap();
        assert_eq!(t, parsed);
    }

    #[test]
    fn test_description() {
        assert!(!TransportType::Plain.description().is_empty());
        assert!(!TransportType::WebRTC.description().is_empty());
        assert!(!TransportType::TlsTunnel.description().is_empty());
    }
}
