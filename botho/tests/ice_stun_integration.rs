// Copyright (c) 2024 Botho Foundation

//! Integration tests for ICE/STUN NAT traversal functionality.
//!
//! These tests verify:
//! - ICE configuration and candidate handling
//! - STUN message encoding/decoding
//! - NAT type detection logic
//! - Transport selection and negotiation

use std::time::Duration;

use botho::network::transport::{
    IceCandidate, IceCandidateType, IceConfig, IceConnectionState, NatType, StunConfig,
    TransportConfig, TransportType, WebRtcTransport,
};

/// Test that ICE configuration defaults are sensible.
#[test]
fn test_ice_config_defaults() {
    let config = IceConfig::default();

    // Should have default STUN servers
    assert!(!config.stun_servers.is_empty());
    assert!(config.stun_servers.len() >= 2);

    // Should use common public STUN servers
    assert!(config
        .stun_servers
        .iter()
        .any(|s| s.contains("google.com") || s.contains("cloudflare.com")));

    // Timeouts should be reasonable
    assert!(config.gathering_timeout >= Duration::from_secs(5));
    assert!(config.connection_timeout >= Duration::from_secs(15));

    // Trickle ICE should be enabled by default
    assert!(config.trickle_ice);
}

/// Test ICE configuration builder pattern.
#[test]
fn test_ice_config_builder() {
    let config = IceConfig::with_stun_servers(vec!["stun:custom.example.com:3478".to_string()])
        .with_gathering_timeout(Duration::from_secs(5))
        .with_connection_timeout(Duration::from_secs(20))
        .with_turn_server("turn:relay.example.com:3478", "user", "secret");

    assert_eq!(config.stun_servers.len(), 1);
    assert_eq!(config.turn_servers.len(), 1);
    assert_eq!(config.gathering_timeout, Duration::from_secs(5));
    assert_eq!(config.connection_timeout, Duration::from_secs(20));

    let turn = &config.turn_servers[0];
    assert_eq!(turn.username, "user");
    assert_eq!(turn.credential, "secret");
}

/// Test ICE candidate priority calculation per RFC 8445.
#[test]
fn test_ice_candidate_priority() {
    // Host candidates should have highest priority
    let host_priority =
        IceCandidate::calculate_priority(IceCandidateType::Host, 1, 65535);

    // Server reflexive candidates have lower priority
    let srflx_priority =
        IceCandidate::calculate_priority(IceCandidateType::ServerReflexive, 1, 65535);

    // Relay candidates have lowest priority
    let relay_priority =
        IceCandidate::calculate_priority(IceCandidateType::Relay, 1, 65535);

    assert!(host_priority > srflx_priority);
    assert!(srflx_priority > relay_priority);

    // Component 1 should have higher priority than component 2
    let comp1_priority =
        IceCandidate::calculate_priority(IceCandidateType::Host, 1, 65535);
    let comp2_priority =
        IceCandidate::calculate_priority(IceCandidateType::Host, 2, 65535);
    assert!(comp1_priority > comp2_priority);
}

/// Test SDP candidate attribute parsing and generation.
#[test]
fn test_ice_candidate_sdp_roundtrip() {
    let original = IceCandidate::new(
        IceCandidateType::Host,
        "udp",
        "192.168.1.100",
        54321,
        2130706431,
    );

    let sdp = original.to_sdp_attribute();
    let parsed = IceCandidate::from_sdp_attribute(&sdp).unwrap();

    assert_eq!(parsed.candidate_type, original.candidate_type);
    assert_eq!(parsed.protocol, original.protocol);
    assert_eq!(parsed.address, original.address);
    assert_eq!(parsed.port, original.port);
    assert_eq!(parsed.priority, original.priority);
    assert_eq!(parsed.component, original.component);
}

/// Test parsing SDP candidates with related addresses (for srflx/prflx).
#[test]
fn test_ice_candidate_sdp_with_related() {
    let sdp = "candidate:abc123 1 udp 1694498815 203.0.113.50 54321 typ srflx raddr 192.168.1.100 rport 12345";
    let parsed = IceCandidate::from_sdp_attribute(sdp).unwrap();

    assert_eq!(parsed.candidate_type, IceCandidateType::ServerReflexive);
    assert_eq!(parsed.address, "203.0.113.50");
    assert_eq!(parsed.port, 54321);
    assert_eq!(parsed.related_address, Some("192.168.1.100".to_string()));
    assert_eq!(parsed.related_port, Some(12345));
}

