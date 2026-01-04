//! Botho Exchange Scanner
//!
//! A client-side deposit detection tool for cryptocurrency exchanges
//! integrating with the Botho blockchain. This library provides:
//!
//! - Efficient output scanning using precomputed subaddress lookup tables
//! - Arbitrary subaddress range support (0 to 2^64)
//! - Sync state persistence for resumable scanning
//! - Multiple output handlers (stdout, webhook, database)
//!
//! # Architecture
//!
//! The scanner connects to a Botho node's RPC endpoint and polls for new
//! outputs using the `chain_getOutputs` method. Each output is checked against
//! a precomputed table of subaddress spend public keys for O(1) ownership
//! detection.
//!
//! # Security Model
//!
//! The exchange's view private key never leaves this scanner. The node only
//! provides raw blockchain data, and all cryptographic operations happen
//! locally.

pub mod config;
pub mod deposit;
pub mod output;
pub mod scanner;
pub mod subaddress;
pub mod sync;

pub use config::{OutputMode, ScannerConfig};
pub use deposit::DetectedDeposit;
pub use scanner::ExchangeScanner;
pub use sync::SyncState;
