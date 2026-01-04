// Copyright (c) 2018-2022 The Botho Foundation

//! Botho SLIP-0010 / BIP39 Based Key Derivation
//!
//! This provides utilities to handle SLIP-0010 key bytes and their relation to
//! the Botho [`Account`](bth_core::Account) structure, which contains a
//! pair of Ristretto255 view/spend private scalars.
//!
//! As well as providing traits to create a Slip10Key from entropy and path,
//! along with the canonical method of converting a BIP-39
//! [`Mnemonic`](tiny_bip32::Mnemonic) with a given BIP-32 path into a
//! [`Slip10Key`](Slip10Key) usable within Botho.

use curve25519_dalek::scalar::Scalar;
use hkdf::Hkdf;
use sha2::Sha512;
use zeroize::Zeroize;

use bth_crypto_keys::RistrettoPrivate;

#[cfg(feature = "bip39")]
pub use bip39::{Language, Mnemonic};

use crate::{
    account::Account,
    consts::{COINTYPE_BOTHO, USAGE_BIP44},
    keys::{RootSpendPrivate, RootViewPrivate},
};

/// [Hardened derivation](https://github.com/bitcoin/bips/blob/master/bip-0043.mediawiki#Security) flag for path components
const BIP39_SECURE: u32 = 0x80000000;

/// Fetch the BIP39 path for a given account index
pub const fn wallet_path(account_index: u32) -> [u32; 3] {
    [
        BIP39_SECURE | USAGE_BIP44,
        BIP39_SECURE | COINTYPE_BOTHO,
        BIP39_SECURE | (account_index & 0x7FFFFFFF),
    ]
}

/// A key derived using SLIP-0010 key derivation
#[derive(Zeroize)]
#[zeroize(drop)]
pub struct Slip10Key([u8; 32]);

/// Access [`Slip10Key`] value as byte slice
impl AsRef<[u8]> for Slip10Key {
    fn as_ref(&self) -> &[u8] {
        &self.0[..]
    }
}

#[cfg(feature = "internals")]
impl Slip10Key {
    /// Create a SLIP-0010 key from raw Ed25519 private key value
    pub fn from_raw(raw: [u8; 32]) -> Self {
        Self(raw)
    }
}

/// Derive an [`Account`] object from slip10 derived Ed25519 private key
/// (see [`wallet_path`] for the BIP32 derivation path)
impl From<&Slip10Key> for Account {
    fn from(src: &Slip10Key) -> Self {
        Account::new(RootViewPrivate::from(src), RootSpendPrivate::from(src))
    }
}

/// Canonical derivation of a [`RootViewPrivate`] key from SLIP-0010 derived
/// Ed25519 key
impl From<&Slip10Key> for RootViewPrivate {
    fn from(src: &Slip10Key) -> Self {
        let mut okm = [0u8; 64];

        let view_kdf = Hkdf::<Sha512>::new(Some(b"botho-ristretto255-view"), src.as_ref());
        view_kdf
            .expand(b"", &mut okm)
            .expect("Invalid okm length when creating private view key");
        let view_scalar = Scalar::from_bytes_mod_order_wide(&okm);
        let view_private_key = RistrettoPrivate::from(view_scalar);

        RootViewPrivate::from(view_private_key)
    }
}

/// Canonical derivation of a [`RootSpendPrivate`] key from SLIP-0010 derived
/// Ed25519 key
impl From<&Slip10Key> for RootSpendPrivate {
    fn from(src: &Slip10Key) -> Self {
        let mut okm = [0u8; 64];

        let spend_kdf = Hkdf::<Sha512>::new(Some(b"botho-ristretto255-spend"), src.as_ref());
        spend_kdf
            .expand(b"", &mut okm)
            .expect("Invalid okm length when creating private spend key");
        let spend_scalar = Scalar::from_bytes_mod_order_wide(&okm);
        let spend_private_key = RistrettoPrivate::from(spend_scalar);

        RootSpendPrivate::from(spend_private_key)
    }
}

