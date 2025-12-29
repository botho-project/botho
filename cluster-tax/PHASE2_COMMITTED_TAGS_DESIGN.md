# Phase 2: Committed Cluster Tags with ZK Proofs

## Overview

Phase 1 stores cluster tags in plaintext on TxOut. While ring signatures hide which
input is the real one, an observer can still see the tag distributions of all
outputs, potentially enabling correlation attacks over time.

Phase 2 provides full privacy by:
1. Committing to tag masses using Pedersen commitments
2. Proving tag conservation (with decay) in zero knowledge
3. Proving fee sufficiency without revealing individual cluster contributions

## Cryptographic Primitives

### Pedersen Commitments

We use the existing Ristretto-based Pedersen commitment scheme:

```
C = v * H + r * G
```

Where:
- `v` is the value (tag mass in our case)
- `r` is a random blinding factor
- `H` is a generator derived from the cluster ID
- `G` is the standard basepoint

### Cluster-Specific Generators

For each cluster `k`, we derive a generator:

```rust
fn cluster_generator(cluster_id: ClusterId) -> RistrettoPoint {
    let mut hasher = Blake2b512::new();
    hasher.update(b"mc_cluster_tag_generator");
    hasher.update(cluster_id.0.to_le_bytes());
    RistrettoPoint::from_hash(hasher)
}
```

This follows the same pattern as token-specific generators in the existing codebase.

## Data Structures

### Committed Tag Mass

Instead of storing `(cluster_id, weight)` pairs, we store commitments to tag masses:

```rust
/// A commitment to tag mass for a single cluster.
/// mass = value * weight, so this hides both the output value and tag weight.
pub struct CommittedTagMass {
    /// The cluster this commitment refers to.
    pub cluster_id: ClusterId,

    /// Pedersen commitment: C = mass * H_k + blinding * G
    /// where H_k = cluster_generator(cluster_id)
    pub commitment: CompressedCommitment,
}

/// Full committed tag vector for a TxOut.
pub struct CommittedTagVector {
    /// Commitments for each cluster with non-zero weight.
    /// Sorted by cluster_id for deterministic ordering.
    pub entries: Vec<CommittedTagMass>,

    /// Commitment to total attributed mass (for background calculation).
    /// C_total = sum(mass_k) * H_total + r_total * G
    pub total_commitment: CompressedCommitment,
}
```

### Tag Blinding Data (Private)

The sender retains blinding factors for proof generation:

```rust
/// Private data for one committed tag entry.
pub struct TagMassSecret {
    pub cluster_id: ClusterId,
    pub mass: u64,           // value * weight
    pub blinding: Scalar,
}

/// Private data for a full committed tag vector.
pub struct CommittedTagVectorSecret {
    pub entries: Vec<TagMassSecret>,
    pub total_mass: u64,
    pub total_blinding: Scalar,
}
```

## The Ring Signature Challenge

In MobileCoin's RingCT, each TxIn contains a ring of possible inputs, and the MLSAG
ring signature proves that exactly one is being spent without revealing which one.

For cluster tags, we face a challenge: we need to prove properties about the
*aggregate* input tags, but we don't know which inputs are real.

### Solution: Pseudo-Tag-Outputs

We use the same approach as pseudo-outputs for amounts:

1. For each TxIn, the prover creates a "pseudo-tag-output" that commits to the
   real input's tag masses
2. The MLSAG ring signature is extended to prove that the pseudo-tag-output
   matches the real input's committed tags
3. Conservation is proven between pseudo-tag-outputs and actual outputs

```
TxIn[0].ring = [TxOut_a, TxOut_b, TxOut_c]  // One is real
TxIn[1].ring = [TxOut_d, TxOut_e, TxOut_f]  // One is real

PseudoTagOutput[0] = commitment to real input from TxIn[0]
PseudoTagOutput[1] = commitment to real input from TxIn[1]

Conservation: sum(PseudoTagOutputs) * (1-decay) = sum(OutputTags)
```

