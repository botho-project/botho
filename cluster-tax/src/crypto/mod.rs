//! Cryptographic integration for cluster taxation.
//!
//! This module defines how cluster tags integrate with the existing
//! MobileCoin transaction model.
//!
//! ## Design Phases
//!
//! ### Phase 1: Public Tags
//! - Tags stored in plaintext alongside TxOut
//! - Privacy from ring signatures (which input is real is hidden)
//! - Simple validation of tag inheritance
//!
//! ### Phase 2: Committed Tags (Future)
//! - Tag masses as Pedersen commitments
//! - ZK proofs for tag inheritance
//! - Full privacy for tag distribution

mod tagged_output;
mod validation;

pub use tagged_output::{TaggedTxOut, CompactTagVector};
pub use validation::{validate_tag_inheritance, TagValidationError};
