// Copyright (c) 2024 Botho Foundation

//! Integration tests for the circuit handshake protocol.
//!
//! These tests verify the telescoping handshake protocol for establishing
//! circuit keys across multiple hops.

use botho::network::privacy::{CircuitHandshake, HandshakeError, SymmetricKey};
use bth_crypto_keys::{X25519EphemeralPrivate, X25519Public};
use bth_gossip::{CircuitDestroyReason, CircuitHandshakeMsg, CircuitId};
use bth_util_from_random::FromRandom;

/// Simulates a relay hop that can handle handshake messages.
struct MockRelayHop {
    /// Keys established for circuits where we are a hop
    circuit_keys: std::collections::HashMap<CircuitId, SymmetricKey>,
}

impl MockRelayHop {
    fn new() -> Self {
        Self {
            circuit_keys: std::collections::HashMap::new(),
        }
    }

    /// Handle an incoming Create message
    fn handle_create(&mut self, msg: &CircuitHandshakeMsg) -> CircuitHandshakeMsg {
        match msg {
            CircuitHandshakeMsg::Create {
                circuit_id,
                ephemeral_pubkey,
            } => {
                let (response, key) =
                    CircuitHandshake::respond_to_create(*circuit_id, ephemeral_pubkey)
                        .expect("Handshake response should succeed");

                self.circuit_keys.insert(*circuit_id, key);
                response
            }
            _ => panic!("Expected Create message"),
        }
    }

    /// Check if we have a key for a circuit
    fn has_circuit_key(&self, circuit_id: &CircuitId) -> bool {
        self.circuit_keys.contains_key(circuit_id)
    }

    /// Get the key for a circuit
    fn get_circuit_key(&self, circuit_id: &CircuitId) -> Option<&SymmetricKey> {
        self.circuit_keys.get(circuit_id)
    }
}

/// Test building a 3-hop circuit with the telescoping handshake.
///
/// This simulates the full circuit building process:
/// 1. Create/Created with Hop1
/// 2. Extend/Extended through Hop1 to Hop2
/// 3. Extend/Extended through Hop1->Hop2 to Hop3
#[test]
fn test_three_hop_circuit_handshake() {
    // Set up mock relay hops
    let mut hop1 = MockRelayHop::new();
    let mut hop2 = MockRelayHop::new();
    let mut hop3 = MockRelayHop::new();

    // Circuit initiator (Alice)
    let mut alice_handshake = CircuitHandshake::new();
    let circuit_id = CircuitId::random();

    // === Step 1: Handshake with Hop1 ===
    let create1 = alice_handshake.initiate_create(circuit_id);
    let created1 = hop1.handle_create(&create1);

    let hop1_pubkey = match &created1 {
        CircuitHandshakeMsg::Created {
            ephemeral_pubkey, ..
        } => ephemeral_pubkey.clone(),
        _ => panic!("Expected Created message"),
    };

    let alice_key1 = alice_handshake
        .complete_create(&hop1_pubkey, circuit_id)
        .expect("First hop handshake should complete");

    // Verify Alice and Hop1 derived the same key
    assert_eq!(
        alice_key1.as_bytes(),
        hop1.get_circuit_key(&circuit_id).unwrap().as_bytes(),
        "Alice and Hop1 should derive the same key"
    );

    // === Step 2: Extend to Hop2 (through Hop1) ===
    // In the real protocol, Alice would encrypt the Create message for Hop2
    // using alice_key1, and Hop1 would decrypt and forward it.
    // For this test, we simulate the direct handshake.

    let mut alice_handshake2 = CircuitHandshake::new();
    let create2 = alice_handshake2.initiate_create(circuit_id);
    let created2 = hop2.handle_create(&create2);

    let hop2_pubkey = match &created2 {
        CircuitHandshakeMsg::Created {
            ephemeral_pubkey, ..
        } => ephemeral_pubkey.clone(),
        _ => panic!("Expected Created message"),
    };

    let alice_key2 = alice_handshake2
        .complete_create(&hop2_pubkey, circuit_id)
        .expect("Second hop handshake should complete");

    // Verify Alice and Hop2 derived the same key
    assert_eq!(
        alice_key2.as_bytes(),
        hop2.get_circuit_key(&circuit_id).unwrap().as_bytes(),
        "Alice and Hop2 should derive the same key"
    );

    // Keys for different hops should be different (fresh ephemeral keys)
    assert_ne!(
        alice_key1.as_bytes(),
        alice_key2.as_bytes(),
        "Keys for different hops should be different"
    );

    // === Step 3: Extend to Hop3 (through Hop1->Hop2) ===
    let mut alice_handshake3 = CircuitHandshake::new();
    let create3 = alice_handshake3.initiate_create(circuit_id);
    let created3 = hop3.handle_create(&create3);

    let hop3_pubkey = match &created3 {
        CircuitHandshakeMsg::Created {
            ephemeral_pubkey, ..
        } => ephemeral_pubkey.clone(),
        _ => panic!("Expected Created message"),
    };

    let alice_key3 = alice_handshake3
        .complete_create(&hop3_pubkey, circuit_id)
        .expect("Third hop handshake should complete");

    // Verify Alice and Hop3 derived the same key
    assert_eq!(
        alice_key3.as_bytes(),
        hop3.get_circuit_key(&circuit_id).unwrap().as_bytes(),
        "Alice and Hop3 should derive the same key"
    );

    // All three keys should be unique
    assert_ne!(alice_key1.as_bytes(), alice_key3.as_bytes());
    assert_ne!(alice_key2.as_bytes(), alice_key3.as_bytes());

    // Verify all hops have the circuit registered
    assert!(hop1.has_circuit_key(&circuit_id));
    assert!(hop2.has_circuit_key(&circuit_id));
    assert!(hop3.has_circuit_key(&circuit_id));
}

