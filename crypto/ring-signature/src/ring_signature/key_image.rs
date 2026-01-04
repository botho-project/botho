// Copyright (c) 2018-2022 The Botho Foundation

use super::{hash_to_point, Error, Scalar};
use bth_crypto_digestible::Digestible;
use bth_crypto_keys::{RistrettoPrivate, RistrettoPublic};
use bth_util_repr_bytes::{
    derive_core_cmp_from_as_ref, derive_debug_and_display_hex_from_as_ref,
    derive_repr_bytes_from_as_ref_and_try_from, typenum::U32, LengthMismatch,
};
use curve25519_dalek::ristretto::CompressedRistretto;

#[cfg(feature = "prost")]
use bth_util_repr_bytes::derive_prost_message_from_repr_bytes;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

#[derive(Clone, Copy, Default, Digestible, Zeroize)]
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[digestible(transparent)]
/// The "image" of a private key `x`: I = x * Hp(x * G) = x * Hp(P).
pub struct KeyImage {
    /// The curve point corresponding to the key image
    pub point: CompressedRistretto,
}

impl KeyImage {
    /// View the underlying `CompressedRistretto` as an array of bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        self.point.as_bytes()
    }

    /// Copies `self` into a new Vec.
    #[cfg(feature = "alloc")]
    pub fn to_vec(&self) -> alloc::vec::Vec<u8> {
        self.point.as_bytes().to_vec()
    }
}

impl From<&RistrettoPrivate> for KeyImage {
    fn from(x: &RistrettoPrivate) -> Self {
        let P = RistrettoPublic::from(x);
        let Hp = hash_to_point(&P);
        let point = x.as_ref() * Hp;
        KeyImage {
            point: point.compress(),
        }
    }
}

// Many tests use this
impl From<u64> for KeyImage {
    fn from(n: u64) -> Self {
        let private_key = RistrettoPrivate::from(Scalar::from(n));
        Self::from(&private_key)
    }
}

impl TryFrom<[u8; 32]> for KeyImage {
    type Error = Error;
    fn try_from(src: [u8; 32]) -> Result<Self, Self::Error> {
        let point = CompressedRistretto::from_slice(&src).map_err(|_e| Error::InvalidCurvePoint)?;
        Ok(Self { point })
    }
}

impl AsRef<CompressedRistretto> for KeyImage {
    fn as_ref(&self) -> &CompressedRistretto {
        &self.point
    }
}

impl AsRef<[u8; 32]> for KeyImage {
    fn as_ref(&self) -> &[u8; 32] {
        self.as_bytes()
    }
}

impl AsRef<[u8]> for KeyImage {
    fn as_ref(&self) -> &[u8] {
        &self.as_bytes()[..]
    }
}

impl TryFrom<&[u8]> for KeyImage {
    type Error = Error;
    fn try_from(src: &[u8]) -> Result<Self, Error> {
        if src.len() != 32 {
            return Err(Error::from(LengthMismatch {
                expected: 32,
                found: src.len(),
            }));
        }
        let point = CompressedRistretto::from_slice(src).map_err(|_e| Error::InvalidCurvePoint)?;
        Ok(Self { point })
    }
}

derive_repr_bytes_from_as_ref_and_try_from!(KeyImage, U32);
derive_core_cmp_from_as_ref!(KeyImage, [u8; 32]);
derive_debug_and_display_hex_from_as_ref!(KeyImage);

#[cfg(feature = "prost")]
derive_prost_message_from_repr_bytes!(KeyImage);

#[cfg(test)]
mod tests {
    use super::*;
    use bth_util_from_random::FromRandom;

    #[test]
    fn test_key_image_from_private_key() {
        let mut rng = rand_core::OsRng;
        let private = RistrettoPrivate::from_random(&mut rng);
        let key_image = KeyImage::from(&private);

        // Key image should be 32 bytes
        assert_eq!(key_image.as_bytes().len(), 32);

        // Same private key should produce same key image
        let key_image2 = KeyImage::from(&private);
        assert_eq!(key_image, key_image2);
    }

    #[test]
    fn test_different_keys_different_images() {
        let mut rng = rand_core::OsRng;
        let private1 = RistrettoPrivate::from_random(&mut rng);
        let private2 = RistrettoPrivate::from_random(&mut rng);

        let image1 = KeyImage::from(&private1);
        let image2 = KeyImage::from(&private2);

        assert_ne!(image1, image2);
    }

    #[test]
    fn test_key_image_from_u64() {
        let image1 = KeyImage::from(1u64);
        let image2 = KeyImage::from(2u64);
        let image1_again = KeyImage::from(1u64);

        assert_ne!(image1, image2);
        assert_eq!(image1, image1_again);
    }

    #[test]
    fn test_key_image_bytes_roundtrip() {
        let mut rng = rand_core::OsRng;
        let private = RistrettoPrivate::from_random(&mut rng);
        let key_image = KeyImage::from(&private);

        let bytes: [u8; 32] = *key_image.as_bytes();
        let recovered = KeyImage::try_from(bytes).expect("Should recover key image");

        assert_eq!(key_image, recovered);
    }

    #[test]
    fn test_key_image_from_slice() {
        let mut rng = rand_core::OsRng;
        let private = RistrettoPrivate::from_random(&mut rng);
        let key_image = KeyImage::from(&private);

        let bytes = key_image.as_bytes();
        let recovered = KeyImage::try_from(&bytes[..]).expect("Should recover from slice");

        assert_eq!(key_image, recovered);
    }

    #[test]
    fn test_key_image_invalid_length() {
        let short_bytes = [0u8; 16];
        let result = KeyImage::try_from(&short_bytes[..]);
        assert!(result.is_err());
    }

    #[test]
    #[cfg(feature = "alloc")]
    fn test_key_image_to_vec() {
        let key_image = KeyImage::from(42u64);
        let vec = key_image.to_vec();
        assert_eq!(vec.len(), 32);
        assert_eq!(&vec[..], key_image.as_bytes());
    }

    #[test]
    fn test_key_image_ordering() {
        let image1 = KeyImage::from(1u64);
        let image2 = KeyImage::from(2u64);

        // Test that ordering works (for use in sets/maps)
        assert!(image1 != image2);
        // One should be less than the other
        assert!(image1 < image2 || image2 < image1);
    }
}
