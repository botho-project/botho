// Copyright (c) 2024 Cadence Foundation

//! SCP consensus integration for Cadence.
//!
//! This module provides:
//! - ConsensusValue: The value type for SCP (transaction hashes)
//! - ConsensusService: Manages SCP node and message handling
//! - Integration with gossip for SCP message propagation

mod value;
mod service;

pub use value::{ConsensusValue, ConsensusValueHash};
pub use service::{ConsensusService, ConsensusConfig, ConsensusEvent, ScpMessage};