/// Test parsing invalid SDP candidates fails gracefully.
#[test]
fn test_ice_candidate_invalid_sdp() {
    // Too short
    assert!(IceCandidate::from_sdp_attribute("candidate:abc").is_err());

    // Invalid type
    assert!(
        IceCandidate::from_sdp_attribute("candidate:abc 1 udp 100 1.2.3.4 5000 typ invalid")
            .is_err()
    );

    // Invalid port
    assert!(
        IceCandidate::from_sdp_attribute("candidate:abc 1 udp 100 1.2.3.4 notaport typ host")
            .is_err()
    );
}

/// Test NAT type classification and relay score.
#[test]
fn test_nat_type_relay_scores() {
    // Open NAT should have highest relay score
    let open_score = NatType::Open.relay_score_modifier();
    let full_cone_score = NatType::FullCone.relay_score_modifier();
    let restricted_score = NatType::Restricted.relay_score_modifier();
    let port_restricted_score = NatType::PortRestricted.relay_score_modifier();
    let symmetric_score = NatType::Symmetric.relay_score_modifier();

    assert!(open_score > full_cone_score);
    assert!(full_cone_score > restricted_score);
    assert!(restricted_score > port_restricted_score);
    assert!(port_restricted_score > symmetric_score);

    // Score should be in valid range [0, 1]
    for nat_type in [
        NatType::Open,
        NatType::FullCone,
        NatType::Restricted,
        NatType::PortRestricted,
        NatType::Symmetric,
        NatType::Unknown,
    ] {
        let score = nat_type.relay_score_modifier();
        assert!(score >= 0.0 && score <= 1.0);
    }
}

/// Test NAT type inbound connection support.
#[test]
fn test_nat_type_inbound_support() {
    // Only Open and FullCone NATs can accept unsolicited inbound
    assert!(NatType::Open.supports_inbound());
    assert!(NatType::FullCone.supports_inbound());

    // Other NAT types cannot accept unsolicited inbound
    assert!(!NatType::Restricted.supports_inbound());
    assert!(!NatType::PortRestricted.supports_inbound());
    assert!(!NatType::Symmetric.supports_inbound());
    assert!(!NatType::Unknown.supports_inbound());
}

/// Test STUN configuration defaults.
#[test]
fn test_stun_config_defaults() {
    let config = StunConfig::default();

    // Should have multiple STUN servers for redundancy
    assert!(config.servers.len() >= 2);

    // Request timeout should be reasonable
    assert!(config.request_timeout >= Duration::from_secs(1));
    assert!(config.request_timeout <= Duration::from_secs(10));

    // Should have at least one retry
    assert!(config.retries >= 1);
}

/// Test transport configuration.
#[test]
fn test_transport_config_defaults() {
    let config = TransportConfig::default();

    // Plain transport should be the default
    assert_eq!(config.preferred, TransportType::Plain);

    // WebRTC should be disabled by default
    assert!(!config.enable_webrtc);

    // Should only have Plain enabled by default
    let enabled = config.enabled_transports();
    assert!(enabled.contains(&TransportType::Plain));
    assert!(!enabled.contains(&TransportType::WebRtc));
}

/// Test transport configuration with WebRTC enabled.
#[test]
fn test_transport_config_with_webrtc() {
    let config = TransportConfig::with_webrtc();

    assert_eq!(config.preferred, TransportType::WebRtc);
    assert!(config.enable_webrtc);

    let enabled = config.enabled_transports();
    assert!(enabled.contains(&TransportType::Plain));
    assert!(enabled.contains(&TransportType::WebRtc));
}

/// Test transport type protocol IDs.
#[test]
fn test_transport_type_protocol_ids() {
    assert!(TransportType::Plain.protocol_id().contains("plain"));
    assert!(TransportType::WebRtc.protocol_id().contains("webrtc"));
    assert!(TransportType::TlsTunnel.protocol_id().contains("tls"));

    // Protocol IDs should follow versioning pattern
    for transport in [
        TransportType::Plain,
        TransportType::WebRtc,
        TransportType::TlsTunnel,
    ] {
        let id = transport.protocol_id();
        assert!(id.starts_with("/botho/transport/"));
        assert!(id.contains("/1.0.0"));
    }
}

