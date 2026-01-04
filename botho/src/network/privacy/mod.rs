// Copyright (c) 2024 Botho Foundation

//! Privacy layer for traffic analysis resistance using Onion Gossip.
//!
//! This module implements the core data structures for Phase 1 of the
//! traffic analysis resistance roadmap (see
//! `docs/design/traffic-privacy-roadmap.md`).
//!
//! # Overview
//!
//! Onion Gossip merges onion routing with gossipsub. Every transaction is
//! routed through a 3-hop circuit of randomly selected peers before being
//! broadcast. Every node participates as a potential relay.
//!
//! ## Key Concepts
//!
//! - **Circuit**: A 3-hop path through the relay network
//! - **Onion Encryption**: Each hop decrypts one layer of encryption
//! - **Relay**: Any node can relay traffic for others
//! - **Exit Hop**: The final hop broadcasts to gossipsub
//! - **Handshake**: X25519 key exchange to establish per-hop symmetric keys
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    ONION GOSSIP FLOW                        │
//! │                                                             │
//! │   Alice wants to broadcast transaction T                   │
//! │                                                             │
//! │   1. Build Circuit: Select 3 random peers [X, Y, Z]        │
//! │   2. Handshake: Establish symmetric keys with each hop     │
//! │   3. Onion Wrap: Encrypt_X(Encrypt_Y(Encrypt_Z(T)))        │
//! │   4. Send: Alice → X → Y → Z → Gossipsub                   │
//! │                                                             │
//! │   Result: No single node knows both origin AND content     │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Module Structure
//!
//! - [`types`]: Core types (CircuitId, SymmetricKey)
//! - [`crypto`]: Onion encryption and decryption primitives
//! - [`circuit`]: Outbound circuit management (OutboundCircuit, CircuitPool)
//! - [`relay`]: Relay state management (RelayState, CircuitHopKey)
//! - [`handshake`]: Circuit handshake protocol (X25519 key exchange)
//!
//! # Example
//!
//! ```
//! use botho::network::privacy::{
//!     CircuitId, SymmetricKey,
//!     OutboundCircuit, CircuitPool, CircuitPoolConfig,
//!     RelayState, RelayStateConfig, CircuitHopKey,
//! };
//! use libp2p::PeerId;
//! use std::time::Duration;
//!
//! // Create a circuit pool for managing outbound circuits
//! let mut pool = CircuitPool::new(CircuitPoolConfig::default());
//!
//! // Create relay state for handling incoming relay traffic
//! let mut relay = RelayState::new(RelayStateConfig::default());
//!
//! // When we become a relay hop, store the circuit key
//! let mut rng = rand::thread_rng();
//! let circuit_id = CircuitId::random(&mut rng);
//! let hop_key = CircuitHopKey::new_exit(SymmetricKey::random(&mut rng));
//! relay.add_circuit_key(circuit_id, hop_key);
//! ```
//!
//! # Security Considerations
//!
//! - All symmetric keys use [`zeroize`] for secure memory handling
//! - Circuit IDs are random 16-byte values to prevent prediction
//! - Per-peer rate limiting prevents relay flooding attacks
//! - Circuit rotation prevents long-term correlation
//! - Ephemeral X25519 keys are generated fresh for each handshake
//!
//! # References
//!
//! - Design document: `docs/design/traffic-privacy-roadmap.md`
//! - Parent issue: #147 (Traffic Analysis Resistance - Phase 1)

mod broadcaster;
mod circuit;
mod crypto;
pub mod handshake;
mod relay;
pub mod relay_handler;
mod types;

// Re-export core types
pub use types::{CircuitId, SymmetricKey, CIRCUIT_ID_LEN, SYMMETRIC_KEY_LEN};

// Re-export crypto primitives
pub use crypto::{
    decrypt_layer, encrypt_exit_layer, encrypt_forward_layer, wrap_onion, CryptoError,
    DecryptedLayer, LayerType, OnionMessage, MAX_PEER_ID_SIZE, MIN_LAYER_SIZE, NONCE_SIZE,
    TAG_SIZE,
};

// Re-export circuit management types
pub use circuit::{
    new_shared_pool, CircuitPool, CircuitPoolConfig, CircuitPoolMaintainer, CircuitPoolMetrics,
    MaintenanceResult, OutboundCircuit, SharedCircuitPool, CIRCUIT_HOPS,
    DEFAULT_MAINTENANCE_INTERVAL, DEFAULT_MIN_CIRCUITS, DEFAULT_REBUILD_THRESHOLD,
    DEFAULT_ROTATION_INTERVAL, MAX_LIFETIME_JITTER,
};

// Re-export relay management types
pub use relay::{
    CircuitHopKey, RateLimiter, RelayState, RelayStateConfig, DEFAULT_CIRCUIT_KEY_LIFETIME,
    DEFAULT_MAX_RELAY_PER_WINDOW, DEFAULT_RATE_LIMIT_WINDOW,
};

// Re-export handshake types
pub use handshake::{
    CircuitHandshake, HandshakeError, HandshakeResult, CIRCUIT_KEY_SIZE, HANDSHAKE_TIMEOUT_SECS,
};

// Re-export relay handler types
pub use relay_handler::{
    RelayAction, RelayHandler, RelayHandlerError, RelayMetrics, RelayMetricsSnapshot,
};

// Re-export broadcaster types
pub use broadcaster::{BroadcastError, BroadcastMetrics, BroadcastMetricsSnapshot, OnionBroadcaster};
