# Entropy Proof Integration Design

## Overview

This document specifies the integration of entropy proofs into Botho's transaction format as part of Phase 2 committed cluster tags. It addresses backward compatibility, migration strategy, and validation rules.

**Status**: Design Specification
**Related Issues**: #262 (Prototype), #263 (Benchmarks), #264 (Security Analysis), #265 (This Design)
**Prerequisites**:
- [Security Analysis](entropy-proof-security-analysis.md) - COMPLETED
- [Entropy-Weighted Decay](entropy-weighted-decay.md) - Design spec
- [Prototype Implementation](../../cluster-tax/src/entropy_decay.rs) - COMPLETED

## 1. Transaction Format Changes

### 1.1 Design Decision: Extended Signature Approach

After analyzing the three options from the issue, we recommend **Option C: Proof in Extended Signature**.

```rust
/// Extended transaction signature with cluster tag and entropy proofs.
///
/// This extends the existing ExtendedTxSignature to include entropy proofs
/// for Phase 2 entropy-weighted decay.
pub struct ExtendedTxSignature {
    /// Pseudo-tag-outputs, one per transaction input.
    /// These commit to the tag masses of the real inputs.
    pub pseudo_tag_outputs: Vec<PseudoTagOutput>,

    /// Proof that output tags correctly inherit from input pseudo-tags.
    pub conservation_proof: TagConservationProof,

    /// NEW (Phase 2B): Entropy proof for decay credit eligibility.
    /// Optional for backward compatibility during transition.
    pub entropy_proof: Option<EntropyProof>,
}
```

### 1.2 Rationale

| Option | Pros | Cons | Verdict |
|--------|------|------|---------|
| **A: Alongside existing proofs** | Simple addition | Breaks existing parsing | Rejected |
| **B: Combined proof** | Cleaner structure | Major refactor, breaks compatibility | Rejected |
| **C: Extended signature** | Preserves compatibility, natural fit | Slightly larger signature | **Recommended** |

Option C is recommended because:
1. **Minimal disruption**: The `ExtendedTxSignature` already contains tag-related proofs
2. **Natural grouping**: Entropy proofs are logically related to tag conservation
3. **Optional field**: The `Option<EntropyProof>` allows gradual adoption
4. **Existing pattern**: Follows the established pattern from Phase 1

### 1.3 New Data Structures

```rust
/// Proof that entropy delta meets threshold for decay credit.
///
/// Proves: entropy_after - entropy_before >= min_threshold
/// without revealing the actual entropy values.
#[derive(Clone, Debug)]
pub struct EntropyProof {
    /// Commitment to entropy before the transaction.
    /// C_before = H2_before * H_E + r_before * G
    pub entropy_before_commitment: CompressedRistretto,

    /// Commitment to entropy after the transaction.
    /// C_after = H2_after * H_E + r_after * G
    pub entropy_after_commitment: CompressedRistretto,

    /// Range proof: entropy_delta = entropy_after - entropy_before >= threshold
    /// Uses Bulletproof for O(log n) size.
    pub threshold_range_proof: BulletproofRangeProof,

    /// Linkage proof: ties entropy commitments to tag commitments.
    /// Proves entropy values are correctly computed from tag weights.
    pub linkage_proof: EntropyLinkageProof,
}

/// Proof linking entropy to tag mass distribution.
///
/// Proves that the committed entropy values were correctly derived
/// from the committed tag mass distribution.
#[derive(Clone, Debug)]
pub struct EntropyLinkageProof {
    /// Intermediate commitments for entropy calculation steps.
    /// These allow verification without revealing actual weights.
    pub intermediate_commitments: Vec<CompressedRistretto>,

    /// Schnorr proofs for each calculation step.
    pub step_proofs: Vec<SchnorrProof>,
}

/// Bulletproof-style range proof.
///
/// Compact proof that a committed value lies in range [0, 2^n).
/// For entropy threshold, we prove: excess = entropy_delta - threshold >= 0
#[derive(Clone, Debug)]
pub struct BulletproofRangeProof {
    /// Compressed proof data.
    pub proof_bytes: Vec<u8>,
}
```

