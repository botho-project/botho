//! Botho Thin Wallet
//!
//! A standalone wallet client that manages its own keys locally and
//! communicates with untrusted Botho nodes via JSON-RPC.
//!
//! ## Security Model
//!
//! - Private keys never leave the wallet
//! - Nodes are untrusted (can lie about balance, withhold transactions)
//! - Transaction signing happens locally
//! - Multiple node connections for verification

pub mod discovery;
pub mod keys;
pub mod rpc_pool;
pub mod storage;
pub mod transaction;

pub mod commands;

pub use discovery::NodeDiscovery;
pub use keys::WalletKeys;
pub use rpc_pool::RpcPool;
pub use storage::EncryptedWallet;
