// Copyright (c) 2024 Botho Foundation

//! Integration tests for transport negotiation protocol.
//!
//! These tests verify the transport negotiation protocol works correctly
//! across different scenarios including:
//! - Full capability peers negotiating WebRTC
//! - Mixed capability peers finding common ground
//! - NAT-aware transport selection
//! - Fallback behavior when no common transport exists

use std::time::Duration;
use tokio::io::duplex;
use tokio::time::timeout;

use botho::network::{
    negotiate_transport_initiator, negotiate_transport_responder, select_transport, NatType,
    NegotiationError, NegotiationMessage, TransportCapabilities, TransportManager,
    TransportManagerConfig, TransportType,
};

/// Test helper to create a pair of connected duplex streams.
fn create_stream_pair() -> (tokio::io::DuplexStream, tokio::io::DuplexStream) {
    duplex(8192)
}

// ============================================================================
// End-to-end negotiation tests
// ============================================================================

#[tokio::test]
async fn test_e2e_negotiate_webrtc_both_full_caps() {
    let (mut client, mut server) = create_stream_pair();

    let client_caps = TransportCapabilities::full(NatType::Open);
    let server_caps = TransportCapabilities::full(NatType::FullCone);

    let client_task = tokio::spawn(async move {
        timeout(
            Duration::from_secs(5),
            negotiate_transport_initiator(&mut client, &client_caps),
        )
        .await
    });

    let server_task = tokio::spawn(async move {
        timeout(
            Duration::from_secs(5),
            negotiate_transport_responder(&mut server, &server_caps),
        )
        .await
    });

    let (client_result, server_result) = tokio::join!(client_task, server_task);

    let client_transport = client_result.unwrap().unwrap().unwrap();
    let server_transport = server_result.unwrap().unwrap().unwrap();

    // Both should agree on WebRTC since both have open NAT
    assert_eq!(client_transport, server_transport);
    assert_eq!(client_transport, TransportType::WebRTC);
}

#[tokio::test]
async fn test_e2e_negotiate_falls_back_to_plain() {
    let (mut client, mut server) = create_stream_pair();

    // Client only supports WebRTC
    let client_caps = TransportCapabilities::new(
        vec![TransportType::WebRTC, TransportType::Plain],
        TransportType::WebRTC,
        NatType::Open,
    );
    // Server only supports Plain
    let server_caps = TransportCapabilities::plain_only();

    let client_task = tokio::spawn(async move {
        timeout(
            Duration::from_secs(5),
            negotiate_transport_initiator(&mut client, &client_caps),
        )
        .await
    });

    let server_task = tokio::spawn(async move {
        timeout(
            Duration::from_secs(5),
            negotiate_transport_responder(&mut server, &server_caps),
        )
        .await
    });

    let (client_result, server_result) = tokio::join!(client_task, server_task);

    let client_transport = client_result.unwrap().unwrap().unwrap();
    let server_transport = server_result.unwrap().unwrap().unwrap();

    // Should fall back to Plain since that's the only common transport
    assert_eq!(client_transport, server_transport);
    assert_eq!(client_transport, TransportType::Plain);
}

#[tokio::test]
async fn test_e2e_negotiate_tls_when_nat_blocks_webrtc() {
    let (mut client, mut server) = create_stream_pair();

    // Both have symmetric NAT - WebRTC won't work
    let client_caps = TransportCapabilities::full(NatType::Symmetric);
    let server_caps = TransportCapabilities::full(NatType::Symmetric);

    let client_task = tokio::spawn(async move {
        timeout(
            Duration::from_secs(5),
            negotiate_transport_initiator(&mut client, &client_caps),
        )
        .await
    });

    let server_task = tokio::spawn(async move {
        timeout(
            Duration::from_secs(5),
            negotiate_transport_responder(&mut server, &server_caps),
        )
        .await
    });

    let (client_result, server_result) = tokio::join!(client_task, server_task);

    let client_transport = client_result.unwrap().unwrap().unwrap();
    let server_transport = server_result.unwrap().unwrap().unwrap();

    // Should pick TLS tunnel instead of WebRTC due to NAT issues
    assert_eq!(client_transport, server_transport);
    assert_eq!(client_transport, TransportType::TlsTunnel);
}