### 1.4 Entropy Generator Derivation

Following the pattern established in `committed_tags.rs`:

```rust
/// Domain separator for entropy generator.
const ENTROPY_GENERATOR_DOMAIN_TAG: &[u8] = b"mc_entropy_value_generator";

/// Derive the generator for entropy commitments.
///
/// H_E is derived via hash-to-curve with unknown discrete log to G.
pub fn entropy_generator() -> RistrettoPoint {
    let mut hasher = Sha512::new();
    hasher.update(ENTROPY_GENERATOR_DOMAIN_TAG);
    RistrettoPoint::from_hash(hasher)
}
```

## 2. Backward Compatibility

### 2.1 Transition Strategy: Soft Fork

The entropy proof is **optional** during the transition period:

```rust
/// Transaction validation with version-aware entropy proof checking.
pub fn validate_entropy_proof(
    tx: &Transaction,
    extended_sig: &ExtendedTxSignature,
    config: &ConsensusConfig,
) -> Result<EntropyValidationResult, ValidationError> {
    // Phase 1: Entropy proof optional (entropy_proof = None allowed)
    if config.block_height < config.entropy_required_height {
        match &extended_sig.entropy_proof {
            Some(proof) => {
                // If provided, validate it
                verify_entropy_proof(proof, extended_sig)?;
                Ok(EntropyValidationResult::ProofValid)
            }
            None => {
                // Not provided, apply default (minimal) decay credit
                Ok(EntropyValidationResult::NotProvided)
            }
        }
    }
    // Phase 2: Entropy proof required for decay credit
    else if config.block_height < config.entropy_mandatory_height {
        match &extended_sig.entropy_proof {
            Some(proof) => {
                verify_entropy_proof(proof, extended_sig)?;
                Ok(EntropyValidationResult::ProofValid)
            }
            None => {
                // No proof = no decay credit (but tx still valid)
                Ok(EntropyValidationResult::NoDdecayCredit)
            }
        }
    }
    // Phase 3: Entropy proof mandatory
    else {
        match &extended_sig.entropy_proof {
            Some(proof) => {
                verify_entropy_proof(proof, extended_sig)?;
                Ok(EntropyValidationResult::ProofValid)
            }
            None => {
                Err(ValidationError::MissingEntropyProof)
            }
        }
    }
}

/// Result of entropy proof validation.
pub enum EntropyValidationResult {
    /// Proof provided and valid - full decay credit
    ProofValid,
    /// Proof not provided (transition period) - minimal decay credit
    NotProvided,
    /// Proof not provided (after transition) - no decay credit
    NoDecayCredit,
}
```

### 2.2 Transition Timeline

| Phase | Block Range | Entropy Proof | Decay Credit | TX Valid Without Proof |
|-------|-------------|---------------|--------------|------------------------|
| **Phase 1** | 0 - N | Optional | Full if provided, minimal if not | Yes |
| **Phase 2** | N - M | Recommended | Full if provided, none if not | Yes |
| **Phase 3** | M+ | Required | Full if provided, none if not | No (consensus reject) |

Specific block heights to be determined based on:
- Network upgrade coordination
- Wallet update cycles
- Exchange integration timelines

### 2.3 Serialization Compatibility

The serialization format uses versioned encoding:

