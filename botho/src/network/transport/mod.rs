// Copyright (c) 2024 Botho Foundation

//! Pluggable transport layer for protocol obfuscation.
//!
//! This module implements Phase 3 of the traffic privacy roadmap:
//! a pluggable transport interface that allows different transport
//! implementations to be used interchangeably.
//!
//! # Overview
//!
//! The transport layer provides an abstraction over the raw network
//! connection, allowing botho to use different protocols that are
//! harder to detect and block:
//!
//! - **Plain**: Standard TCP + Noise (default, best performance)
//! - **WebRTC**: Looks like video call traffic (Phase 3.2)
//! - **TLS Tunnel**: Looks like HTTPS traffic (Phase 3.7)
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    APPLICATION LAYER                        │
//! │                    (Gossipsub, SCP)                         │
//! └──────────────────────────┬──────────────────────────────────┘
//!                            │
//! ┌──────────────────────────▼──────────────────────────────────┐
//! │                  TRANSPORT LAYER                            │
//! │  ┌─────────────────────────────────────────────────────┐    │
//! │  │            PluggableTransport Trait                 │    │
//! │  └─────────────────────────────────────────────────────┘    │
//! │         │                    │                    │         │
//! │  ┌──────▼──────┐     ┌───────▼───────┐    ┌───────▼──────┐  │
//! │  │    Plain    │     │    WebRTC     │    │  TLS Tunnel  │  │
//! │  │ TCP + Noise │     │ DTLS + SCTP   │    │   TLS 1.3    │  │
//! │  └─────────────┘     └───────────────┘    └──────────────┘  │
//! └──────────────────────────┬──────────────────────────────────┘
//!                            │
//! ┌──────────────────────────▼──────────────────────────────────┐
//! │                    NETWORK LAYER                            │
//! │                    (TCP, UDP)                               │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Modules
//!
//! - [`capabilities`]: Transport capabilities advertising and parsing
//! - [`negotiation`]: Transport negotiation protocol between peers
//! - [`signaling`]: SDP exchange for WebRTC connection establishment (Phase
//!   3.5)
//! - [`webrtc`]: WebRTC data channel transport (Phase 3.2)
//! - [`tls_tunnel`]: TLS tunnel transport (Phase 3.7)
//! - [`http2`]: Optional HTTP/2 framing for maximum obfuscation
//! - [`plain`]: Standard TCP + Noise transport
//!
//! # Usage
//!
//! ```
//! use botho::network::transport::{
//!     PlainTransport, PluggableTransport, TransportType,
//! };
//!
//! // Create the default transport
//! let transport = PlainTransport::new();
//! assert_eq!(transport.transport_type(), TransportType::Plain);
//! assert_eq!(transport.name(), "plain");
//!
//! // Check transport properties
//! assert!(!transport.transport_type().is_obfuscated());
//! assert!(transport.is_available());
//! ```
//!
//! # Transport Selection
//!
//! Transport selection is based on:
//! 1. User preference (configured privacy level)
//! 2. Peer capabilities (what transports both sides support)
//! 3. Network conditions (NAT type, firewall rules)
//!
//! See the `TransportManager` for automatic selection.
//!
//! # Security Considerations
//!
//! - All transports provide encryption (Noise, DTLS, or TLS)
//! - Obfuscated transports (WebRTC, TLS) resist DPI detection
//! - Transport negotiation is authenticated to prevent downgrade attacks
//! - Session IDs are random to prevent prediction
//! - Timeout cleanup prevents resource exhaustion
//!
//! # References
//!
//! - Design document: `docs/design/traffic-privacy-roadmap.md` (Phase 3)
//! - Parent issue: #201 (Phase 3: Protocol Obfuscation)
//! - Implementation issue: #202 (Pluggable transport interface)
//! - Negotiation issue: #207 (Transport negotiation protocol)
//! - Signaling issue: #206 (Signaling channel for SDP exchange)

// Transport capabilities and negotiation (Phase 3.6)
mod capabilities;
mod negotiation;

// Transport configuration and selection (Phase 3.8)
pub mod config;
pub mod manager;
pub mod metrics;

// Transport implementations
mod error;
pub mod http2;
mod plain;
pub mod signaling;
pub mod tls_tunnel;
mod traits;
mod types;
pub mod webrtc;

// Re-export capabilities types (Phase 3.6)
pub use capabilities::{
    NatType as NegotiationNatType, TransportCapabilities, TransportType as CapabilityTransportType,
};

// Re-export negotiation types (Phase 3.6)
pub use negotiation::{
    negotiate_transport_initiator, negotiate_transport_responder, read_message, select_transport,
    write_message, NegotiationConfig, NegotiationError, NegotiationMessage, UpgradeResult,
};

// Re-export error types
pub use error::TransportError;

// Re-export transport types
pub use types::{TransportType, TransportTypeParseError};

// Re-export trait and connection types
pub use traits::{BoxedConnection, ConnectionWrapper, PluggableTransport, TransportConnection};

