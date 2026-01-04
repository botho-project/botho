// Copyright (c) 2024 Botho Foundation

//! Pluggable transport layer for protocol obfuscation.
//!
//! This module implements Phase 3.1 of the traffic privacy roadmap:
//! a pluggable transport interface that allows different transport
//! implementations to be used interchangeably.
//!
//! # Overview
//!
//! The transport layer provides an abstraction over the raw network
//! connection, allowing botho to use different protocols that are
//! harder to detect and block:
//!
//! - **Plain**: Standard TCP + Noise (default, best performance)
//! - **WebRTC**: Looks like video call traffic (Phase 3.2)
//! - **TLS Tunnel**: Looks like HTTPS traffic (Phase 3.7)
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    APPLICATION LAYER                        │
//! │                    (Gossipsub, SCP)                         │
//! └──────────────────────────┬──────────────────────────────────┘
//!                            │
//! ┌──────────────────────────▼──────────────────────────────────┐
//! │                  TRANSPORT LAYER                            │
//! │  ┌─────────────────────────────────────────────────────┐    │
//! │  │            PluggableTransport Trait                 │    │
//! │  └─────────────────────────────────────────────────────┘    │
//! │         │                    │                    │         │
//! │  ┌──────▼──────┐     ┌───────▼───────┐    ┌───────▼──────┐  │
//! │  │    Plain    │     │    WebRTC     │    │  TLS Tunnel  │  │
//! │  │ TCP + Noise │     │ DTLS + SCTP   │    │   TLS 1.3    │  │
//! │  └─────────────┘     └───────────────┘    └──────────────┘  │
//! └──────────────────────────┬──────────────────────────────────┘
//!                            │
//! ┌──────────────────────────▼──────────────────────────────────┐
//! │                    NETWORK LAYER                            │
//! │                    (TCP, UDP)                               │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Usage
//!
//! ```
//! use botho::network::transport::{
//!     PlainTransport, PluggableTransport, TransportType,
//! };
//!
//! // Create the default transport
//! let transport = PlainTransport::new();
//! assert_eq!(transport.transport_type(), TransportType::Plain);
//! assert_eq!(transport.name(), "plain");
//!
//! // Check transport properties
//! assert!(!transport.transport_type().is_obfuscated());
//! assert!(transport.is_available());
//! ```
//!
//! # Transport Selection
//!
//! Transport selection is based on:
//! 1. User preference (configured privacy level)
//! 2. Peer capabilities (what transports both sides support)
//! 3. Network conditions (NAT type, firewall rules)
//!
//! See the `TransportManager` (Phase 3.8) for automatic selection.
//!
//! # Security Considerations
//!
//! - All transports provide encryption (Noise, DTLS, or TLS)
//! - Obfuscated transports (WebRTC, TLS) resist DPI detection
//! - Transport negotiation is authenticated to prevent downgrade attacks
//!
//! # References
//!
//! - Design document: `docs/design/traffic-privacy-roadmap.md` (Phase 3)
//! - Parent issue: #201 (Phase 3: Protocol Obfuscation)
//! - Implementation issue: #202 (Pluggable transport interface)

mod error;
mod plain;
mod traits;
mod types;

// Re-export error types
pub use error::TransportError;

// Re-export transport types
pub use types::{TransportType, TransportTypeParseError};

// Re-export trait and connection types
pub use traits::{BoxedConnection, ConnectionWrapper, PluggableTransport, TransportConnection};

// Re-export transport implementations
pub use plain::{PlainConnection, PlainTransport};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_exports() {
        // Verify all expected types are exported and usable
        let _: TransportType = TransportType::Plain;
        let _: PlainTransport = PlainTransport::new();

        // Verify trait is usable
        fn assert_transport<T: PluggableTransport>(_: &T) {}
        let transport = PlainTransport::new();
        assert_transport(&transport);
    }

    #[test]
    fn test_plain_transport_is_default() {
        let transport = PlainTransport::default();
        assert_eq!(transport.transport_type(), TransportType::Plain);
    }
}