```rust
impl ExtendedTxSignature {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();

        // Version byte
        let version = if self.entropy_proof.is_some() { 2u8 } else { 1u8 };
        bytes.push(version);

        // Existing fields (v1 and v2)
        bytes.extend_from_slice(&self.pseudo_tag_outputs_bytes());
        bytes.extend_from_slice(&self.conservation_proof.to_bytes());

        // New field (v2 only)
        if let Some(ref entropy_proof) = self.entropy_proof {
            bytes.extend_from_slice(&entropy_proof.to_bytes());
        }

        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DeserializationError> {
        if bytes.is_empty() {
            return Err(DeserializationError::EmptyInput);
        }

        let version = bytes[0];
        let mut cursor = 1;

        // Parse common fields
        let pseudo_tag_outputs = Self::parse_pseudo_tag_outputs(&bytes[cursor..])?;
        cursor += pseudo_tag_outputs.1; // bytes consumed

        let conservation_proof = TagConservationProof::from_bytes(&bytes[cursor..])?;
        cursor += conservation_proof.serialized_size();

        // Parse entropy proof (v2 only)
        let entropy_proof = if version >= 2 && cursor < bytes.len() {
            Some(EntropyProof::from_bytes(&bytes[cursor..])?)
        } else {
            None
        };

        Ok(Self {
            pseudo_tag_outputs: pseudo_tag_outputs.0,
            conservation_proof,
            entropy_proof,
        })
    }
}
```

## 3. Versioning

### 3.1 Transaction Version Field

```rust
/// Transaction version indicating supported features.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
}
```

### 3.2 Version Detection in Transaction

```rust
impl Transaction {
    /// Determine transaction version from structure.
    pub fn detected_version(&self) -> TransactionVersion {
        // Check for V3 indicators (entropy proof)
        if self.has_entropy_proof() {
            return TransactionVersion::V3;
        }

        // Check for V2 indicators (committed tags)
        if self.has_committed_tags() {
            return TransactionVersion::V2;
        }

        TransactionVersion::V1
    }

    fn has_entropy_proof(&self) -> bool {
        // Entropy proof presence detected from extended signature
        // This requires access to ExtendedTxSignature
        false // Placeholder - actual implementation depends on tx structure
    }

    fn has_committed_tags(&self) -> bool {
        // Check if any output has non-empty committed tag vector
        self.outputs.iter().any(|o| !o.cluster_tags.is_empty())
    }
}
```

## 4. Validation Rules

### 4.1 When Entropy Proof Is Required

| Condition | Entropy Proof Required | Decay Credit |
|-----------|------------------------|--------------|
| Transaction V1 | No | Phase 1 age-based |
| Transaction V2 | Optional | Full if provided |
| Transaction V3 | Yes | Based on proof |
| After mandatory height | Yes (consensus) | Rejected without |
| Claiming decay credit | Yes | Required to verify |
| Pure transfer (no decay) | No | N/A |

### 4.2 Validation Flow

```rust
/// Complete transaction validation including entropy proofs.
pub fn validate_transaction_v3(
    tx: &Transaction,
    extended_sig: &ExtendedTxSignature,
    utxo_set: &UtxoSet,
    config: &ConsensusConfig,
) -> Result<ValidationResult, ValidationError> {
    // 1. Basic structure validation (unchanged)
    tx.is_valid_structure()?;

    // 2. Ring signature validation (unchanged)
    tx.verify_ring_signatures()?;

    // 3. Tag conservation validation (Phase 2)
    if tx.detected_version().supports_committed_tags() {
        validate_tag_conservation(extended_sig)?;
    }

    // 4. NEW: Entropy proof validation (Phase 2B)
    let entropy_result = validate_entropy_proof(tx, extended_sig, config)?;

    // 5. Compute effective decay based on entropy result
    let decay_rate = compute_decay_rate(entropy_result, config);

    // 6. Validate output amounts with decay-adjusted conservation
    validate_amount_conservation(tx, utxo_set, decay_rate)?;

    Ok(ValidationResult::Valid { decay_rate })
}

/// Compute decay rate based on entropy proof validation.
fn compute_decay_rate(
    entropy_result: EntropyValidationResult,
    config: &ConsensusConfig,
) -> TagWeight {
    match entropy_result {
        EntropyValidationResult::ProofValid => {
            // Full decay credit (5%)
            config.base_decay_rate
        }
        EntropyValidationResult::NotProvided => {
            // Transition period: minimal decay (0.5%)
            config.base_decay_rate / 10
        }
        EntropyValidationResult::NoDecayCredit => {
            // No decay credit (0%)
            0
        }
    }
}
```

