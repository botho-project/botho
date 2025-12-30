# Zero-Knowledge Virtual Machines: From Groth16 to Modern zkVMs

## Research Summary

This document summarizes research on Jens Groth's contributions to zero-knowledge proof systems and their evolution toward zkVM (zero-knowledge virtual machine) architectures.

---

## 1. Groth16: The Foundation (2016)

### Overview

Groth16 remains the gold standard for succinct zero-knowledge proofs, achieving optimal proof size in the generic group model.

**Key Properties:**
- **Proof size**: 3 group elements (2 in G₁, 1 in G₂)
- **Verification**: 3 pairings + linear exponentiations in statement size
- **Security**: Generic group model
- **Setup**: Per-circuit trusted setup required

### Technical Contribution

Groth's insight was to compile Linear Interactive Proofs (LIPs) more aggressively than prior work. Where Bitansky et al. proposed 2 group elements per field element (using knowledge-of-exponent assumptions), Groth used 1 group element per field element, proving security in the generic group model.

The paper also proves a **lower bound**: 1-element SNARGs cannot exist for pairing-based constructions. Whether 2-element SNARGs exist remains open.

### Limitations for zkVMs

1. **Per-circuit trusted setup**: Each circuit requires its own ceremony
2. **Not recursion-friendly**: Pairing verification is expensive in-circuit
3. **Optimizes for size, not prover speed**: FFTs dominate prover time

