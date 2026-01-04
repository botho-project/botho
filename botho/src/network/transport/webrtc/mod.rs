// Copyright (c) 2024 Botho Foundation

//! WebRTC transport for protocol obfuscation.
//!
//! This module implements Phase 3 of the traffic privacy roadmap: Protocol Obfuscation
//! using WebRTC data channels to make botho traffic indistinguishable from legitimate
//! video calling applications.
//!
//! # Overview
//!
//! WebRTC is ideal for protocol obfuscation because:
//! - Widely used by video calling apps (Google Meet, Discord, etc.)
//! - Mandates DTLS encryption for all data channels
//! - Designed for P2P with built-in NAT traversal (ICE/STUN/TURN)
//! - Traffic patterns naturally match our needs
//! - Blocking WebRTC would break legitimate video calling
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────┐
//! │   Application   │  (Gossipsub)
//! └────────┬────────┘
//!          │
//! ┌────────▼────────┐
//! │     Yamux       │  (Stream multiplexing)
//! └────────┬────────┘
//!          │
//! ┌────────▼────────┐
//! │  SCTP/DataChan  │  ◄── WebRTC data channel
//! └────────┬────────┘
//!          │
//! ┌────────▼────────┐
//! │   DTLS 1.3      │  ◄── dtls module
//! └────────┬────────┘
//!          │
//! ┌────────▼────────┐
//! │    ICE/UDP      │  ◄── ice/stun modules
//! └─────────────────┘
//! ```
//!
//! # Modules
//!
//! - [`dtls`]: DTLS configuration and certificate handling
//! - [`ice`]: ICE (Interactive Connectivity Establishment) for NAT traversal
//! - [`stun`]: STUN client for reflexive address discovery
//!
//! # Features
//!
//! - **NAT Traversal**: ICE with STUN/TURN support for connectivity through NATs
//! - **Protocol Obfuscation**: Traffic looks like WebRTC video calls
//! - **Trickle ICE**: Candidates sent as gathered for faster connection establishment
//! - **DTLS Security**: Ephemeral certificates for authenticated encryption
//!
//! # References
//!
//! - Design: `docs/design/traffic-privacy-roadmap.md` (Phase 3)
//! - WebRTC: <https://webrtc.org/>

pub mod dtls;
pub mod ice;
pub mod stun;

// Re-export DTLS types
pub use dtls::{
    CertificateFingerprint, DtlsConfig, DtlsError, DtlsRole, DtlsState, DtlsVerification,
    EphemeralCertificate, BROWSER_CIPHER_SUITES, DEFAULT_CERTIFICATE_LIFETIME,
    DEFAULT_FINGERPRINT_ALGORITHM,
};

// Re-export ICE/STUN types
pub use ice::{
    IceCandidate, IceCandidateType, IceConfig, IceConnectionState, IceError, IceGatherer,
};
pub use stun::{NatType, StunClient, StunConfig, StunError};

use std::sync::Arc;
use tokio::sync::Mutex;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::APIBuilder;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_connection_state::RTCIceConnectionState;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;

use super::TransportError;

/// WebRTC transport for protocol-obfuscated connections.
///
/// This transport uses WebRTC data channels to make P2P traffic
/// indistinguishable from video calling applications.
pub struct WebRtcTransport {
    /// ICE configuration
    ice_config: IceConfig,
    /// ICE gatherer for candidate collection
    gatherer: IceGatherer,
    /// STUN client for NAT detection
    stun_client: StunClient,
}

impl WebRtcTransport {
    /// Create a new WebRTC transport with the given configuration.
    pub fn new(ice_config: IceConfig, stun_config: StunConfig) -> Self {
        let gatherer = IceGatherer::new(ice_config.clone());
        let stun_client = StunClient::new(stun_config);

        Self {
            ice_config,
            gatherer,
            stun_client,
        }
    }