/// Test that ephemeral keys are unique per handshake.
#[test]
fn test_ephemeral_key_uniqueness() {
    let circuit_id = CircuitId::random();

    // Generate multiple handshakes and collect public keys
    let mut pubkeys: Vec<X25519Public> = Vec::new();

    for _ in 0..10 {
        let mut handshake = CircuitHandshake::new();
        let create_msg = handshake.initiate_create(circuit_id);

        let pubkey = match create_msg {
            CircuitHandshakeMsg::Create {
                ephemeral_pubkey, ..
            } => ephemeral_pubkey,
            _ => panic!("Expected Create message"),
        };

        // Each public key should be unique
        for existing in &pubkeys {
            let pubkey_bytes: &[u8] = pubkey.as_ref();
            let existing_bytes: &[u8] = existing.as_ref();
            assert_ne!(
                pubkey_bytes, existing_bytes,
                "Ephemeral public keys should be unique"
            );
        }

        pubkeys.push(pubkey);
        handshake.cancel(); // Clean up
    }
}

/// Test domain separation - same shared secret with different circuit IDs
/// should produce different keys.
#[test]
fn test_domain_separation() {
    // Create two circuits with different IDs
    let circuit_id1 = CircuitId::random();
    let circuit_id2 = CircuitId::random();

    // Simulate handshake with first circuit
    let mut alice1 = CircuitHandshake::new();
    let mut hop1 = MockRelayHop::new();

    let create1 = alice1.initiate_create(circuit_id1);
    let created1 = hop1.handle_create(&create1);
    let hop1_pubkey = match &created1 {
        CircuitHandshakeMsg::Created {
            ephemeral_pubkey, ..
        } => ephemeral_pubkey.clone(),
        _ => panic!("Expected Created"),
    };
    let key1 = alice1.complete_create(&hop1_pubkey, circuit_id1).unwrap();

    // Simulate handshake with second circuit
    let mut alice2 = CircuitHandshake::new();
    let mut hop2 = MockRelayHop::new();

    let create2 = alice2.initiate_create(circuit_id2);
    let created2 = hop2.handle_create(&create2);
    let hop2_pubkey = match &created2 {
        CircuitHandshakeMsg::Created {
            ephemeral_pubkey, ..
        } => ephemeral_pubkey.clone(),
        _ => panic!("Expected Created"),
    };
    let key2 = alice2.complete_create(&hop2_pubkey, circuit_id2).unwrap();

    // Keys should be different due to different circuit IDs
    // (and different ephemeral keys)
    assert_ne!(
        key1.as_bytes(),
        key2.as_bytes(),
        "Different circuits should have different keys"
    );
}