### 4.3 Entropy Proof Verification

```rust
/// Verify an entropy proof against tag commitments.
fn verify_entropy_proof(
    proof: &EntropyProof,
    extended_sig: &ExtendedTxSignature,
) -> Result<(), ValidationError> {
    // 1. Verify entropy_before commitment is valid point
    if proof.entropy_before_commitment.decompress().is_none() {
        return Err(ValidationError::InvalidEntropyCommitment);
    }

    // 2. Verify entropy_after commitment is valid point
    if proof.entropy_after_commitment.decompress().is_none() {
        return Err(ValidationError::InvalidEntropyCommitment);
    }

    // 3. Verify linkage proof (entropy derived from tags)
    verify_entropy_linkage(proof, extended_sig)?;

    // 4. Verify threshold range proof
    // Proves: entropy_after - entropy_before >= MIN_ENTROPY_THRESHOLD
    verify_threshold_range_proof(proof)?;

    Ok(())
}

/// Verify that entropy values are correctly linked to tag commitments.
fn verify_entropy_linkage(
    proof: &EntropyProof,
    extended_sig: &ExtendedTxSignature,
) -> Result<(), ValidationError> {
    // The linkage proof demonstrates that:
    // 1. entropy_before was computed from input pseudo-tag-outputs
    // 2. entropy_after was computed from output tag commitments
    //
    // This uses the same algebraic structure as tag conservation
    // but for the entropy calculation (sum of p_k * log(p_k))

    if proof.linkage_proof.intermediate_commitments.is_empty() {
        return Err(ValidationError::MissingLinkageProof);
    }

    // Verify each step proof
    for (i, step_proof) in proof.linkage_proof.step_proofs.iter().enumerate() {
        let expected_point = proof.linkage_proof.intermediate_commitments
            .get(i)
            .ok_or(ValidationError::MissingIntermediateCommitment)?;

        let context = entropy_linkage_context(i);
        if !step_proof.verify(expected_point, &context) {
            return Err(ValidationError::InvalidLinkageProof);
        }
    }

    Ok(())
}

fn entropy_linkage_context(step: usize) -> Vec<u8> {
    let mut context = b"mc_entropy_linkage_".to_vec();
    context.extend_from_slice(&(step as u64).to_le_bytes());
    context
}
```

## 5. Size Impact Analysis

### 5.1 Proof Size Breakdown

| Component | Size (bytes) | Notes |
|-----------|--------------|-------|
| `entropy_before_commitment` | 32 | Compressed Ristretto |
| `entropy_after_commitment` | 32 | Compressed Ristretto |
| `threshold_range_proof` | ~700 | Bulletproof (64-bit range) |
| `linkage_proof` | ~200-400 | Depends on cluster count |
| **Total EntropyProof** | **~964-1164** | Per transaction |

### 5.2 Transaction Size Impact

For a typical 2-input, 2-output transaction:

| Component | V2 (no entropy) | V3 (with entropy) | Delta |
|-----------|-----------------|-------------------|-------|
| Base transaction | ~1,400 | ~1,400 | 0 |
| CLSAG signatures (2) | ~1,400 | ~1,400 | 0 |
| Tag conservation | ~500 | ~500 | 0 |
| **Entropy proof** | **0** | **~1,000** | **+1,000** |
| **Total** | **~3,300** | **~4,300** | **+1,000 (~30%)** |

### 5.3 Aggregation Optimization

For transactions with multiple inputs, the entropy proof can be aggregated:

```rust
/// Aggregated entropy proof for multi-input transactions.
///
/// Instead of proving entropy threshold for each input separately,
/// we prove the aggregate entropy change across all inputs.
pub struct AggregatedEntropyProof {
    /// Single combined entropy_before (from all inputs)
    pub combined_entropy_before: CompressedRistretto,

    /// Single combined entropy_after (from all outputs)
    pub combined_entropy_after: CompressedRistretto,

    /// Single range proof for combined delta
    pub threshold_range_proof: BulletproofRangeProof,

    /// Linkage proofs (one per input + output)
    pub linkage_proofs: Vec<EntropyLinkageProof>,
}
```