#[tokio::test]
async fn test_e2e_negotiate_no_common_transport() {
    let (mut client, mut server) = create_stream_pair();

    // Client only supports WebRTC
    let client_caps = TransportCapabilities::new(
        vec![TransportType::WebRTC],
        TransportType::WebRTC,
        NatType::Open,
    );
    // Server only supports TLS tunnel
    let server_caps = TransportCapabilities::new(
        vec![TransportType::TlsTunnel],
        TransportType::TlsTunnel,
        NatType::Open,
    );

    let client_task = tokio::spawn(async move {
        timeout(
            Duration::from_secs(5),
            negotiate_transport_initiator(&mut client, &client_caps),
        )
        .await
    });

    let server_task = tokio::spawn(async move {
        timeout(
            Duration::from_secs(5),
            negotiate_transport_responder(&mut server, &server_caps),
        )
        .await
    });

    let (client_result, server_result) = tokio::join!(client_task, server_task);

    // Client should get a rejection
    let client_err = client_result.unwrap().unwrap().unwrap_err();
    assert!(matches!(client_err, NegotiationError::Rejected { .. }));

    // Server should return NoCommonTransport
    let server_err = server_result.unwrap().unwrap().unwrap_err();
    assert!(matches!(server_err, NegotiationError::NoCommonTransport));
}

// ============================================================================
// Transport selection algorithm tests
// ============================================================================

#[test]
fn test_select_transport_prefers_higher_score() {
    // Both support all transports with open NAT
    let our_caps = TransportCapabilities::full(NatType::Open);
    let peer_caps = TransportCapabilities::full(NatType::Open);

    let selected = select_transport(&our_caps, &peer_caps);
    assert_eq!(selected, TransportType::WebRTC); // Highest preference score
}

#[test]
fn test_select_transport_considers_preference_order() {
    // We prefer TLS, peer prefers WebRTC
    let our_caps = TransportCapabilities::new(
        vec![TransportType::TlsTunnel, TransportType::WebRTC, TransportType::Plain],
        TransportType::TlsTunnel,
        NatType::Open,
    );
    let peer_caps = TransportCapabilities::new(
        vec![TransportType::WebRTC, TransportType::TlsTunnel, TransportType::Plain],
        TransportType::WebRTC,
        NatType::Open,
    );

    let selected = select_transport(&our_caps, &peer_caps);
    // WebRTC has higher base score, so it should win
    assert_eq!(selected, TransportType::WebRTC);
}

#[test]
fn test_select_transport_nat_penalty_applied() {
    // Both have symmetric NAT
    let our_caps = TransportCapabilities::full(NatType::Symmetric);
    let peer_caps = TransportCapabilities::full(NatType::Symmetric);

    let selected = select_transport(&our_caps, &peer_caps);
    // WebRTC should be penalized, TLS tunnel should be selected
    assert_eq!(selected, TransportType::TlsTunnel);
}

#[test]
fn test_select_transport_one_open_nat_allows_webrtc() {
    // One side has open NAT, other has symmetric
    let our_caps = TransportCapabilities::full(NatType::Open);
    let peer_caps = TransportCapabilities::full(NatType::Symmetric);

    let selected = select_transport(&our_caps, &peer_caps);
    // WebRTC should work since at least one side has open NAT
    assert_eq!(selected, TransportType::WebRTC);
}

#[test]
fn test_select_transport_fallback_to_plain() {
    // No common transport except plain (implicitly always available)
    let our_caps = TransportCapabilities::plain_only();
    let peer_caps = TransportCapabilities::full(NatType::Open);

    let selected = select_transport(&our_caps, &peer_caps);
    assert_eq!(selected, TransportType::Plain);
}

// ============================================================================
// Transport manager tests
// ============================================================================

#[test]
fn test_transport_manager_should_upgrade_from_plain() {
    let caps = TransportCapabilities::full(NatType::Open);
    let manager = TransportManager::new(caps);

    let peer_caps = TransportCapabilities::full(NatType::Open);

    // Should upgrade from plain to WebRTC
    assert!(manager.should_upgrade(TransportType::Plain, &peer_caps));
}

#[test]
fn test_transport_manager_should_not_upgrade_if_already_best() {
    let caps = TransportCapabilities::full(NatType::Open);
    let manager = TransportManager::new(caps);

    let peer_caps = TransportCapabilities::full(NatType::Open);

    // Already on WebRTC, no need to upgrade
    assert!(!manager.should_upgrade(TransportType::WebRTC, &peer_caps));
}

#[test]
fn test_transport_manager_respects_disabled_upgrades() {
    let caps = TransportCapabilities::full(NatType::Open);
    let config = TransportManagerConfig {
        enable_upgrades: false,
        ..Default::default()
    };
    let manager = TransportManager::with_config(caps, config);

    let peer_caps = TransportCapabilities::full(NatType::Open);

    // Upgrades disabled, should not suggest upgrade
    assert!(!manager.should_upgrade(TransportType::Plain, &peer_caps));
}

