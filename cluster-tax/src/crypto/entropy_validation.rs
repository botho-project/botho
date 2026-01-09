//! Consensus-level entropy proof validation for Phase 2B entropy-weighted decay.
//!
//! This module provides the consensus validation layer for entropy proofs,
//! implementing version-aware validation and phase-based requirements.
//!
//! ## Relationship with `entropy_proof` module
//!
//! - `entropy_proof`: Provides proof structures (`EntropyProof`) and cryptographic
//!   verification (`EntropyProofBuilder`, `EntropyProofVerifier`)
//! - `entropy_validation` (this module): Provides consensus-level validation with
//!   version awareness, phase transitions, and decay rate computation
//!
//! ## Version-Aware Validation
//!
//! The validation behavior depends on the current phase:
//! - **Phase 1 (Optional)**: Entropy proofs optional, minimal decay credit if not provided
//! - **Phase 2 (Recommended)**: Same as Phase 1, but signals upcoming requirement
//! - **Phase 3 (RequiredForCredit)**: Required for decay credit, but tx still valid without
//! - **Phase 4 (Mandatory)**: Consensus rejection without proof

use super::entropy_proof::{EntropyProof, EntropyProofVerifier, MIN_ENTROPY_THRESHOLD_SCALED};
use crate::TagWeight;

// ============================================================================
// Transaction Version
// ============================================================================

/// Transaction version indicating supported features.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TransactionVersion {
    /// V1: Original transaction format
    /// - Basic stealth addresses
    /// - CLSAG ring signatures
    V1 = 1,

    /// V2: Phase 1 committed tags
    /// - Committed cluster tags
    /// - Tag conservation proofs
    /// - ExtendedTxSignature without entropy proof
    V2 = 2,

    /// V3: Phase 2 entropy proofs
    /// - All V2 features
    /// - Entropy proof in ExtendedTxSignature
    /// - Entropy-weighted decay
    V3 = 3,
}

impl TransactionVersion {
    /// Check if this version supports entropy proofs.
    pub fn supports_entropy_proof(&self) -> bool {
        *self as u8 >= 3
    }

    /// Check if this version supports committed tags.
    pub fn supports_committed_tags(&self) -> bool {
        *self as u8 >= 2
    }

    /// Parse version from byte.
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            1 => Some(Self::V1),
            2 => Some(Self::V2),
            3 => Some(Self::V3),
            _ => None,
        }
    }

    /// Convert to byte.
    pub fn to_byte(self) -> u8 {
        self as u8
    }
}

impl Default for TransactionVersion {
    fn default() -> Self {
        Self::V2
    }
}

// ============================================================================
// Validation Result and Errors
// ============================================================================

/// Result of entropy proof validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntropyValidationResult {
    /// Proof provided and valid - full decay credit.
    ProofValid,

    /// Proof not provided (transition period) - minimal decay credit.
    NotProvided,

    /// Proof not provided (after transition) - no decay credit.
    NoDecayCredit,
}

impl EntropyValidationResult {
    /// Check if this result grants any decay credit.
    pub fn has_decay_credit(&self) -> bool {
        matches!(self, Self::ProofValid | Self::NotProvided)
    }

    /// Check if this result grants full decay credit.
    pub fn has_full_decay_credit(&self) -> bool {
        matches!(self, Self::ProofValid)
    }
}

/// Errors that can occur during entropy proof validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntropyValidationError {
    /// The entropy proof failed cryptographic verification.
    InvalidProof,

    /// Entropy proof is required but was not provided.
    MissingEntropyProof,
}

impl std::fmt::Display for EntropyValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidProof => {
                write!(f, "entropy proof failed cryptographic verification")
            }
            Self::MissingEntropyProof => {
                write!(f, "entropy proof required but not provided")
            }
        }
    }
}

impl std::error::Error for EntropyValidationError {}

// ============================================================================
// Consensus Configuration
// ============================================================================

