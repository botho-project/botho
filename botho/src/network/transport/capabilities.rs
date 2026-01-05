// Copyright (c) 2024 Botho Foundation

//! Transport capabilities advertising and parsing.
//!
//! This module provides structures for advertising transport capabilities
//! in peer discovery and parsing them from peer info.

use serde::{Deserialize, Serialize};

/// Supported transport types.
///
/// These represent the available transport mechanisms for P2P connections.
/// The enum is ordered by general preference (most private/performant first).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TransportType {
    /// WebRTC data channels - looks like video calls, good NAT traversal
    WebRTC,
    /// TLS 1.3 tunnel - looks like HTTPS
    TlsTunnel,
    /// Standard TCP + Noise (current default)
    Plain,
}

impl TransportType {
    /// Get a short string identifier for this transport type.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::WebRTC => "webrtc",
            Self::TlsTunnel => "tls",
            Self::Plain => "plain",
        }
    }

    /// Parse a transport type from its string identifier.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "webrtc" => Some(Self::WebRTC),
            "tls" | "tlstunnel" | "tls-tunnel" => Some(Self::TlsTunnel),
            "plain" | "tcp" | "noise" => Some(Self::Plain),
            _ => None,
        }
    }

    /// Get the default preference score for this transport.
    /// Higher scores are preferred.
    pub fn preference_score(&self) -> u8 {
        match self {
            Self::WebRTC => 100,
            Self::TlsTunnel => 75,
            Self::Plain => 50,
        }
    }
}

impl Default for TransportType {
    fn default() -> Self {
        Self::Plain
    }
}

impl std::fmt::Display for TransportType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// NAT type classification.
///
/// This affects which transports will work reliably and is used
/// in transport negotiation to select the best option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum NatType {
    /// No NAT or properly configured port forwarding
    Open,
    /// Full cone NAT - any external host can send packets
    FullCone,
    /// Restricted cone NAT - only hosts we've contacted can reply
    Restricted,
    /// Port-restricted cone NAT - only specific port can reply
    PortRestricted,
    /// Symmetric NAT - different mapping per destination
    Symmetric,
    /// NAT type not yet determined
    #[default]
    Unknown,
}

impl NatType {
    /// Returns true if this NAT type supports direct WebRTC connections.
    pub fn supports_webrtc_direct(&self) -> bool {
        matches!(self, Self::Open | Self::FullCone | Self::Restricted)
    }

    /// Returns true if WebRTC will likely work between two peers.
    pub fn webrtc_compatible_with(&self, other: &NatType) -> bool {
        // Symmetric to Symmetric is problematic
        if *self == NatType::Symmetric && *other == NatType::Symmetric {
            return false;
        }
        // At least one side should have reasonably open NAT
        self.supports_webrtc_direct() || other.supports_webrtc_direct()
    }

    /// Get a short string identifier for this NAT type.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::FullCone => "full-cone",
            Self::Restricted => "restricted",
            Self::PortRestricted => "port-restricted",
            Self::Symmetric => "symmetric",
            Self::Unknown => "unknown",
        }
    }

    /// Parse a NAT type from its string identifier.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "open" => Some(Self::Open),
            "full-cone" | "fullcone" => Some(Self::FullCone),
            "restricted" => Some(Self::Restricted),
            "port-restricted" | "portrestricted" => Some(Self::PortRestricted),
            "symmetric" => Some(Self::Symmetric),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

impl std::fmt::Display for NatType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Advertised transport capabilities for a peer.
///
/// This is included in peer discovery to allow negotiation of the best
/// transport between two peers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportCapabilities {
    /// Supported transport types in preference order
    pub supported: Vec<TransportType>,
    /// Currently preferred transport
    pub preferred: TransportType,
    /// NAT type (affects WebRTC success)
    pub nat_type: NatType,
    /// Protocol version for capabilities format
    pub version: u8,
}

impl TransportCapabilities {
    /// Current capabilities version.
    pub const VERSION: u8 = 1;

    /// Create new transport capabilities.
    pub fn new(supported: Vec<TransportType>, preferred: TransportType, nat_type: NatType) -> Self {
        Self {
            supported,
            preferred,
            nat_type,
            version: Self::VERSION,
        }
    }

    /// Create default capabilities (Plain transport only).
    pub fn plain_only() -> Self {
        Self {
            supported: vec![TransportType::Plain],
            preferred: TransportType::Plain,
            nat_type: NatType::Unknown,
            version: Self::VERSION,
        }
    }

