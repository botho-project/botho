// Copyright (c) 2024 Botho Foundation

//! Transport negotiation protocol.
//!
//! This module implements the protocol for negotiating which transport
//! to use between two peers based on their mutual capabilities.

use serde::{Deserialize, Serialize};
use std::io;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::capabilities::{NatType, TransportCapabilities, TransportType};

/// Maximum size of a negotiation message in bytes.
const MAX_NEGOTIATION_MESSAGE_SIZE: usize = 4096;

/// Protocol version for negotiation messages.
const NEGOTIATION_PROTOCOL_VERSION: u8 = 1;

/// Errors that can occur during transport negotiation.
#[derive(Debug, Error)]
pub enum NegotiationError {
    /// No common transport available between peers.
    #[error("no common transport available")]
    NoCommonTransport,

    /// Peer rejected the transport upgrade.
    #[error("peer rejected upgrade: {reason}")]
    Rejected { reason: String },

    /// I/O error during negotiation.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// Serialization error.
    #[error("serialization error: {0}")]
    Serialization(#[from] bincode::Error),

    /// Protocol version mismatch.
    #[error("protocol version mismatch: expected {expected}, got {got}")]
    VersionMismatch { expected: u8, got: u8 },

    /// Message too large.
    #[error("message too large: {size} bytes (max: {max})")]
    MessageTooLarge { size: usize, max: usize },

    /// Invalid message format.
    #[error("invalid message format")]
    InvalidMessage,

    /// Negotiation timeout.
    #[error("negotiation timeout")]
    Timeout,

    /// Upgrade failed after negotiation.
    #[error("upgrade failed: {0}")]
    UpgradeFailed(String),
}

/// Transport negotiation messages.
///
/// These messages are exchanged between peers to negotiate which
/// transport to use for their connection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum NegotiationMessage {
    /// Propose transport upgrade with offered transports.
    Propose {
        /// Protocol version
        version: u8,
        /// Offered transports in preference order
        offered: Vec<TransportType>,
        /// Our NAT type (helps peer make decisions)
        nat_type: NatType,
    },

    /// Accept with selected transport.
    Accept {
        /// Protocol version
        version: u8,
        /// Selected transport from the offered list
        selected: TransportType,
    },

    /// Reject upgrade (stay on current transport).
    Reject {
        /// Protocol version
        version: u8,
        /// Reason for rejection
        reason: String,
    },
}

impl NegotiationMessage {
    /// Create a propose message from capabilities.
    pub fn propose(caps: &TransportCapabilities) -> Self {
        Self::Propose {
            version: NEGOTIATION_PROTOCOL_VERSION,
            offered: caps.supported.clone(),
            nat_type: caps.nat_type,
        }
    }

    /// Create an accept message.
    pub fn accept(transport: TransportType) -> Self {
        Self::Accept {
            version: NEGOTIATION_PROTOCOL_VERSION,
            selected: transport,
        }
    }

    /// Create a reject message.
    pub fn reject(reason: impl Into<String>) -> Self {
        Self::Reject {
            version: NEGOTIATION_PROTOCOL_VERSION,
            reason: reason.into(),
        }
    }

    /// Get the protocol version from any message type.
    pub fn version(&self) -> u8 {
        match self {
            Self::Propose { version, .. } => *version,
            Self::Accept { version, .. } => *version,
            Self::Reject { version, .. } => *version,
        }
    }

    /// Serialize the message to bytes with length prefix.
    pub fn to_bytes(&self) -> Result<Vec<u8>, bincode::Error> {
        let payload = bincode::serialize(self)?;
        let len = payload.len() as u32;

        let mut bytes = Vec::with_capacity(4 + payload.len());
        bytes.extend_from_slice(&len.to_be_bytes());
        bytes.extend_from_slice(&payload);

        Ok(bytes)
    }

    /// Deserialize a message from bytes (without length prefix).
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, bincode::Error> {
        bincode::deserialize(bytes)
    }
}

/// Read a negotiation message from an async stream.
pub async fn read_message<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<NegotiationMessage, NegotiationError> {
    // Read length prefix (4 bytes, big-endian)
    let mut len_bytes = [0u8; 4];
    reader.read_exact(&mut len_bytes).await?;
    let len = u32::from_be_bytes(len_bytes) as usize;

    // Check message size
    if len > MAX_NEGOTIATION_MESSAGE_SIZE {
        return Err(NegotiationError::MessageTooLarge {
            size: len,
            max: MAX_NEGOTIATION_MESSAGE_SIZE,
        });
    }

    // Read message payload
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).await?;

    // Deserialize
    let message = NegotiationMessage::from_bytes(&payload)?;

    // Check version compatibility
    if message.version() > NEGOTIATION_PROTOCOL_VERSION {
        return Err(NegotiationError::VersionMismatch {
            expected: NEGOTIATION_PROTOCOL_VERSION,
            got: message.version(),
        });
    }

    Ok(message)
}

