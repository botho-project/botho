# Ring Signature Tag Propagation: Design Specification

## Overview

This document specifies how cluster tags propagate through ring signatures while preserving sender privacy. Ring signatures hide which input is real by mixing it with decoys, but the progressive fee system requires correct tag propagation to calculate fees.

The core challenge: **calculate the output tag without revealing which input is real**.

## Problem Statement

### Background

Ring signatures provide sender privacy by including the real input among decoys:

```
Ring = [decoy_1, decoy_2, REAL_INPUT, decoy_3, ...]
```

The signature proves:
1. One ring member is being spent (without revealing which)
2. The spender knows the private key for one member
3. No double-spending (via key image)

### The Tag Propagation Problem

In a non-private transaction, tag propagation is straightforward:

```
Output tag = f(real_input_tag, decay_parameters)
```

With ring signatures, the verifier cannot identify `real_input_tag` because all ring members look equivalent. Yet we need:

1. **Correct tag propagation**: Output tags must derive from real input
2. **Privacy preservation**: Must not leak which input is real
3. **Verifiability**: Validators must verify fee is correct
4. **Gaming resistance**: Decoy selection must not allow fee manipulation

### Current State

`cluster-tax/src/age_decay.rs` provides:

```rust
pub struct RingDecayInfo {
    /// For each ring member, whether it's eligible for decay.
    pub member_eligibility: Vec<bool>,
}

impl RingDecayInfo {
    pub fn all_eligible(&self) -> bool;    // Simplest ZK case
    pub fn none_eligible(&self) -> bool;   // Simplest ZK case
    pub fn mixed_eligibility(&self) -> bool;  // Requires complex proof
}
```

This tracks age-based decay eligibility but doesn't address tag value propagation.

## Design Constraints

### Privacy Requirements

1. **No direct revelation**: Output must not encode real input index
2. **No timing attack**: Decoy selection must not leak real input
3. **No fee correlation**: Fee calculation must not reveal input identity
4. **Consistent verification**: All validators reach same conclusion

### Economic Requirements

1. **Progressive fees preserved**: Wealthy clusters still pay more
2. **No fee evasion**: Cannot select decoys to artificially lower fees
3. **Bounded manipulation**: Any gaming is limited in effect
4. **Legitimate decay enabled**: Commerce still provides tag diffusion

## Design Options

### Option 1: Deterministic Ring Aggregate

**Approach**: Output tag = deterministic function of ALL ring member tags.

```rust
fn ring_aggregate_tag(ring_tags: &[TagVector]) -> TagVector {
    // Simple average (value-weighted not possible - don't know values)
    let mut result = TagVector::new();
    for tag in ring_tags {
        for (cluster, weight) in tag.iter() {
            let current = result.get(cluster);
            result.set(cluster, current + weight / ring_tags.len() as u32);
        }
    }
    result
}
```

**Pros**:
- No privacy leak (deterministic from public data)
- Simple implementation
- Verifiable by anyone

**Cons**:
- Decoys "pollute" the output tag
- Attacker can manipulate by choosing specific decoys
- Tag progressivity is diluted by ~1/ring_size

**Gaming Analysis**:
- Attacker selects decoys with high background (low cluster attribution)
- Output tag becomes averaged toward background
- Fee reduction potential: `(ring_size - 1) / ring_size` of cluster factor

**Verdict**: Not recommended - too much gaming potential.

### Option 2: Zero-Knowledge Tag Proof

**Approach**: Include ZK proof that output tag correctly derives from real input.

```rust
struct RingTagProof {
    /// Commitment to output tag
    output_tag_commitment: PedersenCommitment,

    /// ZK proof: "I know index i such that:
    ///   - ring[i] is signed with my key
    ///   - output_tag = decay(ring_tags[i])"
    tag_derivation_proof: ZkProof,

    /// ZK proof: "fee >= expected_fee(output_tag)"
    fee_correctness_proof: ZkProof,
}
```

**Pros**:
- Correct tag propagation
- Full privacy preserved
- Decoys cannot affect output tag
- No gaming possible

**Cons**:
- Requires ZK proof infrastructure
- Increases transaction size (proof data)
- Higher computational cost
- Phase 2 complexity (committed tags required)

**Implementation Considerations**:
- Best suited for Phase 2 with committed tags
- Proof systems: Bulletproofs (no trusted setup) or Groth16 (smaller proofs)
- Can potentially aggregate with existing range proofs

**Verdict**: Best long-term solution, but requires Phase 2 infrastructure.

### Option 3: Conservative Decay (Recommended for Phase 1)

