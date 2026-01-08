# Security Analysis: Integrated Entropy Proofs with Bulletproofs

## Overview

This document provides a formal security analysis of the integrated Bulletproof + entropy constraint approach for Botho's Phase 2 committed cluster tags. The analysis verifies that the combined proof system maintains soundness and zero-knowledge properties while enabling privacy-preserving entropy verification.

**Status**: Phase B Security Analysis
**Related Issues**: #260 (Research), #262 (Prototype), #264 (This Analysis)
**References**:
- Bünz et al. (2018) Bulletproofs: Short Proofs for Confidential Transactions and More
- `docs/design/entropy-weighted-decay.md`

## 1. System Architecture

### 1.1 Proof Components

The integrated proof system combines three cryptographic components:

1. **Range Proofs (Bulletproofs)**: Prove transaction amounts are in [0, 2^64)
2. **Tag Conservation Proofs (Schnorr)**: Prove cluster tag masses are conserved with decay
3. **Entropy Constraint Proofs**: Prove entropy delta meets threshold for decay credit

```
┌─────────────────────────────────────────────────────────────┐
│                    Transaction Proof                         │
├─────────────────────────────────────────────────────────────┤
│  ┌──────────────────┐  ┌──────────────────┐                 │
│  │   Bulletproofs   │  │  Tag Conservation │                 │
│  │  (Range Proofs)  │  │  (Schnorr Proofs) │                 │
│  └────────┬─────────┘  └────────┬─────────┘                 │
│           │                     │                            │
│           └──────────┬──────────┘                            │
│                      ▼                                       │
│           ┌──────────────────────┐                          │
│           │   Entropy Threshold  │                          │
│           │    (Range Proof)     │                          │
│           └──────────────────────┘                          │
└─────────────────────────────────────────────────────────────┘
```

### 1.2 Commitment Structure

The system uses Pedersen commitments throughout:

```
Value Commitment:     C_v = v·H + r_v·G
Tag Mass Commitment:  C_k = m_k·H_k + r_k·G  (for cluster k)
Total Mass:           C_T = Σm_k·H_T + r_T·G
Entropy Commitment:   C_E = H₂·H_E + r_E·G
```

Where:
- `G`: Standard blinding generator (Ristretto basepoint)
- `H`, `H_k`, `H_T`, `H_E`: Domain-separated generators derived via hash-to-curve
- `v`: Transaction value
- `m_k`: Tag mass for cluster k
- `H₂`: Collision entropy (Rényi-2) of tag distribution
- `r_*`: Random blinding factors

## 2. Security Properties

### 2.1 Soundness

**Definition**: A malicious prover cannot convince a verifier of a false statement except with negligible probability.

#### 2.1.1 Bulletproof Soundness (Range Proofs)

**Claim**: The Bulletproof range proofs maintain computational soundness under the discrete logarithm assumption.

**Analysis**:
- Bulletproofs achieve soundness with 128-bit security via the inner product argument
- Soundness holds in the random oracle model (Fiat-Shamir heuristic)
- Error probability: 2^{-128}

**Integration Impact**:
Adding entropy constraints does NOT weaken range proof soundness because:

1. **Separate Generators**: Entropy proofs use `H_E` which has unknown discrete log relation to value generators `H`, `H_k`
2. **Independent Verification**: Range proof and entropy proof verifications are independent
3. **No Algebraic Interaction**: The constraints don't share witnesses (values vs. entropy)

**Formal Reduction**:
```
If adversary A can forge an integrated proof, then either:
1. A can forge a Bulletproof range proof (breaks DLOG)
2. A can forge an entropy threshold proof (breaks DLOG)
3. A can forge a Schnorr proof (breaks DLOG)

Since all three reduce to DLOG, soundness of the combined system
reduces to DLOG assumption.
```

#### 2.1.2 Tag Conservation Soundness

**Claim**: A prover cannot create tag mass without legitimate input.

**Analysis** (from `committed_tags.rs`):
- Conservation uses Schnorr proofs on blinding factor differences
- For each cluster k: `C_out_k - (1-d)·C_in_k = r_diff·G`
- Prover must know `r_diff` to create valid proof

