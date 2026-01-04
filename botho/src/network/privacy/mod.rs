// Copyright (c) 2024 Botho Foundation

//! Privacy-preserving network layer for traffic analysis resistance.
//!
//! This module implements the Onion Gossip protocol, which provides:
//!
//! - **Sender Anonymity**: Onion routing through 3 random peers
//! - **Relationship Anonymity**: Observers can't link sender to transaction
//! - **Traffic Uniformity**: Padding and cover traffic normalize patterns
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                    UNIFIED PRIVACY ARCHITECTURE                          │
//! ├─────────────────────────────────────────────────────────────────────────┤
//! │                                                                         │
//! │                        ┌─────────────────────┐                          │
//! │                        │  EVERY NODE IS THE  │                          │
//! │                        │       SAME          │                          │
//! │                        │                     │                          │
//! │                        │  • Sends traffic    │                          │
//! │                        │  • Relays traffic   │                          │
//! │                        │  • Receives traffic │                          │
//! │                        └──────────┬──────────┘                          │
//! │                                   │                                     │
//! │         ┌─────────────────────────┼─────────────────────────┐           │
//! │         │                         │                         │           │
//! │         ▼                         ▼                         ▼           │
//! │   ┌───────────┐           ┌───────────────┐          ┌────────────┐    │
//! │   │   FAST    │           │    PRIVATE    │          │  PROTOCOL  │    │
//! │   │   PATH    │           │     PATH      │          │ OBFUSCATION│    │
//! │   ├───────────┤           ├───────────────┤          ├────────────┤    │
//! │   │ Direct    │           │ Onion Gossip  │          │ WebRTC     │    │
//! │   │ Gossipsub │           │ (3-hop relay) │          │ Framing    │    │
//! │   └───────────┘           └───────────────┘          └────────────┘    │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Usage
//!
//! ```ignore
//! use botho::network::privacy::{CircuitHandshake, SymmetricKey};
//!
//! // Create a new handshake for circuit building
//! let mut handshake = CircuitHandshake::new();
//!
//! // Perform direct handshake with first hop
//! let key1 = handshake.initiate_create(circuit_id);
//! // ... send to first hop and receive Created response ...
//! let symmetric_key1 = handshake.complete_create(&their_pubkey, circuit_id)?;
//!
//! // Extend to second hop through first hop
//! // ... create encrypted Create message for hop2 ...
//! ```

pub mod handshake;

pub use handshake::{
    CircuitHandshake, HandshakeError, HandshakeResult, SymmetricKey, CIRCUIT_KEY_SIZE,
    HANDSHAKE_TIMEOUT_SECS,
};