Aggregation reduces proof size from O(n) to O(1) for the range proof component.

## 6. Implementation Checklist

### 6.1 Transaction Structure Updates

- [ ] Add `EntropyProof` struct to `cluster-tax/src/crypto/`
- [ ] Add `entropy_proof: Option<EntropyProof>` to `ExtendedTxSignature`
- [ ] Update serialization/deserialization for versioned format
- [ ] Add `TransactionVersion` enum
- [ ] Update `Transaction::detected_version()`

### 6.2 Validation Logic

- [ ] Implement `validate_entropy_proof()`
- [ ] Implement `verify_entropy_linkage()`
- [ ] Implement `verify_threshold_range_proof()`
- [ ] Update consensus validation to include entropy checks
- [ ] Add version-aware validation path

### 6.3 Proof Generation

- [ ] Implement `EntropyProofBuilder`
- [ ] Implement entropy commitment generation
- [ ] Implement Bulletproof range proof for threshold
- [ ] Implement linkage proof generation
- [ ] Add prover tests

### 6.4 RPC/API Changes

- [ ] Add entropy proof fields to transaction RPC responses
- [ ] Add entropy validation status to validation responses
- [ ] Update wallet API to include entropy proof in tx creation

### 6.5 Wallet Updates

- [ ] Update wallet to generate entropy proofs for V3 transactions
- [ ] Add entropy proof estimation for fee calculation
- [ ] Support both V2 and V3 transaction creation during transition

## 7. Migration Plan

### 7.1 Pre-Activation (Current)

1. Implement entropy proof structures and validation
2. Deploy code with entropy proofs disabled
3. Update wallets to support V3 transaction creation (optional)
4. Monitor network for V2 adoption

### 7.2 Phase 1: Soft Fork Activation

1. Set `entropy_required_height` via consensus rule update
2. Miners/validators accept V3 transactions with entropy proofs
3. V2 transactions continue to work with minimal decay credit
4. Wallets encouraged to upgrade

### 7.3 Phase 2: Mandatory Activation

1. Set `entropy_mandatory_height` after sufficient adoption
2. V2 transactions rejected after this height
3. All decay credit requires valid entropy proof
4. Network fully transitioned to entropy-weighted decay

### 7.4 Timeline Estimates

| Milestone | Estimated Time | Dependencies |
|-----------|----------------|--------------|
| Code complete | T+0 | #262, #263, #264 |
| Testnet deployment | T+2 weeks | Code review |
| Mainnet soft fork | T+6 weeks | Wallet updates |
| Mandatory activation | T+12 weeks | Network adoption |

## 8. Open Questions

### Q1: Should entropy proof be mandatory for all V3 transactions?

**Recommendation**: No, keep it optional initially. Transactions without entropy proof simply don't receive decay credit.

**Rationale**: This allows:
- Gradual wallet adoption
- Emergency transactions without proof generation
- Backward compatibility during transition

### Q2: How to handle entropy proof verification failures?

**Recommendation**: Treat as "no proof provided" rather than consensus failure during transition.

**Rationale**: Prevents network splits from proof generation bugs.

### Q3: Should we support aggregated proofs from day one?

**Recommendation**: Start with per-transaction proofs, add aggregation in follow-up.

**Rationale**: Simpler initial implementation, aggregation can be added transparently.

## References

1. [Entropy-Weighted Decay Design](entropy-weighted-decay.md)
2. [Security Analysis](entropy-proof-security-analysis.md)
3. [Committed Tags Implementation](../../cluster-tax/src/crypto/committed_tags.rs)
4. [Extended Signature Implementation](../../cluster-tax/src/crypto/extended_signature.rs)
5. Bulletproofs Paper: Bünz et al. (2018)

## Changelog

- 2026-01-08: Initial design specification (Issue #265)
