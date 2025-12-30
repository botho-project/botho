// Copyright (c) 2024 The Botho Foundation

//! Chain watchers for monitoring deposits and burns.

mod bth;
mod ethereum;

pub use bth::BthWatcher;
pub use ethereum::EthereumWatcher;
