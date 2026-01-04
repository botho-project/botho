// Copyright (c) 2024 Botho Foundation

//! Integration tests for the WebRTC signaling channel.
//!
//! These tests verify the full signaling flow for WebRTC connection
//! establishment, including SDP exchange and ICE candidate handling.

use botho::network::transport::{
    IceCandidate, SessionId, SignalingChannel, SignalingError, SignalingMessage, SignalingRole,
    SignalingSession, SignalingState,
};
use libp2p::PeerId;
use std::time::Duration;
use tokio::io::duplex;

/// Sample SDP offer for testing.
fn sample_offer_sdp() -> String {
    "v=0\r\n\
     o=- 0 0 IN IP4 127.0.0.1\r\n\
     s=-\r\n\
     t=0 0\r\n\
     a=group:BUNDLE 0\r\n\
     m=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n\
     c=IN IP4 0.0.0.0\r\n\
     a=ice-ufrag:offer\r\n\
     a=ice-pwd:offerpassword12345678\r\n\
     a=fingerprint:sha-256 AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99\r\n\
     a=setup:actpass\r\n"
        .to_string()
}

/// Sample SDP answer for testing.
fn sample_answer_sdp() -> String {
    "v=0\r\n\
     o=- 0 0 IN IP4 127.0.0.1\r\n\
     s=-\r\n\
     t=0 0\r\n\
     a=group:BUNDLE 0\r\n\
     m=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n\
     c=IN IP4 0.0.0.0\r\n\
     a=ice-ufrag:answer\r\n\
     a=ice-pwd:answerpassword12345678\r\n\
     a=fingerprint:sha-256 11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF:00\r\n\
     a=setup:active\r\n"
        .to_string()
}

/// Sample ICE candidates for testing.
fn sample_ice_candidates() -> Vec<(String, Option<String>, Option<u16>)> {
    vec![
        (
            "candidate:1 1 UDP 2130706431 192.168.1.1 54400 typ host".to_string(),
            Some("0".to_string()),
            Some(0),
        ),
        (
            "candidate:2 1 UDP 1694498815 203.0.113.1 54401 typ srflx raddr 192.168.1.1 rport 54400".to_string(),
            Some("0".to_string()),
            Some(0),
        ),
        (
            "candidate:3 1 UDP 16777215 198.51.100.1 54402 typ relay raddr 203.0.113.1 rport 54401".to_string(),
            Some("0".to_string()),
            Some(0),
        ),
    ]
}

/// Test full offer/answer SDP exchange between two peers.
#[tokio::test]
async fn test_full_sdp_exchange() {
    let (client, server) = duplex(1024 * 1024);
    let mut offerer = SignalingChannel::new(client);
    let mut answerer = SignalingChannel::new(server);

    let mut rng = rand::thread_rng();
    let session_id = SessionId::random(&mut rng);
    let offer_sdp = sample_offer_sdp();
    let answer_sdp = sample_answer_sdp();

    let offer_sdp_clone = offer_sdp.clone();
    let answer_sdp_clone = answer_sdp.clone();

    // Spawn offerer task
    let offerer_handle = tokio::spawn(async move {
        offerer
            .exchange_sdp(offer_sdp_clone, session_id, true)
            .await
    });

    // Spawn answerer task
    let answerer_handle = tokio::spawn(async move {
        answerer
            .exchange_sdp(answer_sdp_clone, session_id, false)
            .await
    });

    // Wait for both to complete
    let (offerer_result, answerer_result) = tokio::join!(offerer_handle, answerer_handle);

    let remote_answer = offerer_result.unwrap().expect("Offerer should succeed");
    let remote_offer = answerer_result.unwrap().expect("Answerer should succeed");

    // Verify SDPs were exchanged correctly
    assert_eq!(remote_answer, answer_sdp, "Offerer should receive answer");
    assert_eq!(remote_offer, offer_sdp, "Answerer should receive offer");
}

