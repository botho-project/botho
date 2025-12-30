// Copyright (c) 2018-2022 The Botho Foundation

//! Definition of memo payload type
//!
//! This memo payload and its encryption scheme was proposed for standardization
//! in bothofoundation/mcips/pull/3.
//!
//! The encrypted memo of TxOut's is designed to have one encryption scheme and
//! the payload is an extensible format. Two bytes are used for a schema type,
//! and sixty four bytes are used for data according to that schema.
//!
//! The encryption details are defined in the transaction crate, but we would
//! like to avoid making the introduction of a new schema require changes to
//! the transaction-core crate, because this would require a new consensus
//! enclave.
//!
//! We also would like to avoid implementing the interpretation of memo data
//! in the transaction crate, for much the same reasons.
//!
//! Therefore, the code is organized as follows:
//! - A MemoPayload is the collection of bytes ready to be encrypted. This can
//!   be used to construct a TxOut, and it is encrypted at that time. This is
//!   defined in transaction-core crate.
//! - The memo module in transaction-std crate defines specific structures that
//!   can be converted to a MemoPayload, and provides a function that can
//!   interpret a MemoPayload as one of the known high-level objects.
//! - The TransactionBuilder now uses a memo builder to set the "policy" around
//!   memos for this transaction, so that low-level handling of memos is not
//!   needed by the user of the TransactionBuilder.
//! - When interpretting memos on TxOut's that you recieved, the memo module
//!   functionality can be used to assist.

use aes::{
    cipher::{KeyIvInit, StreamCipher},
    Aes256,
};
use core::str::Utf8Error;
use ctr::Ctr64BE;
use displaydoc::Display;
use generic_array::{
    sequence::Split,
    typenum::{U32, U48, U66},
    GenericArray,
};
use hkdf::Hkdf;
use bth_crypto_digestible::Digestible;
use bth_crypto_keys::{CompressedRistrettoPublic, RistrettoPublic};
use bth_util_repr_bytes::{
    derive_debug_and_display_hex_from_as_ref, derive_into_vec_from_repr_bytes,
    derive_prost_message_from_repr_bytes, derive_repr_bytes_from_as_ref_and_try_from,
    derive_serde_from_repr_bytes,
};
use serde::{Deserialize, Serialize};
use sha2::Sha512;
use zeroize::Zeroize;

type Aes256Ctr = Ctr64BE<Aes256>;

/// An encrypted memo, which can be decrypted by the recipient of a TxOut.
#[derive(Clone, Copy, Default, Digestible, Eq, Hash, Ord, PartialEq, PartialOrd, Zeroize)]
pub struct EncryptedMemo(GenericArray<u8, U66>);

impl AsRef<[u8]> for EncryptedMemo {
    fn as_ref(&self) -> &[u8] {
        self.0.as_slice()
    }
}

impl AsRef<GenericArray<u8, U66>> for EncryptedMemo {
    fn as_ref(&self) -> &GenericArray<u8, U66> {
        &self.0
    }
}

impl From<EncryptedMemo> for GenericArray<u8, U66> {
    fn from(src: EncryptedMemo) -> Self {
        src.0
    }
}

impl From<GenericArray<u8, U66>> for EncryptedMemo {
    fn from(src: GenericArray<u8, U66>) -> Self {
        Self(src)
    }
}

impl TryFrom<&[u8]> for EncryptedMemo {
    type Error = MemoError;
    fn try_from(src: &[u8]) -> Result<EncryptedMemo, Self::Error> {
        if src.len() == 66 {
            Ok(Self(*GenericArray::from_slice(src)))
        } else {
            Err(MemoError::BadLength(src.len()))
        }
    }
}

derive_repr_bytes_from_as_ref_and_try_from!(EncryptedMemo, U66);
derive_into_vec_from_repr_bytes!(EncryptedMemo);
derive_serde_from_repr_bytes!(EncryptedMemo);
derive_prost_message_from_repr_bytes!(EncryptedMemo);
derive_debug_and_display_hex_from_as_ref!(EncryptedMemo);