/// Configuration for entropy validation phases.
#[derive(Clone, Debug)]
pub struct EntropyConsensusConfig {
    /// Block height at which entropy proofs become recommended.
    /// Before this: entropy proofs optional, not provided = minimal decay.
    pub entropy_recommended_height: u64,

    /// Block height at which entropy proofs become required for decay credit.
    /// After this: not provided = no decay credit (but tx still valid).
    pub entropy_required_height: u64,

    /// Block height at which entropy proofs become mandatory.
    /// After this: not provided = consensus rejection.
    pub entropy_mandatory_height: u64,

    /// Base decay rate when entropy proof is valid.
    /// Uses same scale as TagWeight (e.g., 50_000 = 5%).
    pub base_decay_rate: TagWeight,

    /// Minimal decay rate during transition period (no proof).
    /// Typically base_decay_rate / 10.
    pub minimal_decay_rate: TagWeight,

    /// Minimum entropy threshold for decay credit (scaled).
    /// Imported from entropy_proof module.
    pub min_entropy_threshold: u64,
}

impl Default for EntropyConsensusConfig {
    fn default() -> Self {
        Self {
            // Placeholder heights - to be determined by network upgrade coordination
            entropy_recommended_height: 1_000_000,
            entropy_required_height: 2_000_000,
            entropy_mandatory_height: 3_000_000,
            base_decay_rate: 50_000,      // 5%
            minimal_decay_rate: 5_000,    // 0.5%
            min_entropy_threshold: MIN_ENTROPY_THRESHOLD_SCALED,
        }
    }
}

impl EntropyConsensusConfig {
    /// Create a config for testnet with earlier activation.
    pub fn testnet() -> Self {
        Self {
            entropy_recommended_height: 10_000,
            entropy_required_height: 20_000,
            entropy_mandatory_height: 30_000,
            ..Default::default()
        }
    }

    /// Determine the current phase based on block height.
    pub fn phase(&self, block_height: u64) -> EntropyPhase {
        if block_height < self.entropy_recommended_height {
            EntropyPhase::Optional
        } else if block_height < self.entropy_required_height {
            EntropyPhase::Recommended
        } else if block_height < self.entropy_mandatory_height {
            EntropyPhase::RequiredForCredit
        } else {
            EntropyPhase::Mandatory
        }
    }
}

/// The current phase of entropy proof requirements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntropyPhase {
    /// Entropy proofs are optional, minimal decay credit if not provided.
    Optional,

    /// Entropy proofs are recommended, minimal decay credit if not provided.
    Recommended,

    /// Entropy proofs required for decay credit, but tx valid without.
    RequiredForCredit,

    /// Entropy proofs mandatory - tx rejected without.
    Mandatory,
}

// ============================================================================
// Core Validation Functions
// ============================================================================

/// Validate an entropy proof for a transaction at the consensus level.
///
/// This is the main entry point for entropy proof validation, implementing
/// version-aware validation as specified in the design document.
///
/// # Arguments
/// * `entropy_proof` - The optional entropy proof from ExtendedTxSignature
/// * `config` - Consensus configuration with phase heights
/// * `block_height` - Current block height
///
/// # Returns
/// * `Ok(EntropyValidationResult)` - Validation result indicating decay credit
/// * `Err(EntropyValidationError)` - If proof is invalid or missing when required
pub fn validate_entropy_proof(
    entropy_proof: Option<&EntropyProof>,
    config: &EntropyConsensusConfig,
    block_height: u64,
) -> Result<EntropyValidationResult, EntropyValidationError> {
    let phase = config.phase(block_height);
    let verifier = EntropyProofVerifier::with_threshold(config.min_entropy_threshold);

    match (phase, entropy_proof) {
        // Phase 1/2: Proof provided - validate it
        (EntropyPhase::Optional | EntropyPhase::Recommended, Some(proof)) => {
            if verifier.verify(proof) {
                Ok(EntropyValidationResult::ProofValid)
            } else {
                Err(EntropyValidationError::InvalidProof)
            }
        }

        // Phase 1/2: No proof - minimal decay credit
        (EntropyPhase::Optional | EntropyPhase::Recommended, None) => {
            Ok(EntropyValidationResult::NotProvided)
        }

        // Phase 3: Proof provided - validate it
        (EntropyPhase::RequiredForCredit, Some(proof)) => {
            if verifier.verify(proof) {
                Ok(EntropyValidationResult::ProofValid)
            } else {
                Err(EntropyValidationError::InvalidProof)
            }
        }

        // Phase 3: No proof - no decay credit (but tx valid)
        (EntropyPhase::RequiredForCredit, None) => {
            Ok(EntropyValidationResult::NoDecayCredit)
        }

        // Phase 4 (Mandatory): Proof provided - validate it
        (EntropyPhase::Mandatory, Some(proof)) => {
            if verifier.verify(proof) {
                Ok(EntropyValidationResult::ProofValid)
            } else {
                Err(EntropyValidationError::InvalidProof)
            }
        }

        // Phase 4 (Mandatory): No proof - consensus rejection
        (EntropyPhase::Mandatory, None) => {
            Err(EntropyValidationError::MissingEntropyProof)
        }
    }
}

