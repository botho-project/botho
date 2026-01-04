// Copyright (c) 2024 Botho Foundation

//! Integration tests for relay message handling.
//!
//! These tests verify the full relay pipeline from receiving an onion message
//! through decryption and forwarding/exit processing.

use botho::network::privacy::{
    encrypt_exit_layer, encrypt_forward_layer, wrap_onion, CircuitHopKey, CircuitId, RelayAction,
    RelayHandler, RelayState, RelayStateConfig, SymmetricKey,
};
use bth_gossip::{InnerMessage, OnionRelayMessage};
use libp2p::PeerId;

fn random_symmetric_key() -> SymmetricKey {
    SymmetricKey::random(&mut rand::thread_rng())
}

fn random_circuit_id() -> CircuitId {
    CircuitId::random(&mut rand::thread_rng())
}

fn to_gossip_circuit_id(id: &CircuitId) -> bth_gossip::CircuitId {
    bth_gossip::CircuitId(*id.as_bytes())
}

/// Test that a message traverses a 3-hop circuit correctly.
#[test]
fn test_three_hop_circuit_relay() {
    // Create 3 hops with unique keys
    let mut hop1_state = RelayState::new(RelayStateConfig::default());
    let mut hop2_state = RelayState::new(RelayStateConfig::default());
    let mut hop3_state = RelayState::new(RelayStateConfig::default());

    let hop1_handler = RelayHandler::new();
    let hop2_handler = RelayHandler::new();
    let hop3_handler = RelayHandler::new();

    let circuit_id = random_circuit_id();
    let key1 = random_symmetric_key();
    let key2 = random_symmetric_key();
    let key3 = random_symmetric_key();

    let hop2_peer = PeerId::random();
    let hop3_peer = PeerId::random();

    // Set up circuit keys at each hop
    hop1_state.add_circuit_key(
        circuit_id,
        CircuitHopKey::new_forward(key1.duplicate(), hop2_peer),
    );
    hop2_state.add_circuit_key(
        circuit_id,
        CircuitHopKey::new_forward(key2.duplicate(), hop3_peer),
    );
    hop3_state.add_circuit_key(circuit_id, CircuitHopKey::new_exit(key3.duplicate()));

    // Create inner message
    let tx_data = b"test transaction for 3-hop circuit".to_vec();
    let tx_hash = [42u8; 32];
    let inner = InnerMessage::Transaction {
        tx_data: tx_data.clone(),
        tx_hash,
    };
    let inner_bytes = bth_util_serial::serialize(&inner).unwrap();

    // Wrap in 3 onion layers
    let hops = [PeerId::random(), hop2_peer, hop3_peer]; // First hop doesn't matter for this test
    let keys = [key1.duplicate(), key2.duplicate(), key3.duplicate()];
    let wrapped = wrap_onion(&inner_bytes, &hops, &keys);

    // Create initial message
    let msg1 = OnionRelayMessage {
        circuit_id: to_gossip_circuit_id(&circuit_id),
        payload: wrapped,
    };

    // Hop 1: Forward to hop 2
    let sender = PeerId::random();
    let action1 = hop1_handler.handle_message(&mut hop1_state, &sender, msg1);

    let msg2 = match action1 {
        RelayAction::Forward { next_hop, message } => {
            assert_eq!(next_hop, hop2_peer);
            message
        }
        _ => panic!("Expected Forward action at hop 1"),
    };

    // Hop 2: Forward to hop 3
    let action2 = hop2_handler.handle_message(&mut hop2_state, &hop2_peer, msg2);

    let msg3 = match action2 {
        RelayAction::Forward { next_hop, message } => {
            assert_eq!(next_hop, hop3_peer);
            message
        }
        _ => panic!("Expected Forward action at hop 2"),
    };

    // Hop 3: Exit - broadcast transaction
    let action3 = hop3_handler.handle_message(&mut hop3_state, &hop3_peer, msg3);

    match action3 {
        RelayAction::Exit { inner } => match inner {
            InnerMessage::Transaction {
                tx_data: td,
                tx_hash: th,
            } => {
                assert_eq!(td, tx_data);
                assert_eq!(th, tx_hash);
            }
            _ => panic!("Expected Transaction inner message"),
        },
        _ => panic!("Expected Exit action at hop 3"),
    }

    // Verify metrics
    assert_eq!(hop1_handler.metrics().snapshot().messages_forwarded, 1);
    assert_eq!(hop2_handler.metrics().snapshot().messages_forwarded, 1);
    assert_eq!(hop3_handler.metrics().snapshot().messages_exited, 1);
}