/// A common interface for constructing a [`Slip10Key`] for Botho given an
/// account index.
pub trait Slip10KeyGenerator {
    /// Derive a Botho SLIP10 key for the given account from the current
    /// object
    fn derive_slip10_key(self, account_index: u32) -> Slip10Key;
}

// This lets us get to
// let account: AccountKey =
// Mnemonic::from_phrases().derive_slip10_key(account_index).into()
#[cfg(feature = "bip39")]
impl Slip10KeyGenerator for Mnemonic {
    /// Derive a SLIP-0010 key for the specified account
    fn derive_slip10_key(self, account_index: u32) -> Slip10Key {
        // We explicitly do not support passphrases for BIP-39 mnemonics, please
        // see the Botho Key Derivation design specification, v1.0.0, for
        // design rationale.
        let seed = bip39::Seed::new(&self, "");

        // This is constructing an `m/44/866/<idx>` BIP32 path for use by SLIP-0010.
        let path = wallet_path(account_index);

        // We're taking what the SLIP-0010 spec calls the "Ed25519 private key"
        // here as our `Slip10Key`. That said, we're not actually using this as
        // an Ed25519 key, just IKM for a pair of HKDF-SHA512 instances whose
        // output will be correctly transformed into the Ristretto255 keypair we
        // need.
        //
        // This will also transform any "unhardened" path components into their
        // "hardened" version.
        let key = slip10_ed25519::derive_ed25519_private_key(seed.as_bytes(), &path);

        Slip10Key(key)
    }
}

#[cfg(test)]
mod test {
    extern crate alloc;
    extern crate std;

    use super::*;
    use alloc::{string::String, vec::Vec};
    use bth_crypto_keys::RistrettoPublic;
    use serde::{Deserialize, Serialize};
    use std::sync::LazyLock;

    // Include test vectors as JSON strings
    const KEY_TO_RISTRETTO_STR: &str = include_str!("../../tests/slip10_key.json");
    const MNEMONIC_TO_RISTRETTO_STR: &str = include_str!("../../tests/slip10_mnemonic.json");

    // Deserialize test vectors on first access
    static SLIPKEY_TO_RISTRETTO_TESTS: LazyLock<Vec<KeyToRistretto>> =
        LazyLock::new(|| serde_json::from_str(KEY_TO_RISTRETTO_STR).unwrap());
    static MNEMONIC_TO_RISTRETTO_TESTS: LazyLock<Vec<MnemonicToRistretto>> =
        LazyLock::new(|| serde_json::from_str(MNEMONIC_TO_RISTRETTO_STR).unwrap());

    /// Slip10 key to ristretto test definitions
    #[derive(Clone, PartialEq, Serialize, Deserialize)]
    pub struct KeyToRistretto {
        slip10_hex: String,
        view_hex: String,
        spend_hex: String,
    }

    /// Slip10 mnemonic to ristretto test definitions
    #[derive(Clone, PartialEq, Serialize, Deserialize)]
    pub struct MnemonicToRistretto {
        phrase: String,
        account_index: u32,
        view_hex: String,
        spend_hex: String,
    }

    /// Test conversion of a SLIP10 seed into botho account keys
    #[test]
    fn slip10key_into_account_key() {
        for data in SLIPKEY_TO_RISTRETTO_TESTS.iter() {
            // TODO: maybe Slip10Key could implement hex::FromHex?
            let mut key_bytes = [0u8; 32];
            hex::decode_to_slice(&data.slip10_hex, &mut key_bytes[..])
                .expect("Could not decode SLIP10 test vector output");

            let slip10_key = Slip10Key(key_bytes);

            let mut expected_view_bytes = [0u8; 64];
            hex::decode_to_slice(&data.view_hex, &mut expected_view_bytes)
                .expect("Could not decode view-key bytes");
            let expected_view_scalar = Scalar::from_bytes_mod_order_wide(&expected_view_bytes);
            let expected_view_key = RistrettoPrivate::from(expected_view_scalar);

            let mut expected_spend_bytes = [0u8; 64];
            hex::decode_to_slice(&data.spend_hex, &mut expected_spend_bytes)
                .expect("Could not decode spend-key bytes");
            let expected_spend_scalar = Scalar::from_bytes_mod_order_wide(&expected_spend_bytes);
            let expected_spend_key = RistrettoPrivate::from(expected_spend_scalar);

            let account_key = Account::from(&slip10_key);

            assert_ne!(
                RistrettoPublic::from(&expected_view_key),
                RistrettoPublic::from(&expected_spend_key),
            );
            assert_eq!(account_key.view_private_key(), &expected_view_key,);
            assert_eq!(account_key.spend_private_key(), &expected_spend_key,);
        }
    }

