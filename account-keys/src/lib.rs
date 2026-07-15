#![no_std]
#![deny(missing_docs)]
#![deny(unsafe_code)]

//! This crate defines account key structures, including private account keys,
//! public addresses, view keys, and subaddresses.
//! It also defines their serialization as protobufs.
//!
//! # Quantum-Safe Keys (Optional)
//!
//! When the `pq` feature is enabled, this crate also provides a quantum-safe
//! account key type ([`QuantumSafeAccountKey`]) that combines classical
//! (Ristretto/Schnorr) keys with post-quantum (ML-KEM/ML-DSA) keys for
//! protection against future quantum computers. The post-quantum public keys
//! are folded directly into the canonical [`PublicAddress`] (address format
//! v2, ADR 0008) rather than a separate address type.

extern crate alloc;

mod account_keys;
mod burn_address;
mod domain_separators;
mod error;
mod identity;

#[cfg(feature = "pq")]
mod quantum_safe;

pub use crate::{
    account_keys::{
        AccountKey, PublicAddress, ShortAddressHash, ViewAccountKey, CHANGE_SUBADDRESS_INDEX,
        DEFAULT_SUBADDRESS_INDEX, GIFT_CODE_SUBADDRESS_INDEX, INVALID_SUBADDRESS_INDEX,
        ML_DSA_65_PUBLIC_KEY_LEN, ML_KEM_768_PUBLIC_KEY_LEN,
    },
    burn_address::{burn_address, burn_address_view_private, BURN_ADDRESS_VIEW_PRIVATE_BYTES},
    error::{Error, Result},
    identity::{RootEntropy, RootIdentity},
};

#[cfg(feature = "pq")]
pub use crate::quantum_safe::QuantumSafeAccountKey;