impl EncryptedMemo {
    /// Helper to ease syntax when decrypting
    ///
    /// The shared-secret is expected to be the TxOut shared secret of the TxOut
    /// that this memo is associated to.
    pub fn decrypt(&self, shared_secret: &RistrettoPublic) -> MemoPayload {
        MemoPayload::decrypt_from(self, shared_secret)
    }
}

/// A plaintext memo payload, with accessors to easily access the memo type
/// bytes and memo data bytes.
///
/// High-level memo objects should be convertible to MemoPayload.
/// Deserialization, across all high-level memo types, is done in
/// mc-transaction-std crate.
///
/// Note that a memo payload may be invalid / uninterpretable, or refer to new
/// memo types that have been introduced at a later date.
#[derive(Clone, Copy, Default, Eq, Digestible, Ord, PartialEq, PartialOrd)]
pub struct MemoPayload(GenericArray<u8, U66>);

impl MemoPayload {
    /// Create a new memo payload from given type bytes and data bytes
    pub fn new(memo_type: [u8; 2], memo_data: [u8; 64]) -> Self {
        let mut result = Self::default();
        result.0[0..2].copy_from_slice(&memo_type);
        result.0[2..66].copy_from_slice(&memo_data);
        result
    }

    /// Get the memo type bytes (two bytes)
    pub fn get_memo_type(&self) -> &[u8; 2] {
        self.0.as_slice()[0..2].try_into().expect("length mismatch")
    }

    /// Get the memo data bytes (sixty-four bytes)
    pub fn get_memo_data(&self) -> &[u8; 64] {
        self.0.as_slice()[2..66]
            .try_into()
            .expect("length mismatch")
    }

    /// Encrypt this memo payload using a given shared-secret, consuming it and
    /// returning underlying buffer.
    ///
    /// The shared-secret is expected to be the TxOut shared secret of the TxOut
    /// that this memo is associated to.
    pub fn encrypt(mut self, shared_secret: &RistrettoPublic) -> EncryptedMemo {
        self.apply_keystream(shared_secret);
        EncryptedMemo(self.0)
    }

    /// Decrypt an EncryptedMemoPayload using a given shared secret, consuming
    /// it and returning the underlying buffer.
    pub fn decrypt_from(encrypted: &EncryptedMemo, shared_secret: &RistrettoPublic) -> Self {
        let mut result = Self::from(encrypted.0);
        result.apply_keystream(shared_secret);
        result
    }

    // Apply AES256 keystream to internal buffer.
    // This is not a user-facing API, since from the user's point of view this
    // object always represents decrypted bytes.
    //
    // The argument is supposed to be the TxOut shared secret associated to the
    // memo.
    fn apply_keystream(&mut self, shared_secret: &RistrettoPublic) {
        // Use HKDF-SHA512 to produce an AES key and AES nonce
        let shared_secret = CompressedRistrettoPublic::from(shared_secret);
        let kdf = Hkdf::<Sha512>::new(Some(b"mc-memo-okm"), shared_secret.as_ref());
        // OKM is "output key material", see RFC HKDF for discussion of terms
        let mut okm = GenericArray::<u8, U48>::default();
        kdf.expand(b"", okm.as_mut_slice())
            .expect("Digest output size is insufficient");

        let (key, nonce) = Split::<u8, U32>::split(okm);

        // Apply AES-256 in counter mode to the buffer
        let mut aes256ctr = Aes256Ctr::new(&key, &nonce);
        aes256ctr.apply_keystream(self.0.as_mut_slice());
    }
}

impl AsRef<[u8]> for MemoPayload {
    fn as_ref(&self) -> &[u8] {
        self.0.as_slice()
    }
}

impl AsRef<GenericArray<u8, U66>> for MemoPayload {
    fn as_ref(&self) -> &GenericArray<u8, U66> {
        &self.0
    }
}

impl From<MemoPayload> for GenericArray<u8, U66> {
    fn from(src: MemoPayload) -> Self {
        src.0
    }
}

impl From<GenericArray<u8, U66>> for MemoPayload {
    fn from(src: GenericArray<u8, U66>) -> Self {
        Self(src)
    }
}

