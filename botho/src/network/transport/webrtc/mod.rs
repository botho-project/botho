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
//! │   DTLS 1.3      │  ◄── This module
//! └────────┬────────┘
//!          │
//! ┌────────▼────────┐
//! │    ICE/UDP      │  ◄── NAT traversal
//! └─────────────────┘
//! ```
//!
//! # Modules
//!
//! - [`dtls`]: DTLS configuration and certificate handling
//!
//! # References
//!
//! - Design: `docs/design/traffic-privacy-roadmap.md` (Phase 3)
//! - WebRTC: <https://webrtc.org/>

pub mod dtls;

pub use dtls::{
    CertificateFingerprint, DtlsConfig, DtlsError, DtlsRole, DtlsState, DtlsVerification,
    EphemeralCertificate, BROWSER_CIPHER_SUITES, DEFAULT_CERTIFICATE_LIFETIME,
    DEFAULT_FINGERPRINT_ALGORITHM,
};
