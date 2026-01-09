//! Cryptographic integration for cluster taxation.
//!
//! This module defines how cluster tags integrate with the existing
//! Botho transaction model.
//!
//! ## Design Phases
//!
//! ### Phase 1: Public Tags
//! - Tags stored in plaintext alongside TxOut
//! - Privacy from ring signatures (which input is real is hidden)
//! - Simple validation of tag inheritance
//!
//! ### Phase 2: Committed Tags
//! - Tag masses as Pedersen commitments
//! - ZK proofs for tag inheritance
//! - Full privacy for tag distribution
//!
//! ### Phase 2B: Entropy Proofs
//! - Entropy-weighted decay credit
//! - Linkage proofs tying entropy to tag commitments
//! - Threshold range proofs for minimum entropy delta

mod committed_tags;
mod entropy_proof;
mod entropy_validation;
mod extended_signature;
mod serialization;
mod tagged_output;
mod validation;

pub use committed_tags::{
    blinding_generator,
    cluster_generator,
    fee_generator,
    total_mass_generator,
    wealth_generator,
    ClusterConservationProof,
    // Phase 2/3: ZK fee verification types
    CommittedFeeProof,
    CommittedFeeProofBuilder,
    CommittedFeeProofVerifier,
    CommittedFeeProver,
    CommittedFeeVerifier,
    CommittedTagMass,
    CommittedTagVector,
    CommittedTagVectorSecret,
    LinearRelationProof,
    RangeProof,
    SchnorrProof,
    SegmentFeeProof,
    SegmentOrProof,
    TagConservationProof,
    TagConservationProver,
    TagConservationVerifier,
    TagMassSecret,
    WealthLinkageProof,
};
pub use extended_signature::{
    ExtendedSignatureBuilder, ExtendedSignatureVerifier, ExtendedTxSignature, PseudoTagOutput,
    RingTagData, TagInheritanceProof,
};

// Phase 2B: Entropy proof generation and verification
pub use entropy_proof::{
    entropy_generator, EntropyLinkageProof, EntropyProof, EntropyProofBuilder,
    EntropyProofVerifier, EntropyRangeProof, ENTROPY_SCALE, MIN_ENTROPY_THRESHOLD_SCALED,
};

// Phase 2B: Consensus-level entropy validation
pub use entropy_validation::{
    // Core validation function
    validate_entropy_proof,
    // Decay rate computation
    compute_decay_rate,
    // Result and error types
    EntropyValidationResult,
    EntropyValidationError,
    // Version and configuration
    TransactionVersion,
    EntropyConsensusConfig,
    EntropyPhase,
};

pub use serialization::DeserializeError;
pub use tagged_output::{CompactTagVector, TaggedTxOut};
pub use validation::{validate_tag_inheritance, TagValidationError};
