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

pub mod decoy_selection;
pub mod discovery;
pub mod fee_estimation;
pub mod keys;
pub mod rpc_pool;
pub mod secmem;
pub mod storage;
pub mod transaction;

pub mod commands;

pub use decoy_selection::{
    select_decoys, select_decoys_with_fallback, validate_decoys, DecoySelectionConfig,
    DecoySelectionError, DecoySelectionResult, UtxoCandidate,
};
pub use discovery::NodeDiscovery;
pub use fee_estimation::{CachedFeeRate, FeeEstimator, StoredTags};
pub use keys::WalletKeys;
pub use rpc_pool::{NetworkFeeRate, RpcPool};
pub use storage::EncryptedWallet;
