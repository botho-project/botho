// Copyright (c) 2018-2022 The Botho Foundation
#![deny(missing_docs)]

//! A CLI tool for generating individual Botho identities

use bip39::{Language, Mnemonic};
use clap::Parser;
use bth_util_keyfile::{config::KeyConfig, keygen};
use rand::{RngCore, SeedableRng};
use rand_hc::Hc128Rng;

/// Keygen config.
#[derive(Debug, Parser)]
struct Config {
    #[clap(flatten)]
    pub general: KeyConfig,

    /// The key name.
    pub name: String,
}

fn main() {
    let config = Config::parse();
    let path = config
        .general
        .output_dir
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let name = config.name.as_str();

    let mut csprng = Hc128Rng::from_seed(config.general.seed);

    let mut entropy = [0u8; 32];
    csprng.fill_bytes(&mut entropy[..]);
    let mnemonic = Mnemonic::from_entropy(&entropy, Language::English)
        .expect("Could not create mnemonic from entropy");

    println!("Writing to {path:?}");

    keygen::write_keyfiles(path, name, &mnemonic, 0).expect("Could not write keyfile");
}