// Re-export transport implementations
pub use plain::{PlainConnection, PlainTransport};

// Re-export TLS tunnel transport (Phase 3.7)
pub use tls_tunnel::{
    TlsClientConnection, TlsConfig, TlsConfigError, TlsServerConnection, TlsTunnelConnection,
    TlsTunnelTransport,
};

// Re-export HTTP/2 framing (Phase 3.7)
pub use http2::{
    Http2FrameError, Http2Wrapper, Http2WrapperConfig, FRAME_HEADER_SIZE, FRAME_TYPE_DATA,
    MAX_FRAME_SIZE,
};

// Re-export WebRTC DTLS types (Phase 3.3)
pub use webrtc::dtls::{
    CertificateFingerprint, DtlsConfig, DtlsError, DtlsRole, DtlsState, DtlsVerification,
    EphemeralCertificate,
};

// Re-export WebRTC ICE/STUN types (Phase 3.4)
pub use webrtc::{
    ice::{
        IceCandidate as IceFullCandidate, IceCandidateType, IceConfig, IceConnectionState,
        IceError, IceGatherer,
    },
    stun::{NatType, StunClient, StunConfig, StunError},
    WebRtcConnection, WebRtcTransport,
};

// Re-export signaling types (Phase 3.5)
pub use signaling::{
    IceCandidate, SessionId, SignalingChannel, SignalingError, SignalingMessage, SignalingRole,
    SignalingSession, SignalingState, DEFAULT_SIGNALING_TIMEOUT_SECS,
    MAX_ICE_CANDIDATES_PER_SESSION, MAX_ICE_CANDIDATE_SIZE, MAX_SDP_SIZE, MAX_SESSIONS_PER_PEER,
    SESSION_ID_LEN,
};

// Re-export transport configuration types (Phase 3.8)
pub use config::{
    TlsTransportConfig, TransportConfig, TransportConfigBuilder, TransportPreference,
    WebRtcTransportConfig,
};

// Re-export transport metrics types (Phase 3.8)
pub use metrics::{
    ConnectResult, MetricsSnapshot, TransportMetrics, TransportMetricsSummary, TransportStats,
};

// Re-export transport selector types (Phase 3.8)
pub use manager::{ConnectionResult, PeerInfo, TransportSelector};

use tokio::io::{AsyncRead, AsyncWrite};

/// Trait for async read/write streams.
///
/// This is a convenience alias for streams that support both
/// async reading and writing with proper bounds for transport usage.
pub trait AsyncReadWrite: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> AsyncReadWrite for T {}

/// Transport manager for handling transport selection and upgrades.
///
/// This struct manages the available transports and provides methods
/// for selecting and upgrading connections.
#[derive(Debug, Clone)]
pub struct TransportManager {
    /// Our transport capabilities
    capabilities: TransportCapabilities,
    /// Configuration
    config: TransportManagerConfig,
}

/// Configuration for the transport manager.
#[derive(Debug, Clone)]
pub struct TransportManagerConfig {
    /// Whether to attempt transport upgrades
    pub enable_upgrades: bool,
    /// Negotiation configuration
    pub negotiation: NegotiationConfig,
    /// Preferred transport (used when multiple are available)
    pub preferred: CapabilityTransportType,
}

impl Default for TransportManagerConfig {
    fn default() -> Self {
        Self {
            enable_upgrades: true,
            negotiation: NegotiationConfig::default(),
            preferred: CapabilityTransportType::Plain,
        }
    }
}

impl TransportManager {
    /// Create a new transport manager with the given capabilities.
    pub fn new(capabilities: TransportCapabilities) -> Self {
        Self {
            capabilities,
            config: TransportManagerConfig::default(),
        }
    }

    /// Create a new transport manager with custom configuration.
    pub fn with_config(
        capabilities: TransportCapabilities,
        config: TransportManagerConfig,
    ) -> Self {
        Self {
            capabilities,
            config,
        }
    }

    /// Get our transport capabilities.
    pub fn capabilities(&self) -> &TransportCapabilities {
        &self.capabilities
    }

    /// Get the agent version suffix for advertising capabilities.
    ///
    /// This should be appended to the peer's agent version string.
    pub fn capabilities_suffix(&self) -> String {
        self.capabilities.to_multiaddr_suffix()
    }

    /// Select the best transport for connecting to a peer.
    pub fn select_for_peer(&self, peer_caps: &TransportCapabilities) -> CapabilityTransportType {
        select_transport(&self.capabilities, peer_caps)
    }

    /// Check if we should attempt to upgrade a connection.
    ///
    /// Returns true if:
    /// 1. Upgrades are enabled
    /// 2. Current transport is not the best available
    /// 3. Peer supports better transports
    pub fn should_upgrade(
        &self,
        current: CapabilityTransportType,
        peer_caps: &TransportCapabilities,
    ) -> bool {
        if !self.config.enable_upgrades {
            return false;
        }

        let best = self.select_for_peer(peer_caps);
        best != current && best.preference_score() > current.preference_score()
    }

