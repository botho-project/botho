// Copyright (c) 2024 Botho Foundation

//! Transport error types for pluggable transports.
//!
//! This module defines the error types used by the transport layer,
//! covering connection failures, handshake errors, and transport-specific issues.

use std::fmt;
use std::io;

use super::webrtc::ice::IceError;
use super::webrtc::stun::StunError;

/// Result type for transport operations.
pub type TransportResult<T> = Result<T, TransportError>;

/// Errors that can occur during transport operations.
#[derive(Debug)]
pub enum TransportError {
    /// Connection to peer failed.
    ConnectionFailed(String),

    /// Transport handshake failed.
    HandshakeFailed(String),

    /// Transport type not supported by peer.
    NotSupported,

    /// Transport negotiation failed.
    NegotiationFailed(String),

    /// Connection timed out.
    Timeout,

    /// Connection was closed unexpectedly.
    ConnectionClosed,

    /// Invalid peer address or identifier.
    InvalidPeer(String),

    /// Transport configuration error.
    Configuration(String),

    /// Underlying I/O error.
    Io(io::Error),

    /// Transport-specific error.
    Transport(String),

    /// ICE-related error.
    Ice(IceError),

    /// STUN-related error.
    Stun(StunError),

    /// WebRTC-specific error.
    WebRtc(WebRtcError),

    /// Signaling error (SDP exchange failed).
    SignalingFailed(String),

    /// ICE connection failed.
    IceFailed(String),

    /// Data channel error.
    DataChannel(String),
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransportError::ConnectionFailed(msg) => write!(f, "connection failed: {}", msg),
            TransportError::HandshakeFailed(msg) => write!(f, "handshake failed: {}", msg),
            TransportError::NotSupported => write!(f, "transport not supported by peer"),
            TransportError::NegotiationFailed(msg) => write!(f, "negotiation failed: {}", msg),
            TransportError::Timeout => write!(f, "connection timed out"),
            TransportError::ConnectionClosed => write!(f, "connection closed unexpectedly"),
            TransportError::InvalidPeer(msg) => write!(f, "invalid peer: {}", msg),
            TransportError::Configuration(msg) => write!(f, "configuration error: {}", msg),
            TransportError::Io(err) => write!(f, "I/O error: {}", err),
            TransportError::Transport(msg) => write!(f, "transport error: {}", msg),
            TransportError::Ice(err) => write!(f, "ICE error: {}", err),
            TransportError::Stun(err) => write!(f, "STUN error: {}", err),
            TransportError::WebRtc(err) => write!(f, "WebRTC error: {}", err),
            TransportError::SignalingFailed(msg) => write!(f, "signaling error: {}", msg),
            TransportError::IceFailed(msg) => write!(f, "ICE connection failed: {}", msg),
            TransportError::DataChannel(msg) => write!(f, "data channel error: {}", msg),
        }
    }
}

impl TransportError {
    /// Creates a handshake failed error.
    pub fn handshake_failed(msg: impl Into<String>) -> Self {
        TransportError::HandshakeFailed(msg.into())
    }

    /// Creates a signaling failed error.
    pub fn signaling_failed(msg: impl Into<String>) -> Self {
        TransportError::SignalingFailed(msg.into())
    }

    /// Creates an ICE failed error.
    pub fn ice_failed(msg: impl Into<String>) -> Self {
        TransportError::IceFailed(msg.into())
    }

    /// Creates a data channel error.
    pub fn data_channel(msg: impl Into<String>) -> Self {
        TransportError::DataChannel(msg.into())
    }

    /// Creates a timeout error.
    pub fn timeout() -> Self {
        TransportError::Timeout
    }

    /// Creates a configuration error.
    pub fn configuration(msg: impl Into<String>) -> Self {
        TransportError::Configuration(msg.into())
    }

    /// Returns true if this error indicates the connection was closed.
    pub fn is_connection_closed(&self) -> bool {
        matches!(self, TransportError::ConnectionClosed)
    }

    /// Returns true if this error might be retryable.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            TransportError::Timeout
                | TransportError::IceFailed(_)
                | TransportError::SignalingFailed(_)
        )
    }
}