    /// Test conversion of a BIP39 mnemonic into botho account keys
    #[test]
    #[cfg(feature = "bip39")]
    fn mnemonic_into_account_key() {
        for data in MNEMONIC_TO_RISTRETTO_TESTS.iter() {
            let mnemonic = Mnemonic::from_phrase(&data.phrase, Language::English)
                .expect("Could not read test phrase into mnemonic");
            let key = mnemonic.derive_slip10_key(data.account_index);
            let account_key = Account::from(&key);

            let mut expected_view_bytes = [0u8; 64];
            hex::decode_to_slice(&data.view_hex, &mut expected_view_bytes)
                .expect("Could not decode view-key bytes");
            let expected_view_scalar = Scalar::from_bytes_mod_order_wide(&expected_view_bytes);
            let expected_view_key = RistrettoPrivate::from(expected_view_scalar);

            let mut expected_spend_bytes = [0u8; 64];
            hex::decode_to_slice(&data.spend_hex, &mut expected_spend_bytes)
                .expect("Could not decode spend-key bytes");
            let expected_spend_scalar = Scalar::from_bytes_mod_order_wide(&expected_spend_bytes);
            let expected_spend_key = RistrettoPrivate::from(expected_spend_scalar);

            assert_ne!(
                RistrettoPublic::from(&expected_view_key),
                RistrettoPublic::from(&expected_spend_key),
            );
            assert_eq!(account_key.view_private_key(), &expected_view_key,);
            assert_eq!(account_key.spend_private_key(), &expected_spend_key,);
        }
    }

    /// Test wallet_path function returns correct BIP32 path components
    #[test]
    fn test_wallet_path() {
        let path = wallet_path(0);
        assert_eq!(path.len(), 3);

        // First component: 44' (BIP44 usage)
        assert_eq!(path[0], BIP39_SECURE | USAGE_BIP44);
        assert_eq!(path[0], 0x8000002C); // 44 with hardened flag

        // Second component: 866' (Botho coin type)
        assert_eq!(path[1], BIP39_SECURE | COINTYPE_BOTHO);
        assert_eq!(path[1], 0x80000362); // 866 with hardened flag

        // Third component: account index with hardened flag
        assert_eq!(path[2], BIP39_SECURE | 0);
    }

    /// Test wallet_path with different account indices
    #[test]
    fn test_wallet_path_different_indices() {
        let path_0 = wallet_path(0);
        let path_1 = wallet_path(1);
        let path_100 = wallet_path(100);

        // First two components should be the same
        assert_eq!(path_0[0], path_1[0]);
        assert_eq!(path_0[1], path_1[1]);
        assert_eq!(path_1[0], path_100[0]);
        assert_eq!(path_1[1], path_100[1]);

        // Third component should differ based on account index
        assert_ne!(path_0[2], path_1[2]);
        assert_ne!(path_1[2], path_100[2]);

        assert_eq!(path_0[2], BIP39_SECURE | 0);
        assert_eq!(path_1[2], BIP39_SECURE | 1);
        assert_eq!(path_100[2], BIP39_SECURE | 100);
    }

    /// Test Slip10Key AsRef implementation
    #[test]
    fn test_slip10key_as_ref() {
        let key_bytes = [42u8; 32];
        let slip10_key = Slip10Key(key_bytes);

        let as_ref: &[u8] = slip10_key.as_ref();
        assert_eq!(as_ref.len(), 32);
        assert_eq!(as_ref, &key_bytes[..]);
    }

