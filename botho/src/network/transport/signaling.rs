// Copyright (c) 2024 Botho Foundation

//! Signaling channel for WebRTC SDP exchange.
//!
//! This module implements the signaling protocol for exchanging SDP (Session
//! Description Protocol) offers and answers between peers during WebRTC
//! connection establishment. The signaling occurs over the existing libp2p
//! connection before upgrading to WebRTC.
//!
//! # Overview
//!
//! WebRTC requires out-of-band signaling to exchange:
//! - **SDP Offer**: Initiator's session description (codecs, ICE candidates, fingerprint)
//! - **SDP Answer**: Responder's session description
//! - **ICE Candidates**: Trickle ICE updates for NAT traversal
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    SIGNALING FLOW                           │
//! │                                                             │
//! │   Peer A (Offerer)              Peer B (Answerer)          │
//! │                                                             │
//! │   1. Create SDP Offer                                       │
//! │   2. Send Offer ──────────────────────────→                 │
//! │                                    3. Process Offer         │
//! │                                    4. Create SDP Answer     │
//! │   5. Process Answer ←─────────────────────── Send Answer   │
//! │   6. Send ICE Candidates ←───────→ ICE Candidates           │
//! │   7. WebRTC Connection Established                          │
//! │                                                             │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Security Considerations
//!
//! - Session IDs are random to prevent prediction
//! - Timeout cleanup prevents resource exhaustion
//! - SDP validation prevents malformed data attacks
//!
//! # References
//!
//! - Design: `docs/design/traffic-privacy-roadmap.md` (Section 3.5)
//! - SDP: RFC 4566
//! - WebRTC Signaling: https://developer.mozilla.org/en-US/docs/Web/API/WebRTC_API/Signaling_and_video_calling

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use libp2p::PeerId;
use rand::{CryptoRng, RngCore};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::timeout;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

/// Length of session identifiers in bytes.
pub const SESSION_ID_LEN: usize = 16;

/// Default timeout for signaling operations.
pub const DEFAULT_SIGNALING_TIMEOUT_SECS: u64 = 30;

/// Maximum SDP size in bytes (128KB should be plenty for any reasonable SDP).
pub const MAX_SDP_SIZE: usize = 128 * 1024;

/// Maximum ICE candidate size in bytes.
pub const MAX_ICE_CANDIDATE_SIZE: usize = 4096;

/// Maximum number of pending sessions per peer.
pub const MAX_SESSIONS_PER_PEER: usize = 4;

/// Maximum number of ICE candidates per session.
pub const MAX_ICE_CANDIDATES_PER_SESSION: usize = 32;

/// Unique identifier for a signaling session.
///
/// Session IDs are 16-byte random values that identify a specific signaling
/// exchange between two peers.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId([u8; SESSION_ID_LEN]);

impl SessionId {
    /// Generate a new random session ID.
    pub fn random<R: RngCore + CryptoRng>(rng: &mut R) -> Self {
        let mut bytes = [0u8; SESSION_ID_LEN];
        rng.fill_bytes(&mut bytes);
        Self(bytes)
    }

    /// Create a session ID from raw bytes.
    ///
    /// # Errors
    ///
    /// Returns `None` if the slice length is not exactly [`SESSION_ID_LEN`].
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != SESSION_ID_LEN {
            return None;
        }
        let mut arr = [0u8; SESSION_ID_LEN];
        arr.copy_from_slice(bytes);
        Some(Self(arr))
    }

    /// Get the raw bytes of this session ID.
    #[inline]
    pub fn as_bytes(&self) -> &[u8; SESSION_ID_LEN] {
        &self.0
    }
}

impl AsRef<[u8]> for SessionId {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SessionId({})", hex::encode(&self.0[..4]))
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", hex::encode(&self.0[..8]))
    }
}

