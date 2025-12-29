// Copyright (c) 2024 Cadence Foundation

//! Network module for gossip-based peer discovery and quorum management.
//!
//! This module handles:
//! - Starting the gossip service for peer discovery
//! - Displaying discovered peers in a table format
//! - Suggesting and validating quorum set configurations

mod discovery;
mod quorum;
mod reputation;

pub use discovery::{CadenceBehaviour, NetworkDiscovery, NetworkEvent, PeerTableEntry};
pub use quorum::{QuorumBuilder, QuorumValidation};
pub use reputation::{PeerReputation, ReputationManager};
