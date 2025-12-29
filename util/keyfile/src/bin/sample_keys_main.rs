// Copyright (c) 2018-2022 The Botho Foundation
#![deny(missing_docs)]
//! Create some default keys for use in demos and testing
use clap::Parser;
use bth_util_keyfile::config::KeyConfig;

#[derive(Debug, Parser)]
struct Config {
    #[clap(flatten)]
    pub general: KeyConfig,

    /// Number of user keys to generate.
    #[clap(short, long, default_value = "10", env = "MC_NUM")]
    pub num: usize,
}

fn main() {
    let config = Config::parse();

    let path = config
        .general
        .output_dir
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap().join("keys"));

    println!("Writing {} keys to {:?}", config.num, path);

    bth_util_keyfile::keygen::write_default_keyfiles(path, config.num, config.general.seed)
        .unwrap();
}
