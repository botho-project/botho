//! Cluster-based progressive transaction fees.
//!
//! This module implements a system where transaction fees are determined by
//! the "cluster ancestry" of coins rather than account identity. Each coin
//! carries decaying identity tags that reflect its origin, and clusters with
//! more concentrated wealth pay higher fees.
//!
//! Key concepts:
//! - **Cluster**: An identity derived from coin creation (e.g., mining rewards).
//!   Not a group of accounts, but a lineage marker that fades through trade.
//! - **Tag Vector**: Each account/UTXO carries weights indicating what fraction
//!   of its value traces back to each cluster origin.
//! - **Cluster Wealth**: The total value in the system tagged to a given cluster.
//! - **Progressive Fee**: Fee rate increases with cluster wealth, so concentrated
//!   holdings pay more regardless of how they structure transactions.

pub mod analysis;
pub mod crypto;
#[cfg(feature = "cli")]
pub mod simulation;
mod cluster;
mod fee_curve;
mod tag;
mod transfer;

pub use cluster::{ClusterId, ClusterWealth};
pub use fee_curve::{FeeCurve, FeeRateBps};
pub use tag::{TagVector, TagWeight, TAG_WEIGHT_SCALE};
pub use transfer::{execute_transfer, mint, Account, TransferConfig, TransferError, TransferResult};