    /// Create a new WebRTC transport with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(IceConfig::default(), StunConfig::default())
    }

    /// Detect the NAT type for this node.
    ///
    /// This is useful for reporting relay capacity - nodes behind
    /// symmetric NATs have limited relay capability.
    pub async fn detect_nat_type(&self) -> Result<NatType, TransportError> {
        self.stun_client
            .detect_nat_type()
            .await
            .map_err(TransportError::Stun)
    }

    /// Create a new peer connection with ICE configuration.
    pub async fn create_peer_connection(&self) -> Result<Arc<RTCPeerConnection>, TransportError> {
        // Create a MediaEngine (required even for data-only connections)
        let mut media_engine = MediaEngine::default();

        // Create interceptor registry
        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut media_engine)
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;

        // Build the API
        let api = APIBuilder::new()
            .with_media_engine(media_engine)
            .with_interceptor_registry(registry)
            .build();

        // Convert our ICE config to WebRTC config
        let ice_servers = self
            .ice_config
            .stun_servers
            .iter()
            .map(|url| RTCIceServer {
                urls: vec![url.clone()],
                ..Default::default()
            })
            .chain(self.ice_config.turn_servers.iter().map(|turn| {
                RTCIceServer {
                    urls: vec![turn.url.clone()],
                    username: turn.username.clone(),
                    credential: turn.credential.clone(),
                    ..Default::default()
                }
            }))
            .collect();

        let config = RTCConfiguration {
            ice_servers,
            ..Default::default()
        };

        // Create peer connection
        let peer_connection = api
            .new_peer_connection(config)
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;

        Ok(Arc::new(peer_connection))
    }

    /// Create a data channel for botho traffic.
    pub async fn create_data_channel(
        peer_connection: &RTCPeerConnection,
        label: &str,
    ) -> Result<Arc<RTCDataChannel>, TransportError> {
        let data_channel = peer_connection
            .create_data_channel(label, None)
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;

        Ok(data_channel)
    }

    /// Create an SDP offer for initiating a connection.
    pub async fn create_offer(
        peer_connection: &RTCPeerConnection,
    ) -> Result<RTCSessionDescription, TransportError> {
        let offer = peer_connection
            .create_offer(None)
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;

        peer_connection
            .set_local_description(offer.clone())
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;

        Ok(offer)
    }

    /// Create an SDP answer in response to an offer.
    pub async fn create_answer(
        peer_connection: &RTCPeerConnection,
        offer: RTCSessionDescription,
    ) -> Result<RTCSessionDescription, TransportError> {
        peer_connection
            .set_remote_description(offer)
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;

        let answer = peer_connection
            .create_answer(None)
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;

        peer_connection
            .set_local_description(answer.clone())
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;

        Ok(answer)
    }

    /// Set the remote SDP answer.
    pub async fn set_remote_answer(
        peer_connection: &RTCPeerConnection,
        answer: RTCSessionDescription,
    ) -> Result<(), TransportError> {
        peer_connection
            .set_remote_description(answer)
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;

        Ok(())
    }

    /// Wait for ICE gathering to complete.
    pub async fn wait_for_ice_gathering(
        &self,
        peer_connection: &RTCPeerConnection,
    ) -> Result<Vec<IceCandidate>, TransportError> {
        self.gatherer
            .gather_candidates(peer_connection)
            .await
            .map_err(TransportError::Ice)
    }

    /// Get the ICE gatherer for trickle ICE support.
    pub fn gatherer(&self) -> &IceGatherer {
        &self.gatherer
    }

    /// Get the current ICE configuration.
    pub fn ice_config(&self) -> &IceConfig {
        &self.ice_config
    }
}

/// WebRTC connection wrapper providing async read/write.
pub struct WebRtcConnection {
    /// The underlying peer connection
    peer_connection: Arc<RTCPeerConnection>,
    /// The data channel for botho traffic
    data_channel: Arc<RTCDataChannel>,
    /// Receive buffer for incoming messages
    recv_buffer: Arc<Mutex<Vec<u8>>>,
}

impl WebRtcConnection {
    /// Create a new WebRTC connection.
    pub fn new(
        peer_connection: Arc<RTCPeerConnection>,
        data_channel: Arc<RTCDataChannel>,
    ) -> Self {
        let recv_buffer = Arc::new(Mutex::new(Vec::new()));
        let buffer_clone = recv_buffer.clone();

        // Set up message handler
        data_channel.on_message(Box::new(move |msg: DataChannelMessage| {
            let buffer = buffer_clone.clone();
            Box::pin(async move {
                let mut buf = buffer.lock().await;
                buf.extend_from_slice(&msg.data);
            })
        }));

        Self {
            peer_connection,
            data_channel,
            recv_buffer,
        }
    }

    /// Send data over the WebRTC data channel.
    pub async fn send(&self, data: &[u8]) -> Result<(), TransportError> {
        self.data_channel
            .send(&bytes::Bytes::copy_from_slice(data))
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;
        Ok(())
    }

    /// Receive data from the WebRTC data channel.
    pub async fn recv(&self, buf: &mut [u8]) -> Result<usize, TransportError> {
        let mut recv_buf = self.recv_buffer.lock().await;
        let len = std::cmp::min(buf.len(), recv_buf.len());
        buf[..len].copy_from_slice(&recv_buf[..len]);
        recv_buf.drain(..len);
        Ok(len)
    }

    /// Check if the connection is still active.
    pub fn is_connected(&self) -> bool {
        matches!(
            self.peer_connection.connection_state(),
            RTCPeerConnectionState::Connected
        )
    }

    /// Get the current ICE connection state.
    pub fn ice_state(&self) -> RTCIceConnectionState {
        self.peer_connection.ice_connection_state()
    }

    /// Close the connection.
    pub async fn close(&self) -> Result<(), TransportError> {
        self.data_channel
            .close()
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;
        self.peer_connection
            .close()
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_webrtc_transport_creation() {
        let transport = WebRtcTransport::with_defaults();
        assert!(!transport.ice_config.stun_servers.is_empty());
    }

    #[test]
    fn test_custom_ice_config() {
        let ice_config = IceConfig {
            stun_servers: vec!["stun:custom.example.com:3478".to_string()],
            ..Default::default()
        };
        let transport = WebRtcTransport::new(ice_config.clone(), StunConfig::default());
        assert_eq!(transport.ice_config.stun_servers, ice_config.stun_servers);
    }
}
