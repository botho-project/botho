// Copyright (c) 2018-2022 The Botho Foundation

//! In cryptography, several private keys can be derived from a single source of
//! entropy using a strong KDF (key derivation function).
//! This is sound, so long as the input-key-material to the KDF itself has
//! at least enough entropy as the length of any one of the derived keys.
//!
//! The RootIdentity object contains 32 bytes of "root entropy", used with HKDF
//! to produce the other Botho private keys. This is useful because an
//! AccountKey derived this way can be represented with a smaller amount of
//! information.
//!
//! RootIdentity is used for account key derivation.

use crate::AccountKey;
use bth_crypto_hashes::Blake2b256;
use bth_crypto_keys::RistrettoPrivate;
use bth_util_from_random::FromRandom;
#[cfg(feature = "prost")]
use bth_util_repr_bytes::derive_prost_message_from_repr_bytes;
use bth_util_repr_bytes::{
    derive_debug_and_display_hex_from_as_ref, derive_repr_bytes_from_as_ref_and_try_from,
    typenum::U32, LengthMismatch,
};
use core::hash::Hash;
use curve25519_dalek::scalar::Scalar;
use hkdf::SimpleHkdf;

#[cfg(feature = "prost")]
use prost::Message;

use rand_core::{CryptoRng, RngCore};
use zeroize::Zeroize;

/// A secret value used as input key material to derive private keys.
#[derive(Clone, Default, PartialEq, Eq, Hash, Zeroize)]
#[zeroize(drop)]
pub struct RootEntropy {
    /// 32 bytes of input key material.
    /// Should be e.g. RDRAND, /dev/random/, or from properly seeded CSPRNG.
    pub bytes: [u8; 32],
}

impl AsRef<[u8]> for RootEntropy {
    fn as_ref(&self) -> &[u8] {
        &self.bytes[..]
    }
}

impl From<&[u8; 32]> for RootEntropy {
    fn from(src: &[u8; 32]) -> Self {
        Self { bytes: *src }
    }
}

impl TryFrom<&[u8]> for RootEntropy {
    type Error = LengthMismatch;

    fn try_from(src: &[u8]) -> Result<RootEntropy, LengthMismatch> {
        if src.len() == 32 {
            let mut result = Self { bytes: [0u8; 32] };
            result.bytes.copy_from_slice(src);
            Ok(result)
        } else {
            Err(LengthMismatch {
                expected: 32,
                found: src.len(),
            })
        }
    }
}

impl FromRandom for RootEntropy {
    fn from_random<T: RngCore + CryptoRng>(rng: &mut T) -> Self {
        let mut result = Self { bytes: [0u8; 32] };
        rng.fill_bytes(&mut result.bytes);
        result
    }
}

derive_repr_bytes_from_as_ref_and_try_from!(RootEntropy, U32);
derive_debug_and_display_hex_from_as_ref!(RootEntropy);

#[cfg(feature = "prost")]
derive_prost_message_from_repr_bytes!(RootEntropy);

/// A RootIdentity contains 32 bytes of root entropy for deriving private keys
/// using a KDF.
#[derive(Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "prost", derive(Message))]
pub struct RootIdentity {
    /// Root entropy used to derive a user's private keys.
    #[cfg_attr(feature = "prost", prost(message, required, tag = 1))]
    pub root_entropy: RootEntropy,
}

// Make RootIdentity from RootEntropy.
impl From<&RootEntropy> for RootIdentity {
    fn from(src: &RootEntropy) -> Self {
        Self {
            root_entropy: src.clone(),
        }
    }
}

/// Generate a random root identity
impl FromRandom for RootIdentity {
    fn from_random<T: RngCore + CryptoRng>(rng: &mut T) -> Self {
        Self::from(&RootEntropy::from_random(rng))
    }
}

/// Derive an AccountKey from RootIdentity
impl From<&RootIdentity> for AccountKey {
    fn from(src: &RootIdentity) -> Self {
        let spend_private_key = RistrettoPrivate::from(root_identity_hkdf_helper(
            src.root_entropy.as_ref(),
            b"spend",
        ));
        let view_private_key = RistrettoPrivate::from(root_identity_hkdf_helper(
            src.root_entropy.as_ref(),
            b"view",
        ));
        AccountKey::new(&spend_private_key, &view_private_key)
    }
}

/// Construct RootIdentity from [u8;32]
impl From<&[u8; 32]> for RootIdentity {
    fn from(src: &[u8; 32]) -> Self {
        Self::from(&RootEntropy::from(src))
    }
}

// Helper function for using hkdf to derive a key
fn root_identity_hkdf_helper(ikm: &[u8], info: &[u8]) -> Scalar {
    let mut result = [0u8; 32];
    let hk = SimpleHkdf::<Blake2b256>::new(None, ikm);

    // expand cannot fail because 32 bytes is a valid keylength for blake2b/256
    hk.expand(info, &mut result)
        .expect("buffer size arithmetic is wrong");

    // Now we reduce the result modulo group order. Cryptonote functions using
    // the `scalar_from_bytes` macro require this because the macro uses
    // `Scalar::from_canonical_bytes` rather than `Scalar::from_bits` or
    // `Scalar::from_bytes_mod_order`. It will returns an error if we don't make
    // the representation canonical
    Scalar::from_bytes_mod_order(result)
}

#[cfg(test)]
mod testing {
    use super::*;

    // Protobuf deserialization should recover a serialized RootIdentity.
    #[test]
    fn prost_roundtrip_root_identity() {
        bth_util_test_helper::run_with_several_seeds(|mut rng| {
            let root_id = RootIdentity::from_random(&mut rng);
            let ser = bth_util_serial::encode(&root_id);
            let result: RootIdentity = bth_util_serial::decode(&ser).unwrap();
            assert_eq!(root_id, result);
        })
    }

    // NOTE: test_with_data test removed - test vector crates were deleted
}