**Attack Vector Analysis**:

| Attack | Description | Mitigated By |
|--------|-------------|--------------|
| Mass Inflation | Create more output mass than decayed input | DLOG hardness on generators |
| Cross-Cluster Transfer | Move mass between clusters without detection | Independent generators H_k |
| Decay Bypass | Avoid decay application | Conservation proof structure |

#### 2.1.3 Entropy Threshold Soundness

**Claim**: A prover cannot prove `H₂ ≥ threshold` when actual entropy is below threshold.

**Analysis**:
The entropy threshold is proven via a range proof on the committed entropy value:

```rust
// Prove: entropy_delta ≥ min_delta_threshold
// Using: C_E - min_threshold·H_E = excess·H_E + r·G
// Where: excess = entropy_delta - min_threshold ≥ 0

RangeProof::prove(excess, blinding, H_E, G)
```

**Security Argument**:
1. Verifier computes `C_excess = C_E - min_threshold·H_E`
2. Prover must show `C_excess` commits to non-negative value
3. If entropy_delta < min_threshold, then excess < 0
4. Bulletproofs cannot prove negative values (soundness)
5. Therefore, a false entropy claim fails with probability 2^{-128}

### 2.2 Zero-Knowledge

**Definition**: The proof reveals nothing beyond the truth of the statement.

#### 2.2.1 Amount Privacy (Existing Bulletproofs)

**Claim**: Transaction amounts remain hidden.

**Analysis**:
- Bulletproofs are perfect zero-knowledge (statistical ZK)
- Amounts are hidden in Pedersen commitments
- Range proofs reveal only that values are in [0, 2^64)

#### 2.2.2 Tag Distribution Privacy

**Claim**: Entropy constraints do not leak tag distribution information.

**Analysis**:

The entropy is computed from tag weights: `H₂ = -log₂(Σ p_k²)` where `p_k = w_k / Σw_j`

**Privacy Concerns**:
1. Does the entropy commitment leak tag distribution?
2. Can observers distinguish high-entropy from low-entropy transactions?
3. Is simulation possible?

**Mitigation Analysis**:

| Concern | Analysis | Conclusion |
|---------|----------|------------|
| Entropy Value Leakage | Entropy committed with fresh blinding | Hidden |
| Distribution Inference | Multiple distributions map to same entropy | Ambiguous |
| Transaction Distinguishing | All valid proofs are indistinguishable | Private |
| Side-Channel Timing | Proof generation is constant-time | Protected |

**Formal ZK Argument**:

```
Simulator construction:
1. Receive statement (threshold, public commitments)
2. Generate random C_E' (fake entropy commitment)
3. Run Bulletproof simulator for range proof
4. Run Schnorr simulator for conservation proofs
5. Output simulated proof transcript

Indistinguishability:
- Pedersen commitments are perfectly hiding
- Bulletproof transcripts are simulatable
- Schnorr proofs are honest-verifier zero-knowledge
- Joint distribution is computationally indistinguishable
```

#### 2.2.3 Information Leakage Assessment

| Information | Visibility | Leakage |
|------------|------------|---------|
| Transaction value | Hidden (commitment) | 0 bits |
| Individual tag weights | Hidden (commitment) | 0 bits |
| Entropy value | Hidden (commitment) | 0 bits |
| Entropy ≥ threshold | Public (binary) | 1 bit |
| Decay was applied | Public (implicit) | 1 bit |
| Number of clusters | Public (proof structure) | ~4 bits |

**Total Information Leakage**: ~6 bits per transaction (cluster count + threshold satisfaction)

This is acceptable as:
- Cluster count is necessary for verification efficiency
- Threshold satisfaction is the intended disclosure

### 2.3 Binding

**Definition**: A prover cannot open a commitment to different values.

#### 2.3.1 Pedersen Commitment Binding

**Claim**: Commitments are computationally binding under DLOG.

**Analysis**:
Pedersen commitments `C = v·H + r·G` are:
- **Perfectly hiding**: For any C, any v has some r that produces C
- **Computationally binding**: Opening to two values reveals DLOG(H, G)

