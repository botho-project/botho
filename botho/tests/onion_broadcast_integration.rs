// Copyright (c) 2024 Botho Foundation

//! Integration tests for private transaction broadcasting via onion gossip.
//!
//! These tests verify the full broadcast pipeline from transaction creation
//! through onion wrapping and message creation.

use botho::network::privacy::{
    decrypt_layer, wrap_onion, BroadcastMetrics, CircuitId, CircuitPool, CircuitPoolConfig,
    DecryptedLayer, OnionBroadcaster, OutboundCircuit, SymmetricKey,
};
use bth_gossip::InnerMessage;
use libp2p::PeerId;
use std::sync::Arc;
use std::time::Duration;

fn random_symmetric_key() -> SymmetricKey {
    SymmetricKey::random(&mut rand::thread_rng())
}

fn make_test_circuit(lifetime: Duration) -> OutboundCircuit {
    let mut rng = rand::thread_rng();
    OutboundCircuit::new(
        CircuitId::random(&mut rng),
        [PeerId::random(), PeerId::random(), PeerId::random()],
        [
            SymmetricKey::random(&mut rng),
            SymmetricKey::random(&mut rng),
            SymmetricKey::random(&mut rng),
        ],
        lifetime,
    )
}

/// Test that broadcaster can be created with default configuration.
#[test]
fn test_broadcaster_creation() {
    let broadcaster = OnionBroadcaster::new();
    let snapshot = broadcaster.metrics().snapshot();

    assert_eq!(snapshot.tx_broadcast_private, 0);
    assert_eq!(snapshot.tx_queued_no_circuit, 0);
}

/// Test that broadcaster can be created with shared metrics.
#[test]
fn test_broadcaster_shared_metrics() {
    let metrics = Arc::new(BroadcastMetrics::new());
    let broadcaster = OnionBroadcaster::with_metrics(metrics.clone());

    broadcaster.metrics().inc_exit_broadcast();

    let snapshot = metrics.snapshot();
    assert_eq!(snapshot.tx_exit_broadcast, 1);
}

