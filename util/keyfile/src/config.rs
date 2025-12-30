// Copyright (c) 2018-2022 The Botho Foundation
//! Configuration parameters for generating key files for a new user identity
use clap::Parser;
use std::path::PathBuf;

/// Configuration for generating key files for a new user identity
#[derive(Debug, Parser)]
pub struct KeyConfig {
    /// Output directory, defaults to current directory.
    #[clap(long, env = "BTH_OUTPUT_DIR")]
    pub output_dir: Option<PathBuf>,

    /// Seed to use when generating keys (e.g.
    /// 1234567812345678123456781234567812345678123456781234567812345678).
    #[clap(short, long, value_parser = bth_util_parse::parse_hex::<[u8; 32]>, env = "BTH_SEED", default_value = "0101010101010101010101010101010101010101010101010101010101010101")]
    pub seed: [u8; 32],
}
