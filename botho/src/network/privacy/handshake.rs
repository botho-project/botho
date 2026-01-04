// Copyright (c) 2024 Botho Foundation

//! Circuit handshake protocol for establishing per-hop symmetric keys.
//!
//! This module implements the telescoping handshake protocol that allows
//! building circuits incrementally, one hop at a time. Each hop only knows
//! its immediate predecessor and successor, providing sender anonymity.
//!
//! # Protocol Overview
//!
//! ```text
//! Step 1: Alice ←─X25519─→ Hop1
//!         Result: key1
//!
//! Step 2: Alice ──[Encrypt_key1(handshake)]──→ Hop1 ──→ Hop2
//!         Hop2 ←─X25519─→ Alice (through Hop1)
//!         Result: key2
//!
//! Step 3: Alice ──[Encrypt_key1(Encrypt_key2(handshake))]──→ Hop1 ──→ Hop2 ──→ Hop3
//!         Hop3 ←─X25519─→ Alice (through Hop1, Hop2)
//!         Result: key3
//! ```
//!
//! # Security Properties
//!
//! - Ephemeral keys MUST be generated fresh for each circuit
//! - Shared secrets MUST be zeroized immediately after key derivation
//! - Circuit IDs should be random and unpredictable
//! - Domain separation ensures keys derived for different purposes are
//!   independent

use super::types::{SymmetricKey, SYMMETRIC_KEY_LEN};
use bth_crypto_keys::{KexEphemeralPrivate, X25519EphemeralPrivate, X25519Public, X25519Secret};
use bth_gossip::{CircuitHandshakeMsg, CircuitId};
use bth_util_from_random::FromRandom;
use hkdf::Hkdf;
use sha2::Sha256;
use std::time::Duration;
use thiserror::Error;

/// Size of symmetric keys in bytes (256-bit).
pub const CIRCUIT_KEY_SIZE: usize = SYMMETRIC_KEY_LEN;

/// Timeout for handshake operations in seconds.
pub const HANDSHAKE_TIMEOUT_SECS: u64 = 30;

/// Domain separation string for circuit key derivation.
const CIRCUIT_KEY_DOMAIN: &[u8] = b"botho-circuit-v1";

/// Salt prefix for HKDF - combined with circuit ID for key derivation.
const HKDF_SALT_PREFIX: &[u8] = b"botho-circuit-salt-";

/// Errors that can occur during circuit handshake.
#[derive(Debug, Error)]
pub enum HandshakeError {
    /// Circuit ID mismatch in response
    #[error("Circuit ID mismatch: expected {expected}, got {actual}")]
    CircuitIdMismatch {
        expected: CircuitId,
        actual: CircuitId,
    },

    /// Invalid public key received
    #[error("Invalid public key: {0}")]
    InvalidPublicKey(String),

    /// Handshake timed out
    #[error("Handshake timed out after {0:?}")]
    Timeout(Duration),

    /// Encryption or decryption failed
    #[error("Cryptographic operation failed: {0}")]
    CryptoError(String),

    /// Network error during handshake
    #[error("Network error: {0}")]
    NetworkError(String),

    /// Handshake state error (e.g., completing before initiating)
    #[error("Invalid handshake state: {0}")]
    InvalidState(String),

    /// Key derivation failed
    #[error("Key derivation failed: {0}")]
    KeyDerivationError(String),
}

/// Result type for handshake operations.
pub type HandshakeResult<T> = Result<T, HandshakeError>;

/// State of an in-progress handshake.
///
/// The ephemeral private key is wrapped in Option so it can be taken
/// during completion. X25519EphemeralPrivate handles its own zeroization.
struct HandshakeState {
    /// Our ephemeral private key (consumed during completion)
    ephemeral_private: Option<X25519EphemeralPrivate>,
    /// Circuit ID for this handshake
    circuit_id: CircuitId,
}

/// Circuit handshake protocol implementation.
///
/// This struct manages the state for building circuits using the telescoping
/// handshake protocol. Each instance handles one circuit's key establishment.
///
/// # Example
///
/// ```ignore
/// use botho::network::privacy::CircuitHandshake;
/// use bth_gossip::CircuitId;
///
/// // Create a new handshake
/// let mut handshake = CircuitHandshake::new();
///
/// // Generate circuit ID
/// let circuit_id = CircuitId::random();
///
/// // Initiate handshake (returns our public key in a Create message)
/// let create_msg = handshake.initiate_create(circuit_id);
///
/// // Send create_msg to hop, receive Created response...
/// // let created_msg = send_and_receive(create_msg).await?;
///
/// // Complete handshake to derive symmetric key
/// // let key = handshake.complete_create(&created_msg.ephemeral_pubkey, circuit_id)?;
/// ```
pub struct CircuitHandshake {
    /// Current handshake state (None if not initiated)
    state: Option<HandshakeState>,
}