/// Signaling message types for WebRTC connection establishment.
///
/// These messages are exchanged over the existing libp2p connection to
/// establish a WebRTC data channel connection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SignalingMessage {
    /// WebRTC SDP offer from the initiator.
    Offer {
        /// The SDP offer string.
        sdp: String,
        /// Session identifier for this exchange.
        session_id: SessionId,
    },

    /// WebRTC SDP answer from the responder.
    Answer {
        /// The SDP answer string.
        sdp: String,
        /// Session identifier for this exchange.
        session_id: SessionId,
    },

    /// ICE candidate for NAT traversal (trickle ICE).
    IceCandidate {
        /// The ICE candidate string.
        candidate: String,
        /// Media stream identification tag.
        sdp_mid: Option<String>,
        /// Index of the media description.
        sdp_mline_index: Option<u16>,
        /// Session identifier for this exchange.
        session_id: SessionId,
    },

    /// Reject transport upgrade request.
    Reject {
        /// Session identifier for the rejected exchange.
        session_id: SessionId,
        /// Reason for rejection.
        reason: String,
    },
}

impl SignalingMessage {
    /// Get the session ID associated with this message.
    pub fn session_id(&self) -> SessionId {
        match self {
            Self::Offer { session_id, .. } => *session_id,
            Self::Answer { session_id, .. } => *session_id,
            Self::IceCandidate { session_id, .. } => *session_id,
            Self::Reject { session_id, .. } => *session_id,
        }
    }