/// Test full signaling flow with SDP exchange and ICE candidates.
#[tokio::test]
async fn test_full_signaling_flow_with_ice() {
    let (client, server) = duplex(1024 * 1024);
    let mut offerer = SignalingChannel::new(client);
    let mut answerer = SignalingChannel::new(server);

    let mut rng = rand::thread_rng();
    let session_id = SessionId::random(&mut rng);
    let offer_sdp = sample_offer_sdp();
    let answer_sdp = sample_answer_sdp();

    // Phase 1: Exchange SDPs
    let offer_sdp_clone = offer_sdp.clone();
    let answer_sdp_clone = answer_sdp.clone();

    let offerer_exchange = tokio::spawn(async move {
        offerer
            .exchange_sdp(offer_sdp_clone, session_id, true)
            .await
            .expect("SDP exchange should succeed");
        offerer
    });

    let answerer_exchange = tokio::spawn(async move {
        answerer
            .exchange_sdp(answer_sdp_clone, session_id, false)
            .await
            .expect("SDP exchange should succeed");
        answerer
    });

    let (offerer_result, answerer_result) = tokio::join!(offerer_exchange, answerer_exchange);
    let mut offerer = offerer_result.unwrap();
    let mut answerer = answerer_result.unwrap();

    // Phase 2: Exchange ICE candidates
    let candidates = sample_ice_candidates();

    // Offerer sends ICE candidates
    for (candidate, sdp_mid, sdp_mline_index) in &candidates {
        offerer
            .send_ice_candidate(session_id, candidate.clone(), sdp_mid.clone(), *sdp_mline_index)
            .await
            .expect("Should send ICE candidate");
    }

    // Answerer receives ICE candidates
    for expected in &candidates {
        let msg = answerer.recv().await.expect("Should receive message");
        match msg {
            SignalingMessage::IceCandidate {
                candidate,
                sdp_mid,
                sdp_mline_index,
                session_id: recv_session_id,
            } => {
                assert_eq!(candidate, expected.0);
                assert_eq!(sdp_mid, expected.1);
                assert_eq!(sdp_mline_index, expected.2);
                assert_eq!(recv_session_id, session_id);
            }
            _ => panic!("Expected IceCandidate message"),
        }
    }
}

/// Test rejection handling.
#[tokio::test]
async fn test_signaling_rejection() {
    let (client, server) = duplex(1024 * 1024);
    let mut offerer = SignalingChannel::new(client);
    let mut answerer = SignalingChannel::new(server);

    let mut rng = rand::thread_rng();
    let session_id = SessionId::random(&mut rng);
    let offer_sdp = sample_offer_sdp();

    // Offerer sends offer
    let offerer_handle = tokio::spawn(async move {
        offerer
            .send(SignalingMessage::Offer {
                sdp: offer_sdp,
                session_id,
            })
            .await
            .expect("Should send offer");

        // Wait for response (expecting rejection)
        offerer.recv().await
    });

    // Answerer receives offer and rejects
    let answerer_handle = tokio::spawn(async move {
        let msg = answerer.recv().await.expect("Should receive offer");
        assert!(matches!(msg, SignalingMessage::Offer { .. }));

        // Send rejection
        answerer
            .reject(session_id, "Bandwidth limit exceeded".to_string())
            .await
            .expect("Should send rejection");
    });

    let (offerer_result, _) = tokio::join!(offerer_handle, answerer_handle);
    let response = offerer_result.unwrap();

    match response {
        Ok(SignalingMessage::Reject { reason, .. }) => {
            assert_eq!(reason, "Bandwidth limit exceeded");
        }
        _ => panic!("Expected Reject message"),
    }
}

/// Test session state management with multiple peers.
#[test]
fn test_signaling_state_multi_peer() {
    let mut state = SignalingState::default();
    let mut rng = rand::thread_rng();

    let peer1 = PeerId::random();
    let peer2 = PeerId::random();

    // Create sessions for peer1
    let session1a = SessionId::random(&mut rng);
    let session1b = SessionId::random(&mut rng);

    state
        .create_session(session1a, peer1, SignalingRole::Offerer)
        .unwrap();
    state
        .create_session(session1b, peer1, SignalingRole::Answerer)
        .unwrap();

    // Create sessions for peer2
    let session2 = SessionId::random(&mut rng);
    state
        .create_session(session2, peer2, SignalingRole::Offerer)
        .unwrap();

    // Verify counts
    assert_eq!(state.session_count(), 3);
    assert_eq!(state.peer_session_count(&peer1), 2);
    assert_eq!(state.peer_session_count(&peer2), 1);

    // Remove a session
    state.remove_session(&session1a);
    assert_eq!(state.session_count(), 2);
    assert_eq!(state.peer_session_count(&peer1), 1);

    // Verify session data
    let session = state.get_session(&session1b).unwrap();
    assert_eq!(session.peer, peer1);
    assert_eq!(session.role, SignalingRole::Answerer);
}

/// Test session with ICE candidate collection.
#[test]
fn test_session_ice_candidate_collection() {
    let peer = PeerId::random();
    let mut session = SignalingSession::new(peer, SignalingRole::Offerer);

    // Set local SDP
    session.local_sdp = Some(sample_offer_sdp());
    assert!(!session.is_complete());

    // Add ICE candidates
    let candidates = sample_ice_candidates();
    for (candidate, sdp_mid, sdp_mline_index) in candidates {
        session
            .add_ice_candidate(IceCandidate {
                candidate,
                sdp_mid,
                sdp_mline_index,
            })
            .expect("Should add ICE candidate");
    }
    assert_eq!(session.ice_candidates.len(), 3);

    // Set remote SDP
    session.remote_sdp = Some(sample_answer_sdp());
    assert!(session.is_complete());
}