    /// Create capabilities with all transports enabled.
    pub fn full(nat_type: NatType) -> Self {
        Self {
            supported: vec![
                TransportType::WebRTC,
                TransportType::TlsTunnel,
                TransportType::Plain,
            ],
            preferred: TransportType::WebRTC,
            nat_type,
            version: Self::VERSION,
        }
    }

    /// Check if a specific transport is supported.
    pub fn supports(&self, transport: TransportType) -> bool {
        self.supported.contains(&transport)
    }

    /// Encode capabilities as a multiaddr-compatible suffix string.
    ///
    /// Format: `/transport-caps/<version>/<transports>/<nat>`
    /// Example: `/transport-caps/1/webrtc,tls,plain/open`
    pub fn to_multiaddr_suffix(&self) -> String {
        let transports: Vec<&str> = self.supported.iter().map(|t| t.as_str()).collect();
        format!(
            "/transport-caps/{}/{}/{}",
            self.version,
            transports.join(","),
            self.nat_type.as_str()
        )
    }

    /// Parse capabilities from a multiaddr suffix.
    pub fn from_multiaddr_suffix(suffix: &str) -> Option<Self> {
        let parts: Vec<&str> = suffix.trim_start_matches('/').split('/').collect();
        if parts.len() != 4 || parts[0] != "transport-caps" {
            return None;
        }

        let version: u8 = parts[1].parse().ok()?;
        if version > Self::VERSION {
            // Unknown version - return None to allow graceful degradation
            return None;
        }

        let transports: Vec<TransportType> = parts[2]
            .split(',')
            .filter_map(TransportType::from_str)
            .collect();

        if transports.is_empty() {
            return None;
        }

        let nat_type = NatType::from_str(parts[3]).unwrap_or(NatType::Unknown);

        Some(Self {
            supported: transports.clone(),
            preferred: transports[0],
            nat_type,
            version,
        })
    }

    /// Parse capabilities from a peer's agent version string.
    ///
    /// Looks for transport capabilities in the agent version field.
    /// Format: `botho/1.0.0/5/transport-caps/1/webrtc,plain/open`
    pub fn from_agent_version(agent_version: &str) -> Option<Self> {
        // Find the transport-caps part
        if let Some(idx) = agent_version.find("/transport-caps/") {
            let suffix = &agent_version[idx..];
            return Self::from_multiaddr_suffix(suffix);
        }
        None
    }

    /// Get the preference ranking for a transport type (lower is better).
    fn preference_rank(&self, transport: TransportType) -> usize {
        self.supported
            .iter()
            .position(|&t| t == transport)
            .unwrap_or(usize::MAX)
    }

    /// Find the best common transport between two capability sets.
    ///
    /// Returns the transport that is:
    /// 1. Supported by both peers
    /// 2. Likely to work given NAT types
    /// 3. Highest preference on average
    pub fn best_common(&self, other: &TransportCapabilities) -> Option<TransportType> {
        // Find common transports
        let common: Vec<TransportType> = self
            .supported
            .iter()
            .copied()
            .filter(|t| other.supports(*t))
            .collect();

        if common.is_empty() {
            return None;
        }

        // Score each common transport
        let mut best: Option<(TransportType, i32)> = None;

        for transport in common {
            let mut score: i32 = 0;

            // Base score from transport preference
            score += transport.preference_score() as i32;

            // Preference ranking boost (prefer transports both sides want)
            let our_rank = self.preference_rank(transport);
            let their_rank = other.preference_rank(transport);
            score -= (our_rank + their_rank) as i32;

            // NAT compatibility check for WebRTC
            if transport == TransportType::WebRTC {
                if !self.nat_type.webrtc_compatible_with(&other.nat_type) {
                    // Significant penalty for likely connection failure
                    score -= 100;
                }
            }

            match &best {
                Some((_, best_score)) if score > *best_score => {
                    best = Some((transport, score));
                }
                None => {
                    best = Some((transport, score));
                }
                _ => {}
            }
        }

        best.map(|(t, _)| t)
    }
}