/// Test that cover traffic is silently dropped at exit.
#[test]
fn test_cover_traffic_dropped() {
    let mut relay_state = RelayState::new(RelayStateConfig::default());
    let handler = RelayHandler::new();

    let circuit_id = random_circuit_id();
    let key = random_symmetric_key();
    relay_state.add_circuit_key(circuit_id, CircuitHopKey::new_exit(key.duplicate()));

    // Create cover traffic
    let inner = InnerMessage::Cover;
    let inner_bytes = bth_util_serial::serialize(&inner).unwrap();
    let encrypted = encrypt_exit_layer(&key, &inner_bytes);

    let msg = OnionRelayMessage {
        circuit_id: to_gossip_circuit_id(&circuit_id),
        payload: encrypted,
    };

    let from = PeerId::random();
    let action = handler.handle_message(&mut relay_state, &from, msg);

    match action {
        RelayAction::Dropped { reason } => {
            assert!(reason.contains("cover"));
        }
        _ => panic!("Expected Dropped action for cover traffic"),
    }

    let metrics = handler.metrics().snapshot();
    assert_eq!(metrics.cover_traffic_received, 1);
    assert_eq!(metrics.messages_exited, 0);
}

/// Test that unknown circuits are silently ignored.
#[test]
fn test_unknown_circuit_ignored() {
    let mut relay_state = RelayState::new(RelayStateConfig::default());
    let handler = RelayHandler::new();

    // Don't add any circuit keys

    let msg = OnionRelayMessage {
        circuit_id: bth_gossip::CircuitId::random(),
        payload: vec![1, 2, 3, 4, 5],
    };

    let from = PeerId::random();
    let action = handler.handle_message(&mut relay_state, &from, msg);

    match action {
        RelayAction::Dropped { reason } => {
            assert!(reason.contains("unknown circuit"));
        }
        _ => panic!("Expected Dropped action"),
    }

    let metrics = handler.metrics().snapshot();
    assert_eq!(metrics.unknown_circuits, 1);
}