    /// Attempt to upgrade an existing connection to a better transport.
    ///
    /// This function:
    /// 1. Negotiates the best transport with the peer
    /// 2. If successful, returns the new transport type
    /// 3. On failure, returns the original connection unchanged
    ///
    /// The actual transport upgrade (creating new encrypted channel) is
    /// handled by the caller based on the negotiated transport type.
    pub async fn negotiate_upgrade<S>(
        &self,
        stream: &mut S,
        is_initiator: bool,
    ) -> Result<CapabilityTransportType, NegotiationError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        if is_initiator {
            negotiate_transport_initiator(stream, &self.capabilities).await
        } else {
            negotiate_transport_responder(stream, &self.capabilities).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_exports() {
        // Verify all expected types are exported and usable
        let _: TransportType = TransportType::Plain;
        let _: PlainTransport = PlainTransport::new();

        // Verify trait is usable
        fn assert_transport<T: PluggableTransport>(_: &T) {}
        let transport = PlainTransport::new();
        assert_transport(&transport);
    }

    #[test]
    fn test_plain_transport_is_default() {
        let transport = PlainTransport::default();
        assert_eq!(transport.transport_type(), TransportType::Plain);
    }

    #[test]
    fn test_dtls_types_exported() {
        // Verify DTLS types are accessible from transport module
        let config = DtlsConfig::generate_ephemeral().unwrap();
        assert_eq!(config.role(), DtlsRole::Auto);
    }

    #[test]
    fn test_ice_stun_types_exported() {
        // Verify ICE/STUN types are accessible from transport module
        let ice_config = IceConfig::default();
        assert!(!ice_config.stun_servers.is_empty());

        let stun_config = StunConfig::default();
        assert!(!stun_config.servers.is_empty());
    }

    #[test]
    fn test_webrtc_transport_creation() {
        let transport = WebRtcTransport::with_defaults();
        assert!(!transport.ice_config().stun_servers.is_empty());
    }

    #[test]
    fn test_transport_manager_new() {
        let caps = TransportCapabilities::full(NegotiationNatType::Open);
        let manager = TransportManager::new(caps.clone());

        assert_eq!(manager.capabilities(), &caps);
    }

    #[test]
    fn test_transport_manager_capabilities_suffix() {
        let caps = TransportCapabilities::full(NegotiationNatType::Open);
        let manager = TransportManager::new(caps);

        let suffix = manager.capabilities_suffix();
        assert!(suffix.starts_with("/transport-caps/"));
    }

    #[test]
    fn test_transport_manager_select_for_peer() {
        let our_caps = TransportCapabilities::full(NegotiationNatType::Open);
        let peer_caps = TransportCapabilities::full(NegotiationNatType::FullCone);
        let manager = TransportManager::new(our_caps);

        let selected = manager.select_for_peer(&peer_caps);
        assert_eq!(selected, CapabilityTransportType::WebRTC);
    }

    #[test]
    fn test_transport_manager_should_upgrade() {
        let our_caps = TransportCapabilities::full(NegotiationNatType::Open);
        let peer_caps = TransportCapabilities::full(NegotiationNatType::Open);
        let manager = TransportManager::new(our_caps);

        // Currently on plain, should upgrade to WebRTC
        assert!(manager.should_upgrade(CapabilityTransportType::Plain, &peer_caps));

        // Already on WebRTC, should not upgrade
        assert!(!manager.should_upgrade(CapabilityTransportType::WebRTC, &peer_caps));
    }

    #[test]
    fn test_transport_manager_should_upgrade_disabled() {
        let our_caps = TransportCapabilities::full(NegotiationNatType::Open);
        let peer_caps = TransportCapabilities::full(NegotiationNatType::Open);

        let config = TransportManagerConfig {
            enable_upgrades: false,
            ..Default::default()
        };
        let manager = TransportManager::with_config(our_caps, config);

        // Upgrades disabled, should not upgrade even if better available
        assert!(!manager.should_upgrade(CapabilityTransportType::Plain, &peer_caps));
    }

    #[test]
    fn test_transport_manager_config_default() {
        let config = TransportManagerConfig::default();
        assert!(config.enable_upgrades);
        assert_eq!(config.preferred, CapabilityTransportType::Plain);
    }

    #[test]
    fn test_tls_tunnel_types_exported() {
        // Install ring crypto provider for TLS tests
        let _ = rustls::crypto::ring::default_provider().install_default();

        // Verify TLS tunnel types are accessible from transport module
        let config = TlsConfig::generate_self_signed().unwrap();
        let transport = TlsTunnelTransport::new(config).unwrap();
        assert_eq!(transport.transport_type(), TransportType::TlsTunnel);
    }

    #[test]
    fn test_http2_types_exported() {
        // Verify HTTP/2 types are accessible from transport module
        let mut wrapper = Http2Wrapper::default();
        let data = b"test";
        let frame = wrapper.wrap(data);
        assert!(frame.len() >= FRAME_HEADER_SIZE);
    }
}
