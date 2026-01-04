// Copyright (c) 2024 Botho Foundation

//! Pluggable transport layer for protocol obfuscation.
//!
//! This module implements Phase 3 of the traffic analysis resistance roadmap:
//! Protocol Obfuscation. The goal is to make botho traffic indistinguishable
//! from common protocols like video calls.
//!
//! # Overview
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    TRANSPORT LAYER                          │
//! │                                                             │
//! │   ┌───────────────┐        ┌───────────────┐               │
//! │   │   Plain TCP   │        │    WebRTC     │               │
//! │   │    + Noise    │        │ Data Channels │               │
//! │   └───────┬───────┘        └───────┬───────┘               │
//! │           │                        │                        │
//! │           │    ┌───────────────┐   │                        │
//! │           └────►  Signaling   ◄────┘                        │
//! │                │   Channel    │                             │
//! │                └───────────────┘                            │
//! │                                                             │
//! │   DPI sees: "Custom P2P"    DPI sees: "Video call"         │
//! │                                                             │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Modules
//!
//! - [`signaling`]: SDP exchange for WebRTC connection establishment
//!
//! # References
//!
//! - Design: `docs/design/traffic-privacy-roadmap.md` (Section 3)

pub mod signaling;

// Re-export signaling types
pub use signaling::{
    IceCandidate, SessionId, SignalingChannel, SignalingError, SignalingMessage, SignalingRole,
    SignalingSession, SignalingState, DEFAULT_SIGNALING_TIMEOUT_SECS, MAX_ICE_CANDIDATES_PER_SESSION,
    MAX_ICE_CANDIDATE_SIZE, MAX_SDP_SIZE, MAX_SESSIONS_PER_PEER, SESSION_ID_LEN,
};