If prover could open `C` to both `(v, r)` and `(v', r')`:
```
v·H + r·G = v'·H + r'·G
(v - v')·H = (r' - r)·G
log_G(H) = (r' - r) / (v - v')
```
This solves DLOG, contradiction under DLOG assumption.

#### 2.3.2 Entropy Commitment Binding

**Claim**: Entropy commitments cannot be opened to multiple entropy values.

**Analysis**:
- Uses same Pedersen structure: `C_E = H₂·H_E + r_E·G`
- Binding follows from DLOG hardness between G and H_E
- H_E derived via hash-to-curve (unknown discrete log)

**Attack Consideration**: Could an attacker create a proof that works for multiple entropy values?

**No**: The range proof binds the entropy commitment to a specific value (or range). Once the commitment is fixed in the transaction, the prover cannot change what entropy value they're proving about.

## 3. Attack Analysis

### 3.1 Algebraic Attacks on Combined Constraints

#### 3.1.1 Generator Relation Attacks

**Attack Vector**: Exploit unknown relationships between generators H, H_k, H_E.

**Analysis**:
All generators are derived via hash-to-curve with unique domain separators:
```rust
// From committed_tags.rs
const CLUSTER_GENERATOR_DOMAIN_TAG: &[u8] = b"mc_cluster_tag_generator";
const TOTAL_MASS_GENERATOR_DOMAIN_TAG: &[u8] = b"mc_cluster_total_mass_generator";
```

The discrete log relationships are:
- Unknown (hash-to-curve)
- Independent (different domain separators)
- Collision-resistant (SHA-512)

**Mitigation**: Random Oracle Model assumption on hash function.

**Risk Assessment**: LOW - Standard assumption in deployed cryptography.

#### 3.1.2 Commitment Malleability

**Attack Vector**: Modify commitments while maintaining valid proofs.

**Analysis**:
Commitments are bound to:
1. Transaction hash (included in Fiat-Shamir challenge)
2. Ring signature linking (key images)
3. Extended message digest (MLSAG signing)

**Mitigation**: Transaction-wide binding via MLSAG signatures.

**Risk Assessment**: LOW - Standard transaction binding.

#### 3.1.3 Proof Aggregation Attacks

**Attack Vector**: Combine proofs from different transactions to create invalid combined proof.

**Analysis**:
Each proof includes:
- Unique challenge derived from transaction-specific data
- Blinding factors are fresh per proof
- Key images prevent replay

**Mitigation**: Fiat-Shamir binding to transaction context.

**Risk Assessment**: LOW - Standard technique.

### 3.2 Grinding Attacks on Challenge Generation

#### 3.2.1 Challenge Precomputation

**Attack Vector**: Precompute favorable challenges to bias proof verification.

**Analysis**:
Fiat-Shamir challenges include:
```rust
fn compute_total_challenge(&self, proofs: &[SegmentFeeProof]) -> Scalar {
    let mut hasher = Sha512::new();
    hasher.update(b"mc_segment_or_challenge");
    hasher.update(self.fee_paid.to_le_bytes());
    hasher.update(self.base_fee.to_le_bytes());
    for proof in proofs {
        hasher.update(proof.range_proof.lower_commitment.as_bytes());
        // ... all proof components
    }
    Scalar::from_hash(hasher)
}
```

**Grinding Cost Analysis**:
- Challenge space: 252 bits (Ristretto scalar field)
- To find favorable challenge: 2^128 hash operations (birthday bound)
- Cost: Computationally infeasible

**Mitigation**: Full proof binding in challenge.

**Risk Assessment**: NEGLIGIBLE - Infeasible computational cost.

#### 3.2.2 Entropy Value Grinding

**Attack Vector**: Grind input selection to achieve favorable entropy.

**Analysis**:
An attacker might try to select inputs to maximize entropy delta:

| Strategy | Cost | Effectiveness |
|----------|------|---------------|
| Random Input Selection | O(n) | Limited by UTXO set |
| Optimal Input Selection | O(n²) | Bounded by entropy math |
| Fake Input Creation | Economic cost | Requires real funds |

