// Copyright (c) 2024 Botho Foundation

//! WebRTC transport for protocol obfuscation.
//!
//! This module implements Phase 3 of the traffic privacy roadmap: Protocol
//! Obfuscation using WebRTC data channels to make botho traffic
//! indistinguishable from legitimate video calling applications.
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
//! - **NAT Traversal**: ICE with STUN/TURN support for connectivity through
//!   NATs
//! - **Protocol Obfuscation**: Traffic looks like WebRTC video calls
//! - **Trickle ICE**: Candidates sent as gathered for faster connection
//!   establishment
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

use std::{
    collections::VecDeque,
    net::SocketAddr,
    sync::{Arc, Mutex as StdMutex},
};
use tokio::sync::{mpsc, watch, Mutex};
use webrtc::{
    data_channel::{DataChannel, DataChannelEvent},
    peer_connection::{
        PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCConfigurationBuilder,
        RTCIceConnectionState, RTCIceGatheringState, RTCIceServer, RTCPeerConnectionIceEvent,
        RTCPeerConnectionState, RTCSessionDescription,
    },
};

/// Per-peer state is installed before the driver starts, so early ICE events
/// are retained.
pub struct WebRtcPeer {
    inner: Arc<dyn PeerConnection>,
    events: Arc<PeerEvents>,
    incoming: Mutex<mpsc::Receiver<Arc<dyn DataChannel>>>,
}

impl std::ops::Deref for WebRtcPeer {
    type Target = dyn PeerConnection;
    fn deref(&self) -> &Self::Target {
        self.inner.as_ref()
    }
}

impl WebRtcPeer {
    /// Receive a remotely opened channel. Cancellation does not consume a
    /// channel.
    pub async fn accept_data_channel(&self) -> Option<Arc<dyn DataChannel>> {
        let mut state = self.events.connection.subscribe();
        loop {
            if *state.borrow_and_update() == RTCPeerConnectionState::Closed {
                return None;
            }
            tokio::select! {
                channel = async { self.incoming.lock().await.recv().await } => return channel,
                _ = state.changed() => {}
            }
        }
    }

    /// Close the driver and wake pending channel acceptors.
    pub async fn close(&self) -> webrtc::error::Result<()> {
        self.events
            .connection
            .send_replace(RTCPeerConnectionState::Closed);
        self.events
            .ice_connection
            .send_replace(RTCIceConnectionState::Closed);
        self.inner.close().await
    }
}

struct PeerEvents {
    ice: ice::CandidateEvents,
    gathering: watch::Sender<RTCIceGatheringState>,
    connection: watch::Sender<RTCPeerConnectionState>,
    ice_connection: watch::Sender<RTCIceConnectionState>,
    incoming: mpsc::Sender<Arc<dyn DataChannel>>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for PeerEvents {
    async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
        self.ice.record(&event.candidate);
    }
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        self.gathering.send_replace(state);
    }
    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        self.connection.send_replace(state);
    }
    async fn on_ice_connection_state_change(&self, state: RTCIceConnectionState) {
        self.ice_connection.send_replace(state);
    }
    async fn on_data_channel(&self, channel: Arc<dyn DataChannel>) {
        if let Err(error) = self.incoming.try_send(channel) {
            let channel = error.into_inner();
            // DataChannel::close only updates the core and wakes writes; unlike
            // PeerConnection::close, it does not wait on this event-handler task.
            let _ = channel.close().await;
        }
    }
}

