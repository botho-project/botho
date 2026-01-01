# Phase 2: Committed Tags Performance Characteristics

This document describes the performance characteristics of the Phase 2 committed cluster tag system, including proof generation/verification timing and proof size measurements.

## Overview

Phase 2 replaces public tag weights with Pedersen commitments, providing full privacy for cluster attribution while maintaining verifiable tag conservation through zero-knowledge proofs.

## Proof Size Analysis

### Component Sizes

| Component | Size (bytes) | Description |
|-----------|-------------|-------------|
| SchnorrProof | 64 | 32 (commitment R) + 32 (response s) |
| CompressedRistretto | 32 | Single elliptic curve point |
| Scalar | 32 | Field element |
| ClusterId | 8 | u64 identifier |

### Per-Structure Sizes

| Structure | Formula | Notes |
|-----------|---------|-------|
| CommittedTagMass | 40 bytes | 8 (cluster_id) + 32 (commitment) |
| CommittedTagVector | 36 + 40n bytes | 4 (count) + 40n (entries) + 32 (total) |
| ClusterConservationProof | 72 bytes | 8 (cluster_id) + 64 (schnorr) |
| TagConservationProof | 68 + 72n bytes | 4 (count) + 72n (cluster proofs) + 64 (total) |

### Total Proof Overhead by Cluster Count

For a single-input, single-output transaction:

| Clusters | Conservation Proof | Input Vector | Output Vector | **Total** |
|----------|-------------------|--------------|---------------|-----------|
| 1 | 140 bytes | 76 bytes | 76 bytes | **292 bytes** |
| 2 | 212 bytes | 116 bytes | 116 bytes | **444 bytes** |
| 3 | 284 bytes | 156 bytes | 156 bytes | **596 bytes** |
| 5 | 428 bytes | 236 bytes | 236 bytes | **900 bytes** |
| 8 | 644 bytes | 356 bytes | 356 bytes | **1,356 bytes** |

### Target Compliance

**Target**: Proof overhead < 1KB for typical transactions (1-3 clusters)

| Scenario | Total Size | Status |
|----------|-----------|--------|
| 1 cluster | 292 bytes | ✅ Well under target |
| 2 clusters | 444 bytes | ✅ Well under target |
| 3 clusters | 596 bytes | ✅ Under target |
| 5 clusters | 900 bytes | ✅ Under target |
| 8 clusters | 1,356 bytes | ⚠️ Exceeds 1KB |

The implementation meets the < 1KB target for typical 1-3 cluster transactions, with comfortable margin for future optimizations.

## Performance Benchmarks

### Running Benchmarks

```bash
cd cluster-tax
cargo bench
```

### Benchmark Categories

#### 1. Commitment Creation

- **`commitment_create`**: Time to create a single Pedersen commitment
- **`cluster_generator`**: Time to derive a cluster's generator point via hash-to-curve

#### 2. Vector Operations

- **`vector_creation/{n}_clusters`**: Time to create CommittedTagVector from secrets for n clusters
- Includes commitment creation, sorting, and total commitment computation

#### 3. Schnorr Proofs

- **`schnorr_prove`**: Time to generate a Schnorr proof of discrete log knowledge
- **`schnorr_verify`**: Time to verify a Schnorr proof

#### 4. Conservation Proofs

- **`conservation_prove/{n}_clusters`**: Time to generate TagConservationProof for n clusters
- **`conservation_verify/{n}_clusters`**: Time to verify TagConservationProof for n clusters

#### 5. Multi-Input Transactions

- **`multi_input_conservation/{n}_clusters`**: Conservation proof for 2-input transactions

#### 6. Decay Application

- **`decay_application/{n}_clusters`**: Time to apply decay to tag secrets

### Expected Performance Characteristics

Based on cryptographic operations:

| Operation | Dominant Cost | Scaling |
|-----------|--------------|---------|
| Commitment creation | 1 scalar multiplication | O(1) |
| Generator derivation | Hash-to-curve | O(1) |
| Schnorr prove | 1 random scalar + 2 scalar mults | O(1) |
| Schnorr verify | 2 scalar multiplications | O(1) |
| Conservation prove | n cluster proofs + 1 total proof | O(n) |
| Conservation verify | n cluster verifications + 1 total | O(n) |

## Optimization Opportunities

### 1. Generator Caching

**Current**: `cluster_generator()` computes hash-to-curve on every call.

**Optimization**: Cache generators for frequently-used cluster IDs using lazy_static or OnceCell.

```rust
use std::sync::OnceLock;
use std::collections::HashMap;

static GENERATOR_CACHE: OnceLock<HashMap<ClusterId, RistrettoPoint>> = OnceLock::new();
```

**Expected Impact**: Eliminates hash-to-curve for repeated cluster access.

### 2. Batch Verification

**Current**: Each Schnorr proof verified independently.

**Optimization**: Use multi-scalar multiplication for batch verification:

```rust
// Instead of: for proof in proofs { proof.verify() }
// Use: batch_verify(proofs)  // Single multi-scalar mult
```

**Expected Impact**: ~2x speedup for multi-cluster verification.

### 3. Pre-grouped Commitments

**Current**: `sum_cluster_commitments()` iterates O(n*m) for n vectors and m clusters.

**Optimization**: Pre-group entries by cluster_id during vector creation.

**Expected Impact**: Reduces verification iteration from O(n*m) to O(n+m).

### 4. Parallel Scalar Multiplication

**Current**: Sequential scalar multiplications.

**Optimization**: Use rayon for parallel computation when generating multiple proofs.

**Expected Impact**: Near-linear speedup with core count for large cluster counts.

## Comparison to Phase 1

| Aspect | Phase 1 (Public) | Phase 2 (Committed) |
|--------|-----------------|---------------------|
| Privacy | Ring signature only | Full tag privacy |
| Proof size | 0 bytes | 300-600 bytes (1-3 clusters) |
| Verification cost | O(n) additions | O(n) scalar mults |
| Tag visibility | Validators see weights | Weights hidden |

## Recommendations

1. **Current implementation is sufficient** for the < 1KB target with 1-3 clusters.

2. **Consider generator caching** if profiling shows hash-to-curve as a bottleneck.

3. **Implement batch verification** for nodes validating many transactions.

4. **Monitor real-world cluster distributions** to validate the 1-3 cluster assumption.

## Test Coverage

Proof size measurements are verified in `cluster-tax/src/crypto/committed_tags.rs`:

- `test_schnorr_proof_size`: Verifies 64-byte Schnorr proofs
- `test_committed_tag_vector_sizes`: Verifies vector sizes for 1, 3, 5, 8 clusters
- `test_conservation_proof_sizes`: Verifies conservation proof sizes
- `test_proof_size_under_1kb_target`: Asserts < 1KB for 1-3 clusters
- `test_proof_size_summary`: Documents size formulas

## References

- Parent Issue: #69 (Committed Cluster Tags)
- Implementation: `cluster-tax/src/crypto/committed_tags.rs`
- Benchmarks: `cluster-tax/benches/committed_tags_benchmarks.rs`
- Serialization: `cluster-tax/src/crypto/serialization.rs`
