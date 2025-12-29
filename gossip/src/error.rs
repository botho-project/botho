// Copyright (c) 2024 Botho Foundation

//! Error types for the gossip module.

use displaydoc::Display;
use thiserror::Error;

/// Errors that can occur in the gossip module.
#[derive(Debug, Display, Error)]
pub enum GossipError {
    /// Invalid announcement signature
    InvalidSignature,

    /// Announcement is expired (older than max age)
    AnnouncementExpired,

    /// Announcement timestamp is in the future
    FutureTimestamp,

    /// Failed to serialize message: {0}
    SerializationError(String),

    /// Failed to deserialize message: {0}
    DeserializationError(String),

    /// Network error: {0}
    NetworkError(String),

    /// Peer not found: {0}
    PeerNotFound(String),

    /// Store is full
    StoreFull,

    /// Invalid peer URI: {0}
    InvalidPeerUri(String),

    /// libp2p error: {0}
    Libp2pError(String),

    /// Channel closed
    ChannelClosed,

    /// Timeout waiting for response
    Timeout,

    /// Bootstrap failed: {0}
    BootstrapFailed(String),
}

impl From<bth_util_serial::encode::Error> for GossipError {
    fn from(err: bth_util_serial::encode::Error) -> Self {
        GossipError::SerializationError(err.to_string())
    }
}

impl From<bth_util_serial::decode::Error> for GossipError {
    fn from(err: bth_util_serial::decode::Error) -> Self {
        GossipError::DeserializationError(err.to_string())
    }
}

/// Result type for gossip operations.
pub type GossipResult<T> = Result<T, GossipError>;