/// Write a negotiation message to an async stream.
pub async fn write_message<W: AsyncWrite + Unpin>(
    writer: &mut W,
    message: &NegotiationMessage,
) -> Result<(), NegotiationError> {
    let bytes = message.to_bytes()?;
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}

/// Select the best transport for two peers based on their capabilities.
///
/// This function implements the transport selection algorithm that considers:
/// 1. Transport preferences of both peers
/// 2. NAT compatibility for WebRTC
/// 3. Fallback to more compatible transports
pub fn select_transport(
    our_caps: &TransportCapabilities,
    peer_caps: &TransportCapabilities,
) -> TransportType {
    // Use the best_common algorithm from capabilities
    our_caps
        .best_common(peer_caps)
        .unwrap_or(TransportType::Plain)
}

/// Negotiate transport upgrade as the initiator.
///
/// This function:
/// 1. Sends a Propose message with our capabilities
/// 2. Waits for Accept or Reject from peer
/// 3. Returns the agreed transport or an error
pub async fn negotiate_transport_initiator<S>(
    stream: &mut S,
    our_caps: &TransportCapabilities,
) -> Result<TransportType, NegotiationError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    // Send proposal
    let propose = NegotiationMessage::propose(our_caps);
    write_message(stream, &propose).await?;

    // Wait for response
    let response = read_message(stream).await?;

    match response {
        NegotiationMessage::Accept { selected, .. } => {
            // Verify selected transport is one we offered
            if !our_caps.supports(selected) {
                return Err(NegotiationError::InvalidMessage);
            }
            Ok(selected)
        }
        NegotiationMessage::Reject { reason, .. } => Err(NegotiationError::Rejected { reason }),
        NegotiationMessage::Propose { .. } => {
            // Unexpected message type
            Err(NegotiationError::InvalidMessage)
        }
    }
}

/// Negotiate transport upgrade as the responder.
///
/// This function:
/// 1. Waits for a Propose message from the initiator
/// 2. Selects the best common transport
/// 3. Sends Accept or Reject
/// 4. Returns the agreed transport or an error
pub async fn negotiate_transport_responder<S>(
    stream: &mut S,
    our_caps: &TransportCapabilities,
) -> Result<TransportType, NegotiationError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    // Wait for proposal
    let proposal = read_message(stream).await?;

    match proposal {
        NegotiationMessage::Propose {
            offered, nat_type, ..
        } => {
            // Create peer capabilities from proposal
            let peer_caps = TransportCapabilities::new(
                offered.clone(),
                offered.first().copied().unwrap_or(TransportType::Plain),
                nat_type,
            );

            // Find best common transport
            match our_caps.best_common(&peer_caps) {
                Some(selected) => {
                    // Send accept
                    let accept = NegotiationMessage::accept(selected);
                    write_message(stream, &accept).await?;
                    Ok(selected)
                }
                None => {
                    // No common transport - reject
                    let reject = NegotiationMessage::reject("no common transport supported");
                    write_message(stream, &reject).await?;
                    Err(NegotiationError::NoCommonTransport)
                }
            }
        }
        _ => Err(NegotiationError::InvalidMessage),
    }
}

/// Configuration for transport negotiation.
#[derive(Debug, Clone)]
pub struct NegotiationConfig {
    /// Timeout for negotiation (default: 10 seconds)
    pub timeout: std::time::Duration,
    /// Whether to allow fallback to plain transport on failure
    pub allow_plain_fallback: bool,
}

impl Default for NegotiationConfig {
    fn default() -> Self {
        Self {
            timeout: std::time::Duration::from_secs(10),
            allow_plain_fallback: true,
        }
    }
}

