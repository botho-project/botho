// Copyright (c) 2018-2022 The Botho Foundation

#![doc = include_str!("../README.md")]
#![deny(missing_docs)]

mod error;
mod json_format;
mod mnemonic_acct;
pub use json_format::RootIdentityJson;
pub use mnemonic_acct::UncheckedMnemonicAccount;
pub mod config;
pub mod keygen;

use crate::error::Error;
use bip39::Mnemonic;
use bth_account_keys::{AccountKey, PublicAddress, RootIdentity};
use bth_api::printable::{printable_wrapper, PrintableWrapper};
use std::{
    fs::File,
    io::{Read, Write},
    path::Path,
};

/// Write a user's account details to disk
pub fn write_keyfile<P: AsRef<Path>>(
    path: P,
    mnemonic: &Mnemonic,
    account_index: u32,
) -> Result<(), Error> {
    let json = UncheckedMnemonicAccount {
        mnemonic: Some(mnemonic.clone().into_phrase()),
        account_index: Some(account_index),
        fog_report_url: None,        // Fog support removed
        fog_report_id: None,         // Fog support removed
        fog_authority_spki: None,    // Fog support removed
    };
    Ok(serde_json::to_writer(File::create(path)?, &json)?)
}

/// Read a keyfile intended for use with the legacy `RootEntropy`
/// key-derivation method.
pub fn read_root_entropy_keyfile<P: AsRef<Path>>(path: P) -> Result<RootIdentity, Error> {
    read_root_entropy_keyfile_data(File::open(path)?)
}

/// Read keyfile data from the given buffer into a legacy `RootIdentity`
/// structure
pub fn read_root_entropy_keyfile_data<R: Read>(buffer: R) -> Result<RootIdentity, Error> {
    Ok(serde_json::from_reader::<R, RootIdentityJson>(buffer)?.into())
}

/// Read user mnemonic from disk
pub fn read_mnemonic_keyfile<P: AsRef<Path>>(path: P) -> Result<AccountKey, Error> {
    read_mnemonic_keyfile_data(File::open(path)?)
}

/// Read user root identity from any implementor of `Read`
pub fn read_mnemonic_keyfile_data<R: Read>(buffer: R) -> Result<AccountKey, Error> {
    Ok(serde_json::from_reader::<R, UncheckedMnemonicAccount>(buffer)?.try_into()?)
}

/// Read an account either in the RootIdentity format or the mnemonic format
/// from disk
pub fn read_keyfile<P: AsRef<Path>>(path: P) -> Result<AccountKey, Error> {
    read_keyfile_data(File::open(path)?)
}

/// Read an account key file in either format
pub fn read_keyfile_data<R: Read>(buffer: R) -> Result<AccountKey, Error> {
    let value = serde_json::from_reader::<R, serde_json::Value>(buffer)?;
    let obj = value
        .as_object()
        .ok_or_else(|| Error::Json("Expected json object".to_string()))?;
    if obj.contains_key("root_entropy") {
        let root_identity_json: RootIdentityJson = serde_json::from_value(value)?;
        let root_id = RootIdentity::from(root_identity_json);
        Ok(AccountKey::from(&root_id))
    } else {
        let mnemonic_json: UncheckedMnemonicAccount = serde_json::from_value(value)?;
        Ok(AccountKey::try_from(mnemonic_json)?)
    }
}

/// Write user public address to disk
pub fn write_pubfile<P: AsRef<Path>>(path: P, addr: &PublicAddress) -> Result<(), Error> {
    File::create(path)?.write_all(&bth_util_serial::encode(addr))?;
    Ok(())
}
/// Read user public address from disk
pub fn read_pubfile<P: AsRef<Path>>(path: P) -> Result<PublicAddress, Error> {
    read_pubfile_data(&mut File::open(path)?)
}

/// Read user pubfile from any implementor of `Read`
pub fn read_pubfile_data<R: Read>(buffer: &mut R) -> Result<PublicAddress, Error> {
    let data = {
        let mut data = Vec::new();
        buffer.read_to_end(&mut data)?;
        data
    };
    let result: PublicAddress = bth_util_serial::decode(&data)?;
    Ok(result)
}

/// Write user b58 public address to disk
pub fn write_b58pubfile<P: AsRef<Path>>(
    path: P,
    addr: &PublicAddress,
) -> Result<(), std::io::Error> {
    let wrapper = PrintableWrapper {
        wrapper: Some(printable_wrapper::Wrapper::PublicAddress(addr.into())),
    };

    let data = wrapper.b58_encode().map_err(to_io_error)?;

    File::create(path)?.write_all(data.as_ref())?;
    Ok(())
}

/// Read user b58 public address from disk
pub fn read_b58pubfile<P: AsRef<Path>>(path: P) -> Result<PublicAddress, std::io::Error> {
    read_b58pubfile_data(&mut File::open(path)?)
}

/// Read user b58 pubfile from any implementor of `Read`
pub fn read_b58pubfile_data<R: Read>(buffer: &mut R) -> Result<PublicAddress, std::io::Error> {
    let data = {
        let mut data = String::new();
        buffer.read_to_string(&mut data)?;
        data
    };

    let wrapper = PrintableWrapper::b58_decode(data).map_err(to_io_error)?;
    if let Some(printable_wrapper::Wrapper::PublicAddress(address)) = wrapper.wrapper.as_ref() {
        address.try_into().map_err(to_io_error)
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "Printable Wrapper did not contain public address",
        ))
    }
}

fn to_io_error<E: 'static + std::error::Error + Send + Sync>(err: E) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, Box::new(err))
}

#[cfg(test)]
mod testing {

    use super::*;
    use bip39::{Language, MnemonicType};
    use bth_core::slip10::Slip10KeyGenerator;

    /// Test that round-tripping through a keyfile gets the same
    /// result as creating the key directly.
    #[test]
    fn keyfile_roundtrip() {
        let dir = tempfile::tempdir().expect("Could not create temp dir");
        let mnemonic = Mnemonic::new(MnemonicType::Words24, Language::English);
        let path = dir.path().join("keyfile");
        write_keyfile(&path, &mnemonic, 0).expect("Could not write keyfile");
        let expected = AccountKey::from(mnemonic.derive_slip10_key(0));
        let actual = read_keyfile(&path).expect("Could not read keyfile");
        assert_eq!(expected, actual);
    }

    /// Test that writing a [`PublicAddress`](bth_account_keys::PublicAddress)
    /// and reading it back gets the same results.
    #[test]
    fn pubfile_roundtrip() {
        let mn = Mnemonic::new(MnemonicType::Words24, Language::English);

        let expected = AccountKey::from(mn.derive_slip10_key(0)).default_subaddress();

        let dir = tempfile::tempdir().expect("Could not create temporary directory");
        let path = dir.path().join("pubfile");
        write_pubfile(&path, &expected).expect("Could not write pubfile");
        let actual = read_pubfile(&path).expect("Could not read back pubfile");
        assert_eq!(expected, actual);
    }
}
