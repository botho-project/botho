// Copyright (c) 2024 Botho Foundation

//! SCP consensus integration for Botho.
//!
//! This module provides:
//! - ConsensusValue: The value type for SCP (transaction hashes)
//! - ConsensusService: Manages SCP node and message handling
//! - TransactionValidator: Separate validation for minting vs transfer
//!   transactions
//! - BlockBuilder: Constructs blocks from externalized consensus values
//! - Integration with gossip for SCP message propagation

mod block_builder;
pub mod lottery;
mod service;
mod validation;
mod value;

pub use block_builder::{BlockBuildError, BlockBuilder, BuiltBlock};
pub use lottery::{
    draw_lottery_winners, split_fees, validate_block_lottery, verify_lottery_result,
    utxo_to_candidate, BlockLotteryResult, LotteryFeeConfig, LotteryStats,
    LotteryValidationError,
};
pub use service::{ConsensusConfig, ConsensusEvent, ConsensusService, ScpMessage};
pub use validation::{BatchValidationResult, TransactionValidator, ValidationError};
pub use value::{ConsensusValue, ConsensusValueHash};
