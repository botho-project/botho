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
//!
//! ### V2 lottery payouts: "trusts own node" mode (#1642)
//!
//! A V2 lottery payout row carries a derivation context (`lotteryContext`:
//! base index `j` and cumulative tweak `T`) that the wallet takes from its
//! node without authenticating it against the block's V2 body root. The
//! wallet scans the base target `P − T·G` at `j` and only signs with a key
//! `x` that proves `x·G = P` for the row's own target. So a node that forges
//! a context can only cause a **missed payout** (nothing is discovered) or a
//! **spend the ledger rejects**; it cannot redirect funds or obtain a key.
//! Complete payout discovery therefore assumes the wallet talks to a node the
//! operator runs (the stress signer's deployment). Legacy context-less
//! lottery rows are excluded from spending, as before (#1450). See
//! [`rpc_pool::LotteryContext`].

pub mod decoy_selection;
pub mod discovery;
pub mod fee_estimation;
pub mod keys;
#[cfg(test)]
mod lottery_fixtures;
pub mod ring_builder;
pub mod rpc_pool;
pub mod secmem;
pub mod storage;
#[cfg(all(unix, feature = "pq"))]
pub mod stress;
pub mod transaction;
/// Legacy `botho-tx-v1` transaction types, quarantined (see module docs).
pub mod transaction_legacy;

pub mod commands;

pub use decoy_selection::{
    select_decoys, select_decoys_with_fallback, validate_decoys, DecoySelectionConfig,
    DecoySelectionError, DecoySelectionResult, UtxoCandidate,
};
pub use discovery::NodeDiscovery;
pub use fee_estimation::{CachedFeeRate, FeeEstimator, StoredTags};
pub use keys::WalletKeys;
pub use rpc_pool::{FaucetRequestResult, NetworkFeeRate, RpcPool};
pub use storage::EncryptedWallet;
