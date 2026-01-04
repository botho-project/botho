// Copyright (c) 2024 Botho Foundation

//! Pluggable transport layer for protocol obfuscation.
//!
//! This module implements Phase 3 of the traffic privacy roadmap: Protocol Obfuscation.
//! It provides pluggable transports that make botho traffic indistinguishable from
//! common protocols to prevent protocol-level blocking and deep packet inspection.
//!
//! # Overview
//!
//! Protocol obfuscation ensures that even if an adversary can observe our traffic,
//! they cannot distinguish it from legitimate applications. This is achieved by
//! wrapping our protocol in transports that mimic common protocols:
//!
//! - **WebRTC**: Makes traffic look like video calls (Google Meet, Discord, etc.)
//! - **TLS Tunnel**: Makes traffic look like HTTPS (future)
//! - **obfs4**: Randomized transport for censorship resistance (future)
//!
//! # Current Status
//!
//! - ✅ DTLS configuration and certificate handling (Phase 3.3)
//! - 🔲 WebRTC data channel transport (Phase 3.2)
//! - 🔲 ICE/STUN NAT traversal (Phase 3.4)
//! - 🔲 Signaling channel for SDP exchange (Phase 3.5)
//! - 🔲 Transport negotiation protocol (Phase 3.6)
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                    TRANSPORT LAYER                               │
//! ├─────────────────────────────────────────────────────────────────┤
//! │                                                                 │
//! │   ┌─────────────┐  ┌─────────────┐  ┌─────────────┐             │
//! │   │    Plain    │  │   WebRTC    │  │ TLS Tunnel  │   ...       │
//! │   │  (current)  │  │  (primary)  │  │  (future)   │             │
//! │   └──────┬──────┘  └──────┬──────┘  └──────┬──────┘             │
//! │          │                │                │                    │
//! │          └────────────────┼────────────────┘                    │
//! │                           │                                     │
//! │                    ┌──────▼──────┐                              │
//! │                    │  Transport  │                              │
//! │                    │   Manager   │                              │
//! │                    └─────────────┘                              │
//! │                                                                 │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Modules
//!
//! - [`webrtc`]: WebRTC-based transport (makes traffic look like video calls)
//!   - [`webrtc::dtls`]: DTLS configuration and certificate handling
//!
//! # References
//!
//! - Design: `docs/design/traffic-privacy-roadmap.md` (Phase 3)

pub mod webrtc;

// Re-export commonly used types
pub use webrtc::dtls::{
    CertificateFingerprint, DtlsConfig, DtlsError, DtlsRole, DtlsState, DtlsVerification,
    EphemeralCertificate,
};
