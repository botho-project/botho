# Entropy Proof Aggregation Research

**Status**: Phase A Feasibility Study
**Issue**: #260
**Related**: #232 (Phase 2 committed tags), #257 (Entropy-weighted decay)

## Executive Summary

This document presents findings from the Phase A feasibility study on aggregating entropy proofs with Bulletproof range proofs. The goal is to minimize transaction size overhead for Phase 2 committed tags while enabling entropy-weighted decay verification.

**Key Findings**:

1. **Shannon entropy is circuit-hostile** - requires logarithms that don't map well to arithmetic circuits
2. **Collision entropy (H₂) is a viable alternative** - computable as sum of squared probabilities, circuit-friendly
3. **Aggregation is feasible** - entropy constraints can share inner-product structure with range proofs
4. **Estimated savings: ~33%** - combined proof ~900 bytes vs ~1344 bytes separate (Option 1)
5. **Recommended approach**: Bulletproofs with auxiliary inner-product constraints

## Background

### The Problem

Phase 2 requires zero-knowledge proofs for:
1. **Tag conservation**: Output tags derive from input tags (existing Schnorr proofs)
2. **Fee correctness**: Fee paid matches cluster factor (existing OR-proof structure)
3. **Entropy delta** (new): Decay credit proportional to genuine entropy increase

Current transaction overhead (Phase 1 baseline):
- Ring signature (CLSAG, ring=11): ~350 bytes
- Range proofs: Not yet implemented, estimated ~700 bytes with Bulletproofs

Adding separate entropy proof: ~500-800 bytes additional

**Target**: Combined proof ≤ 1000 bytes total for range + entropy

### Current Crypto Infrastructure

From `cluster-tax/src/crypto/committed_tags.rs`:
- Pedersen commitments for tag masses: `C = mass * H_k + r * G`
- Per-cluster generators derived via hash-to-curve
- Schnorr proofs for tag conservation
- OR-proof structure for fee segment verification

This provides a foundation for extending with entropy proofs.

## Entropy Reformulation Analysis

### Shannon Entropy: Circuit-Hostile

Standard Shannon entropy:
```
H = -Σ (p_i × log₂(p_i))
```

**Problems for arithmetic circuits**:
1. **Logarithms**: Not expressible as polynomial constraints
2. **Lookup tables**: Require pre-computed tables, add significant overhead
3. **Polynomial approximations**: Taylor series for log₂ needs many terms for accuracy
4. **Floating point**: Entropy is real-valued, circuits work with field elements

**Estimated circuit cost**: 500+ constraints per tag entry using lookup tables

### Collision Entropy (H₂): Circuit-Friendly

Collision entropy (Rényi entropy of order 2):
```
H₂ = -log₂(Σ p_i²)
```

**Key insight**: We don't need to compute H₂ directly. For threshold comparisons:
```
H₂ ≥ threshold  ⟺  Σ p_i² ≤ 2^(-threshold)
```

The sum of squared probabilities is purely arithmetic:
```
collision_sum = Σ (w_i / total)² = Σ w_i² / total²
```

**Circuit cost**: O(n) multiplications where n = number of clusters

### Comparison: Shannon vs Collision Entropy

| Metric | Shannon H | Collision H₂ |
|--------|-----------|--------------|
| Formula | -Σ p log p | -log(Σ p²) |
| Range | [0, log₂(n)] | [0, log₂(n)] |
| Single source | 0 | 0 |
| Uniform (n sources) | log₂(n) | log₂(n) |
| Decay-invariant | Yes* | Yes* |
| Circuit-friendly | No | Yes |

*When computed over cluster weights only, excluding background

### Collision Entropy for Wash Trading Detection

**Key question**: Does collision entropy distinguish wash trading from genuine commerce?

**Analysis**:

1. **Wash trading (A→B→C→A)**:
   - Real input: 100% cluster A, H₂ = 0
   - Output: 100% cluster A, H₂ = 0
   - Δ H₂ = 0 → No decay credit ✓

2. **Genuine commerce (receive from diverse sources)**:
   - Before: 100% cluster A, H₂ = 0
   - After mixing with 50% B: H₂ = 1 bit
   - Δ H₂ = 1 → Decay credit granted ✓