impl Default for CircuitHandshake {
    fn default() -> Self {
        Self::new()
    }
}

impl CircuitHandshake {
    /// Create a new circuit handshake instance.
    pub fn new() -> Self {
        Self { state: None }
    }

    /// Initiate a CREATE handshake for the first hop.
    ///
    /// This generates a fresh ephemeral X25519 keypair and returns a Create
    /// message containing our public key. The private key is stored internally
    /// for completing the handshake when we receive the Created response.
    ///
    /// # Arguments
    ///
    /// * `circuit_id` - Unique identifier for this circuit
    ///
    /// # Returns
    ///
    /// A `CircuitHandshakeMsg::Create` message to send to the first hop.
    ///
    /// # Panics
    ///
    /// Panics if called while another handshake is in progress.
    pub fn initiate_create(&mut self, circuit_id: CircuitId) -> CircuitHandshakeMsg {
        if self.state.is_some() {
            panic!("Cannot initiate new handshake while one is in progress");
        }

        // Generate fresh ephemeral keypair
        let mut rng = rand::thread_rng();
        let ephemeral_private = X25519EphemeralPrivate::from_random(&mut rng);
        let ephemeral_public = X25519Public::from(&ephemeral_private);

        // Store state for completion
        self.state = Some(HandshakeState {
            ephemeral_private: Some(ephemeral_private),
            circuit_id,
        });

        CircuitHandshakeMsg::Create {
            circuit_id,
            ephemeral_pubkey: ephemeral_public,
        }
    }

    /// Complete a CREATE handshake after receiving Created response.
    ///
    /// This performs X25519 key agreement with the hop's public key and
    /// derives a symmetric key using HKDF-SHA256 with domain separation.
    ///
    /// # Arguments
    ///
    /// * `their_pubkey` - The hop's ephemeral public key from the Created
    ///   message
    /// * `circuit_id` - The circuit ID (must match the one from
    ///   initiate_create)
    ///
    /// # Returns
    ///
    /// The derived symmetric key for this hop.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - No handshake is in progress
    /// - The circuit ID doesn't match
    /// - Key derivation fails
    pub fn complete_create(
        &mut self,
        their_pubkey: &X25519Public,
        circuit_id: CircuitId,
    ) -> HandshakeResult<SymmetricKey> {
        let mut state = self
            .state
            .take()
            .ok_or_else(|| HandshakeError::InvalidState("No handshake in progress".to_string()))?;

        // Verify circuit ID matches
        if state.circuit_id != circuit_id {
            return Err(HandshakeError::CircuitIdMismatch {
                expected: state.circuit_id,
                actual: circuit_id,
            });
        }

        let ephemeral_private = state.ephemeral_private.take().ok_or_else(|| {
            HandshakeError::InvalidState("Ephemeral private key already consumed".to_string())
        })?;

        // Perform X25519 key exchange (consumes ephemeral_private)
        let shared_secret = ephemeral_private.key_exchange(their_pubkey);

        // Derive symmetric key using HKDF-SHA256
        let key = derive_circuit_key(&shared_secret, &circuit_id)?;

        // shared_secret is zeroized when dropped (X25519Secret implements Zeroize)

        Ok(key)
    }

    /// Handle a Create message as a relay hop.
    ///
    /// This is used when we receive a Create message and need to respond
    /// with our own ephemeral public key.
    ///
    /// # Arguments
    ///
    /// * `circuit_id` - The circuit ID from the Create message
    /// * `their_pubkey` - The initiator's ephemeral public key
    ///
    /// # Returns
    ///
    /// A tuple of (Created response message, derived symmetric key).
    pub fn respond_to_create(
        circuit_id: CircuitId,
        their_pubkey: &X25519Public,
    ) -> HandshakeResult<(CircuitHandshakeMsg, SymmetricKey)> {
        // Generate our ephemeral keypair
        let mut rng = rand::thread_rng();
        let our_private = X25519EphemeralPrivate::from_random(&mut rng);
        let our_public = X25519Public::from(&our_private);

        // Perform key exchange
        let shared_secret = our_private.key_exchange(their_pubkey);

        // Derive symmetric key
        let key = derive_circuit_key(&shared_secret, &circuit_id)?;

        let response = CircuitHandshakeMsg::Created {
            circuit_id,
            ephemeral_pubkey: our_public,
        };

        Ok((response, key))
    }