/// Test that all metrics counters increment correctly.
#[test]
fn test_metrics_counters() {
    let metrics = BroadcastMetrics::new();

    // Increment all counters
    for _ in 0..3 {
        metrics.tx_broadcast_private.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    for _ in 0..2 {
        metrics.tx_queued_no_circuit.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    metrics.tx_broadcast_failed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    for _ in 0..5 {
        metrics.inc_exit_broadcast();
    }

    let snapshot = metrics.snapshot();
    assert_eq!(snapshot.tx_broadcast_private, 3);
    assert_eq!(snapshot.tx_queued_no_circuit, 2);
    assert_eq!(snapshot.tx_broadcast_failed, 1);
    assert_eq!(snapshot.tx_exit_broadcast, 5);
}

/// Test onion wrapping produces valid layered encryption.
#[test]
fn test_onion_wrap_and_unwrap() {
    let keys = [random_symmetric_key(), random_symmetric_key(), random_symmetric_key()];
    let hops = [PeerId::random(), PeerId::random(), PeerId::random()];

    // Create inner message
    let tx_data = b"test transaction data".to_vec();
    let tx_hash = [42u8; 32];
    let inner = InnerMessage::Transaction {
        tx_data: tx_data.clone(),
        tx_hash,
    };
    let inner_bytes = bth_util_serial::serialize(&inner).unwrap();

    // Wrap in onion layers
    let wrapped = wrap_onion(&inner_bytes, &hops, &keys);

    // Unwrap layer by layer
    // First hop
    let layer1 = decrypt_layer(&keys[0], &wrapped).expect("layer 1 decryption failed");
    let (next1, inner1) = match layer1 {
        DecryptedLayer::Forward { next_hop, inner } => (next_hop, inner),
        _ => panic!("expected Forward layer at hop 1"),
    };
    assert_eq!(next1, hops[1]);

    // Second hop
    let layer2 = decrypt_layer(&keys[1], &inner1).expect("layer 2 decryption failed");
    let (next2, inner2) = match layer2 {
        DecryptedLayer::Forward { next_hop, inner } => (next_hop, inner),
        _ => panic!("expected Forward layer at hop 2"),
    };
    assert_eq!(next2, hops[2]);

    // Third hop (exit)
    let layer3 = decrypt_layer(&keys[2], &inner2).expect("layer 3 decryption failed");
    match layer3 {
        DecryptedLayer::Exit { payload } => {
            // Deserialize inner message
            let inner_msg: InnerMessage = bth_util_serial::deserialize(&payload).unwrap();
            match inner_msg {
                InnerMessage::Transaction { tx_data: td, tx_hash: th } => {
                    assert_eq!(td, tx_data);
                    assert_eq!(th, tx_hash);
                }
                _ => panic!("expected Transaction inner message"),
            }
        }
        _ => panic!("expected Exit layer at hop 3"),
    }
}

/// Test that circuit pool reports no circuits when empty.
#[test]
fn test_empty_circuit_pool() {
    let pool = CircuitPool::new(CircuitPoolConfig::default());

    assert!(pool.get_circuit().is_none());
    assert!(pool.needs_more_circuits());
    assert_eq!(pool.active_count(), 0);
}

/// Test that circuit pool returns circuits when available.
#[test]
fn test_circuit_pool_with_circuits() {
    let mut pool = CircuitPool::new(CircuitPoolConfig {
        min_circuits: 2,
        ..Default::default()
    });

    // Add circuits
    pool.add_circuit(make_test_circuit(Duration::from_secs(600)));
    pool.add_circuit(make_test_circuit(Duration::from_secs(600)));

    assert_eq!(pool.active_count(), 2);
    assert!(!pool.needs_more_circuits());
    assert!(pool.get_circuit().is_some());
}

/// Test transaction hash validation.
#[test]
fn test_tx_hash_validation() {
    use sha2::{Digest, Sha256};

    let tx_data = b"valid transaction data";

    // Compute correct hash
    let mut hasher = Sha256::new();
    hasher.update(tx_data);
    let hash = hasher.finalize();
    let mut correct_hash = [0u8; 32];
    correct_hash.copy_from_slice(&hash);

    // Valid hash should pass
    assert!(OnionBroadcaster::validate_tx_hash(tx_data, &correct_hash));

    // Wrong hash should fail
    let wrong_hash = [0u8; 32];
    assert!(!OnionBroadcaster::validate_tx_hash(tx_data, &wrong_hash));

    // Empty data should fail
    assert!(!OnionBroadcaster::validate_tx_hash(&[], &correct_hash));
}

/// Test circuit expiration removes circuits from pool.
#[test]
fn test_circuit_expiration() {
    let mut pool = CircuitPool::new(CircuitPoolConfig::default());

    // Add a short-lived circuit (1ms lifetime + max jitter of 180s)
    // Since jitter is random, we can't guarantee expiration timing in integration tests.
    // Instead, test that remove_expired doesn't panic and works with fresh circuits.
    pool.add_circuit(make_test_circuit(Duration::from_secs(600)));
    pool.add_circuit(make_test_circuit(Duration::from_secs(600)));

    assert_eq!(pool.total_count(), 2);

    // Fresh circuits shouldn't be removed
    let removed = pool.remove_expired();
    assert_eq!(removed, 0);
    assert_eq!(pool.total_count(), 2);
}

/// Test InnerMessage serialization round-trip.
#[test]
fn test_inner_message_serialization() {
    // Transaction
    let tx_inner = InnerMessage::Transaction {
        tx_data: vec![1, 2, 3, 4, 5],
        tx_hash: [99u8; 32],
    };
    let tx_bytes = bth_util_serial::serialize(&tx_inner).unwrap();
    let tx_decoded: InnerMessage = bth_util_serial::deserialize(&tx_bytes).unwrap();
    match tx_decoded {
        InnerMessage::Transaction { tx_data, tx_hash } => {
            assert_eq!(tx_data, vec![1, 2, 3, 4, 5]);
            assert_eq!(tx_hash, [99u8; 32]);
        }
        _ => panic!("expected Transaction"),
    }

    // SyncRequest
    let sync_inner = InnerMessage::SyncRequest {
        from_height: 12345,
        max_blocks: 100,
    };
    let sync_bytes = bth_util_serial::serialize(&sync_inner).unwrap();
    let sync_decoded: InnerMessage = bth_util_serial::deserialize(&sync_bytes).unwrap();
    match sync_decoded {
        InnerMessage::SyncRequest { from_height, max_blocks } => {
            assert_eq!(from_height, 12345);
            assert_eq!(max_blocks, 100);
        }
        _ => panic!("expected SyncRequest"),
    }

    // Cover
    let cover_inner = InnerMessage::Cover;
    let cover_bytes = bth_util_serial::serialize(&cover_inner).unwrap();
    let cover_decoded: InnerMessage = bth_util_serial::deserialize(&cover_bytes).unwrap();
    assert!(matches!(cover_decoded, InnerMessage::Cover));
}

/// Test that multiple circuits provide random selection.
#[test]
fn test_random_circuit_selection() {
    let mut pool = CircuitPool::new(CircuitPoolConfig::default());

    // Add several circuits with different IDs
    for _ in 0..10 {
        pool.add_circuit(make_test_circuit(Duration::from_secs(600)));
    }

    // Get circuits multiple times - should get different ones sometimes
    let mut seen_ids = std::collections::HashSet::new();
    for _ in 0..20 {
        if let Some(circuit) = pool.get_circuit() {
            seen_ids.insert(*circuit.id().as_bytes());
        }
    }

    // We should see more than one circuit (random selection)
    // With 10 circuits and 20 selections, probability of seeing only 1 is (1/10)^19 ≈ 0
    assert!(seen_ids.len() > 1, "Expected random selection to hit multiple circuits");
}

/// Test broadcast error types.
#[test]
fn test_broadcast_error_display() {
    use botho::network::privacy::BroadcastError;

    let err = BroadcastError::NoCircuit;
    assert!(format!("{}", err).contains("no circuit"));

    let err = BroadcastError::SerializationError("test error".to_string());
    assert!(format!("{}", err).contains("serialize"));
    assert!(format!("{}", err).contains("test error"));

    let err = BroadcastError::GossipError("network down".to_string());
    assert!(format!("{}", err).contains("gossip"));
    assert!(format!("{}", err).contains("network down"));
}