**Mitigation**:
- Entropy is derived from actual cluster tags
- Creating favorable entropy requires genuine diverse holdings
- This is the intended behavior (rewarding real commerce)

**Risk Assessment**: LOW - Grinding produces intended behavior.

### 3.3 Timing and Side-Channel Attacks

#### 3.3.1 Proof Generation Timing

**Attack Vector**: Infer entropy value from proof generation time.

**Analysis**:
Proof generation operations:
```rust
// Schnorr proof generation
let k = Scalar::random(rng);        // Constant time
let r = k * g;                       // Constant time (Edwards arithmetic)
let c = Self::compute_challenge(...); // Hash (constant time)
let s = k + c * x;                   // Constant time scalar ops
```

**Mitigation**:
- curve25519-dalek provides constant-time operations
- Hash operations are data-independent timing
- No branching on secret values

**Risk Assessment**: LOW - Using constant-time implementations.

#### 3.3.2 Memory Access Patterns

**Attack Vector**: Cache-timing attacks on memory access.

**Analysis**:
- Ristretto operations are designed cache-timing resistant
- No table lookups based on secret data
- Scalar multiplication uses constant-time Montgomery ladder

**Mitigation**: Using audited constant-time library (curve25519-dalek).

**Risk Assessment**: LOW - Library-level protection.

#### 3.3.3 Network Timing

**Attack Vector**: Infer entropy from transaction submission timing.

**Analysis**:
- Proof verification time is independent of entropy value
- Network latency dominates any proof timing differences
- Verification is batch-able (amortizes timing)

**Mitigation**: Network noise provides natural protection.

**Risk Assessment**: NEGLIGIBLE - Dominated by network variance.

### 3.4 Economic Attacks

#### 3.4.1 Entropy Purchasing

**Attack Vector**: Buy entropy through strategic transactions.

**Analysis** (from entropy-weighted-decay.md):
This is explicitly NOT an attack - it's the intended behavior:

> "Entropy purchasing" requires:
> 1. Finding willing counterparties
> 2. Paying market rate + fees
> 3. Waiting for age requirement
>
> This creates genuine economic activity.

**Risk Assessment**: NOT APPLICABLE - Intended system behavior.

#### 3.4.2 Entropy Mining via Dust

**Attack Vector**: Receive many dust payments to increase entropy.

**Analysis**:
```rust
// Entropy is weighted by value, not count
fn cluster_entropy_weighted(&self) -> f64 {
    // Weight each source by its contribution
    // Dust sources contribute minimally
}
```

Receiving 1000 dust payments provides less entropy benefit than one meaningful transaction.

**Mitigation**: Value-weighted entropy calculation.

**Risk Assessment**: LOW - Economic disincentive.

## 4. Cryptographic Assumptions

### 4.1 Primary Assumptions

| Assumption | Description | Security Level |
|------------|-------------|----------------|
| **DLOG** | Discrete Logarithm Problem is hard on Ristretto255 | 128-bit |
| **ROM** | Random Oracle Model for Fiat-Shamir | Standard |
| **CDH** | Computational Diffie-Hellman (implied by DLOG) | 128-bit |

### 4.2 DLOG Hardness (Primary)

**Assumption**: Given G and H = x·G, finding x is computationally infeasible.

**Instantiation**:
- Group: Ristretto255 (Curve25519 quotient group)
- Order: ~2^252
- Best known attack: Pollard's rho O(√p) ≈ 2^126 operations

**Usage in System**:
1. Pedersen commitment hiding/binding
2. Schnorr proof soundness
3. Generator independence

**Confidence**: HIGH - Well-studied assumption, decades of cryptanalysis.

### 4.3 Random Oracle Model

**Assumption**: Hash functions (SHA-512, SHAKE256) behave as random oracles.

**Instantiation**:
- Hash-to-curve: SHA-512 → Ristretto255
- Fiat-Shamir: SHA-512 → Scalar

**Usage in System**:
1. Challenge generation (Fiat-Shamir)
2. Generator derivation (hash-to-curve)
3. Domain separation

**Confidence**: MEDIUM-HIGH - Standard assumption, no known practical attacks on SHA-512.

