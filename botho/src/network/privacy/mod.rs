// Copyright (c) 2024 Botho Foundation

//! Privacy-preserving network layer for traffic analysis resistance.
//!
//! This module implements the Onion Gossip protocol, which merges onion routing
//! with gossipsub at the protocol level. Every node participates as both sender
//! and relay, eliminating the distinction between users and relays.
//!
//! # Design Philosophy
//!
//! > "In a privacy network, there should be no special nodes. Every participant
//! > is equal."
//!
//! # Modules
//!
//! - [`crypto`]: Onion encryption and decryption primitives

mod crypto;

pub use crypto::{
    decrypt_layer, encrypt_exit_layer, encrypt_forward_layer, wrap_onion, CircuitId, CryptoError,
    DecryptedLayer, LayerType, OnionMessage, SymmetricKey, MAX_PEER_ID_SIZE, MIN_LAYER_SIZE,
    NONCE_SIZE, TAG_SIZE,
};