impl Default for TransportCapabilities {
    fn default() -> Self {
        Self::plain_only()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // TransportType tests
    // ========================================================================

    #[test]
    fn test_transport_type_as_str() {
        assert_eq!(TransportType::WebRTC.as_str(), "webrtc");
        assert_eq!(TransportType::TlsTunnel.as_str(), "tls");
        assert_eq!(TransportType::Plain.as_str(), "plain");
    }

    #[test]
    fn test_transport_type_from_str() {
        assert_eq!(TransportType::from_str("webrtc"), Some(TransportType::WebRTC));
        assert_eq!(TransportType::from_str("WEBRTC"), Some(TransportType::WebRTC));
        assert_eq!(TransportType::from_str("tls"), Some(TransportType::TlsTunnel));
        assert_eq!(TransportType::from_str("tlstunnel"), Some(TransportType::TlsTunnel));
        assert_eq!(TransportType::from_str("plain"), Some(TransportType::Plain));
        assert_eq!(TransportType::from_str("tcp"), Some(TransportType::Plain));
        assert_eq!(TransportType::from_str("invalid"), None);
    }

    #[test]
    fn test_transport_type_preference_score() {
        assert!(TransportType::WebRTC.preference_score() > TransportType::TlsTunnel.preference_score());
        assert!(TransportType::TlsTunnel.preference_score() > TransportType::Plain.preference_score());
    }

    #[test]
    fn test_transport_type_default() {
        assert_eq!(TransportType::default(), TransportType::Plain);
    }

    #[test]
    fn test_transport_type_display() {
        assert_eq!(format!("{}", TransportType::WebRTC), "webrtc");
        assert_eq!(format!("{}", TransportType::TlsTunnel), "tls");
        assert_eq!(format!("{}", TransportType::Plain), "plain");
    }

    // ========================================================================
    // NatType tests
    // ========================================================================

    #[test]
    fn test_nat_type_supports_webrtc_direct() {
        assert!(NatType::Open.supports_webrtc_direct());
        assert!(NatType::FullCone.supports_webrtc_direct());
        assert!(NatType::Restricted.supports_webrtc_direct());
        assert!(!NatType::PortRestricted.supports_webrtc_direct());
        assert!(!NatType::Symmetric.supports_webrtc_direct());
        assert!(!NatType::Unknown.supports_webrtc_direct());
    }

    #[test]
    fn test_nat_type_webrtc_compatible() {
        // Open to anything works
        assert!(NatType::Open.webrtc_compatible_with(&NatType::Symmetric));
        assert!(NatType::Symmetric.webrtc_compatible_with(&NatType::Open));

        // Symmetric to Symmetric doesn't work
        assert!(!NatType::Symmetric.webrtc_compatible_with(&NatType::Symmetric));

        // Two open NATs work
        assert!(NatType::Open.webrtc_compatible_with(&NatType::Open));
    }

    #[test]
    fn test_nat_type_as_str() {
        assert_eq!(NatType::Open.as_str(), "open");
        assert_eq!(NatType::FullCone.as_str(), "full-cone");
        assert_eq!(NatType::Symmetric.as_str(), "symmetric");
    }

    #[test]
    fn test_nat_type_from_str() {
        assert_eq!(NatType::from_str("open"), Some(NatType::Open));
        assert_eq!(NatType::from_str("full-cone"), Some(NatType::FullCone));
        assert_eq!(NatType::from_str("fullcone"), Some(NatType::FullCone));
        assert_eq!(NatType::from_str("invalid"), None);
    }

    #[test]
    fn test_nat_type_default() {
        assert_eq!(NatType::default(), NatType::Unknown);
    }

    // ========================================================================
    // TransportCapabilities tests
    // ========================================================================

    #[test]
    fn test_capabilities_plain_only() {
        let caps = TransportCapabilities::plain_only();
        assert!(caps.supports(TransportType::Plain));
        assert!(!caps.supports(TransportType::WebRTC));
        assert_eq!(caps.preferred, TransportType::Plain);
    }

    #[test]
    fn test_capabilities_full() {
        let caps = TransportCapabilities::full(NatType::Open);
        assert!(caps.supports(TransportType::WebRTC));
        assert!(caps.supports(TransportType::TlsTunnel));
        assert!(caps.supports(TransportType::Plain));
        assert_eq!(caps.nat_type, NatType::Open);
    }

    #[test]
    fn test_capabilities_to_multiaddr_suffix() {
        let caps = TransportCapabilities::new(
            vec![TransportType::WebRTC, TransportType::Plain],
            TransportType::WebRTC,
            NatType::Open,
        );
        let suffix = caps.to_multiaddr_suffix();
        assert_eq!(suffix, "/transport-caps/1/webrtc,plain/open");
    }

    #[test]
    fn test_capabilities_from_multiaddr_suffix() {
        let suffix = "/transport-caps/1/webrtc,tls,plain/restricted";
        let caps = TransportCapabilities::from_multiaddr_suffix(suffix).unwrap();

        assert_eq!(caps.version, 1);
        assert_eq!(caps.supported.len(), 3);
        assert!(caps.supports(TransportType::WebRTC));
        assert_eq!(caps.nat_type, NatType::Restricted);
    }

    #[test]
    fn test_capabilities_from_multiaddr_suffix_invalid() {
        assert!(TransportCapabilities::from_multiaddr_suffix("invalid").is_none());
        assert!(TransportCapabilities::from_multiaddr_suffix("/transport-caps/999/webrtc/open").is_none());
        assert!(TransportCapabilities::from_multiaddr_suffix("/transport-caps/1//open").is_none());
    }

    #[test]
    fn test_capabilities_roundtrip() {
        let original = TransportCapabilities::full(NatType::FullCone);
        let suffix = original.to_multiaddr_suffix();
        let parsed = TransportCapabilities::from_multiaddr_suffix(&suffix).unwrap();

        assert_eq!(parsed.supported, original.supported);
        assert_eq!(parsed.nat_type, original.nat_type);
        assert_eq!(parsed.version, original.version);
    }

    #[test]
    fn test_capabilities_from_agent_version() {
        let agent = "botho/1.0.0/5/transport-caps/1/webrtc,plain/open";
        let caps = TransportCapabilities::from_agent_version(agent).unwrap();

        assert!(caps.supports(TransportType::WebRTC));
        assert!(caps.supports(TransportType::Plain));
        assert_eq!(caps.nat_type, NatType::Open);
    }

    #[test]
    fn test_capabilities_from_agent_version_no_caps() {
        let agent = "botho/1.0.0/5";
        assert!(TransportCapabilities::from_agent_version(agent).is_none());
    }

    #[test]
    fn test_best_common_simple() {
        let caps1 = TransportCapabilities::new(
            vec![TransportType::WebRTC, TransportType::Plain],
            TransportType::WebRTC,
            NatType::Open,
        );
        let caps2 = TransportCapabilities::new(
            vec![TransportType::Plain],
            TransportType::Plain,
            NatType::Open,
        );

        let best = caps1.best_common(&caps2);
        assert_eq!(best, Some(TransportType::Plain));
    }

    #[test]
    fn test_best_common_webrtc_preferred() {
        let caps1 = TransportCapabilities::full(NatType::Open);
        let caps2 = TransportCapabilities::full(NatType::FullCone);

        let best = caps1.best_common(&caps2);
        assert_eq!(best, Some(TransportType::WebRTC));
    }

    #[test]
    fn test_best_common_webrtc_nat_penalty() {
        let caps1 = TransportCapabilities::full(NatType::Symmetric);
        let caps2 = TransportCapabilities::full(NatType::Symmetric);

        // WebRTC should be penalized due to symmetric NAT on both sides
        let best = caps1.best_common(&caps2);
        // Should fall back to TLS tunnel
        assert_eq!(best, Some(TransportType::TlsTunnel));
    }

    #[test]
    fn test_best_common_no_common() {
        let caps1 = TransportCapabilities::new(
            vec![TransportType::WebRTC],
            TransportType::WebRTC,
            NatType::Open,
        );
        let caps2 = TransportCapabilities::new(
            vec![TransportType::TlsTunnel],
            TransportType::TlsTunnel,
            NatType::Open,
        );

        let best = caps1.best_common(&caps2);
        assert!(best.is_none());
    }

    #[test]
    fn test_capabilities_default() {
        let caps = TransportCapabilities::default();
        assert_eq!(caps.supported, vec![TransportType::Plain]);
        assert_eq!(caps.preferred, TransportType::Plain);
        assert_eq!(caps.nat_type, NatType::Unknown);
    }

    #[test]
    fn test_capabilities_serialization() {
        let caps = TransportCapabilities::full(NatType::Open);
        let serialized = bincode::serialize(&caps).unwrap();
        let deserialized: TransportCapabilities = bincode::deserialize(&serialized).unwrap();

        assert_eq!(caps, deserialized);
    }
}