    /// Test that same Slip10Key produces same Account
    #[test]
    fn test_slip10key_deterministic() {
        let key_bytes = [42u8; 32];
        let slip10_key_1 = Slip10Key(key_bytes);
        let slip10_key_2 = Slip10Key(key_bytes);

        let account_1 = Account::from(&slip10_key_1);
        let account_2 = Account::from(&slip10_key_2);

        assert_eq!(
            account_1.view_private_key().to_bytes(),
            account_2.view_private_key().to_bytes()
        );
        assert_eq!(
            account_1.spend_private_key().to_bytes(),
            account_2.spend_private_key().to_bytes()
        );
    }

    /// Test that different Slip10Keys produce different Accounts
    #[test]
    fn test_different_slip10keys_produce_different_accounts() {
        let slip10_key_1 = Slip10Key([1u8; 32]);
        let slip10_key_2 = Slip10Key([2u8; 32]);

        let account_1 = Account::from(&slip10_key_1);
        let account_2 = Account::from(&slip10_key_2);

        assert_ne!(
            account_1.view_private_key().to_bytes(),
            account_2.view_private_key().to_bytes()
        );
        assert_ne!(
            account_1.spend_private_key().to_bytes(),
            account_2.spend_private_key().to_bytes()
        );
    }

    /// Test RootViewPrivate derivation from Slip10Key
    #[test]
    fn test_root_view_private_from_slip10key() {
        let slip10_key = Slip10Key([42u8; 32]);
        let view_private = RootViewPrivate::from(&slip10_key);

        // View private key should be 32 bytes
        assert_eq!(view_private.to_bytes().len(), 32);

        // Same key should produce same result
        let view_private_2 = RootViewPrivate::from(&Slip10Key([42u8; 32]));
        assert_eq!(view_private.to_bytes(), view_private_2.to_bytes());
    }

    /// Test RootSpendPrivate derivation from Slip10Key
    #[test]
    fn test_root_spend_private_from_slip10key() {
        let slip10_key = Slip10Key([42u8; 32]);
        let spend_private = RootSpendPrivate::from(&slip10_key);

        // Spend private key should be 32 bytes
        assert_eq!(spend_private.to_bytes().len(), 32);

        // Same key should produce same result
        let spend_private_2 = RootSpendPrivate::from(&Slip10Key([42u8; 32]));
        assert_eq!(spend_private.to_bytes(), spend_private_2.to_bytes());
    }

    /// Test view and spend keys are different from same Slip10Key
    #[test]
    fn test_view_and_spend_are_different() {
        let slip10_key = Slip10Key([42u8; 32]);
        let view_private = RootViewPrivate::from(&slip10_key);
        let spend_private = RootSpendPrivate::from(&slip10_key);

        // View and spend private keys should be different
        assert_ne!(view_private.to_bytes(), spend_private.to_bytes());
    }

    /// Test different mnemonic phrases produce different keys
    #[test]
    #[cfg(feature = "bip39")]
    fn test_different_mnemonics_different_keys() {
        // Use two different valid BIP39 phrases
        let phrase1 = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let phrase2 = "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong";

        let mnemonic1 = Mnemonic::from_phrase(phrase1, Language::English).unwrap();
        let mnemonic2 = Mnemonic::from_phrase(phrase2, Language::English).unwrap();

        let key1 = mnemonic1.derive_slip10_key(0);
        let key2 = mnemonic2.derive_slip10_key(0);

        let account1 = Account::from(&key1);
        let account2 = Account::from(&key2);

        // Different mnemonics should produce different keys
        assert_ne!(
            account1.view_private_key().to_bytes(),
            account2.view_private_key().to_bytes()
        );
        assert_ne!(
            account1.spend_private_key().to_bytes(),
            account2.spend_private_key().to_bytes()
        );
    }

    /// Test same mnemonic with different account indices
    #[test]
    #[cfg(feature = "bip39")]
    fn test_same_mnemonic_different_indices() {
        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let mnemonic1 = Mnemonic::from_phrase(phrase, Language::English).unwrap();
        let mnemonic2 = Mnemonic::from_phrase(phrase, Language::English).unwrap();

        let key1 = mnemonic1.derive_slip10_key(0);
        let key2 = mnemonic2.derive_slip10_key(1);

        let account1 = Account::from(&key1);
        let account2 = Account::from(&key2);

        // Different account indices should produce different keys
        assert_ne!(
            account1.view_private_key().to_bytes(),
            account2.view_private_key().to_bytes()
        );
        assert_ne!(
            account1.spend_private_key().to_bytes(),
            account2.spend_private_key().to_bytes()
        );
    }

