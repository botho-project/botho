// Copyright (c) 2024 Botho Foundation

//! Botho node library - a privacy-preserving mined cryptocurrency.
//!
//! This library provides the core functionality for the Botho node,
//! including blockchain types, networking, consensus, and wallet support.

#![deny(clippy::print_stdout)]

pub mod address;
pub mod block;
pub mod config;
pub mod consensus;
pub mod decoy_selection;
pub mod ledger;
pub mod mempool;
pub mod monetary;
pub mod network;
pub mod node;
pub mod rpc;
pub mod telemetry;
pub mod transaction;
pub mod wallet;

#[cfg(feature = "pq")]
pub mod transaction_pq;

// Re-export commands module for CLI binary
pub mod commands;
