// Copyright (c) 2024 Cadence Foundation

//! Quorum set management for consensus.

use libp2p::PeerId;
use std::collections::HashSet;

/// Validates and builds quorum sets
pub struct QuorumBuilder {
    /// Required threshold (e.g., 3 in a 3-of-5)
    threshold: u32,
    /// Member peer IDs
    members: HashSet<PeerId>,
}

impl QuorumBuilder {
    /// Create a new quorum builder
    pub fn new(threshold: u32) -> Self {
        Self {
            threshold,
            members: HashSet::new(),
        }
    }

    /// Add a member to the quorum
    pub fn add_member(&mut self, peer_id: PeerId) {
        self.members.insert(peer_id);
    }

    /// Get the current member count
    pub fn member_count(&self) -> usize {
        self.members.len()
    }

    /// Get the threshold
    pub fn threshold(&self) -> u32 {
        self.threshold
    }

    /// Check if we have enough members for a valid quorum
    pub fn is_valid(&self) -> bool {
        self.members.len() >= self.threshold as usize
    }

    /// Get all members
    pub fn members(&self) -> Vec<PeerId> {
        self.members.iter().copied().collect()
    }
}

/// Result of quorum validation
#[derive(Debug, Clone)]
pub struct QuorumValidation {
    /// Whether the quorum is valid
    pub is_valid: bool,
    /// Current member count
    pub member_count: usize,
    /// Required threshold
    pub threshold: u32,
    /// Message describing the validation result
    pub message: String,
}

impl QuorumValidation {
    /// Create a validation result
    pub fn new(builder: &QuorumBuilder) -> Self {
        let is_valid = builder.is_valid();
        let member_count = builder.member_count();
        let threshold = builder.threshold();

        let message = if is_valid {
            format!(
                "Quorum is valid: {}/{} members (threshold: {})",
                member_count, member_count, threshold
            )
        } else {
            format!(
                "Quorum needs {} more members: {}/{} (threshold: {})",
                threshold as usize - member_count,
                member_count,
                threshold,
                threshold
            )
        };

        Self {
            is_valid,
            member_count,
            threshold,
            message,
        }
    }
}