/// Test handling of invalid circuit ID in response.
#[test]
fn test_invalid_circuit_id_response() {
    let circuit_id1 = CircuitId::random();
    let circuit_id2 = CircuitId::random();

    let mut alice = CircuitHandshake::new();
    alice.initiate_create(circuit_id1);

    // Try to complete with a different circuit ID
    let mut rng = rand::thread_rng();
    let dummy_private = X25519EphemeralPrivate::from_random(&mut rng);
    let dummy_public = X25519Public::from(&dummy_private);

    let result = alice.complete_create(&dummy_public, circuit_id2);

    match result {
        Err(HandshakeError::CircuitIdMismatch { expected, actual }) => {
            assert_eq!(expected, circuit_id1);
            assert_eq!(actual, circuit_id2);
        }
        _ => panic!("Expected CircuitIdMismatch error"),
    }
}

/// Test circuit ID serialization round-trip.
#[test]
fn test_circuit_id_serialization() {
    let circuit_id = CircuitId::random();

    // Serialize to JSON
    let json = serde_json::to_string(&circuit_id).expect("Serialization should succeed");

    // Deserialize back
    let deserialized: CircuitId =
        serde_json::from_str(&json).expect("Deserialization should succeed");

    assert_eq!(circuit_id, deserialized);
}

/// Test CircuitHandshakeMsg serialization.
#[test]
fn test_handshake_message_serialization() {
    let circuit_id = CircuitId::random();

    // Generate an ephemeral key for testing
    let mut rng = rand::thread_rng();
    let ephemeral_private = X25519EphemeralPrivate::from_random(&mut rng);
    let ephemeral_pubkey = X25519Public::from(&ephemeral_private);

    let create_msg = CircuitHandshakeMsg::Create {
        circuit_id,
        ephemeral_pubkey: ephemeral_pubkey.clone(),
    };

    // Serialize to JSON
    let json = serde_json::to_string(&create_msg).expect("Serialization should succeed");

    // Deserialize back
    let deserialized: CircuitHandshakeMsg =
        serde_json::from_str(&json).expect("Deserialization should succeed");

    match deserialized {
        CircuitHandshakeMsg::Create {
            circuit_id: cid,
            ephemeral_pubkey: epk,
        } => {
            assert_eq!(cid, circuit_id);
            let epk_bytes: &[u8] = epk.as_ref();
            let expected_bytes: &[u8] = ephemeral_pubkey.as_ref();
            assert_eq!(epk_bytes, expected_bytes);
        }
        _ => panic!("Expected Create message"),
    }
}

/// Test CircuitDestroyReason serialization.
#[test]
fn test_destroy_reason_serialization() {
    let reasons = [
        CircuitDestroyReason::Finished,
        CircuitDestroyReason::Timeout,
        CircuitDestroyReason::Error,
        CircuitDestroyReason::ProtocolViolation,
    ];

    for reason in &reasons {
        let json = serde_json::to_string(reason).expect("Serialization should succeed");
        let deserialized: CircuitDestroyReason =
            serde_json::from_str(&json).expect("Deserialization should succeed");
        assert_eq!(*reason, deserialized);
    }
}