## ZK Proofs Required

### 1. Tag Mass Conservation Proof

For each cluster `k`, prove:

```
sum(output_mass_k) = (1 - decay_rate) * sum(input_mass_k)
```

In commitment form, using homomorphic properties:

```
sum(C_output_k) = (1 - decay) * sum(C_pseudo_input_k) + r_diff * G
```

Where `r_diff` is a correction term for blinding factors.

**Proof approach**: Schnorr-style proof of knowledge of the blinding difference.

```rust
pub struct TagConservationProof {
    /// Per-cluster proofs that masses are conserved (with decay).
    pub cluster_proofs: Vec<ClusterConservationProof>,

    /// Proof that output tag masses sum correctly (no inflation).
    pub sum_proof: SumValidityProof,
}

pub struct ClusterConservationProof {
    pub cluster_id: ClusterId,

    /// Schnorr proof of knowledge of blinding difference.
    /// Proves: sum(C_out) - (1-decay)*sum(C_in) = r * G for known r
    pub blinding_proof: SchnorrProof,
}
```

### 2. Range Proofs for Tag Masses

Each tag mass commitment needs a range proof to ensure:
- Mass is non-negative
- Mass does not exceed the output value (weight <= 100%)

```rust
pub struct TagMassRangeProof {
    /// Bulletproof that each mass is in [0, value]
    /// Combined with existing amount range proofs for efficiency.
    pub range_proof: RangeProof,
}
```

### 3. Fee Sufficiency Proof

Prove that the paid fee meets the progressive rate requirement:

```
fee >= sum_k(mass_k * rate_k) / total_value
```

This is tricky because `rate_k` depends on public cluster wealth (known), but
`mass_k` is hidden in commitments.

**Approach**: Commit to the required fee, prove it's less than or equal to actual fee.

```rust
pub struct FeeSufficiencyProof {
    /// Commitment to the computed required fee.
    pub required_fee_commitment: CompressedCommitment,

    /// Proof that required_fee was computed correctly from tag masses.
    pub computation_proof: FeeComputationProof,

    /// Proof that actual_fee >= required_fee.
    pub comparison_proof: ComparisonProof,
}
```

The `FeeComputationProof` proves:
```
C_required = sum_k(rate_k * C_mass_k) / total_value
```

Since `rate_k` is public (derived from public cluster wealth), this is a linear
combination proof.

## Extended MLSAG for Tags

The MLSAG ring signature must be extended to commit to pseudo-tag-outputs.

### Current MLSAG Structure

For each ring member `i`:
- `L0 = r_{i,0} * G + c_i * P_i` (onetime address)
- `R0 = r_{i,0} * Hp(P_i) + c_i * I` (key image)
- `L1 = r_{i,1} * G + c_i * (C_pseudo - C_input)` (amount balance)

### Extended MLSAG for Tags

Add additional terms for each cluster with tags:

```
L_{k+2} = r_{i,k+2} * G + c_i * (C_pseudo_tag_k - C_input_tag_k)
```

Where:
- `C_pseudo_tag_k` is the pseudo-tag-output commitment for cluster k
- `C_input_tag_k` is the input's tag commitment for cluster k

The challenge becomes:
```
c_{i+1} = H(msg | I | L0 | R0 | L1 | L2 | L3 | ...)
```

This proves that for the real input, the pseudo-tag-output exactly matches the
input's tag masses.

## Signature Structure