impl TryFrom<&[u8]> for MemoPayload {
    type Error = MemoError;
    fn try_from(src: &[u8]) -> Result<MemoPayload, Self::Error> {
        if src.len() == 66 {
            Ok(Self(*GenericArray::from_slice(src)))
        } else {
            Err(MemoError::BadLength(src.len()))
        }
    }
}

derive_repr_bytes_from_as_ref_and_try_from!(MemoPayload, U66);
derive_into_vec_from_repr_bytes!(MemoPayload);
derive_serde_from_repr_bytes!(MemoPayload);
derive_prost_message_from_repr_bytes!(MemoPayload);
derive_debug_and_display_hex_from_as_ref!(MemoPayload);

/// An error which can occur when handling memos
#[derive(Clone, Debug, Deserialize, Display, Eq, PartialEq, Serialize)]
pub enum MemoError {
    /// Wrong length for memo payload: {0}
    BadLength(usize),

    /// Utf-8 did not properly decode
    Utf8Decoding,

    /// Max fee of {0} exceeded. Attempted to set fee amount: {1}
    MaxFeeExceeded(u64, u64),
}

impl From<Utf8Error> for MemoError {
    fn from(_: Utf8Error) -> Self {
        Self::Utf8Decoding
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bth_util_from_random::FromRandom;
    use bth_util_test_helper::{RngType, SeedableRng};

    #[test]
    fn test_memo_payload_round_trip() {
        let mut rng = RngType::seed_from_u64(37);

        let key1 = RistrettoPublic::from_random(&mut rng);
        let key2 = RistrettoPublic::from_random(&mut rng);

        let memo1 = MemoPayload::default();
        let e_memo1 = memo1.encrypt(&key1);
        assert_eq!(memo1, e_memo1.decrypt(&key1), "roundtrip failed");

        let memo2 = MemoPayload::new([1u8, 2u8], [47u8; 64]);
        let e_memo2 = memo2.encrypt(&key1);
        assert_eq!(memo2, e_memo2.decrypt(&key1), "roundtrip failed");

        let memo1 = MemoPayload::default();
        let e_memo1 = memo1.encrypt(&key1);
        assert_ne!(
            memo1,
            e_memo1.decrypt(&key2),
            "decrypting with wrong key succeeded"
        );

        let memo2 = MemoPayload::new([1u8, 2u8], [47u8; 64]);
        let e_memo2 = memo2.encrypt(&key2);
        assert_ne!(
            memo2,
            e_memo2.decrypt(&key1),
            "decrypting with wrong key succeeded"
        );
    }

    #[test]
    fn test_memo_payload_new() {
        let memo_type = [0x01, 0x02];
        let memo_data = [0xAB; 64];
        let memo = MemoPayload::new(memo_type, memo_data);

        assert_eq!(memo.get_memo_type(), &memo_type);
        assert_eq!(memo.get_memo_data(), &memo_data);
    }

    #[test]
    fn test_memo_payload_default() {
        let memo = MemoPayload::default();
        assert_eq!(memo.get_memo_type(), &[0u8, 0u8]);
        assert_eq!(memo.get_memo_data(), &[0u8; 64]);
    }

    #[test]
    fn test_encrypted_memo_try_from_valid_slice() {
        let bytes = [0u8; 66];
        let result = EncryptedMemo::try_from(&bytes[..]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_encrypted_memo_try_from_invalid_slice() {
        let short_bytes = [0u8; 32];
        let result = EncryptedMemo::try_from(&short_bytes[..]);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), MemoError::BadLength(32));

        let long_bytes = [0u8; 100];
        let result2 = EncryptedMemo::try_from(&long_bytes[..]);
        assert!(result2.is_err());
        assert_eq!(result2.unwrap_err(), MemoError::BadLength(100));
    }

    #[test]
    fn test_memo_payload_try_from_valid_slice() {
        let bytes = [0u8; 66];
        let result = MemoPayload::try_from(&bytes[..]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_memo_payload_try_from_invalid_slice() {
        let short_bytes = [0u8; 32];
        let result = MemoPayload::try_from(&short_bytes[..]);
        assert!(result.is_err());
    }

    #[test]
    fn test_encrypted_memo_as_ref() {
        let mut rng = RngType::seed_from_u64(42);
        let key = RistrettoPublic::from_random(&mut rng);
        let memo = MemoPayload::new([1, 2], [3; 64]);
        let encrypted = memo.encrypt(&key);

        let slice: &[u8] = encrypted.as_ref();
        assert_eq!(slice.len(), 66);
    }

    #[test]
    fn test_memo_payload_as_ref() {
        let memo = MemoPayload::new([1, 2], [3; 64]);
        let slice: &[u8] = memo.as_ref();
        assert_eq!(slice.len(), 66);
        assert_eq!(slice[0], 1);
        assert_eq!(slice[1], 2);
    }

    #[test]
    fn test_encrypted_memo_from_generic_array() {
        let arr = GenericArray::<u8, U66>::default();
        let encrypted = EncryptedMemo::from(arr.clone());
        let recovered: GenericArray<u8, U66> = encrypted.into();
        assert_eq!(arr, recovered);
    }

    #[test]
    fn test_memo_payload_from_generic_array() {
        let arr = GenericArray::<u8, U66>::default();
        let memo = MemoPayload::from(arr.clone());
        let recovered: GenericArray<u8, U66> = memo.into();
        assert_eq!(arr, recovered);
    }

    #[test]
    fn test_memo_payload_ordering() {
        let memo1 = MemoPayload::new([0, 0], [0; 64]);
        let memo2 = MemoPayload::new([0, 1], [0; 64]);
        let memo3 = MemoPayload::new([1, 0], [0; 64]);

        assert!(memo1 < memo2);
        assert!(memo2 < memo3);
        assert!(memo1 < memo3);
    }

    #[test]
    fn test_memo_payload_equality() {
        let memo1 = MemoPayload::new([1, 2], [3; 64]);
        let memo2 = MemoPayload::new([1, 2], [3; 64]);
        let memo3 = MemoPayload::new([1, 2], [4; 64]);

        assert_eq!(memo1, memo2);
        assert_ne!(memo1, memo3);
    }

    #[test]
    fn test_encrypted_memo_default() {
        let encrypted = EncryptedMemo::default();
        let slice: &[u8] = encrypted.as_ref();
        assert!(slice.iter().all(|&b| b == 0));
    }

    #[test]
    fn test_memo_error_variants() {
        use alloc::string::ToString;

        let err1 = MemoError::BadLength(100);
        assert!(err1.to_string().contains("100"));

        let err2 = MemoError::Utf8Decoding;
        assert!(!err2.to_string().is_empty());

        let err3 = MemoError::MaxFeeExceeded(100, 200);
        assert!(err3.to_string().contains("100"));
        assert!(err3.to_string().contains("200"));
    }

    #[test]
    fn test_memo_error_equality() {
        let err1 = MemoError::BadLength(100);
        let err2 = MemoError::BadLength(100);
        let err3 = MemoError::BadLength(200);

        assert_eq!(err1, err2);
        assert_ne!(err1, err3);
    }

    #[test]
    fn test_encryption_is_deterministic() {
        let mut rng = RngType::seed_from_u64(123);
        let key = RistrettoPublic::from_random(&mut rng);

        let memo1 = MemoPayload::new([1, 2], [3; 64]);
        let memo2 = MemoPayload::new([1, 2], [3; 64]);

        let encrypted1 = memo1.encrypt(&key);
        let encrypted2 = memo2.encrypt(&key);

        // Same memo with same key should produce same ciphertext
        assert_eq!(encrypted1.as_ref() as &[u8], encrypted2.as_ref() as &[u8]);
    }

    #[test]
    fn test_different_keys_produce_different_ciphertext() {
        let mut rng = RngType::seed_from_u64(456);
        let key1 = RistrettoPublic::from_random(&mut rng);
        let key2 = RistrettoPublic::from_random(&mut rng);

        let memo = MemoPayload::new([1, 2], [3; 64]);

        let encrypted1 = memo.clone().encrypt(&key1);
        let encrypted2 = memo.encrypt(&key2);

        // Same memo with different keys should produce different ciphertext
        assert_ne!(encrypted1.as_ref() as &[u8], encrypted2.as_ref() as &[u8]);
    }
}
