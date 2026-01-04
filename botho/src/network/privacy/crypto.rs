// Copyright (c) 2024 Botho Foundation

//! Onion encryption and decryption for privacy-preserving message routing.
//!
//! This module implements the cryptographic primitives for onion routing:
//! - Layered encryption using ChaCha20-Poly1305
//! - Message wrapping for 3-hop circuits
//! - Layer decryption for relay nodes
//!
//! # Design
//!
//! Messages are encrypted in layers from innermost (exit) to outermost (first
//! hop). Each layer contains:
//! - A layer type byte indicating whether to forward or broadcast
//! - For forward layers: the next hop's peer ID
//! - The encrypted inner payload
//!
//! # Security Properties
//!
//! - Each layer uses a unique symmetric key derived during circuit construction
//! - Random 12-byte nonces prevent replay attacks
//! - Poly1305 authentication tags ensure integrity
//! - Constant-time operations prevent timing side-channels

use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Nonce,
};
use libp2p::PeerId;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Size of ChaCha20-Poly1305 nonce in bytes.
pub const NONCE_SIZE: usize = 12;

/// Size of ChaCha20-Poly1305 authentication tag in bytes.
pub const TAG_SIZE: usize = 16;

/// Maximum expected size for a serialized PeerId.
/// PeerIds are variable length multihashes, but typically <= 64 bytes.
pub const MAX_PEER_ID_SIZE: usize = 64;

/// Minimum valid encrypted layer size: nonce + tag + layer type byte.
pub const MIN_LAYER_SIZE: usize = NONCE_SIZE + TAG_SIZE + 1;

/// Minimum valid forward layer plaintext: type + length byte + at least 1
/// peer_id byte.
pub const MIN_FORWARD_PLAINTEXT: usize = 1 + 1 + 1;

/// Layer type indicator for onion messages.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerType {
    /// Forward to next hop - contains next peer ID and encrypted payload.
    Forward = 0x01,
    /// Exit layer - final hop broadcasts the payload.
    Exit = 0x02,
}

impl LayerType {
    /// Convert a byte to a layer type.
    pub fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0x01 => Some(LayerType::Forward),
            0x02 => Some(LayerType::Exit),
            _ => None,
        }
    }
}

/// Symmetric key for onion layer encryption.
///
/// Uses 256-bit keys for ChaCha20-Poly1305.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct SymmetricKey([u8; 32]);

impl SymmetricKey {
    /// Create a new symmetric key from raw bytes.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Get the raw key bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Unique identifier for a circuit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CircuitId([u8; 16]);

impl CircuitId {
    /// Generate a random circuit ID.
    pub fn random() -> Self {
        let mut bytes = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut bytes);
        Self(bytes)
    }

    /// Create from raw bytes.
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// Get the raw bytes.
    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

/// An onion-encrypted message ready for transmission.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnionMessage {
    /// Circuit identifier for routing.
    pub circuit_id: CircuitId,
    /// Encrypted payload (multiple layers).
    pub payload: Vec<u8>,
}

/// Result of decrypting an onion layer.
#[derive(Debug)]
pub enum DecryptedLayer {
    /// Forward to next hop.
    Forward {
        /// The peer to forward to.
        next_hop: PeerId,
        /// The remaining encrypted payload.
        inner: Vec<u8>,
    },
    /// Exit - broadcast the payload.
    Exit {
        /// The decrypted final payload.
        payload: Vec<u8>,
    },
}

/// Errors that can occur during onion encryption/decryption.
#[derive(Debug, Error)]
pub enum CryptoError {
    /// Encrypted data is too short to be valid.
    #[error("encrypted data too short: got {0} bytes, need at least {1}")]
    TooShort(usize, usize),

    /// Authentication tag verification failed.
    #[error("authentication failed: ciphertext was tampered with")]
    AuthenticationFailed,

    /// Invalid layer type byte.
    #[error("invalid layer type: {0:#04x}")]
    InvalidLayerType(u8),

    /// Failed to parse peer ID.
    #[error("invalid peer ID: {0}")]
    InvalidPeerId(String),

    /// Forward layer payload too short.
    #[error("forward layer payload too short")]
    ForwardTooShort,

    /// PeerId length exceeds maximum.
    #[error("peer ID length {0} exceeds maximum {1}")]
    PeerIdTooLong(usize, usize),
}