/// Test concurrent signaling sessions.
#[tokio::test]
async fn test_concurrent_signaling_sessions() {
    let num_sessions = 4;
    let mut handles = Vec::new();

    // Generate all session IDs upfront to avoid thread_rng() in async blocks
    let mut rng = rand::thread_rng();
    let session_ids: Vec<SessionId> = (0..num_sessions)
        .map(|_| SessionId::random(&mut rng))
        .collect();

    for (i, session_id) in session_ids.into_iter().enumerate() {
        let handle = tokio::spawn(async move {
            let (client, server) = duplex(1024 * 1024);
            let mut offerer = SignalingChannel::new(client);
            let mut answerer = SignalingChannel::new(server);

            let offer_sdp = format!(
                "v=0\r\no=session{} 0 0 IN IP4 127.0.0.1\r\ns=-\r\n",
                i
            );
            let answer_sdp = format!(
                "v=0\r\no=answer{} 0 0 IN IP4 127.0.0.1\r\ns=-\r\n",
                i
            );

            let offer_clone = offer_sdp.clone();
            let answer_clone = answer_sdp.clone();

            let offerer_task = tokio::spawn(async move {
                offerer.exchange_sdp(offer_clone, session_id, true).await
            });

            let answerer_task = tokio::spawn(async move {
                answerer.exchange_sdp(answer_clone, session_id, false).await
            });

            let (offerer_result, answerer_result) = tokio::join!(offerer_task, answerer_task);

            (
                offerer_result.unwrap().expect("Offerer should succeed"),
                answerer_result.unwrap().expect("Answerer should succeed"),
                offer_sdp,
                answer_sdp,
            )
        });
        handles.push(handle);
    }

    // Wait for all sessions to complete
    for handle in handles {
        let (remote_answer, remote_offer, original_offer, original_answer) =
            handle.await.expect("Session should complete");
        assert_eq!(remote_answer, original_answer);
        assert_eq!(remote_offer, original_offer);
    }
}

/// Test signaling channel with short timeout.
#[tokio::test]
async fn test_signaling_timeout_behavior() {
    let (client, _server) = duplex(1024);
    let mut channel = SignalingChannel::with_timeout(client, Duration::from_millis(50));

    let start = std::time::Instant::now();
    let result = channel.recv().await;
    let elapsed = start.elapsed();

    assert!(matches!(result, Err(SignalingError::Timeout)));
    assert!(
        elapsed >= Duration::from_millis(50),
        "Should wait at least 50ms"
    );
    assert!(
        elapsed < Duration::from_millis(200),
        "Should not wait too long"
    );
}

/// Test signaling message validation rejects invalid data.
#[test]
fn test_invalid_message_rejection() {
    let mut rng = rand::thread_rng();
    let session_id = SessionId::random(&mut rng);

    // Invalid SDP (doesn't start with v=)
    let invalid_offer = SignalingMessage::Offer {
        sdp: "invalid sdp content".to_string(),
        session_id,
    };
    assert!(matches!(
        invalid_offer.validate(),
        Err(SignalingError::InvalidSdp(_))
    ));

    // Oversized SDP
    let oversized = SignalingMessage::Offer {
        sdp: format!("v={}", "x".repeat(200 * 1024)),
        session_id,
    };
    assert!(matches!(
        oversized.validate(),
        Err(SignalingError::InvalidSdp(_))
    ));

    // Valid message should pass
    let valid = SignalingMessage::Offer {
        sdp: sample_offer_sdp(),
        session_id,
    };
    assert!(valid.validate().is_ok());
}

/// Test that session expiration cleanup works correctly.
#[test]
fn test_session_expiration() {
    let short_timeout = Duration::from_millis(10);
    let mut state = SignalingState::new(short_timeout);
    let mut rng = rand::thread_rng();
    let peer = PeerId::random();

    // Create session
    let session_id = SessionId::random(&mut rng);
    state
        .create_session(session_id, peer, SignalingRole::Offerer)
        .unwrap();
    assert_eq!(state.session_count(), 1);

    // Wait for expiration
    std::thread::sleep(Duration::from_millis(50));

    // Cleanup should remove expired session
    let cleaned = state.cleanup_expired();
    assert_eq!(cleaned, 1);
    assert_eq!(state.session_count(), 0);
}