### 4.4 Additional Assumptions from Entropy Constraints

The entropy constraint system does NOT introduce new cryptographic assumptions beyond those already required for Bulletproofs:

| Component | Assumption | Already in Bulletproofs? |
|-----------|------------|--------------------------|
| Entropy commitment | DLOG | Yes |
| Threshold range proof | DLOG + ROM | Yes |
| Conservation proofs | DLOG | Yes (Schnorr) |

**Conclusion**: The integrated system relies on the same assumption set as standard Bulletproofs.

### 4.5 Assumption Hierarchy

```
DLOG (Discrete Logarithm)
    │
    ├── Pedersen Binding
    │       └── Commitment Security
    │
    ├── Schnorr Soundness
    │       └── Conservation Proofs
    │
    └── Inner Product Soundness
            └── Bulletproof Range Proofs

ROM (Random Oracle)
    │
    ├── Fiat-Shamir Security
    │       └── Non-interactive Proofs
    │
    └── Hash-to-Curve Security
            └── Generator Independence
```

## 5. Security Comparison with Alternatives

### 5.1 Comparison Table

| Property | Option 1: Integrated Bulletproofs | Option 2: Groth16 zkSNARK | Option 4: Commit-Prove |
|----------|-----------------------------------|---------------------------|------------------------|
| **Soundness** | Computational (DLOG) | Computational (Pairing) | Computational (DLOG) |
| **ZK Type** | Perfect/Statistical | Perfect | Perfect |
| **Setup Trust** | None (transparent) | MPC ceremony required | None (transparent) |
| **Primary Assumptions** | DLOG + ROM | q-PKE + DLOG + ROM | DLOG + ROM |
| **Quantum Resistance** | No (DLOG broken) | No (Pairing broken) | No (DLOG broken) |
| **Proof Size** | O(log n) | O(1) | O(n) |
| **Verification Time** | O(n) | O(1) | O(n) |
| **Prover Time** | O(n log n) | O(n²) | O(n) |
| **Aggregation** | Yes (native) | Yes (recursive) | Limited |
| **Maturity** | High (5+ years) | High (7+ years) | Medium |
| **Implementation Risk** | Low | Medium (ceremony) | Low |

### 5.2 Option 1: Integrated Bulletproofs (Recommended)

**Advantages**:
- No trusted setup required
- Natural integration with existing range proofs
- Proof aggregation reduces verification cost
- Same security assumptions as existing system
- Mature, audited implementations available

**Disadvantages**:
- O(n) verification time (vs O(1) for SNARKs)
- Larger proofs than SNARKs for complex circuits
- Prover time increases with constraint complexity

**Security Assessment**: STRONG

### 5.3 Option 2: Groth16 zkSNARK

**Advantages**:
- Constant proof size (288 bytes)
- Constant verification time
- Can prove arbitrary computations

**Disadvantages**:
- Requires trusted setup (MPC ceremony)
- Additional assumption (q-PKE, pairing-based)
- Setup must be repeated for circuit changes
- Higher implementation complexity

**Security Assessment**: STRONG (with proper ceremony)

### 5.4 Option 4: Commit-and-Prove

**Advantages**:
- Simplest implementation
- Same assumptions as Bulletproofs
- No new cryptographic machinery

**Disadvantages**:
- Linear proof size
- No aggregation benefits
- Higher verification cost for batches

**Security Assessment**: STRONG (but less efficient)

### 5.5 Recommendation

**Option 1 (Integrated Bulletproofs)** is recommended because:

1. **No new trust assumptions**: No MPC ceremony required
2. **Consistent security model**: Same DLOG + ROM as existing system
3. **Implementation simplicity**: Extends existing Bulletproof infrastructure
4. **Aggregation benefits**: Multiple proofs can be batched
5. **Proven track record**: Bulletproofs deployed in production (Monero, Mimblewimble)

## 6. Security Reduction

### 6.1 Formal Reduction Statement

**Theorem**: The security of the integrated entropy proof system reduces to the hardness of the Discrete Logarithm Problem on Ristretto255 in the Random Oracle Model.

### 6.2 Reduction Proof (Sketch)