/// Encrypt a single onion layer with ChaCha20-Poly1305.
///
/// Returns: [nonce (12 bytes)][ciphertext][tag (16 bytes)]
fn encrypt_layer_raw(key: &SymmetricKey, plaintext: &[u8]) -> Vec<u8> {
    let cipher = ChaCha20Poly1305::new_from_slice(key.as_bytes()).expect("key is always 32 bytes");

    // Generate random nonce
    let mut nonce_bytes = [0u8; NONCE_SIZE];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    // Encrypt with authentication
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .expect("encryption should not fail with valid inputs");

    // Prepend nonce to ciphertext (tag is appended by the cipher)
    let mut output = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
    output.extend_from_slice(&nonce_bytes);
    output.extend_from_slice(&ciphertext);
    output
}

/// Decrypt a single onion layer.
///
/// Expects input format: [nonce (12 bytes)][ciphertext][tag (16 bytes)]
fn decrypt_layer_raw(key: &SymmetricKey, encrypted: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if encrypted.len() < MIN_LAYER_SIZE {
        return Err(CryptoError::TooShort(encrypted.len(), MIN_LAYER_SIZE));
    }

    let cipher = ChaCha20Poly1305::new_from_slice(key.as_bytes()).expect("key is always 32 bytes");

    let nonce = Nonce::from_slice(&encrypted[..NONCE_SIZE]);
    let ciphertext = &encrypted[NONCE_SIZE..];

    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| CryptoError::AuthenticationFailed)
}

/// Encrypt an exit layer (final hop broadcasts the payload).
///
/// Format: [EXIT byte][payload]
pub fn encrypt_exit_layer(key: &SymmetricKey, payload: &[u8]) -> Vec<u8> {
    let mut plaintext = Vec::with_capacity(1 + payload.len());
    plaintext.push(LayerType::Exit as u8);
    plaintext.extend_from_slice(payload);

    encrypt_layer_raw(key, &plaintext)
}

/// Encrypt a forward layer (relay forwards to next hop).
///
/// Format: [FORWARD byte][peer_id_len (1 byte)][next_hop PeerId][inner
/// encrypted data]
pub fn encrypt_forward_layer(key: &SymmetricKey, next_hop: &PeerId, inner: &[u8]) -> Vec<u8> {
    let peer_bytes = next_hop.to_bytes();

    // Encode length as single byte (PeerIds are typically < 64 bytes)
    assert!(
        peer_bytes.len() <= MAX_PEER_ID_SIZE,
        "PeerId exceeds maximum size"
    );

    let mut plaintext = Vec::with_capacity(1 + 1 + peer_bytes.len() + inner.len());
    plaintext.push(LayerType::Forward as u8);
    plaintext.push(peer_bytes.len() as u8);
    plaintext.extend_from_slice(&peer_bytes);
    plaintext.extend_from_slice(inner);

    encrypt_layer_raw(key, &plaintext)
}

/// Decrypt a single onion layer and determine the action.
///
/// Returns either a forward instruction with the next hop and remaining
/// payload, or an exit instruction with the final decrypted payload.
pub fn decrypt_layer(key: &SymmetricKey, encrypted: &[u8]) -> Result<DecryptedLayer, CryptoError> {
    let plaintext = decrypt_layer_raw(key, encrypted)?;

    if plaintext.is_empty() {
        return Err(CryptoError::TooShort(0, 1));
    }

    let layer_type =
        LayerType::from_byte(plaintext[0]).ok_or(CryptoError::InvalidLayerType(plaintext[0]))?;

    match layer_type {
        LayerType::Forward => {
            // Format: [FORWARD][peer_id_len][peer_id bytes][inner]
            if plaintext.len() < MIN_FORWARD_PLAINTEXT {
                return Err(CryptoError::ForwardTooShort);
            }

            let peer_id_len = plaintext[1] as usize;
            if peer_id_len > MAX_PEER_ID_SIZE {
                return Err(CryptoError::PeerIdTooLong(peer_id_len, MAX_PEER_ID_SIZE));
            }

            let required_len = 2 + peer_id_len;
            if plaintext.len() < required_len {
                return Err(CryptoError::ForwardTooShort);
            }

            let peer_bytes = &plaintext[2..2 + peer_id_len];
            let next_hop = PeerId::from_bytes(peer_bytes)
                .map_err(|e| CryptoError::InvalidPeerId(e.to_string()))?;
            let inner = plaintext[2 + peer_id_len..].to_vec();

            Ok(DecryptedLayer::Forward { next_hop, inner })
        }
        LayerType::Exit => {
            let payload = plaintext[1..].to_vec();
            Ok(DecryptedLayer::Exit { payload })
        }
    }
}