    /// Test mnemonic derivation is deterministic
    #[test]
    #[cfg(feature = "bip39")]
    fn test_mnemonic_derivation_deterministic() {
        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

        let mnemonic1 = Mnemonic::from_phrase(phrase, Language::English).unwrap();
        let mnemonic2 = Mnemonic::from_phrase(phrase, Language::English).unwrap();

        let key1 = mnemonic1.derive_slip10_key(0);
        let key2 = mnemonic2.derive_slip10_key(0);

        let account1 = Account::from(&key1);
        let account2 = Account::from(&key2);

        // Same mnemonic and index should produce identical keys
        assert_eq!(
            account1.view_private_key().to_bytes(),
            account2.view_private_key().to_bytes()
        );
        assert_eq!(
            account1.spend_private_key().to_bytes(),
            account2.spend_private_key().to_bytes()
        );
    }

    /// Regenerate slip10_key.json test vectors
    /// Run with: cargo test -p bth-core --lib -- --nocapture --ignored
    /// regen_slip10_key_vectors
    #[test]
    #[ignore]
    fn regen_slip10_key_vectors() {
        std::println!("\n=== REGENERATED slip10_key.json ===\n[");
        for data in SLIPKEY_TO_RISTRETTO_TESTS.iter() {
            let mut key_bytes = [0u8; 32];
            hex::decode_to_slice(&data.slip10_hex, &mut key_bytes).unwrap();

            // Get the raw scalar bytes (64 bytes for HKDF output)
            let view_kdf = Hkdf::<Sha512>::new(Some(b"botho-ristretto255-view"), &key_bytes);
            let mut view_okm = [0u8; 64];
            view_kdf.expand(b"", &mut view_okm).unwrap();

            let spend_kdf = Hkdf::<Sha512>::new(Some(b"botho-ristretto255-spend"), &key_bytes);
            let mut spend_okm = [0u8; 64];
            spend_kdf.expand(b"", &mut spend_okm).unwrap();

            std::println!(
                r#"    {{
        "slip10_hex": "{}",
        "view_hex": "{}",
        "spend_hex": "{}"
    }},"#,
                data.slip10_hex,
                hex::encode(view_okm),
                hex::encode(spend_okm)
            );
        }
        std::println!("]");
    }

    /// Regenerate slip10_mnemonic.json test vectors
    /// Run with: cargo test -p bth-core --lib -- --nocapture --ignored
    /// regen_slip10_mnemonic_vectors
    #[test]
    #[ignore]
    #[cfg(feature = "bip39")]
    fn regen_slip10_mnemonic_vectors() {
        std::println!("\n=== REGENERATED slip10_mnemonic.json ===\n[");
        for data in MNEMONIC_TO_RISTRETTO_TESTS.iter() {
            let mnemonic = Mnemonic::from_phrase(&data.phrase, Language::English).unwrap();
            let slip10_key = mnemonic.derive_slip10_key(data.account_index);

            let key_bytes: &[u8] = slip10_key.as_ref();

            let view_kdf = Hkdf::<Sha512>::new(Some(b"botho-ristretto255-view"), key_bytes);
            let mut view_okm = [0u8; 64];
            view_kdf.expand(b"", &mut view_okm).unwrap();

            let spend_kdf = Hkdf::<Sha512>::new(Some(b"botho-ristretto255-spend"), key_bytes);
            let mut spend_okm = [0u8; 64];
            spend_kdf.expand(b"", &mut spend_okm).unwrap();

            std::println!(
                r#"    {{
        "phrase": "{}",
        "account_index": {},
        "view_hex": "{}",
        "spend_hex": "{}"
    }},"#,
                data.phrase,
                data.account_index,
                hex::encode(view_okm),
                hex::encode(spend_okm)
            );
        }
        std::println!("]");
    }
}
