// Copyright (c) 2024 Botho Foundation
//
//! Common test utilities for e2e integration tests.
//!
//! This module provides shared infrastructure for running multi-node
//! SCP consensus networks with LMDB-backed ledgers. Tests can focus
//! on their specific scenarios without duplicating network setup code.
//!
//! # Example
//!
//! ```ignore
//! use common::{TestNetwork, TestNetworkConfig, mine_block, get_wallet_balance};
//!
//! let mut network = TestNetwork::build(TestNetworkConfig::default());
//! mine_block(&network, 0);
//! let balance = get_wallet_balance(&network, &network.wallets[0]);
//! network.stop();
//! ```

mod constants;
mod consensus;
mod mining;
mod network;
mod transactions;
mod wallets;

pub use constants::*;
pub use consensus::*;
pub use mining::*;
pub use network::*;
pub use transactions::*;
pub use wallets::*;