```rust
pub struct ExtendedRingMLSAG {
    /// Standard MLSAG fields
    pub c_zero: CurveScalar,
    pub responses: Vec<CurveScalar>,  // Now longer: 2 + num_clusters per ring member
    pub key_image: KeyImage,

    /// Tag-related data
    pub pseudo_tag_commitments: Vec<CommittedTagMass>,
}

pub struct SignatureRctBulletproofsWithTags {
    /// Ring signatures (extended for tags)
    pub ring_signatures: Vec<ExtendedRingMLSAG>,

    /// Pseudo-output commitments (amounts)
    pub pseudo_output_commitments: Vec<CompressedCommitment>,

    /// Range proofs (amounts)
    pub range_proofs: Vec<Vec<u8>>,

    /// Tag conservation proofs
    pub tag_conservation_proof: TagConservationProof,

    /// Fee sufficiency proof
    pub fee_sufficiency_proof: FeeSufficiencyProof,

    /// Range proofs for tag masses (combined)
    pub tag_mass_range_proof: TagMassRangeProof,
}
```

## Optimization: Aggregate Cluster Commitments

Instead of committing to each cluster separately (which leaks the number of
clusters), we can aggregate:

### Approach 1: Fixed-Size Commitment Vector

Use a fixed number of slots (e.g., 16) regardless of actual cluster count.
Unused slots commit to zero.

### Approach 2: Single Aggregate Commitment

Commit to a polynomial evaluation:
```
C_agg = sum_k(mass_k * H_k) + r * G
```

Conservation becomes:
```
C_agg_output = (1-decay) * C_agg_input + r_diff * G
```

This hides the number of clusters and their identities, but makes fee
computation more complex.

## Implementation Phases

### Phase 2a: Basic Committed Tags (No Fee Proof)
1. Add committed tag structures
2. Extend MLSAG for tag pseudo-outputs
3. Add tag conservation proofs
4. Tag mass range proofs

This provides tag privacy but uses a simplified flat fee rate.

### Phase 2b: Full Progressive Fee Proof
1. Fee computation proofs
2. Comparison proofs
3. Integration with cluster wealth oracle

### Phase 2c: Optimization
1. Aggregate commitments
2. Proof batching
3. Constant-size proofs regardless of cluster count

## Security Considerations

### Timing Attacks
- Tag proof generation/verification should be constant-time
- Number of clusters should not leak through timing

### Linkability
- Same cluster IDs across transactions could enable correlation
- Consider: randomized cluster ID mapping per transaction with proof of validity

### Trusted Setup
- Bulletproofs require no trusted setup
- Extended MLSAG uses discrete log assumptions (standard)

## Transaction Size Impact

Current TxOut size: ~300 bytes

With Phase 1 (plaintext tags, 16 clusters max):
- Add ~200 bytes per TxOut

With Phase 2 (committed tags, 16 clusters max):
- Commitments: 16 * 32 = 512 bytes
- Range proofs: ~800 bytes (aggregated)
- Conservation proofs: ~200 bytes
- Total: ~1500 bytes additional per TxOut

With Phase 2c optimization (aggregate commitment):
- Single aggregate commitment: 32 bytes
- Aggregate proofs: ~500 bytes
- Total: ~600 bytes additional per TxOut

## Open Questions

1. **Cluster Wealth Oracle**: How is cluster wealth computed and proven?
   - Option A: Public on-chain aggregate (derived from all committed tags)
   - Option B: Trusted aggregator with signature
   - Option C: ZK proof of correct aggregation

2. **New Cluster Assignment**: When minting, how is cluster ID assigned?
   - Derived from miner identity?
   - Random with proof of freshness?
   - Block-deterministic?

3. **Background Weight Handling**: How to prove background weight constraints?
   - Implicit from sum constraint
   - Explicit commitment with proof

4. **Decay Rate Verification**: Is decay rate fixed or variable?
   - Fixed: simpler proofs
   - Variable: needs additional proof of correct rate application

## Next Steps

1. Implement `CommittedTagMass` and `CommittedTagVector` types
2. Implement cluster generator derivation
3. Design and implement extended MLSAG
4. Implement tag conservation Schnorr proofs
5. Integrate with existing Bulletproofs for range proofs
6. Design fee sufficiency proof system