impl std::error::Error for TransportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            TransportError::Io(err) => Some(err),
            TransportError::Ice(err) => Some(err),
            TransportError::Stun(err) => Some(err),
            TransportError::WebRtc(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for TransportError {
    fn from(err: io::Error) -> Self {
        TransportError::Io(err)
    }
}

impl From<IceError> for TransportError {
    fn from(err: IceError) -> Self {
        TransportError::Ice(err)
    }
}

impl From<StunError> for TransportError {
    fn from(err: StunError) -> Self {
        TransportError::Stun(err)
    }
}

impl From<WebRtcError> for TransportError {
    fn from(err: WebRtcError) -> Self {
        TransportError::WebRtc(err)
    }
}

/// WebRTC-specific errors.
#[derive(Debug)]
pub enum WebRtcError {
    /// Failed to create peer connection.
    PeerConnectionCreate(String),

    /// Failed to create data channel.
    DataChannelCreate(String),

    /// Failed to create SDP offer.
    CreateOffer(String),

    /// Failed to create SDP answer.
    CreateAnswer(String),

    /// Failed to set local description.
    SetLocalDescription(String),

    /// Failed to set remote description.
    SetRemoteDescription(String),

    /// Failed to gather ICE candidates.
    IceGathering(String),

    /// Invalid SDP format.
    InvalidSdp(String),

    /// Data channel not open.
    DataChannelNotOpen,

    /// Failed to send data.
    SendFailed(String),

    /// Failed to receive data.
    ReceiveFailed(String),

    /// Connection state error.
    InvalidState { expected: String, actual: String },
}

impl fmt::Display for WebRtcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WebRtcError::PeerConnectionCreate(msg) => {
                write!(f, "failed to create peer connection: {}", msg)
            }
            WebRtcError::DataChannelCreate(msg) => {
                write!(f, "failed to create data channel: {}", msg)
            }
            WebRtcError::CreateOffer(msg) => write!(f, "failed to create offer: {}", msg),
            WebRtcError::CreateAnswer(msg) => write!(f, "failed to create answer: {}", msg),
            WebRtcError::SetLocalDescription(msg) => {
                write!(f, "failed to set local description: {}", msg)
            }
            WebRtcError::SetRemoteDescription(msg) => {
                write!(f, "failed to set remote description: {}", msg)
            }
            WebRtcError::IceGathering(msg) => write!(f, "failed to gather ICE candidates: {}", msg),
            WebRtcError::InvalidSdp(msg) => write!(f, "invalid SDP: {}", msg),
            WebRtcError::DataChannelNotOpen => write!(f, "data channel not open"),
            WebRtcError::SendFailed(msg) => write!(f, "failed to send data: {}", msg),
            WebRtcError::ReceiveFailed(msg) => write!(f, "failed to receive data: {}", msg),
            WebRtcError::InvalidState { expected, actual } => {
                write!(f, "connection state error: expected {}, got {}", expected, actual)
            }
        }
    }
}

impl std::error::Error for WebRtcError {}

impl WebRtcError {
    /// Creates a peer connection create error.
    pub fn peer_connection_create(msg: impl Into<String>) -> Self {
        WebRtcError::PeerConnectionCreate(msg.into())
    }

    /// Creates a data channel create error.
    pub fn data_channel_create(msg: impl Into<String>) -> Self {
        WebRtcError::DataChannelCreate(msg.into())
    }

    /// Creates a create offer error.
    pub fn create_offer(msg: impl Into<String>) -> Self {
        WebRtcError::CreateOffer(msg.into())
    }

    /// Creates a create answer error.
    pub fn create_answer(msg: impl Into<String>) -> Self {
        WebRtcError::CreateAnswer(msg.into())
    }

    /// Creates a set local description error.
    pub fn set_local_description(msg: impl Into<String>) -> Self {
        WebRtcError::SetLocalDescription(msg.into())
    }

    /// Creates a set remote description error.
    pub fn set_remote_description(msg: impl Into<String>) -> Self {
        WebRtcError::SetRemoteDescription(msg.into())
    }

    /// Creates an ICE gathering error.
    pub fn ice_gathering(msg: impl Into<String>) -> Self {
        WebRtcError::IceGathering(msg.into())
    }

    /// Creates an invalid SDP error.
    pub fn invalid_sdp(msg: impl Into<String>) -> Self {
        WebRtcError::InvalidSdp(msg.into())
    }

    /// Creates a send failed error.
    pub fn send_failed(msg: impl Into<String>) -> Self {
        WebRtcError::SendFailed(msg.into())
    }

    /// Creates a receive failed error.
    pub fn receive_failed(msg: impl Into<String>) -> Self {
        WebRtcError::ReceiveFailed(msg.into())
    }

    /// Creates an invalid state error.
    pub fn invalid_state(expected: impl Into<String>, actual: impl Into<String>) -> Self {
        WebRtcError::InvalidState {
            expected: expected.into(),
            actual: actual.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn test_error_display() {
        let err = TransportError::ConnectionFailed("refused".to_string());
        assert_eq!(err.to_string(), "connection failed: refused");

        let err = TransportError::NotSupported;
        assert_eq!(err.to_string(), "transport not supported by peer");

        let err = TransportError::Timeout;
        assert_eq!(err.to_string(), "connection timed out");
    }

    #[test]
    fn test_error_from_io() {
        let io_err = io::Error::new(io::ErrorKind::ConnectionRefused, "refused");
        let transport_err: TransportError = io_err.into();

        match transport_err {
            TransportError::Io(e) => assert_eq!(e.kind(), io::ErrorKind::ConnectionRefused),
            _ => panic!("expected Io variant"),
        }
    }

    #[test]
    fn test_error_source() {
        let io_err = io::Error::new(io::ErrorKind::Other, "test");
        let transport_err = TransportError::Io(io_err);
        assert!(transport_err.source().is_some());

        let other_err = TransportError::Timeout;
        assert!(other_err.source().is_none());
    }

    #[test]
    fn test_transport_error_is_retryable() {
        assert!(TransportError::timeout().is_retryable());
        assert!(TransportError::ice_failed("test").is_retryable());
        assert!(TransportError::signaling_failed("test").is_retryable());
        assert!(!TransportError::ConnectionClosed.is_retryable());
        assert!(!TransportError::handshake_failed("test").is_retryable());
    }

    #[test]
    fn test_transport_error_is_connection_closed() {
        assert!(TransportError::ConnectionClosed.is_connection_closed());
        assert!(!TransportError::timeout().is_connection_closed());
    }

    #[test]
    fn test_webrtc_error_display() {
        let err = WebRtcError::data_channel_create("test error");
        assert!(err.to_string().contains("test error"));
    }
}