3. **Split attack (same entropy preserved)**:
   - Parent: 60% A, 40% B, H₂ ≈ 0.97 bits
   - Children (10 outputs): Each 60% A, 40% B, H₂ ≈ 0.97 bits
   - No entropy gain per split ✓

**Conclusion**: Collision entropy preserves all required properties for decay credit.

### Min-Entropy (H_∞): Even Simpler

Min-entropy:
```
H_∞ = -log₂(max(p_i))
```

**For threshold comparisons**:
```
H_∞ ≥ threshold  ⟺  max(p_i) ≤ 2^(-threshold)
```

**Advantages**:
- Single comparison instead of sum
- Even simpler circuit

**Disadvantages**:
- Less granular than H₂
- A 51%/49% split has same H_∞ as 51%/24.5%/24.5%
- May be too coarse for decay credit weighting

**Recommendation**: Use collision entropy (H₂) for better granularity while maintaining circuit-friendliness.

## Bulletproof Aggregation Analysis

### Bulletproofs Background

Bulletproofs (Bünz et al., 2018) provide:
- **Range proofs**: Prove v ∈ [0, 2^n) without revealing v
- **Arithmetic circuit proofs**: General constraints (Bulletproofs paper §5)
- **Aggregation**: Multiple proofs share inner-product argument

**Core structure**:
```
Proof = (A, S, T₁, T₂, τ, μ, t̂, L, R)
Where:
  A, S, T₁, T₂: Curve points (commitments)
  τ, μ, t̂: Scalars (blinding, evaluation)
  L, R: log₂(n) curve point pairs (inner-product argument)
```

**Size scaling**:
- Single range proof: ~672 bytes (n=64 bits)
- Aggregated m proofs: 672 + 64(m-1) bytes (sub-linear!)

### Aggregation Opportunity

The inner-product argument proves:
```
⟨a, b⟩ = c  where a, b are vectors
```

For range proofs:
- a encodes the bit decomposition of value
- b encodes powers of 2 and challenges
- c is the claimed inner product

For entropy constraints:
- a could encode squared weights (w_i²)
- b could encode normalization factors (1/total²)
- c is the collision sum

**Key insight**: Both use the same inner-product structure. They can share:
- Generator points
- Challenge derivation
- Final inner-product argument

### Aggregation Design

**Option A: Sequential Aggregation**
```
proof = aggregate([range_proof, entropy_proof])
```
- Run range proof, then entropy proof, combine at inner-product stage
- Savings: ~35-40% (shared generators, combined inner-product)

**Option B: Interleaved Constraints**
```
extended_range_proof = range_proof.with_auxiliary(entropy_constraints)
```
- Add entropy constraints as auxiliary statements within range proof circuit
- Savings: ~40-45% (fully integrated)

**Option C: Batch Verification Only**
```
batch_verify([range_proof, entropy_proof])
```
- Separate proofs, but verify together efficiently
- Savings: ~20-25% (only verification speedup, same proof size)

**Recommendation**: Option B (interleaved constraints) for maximum size reduction.

## Alternative Approaches Evaluation

### Option 1: Bulletproofs with Auxiliary Constraints

**Description**: Extend Bulletproof range proof to include entropy constraint as additional inner-product argument.

**Implementation sketch**:
```rust
struct AugmentedRangeProof {
    // Standard range proof components
    range_A: CompressedRistretto,
    range_S: CompressedRistretto,
    // Entropy constraint components
    entropy_commitment: CompressedRistretto,
    // Shared inner-product argument
    L: Vec<CompressedRistretto>,
    R: Vec<CompressedRistretto>,
    // Combined responses
    a_range: Scalar,
    b_range: Scalar,
    a_entropy: Scalar,
    b_entropy: Scalar,
}
```

**Size estimate**: 850-950 bytes total

**Pros**:
- Single proof, well-understood security
- Maximum size reduction
- Single verification pass

**Cons**:
- Requires custom Bulletproof variant
- More complex implementation
- Needs careful security analysis