use super::{TransportError, WebRtcError};

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
    pub async fn create_peer_connection(&self) -> Result<Arc<WebRtcPeer>, TransportError> {
        // 0.20.2 publishes bound addresses verbatim; wildcard binds would advertise
        // 0.0.0.0.
        let addresses = if_addrs::get_if_addrs()
            .map_err(|e| WebRtcError::peer_connection_create(e.to_string()))?
            .into_iter()
            .filter(|interface| interface.is_oper_up() && !interface.is_loopback())
            // Scoped IPv6 link-local candidates are not representable by this ICE wrapper.
            .filter(|interface| !matches!(interface.ip(), std::net::IpAddr::V6(ip) if ip.is_unicast_link_local()))
            .map(|interface| SocketAddr::new(interface.ip(), 0))
            .filter(|address| !address.ip().is_unspecified())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        self.create_peer_connection_bound(addresses).await
    }

    /// Bind a concrete local address (also permits isolated loopback tests).
    pub async fn create_peer_connection_on(
        &self,
        address: SocketAddr,
    ) -> Result<Arc<WebRtcPeer>, TransportError> {
        self.create_peer_connection_bound(vec![address]).await
    }

    async fn create_peer_connection_bound(
        &self,
        addresses: Vec<SocketAddr>,
    ) -> Result<Arc<WebRtcPeer>, TransportError> {
        if addresses.is_empty()
            || addresses
                .iter()
                .any(|address| address.ip().is_unspecified())
        {
            return Err(WebRtcError::peer_connection_create(
                "concrete local interface address required",
            )
            .into());
        }
        // Convert our ICE config to WebRTC config
        let ice_servers = self
            .ice_config
            .stun_servers
            .iter()
            .map(|url| RTCIceServer {
                urls: vec![url.clone()],
                ..Default::default()
            })
            .chain(
                self.ice_config
                    .turn_servers
                    .iter()
                    .map(|turn| RTCIceServer {
                        urls: vec![turn.url.clone()],
                        username: turn.username.clone(),
                        credential: turn.credential.clone(),
                    }),
            )
            .collect();

        let config = RTCConfigurationBuilder::default()
            .with_ice_servers(ice_servers)
            .build();
        let (incoming_tx, incoming_rx) = mpsc::channel(8);
        let events = Arc::new(PeerEvents {
            ice: ice::CandidateEvents::default(),
            gathering: watch::channel(RTCIceGatheringState::New).0,
            connection: watch::channel(RTCPeerConnectionState::New).0,
            ice_connection: watch::channel(RTCIceConnectionState::New).0,
            incoming: incoming_tx,
        });
        let peer = PeerConnectionBuilder::new()
            .with_configuration(config)
            .with_handler(events.clone())
            .with_udp_addrs(addresses)
            // Upstream only guarantees driver shutdown on Drop for its bounded reactor pool.
            .with_dedicated_reactor_thread(true)
            .with_data_channel_send_buffer_limit(MAX_RECEIVE_BYTES)
            .build()
            .await
            .map_err(|e| WebRtcError::peer_connection_create(e.to_string()))?;
        Ok(Arc::new(WebRtcPeer {
            inner: Arc::new(peer),
            events,
            incoming: Mutex::new(incoming_rx),
        }))
    }

    /// Create a data channel for botho traffic.
    pub async fn create_data_channel(
        peer_connection: &WebRtcPeer,
        label: &str,
    ) -> Result<Arc<dyn DataChannel>, TransportError> {
        let data_channel = peer_connection
            .create_data_channel(label, None)
            .await
            .map_err(|e| WebRtcError::data_channel_create(e.to_string()))?;

        Ok(data_channel)
    }

    /// Create an SDP offer for initiating a connection.
    pub async fn create_offer(
        peer_connection: &WebRtcPeer,
    ) -> Result<RTCSessionDescription, TransportError> {
        let offer = peer_connection
            .create_offer(None)
            .await
            .map_err(|e| WebRtcError::create_offer(e.to_string()))?;

        peer_connection
            .set_local_description(offer.clone())
            .await
            .map_err(|e| WebRtcError::set_local_description(e.to_string()))?;

        Ok(offer)
    }

    /// Create an SDP answer in response to an offer.
    pub async fn create_answer(
        peer_connection: &WebRtcPeer,
        offer: RTCSessionDescription,
    ) -> Result<RTCSessionDescription, TransportError> {
        peer_connection
            .set_remote_description(offer)
            .await
            .map_err(|e| WebRtcError::set_remote_description(e.to_string()))?;

        let answer = peer_connection
            .create_answer(None)
            .await
            .map_err(|e| WebRtcError::create_answer(e.to_string()))?;

        peer_connection
            .set_local_description(answer.clone())
            .await
            .map_err(|e| WebRtcError::set_local_description(e.to_string()))?;

        Ok(answer)
    }

    /// Set the remote SDP answer.
    pub async fn set_remote_answer(
        peer_connection: &WebRtcPeer,
        answer: RTCSessionDescription,
    ) -> Result<(), TransportError> {
        peer_connection
            .set_remote_description(answer)
            .await
            .map_err(|e| WebRtcError::set_remote_description(e.to_string()))?;

        Ok(())
    }

    /// Wait for ICE gathering to complete.
    pub async fn wait_for_ice_gathering(
        &self,
        peer_connection: &WebRtcPeer,
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

/// Maximum unread application bytes retained per connection. Overflow closes
/// the channel.
pub const MAX_RECEIVE_BYTES: usize = 1024 * 1024;

#[derive(Default)]
struct ReceiveBuffer {
    bytes: VecDeque<u8>,
    failed: bool,
    closed: bool,
}

/// WebRTC connection wrapper providing async read/write.
/// `recv` retains its nonblocking polling contract: zero can mean temporarily
/// empty.
pub struct WebRtcConnection {
    peer_connection: Arc<WebRtcPeer>,
    data_channel: Arc<dyn DataChannel>,
    recv_buffer: Arc<Mutex<ReceiveBuffer>>,
    receiver: StdMutex<Option<tokio::task::JoinHandle<()>>>,
    // Retain cancellation even if a close future took the join handle then was dropped.
    receiver_abort: tokio::task::AbortHandle,
}

impl WebRtcConnection {
    /// Own the channel's single event consumer until close or drop.
    pub fn new(peer_connection: Arc<WebRtcPeer>, data_channel: Arc<dyn DataChannel>) -> Self {
        let recv_buffer = Arc::new(Mutex::new(ReceiveBuffer::default()));
        let buffer = recv_buffer.clone();
        let channel = data_channel.clone();
        let receiver = tokio::spawn(async move {
            while let Some(event) = channel.poll().await {
                match event {
                    DataChannelEvent::OnMessage(message) => {
                        let mut buffer = buffer.lock().await;
                        if message.data.len() > MAX_RECEIVE_BYTES - buffer.bytes.len() {
                            buffer.failed = true;
                            buffer.closed = true;
                            drop(buffer);
                            let _ = channel.close().await;
                            return;
                        }
                        buffer.bytes.extend(message.data);
                    }
                    DataChannelEvent::OnError => {
                        let mut buffer = buffer.lock().await;
                        buffer.failed = true;
                        buffer.closed = true;
                        break;
                    }
                    DataChannelEvent::OnClose => break,
                    DataChannelEvent::OnClosing => {
                        buffer.lock().await.closed = true;
                    }
                    _ => {}
                }
            }
            buffer.lock().await.closed = true;
        });
        Self {
            peer_connection,
            data_channel,
            recv_buffer,
            receiver_abort: receiver.abort_handle(),
            receiver: StdMutex::new(Some(receiver)),
        }
    }

    /// Send binary data; upstream applies bounded send backpressure.
    pub async fn send(&self, data: &[u8]) -> Result<(), TransportError> {
        if self.recv_buffer.lock().await.closed {
            return Err(TransportError::ConnectionClosed);
        }
        self.data_channel
            .send(bytes::BytesMut::from(data))
            .await
            .map_err(|e| WebRtcError::send_failed(e.to_string()))?;
        Ok(())
    }

    /// Drain available bytes, preserving the previous polling and partial-read
    /// behavior.
    pub async fn recv(&self, buf: &mut [u8]) -> Result<usize, TransportError> {
        let mut received = self.recv_buffer.lock().await;
        if received.failed {
            return Err(TransportError::DataChannel(
                "receive channel failed or exceeded capacity".into(),
            ));
        }
        let len = buf.len().min(received.bytes.len());
        for byte in &mut buf[..len] {
            *byte = received.bytes.pop_front().expect("length checked");
        }
        Ok(len)
    }

    pub fn is_connected(&self) -> bool {
        *self.peer_connection.events.connection.borrow() == RTCPeerConnectionState::Connected
            && !self.receiver_abort.is_finished()
    }

    pub fn ice_state(&self) -> RTCIceConnectionState {
        *self.peer_connection.events.ice_connection.borrow()
    }

    /// Stop the receiver and always attempt peer cleanup, including after
    /// channel errors.
    pub async fn close(&self) -> Result<(), TransportError> {
        self.recv_buffer.lock().await.closed = true;
        let receiver = self.receiver.lock().expect("receiver lock").take();
        let channel_result = self.data_channel.close().await;
        if let Some(mut receiver) = receiver {
            // ready_state becomes Closed before SCTP's reset is flushed in 0.20.2.
            // Wait for the actual close event, keeping the driver alive, but bound
            // teardown when the remote disappears or the channel never opened.
            if channel_result.is_err()
                || tokio::time::timeout(std::time::Duration::from_secs(1), &mut receiver)
                    .await
                    .is_err()
            {
                receiver.abort();
                let _ = receiver.await;
            }
        }
        let peer_result = self.peer_connection.close().await;
        peer_result.map_err(|_| TransportError::ConnectionClosed)?;
        channel_result.map_err(|e| TransportError::DataChannel(e.to_string()))
    }
}

impl Drop for WebRtcConnection {
    fn drop(&mut self) {
        self.receiver_abort.abort();
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

#[cfg(test)]
mod lifecycle_tests;