/// Test ICE connection state display.
#[test]
fn test_ice_connection_state_display() {
    assert_eq!(IceConnectionState::New.to_string(), "new");
    assert_eq!(IceConnectionState::Checking.to_string(), "checking");
    assert_eq!(IceConnectionState::Connected.to_string(), "connected");
    assert_eq!(IceConnectionState::Completed.to_string(), "completed");
    assert_eq!(IceConnectionState::Disconnected.to_string(), "disconnected");
    assert_eq!(IceConnectionState::Failed.to_string(), "failed");
    assert_eq!(IceConnectionState::Closed.to_string(), "closed");
}

/// Test WebRTC transport creation.
#[test]
fn test_webrtc_transport_creation() {
    let transport = WebRtcTransport::with_defaults();

    // Should have default ICE config
    let ice_config = transport.ice_config();
    assert!(!ice_config.stun_servers.is_empty());
}

/// Test WebRTC transport with custom configuration.
#[test]
fn test_webrtc_transport_custom_config() {
    let ice_config = IceConfig {
        stun_servers: vec!["stun:custom.stun.server:3478".to_string()],
        gathering_timeout: Duration::from_secs(5),
        ..Default::default()
    };
    let stun_config = StunConfig::with_servers(vec!["custom.stun.server:3478".to_string()]);

    let transport = WebRtcTransport::new(ice_config.clone(), stun_config);

    assert_eq!(transport.ice_config().stun_servers, ice_config.stun_servers);
    assert_eq!(
        transport.ice_config().gathering_timeout,
        Duration::from_secs(5)
    );
}

/// Test candidate type priority modifiers follow RFC 8445 ordering.
#[test]
fn test_candidate_type_priority_ordering() {
    let host = IceCandidateType::Host.priority_modifier();
    let prflx = IceCandidateType::PeerReflexive.priority_modifier();
    let srflx = IceCandidateType::ServerReflexive.priority_modifier();
    let relay = IceCandidateType::Relay.priority_modifier();

    // RFC 8445 Section 5.1.2.2 defines the ordering
    assert!(host > prflx, "host should be preferred over prflx");
    assert!(prflx > srflx, "prflx should be preferred over srflx");
    assert!(srflx > relay, "srflx should be preferred over relay");
}

/// Test ICE candidate foundation computation is deterministic.
#[test]
fn test_ice_candidate_foundation_deterministic() {
    let candidate1 = IceCandidate::new(
        IceCandidateType::Host,
        "udp",
        "192.168.1.100",
        54321,
        2130706431,
    );

    let candidate2 = IceCandidate::new(
        IceCandidateType::Host,
        "udp",
        "192.168.1.100",
        12345, // Different port
        1000,  // Different priority
    );

    // Same type and base address should produce same foundation
    assert_eq!(candidate1.foundation, candidate2.foundation);

    // Different address should produce different foundation
    let candidate3 = IceCandidate::new(
        IceCandidateType::Host,
        "udp",
        "192.168.1.200", // Different address
        54321,
        2130706431,
    );
    assert_ne!(candidate1.foundation, candidate3.foundation);
}

/// Integration test for transport configuration in privacy context.
#[test]
fn test_transport_config_privacy_integration() {
    // Standard privacy: Plain transport only
    let standard = TransportConfig::default();
    assert_eq!(standard.preferred, TransportType::Plain);
    assert!(!standard.enable_webrtc);

    // High privacy/censorship resistant: WebRTC enabled
    let high_privacy = TransportConfig::with_webrtc();
    assert_eq!(high_privacy.preferred, TransportType::WebRtc);
    assert!(high_privacy.enable_webrtc);

    // Verify ICE config is included with WebRTC
    assert!(!high_privacy.ice_config.stun_servers.is_empty());
}

/// Test that all ICE candidate types can be parsed from strings.
#[test]
fn test_all_candidate_types_parse() {
    assert_eq!(
        IceCandidateType::from_str("host"),
        Some(IceCandidateType::Host)
    );
    assert_eq!(
        IceCandidateType::from_str("srflx"),
        Some(IceCandidateType::ServerReflexive)
    );
    assert_eq!(
        IceCandidateType::from_str("prflx"),
        Some(IceCandidateType::PeerReflexive)
    );
    assert_eq!(
        IceCandidateType::from_str("relay"),
        Some(IceCandidateType::Relay)
    );

    // Case insensitive
    assert_eq!(
        IceCandidateType::from_str("HOST"),
        Some(IceCandidateType::Host)
    );
    assert_eq!(
        IceCandidateType::from_str("Srflx"),
        Some(IceCandidateType::ServerReflexive)
    );

    // Invalid returns None
    assert_eq!(IceCandidateType::from_str("invalid"), None);
    assert_eq!(IceCandidateType::from_str(""), None);
}
