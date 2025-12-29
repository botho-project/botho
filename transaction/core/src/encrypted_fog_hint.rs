// Copyright (c) 2018-2022 The Botho Foundation
// Copyright (c) 2024 Botho Foundation

//! Stub module for EncryptedFogHint.
//!
//! Fog support has been removed from Botho. This module provides a minimal
//! stub type for backwards compatibility with serialized data and APIs.

use alloc::vec::Vec;
use bth_crypto_digestible::Digestible;
use bth_util_repr_bytes::{
    derive_prost_message_from_repr_bytes, derive_repr_bytes_from_as_ref_and_try_from,
    typenum::U128, GenericArray, LengthMismatch,
};
use serde::{Deserialize, Serialize};

/// Length of the encrypted fog hint field (for backwards compatibility)
pub const ENCRYPTED_FOG_HINT_LEN: usize = 128;

/// A stub type for encrypted fog hints.
///
/// Fog support has been removed. This type exists only for backwards
/// compatibility with serialized transactions and APIs.
#[derive(Clone, Debug, Default, Deserialize, Digestible, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[digestible(transparent)]
pub struct EncryptedFogHint {
    data: GenericArray<u8, U128>,
}

impl EncryptedFogHint {
    /// Create a new EncryptedFogHint from raw bytes
    pub fn new(data: &[u8; ENCRYPTED_FOG_HINT_LEN]) -> Self {
        Self {
            data: GenericArray::clone_from_slice(data),
        }
    }

    /// Generate a fake "onetime hint" (all zeros - fog is deprecated)
    pub fn fake_onetime_hint<R>(_rng: &mut R) -> Self {
        Self::default()
    }
}

impl AsRef<[u8]> for EncryptedFogHint {
    fn as_ref(&self) -> &[u8] {
        self.data.as_slice()
    }
}

impl From<&[u8; ENCRYPTED_FOG_HINT_LEN]> for EncryptedFogHint {
    fn from(src: &[u8; ENCRYPTED_FOG_HINT_LEN]) -> Self {
        Self::new(src)
    }
}

impl TryFrom<&[u8]> for EncryptedFogHint {
    type Error = LengthMismatch;

    fn try_from(src: &[u8]) -> Result<Self, Self::Error> {
        if src.len() == ENCRYPTED_FOG_HINT_LEN {
            let arr: [u8; ENCRYPTED_FOG_HINT_LEN] = src.try_into().map_err(|_| LengthMismatch {
                expected: ENCRYPTED_FOG_HINT_LEN,
                found: src.len(),
            })?;
            Ok(Self::new(&arr))
        } else {
            Err(LengthMismatch {
                expected: ENCRYPTED_FOG_HINT_LEN,
                found: src.len(),
            })
        }
    }
}

impl TryFrom<&Vec<u8>> for EncryptedFogHint {
    type Error = LengthMismatch;

    fn try_from(src: &Vec<u8>) -> Result<Self, Self::Error> {
        Self::try_from(src.as_slice())
    }
}

derive_repr_bytes_from_as_ref_and_try_from!(EncryptedFogHint, U128);
derive_prost_message_from_repr_bytes!(EncryptedFogHint);
