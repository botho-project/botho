// Copyright (c) 2018-2022 The MobileCoin Foundation
// Copyright (c) 2024 Cadence Foundation

//! Token governors map type.
//!
//! This type maps token IDs to the set of signers (governors) authorized
//! to mint that token. Previously defined in the enclave API, but not
//! inherently SGX-specific.

use displaydoc::Display;
use mc_crypto_keys::Ed25519Public;
use mc_crypto_multisig::SignerSet;
use mc_transaction_core::TokenId;
use std::collections::HashMap;

/// A map from token ID to the set of governors (signers) authorized for that token.
#[derive(Clone, Debug, Default)]
pub struct GovernorsMap {
    inner: HashMap<TokenId, SignerSet<Ed25519Public>>,
}

impl GovernorsMap {
    /// Create a new empty GovernorsMap.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a GovernorsMap from an iterator of (TokenId, SignerSet) pairs.
    pub fn try_from_iter<I>(iter: I) -> Result<Self, GovernorsMapError>
    where
        I: IntoIterator<Item = (TokenId, SignerSet<Ed25519Public>)>,
    {
        let inner: HashMap<_, _> = iter.into_iter().collect();
        Ok(Self { inner })
    }

    /// Get the signer set for a token ID.
    pub fn get(&self, token_id: &TokenId) -> Option<&SignerSet<Ed25519Public>> {
        self.inner.get(token_id)
    }

    /// Check if the map contains a token ID.
    pub fn contains_key(&self, token_id: &TokenId) -> bool {
        self.inner.contains_key(token_id)
    }

    /// Iterate over all entries.
    pub fn iter(&self) -> impl Iterator<Item = (&TokenId, &SignerSet<Ed25519Public>)> {
        self.inner.iter()
    }

    /// Get the number of entries in the map.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Check if the map is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

/// Error type for GovernorsMap operations.
#[derive(Clone, Debug, Display, Eq, PartialEq)]
pub enum GovernorsMapError {
    /// Duplicate token ID
    DuplicateTokenId,
    /// Invalid signer set
    InvalidSignerSet,
}

impl std::error::Error for GovernorsMapError {}
