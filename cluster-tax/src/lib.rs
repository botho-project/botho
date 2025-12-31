#![deny(clippy::print_stdout)]

//! Botho fee system with size-based progressive pricing.
//!
//! This module implements Botho's fee model with two key features:
//!
//! 1. **Size-based fees**: Fees scale with transaction size in bytes, ensuring
//!    larger transactions (e.g., PQ-Private with ~63 KB signatures) pay more.
//!
//! 2. **Progressive wealth taxation**: A cluster factor (1x-6x) multiplies the
//!    size fee, discouraging wealth concentration.
//!
//! ## Transaction Types
//!
//! | Type             | Ring Signature | Fee Formula                           |
//! |------------------|----------------|---------------------------------------|
//! | Standard-Private | CLSAG (~700B)  | fee_per_byte × size × cluster_factor  |
//! | PQ-Private       | LION (~63 KB)  | fee_per_byte × size × cluster_factor  |
//! | Minting          | N/A            | No fee (PoW reward claim)             |
//!
//! ## Key Concepts
//!
//! - **Cluster**: An identity derived from coin creation (minting rewards). A
//!   lineage marker that fades through trade via decay.
//! - **Tag Vector**: Each UTXO carries weights indicating what fraction of its
//!   value traces back to each cluster origin.
//! - **Cluster Wealth**: Total value in the system tagged to a given cluster.
//! - **Cluster Factor**: Wealthy clusters pay 1x-6x the base fee.
//!
//! ## Rationale
//!
//! - **Size-based fees** ensure fair pricing regardless of transaction type.
//! - **Progressive taxation** discourages wealth concentration by increasing
//!   fees for wealthy clusters.
//! - **Minting transactions** create new coins via PoW and establish new
//!   clusters.

pub mod analysis;
pub mod crypto;
pub mod dynamic_fee;
pub mod monetary;
pub mod signing;
#[cfg(feature = "cli")]
pub mod simulation;
pub mod validate;

mod age_decay;
mod block_decay;
mod cluster;
mod fee_curve;
mod tag;
mod transfer;

pub use cluster::{ClusterId, ClusterWealth};

// ============================================================================
// Monetary Policy (Canonical)
// ============================================================================
//
// The Two-Phase Monetary Model is the canonical approach:
// - Phase 1 (Halving): Fixed rewards with halving schedule, timing-based
//   difficulty
// - Phase 2 (Tail): Fixed tail reward, inflation-targeting difficulty
//
// Key insight: Difficulty should adapt to hit monetary targets, not rewards.
// This gives minters predictable income while absorbing fee volatility.
pub use monetary::{DifficultyController, MonetaryPolicy, MonetaryState, MonetaryStats};

pub use crypto::{CommittedTagVector, CommittedTagVectorSecret, ExtendedTxSignature, RingTagData};
pub use dynamic_fee::{DynamicFeeBase, DynamicFeeState, FeeSuggestion};
pub use fee_curve::{
    count_outputs_with_memos, ClusterFactorCurve, FeeConfig, FeeCurve, FeeRateBps, TransactionType,
};
pub use signing::{
    create_tag_signature, verify_tag_signature, TagSigningConfig, TagSigningError, TagSigningInput,
    TagSigningOutput, TagSigningResult,
};
pub use age_decay::{apply_age_decay, AgeDecayConfig, RingDecayInfo};
pub use block_decay::{
    AndDecayConfig, AndTagVector, BlockAwareTagVector, BlockDecayConfig, RateLimitedDecayConfig,
    RateLimitedTagVector,
};
pub use tag::{TagVector, TagWeight, TAG_WEIGHT_SCALE};
pub use transfer::{
    execute_transfer, mint, Account, TransferConfig, TransferError, TransferResult,
};
pub use validate::{
    validate_committed_tag_structure, validate_committed_tags, CommittedTagConfig,
    CommittedTagValidationError, CommittedTagValidationResult,
};