**Security considerations**:
- Soundness: Auxiliary constraints don't weaken range proof
- Zero-knowledge: Entropy value remains hidden
- Needs formal security proof or reduction

### Option 2: Groth16 for Entropy Only

**Description**: Use Bulletproofs for range (no trusted setup), Groth16 for entropy (small proof).

**Implementation**:
```rust
struct HybridProof {
    range_proof: Bulletproof,      // ~672 bytes
    entropy_proof: Groth16Proof,   // ~192 bytes
}
```

**Size estimate**: ~864 bytes total

**Pros**:
- Proven systems, minimal custom work
- Very small entropy proof
- Clear security properties

**Cons**:
- Two proof systems to maintain
- Groth16 requires trusted setup (ceremony)
- Trusted setup is per-circuit (changes if constraints change)

**Trusted setup options**:
1. **Powers of Tau**: Community ceremony (like Zcash)
2. **Universal setup**: PLONK/Marlin style (one-time, any circuit)
3. **MPC ceremony**: Distributed trust

### Option 3: PLONK/Halo2 Unified Proof

**Description**: Modern proof system with universal setup, express all constraints in single circuit.

**Implementation**:
```rust
struct UnifiedCircuit {
    // Range constraint: 0 ≤ v < 2^64
    range_check: RangeGadget,
    // Entropy constraint: Σ w_i² / total² ≤ threshold
    entropy_check: EntropyGadget,
    // Tag conservation (could also include)
    conservation_check: ConservationGadget,
}
```

**Size estimate**: 400-600 bytes (PLONK proofs are very compact)

**Pros**:
- Clean architecture, single circuit
- Universal setup (one ceremony for all circuits)
- Good tooling (halo2, arkworks)
- Smallest proof size

**Cons**:
- Larger proving time than Bulletproofs
- More complex implementation
- May be overkill if only entropy needed
- Requires migrating existing proofs

**Consideration**: If planning extensive ZK work beyond entropy, PLONK may be worth the investment.

### Option 4: Commit-and-Prove Separation

**Description**: Commit to entropy at output creation, prove relationship only at spend time.

**Flow**:
```
Output Creation:
  - Compute entropy of output tags
  - Create entropy_commitment = entropy * G + r * H
  - Include in output (32 bytes)

Spend Time:
  - Prove entropy_delta = output_entropy - input_entropy
  - Prove relationship to committed values
```

**Implementation**:
```rust
struct TaggedOutput {
    // Existing
    amount_commitment: CompressedRistretto,
    tag_commitments: Vec<CommittedTagMass>,
    // New
    entropy_commitment: CompressedRistretto,
}

struct EntropyDeltaProof {
    // Prove: committed_output_entropy - committed_input_entropy ≥ threshold
    // Uses homomorphic property of Pedersen commitments
    delta_commitment: CompressedRistretto,
    schnorr_proof: SchnorrProof,
}
```

**Size estimate**:
- Output overhead: +32 bytes per output
- Spend proof: ~128 bytes (Schnorr-style)
- Total per-spend: ~128 bytes (but outputs are larger)

**Pros**:
- Amortizes computation over UTXO lifetime
- Simple proofs at spend time
- Homomorphic subtraction for delta