**Reference**: Groth, J. "On the Size of Pairing-based Non-interactive Arguments." EUROCRYPT 2016. [ePrint 2016/260](https://eprint.iacr.org/2016/260.pdf)

---

## 2. Nova: Folding Schemes for IVC (2022)

### The Paradigm Shift

Nova introduced **folding schemes** as an alternative to SNARK-based recursion. Instead of proving knowledge of a valid SNARK at each step, fold NP instances directly.

**Key Insight**: A folding scheme is strictly weaker than a SNARK—it only reduces satisfiability of two instances to one. But this weakness enables:

- **Constant-size verifier circuit**: ~10,000 R1CS constraints (2 group scalar multiplications)
- **No FFTs**: Prover only computes MSMs
- **No trusted setup**: Uses Pedersen commitments over cycles of curves

### Performance Comparison

| Approach | Verifier Circuit | Prover (per step) | Proof Size |
|----------|------------------|-------------------|------------|
| SNARK-based IVC | 3 pairings | O(C) FFT + MSM | O(1) |
| Halo | O(log C) scalar muls | O(C) FFT + EXP | O(log C) |
| Nova | 2 scalar muls | O(C) MSM only | O(log C)* |

*After compression with Spartan-based zkSNARK

### Relaxed R1CS

Standard R1CS: `A·z ⊙ B·z = C·z`

Cannot be folded directly (cross-terms appear). Nova introduces **relaxed R1CS**:

`A·z ⊙ B·z = u·(C·z) + E`

Where `u` is a scalar and `E` is an error vector. This absorbs cross-terms during folding.

**Reference**: Kothapalli, A., Setty, S., Tzialla, I. "Nova: Recursive Zero-Knowledge Arguments from Folding Schemes." CRYPTO 2022. [ePrint 2021/370](https://eprint.iacr.org/2021/370.pdf)

---

## 3. Nexus zkVM: Groth's Current Work (2024)

### Architecture

Nexus is a distributed zkVM targeting "a trillion CPU cycles per second" through:

1. **Nexus Virtual Machine (NVM)**: Minimal 32-bit ISA (40 instructions), von Neumann architecture
2. **Folding-based IVC**: Nova/HyperNova for SNARK-less proof accumulation
3. **Tree-structured parallelism**: Proofs accumulate in r-ary trees
4. **Final compression**: Spartan → Groth16 for Ethereum verification

### Execution Sequence

```
Program → Compile to NVM → Execute (trace) → Fold (parallel) → Compress (SNARK)
```

### The Memory Checking Problem

The dominant cost in NVM arithmetization is **memory checking**—proving RAM access consistency.

**Current approach** (Nexus 1.0):
- Merkle trees with Poseidon hashes
- ~30k constraints per CPU cycle (mostly memory)
- Target: 10x reduction with improved techniques

**Groth's ZKProof 2024 contribution**: "Memory Checking in Folding zkVMs" (Berlin, May 2024)

Likely techniques:
- **Logarithmic derivatives**: Batch memory consistency via polynomial identity
- **Offline memory checking**: Defer verification to end of execution
- **Permutation arguments**: Sort-based approaches (cf. Plookup)

### zkVM Co-processors

Non-uniform IVC allows "ASIC-like" extensions without per-step overhead:

```
Cost of SHA-256:
- VM emulation: ~64,000 CPU cycles
- Direct circuit: ~30,000 constraints
- Ratio: ~1000x overhead for abstraction
```

Co-processors pay cost only when executed, enabling hybrid CPU/ASIC proving.

### Nexus 3.0: The STARK Pivot

Notably, Nexus 3.0 moved from folding schemes to the **Stwo STARK prover**, claiming ~1000x speedup. This suggests:

1. Memory checking may be fundamentally expensive in R1CS/CCS
2. AIR (Algebraic Intermediate Representation) may better suit execution traces
3. Engineering maturity of STARK provers (StarkWare investment)

**References**:
- Marin, D. et al. "Nexus 1.0: Enabling Verifiable Computation." January 2024. [Whitepaper](https://nexus-xyz.github.io/assets/nexus_whitepaper.pdf)
- [Nexus zkVM 3.0 Specification](https://specification.nexus.xyz/)
- [ZKProof 6th Workshop Recordings](https://docs.zkproof.org/presentations)

---

## 4. Key Concepts

### Incrementally Verifiable Computation (IVC)

Introduced by Valiant (2008). Prover produces proofs of correct execution that can be updated incrementally:

```
π₀ → F(x₀) → π₁ → F(x₁) → π₂ → ... → πₙ
```

Each πᵢ proves correct execution of all steps up to i.

### Proof-Carrying Data (PCD)

Generalization of IVC to DAGs (distributed setting). Multiple provers can work in parallel, aggregating into a single proof.

### Folding vs. Accumulation

- **Folding**: Combine two instances into one (Nova)
- **Accumulation**: Defer verification steps (Halo)
- **Multi-folding**: Fold µ instances into ν (HyperNova)

### Customizable Constraint Systems (CCS)

Generalization unifying R1CS, Plonkish, and AIR:

```
∑ cᵢ · ⊙ⱼ∈Sᵢ (Mⱼ · z) = 0
```

Enables expressing different constraint systems in one framework.

---

## 5. Evolution of Optimization Targets

| Era | System | Primary Goal | Key Metric |
|-----|--------|--------------|------------|
| 2016 | Groth16 | Proof size | 3 elements |
| 2019 | Halo | No trusted setup | O(log n) recursion |
| 2022 | Nova | Recursion overhead | 10k constraints |
| 2024 | Nexus | Prover throughput | Cycles/second |

---

## 6. Open Research Questions

1. **Optimal memory checking in folding**: Can folding-native RAM checking match STARK efficiency?

2. **Folding + STARK hybrids**: Use STARKs for execution, fold STARK verifiers?

3. **2-element SNARGs**: Can Groth16's lower bound be tightened?

4. **Post-quantum folding**: Current schemes rely on discrete log; lattice alternatives?

5. **Hardware acceleration**: MSM-optimized ASICs for folding provers?

---

## 7. Implications for Blockchain Design

### Proof Size vs. Verification Time

- **Groth16**: Smallest proofs (~128 bytes), fast verification, but requires trusted setup
- **STARKs**: Large proofs (~100KB), slower verification, no trusted setup
- **Folding + compression**: Medium proofs (~8KB), configurable tradeoffs

### On-chain Verification

For L1 verification (e.g., Ethereum):
- Groth16 verifier: ~200k gas
- STARK verifier: ~2M+ gas
- Nexus approach: Fold off-chain, Groth16 final proof on-chain

### Privacy Considerations

All systems support zero-knowledge variants. Key difference:
- **Groth16**: ZK "for free" (simulation-based)
- **STARKs**: Require explicit randomization
- **Folding**: ZK in final compression step

---

## References

### Primary Sources

1. Groth, J. "On the Size of Pairing-based Non-interactive Arguments." EUROCRYPT 2016. https://eprint.iacr.org/2016/260.pdf

2. Kothapalli, A., Setty, S., Tzialla, I. "Nova: Recursive Zero-Knowledge Arguments from Folding Schemes." CRYPTO 2022. https://eprint.iacr.org/2021/370.pdf

3. Marin, D. et al. "Nexus 1.0: Enabling Verifiable Computation." Nexus Labs, January 2024. https://nexus-xyz.github.io/assets/nexus_whitepaper.pdf

### Secondary Sources

4. Kothapalli, A., Setty, S. "SuperNova: Proving Universal Machine Executions without Universal Circuits." https://eprint.iacr.org/2022/1758.pdf

5. Kothapalli, A., Setty, S. "HyperNova: Recursive Arguments for Customizable Constraint Systems." https://eprint.iacr.org/2023/573.pdf

6. Kothapalli, A., Setty, S. "CycleFold: Folding-scheme-based recursive arguments over a cycle of elliptic curves." https://eprint.iacr.org/2023/1192.pdf

7. Setty, S. "Spartan: Efficient and general-purpose zkSNARKs without trusted setup." CRYPTO 2020. https://eprint.iacr.org/2019/550.pdf

### Podcast & Presentations

8. Zero Knowledge Podcast, Episode 335: "Groth16, IVC and Formal Verification with Nexus." August 2024. https://zeroknowledge.fm/335-2/

9. ZKProof 6th Workshop, Berlin, May 2024. Groth, J. "Memory Checking in Folding zkVMs." https://docs.zkproof.org/presentations
