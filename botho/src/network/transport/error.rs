// Copyright (c) 2024 Botho Foundation

//! Transport error types for pluggable transports.
//!
//! This module defines the error types used by the transport layer,
//! covering connection failures, handshake errors, and transport-specific issues.

use std::fmt;
use std::io;

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
        }
    }
}

impl std::error::Error for TransportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            TransportError::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for TransportError {
    fn from(err: io::Error) -> Self {
        TransportError::Io(err)
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
}