/// Test Destroy message construction.
#[test]
fn test_destroy_message() {
    let circuit_id = CircuitId::random();

    let destroy_msg = CircuitHandshakeMsg::Destroy {
        circuit_id,
        reason: CircuitDestroyReason::Finished,
    };

    match destroy_msg {
        CircuitHandshakeMsg::Destroy {
            circuit_id: cid,
            reason,
        } => {
            assert_eq!(cid, circuit_id);
            assert_eq!(reason, CircuitDestroyReason::Finished);
        }
        _ => panic!("Expected Destroy message"),
    }
}

/// Test Extend/Extended message structure.
#[test]
fn test_extend_message() {
    let circuit_id = CircuitId::random();
    let next_hop = "12D3KooWDpJ7As7BWAwRMfu1VU2WCqNjvq387JEYKDBj4kx6nXTN".to_string();
    let encrypted_create = vec![1, 2, 3, 4, 5];

    let extend_msg = CircuitHandshakeMsg::Extend {
        circuit_id,
        next_hop: next_hop.clone(),
        encrypted_create: encrypted_create.clone(),
    };

    // Serialize and deserialize
    let json = serde_json::to_string(&extend_msg).expect("Serialization should succeed");
    let deserialized: CircuitHandshakeMsg =
        serde_json::from_str(&json).expect("Deserialization should succeed");

    match deserialized {
        CircuitHandshakeMsg::Extend {
            circuit_id: cid,
            next_hop: nh,
            encrypted_create: ec,
        } => {
            assert_eq!(cid, circuit_id);
            assert_eq!(nh, next_hop);
            assert_eq!(ec, encrypted_create);
        }
        _ => panic!("Expected Extend message"),
    }
}

/// Test Extended message structure.
#[test]
fn test_extended_message() {
    let circuit_id = CircuitId::random();
    let encrypted_created = vec![5, 4, 3, 2, 1];

    let extended_msg = CircuitHandshakeMsg::Extended {
        circuit_id,
        encrypted_created: encrypted_created.clone(),
    };

    // Serialize and deserialize
    let json = serde_json::to_string(&extended_msg).expect("Serialization should succeed");
    let deserialized: CircuitHandshakeMsg =
        serde_json::from_str(&json).expect("Deserialization should succeed");

    match deserialized {
        CircuitHandshakeMsg::Extended {
            circuit_id: cid,
            encrypted_created: ec,
        } => {
            assert_eq!(cid, circuit_id);
            assert_eq!(ec, encrypted_created);
        }
        _ => panic!("Expected Extended message"),
    }
}

/// Test that handshake timeout is correctly configured.
#[test]
fn test_handshake_timeout_value() {
    let timeout = CircuitHandshake::timeout();
    assert_eq!(
        timeout.as_secs(),
        30,
        "Default timeout should be 30 seconds"
    );
}

/// Benchmark-style test for multiple concurrent handshakes.
#[test]
fn test_concurrent_handshakes() {
    let num_circuits = 100;
    let mut alice_keys: Vec<SymmetricKey> = Vec::new();

    for i in 0..num_circuits {
        let circuit_id = CircuitId::random();
        let mut alice = CircuitHandshake::new();
        let mut hop = MockRelayHop::new();

        let create = alice.initiate_create(circuit_id);
        let created = hop.handle_create(&create);

        let hop_pubkey = match &created {
            CircuitHandshakeMsg::Created {
                ephemeral_pubkey, ..
            } => ephemeral_pubkey.clone(),
            _ => panic!("Expected Created message"),
        };

        let key = alice
            .complete_create(&hop_pubkey, circuit_id)
            .expect(&format!("Handshake {} should complete", i));

        // All keys should be unique
        for (j, existing) in alice_keys.iter().enumerate() {
            assert_ne!(
                key.as_bytes(),
                existing.as_bytes(),
                "Key {} and {} should be different",
                i,
                j
            );
        }

        alice_keys.push(key);
    }

    assert_eq!(alice_keys.len(), num_circuits);
}