**Setup**: Assume adversary A can break the integrated proof system with non-negligible advantage ε.

**Reduction B**: We construct B that uses A to solve DLOG.

**Case 1**: A breaks range proof soundness (forges value)
- B embeds DLOG challenge in value generator H
- A's forgery reveals discrete log
- Contradiction: DLOG is hard

**Case 2**: A breaks tag conservation (inflates mass)
- B embeds DLOG challenge in cluster generator H_k
- A's forgery reveals discrete log
- Contradiction: DLOG is hard

**Case 3**: A breaks entropy threshold (false entropy claim)
- B embeds DLOG challenge in entropy generator H_E
- A's false proof reveals discrete log
- Contradiction: DLOG is hard

**Case 4**: A breaks zero-knowledge (extracts witness)
- Simulator S can produce indistinguishable transcripts
- A distinguishing real from simulated breaks ROM
- Contradiction: ROM assumption

**Conclusion**: Security reduces to DLOG + ROM with tightness factor O(q_H) where q_H is hash queries.

### 6.3 Security Level

```
Security = min(DLOG security, ROM security)
         = min(126 bits, 256 bits)
         = 126 bits

With safety margin: Target 128-bit security achieved.
```

## 7. Implementation Security Considerations

### 7.1 Constant-Time Requirements

All operations on secret data must be constant-time:

```rust
// GOOD: Constant-time scalar operations
let blinding = Scalar::random(rng);
let commitment = value * H + blinding * G;

// BAD: Variable-time branching on secrets
if secret_value > threshold {  // TIMING LEAK
    // ...
}
```

### 7.2 Randomness Requirements

Critical for:
- Blinding factor generation
- Schnorr proof nonces
- Challenge contributions

**Requirement**: Use `CryptoRng` implementations only.

### 7.3 Error Handling

Proofs must fail securely:
```rust
// GOOD: Return None/Err without leaking why
fn prove(...) -> Option<Proof> {
    if !conservation_holds() {
        return None;  // No information about which check failed
    }
}

// BAD: Detailed error messages
fn prove(...) -> Result<Proof, DetailedError> {
    return Err(DetailedError::EntropyTooLow(actual_value));  // LEAK
}
```

### 7.4 Serialization Security

- Use canonical serialization (deterministic)
- Validate all deserialized points are on curve
- Check scalar values are in valid range

## 8. Conclusion

### 8.1 Summary of Findings

| Property | Status | Confidence |
|----------|--------|------------|
| Soundness | SECURE | HIGH |
| Zero-Knowledge | SECURE | HIGH |
| Binding | SECURE | HIGH |
| No New Assumptions | VERIFIED | HIGH |
| Attack Resistance | ADEQUATE | HIGH |

### 8.2 Recommendations

1. **Proceed with Integration**: The security analysis supports Option 1 (Integrated Bulletproofs)
2. **Use Existing Libraries**: Leverage audited curve25519-dalek and bulletproofs-og
3. **Maintain Constant-Time**: Use constant-time operations throughout
4. **Regular Audits**: Schedule security audits before production deployment

### 8.3 Open Items for Future Work

1. **Formal Verification**: Consider machine-checked proofs (Coq/Lean)
2. **Post-Quantum**: Research lattice-based alternatives for future migration
3. **Batch Verification**: Optimize multi-proof verification
4. **Audit Planning**: Engage external security auditors

## References

1. Bünz, B., Bootle, J., Boneh, D., Poelstra, A., Wuille, P., & Maxwell, G. (2018). Bulletproofs: Short proofs for confidential transactions and more. IEEE S&P 2018.

2. Bernstein, D. J., Duif, N., Lange, T., Schwabe, P., & Yang, B. Y. (2012). High-speed high-security signatures. Journal of Cryptographic Engineering.

3. Hamburg, M. (2015). Decaf: Eliminating cofactors through point compression. CRYPTO 2015.

4. de Valence, H. (2019). curve25519-dalek documentation. dalek-cryptography.

5. Groth, J. (2016). On the size of pairing-based non-interactive arguments. EUROCRYPT 2016.

## Changelog

- 2026-01-08: Initial security analysis (Issue #264)