**Cons**:
- Larger outputs (+32 bytes each)
- More complex state management
- Entropy is fixed at creation (can't recompute)
- Requires computing entropy during output creation

## Size Comparison Summary

| Approach | Range | Entropy | Total | Savings vs Separate |
|----------|-------|---------|-------|---------------------|
| Separate proofs | 672 | 672 | 1344 | 0% (baseline) |
| **Option 1: Integrated Bulletproofs** | - | - | **~900** | **~33%** |
| Option 2: Bulletproofs + Groth16 | 672 | 192 | 864 | ~36% |
| Option 3: PLONK unified | - | - | ~500 | ~63% |
| Option 4: Commit-and-prove | 672 | 128 | 800* | ~40% |

*Option 4 adds 32 bytes to every output, which may offset savings for multi-output transactions.

## Verification Cost Analysis

| Approach | Verification Time | Batch Efficiency |
|----------|-------------------|------------------|
| Separate proofs | ~8ms × 2 = 16ms | Good (separate batching) |
| Integrated Bulletproofs | ~10ms | Excellent (single verification) |
| Bulletproofs + Groth16 | ~8ms + ~2ms = 10ms | Good (pairing batch) |
| PLONK unified | ~5ms | Excellent |
| Commit-and-prove | ~8ms + ~1ms = 9ms | Good |

All approaches meet the 15ms verification target.

## Recommendation

**Primary recommendation**: **Option 1 (Integrated Bulletproofs with Auxiliary Constraints)**

**Rationale**:
1. **Best fit for existing infrastructure**: Already using Pedersen commitments and Schnorr-style proofs
2. **No trusted setup**: Important for decentralization
3. **Good size/complexity tradeoff**: ~33% savings without major architectural changes
4. **Single proof system**: Simpler to audit and maintain

**Implementation path**:
1. Implement collision entropy computation in existing `TagVector`
2. Design auxiliary constraint circuit for entropy threshold
3. Extend Bulletproof generation to include auxiliary constraints
4. Implement combined verification
5. Benchmark and optimize

**Fallback**: If Option 1 proves too complex, Option 4 (commit-and-prove) offers good savings with simpler proofs.

## Next Steps (Phase B)

1. **Prototype entropy circuit**:
   - Implement collision sum computation
   - Define constraint format for Bulletproofs

2. **Benchmarking**:
   - Measure actual proof sizes
   - Profile proving time
   - Profile verification time

3. **Security analysis**:
   - Formal reduction to Bulletproofs security
   - Analyze any additional assumptions

4. **Integration design**:
   - Transaction format changes
   - Backward compatibility
   - Migration path from Phase 1

## Appendix A: Collision Entropy Circuit

**Constraint system for proving H₂ ≥ threshold**:

```
Public inputs:
  - threshold_squared = 2^(-2×threshold)  // Pre-computed constant
  - total_commitment: Commitment to Σ w_i

Private inputs (witness):
  - weights: [w_1, w_2, ..., w_n]
  - blinding factors

Constraints:
  1. Σ w_i = total (sum constraint)
  2. For each i: sq_i = w_i × w_i (squaring)
  3. collision_sum = Σ sq_i
  4. collision_sum × threshold_squared ≤ total² (threshold check)

The final constraint uses the equivalence:
  H₂ ≥ threshold
  ⟺ -log₂(Σ p_i²) ≥ threshold
  ⟺ Σ p_i² ≤ 2^(-threshold)
  ⟺ Σ (w_i/total)² ≤ 2^(-threshold)
  ⟺ Σ w_i² ≤ total² × 2^(-threshold)
```

**Constraint count**: O(n) where n = number of clusters (typically 1-5)

## Appendix B: Alternative Entropy Measures

### Quadratic Rényi Entropy (H₂)

Used in this analysis. Equals Shannon entropy for uniform distributions.

### Tsallis Entropy

```
S_q = (1 - Σ p_i^q) / (q - 1)
```

For q=2: S₂ = 1 - Σ p_i², directly related to collision probability.

### Gini-Simpson Index

```
GS = 1 - Σ p_i²
```

The "collision probability complement" - also circuit-friendly.

Any of these could substitute for Shannon entropy with similar properties for wash trading detection.

## References

- Bünz et al. (2018). "Bulletproofs: Short Proofs for Confidential Transactions and More." https://eprint.iacr.org/2017/1066
- Bünz et al. (2020). "Bulletproofs+: Shorter Proofs for Privacy-Enhanced Distributed Ledger." https://eprint.iacr.org/2020/735
- dalek-cryptography/bulletproofs: https://github.com/dalek-cryptography/bulletproofs
- Gabizon et al. (2019). "PLONK: Permutations over Lagrange-bases for Oecumenical Noninteractive arguments of Knowledge." https://eprint.iacr.org/2019/953
- Issue #232: Phase 2 committed tags epic
- Issue #257: Entropy-weighted decay design
- `cluster-tax/src/crypto/committed_tags.rs`: Current committed tag infrastructure
