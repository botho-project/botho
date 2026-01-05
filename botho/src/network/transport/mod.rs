// Copyright (c) 2024 Botho Foundation

//! Transport layer for P2P connections.
//!
//! This module provides:
//! - Pluggable transport interface for different connection types
//! - Transport capabilities advertising and parsing
//! - Transport negotiation protocol between peers
//! - Connection upgrade mechanism
//!
//! # Overview
//!
//! The transport layer supports multiple connection types:
//! - **Plain**: Standard TCP + Noise (default)
//! - **TLS Tunnel**: TLS 1.3 wrapped connections (looks like HTTPS)
//! - **WebRTC**: Data channels (looks like video calls, good NAT traversal)
//!
//! Peers advertise their supported transports in discovery, and negotiate
//! the best common transport when establishing connections.
//!
//! # Example
//!
//! ```ignore
//! use botho::network::transport::{
//!     TransportCapabilities, TransportType, NatType,
//!     select_transport, negotiate_transport_initiator,
//! };
//!
//! // Create capabilities
//! let our_caps = TransportCapabilities::full(NatType::Open);
//!
//! // Select best transport for a peer
//! let peer_caps = TransportCapabilities::from_agent_version(peer_agent_version)?;
//! let transport = select_transport(&our_caps, &peer_caps);
//!
//! // Negotiate with peer
//! let agreed = negotiate_transport_initiator(&mut stream, &our_caps).await?;
//! ```

mod capabilities;
mod negotiation;

pub use capabilities::{NatType, TransportCapabilities, TransportType};
pub use negotiation::{
    negotiate_transport_initiator, negotiate_transport_responder, read_message, select_transport,
    write_message, NegotiationConfig, NegotiationError, NegotiationMessage, UpgradeResult,
};

use std::io;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};

/// Errors that can occur during transport operations.
#[derive(Debug, Error)]
pub enum TransportError {
    /// Negotiation failed.
    #[error("negotiation failed: {0}")]
    Negotiation(#[from] NegotiationError),

    /// Connection error.
    #[error("connection error: {0}")]
    Connection(#[from] io::Error),

    /// Transport not supported.
    #[error("transport not supported: {0}")]
    NotSupported(TransportType),

    /// Upgrade failed.
    #[error("upgrade failed: {0}")]
    UpgradeFailed(String),
}

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
    pub preferred: TransportType,
}

impl Default for TransportManagerConfig {
    fn default() -> Self {
        Self {
            enable_upgrades: true,
            negotiation: NegotiationConfig::default(),
            preferred: TransportType::Plain,
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
    pub fn with_config(capabilities: TransportCapabilities, config: TransportManagerConfig) -> Self {
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
    pub fn select_for_peer(&self, peer_caps: &TransportCapabilities) -> TransportType {
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
        current: TransportType,
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
    ) -> Result<TransportType, NegotiationError>
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
    fn test_transport_manager_new() {
        let caps = TransportCapabilities::full(NatType::Open);
        let manager = TransportManager::new(caps.clone());

        assert_eq!(manager.capabilities(), &caps);
    }

    #[test]
    fn test_transport_manager_capabilities_suffix() {
        let caps = TransportCapabilities::full(NatType::Open);
        let manager = TransportManager::new(caps);

        let suffix = manager.capabilities_suffix();
        assert!(suffix.starts_with("/transport-caps/"));
    }

    #[test]
    fn test_transport_manager_select_for_peer() {
        let our_caps = TransportCapabilities::full(NatType::Open);
        let peer_caps = TransportCapabilities::full(NatType::FullCone);
        let manager = TransportManager::new(our_caps);

        let selected = manager.select_for_peer(&peer_caps);
        assert_eq!(selected, TransportType::WebRTC);
    }

    #[test]
    fn test_transport_manager_should_upgrade() {
        let our_caps = TransportCapabilities::full(NatType::Open);
        let peer_caps = TransportCapabilities::full(NatType::Open);
        let manager = TransportManager::new(our_caps);

        // Currently on plain, should upgrade to WebRTC
        assert!(manager.should_upgrade(TransportType::Plain, &peer_caps));

        // Already on WebRTC, should not upgrade
        assert!(!manager.should_upgrade(TransportType::WebRTC, &peer_caps));
    }

    #[test]
    fn test_transport_manager_should_upgrade_disabled() {
        let our_caps = TransportCapabilities::full(NatType::Open);
        let peer_caps = TransportCapabilities::full(NatType::Open);

        let config = TransportManagerConfig {
            enable_upgrades: false,
            ..Default::default()
        };
        let manager = TransportManager::with_config(our_caps, config);

        // Upgrades disabled, should not upgrade even if better available
        assert!(!manager.should_upgrade(TransportType::Plain, &peer_caps));
    }

    #[test]
    fn test_transport_error_display() {
        let err = TransportError::NotSupported(TransportType::WebRTC);
        assert!(format!("{}", err).contains("webrtc"));

        let err = TransportError::UpgradeFailed("test".to_string());
        assert!(format!("{}", err).contains("test"));
    }

    #[test]
    fn test_transport_manager_config_default() {
        let config = TransportManagerConfig::default();
        assert!(config.enable_upgrades);
        assert_eq!(config.preferred, TransportType::Plain);
    }
}
