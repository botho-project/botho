// Copyright (c) 2018-2023 The Botho Foundation

//! This module contains code related to reading/writing mnemonic-based accounts
//! (either as protobuf or JSON strings) and converting them into AccountKey
//! data structures.

use bip39::{Language, Mnemonic};
use displaydoc::Display;
use bth_account_keys::AccountKey;
use bth_core::slip10::Slip10KeyGenerator;
use mc_rand::{CryptoRng, RngCore};
use prost::Message;
use serde::{Deserialize, Serialize};

/// An enumeration of errors which can occur when converting an
/// [`UncheckedMnemonicAccount`] to an
/// [`AccountKey`](bth_account_keys::AccountKey).
#[derive(Clone, Debug, Display, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Error {
    /// No mnemonic was provided
    NoMnemonic,
    /// The mnemonic was invalid: {0}
    InvalidMnemonic(String),
    /// No account index was provided
    NoAccountIndex,
}
/// A serialized mnemonic-based account key
#[derive(Clone, Eq, Hash, Ord, PartialOrd, PartialEq, Serialize, Deserialize, Message)]
pub struct UncheckedMnemonicAccount {
    /// The mnemonic string representation of the entropy
    #[prost(string, optional, tag = "1")]
    pub mnemonic: Option<String>,
    /// The account index the mnemonic is intended to work with
    #[prost(uint32, optional, tag = "2")]
    pub account_index: Option<u32>,
}

impl TryFrom<UncheckedMnemonicAccount> for AccountKey {
    type Error = Error;

    fn try_from(src: UncheckedMnemonicAccount) -> Result<AccountKey, Self::Error> {
        let mnemonic = Mnemonic::from_phrase(
            src.mnemonic.ok_or(Error::NoMnemonic)?.as_str(),
            Language::English,
        )
        .map_err(|e| Error::InvalidMnemonic(format!("{e}")))?;
        let slip10 = mnemonic.derive_slip10_key(src.account_index.ok_or(Error::NoAccountIndex)?);
        Ok(AccountKey::from(slip10))
    }
}

impl UncheckedMnemonicAccount {
    /// Construct an identity with a random mnemonic key
    pub fn random<T: RngCore + CryptoRng>(rng: &mut T) -> Self {
        let mut entropy = [0u8; 32];
        rng.fill_bytes(&mut entropy[..]);
        let mnemonic = Mnemonic::from_entropy(&entropy, Language::English);
        match mnemonic {
            Ok(v) => Self {
                mnemonic: Some(v.phrase().to_string()),
                ..Default::default()
            },
            Err(_) => Self {
                mnemonic: None,
                ..Default::default()
            },
        }
    }

}