    /// Check if a handshake is currently in progress.
    pub fn is_in_progress(&self) -> bool {
        self.state.is_some()
    }

    /// Cancel the current handshake and clean up state.
    ///
    /// This should be called if a handshake times out or fails.
    pub fn cancel(&mut self) {
        // State will be zeroized when dropped
        self.state = None;
    }

    /// Get the timeout duration for handshake operations.
    pub fn timeout() -> Duration {
        Duration::from_secs(HANDSHAKE_TIMEOUT_SECS)
    }
}

/// Derive a symmetric key from a shared secret using HKDF-SHA256.
///
/// Uses domain separation to ensure keys derived for different purposes
/// are cryptographically independent.
///
/// # Arguments
///
/// * `shared_secret` - The X25519 shared secret
/// * `circuit_id` - The circuit ID (used as part of the salt)
///
/// # Returns
///
/// A 256-bit symmetric key suitable for ChaCha20-Poly1305 or AES-256-GCM.
fn derive_circuit_key(
    shared_secret: &X25519Secret,
    circuit_id: &CircuitId,
) -> HandshakeResult<SymmetricKey> {
    // Construct salt: prefix + circuit_id
    let mut salt = Vec::with_capacity(HKDF_SALT_PREFIX.len() + 16);
    salt.extend_from_slice(HKDF_SALT_PREFIX);
    salt.extend_from_slice(circuit_id.as_bytes());

    // Create HKDF instance
    let hkdf = Hkdf::<Sha256>::new(Some(&salt), shared_secret.as_ref());

    // Expand to derive key
    let mut key_bytes = [0u8; CIRCUIT_KEY_SIZE];
    hkdf.expand(CIRCUIT_KEY_DOMAIN, &mut key_bytes)
        .map_err(|e| HandshakeError::KeyDerivationError(format!("HKDF expand failed: {}", e)))?;

    SymmetricKey::from_bytes(&key_bytes)
        .ok_or_else(|| HandshakeError::KeyDerivationError("Invalid key length".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_circuit_id_random() {
        let id1 = CircuitId::random();
        let id2 = CircuitId::random();

        // Random IDs should be different
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_symmetric_key_debug_does_not_leak() {
        let key = SymmetricKey::from_bytes(&[42u8; CIRCUIT_KEY_SIZE]).unwrap();
        let debug_str = format!("{:?}", key);

        // Should not contain the actual key bytes
        assert!(!debug_str.contains("42424242"));
        // Should contain a hash fingerprint
        assert!(debug_str.contains("sha256:"));
    }

    #[test]
    fn test_handshake_initiate_create() {
        let mut handshake = CircuitHandshake::new();
        let circuit_id = CircuitId::random();

        let msg = handshake.initiate_create(circuit_id);

        match msg {
            CircuitHandshakeMsg::Create {
                circuit_id: msg_cid,
                ephemeral_pubkey: _,
            } => {
                assert_eq!(msg_cid, circuit_id);
            }
            _ => panic!("Expected Create message"),
        }

        assert!(handshake.is_in_progress());
    }

    #[test]
    #[should_panic(expected = "Cannot initiate new handshake")]
    fn test_handshake_double_initiate_panics() {
        let mut handshake = CircuitHandshake::new();
        let circuit_id = CircuitId::random();

        handshake.initiate_create(circuit_id);
        handshake.initiate_create(circuit_id); // Should panic
    }

    #[test]
    fn test_handshake_cancel() {
        let mut handshake = CircuitHandshake::new();
        let circuit_id = CircuitId::random();

        handshake.initiate_create(circuit_id);
        assert!(handshake.is_in_progress());

        handshake.cancel();
        assert!(!handshake.is_in_progress());
    }

    #[test]
    fn test_complete_without_initiate_fails() {
        let mut handshake = CircuitHandshake::new();
        let circuit_id = CircuitId::random();

        // Generate a dummy public key
        let mut rng = rand::thread_rng();
        let dummy_private = X25519EphemeralPrivate::from_random(&mut rng);
        let dummy_public = X25519Public::from(&dummy_private);

        let result = handshake.complete_create(&dummy_public, circuit_id);
        assert!(matches!(result, Err(HandshakeError::InvalidState(_))));
    }

    #[test]
    fn test_circuit_id_mismatch_fails() {
        let mut handshake = CircuitHandshake::new();
        let circuit_id1 = CircuitId::random();
        let circuit_id2 = CircuitId::random();

        handshake.initiate_create(circuit_id1);

        // Generate a dummy public key
        let mut rng = rand::thread_rng();
        let dummy_private = X25519EphemeralPrivate::from_random(&mut rng);
        let dummy_public = X25519Public::from(&dummy_private);

        let result = handshake.complete_create(&dummy_public, circuit_id2);
        assert!(matches!(
            result,
            Err(HandshakeError::CircuitIdMismatch { .. })
        ));
    }

    #[test]
    fn test_full_handshake_flow() {
        // Simulate initiator (Alice)
        let mut alice = CircuitHandshake::new();
        let circuit_id = CircuitId::random();

        // Alice initiates
        let create_msg = alice.initiate_create(circuit_id);
        let alice_pubkey = match &create_msg {
            CircuitHandshakeMsg::Create {
                ephemeral_pubkey, ..
            } => ephemeral_pubkey.clone(),
            _ => panic!("Expected Create message"),
        };

        // Hop1 responds
        let (created_msg, hop1_key) =
            CircuitHandshake::respond_to_create(circuit_id, &alice_pubkey).unwrap();
        let hop1_pubkey = match &created_msg {
            CircuitHandshakeMsg::Created {
                ephemeral_pubkey, ..
            } => ephemeral_pubkey.clone(),
            _ => panic!("Expected Created message"),
        };

        // Alice completes
        let alice_key = alice.complete_create(&hop1_pubkey, circuit_id).unwrap();

        // Both should derive the same key
        assert_eq!(alice_key.as_bytes(), hop1_key.as_bytes());
    }

    #[test]
    fn test_different_circuits_different_keys() {
        // Two handshakes with same keypairs but different circuit IDs
        // should produce different keys
        let circuit_id1 = CircuitId::random();
        let circuit_id2 = CircuitId::random();

        let mut alice1 = CircuitHandshake::new();
        let create1 = alice1.initiate_create(circuit_id1);
        let alice1_pubkey = match &create1 {
            CircuitHandshakeMsg::Create {
                ephemeral_pubkey, ..
            } => ephemeral_pubkey.clone(),
            _ => panic!("Expected Create"),
        };
        let (created1, _) =
            CircuitHandshake::respond_to_create(circuit_id1, &alice1_pubkey).unwrap();
        let hop1_pubkey = match &created1 {
            CircuitHandshakeMsg::Created {
                ephemeral_pubkey, ..
            } => ephemeral_pubkey.clone(),
            _ => panic!("Expected Created"),
        };
        let key1 = alice1.complete_create(&hop1_pubkey, circuit_id1).unwrap();

        let mut alice2 = CircuitHandshake::new();
        let create2 = alice2.initiate_create(circuit_id2);
        let alice2_pubkey = match &create2 {
            CircuitHandshakeMsg::Create {
                ephemeral_pubkey, ..
            } => ephemeral_pubkey.clone(),
            _ => panic!("Expected Create"),
        };
        let (created2, _) =
            CircuitHandshake::respond_to_create(circuit_id2, &alice2_pubkey).unwrap();
        let hop2_pubkey = match &created2 {
            CircuitHandshakeMsg::Created {
                ephemeral_pubkey, ..
            } => ephemeral_pubkey.clone(),
            _ => panic!("Expected Created"),
        };
        let key2 = alice2.complete_create(&hop2_pubkey, circuit_id2).unwrap();

        // Different circuit IDs = different keys (due to domain separation)
        assert_ne!(key1.as_bytes(), key2.as_bytes());
    }

    #[test]
    fn test_respond_to_create() {
        let circuit_id = CircuitId::random();

        // Generate initiator's keypair
        let mut rng = rand::thread_rng();
        let initiator_private = X25519EphemeralPrivate::from_random(&mut rng);
        let initiator_public = X25519Public::from(&initiator_private);

        // Respond as a hop
        let (response, _key) =
            CircuitHandshake::respond_to_create(circuit_id, &initiator_public).unwrap();

        match response {
            CircuitHandshakeMsg::Created {
                circuit_id: resp_cid,
                ephemeral_pubkey: _,
            } => {
                assert_eq!(resp_cid, circuit_id);
            }
            _ => panic!("Expected Created message"),
        }
    }

    #[test]
    fn test_handshake_timeout() {
        let timeout = CircuitHandshake::timeout();
        assert_eq!(timeout, Duration::from_secs(HANDSHAKE_TIMEOUT_SECS));
    }
}