/// Test rate limiting prevents relay abuse.
#[test]
fn test_rate_limiting() {
    use botho::network::privacy::rate_limit::RelayRateLimits;

    // Configure with low relay message limit (1 msg/sec = capacity 2)
    let config = RelayStateConfig {
        max_relay_per_window: 3,
        rate_limits: RelayRateLimits {
            relay_msgs_per_sec: 1, // Token bucket capacity = 2
            relay_bandwidth_per_peer: 10_000,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut relay_state = RelayState::new(config);
    let handler = RelayHandler::new();

    let circuit_id = random_circuit_id();
    let key = random_symmetric_key();
    relay_state.add_circuit_key(circuit_id, CircuitHopKey::new_exit(key.duplicate()));

    let inner = InnerMessage::Cover;
    let inner_bytes = bth_util_serial::serialize(&inner).unwrap();
    let encrypted = encrypt_exit_layer(&key, &inner_bytes);

    let msg = OnionRelayMessage {
        circuit_id: to_gossip_circuit_id(&circuit_id),
        payload: encrypted.clone(),
    };

    let abusive_peer = PeerId::random();

    // First 2 messages should succeed (token bucket capacity = 2)
    for _ in 0..2 {
        let action = handler.handle_message(&mut relay_state, &abusive_peer, msg.clone());
        match action {
            RelayAction::Dropped { reason } => {
                assert!(reason.contains("cover"), "Should be cover traffic");
            }
            _ => panic!("Expected Dropped (cover) action"),
        }
    }

    // 3rd message should be rate limited (out of tokens)
    let action = handler.handle_message(&mut relay_state, &abusive_peer, msg);
    match action {
        RelayAction::Dropped { reason } => {
            assert!(
                reason.contains("rate limited"),
                "Expected 'rate limited' but got: {}",
                reason
            );
        }
        _ => panic!("Expected rate limited"),
    }

    let metrics = handler.metrics().snapshot();
    assert!(metrics.rate_limited >= 1);
}

/// Test decryption failures are handled gracefully.
#[test]
fn test_decryption_failure_handled() {
    let mut relay_state = RelayState::new(RelayStateConfig::default());
    let handler = RelayHandler::new();

    let circuit_id = random_circuit_id();
    let key = random_symmetric_key();
    relay_state.add_circuit_key(circuit_id, CircuitHopKey::new_exit(key.duplicate()));

    // Create message with garbage payload (tampered data)
    let msg = OnionRelayMessage {
        circuit_id: to_gossip_circuit_id(&circuit_id),
        payload: vec![0xDE; 100],
    };

    let from = PeerId::random();
    let action = handler.handle_message(&mut relay_state, &from, msg);

    match action {
        RelayAction::Dropped { reason } => {
            assert!(reason.contains("decryption failed"));
        }
        _ => panic!("Expected Dropped action"),
    }

    let metrics = handler.metrics().snapshot();
    assert_eq!(metrics.decryption_failures, 1);
}

/// Test sync request message through circuit.
#[test]
fn test_sync_request_message() {
    let mut relay_state = RelayState::new(RelayStateConfig::default());
    let handler = RelayHandler::new();

    let circuit_id = random_circuit_id();
    let key = random_symmetric_key();
    relay_state.add_circuit_key(circuit_id, CircuitHopKey::new_exit(key.duplicate()));

    // Create sync request inner message
    let inner = InnerMessage::SyncRequest {
        from_height: 12345,
        max_blocks: 100,
    };
    let inner_bytes = bth_util_serial::serialize(&inner).unwrap();
    let encrypted = encrypt_exit_layer(&key, &inner_bytes);

    let msg = OnionRelayMessage {
        circuit_id: to_gossip_circuit_id(&circuit_id),
        payload: encrypted,
    };

    let from = PeerId::random();
    let action = handler.handle_message(&mut relay_state, &from, msg);

    match action {
        RelayAction::Exit { inner } => match inner {
            InnerMessage::SyncRequest {
                from_height,
                max_blocks,
            } => {
                assert_eq!(from_height, 12345);
                assert_eq!(max_blocks, 100);
            }
            _ => panic!("Expected SyncRequest inner message"),
        },
        _ => panic!("Expected Exit action"),
    }
}

/// Test multiple circuits from different peers.
#[test]
fn test_multiple_circuits() {
    let mut relay_state = RelayState::new(RelayStateConfig::default());
    let handler = RelayHandler::new();

    // Set up 3 different circuits
    let circuits: Vec<_> = (0..3)
        .map(|_| {
            let circuit_id = random_circuit_id();
            let key = random_symmetric_key();
            relay_state.add_circuit_key(circuit_id, CircuitHopKey::new_exit(key.duplicate()));
            (circuit_id, key)
        })
        .collect();

    // Send cover traffic through each circuit
    for (circuit_id, key) in &circuits {
        let inner = InnerMessage::Cover;
        let inner_bytes = bth_util_serial::serialize(&inner).unwrap();
        let encrypted = encrypt_exit_layer(key, &inner_bytes);

        let msg = OnionRelayMessage {
            circuit_id: to_gossip_circuit_id(circuit_id),
            payload: encrypted,
        };

        let from = PeerId::random();
        let action = handler.handle_message(&mut relay_state, &from, msg);

        match action {
            RelayAction::Dropped { reason } => {
                assert!(reason.contains("cover"));
            }
            _ => panic!("Expected Dropped action"),
        }
    }

    let metrics = handler.metrics().snapshot();
    assert_eq!(metrics.messages_received, 3);
    assert_eq!(metrics.cover_traffic_received, 3);
}

/// Test circuit cleanup doesn't affect active circuits.
#[test]
fn test_circuit_cleanup() {
    let config = RelayStateConfig {
        circuit_key_lifetime: std::time::Duration::from_secs(1),
        ..Default::default()
    };
    let mut relay_state = RelayState::new(config);
    let handler = RelayHandler::new();

    // Add a circuit
    let circuit_id = random_circuit_id();
    let key = random_symmetric_key();
    relay_state.add_circuit_key(circuit_id, CircuitHopKey::new_exit(key.duplicate()));

    // Message should work immediately
    let inner = InnerMessage::Cover;
    let inner_bytes = bth_util_serial::serialize(&inner).unwrap();
    let encrypted = encrypt_exit_layer(&key, &inner_bytes);

    let msg = OnionRelayMessage {
        circuit_id: to_gossip_circuit_id(&circuit_id),
        payload: encrypted.clone(),
    };

    let from = PeerId::random();
    let action = handler.handle_message(&mut relay_state, &from, msg.clone());
    assert!(matches!(action, RelayAction::Dropped { .. })); // Cover traffic

    // Wait for circuit to expire
    std::thread::sleep(std::time::Duration::from_millis(1100));

    // Cleanup expired circuits
    let removed = relay_state.cleanup_expired_keys();
    assert_eq!(removed, 1);

    // Now message should fail (unknown circuit)
    let action = handler.handle_message(&mut relay_state, &from, msg);
    match action {
        RelayAction::Dropped { reason } => {
            assert!(reason.contains("unknown circuit"));
        }
        _ => panic!("Expected unknown circuit error"),
    }
}

/// Test transaction hash validation in should_broadcast_transaction.
#[test]
fn test_transaction_hash_validation() {
    use sha2::{Digest, Sha256};

    let tx_data = b"valid transaction data";

    // Compute correct hash
    let mut hasher = Sha256::new();
    hasher.update(tx_data);
    let hash = hasher.finalize();
    let mut correct_hash = [0u8; 32];
    correct_hash.copy_from_slice(&hash);

    // Should pass with correct hash
    assert!(RelayHandler::should_broadcast_transaction(
        tx_data,
        &correct_hash
    ));

    // Should fail with wrong hash
    let wrong_hash = [0u8; 32];
    assert!(!RelayHandler::should_broadcast_transaction(
        tx_data,
        &wrong_hash
    ));

    // Should fail with empty data
    assert!(!RelayHandler::should_broadcast_transaction(
        &[],
        &correct_hash
    ));
}