#[tokio::test]
async fn test_transport_manager_negotiate_as_initiator() {
    let (mut client, mut server) = create_stream_pair();

    let client_caps = TransportCapabilities::full(NatType::Open);
    let server_caps = TransportCapabilities::full(NatType::FullCone);

    let client_manager = TransportManager::new(client_caps);

    let client_task = tokio::spawn(async move {
        timeout(
            Duration::from_secs(5),
            client_manager.negotiate_upgrade(&mut client, true),
        )
        .await
    });

    let server_task = tokio::spawn(async move {
        timeout(
            Duration::from_secs(5),
            negotiate_transport_responder(&mut server, &server_caps),
        )
        .await
    });

    let (client_result, server_result) = tokio::join!(client_task, server_task);

    let client_transport = client_result.unwrap().unwrap().unwrap();
    let server_transport = server_result.unwrap().unwrap().unwrap();

    assert_eq!(client_transport, server_transport);
}

// ============================================================================
// Transport capabilities parsing tests
// ============================================================================

#[test]
fn test_capabilities_agent_version_roundtrip() {
    let caps = TransportCapabilities::full(NatType::FullCone);

    // Simulate what would be in agent version
    let suffix = caps.to_multiaddr_suffix();
    let agent_version = format!("botho/1.0.0/5{}", suffix);

    // Parse it back
    let parsed = TransportCapabilities::from_agent_version(&agent_version).unwrap();

    assert_eq!(parsed.supported.len(), 3);
    assert!(parsed.supports(TransportType::WebRTC));
    assert!(parsed.supports(TransportType::TlsTunnel));
    assert!(parsed.supports(TransportType::Plain));
    assert_eq!(parsed.nat_type, NatType::FullCone);
}

#[test]
fn test_capabilities_without_transport_info() {
    let agent_version = "botho/1.0.0/5";

    let parsed = TransportCapabilities::from_agent_version(agent_version);
    assert!(parsed.is_none());
}

// ============================================================================
// NAT compatibility tests
// ============================================================================

#[test]
fn test_nat_webrtc_compatibility_matrix() {
    // Test the NAT compatibility matrix for WebRTC

    // Open to anything works
    assert!(NatType::Open.webrtc_compatible_with(&NatType::Open));
    assert!(NatType::Open.webrtc_compatible_with(&NatType::FullCone));
    assert!(NatType::Open.webrtc_compatible_with(&NatType::Restricted));
    assert!(NatType::Open.webrtc_compatible_with(&NatType::PortRestricted));
    assert!(NatType::Open.webrtc_compatible_with(&NatType::Symmetric));

    // Full cone to most things works
    assert!(NatType::FullCone.webrtc_compatible_with(&NatType::Open));
    assert!(NatType::FullCone.webrtc_compatible_with(&NatType::FullCone));

    // Symmetric to Symmetric doesn't work
    assert!(!NatType::Symmetric.webrtc_compatible_with(&NatType::Symmetric));

    // Symmetric to Open should work (open side can help)
    assert!(NatType::Symmetric.webrtc_compatible_with(&NatType::Open));
}

#[test]
fn test_transport_type_preference_ordering() {
    // Verify preference scores are ordered correctly
    assert!(TransportType::WebRTC.preference_score() > TransportType::TlsTunnel.preference_score());
    assert!(TransportType::TlsTunnel.preference_score() > TransportType::Plain.preference_score());
}

// ============================================================================
// Message serialization tests
// ============================================================================

#[test]
fn test_negotiation_message_bincode_size() {
    // Verify messages are reasonably sized
    let propose = NegotiationMessage::propose(&TransportCapabilities::full(NatType::Open));
    let accept = NegotiationMessage::accept(TransportType::WebRTC);
    let reject = NegotiationMessage::reject("No common transport available");

    let propose_bytes = propose.to_bytes().unwrap();
    let accept_bytes = accept.to_bytes().unwrap();
    let reject_bytes = reject.to_bytes().unwrap();

    // Messages should be small (under 1KB each)
    assert!(propose_bytes.len() < 1024);
    assert!(accept_bytes.len() < 1024);
    assert!(reject_bytes.len() < 1024);

    // Propose is larger than accept (has more data)
    assert!(propose_bytes.len() > accept_bytes.len());
}