**Approach**: Apply decay based on youngest ring member (worst case from attacker's perspective).

```rust
fn conservative_ring_decay(
    ring_creation_blocks: &[u64],
    current_block: u64,
    config: &AgeDecayConfig,
) -> bool {
    // Use youngest member's age (most conservative)
    let youngest_creation = ring_creation_blocks.iter().max().copied().unwrap_or(0);
    config.is_eligible(youngest_creation, current_block)
}
```

For tag propagation in Phase 1:

```rust
fn conservative_tag_propagation(ring_tags: &[TagVector]) -> TagVector {
    // Use highest cluster factor among ring members (most conservative)
    ring_tags.iter()
        .max_by_key(|t| t.total_attributed())
        .cloned()
        .unwrap_or_default()
}
```

**Pros**:
- Simple implementation
- No privacy leak (uses public information)
- Conservative = harder to game
- Works with current Phase 1 public tags

**Cons**:
- Legitimate users may pay slightly higher fees than necessary
- Decoy selection still matters (but penalizes, not helps, gaming)

**Gaming Analysis**:
- Attacker wants LOW fees, so needs decoys with LOW cluster factors
- But "highest cluster factor" selection means any high-factor decoy penalizes them
- Gaming becomes counter-productive: must carefully select ALL-low decoys

**Decoy Constraint**: To prevent fee inflation attacks (malicious node selecting high-factor decoys to inflate someone else's fees), we can require:

```rust
fn valid_decoy_selection(real_tag: &TagVector, decoy_tags: &[&TagVector]) -> bool {
    let real_factor = cluster_factor(real_tag);
    for decoy_tag in decoy_tags {
        let decoy_factor = cluster_factor(decoy_tag);
        // Decoys must not have significantly higher cluster factor
        if decoy_factor > real_factor * 1.5 {
            return false;
        }
    }
    true
}
```

This constraint is enforced during transaction creation (by the wallet) and ensures the conservative estimate doesn't vastly exceed the true fee.

**Verdict**: Recommended for Phase 1 - simple, safe, good properties.

### Option 4: Constrained Ring Composition

**Approach**: Require ring members to have similar properties.

```rust
fn valid_ring_composition(ring_creation_blocks: &[u64], ring_tags: &[TagVector]) -> bool {
    // Age similarity
    let ages: Vec<u64> = ring_creation_blocks.iter()
        .map(|&b| current_block - b)
        .collect();
    let age_variance = variance(&ages);
    if age_variance > MAX_AGE_VARIANCE {
        return false;
    }

    // Tag similarity
    let factors: Vec<f64> = ring_tags.iter()
        .map(|t| cluster_factor(t))
        .collect();
    let factor_variance = variance(&factors);
    if factor_variance > MAX_FACTOR_VARIANCE {
        return false;
    }

    true
}
```

**Pros**:
- Bounds the effect of any single decoy
- Makes conservative estimate closer to true value
- Limits gaming potential

**Cons**:
- Reduces anonymity set (fewer valid decoys)
- Complexity in decoy selection
- Privacy concern: ring composition may leak information

**Trade-off Analysis**:

| Constraint Strictness | Anonymity Set | Fee Accuracy | Privacy Leak Risk |
|-----------------------|---------------|--------------|-------------------|
| Very loose            | Large         | Low          | Low               |
| Moderate              | Medium        | Medium       | Medium            |
| Very strict           | Small         | High         | High              |

**Verdict**: Useful as a secondary measure, but shouldn't be too strict.

## Recommended Design

### Phase 1: Public Tags with Conservative Propagation

For the initial launch with public tags, use **Option 3 (Conservative Decay)** with **Option 4 (Constrained Ring Composition)** as a secondary measure.

#### Implementation

1. **Decay Eligibility**: Use youngest ring member
   ```rust
   // In age_decay.rs
   impl RingDecayInfo {
       /// Get conservative decay decision (youngest member determines eligibility)
       pub fn conservative_decay_eligible(&self) -> bool {
           // If ANY member is NOT eligible (too young), no decay
           // This is conservative: decay only if ALL are eligible
           self.all_eligible()
       }
   }
   ```

2. **Tag Selection for Fee Calculation**: Use maximum cluster factor
   ```rust
   // In validate.rs
   pub fn ring_cluster_factor(ring_tags: &[TagVector]) -> f64 {
       ring_tags.iter()
           .map(|t| calculate_cluster_factor(t))
           .fold(0.0, f64::max)
   }
   ```

3. **Decoy Constraints** (soft, wallet-enforced):
   ```rust
   // In wallet/decoy_selection.rs
   pub fn select_decoys(
       real_utxo: &Utxo,
       utxo_pool: &[Utxo],
       config: &DecoyConfig,
   ) -> Vec<Utxo> {
       let real_age = current_block - real_utxo.creation_block;
       let real_factor = cluster_factor(&real_utxo.tags);

       utxo_pool.iter()
           .filter(|u| {
               let age = current_block - u.creation_block;
               let factor = cluster_factor(&u.tags);

               // Age within 2x
               age > real_age / 2 && age < real_age * 2 &&
               // Factor not more than 1.5x (prevents inflation attack)
               factor <= real_factor * 1.5
           })
           .take(config.ring_size - 1)
           .collect()
   }
   ```

4. **Output Tag Derivation**:
   ```rust
   // The output tag equals the real input tag (after decay if eligible)
   // Validators cannot verify this directly in Phase 1
   // But the conservative fee calculation bounds the manipulation
   ```

#### Security Properties

| Property | Status | Mechanism |
|----------|--------|-----------|
| Privacy preserved | Yes | Ring signature hides real input |
| Correct propagation | Partial | Real tag propagates, but unverifiable |
| Fee evasion bounded | Yes | Conservative max prevents underpayment |
| Gaming resistant | Yes | Gaming is counter-productive |
| Inflation attack prevented | Yes | Decoy constraints limit overpayment |

### Phase 2: Committed Tags with ZK Proofs

For full privacy with verified correctness, use **Option 2 (ZK Tag Proof)**.

#### Architecture

```
Transaction Structure (Phase 2):
├── inputs: Vec<RingInput>
│   ├── ring: Vec<ReducedTxOut>      // Decoys + real
│   ├── key_image: KeyImage
│   └── signature: CLSAG
├── outputs: Vec<TaggedOutput>
│   ├── commitment: PedersenCommitment  // Amount
│   ├── tag_commitment: PedersenCommitment  // Tag
│   └── public_key: RistrettoPublic
├── fee: u64
└── proofs: TransactionProofs
    ├── range_proofs: Vec<Bulletproof>
    ├── tag_conservation_proof: TagProof
    └── fee_correctness_proof: FeeProof
```

#### ZK Circuit Requirements

**Tag Conservation Proof** proves:
1. "I know index `i` such that ring[i] was signed"
2. "Output tag commitments = f(input_tag_commitment[i], decay)"
3. "Sum of output tags = input tag (conservation)"

**Fee Correctness Proof** proves:
1. "Committed fee value >= expected_fee(committed_tag)"
2. "Fee formula was correctly applied"

#### Implementation Approach

```rust
// In crypto/ring_tag_proof.rs

pub struct TagProof {
    /// Bulletproof for tag conservation
    conservation_proof: BulletproofPlus,

    /// Auxiliary commitments for verification
    aux_commitments: Vec<PedersenCommitment>,
}

impl TagProof {
    pub fn create(
        ring_tags: &[TagCommitment],
        real_index: usize,
        output_tags: &[TagCommitment],
        blinding_factors: &BlindingFactors,
    ) -> Self {
        // ZK proof that output = decay(ring[real_index])
        // without revealing real_index
        todo!("Implement using Bulletproofs or similar")
    }

    pub fn verify(
        &self,
        ring_tags: &[TagCommitment],
        output_tags: &[TagCommitment],
    ) -> Result<(), Error> {
        // Verify the proof without knowing real_index
        todo!("Implement verification")
    }
}
```

## Implementation Roadmap

### Phase 1 (Current Priority)

1. **Extend RingDecayInfo** for conservative decay
   - Add `conservative_decay_eligible()` method
   - Integrate with transaction validation

2. **Add ring cluster factor calculation**
   - New function in `validate.rs`
   - Uses maximum cluster factor from ring

3. **Update fee validation**
   - Fee must meet conservative estimate
   - Reject transactions with insufficient fees

4. **Wallet decoy selection guidelines**
   - Document constraints for decoy selection
   - Implement reference selection algorithm

### Phase 2 (Future)

1. **Tag commitment infrastructure**
   - Pedersen commitments for tags
   - Blinding factor derivation

2. **ZK proof system selection**
   - Evaluate Bulletproofs vs alternatives
   - Consider aggregation with range proofs

3. **Proof generation/verification**
   - Tag conservation proof
   - Fee correctness proof

4. **Transaction format update**
   - Add committed tags to outputs
   - Add proof fields

## Open Questions

### Resolved

1. **Can decoy selection leak tag information?**
   - Answer: Yes, if unconstrained. Soft constraints limit this.

2. **Should we constrain decoy tags?**
   - Answer: Yes, via wallet guidelines (not consensus).

### Remaining

1. **Minimum ring size for tag privacy?**
   - Needs analysis based on tag distribution in practice
   - Suggested: minimum 11 (same as Monero)

2. **How does tag-based selection affect ring signature privacy?**
   - If tags cluster naturally, may reduce effective anonymity
   - Mitigation: ensure diverse tag distribution over time

3. **Can tag proofs aggregate with range proofs?**
   - Potential for significant size savings
   - Requires research on proof system compatibility

4. **Impact of constrained decoy selection on privacy?**
   - Quantitative privacy analysis needed
   - Trade-off between fee accuracy and anonymity set size

## References

- `cluster-tax/src/age_decay.rs` - RingDecayInfo implementation
- `cluster-tax/src/tag.rs` - TagVector and cluster factor
- `cluster-tax/src/validate.rs` - Transaction validation
- `crypto/ring-signature/src/ring_signature/clsag.rs` - CLSAG implementation
- `docs/design/cluster-tag-decay.md` - Age-based decay design
- Issue #230: Transaction validation integration
- Issue #232: Phase 2 ZK proofs epic

## Changelog

- 2025-01-05: Initial design specification for issue #233