/// Result of a transport upgrade attempt.
#[derive(Debug)]
pub enum UpgradeResult<T> {
    /// Successfully upgraded to new transport
    Upgraded(T),
    /// Kept original connection (upgrade not needed or failed gracefully)
    Kept(T),
    /// Upgrade failed
    Failed(NegotiationError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{duplex, DuplexStream};

    // ========================================================================
    // NegotiationMessage tests
    // ========================================================================

    #[test]
    fn test_message_propose() {
        let caps = TransportCapabilities::full(NatType::Open);
        let msg = NegotiationMessage::propose(&caps);

        match msg {
            NegotiationMessage::Propose {
                version,
                offered,
                nat_type,
            } => {
                assert_eq!(version, NEGOTIATION_PROTOCOL_VERSION);
                assert_eq!(offered.len(), 3);
                assert_eq!(nat_type, NatType::Open);
            }
            _ => panic!("Expected Propose message"),
        }
    }

    #[test]
    fn test_message_accept() {
        let msg = NegotiationMessage::accept(TransportType::WebRTC);

        match msg {
            NegotiationMessage::Accept { version, selected } => {
                assert_eq!(version, NEGOTIATION_PROTOCOL_VERSION);
                assert_eq!(selected, TransportType::WebRTC);
            }
            _ => panic!("Expected Accept message"),
        }
    }

    #[test]
    fn test_message_reject() {
        let msg = NegotiationMessage::reject("test reason");

        match msg {
            NegotiationMessage::Reject { version, reason } => {
                assert_eq!(version, NEGOTIATION_PROTOCOL_VERSION);
                assert_eq!(reason, "test reason");
            }
            _ => panic!("Expected Reject message"),
        }
    }

    #[test]
    fn test_message_version() {
        let propose = NegotiationMessage::propose(&TransportCapabilities::default());
        let accept = NegotiationMessage::accept(TransportType::Plain);
        let reject = NegotiationMessage::reject("test");

        assert_eq!(propose.version(), NEGOTIATION_PROTOCOL_VERSION);
        assert_eq!(accept.version(), NEGOTIATION_PROTOCOL_VERSION);
        assert_eq!(reject.version(), NEGOTIATION_PROTOCOL_VERSION);
    }

    #[test]
    fn test_message_serialization() {
        let original = NegotiationMessage::propose(&TransportCapabilities::full(NatType::Open));
        let bytes = original.to_bytes().unwrap();

        // Skip 4-byte length prefix for deserialization
        let payload = &bytes[4..];
        let deserialized = NegotiationMessage::from_bytes(payload).unwrap();

        assert_eq!(original, deserialized);
    }

    #[test]
    fn test_message_roundtrip_all_types() {
        let messages = vec![
            NegotiationMessage::propose(&TransportCapabilities::default()),
            NegotiationMessage::accept(TransportType::TlsTunnel),
            NegotiationMessage::reject("no common transport"),
        ];

        for original in messages {
            let bytes = original.to_bytes().unwrap();
            let payload = &bytes[4..];
            let deserialized = NegotiationMessage::from_bytes(payload).unwrap();
            assert_eq!(original, deserialized);
        }
    }

    // ========================================================================
    // Transport selection tests
    // ========================================================================

    #[test]
    fn test_select_transport_both_webrtc() {
        let our_caps = TransportCapabilities::full(NatType::Open);
        let peer_caps = TransportCapabilities::full(NatType::FullCone);

        let selected = select_transport(&our_caps, &peer_caps);
        assert_eq!(selected, TransportType::WebRTC);
    }

    #[test]
    fn test_select_transport_fallback_to_plain() {
        let our_caps = TransportCapabilities::plain_only();
        let peer_caps = TransportCapabilities::full(NatType::Open);

        let selected = select_transport(&our_caps, &peer_caps);
        assert_eq!(selected, TransportType::Plain);
    }

    #[test]
    fn test_select_transport_webrtc_nat_penalty() {
        let our_caps = TransportCapabilities::full(NatType::Symmetric);
        let peer_caps = TransportCapabilities::full(NatType::Symmetric);

        // Both symmetric NAT - should avoid WebRTC
        let selected = select_transport(&our_caps, &peer_caps);
        assert_eq!(selected, TransportType::TlsTunnel);
    }

    #[test]
    fn test_select_transport_no_common_returns_plain() {
        let our_caps = TransportCapabilities::new(
            vec![TransportType::WebRTC],
            TransportType::WebRTC,
            NatType::Open,
        );
        let peer_caps = TransportCapabilities::new(
            vec![TransportType::TlsTunnel],
            TransportType::TlsTunnel,
            NatType::Open,
        );

        // No common transport - falls back to Plain
        let selected = select_transport(&our_caps, &peer_caps);
        assert_eq!(selected, TransportType::Plain);
    }

    // ========================================================================
    // Async negotiation tests
    // ========================================================================

    fn create_stream_pair() -> (DuplexStream, DuplexStream) {
        duplex(8192)
    }

    #[tokio::test]
    async fn test_read_write_message() {
        let (mut client, mut server) = create_stream_pair();

        let original = NegotiationMessage::propose(&TransportCapabilities::default());

        // Write from client
        tokio::spawn(async move {
            write_message(&mut client, &original).await.unwrap();
        });

        // Read from server
        let received = read_message(&mut server).await.unwrap();

        match received {
            NegotiationMessage::Propose { .. } => {}
            _ => panic!("Expected Propose message"),
        }
    }

    #[tokio::test]
    async fn test_negotiate_success() {
        let (mut client, mut server) = create_stream_pair();

        let our_caps = TransportCapabilities::full(NatType::Open);
        let peer_caps = TransportCapabilities::full(NatType::FullCone);

        // Run initiator and responder concurrently
        let initiator = tokio::spawn(async move {
            negotiate_transport_initiator(&mut client, &our_caps).await
        });

        let responder = tokio::spawn(async move {
            negotiate_transport_responder(&mut server, &peer_caps).await
        });

        let (init_result, resp_result) = tokio::join!(initiator, responder);

        let init_transport = init_result.unwrap().unwrap();
        let resp_transport = resp_result.unwrap().unwrap();

        // Both should agree on the same transport
        assert_eq!(init_transport, resp_transport);
        assert_eq!(init_transport, TransportType::WebRTC);
    }

    #[tokio::test]
    async fn test_negotiate_plain_only() {
        let (mut client, mut server) = create_stream_pair();

        let our_caps = TransportCapabilities::plain_only();
        let peer_caps = TransportCapabilities::full(NatType::Open);

        let initiator = tokio::spawn(async move {
            negotiate_transport_initiator(&mut client, &our_caps).await
        });

        let responder = tokio::spawn(async move {
            negotiate_transport_responder(&mut server, &peer_caps).await
        });

        let (init_result, resp_result) = tokio::join!(initiator, responder);

        let init_transport = init_result.unwrap().unwrap();
        let resp_transport = resp_result.unwrap().unwrap();

        assert_eq!(init_transport, TransportType::Plain);
        assert_eq!(resp_transport, TransportType::Plain);
    }

    #[tokio::test]
    async fn test_negotiate_no_common_transport() {
        let (mut client, mut server) = create_stream_pair();

        // Initiator only supports WebRTC
        let our_caps = TransportCapabilities::new(
            vec![TransportType::WebRTC],
            TransportType::WebRTC,
            NatType::Open,
        );
        // Responder only supports TLS tunnel
        let peer_caps = TransportCapabilities::new(
            vec![TransportType::TlsTunnel],
            TransportType::TlsTunnel,
            NatType::Open,
        );

        let initiator = tokio::spawn(async move {
            negotiate_transport_initiator(&mut client, &our_caps).await
        });

        let responder = tokio::spawn(async move {
            negotiate_transport_responder(&mut server, &peer_caps).await
        });

        let (init_result, resp_result) = tokio::join!(initiator, responder);

        // Both should get rejection
        assert!(matches!(
            init_result.unwrap(),
            Err(NegotiationError::Rejected { .. })
        ));
        assert!(matches!(
            resp_result.unwrap(),
            Err(NegotiationError::NoCommonTransport)
        ));
    }

    // ========================================================================
    // NegotiationConfig tests
    // ========================================================================

    #[test]
    fn test_negotiation_config_default() {
        let config = NegotiationConfig::default();
        assert_eq!(config.timeout, std::time::Duration::from_secs(10));
        assert!(config.allow_plain_fallback);
    }

    // ========================================================================
    // Error tests
    // ========================================================================

    #[test]
    fn test_negotiation_error_display() {
        let err = NegotiationError::NoCommonTransport;
        assert_eq!(format!("{}", err), "no common transport available");

        let err = NegotiationError::Rejected {
            reason: "test".to_string(),
        };
        assert_eq!(format!("{}", err), "peer rejected upgrade: test");

        let err = NegotiationError::VersionMismatch {
            expected: 1,
            got: 2,
        };
        assert_eq!(
            format!("{}", err),
            "protocol version mismatch: expected 1, got 2"
        );

        let err = NegotiationError::MessageTooLarge {
            size: 5000,
            max: 4096,
        };
        assert_eq!(
            format!("{}", err),
            "message too large: 5000 bytes (max: 4096)"
        );
    }
}
