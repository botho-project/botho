//! Botho fee system with privacy-tiered pricing.
//!
//! This module implements Botho's dual-incentive fee model:
//!
//! 1. **Privacy as a priced resource**: Private transactions cost more because
//!    they impose verification burden and reduce network transparency.
//!
//! 2. **Progressive wealth taxation**: For private transactions, wealthy clusters
//!    pay a multiplier, limiting wealth concentration.
//!
//! ## Transaction Types
//!
//! | Type    | Privacy | Fee                                    |
//! |---------|---------|----------------------------------------|
//! | Plain   | None    | 0.05% flat (transparent, Bitcoin-like) |
//! | Hidden  | Full    | 0.2% × cluster_factor (1x-6x)          |
//! | Mining  | N/A     | No fee (PoW reward claim)              |
//!
//! ## Key Concepts
//!
//! - **Cluster**: An identity derived from coin creation (mining rewards).
//!   A lineage marker that fades through trade via decay.
//! - **Tag Vector**: Each UTXO carries weights indicating what fraction of its
//!   value traces back to each cluster origin.
//! - **Cluster Wealth**: Total value in the system tagged to a given cluster.
//! - **Cluster Factor**: For hidden transactions, wealthy clusters pay 1x-6x
//!   the base privacy fee.
//!
//! ## Rationale
//!
//! - **Plain transactions** enable cheap, auditable transfers for those who
//!   don't need privacy (exchanges, public payments, transparency by choice).
//! - **Hidden transactions** pay for the societal cost of moving money in the
//!   dark. Whales can opt out by going transparent.
//! - **Mining transactions** create new coins via PoW and establish new clusters.

pub mod analysis;
pub mod crypto;
pub mod emission;
pub mod monetary;
#[cfg(feature = "cli")]
pub mod simulation;
pub mod signing;
pub mod validate;

mod cluster;
mod fee_curve;
mod tag;
mod transfer;

pub use cluster::{ClusterId, ClusterWealth};
pub use emission::{EmissionConfig, EmissionController, EmissionState};
pub use fee_curve::{ClusterFactorCurve, FeeConfig, FeeCurve, FeeRateBps, TransactionType};
pub use monetary::{DifficultyController, MonetaryPolicy, MonetaryState, MonetaryStats};
pub use tag::{TagVector, TagWeight, TAG_WEIGHT_SCALE};
pub use transfer::{execute_transfer, mint, Account, TransferConfig, TransferError, TransferResult};
pub use validate::{
    validate_committed_tags, validate_committed_tag_structure,
    CommittedTagConfig, CommittedTagValidationError, CommittedTagValidationResult,
};
pub use signing::{
    create_tag_signature, verify_tag_signature,
    TagSigningConfig, TagSigningError, TagSigningInput, TagSigningOutput, TagSigningResult,
};
pub use crypto::{
    CommittedTagVector, CommittedTagVectorSecret, RingTagData, ExtendedTxSignature,
};
