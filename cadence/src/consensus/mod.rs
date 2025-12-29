// Copyright (c) 2024 Cadence Foundation

//! SCP consensus integration for Cadence.
//!
//! This module provides:
//! - ConsensusValue: The value type for SCP (transaction hashes)
//! - ConsensusService: Manages SCP node and message handling
//! - TransactionValidator: Separate validation for mining vs transfer transactions
//! - BlockBuilder: Constructs blocks from externalized consensus values
//! - Integration with gossip for SCP message propagation

mod block_builder;
mod service;
mod validation;
mod value;

pub use block_builder::{BlockBuildError, BlockBuilder, BuiltBlock};
pub use service::{ConsensusConfig, ConsensusEvent, ConsensusService, ScpMessage};
pub use validation::{BatchValidationResult, TransactionValidator, ValidationError};
pub use value::{ConsensusValue, ConsensusValueHash};