    /// Validate the message contents.
    ///
    /// Returns an error if the message contains invalid or oversized data.
    pub fn validate(&self) -> Result<(), SignalingError> {
        match self {
            Self::Offer { sdp, .. } | Self::Answer { sdp, .. } => {
                if sdp.len() > MAX_SDP_SIZE {
                    return Err(SignalingError::InvalidSdp(format!(
                        "SDP exceeds maximum size: {} > {}",
                        sdp.len(),
                        MAX_SDP_SIZE
                    )));
                }
                // Basic SDP validation: should start with v=
                if !sdp.starts_with("v=") {
                    return Err(SignalingError::InvalidSdp(
                        "SDP must start with 'v=' line".to_string(),
                    ));
                }
            }
            Self::IceCandidate { candidate, .. } => {
                if candidate.len() > MAX_ICE_CANDIDATE_SIZE {
                    return Err(SignalingError::InvalidSdp(format!(
                        "ICE candidate exceeds maximum size: {} > {}",
                        candidate.len(),
                        MAX_ICE_CANDIDATE_SIZE
                    )));
                }
            }
            Self::Reject { reason, .. } => {
                if reason.len() > 1024 {
                    return Err(SignalingError::InvalidSdp(
                        "Reject reason too long".to_string(),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Serialize the message to bytes.
    pub fn to_bytes(&self) -> Result<Vec<u8>, SignalingError> {
        bincode::serialize(self).map_err(|e| SignalingError::Io(std::io::Error::other(e)))
    }

    /// Deserialize a message from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, SignalingError> {
        let msg: Self =
            bincode::deserialize(bytes).map_err(|e| SignalingError::Io(std::io::Error::other(e)))?;
        msg.validate()?;
        Ok(msg)
    }
}

/// Errors that can occur during signaling.
#[derive(Debug, Error)]
pub enum SignalingError {
    /// Signaling operation timed out.
    #[error("signaling timeout")]
    Timeout,

    /// Peer rejected the transport upgrade.
    #[error("peer rejected: {0}")]
    Rejected(String),

    /// Invalid SDP content.
    #[error("invalid SDP: {0}")]
    InvalidSdp(String),

    /// Session not found.
    #[error("session not found: {0}")]
    SessionNotFound(SessionId),

    /// Session already exists.
    #[error("session already exists: {0}")]
    SessionExists(SessionId),

    /// Too many pending sessions.
    #[error("too many pending sessions for peer")]
    TooManySessions,

    /// Too many ICE candidates.
    #[error("too many ICE candidates for session")]
    TooManyIceCandidates,

    /// I/O error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Protocol error (unexpected message type).
    #[error("protocol error: {0}")]
    Protocol(String),

    /// Channel closed.
    #[error("channel closed")]
    ChannelClosed,
}

/// Role in the signaling exchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalingRole {
    /// The peer initiating the connection (sends offer).
    Offerer,
    /// The peer responding (sends answer).
    Answerer,
}

/// State of a pending signaling session.
#[derive(Debug)]
pub struct SignalingSession {
    /// The remote peer.
    pub peer: PeerId,
    /// Our role in this exchange.
    pub role: SignalingRole,
    /// Our local SDP (offer or answer).
    pub local_sdp: Option<String>,
    /// Remote SDP (offer or answer).
    pub remote_sdp: Option<String>,
    /// Collected ICE candidates from the remote peer.
    pub ice_candidates: Vec<IceCandidate>,
    /// When this session was created.
    pub created_at: Instant,
}

/// ICE candidate information.
#[derive(Debug, Clone)]
pub struct IceCandidate {
    /// The ICE candidate string.
    pub candidate: String,
    /// Media stream identification tag.
    pub sdp_mid: Option<String>,
    /// Index of the media description.
    pub sdp_mline_index: Option<u16>,
}

impl SignalingSession {
    /// Create a new signaling session.
    pub fn new(peer: PeerId, role: SignalingRole) -> Self {
        Self {
            peer,
            role,
            local_sdp: None,
            remote_sdp: None,
            ice_candidates: Vec::new(),
            created_at: Instant::now(),
        }
    }

    /// Check if the session has expired.
    pub fn is_expired(&self, timeout: Duration) -> bool {
        self.created_at.elapsed() > timeout
    }

    /// Check if we have both local and remote SDP.
    pub fn is_complete(&self) -> bool {
        self.local_sdp.is_some() && self.remote_sdp.is_some()
    }

    /// Add an ICE candidate.
    pub fn add_ice_candidate(&mut self, candidate: IceCandidate) -> Result<(), SignalingError> {
        if self.ice_candidates.len() >= MAX_ICE_CANDIDATES_PER_SESSION {
            return Err(SignalingError::TooManyIceCandidates);
        }
        self.ice_candidates.push(candidate);
        Ok(())
    }
}

/// Signaling state manager for tracking pending sessions.
///
/// This struct manages the state of all pending signaling sessions,
/// handles timeout cleanup, and enforces resource limits.
pub struct SignalingState {
    /// Active signaling sessions indexed by session ID.
    sessions: HashMap<SessionId, SignalingSession>,
    /// Index from peer ID to their session IDs.
    peer_sessions: HashMap<PeerId, Vec<SessionId>>,
    /// Session timeout duration.
    timeout: Duration,
}

impl SignalingState {
    /// Create a new signaling state manager.
    pub fn new(timeout: Duration) -> Self {
        Self {
            sessions: HashMap::new(),
            peer_sessions: HashMap::new(),
            timeout,
        }
    }

    /// Create a new session.
    pub fn create_session(
        &mut self,
        session_id: SessionId,
        peer: PeerId,
        role: SignalingRole,
    ) -> Result<&mut SignalingSession, SignalingError> {
        // Check if session already exists
        if self.sessions.contains_key(&session_id) {
            return Err(SignalingError::SessionExists(session_id));
        }

        // Check per-peer limit
        let peer_sessions = self.peer_sessions.entry(peer).or_default();
        if peer_sessions.len() >= MAX_SESSIONS_PER_PEER {
            return Err(SignalingError::TooManySessions);
        }

        // Create the session
        let session = SignalingSession::new(peer, role);
        self.sessions.insert(session_id, session);
        peer_sessions.push(session_id);

        Ok(self.sessions.get_mut(&session_id).unwrap())
    }

    /// Get a session by ID.
    pub fn get_session(&self, session_id: &SessionId) -> Option<&SignalingSession> {
        self.sessions.get(session_id)
    }

    /// Get a mutable session by ID.
    pub fn get_session_mut(&mut self, session_id: &SessionId) -> Option<&mut SignalingSession> {
        self.sessions.get_mut(session_id)
    }

    /// Remove a session.
    pub fn remove_session(&mut self, session_id: &SessionId) -> Option<SignalingSession> {
        if let Some(session) = self.sessions.remove(session_id) {
            // Remove from peer index
            if let Some(peer_sessions) = self.peer_sessions.get_mut(&session.peer) {
                peer_sessions.retain(|id| id != session_id);
                if peer_sessions.is_empty() {
                    self.peer_sessions.remove(&session.peer);
                }
            }
            Some(session)
        } else {
            None
        }
    }

    /// Clean up expired sessions.
    ///
    /// Returns the number of sessions cleaned up.
    pub fn cleanup_expired(&mut self) -> usize {
        let timeout = self.timeout;
        let expired: Vec<SessionId> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.is_expired(timeout))
            .map(|(id, _)| *id)
            .collect();

        let count = expired.len();
        for session_id in expired {
            self.remove_session(&session_id);
        }
        count
    }

    /// Get the number of active sessions.
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Get the number of sessions for a specific peer.
    pub fn peer_session_count(&self, peer: &PeerId) -> usize {
        self.peer_sessions.get(peer).map_or(0, |s| s.len())
    }
}

impl Default for SignalingState {
    fn default() -> Self {
        Self::new(Duration::from_secs(DEFAULT_SIGNALING_TIMEOUT_SECS))
    }
}

/// Signaling channel over an existing connection.
///
/// This struct wraps an async read/write stream and provides framed
/// message exchange for signaling.
pub struct SignalingChannel<S> {
    /// Framed stream for length-delimited message exchange.
    framed: Framed<S, LengthDelimitedCodec>,
    /// Default timeout for operations.
    timeout: Duration,
}

impl<S> SignalingChannel<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    /// Create a new signaling channel over the given stream.
    pub fn new(stream: S) -> Self {
        Self::with_timeout(stream, Duration::from_secs(DEFAULT_SIGNALING_TIMEOUT_SECS))
    }

    /// Create a new signaling channel with a custom timeout.
    pub fn with_timeout(stream: S, timeout: Duration) -> Self {
        let mut codec = LengthDelimitedCodec::new();
        codec.set_max_frame_length(MAX_SDP_SIZE + 1024); // Allow for framing overhead
        Self {
            framed: Framed::new(stream, codec),
            timeout,
        }
    }

    /// Send a signaling message.
    pub async fn send(&mut self, msg: SignalingMessage) -> Result<(), SignalingError> {
        msg.validate()?;
        let bytes = msg.to_bytes()?;
        self.framed
            .send(Bytes::from(bytes))
            .await
            .map_err(SignalingError::Io)
    }

    /// Receive a signaling message with timeout.
    pub async fn recv(&mut self) -> Result<SignalingMessage, SignalingError> {
        self.recv_with_timeout(self.timeout).await
    }

    /// Receive a signaling message with a custom timeout.
    pub async fn recv_with_timeout(
        &mut self,
        recv_timeout: Duration,
    ) -> Result<SignalingMessage, SignalingError> {
        let result = timeout(recv_timeout, self.framed.next()).await;

        match result {
            Ok(Some(Ok(bytes))) => SignalingMessage::from_bytes(&bytes),
            Ok(Some(Err(e))) => Err(SignalingError::Io(e)),
            Ok(None) => Err(SignalingError::ChannelClosed),
            Err(_) => Err(SignalingError::Timeout),
        }
    }

    /// Complete an SDP offer/answer exchange.
    ///
    /// If `is_offerer` is true, sends the local SDP as an offer and waits for an answer.
    /// If `is_offerer` is false, waits for an offer and sends the local SDP as an answer.
    ///
    /// Returns the remote SDP on success.
    pub async fn exchange_sdp(
        &mut self,
        local_sdp: String,
        session_id: SessionId,
        is_offerer: bool,
    ) -> Result<String, SignalingError> {
        if is_offerer {
            // Send offer
            self.send(SignalingMessage::Offer {
                sdp: local_sdp,
                session_id,
            })
            .await?;

            // Wait for answer
            let response = self.recv().await?;
            match response {
                SignalingMessage::Answer { sdp, session_id: resp_id } => {
                    if resp_id != session_id {
                        return Err(SignalingError::Protocol(format!(
                            "Session ID mismatch: expected {}, got {}",
                            session_id, resp_id
                        )));
                    }
                    Ok(sdp)
                }
                SignalingMessage::Reject { reason, .. } => Err(SignalingError::Rejected(reason)),
                _ => Err(SignalingError::Protocol(
                    "Expected Answer or Reject message".to_string(),
                )),
            }
        } else {
            // Wait for offer
            let offer = self.recv().await?;
            let (remote_sdp, offer_session_id) = match offer {
                SignalingMessage::Offer { sdp, session_id: offer_id } => (sdp, offer_id),
                _ => {
                    return Err(SignalingError::Protocol(
                        "Expected Offer message".to_string(),
                    ))
                }
            };

            // Verify session ID if provided
            if offer_session_id != session_id {
                // Use the session ID from the offer
                // This allows the answerer to adopt the offerer's session ID
            }

            // Send answer
            self.send(SignalingMessage::Answer {
                sdp: local_sdp,
                session_id: offer_session_id,
            })
            .await?;

            Ok(remote_sdp)
        }
    }

    /// Send an ICE candidate.
    pub async fn send_ice_candidate(
        &mut self,
        session_id: SessionId,
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u16>,
    ) -> Result<(), SignalingError> {
        self.send(SignalingMessage::IceCandidate {
            candidate,
            sdp_mid,
            sdp_mline_index,
            session_id,
        })
        .await
    }

    /// Send a rejection message.
    pub async fn reject(
        &mut self,
        session_id: SessionId,
        reason: String,
    ) -> Result<(), SignalingError> {
        self.send(SignalingMessage::Reject { session_id, reason })
            .await
    }

    /// Get the inner stream back.
    pub fn into_inner(self) -> S {
        self.framed.into_inner()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    fn sample_sdp() -> String {
        "v=0\r\n\
         o=- 0 0 IN IP4 127.0.0.1\r\n\
         s=-\r\n\
         t=0 0\r\n\
         a=group:BUNDLE 0\r\n\
         m=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n\
         c=IN IP4 0.0.0.0\r\n\
         a=ice-ufrag:test\r\n\
         a=ice-pwd:testpassword1234567890\r\n\
         a=fingerprint:sha-256 00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00\r\n\
         a=setup:actpass\r\n"
            .to_string()
    }

    #[test]
    fn test_session_id_random_uniqueness() {
        let mut rng = rand::thread_rng();
        let id1 = SessionId::random(&mut rng);
        let id2 = SessionId::random(&mut rng);
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_session_id_from_bytes() {
        let bytes = [0x42u8; SESSION_ID_LEN];
        let id = SessionId::from_bytes(&bytes).unwrap();
        assert_eq!(id.as_bytes(), &bytes);
    }

    #[test]
    fn test_session_id_from_bytes_wrong_length() {
        let bytes = [0x42u8; 8];
        assert!(SessionId::from_bytes(&bytes).is_none());
    }

    #[test]
    fn test_session_id_debug_format() {
        let id = SessionId([0xDE, 0xAD, 0xBE, 0xEF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let debug = format!("{:?}", id);
        assert!(debug.contains("deadbeef"));
    }

    #[test]
    fn test_signaling_message_offer_serialization() {
        let mut rng = rand::thread_rng();
        let session_id = SessionId::random(&mut rng);
        let msg = SignalingMessage::Offer {
            sdp: sample_sdp(),
            session_id,
        };

        let bytes = msg.to_bytes().unwrap();
        let decoded = SignalingMessage::from_bytes(&bytes).unwrap();

        assert_eq!(msg, decoded);
    }

    #[test]
    fn test_signaling_message_answer_serialization() {
        let mut rng = rand::thread_rng();
        let session_id = SessionId::random(&mut rng);
        let msg = SignalingMessage::Answer {
            sdp: sample_sdp(),
            session_id,
        };

        let bytes = msg.to_bytes().unwrap();
        let decoded = SignalingMessage::from_bytes(&bytes).unwrap();

        assert_eq!(msg, decoded);
    }

    #[test]
    fn test_signaling_message_ice_candidate_serialization() {
        let mut rng = rand::thread_rng();
        let session_id = SessionId::random(&mut rng);
        let msg = SignalingMessage::IceCandidate {
            candidate: "candidate:1 1 UDP 2130706431 192.168.1.1 54400 typ host".to_string(),
            sdp_mid: Some("0".to_string()),
            sdp_mline_index: Some(0),
            session_id,
        };

        let bytes = msg.to_bytes().unwrap();
        let decoded = SignalingMessage::from_bytes(&bytes).unwrap();

        assert_eq!(msg, decoded);
    }

    #[test]
    fn test_signaling_message_reject_serialization() {
        let mut rng = rand::thread_rng();
        let session_id = SessionId::random(&mut rng);
        let msg = SignalingMessage::Reject {
            session_id,
            reason: "Connection refused".to_string(),
        };

        let bytes = msg.to_bytes().unwrap();
        let decoded = SignalingMessage::from_bytes(&bytes).unwrap();

        assert_eq!(msg, decoded);
    }

    #[test]
    fn test_signaling_message_validation_valid_sdp() {
        let mut rng = rand::thread_rng();
        let msg = SignalingMessage::Offer {
            sdp: sample_sdp(),
            session_id: SessionId::random(&mut rng),
        };
        assert!(msg.validate().is_ok());
    }

    #[test]
    fn test_signaling_message_validation_invalid_sdp_prefix() {
        let mut rng = rand::thread_rng();
        let msg = SignalingMessage::Offer {
            sdp: "invalid sdp content".to_string(),
            session_id: SessionId::random(&mut rng),
        };
        assert!(matches!(msg.validate(), Err(SignalingError::InvalidSdp(_))));
    }

    #[test]
    fn test_signaling_message_validation_oversized_sdp() {
        let mut rng = rand::thread_rng();
        let msg = SignalingMessage::Offer {
            sdp: format!("v={}", "x".repeat(MAX_SDP_SIZE + 1)),
            session_id: SessionId::random(&mut rng),
        };
        assert!(matches!(msg.validate(), Err(SignalingError::InvalidSdp(_))));
    }

    #[test]
    fn test_signaling_state_create_session() {
        let mut state = SignalingState::default();
        let mut rng = rand::thread_rng();
        let session_id = SessionId::random(&mut rng);
        let peer = PeerId::random();

        let result = state.create_session(session_id, peer, SignalingRole::Offerer);
        assert!(result.is_ok());
        assert_eq!(state.session_count(), 1);
    }

    #[test]
    fn test_signaling_state_duplicate_session() {
        let mut state = SignalingState::default();
        let mut rng = rand::thread_rng();
        let session_id = SessionId::random(&mut rng);
        let peer = PeerId::random();

        state
            .create_session(session_id, peer, SignalingRole::Offerer)
            .unwrap();
        let result = state.create_session(session_id, peer, SignalingRole::Answerer);
        assert!(matches!(result, Err(SignalingError::SessionExists(_))));
    }

    #[test]
    fn test_signaling_state_too_many_sessions() {
        let mut state = SignalingState::default();
        let mut rng = rand::thread_rng();
        let peer = PeerId::random();

        // Create max sessions
        for _ in 0..MAX_SESSIONS_PER_PEER {
            let session_id = SessionId::random(&mut rng);
            state
                .create_session(session_id, peer, SignalingRole::Offerer)
                .unwrap();
        }

        // Try to create one more
        let session_id = SessionId::random(&mut rng);
        let result = state.create_session(session_id, peer, SignalingRole::Offerer);
        assert!(matches!(result, Err(SignalingError::TooManySessions)));
    }

    #[test]
    fn test_signaling_state_remove_session() {
        let mut state = SignalingState::default();
        let mut rng = rand::thread_rng();
        let session_id = SessionId::random(&mut rng);
        let peer = PeerId::random();

        state
            .create_session(session_id, peer, SignalingRole::Offerer)
            .unwrap();
        assert_eq!(state.session_count(), 1);

        let removed = state.remove_session(&session_id);
        assert!(removed.is_some());
        assert_eq!(state.session_count(), 0);
    }

    #[test]
    fn test_signaling_state_cleanup_expired() {
        let mut state = SignalingState::new(Duration::from_millis(1));
        let mut rng = rand::thread_rng();
        let session_id = SessionId::random(&mut rng);
        let peer = PeerId::random();

        state
            .create_session(session_id, peer, SignalingRole::Offerer)
            .unwrap();

        // Wait for expiration
        std::thread::sleep(Duration::from_millis(10));

        let cleaned = state.cleanup_expired();
        assert_eq!(cleaned, 1);
        assert_eq!(state.session_count(), 0);
    }

    #[test]
    fn test_signaling_session_add_ice_candidates() {
        let peer = PeerId::random();
        let mut session = SignalingSession::new(peer, SignalingRole::Offerer);

        for i in 0..MAX_ICE_CANDIDATES_PER_SESSION {
            let candidate = IceCandidate {
                candidate: format!("candidate:{}", i),
                sdp_mid: Some("0".to_string()),
                sdp_mline_index: Some(0),
            };
            assert!(session.add_ice_candidate(candidate).is_ok());
        }

        // One more should fail
        let candidate = IceCandidate {
            candidate: "overflow".to_string(),
            sdp_mid: None,
            sdp_mline_index: None,
        };
        assert!(matches!(
            session.add_ice_candidate(candidate),
            Err(SignalingError::TooManyIceCandidates)
        ));
    }

    #[tokio::test]
    async fn test_signaling_channel_send_recv() {
        let (client, server) = duplex(1024 * 1024);
        let mut client_channel = SignalingChannel::new(client);
        let mut server_channel = SignalingChannel::new(server);

        let mut rng = rand::thread_rng();
        let session_id = SessionId::random(&mut rng);
        let msg = SignalingMessage::Offer {
            sdp: sample_sdp(),
            session_id,
        };

        // Spawn sender
        let send_msg = msg.clone();
        let send_task = tokio::spawn(async move {
            client_channel.send(send_msg).await.unwrap();
        });

        // Receive
        let received = server_channel.recv().await.unwrap();
        send_task.await.unwrap();

        assert_eq!(msg, received);
    }

    #[tokio::test]
    async fn test_signaling_channel_exchange_sdp_as_offerer() {
        let (client, server) = duplex(1024 * 1024);
        let mut offerer = SignalingChannel::new(client);
        let mut answerer = SignalingChannel::new(server);

        let mut rng = rand::thread_rng();
        let session_id = SessionId::random(&mut rng);
        let offer_sdp = sample_sdp();
        let answer_sdp = sample_sdp();

        let offer_sdp_clone = offer_sdp.clone();
        let answer_sdp_clone = answer_sdp.clone();

        // Spawn offerer
        let offerer_task = tokio::spawn(async move {
            offerer.exchange_sdp(offer_sdp_clone, session_id, true).await
        });

        // Run answerer
        let answerer_task = tokio::spawn(async move {
            answerer
                .exchange_sdp(answer_sdp_clone, session_id, false)
                .await
        });

        let (offerer_result, answerer_result) = tokio::join!(offerer_task, answerer_task);

        let remote_answer = offerer_result.unwrap().unwrap();
        let remote_offer = answerer_result.unwrap().unwrap();

        assert_eq!(remote_answer, answer_sdp);
        assert_eq!(remote_offer, offer_sdp);
    }

    #[tokio::test]
    async fn test_signaling_channel_timeout() {
        let (client, _server) = duplex(1024);
        let mut channel = SignalingChannel::with_timeout(client, Duration::from_millis(10));

        let result = channel.recv().await;
        assert!(matches!(result, Err(SignalingError::Timeout)));
    }

    #[tokio::test]
    async fn test_signaling_channel_reject() {
        let (client, server) = duplex(1024 * 1024);
        let mut sender = SignalingChannel::new(client);
        let mut receiver = SignalingChannel::new(server);

        let mut rng = rand::thread_rng();
        let session_id = SessionId::random(&mut rng);

        // Spawn sender
        let send_task = tokio::spawn(async move {
            sender
                .reject(session_id, "Test rejection".to_string())
                .await
                .unwrap();
        });

        // Receive
        let received = receiver.recv().await.unwrap();
        send_task.await.unwrap();

        match received {
            SignalingMessage::Reject { reason, .. } => {
                assert_eq!(reason, "Test rejection");
            }
            _ => panic!("Expected Reject message"),
        }
    }

    #[tokio::test]
    async fn test_signaling_channel_ice_candidate() {
        let (client, server) = duplex(1024 * 1024);
        let mut sender = SignalingChannel::new(client);
        let mut receiver = SignalingChannel::new(server);

        let mut rng = rand::thread_rng();
        let session_id = SessionId::random(&mut rng);

        // Spawn sender
        let send_task = tokio::spawn(async move {
            sender
                .send_ice_candidate(
                    session_id,
                    "candidate:1 1 UDP 2130706431 192.168.1.1 54400 typ host".to_string(),
                    Some("0".to_string()),
                    Some(0),
                )
                .await
                .unwrap();
        });

        // Receive
        let received = receiver.recv().await.unwrap();
        send_task.await.unwrap();

        match received {
            SignalingMessage::IceCandidate {
                candidate,
                sdp_mid,
                sdp_mline_index,
                ..
            } => {
                assert!(candidate.contains("candidate:1"));
                assert_eq!(sdp_mid, Some("0".to_string()));
                assert_eq!(sdp_mline_index, Some(0));
            }
            _ => panic!("Expected IceCandidate message"),
        }
    }
}