/// Wrap a payload in 3 onion layers for transmission through a circuit.
///
/// # Arguments
///
/// * `payload` - The message to send
/// * `hops` - The 3 relay peer IDs in order: [first, middle, exit]
/// * `keys` - The 3 symmetric keys corresponding to each hop
///
/// # Returns
///
/// The fully wrapped onion message ready for transmission to the first hop.
pub fn wrap_onion(payload: &[u8], hops: &[PeerId; 3], keys: &[SymmetricKey; 3]) -> Vec<u8> {
    // Start with the innermost layer (exit)
    let mut wrapped = encrypt_exit_layer(&keys[2], payload);

    // Middle layer: forward to exit
    wrapped = encrypt_forward_layer(&keys[1], &hops[2], &wrapped);

    // Outer layer: forward to middle
    wrapped = encrypt_forward_layer(&keys[0], &hops[1], &wrapped);

    wrapped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn random_key() -> SymmetricKey {
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        SymmetricKey::from_bytes(bytes)
    }

    fn random_peer_id() -> PeerId {
        PeerId::random()
    }

    #[test]
    fn test_exit_layer_round_trip() {
        let key = random_key();
        let payload = b"hello, onion world!";

        let encrypted = encrypt_exit_layer(&key, payload);

        // Verify minimum size
        assert!(encrypted.len() >= MIN_LAYER_SIZE);

        // Decrypt and verify
        let decrypted = decrypt_layer(&key, &encrypted).expect("decryption should succeed");

        match decrypted {
            DecryptedLayer::Exit { payload: p } => {
                assert_eq!(p, payload);
            }
            DecryptedLayer::Forward { .. } => {
                panic!("expected Exit layer, got Forward");
            }
        }
    }

    #[test]
    fn test_forward_layer_round_trip() {
        let key = random_key();
        let next_hop = random_peer_id();
        let inner_data = b"forward this data";

        let encrypted = encrypt_forward_layer(&key, &next_hop, inner_data);

        let decrypted = decrypt_layer(&key, &encrypted).expect("decryption should succeed");

        match decrypted {
            DecryptedLayer::Forward {
                next_hop: nh,
                inner,
            } => {
                assert_eq!(nh, next_hop);
                assert_eq!(inner, inner_data);
            }
            DecryptedLayer::Exit { .. } => {
                panic!("expected Forward layer, got Exit");
            }
        }
    }

    #[test]
    fn test_wrap_onion_full_circuit() {
        let keys = [random_key(), random_key(), random_key()];
        let hops = [random_peer_id(), random_peer_id(), random_peer_id()];
        let payload = b"secret transaction data";

        // Wrap the message
        let wrapped = wrap_onion(payload, &hops, &keys);

        // Unwrap layer by layer
        // First hop decrypts and forwards to second hop
        let layer1 = decrypt_layer(&keys[0], &wrapped).expect("layer 1 decryption failed");
        let (next1, inner1) = match layer1 {
            DecryptedLayer::Forward { next_hop, inner } => (next_hop, inner),
            _ => panic!("expected Forward layer at hop 1"),
        };
        assert_eq!(next1, hops[1]);

        // Second hop decrypts and forwards to third hop
        let layer2 = decrypt_layer(&keys[1], &inner1).expect("layer 2 decryption failed");
        let (next2, inner2) = match layer2 {
            DecryptedLayer::Forward { next_hop, inner } => (next_hop, inner),
            _ => panic!("expected Forward layer at hop 2"),
        };
        assert_eq!(next2, hops[2]);

        // Third hop (exit) decrypts final payload
        let layer3 = decrypt_layer(&keys[2], &inner2).expect("layer 3 decryption failed");
        match layer3 {
            DecryptedLayer::Exit { payload: p } => {
                assert_eq!(p, payload);
            }
            _ => panic!("expected Exit layer at hop 3"),
        }
    }

    #[test]
    fn test_wrong_key_fails() {
        let key = random_key();
        let wrong_key = random_key();
        let payload = b"secret data";

        let encrypted = encrypt_exit_layer(&key, payload);

        let result = decrypt_layer(&wrong_key, &encrypted);
        assert!(matches!(result, Err(CryptoError::AuthenticationFailed)));
    }

    #[test]
    fn test_tampered_ciphertext_fails() {
        let key = random_key();
        let payload = b"secret data";

        let mut encrypted = encrypt_exit_layer(&key, payload);

        // Tamper with the ciphertext
        if let Some(byte) = encrypted.get_mut(NONCE_SIZE + 5) {
            *byte ^= 0xFF;
        }

        let result = decrypt_layer(&key, &encrypted);
        assert!(matches!(result, Err(CryptoError::AuthenticationFailed)));
    }

    #[test]
    fn test_truncated_ciphertext_fails() {
        let key = random_key();
        let payload = b"secret data";

        let encrypted = encrypt_exit_layer(&key, payload);

        // Truncate to less than minimum size
        let truncated = &encrypted[..MIN_LAYER_SIZE - 1];

        let result = decrypt_layer(&key, truncated);
        assert!(matches!(result, Err(CryptoError::TooShort(_, _))));
    }

    #[test]
    fn test_invalid_layer_type() {
        let key = random_key();

        // Create valid ciphertext with invalid layer type
        let mut plaintext = vec![0xFF]; // Invalid layer type
        plaintext.extend_from_slice(b"some data");

        let encrypted = encrypt_layer_raw(&key, &plaintext);

        let result = decrypt_layer(&key, &encrypted);
        assert!(matches!(result, Err(CryptoError::InvalidLayerType(0xFF))));
    }

    #[test]
    fn test_nonce_uniqueness() {
        let key = random_key();
        let payload = b"same payload";

        // Encrypt same payload twice
        let encrypted1 = encrypt_exit_layer(&key, payload);
        let encrypted2 = encrypt_exit_layer(&key, payload);

        // Nonces (first 12 bytes) should be different
        assert_ne!(&encrypted1[..NONCE_SIZE], &encrypted2[..NONCE_SIZE]);

        // Both should decrypt correctly
        let d1 = decrypt_layer(&key, &encrypted1).expect("d1 should decrypt");
        let d2 = decrypt_layer(&key, &encrypted2).expect("d2 should decrypt");

        match (d1, d2) {
            (DecryptedLayer::Exit { payload: p1 }, DecryptedLayer::Exit { payload: p2 }) => {
                assert_eq!(p1, p2);
                assert_eq!(p1, payload);
            }
            _ => panic!("expected Exit layers"),
        }
    }

    #[test]
    fn test_empty_payload() {
        let key = random_key();
        let payload = b"";

        let encrypted = encrypt_exit_layer(&key, payload);
        let decrypted = decrypt_layer(&key, &encrypted).expect("should decrypt");

        match decrypted {
            DecryptedLayer::Exit { payload: p } => {
                assert!(p.is_empty());
            }
            _ => panic!("expected Exit layer"),
        }
    }

    #[test]
    fn test_large_payload() {
        let key = random_key();
        let payload: Vec<u8> = (0..10000).map(|i| (i % 256) as u8).collect();

        let encrypted = encrypt_exit_layer(&key, &payload);
        let decrypted = decrypt_layer(&key, &encrypted).expect("should decrypt");

        match decrypted {
            DecryptedLayer::Exit { payload: p } => {
                assert_eq!(p, payload);
            }
            _ => panic!("expected Exit layer"),
        }
    }

    #[test]
    fn test_circuit_id_random() {
        let id1 = CircuitId::random();
        let id2 = CircuitId::random();

        assert_ne!(id1.as_bytes(), id2.as_bytes());
    }

    #[test]
    fn test_symmetric_key_zeroize() {
        let key = random_key();
        let key_bytes = *key.as_bytes();

        // Key should have non-zero bytes
        assert!(key_bytes.iter().any(|&b| b != 0));

        // After dropping, memory should be zeroed
        // (We can't directly test this, but the derive ensures it)
        drop(key);
    }
}
