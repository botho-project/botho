// Copyright (c) 2024 Botho Foundation

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quorum_builder_new() {
        let builder = QuorumBuilder::new(3);
        assert_eq!(builder.threshold(), 3);
        assert_eq!(builder.member_count(), 0);
        assert!(!builder.is_valid());
    }

    #[test]
    fn test_quorum_builder_add_members() {
        let mut builder = QuorumBuilder::new(2);

        let peer1 = PeerId::random();
        let peer2 = PeerId::random();

        builder.add_member(peer1);
        assert_eq!(builder.member_count(), 1);
        assert!(!builder.is_valid());

        builder.add_member(peer2);
        assert_eq!(builder.member_count(), 2);
        assert!(builder.is_valid());
    }

    #[test]
    fn test_quorum_builder_duplicate_members() {
        let mut builder = QuorumBuilder::new(2);
        let peer = PeerId::random();

        builder.add_member(peer);
        builder.add_member(peer); // Same peer

        assert_eq!(builder.member_count(), 1); // Should not count duplicates
    }

    #[test]
    fn test_quorum_builder_members_list() {
        let mut builder = QuorumBuilder::new(2);
        let peer1 = PeerId::random();
        let peer2 = PeerId::random();

        builder.add_member(peer1);
        builder.add_member(peer2);

        let members = builder.members();
        assert_eq!(members.len(), 2);
        assert!(members.contains(&peer1));
        assert!(members.contains(&peer2));
    }

    #[test]
    fn test_quorum_validation_valid() {
        let mut builder = QuorumBuilder::new(2);
        builder.add_member(PeerId::random());
        builder.add_member(PeerId::random());

        let validation = QuorumValidation::new(&builder);
        assert!(validation.is_valid);
        assert_eq!(validation.member_count, 2);
        assert_eq!(validation.threshold, 2);
        assert!(validation.message.contains("valid"));
    }

    #[test]
    fn test_quorum_validation_invalid() {
        let mut builder = QuorumBuilder::new(3);
        builder.add_member(PeerId::random());

        let validation = QuorumValidation::new(&builder);
        assert!(!validation.is_valid);
        assert_eq!(validation.member_count, 1);
        assert_eq!(validation.threshold, 3);
        assert!(validation.message.contains("needs 2 more"));
    }

    #[test]
    fn test_quorum_threshold_of_one() {
        let mut builder = QuorumBuilder::new(1);
        assert!(!builder.is_valid());

        builder.add_member(PeerId::random());
        assert!(builder.is_valid());
    }

    #[test]
    fn test_quorum_zero_threshold() {
        let builder = QuorumBuilder::new(0);
        // Zero threshold means valid with zero members
        assert!(builder.is_valid());
    }
}