// ============================================================================
// Decay Rate Computation
// ============================================================================

/// Compute the effective decay rate based on entropy validation result.
///
/// # Arguments
/// * `result` - The entropy validation result
/// * `config` - Consensus configuration with decay rates
///
/// # Returns
/// The decay rate to apply (in TAG_WEIGHT_SCALE units).
pub fn compute_decay_rate(
    result: &EntropyValidationResult,
    config: &EntropyConsensusConfig,
) -> TagWeight {
    match result {
        EntropyValidationResult::ProofValid => {
            // Full decay credit
            config.base_decay_rate
        }
        EntropyValidationResult::NotProvided => {
            // Transition period: minimal decay
            config.minimal_decay_rate
        }
        EntropyValidationResult::NoDecayCredit => {
            // No decay credit
            0
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{CommittedTagVectorSecret, EntropyProofBuilder};
    use crate::{ClusterId, TAG_WEIGHT_SCALE};
    use rand_core::OsRng;
    use std::collections::HashMap;

    fn create_test_secret(value: u64, clusters: &[(u64, u32)]) -> CommittedTagVectorSecret {
        let mut tags = HashMap::new();
        for &(cluster_id, weight) in clusters {
            tags.insert(ClusterId(cluster_id), weight);
        }
        CommittedTagVectorSecret::from_plaintext(value, &tags, &mut OsRng)
    }

    fn create_valid_entropy_proof() -> EntropyProof {
        // Input: single cluster (0 entropy)
        let input_secret = create_test_secret(1_000_000, &[(1, TAG_WEIGHT_SCALE)]);

        // Output: two clusters (>0 entropy)
        let output_secret = create_test_secret(
            1_000_000,
            &[
                (1, TAG_WEIGHT_SCALE / 2),
                (2, TAG_WEIGHT_SCALE / 2),
            ],
        );

        EntropyProofBuilder::new(vec![input_secret], vec![output_secret])
            .prove(&mut OsRng)
            .expect("Should generate valid proof")
    }

    #[test]
    fn test_transaction_version_ordering() {
        assert!(TransactionVersion::V1 < TransactionVersion::V2);
        assert!(TransactionVersion::V2 < TransactionVersion::V3);
    }

    #[test]
    fn test_transaction_version_features() {
        assert!(!TransactionVersion::V1.supports_committed_tags());
        assert!(!TransactionVersion::V1.supports_entropy_proof());

        assert!(TransactionVersion::V2.supports_committed_tags());
        assert!(!TransactionVersion::V2.supports_entropy_proof());

        assert!(TransactionVersion::V3.supports_committed_tags());
        assert!(TransactionVersion::V3.supports_entropy_proof());
    }

    #[test]
    fn test_transaction_version_byte_roundtrip() {
        for version in [TransactionVersion::V1, TransactionVersion::V2, TransactionVersion::V3] {
            let byte = version.to_byte();
            let restored = TransactionVersion::from_byte(byte);
            assert_eq!(restored, Some(version));
        }
        assert_eq!(TransactionVersion::from_byte(0), None);
        assert_eq!(TransactionVersion::from_byte(4), None);
    }

    #[test]
    fn test_consensus_config_phases() {
        let config = EntropyConsensusConfig::default();

        assert_eq!(config.phase(0), EntropyPhase::Optional);
        assert_eq!(config.phase(500_000), EntropyPhase::Optional);
        assert_eq!(config.phase(1_500_000), EntropyPhase::Recommended);
        assert_eq!(config.phase(2_500_000), EntropyPhase::RequiredForCredit);
        assert_eq!(config.phase(3_500_000), EntropyPhase::Mandatory);
    }

    #[test]
    fn test_consensus_config_testnet() {
        let config = EntropyConsensusConfig::testnet();

        assert_eq!(config.phase(0), EntropyPhase::Optional);
        assert_eq!(config.phase(15_000), EntropyPhase::Recommended);
        assert_eq!(config.phase(25_000), EntropyPhase::RequiredForCredit);
        assert_eq!(config.phase(35_000), EntropyPhase::Mandatory);
    }

    #[test]
    fn test_validate_entropy_proof_optional_phase_no_proof() {
        let config = EntropyConsensusConfig::default();

        let result = validate_entropy_proof(None, &config, 0);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), EntropyValidationResult::NotProvided);
    }

    #[test]
    fn test_validate_entropy_proof_optional_phase_with_proof() {
        let config = EntropyConsensusConfig::default();
        let proof = create_valid_entropy_proof();

        let result = validate_entropy_proof(Some(&proof), &config, 0);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), EntropyValidationResult::ProofValid);
    }

    #[test]
    fn test_validate_entropy_proof_mandatory_phase_no_proof() {
        let config = EntropyConsensusConfig::default();

        let result = validate_entropy_proof(None, &config, 4_000_000);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            EntropyValidationError::MissingEntropyProof
        );
    }

    #[test]
    fn test_validate_entropy_proof_mandatory_phase_with_proof() {
        let config = EntropyConsensusConfig::default();
        let proof = create_valid_entropy_proof();

        let result = validate_entropy_proof(Some(&proof), &config, 4_000_000);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), EntropyValidationResult::ProofValid);
    }

    #[test]
    fn test_validate_entropy_proof_required_phase_no_proof() {
        let config = EntropyConsensusConfig::default();

        let result = validate_entropy_proof(None, &config, 2_500_000);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), EntropyValidationResult::NoDecayCredit);
    }

    #[test]
    fn test_compute_decay_rate() {
        let config = EntropyConsensusConfig::default();

        assert_eq!(
            compute_decay_rate(&EntropyValidationResult::ProofValid, &config),
            config.base_decay_rate
        );

        assert_eq!(
            compute_decay_rate(&EntropyValidationResult::NotProvided, &config),
            config.minimal_decay_rate
        );

        assert_eq!(
            compute_decay_rate(&EntropyValidationResult::NoDecayCredit, &config),
            0
        );
    }

    #[test]
    fn test_entropy_validation_result_methods() {
        assert!(EntropyValidationResult::ProofValid.has_decay_credit());
        assert!(EntropyValidationResult::ProofValid.has_full_decay_credit());

        assert!(EntropyValidationResult::NotProvided.has_decay_credit());
        assert!(!EntropyValidationResult::NotProvided.has_full_decay_credit());

        assert!(!EntropyValidationResult::NoDecayCredit.has_decay_credit());
        assert!(!EntropyValidationResult::NoDecayCredit.has_full_decay_credit());
    }

    #[test]
    fn test_entropy_validation_error_display() {
        let err1 = EntropyValidationError::InvalidProof;
        let err2 = EntropyValidationError::MissingEntropyProof;

        assert!(err1.to_string().contains("cryptographic verification"));
        assert!(err2.to_string().contains("required but not provided"));
    }
}
