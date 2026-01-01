# Ring Signature Privacy Simulation Results

This document summarizes privacy simulation results for Botho's ring signature scheme.

## Methodology

Monte Carlo simulation with 10,000 ring formations per configuration:
- Synthetic UTXO pool (100,000 outputs)
- Realistic output type distribution (50% standard, 25% exchange, 10% whale, etc.)
- Gamma-distributed output ages (k=19.28, θ=1.61 days)
- Cluster-aware decoy selection with 70% minimum similarity threshold

## Results (Ring Size 20)

| Adversary Model | Measured Bits | Efficiency | ID Rate |
|-----------------|---------------|------------|---------|
| Naive | 4.32 | 100% | 5.0% |
| Age-Heuristic | 4.26 | 98.6% | 3.6% |
| Cluster-Fingerprint | 4.11 | 95.1% | 59.5% |
| Combined | 4.10 | 94.8% | 32.6% |

Theoretical maximum: 4.32 bits (log₂(20))

## Ring Size Comparison

| Ring Size | Theoretical | Measured | Efficiency | Signature Size |
|-----------|-------------|----------|------------|----------------|
| 11 | 3.46 bits | 3.30 bits | 95.3% | 35 KB |
| 16 | 4.00 bits | 3.78 bits | 94.5% | 50 KB |
| 20 | 4.32 bits | 4.10 bits | 94.8% | 62 KB |

## Context: Monero Comparison

For reference, Monero Research Lab's OSPEAD analysis (April 2025) found that timing-based attacks reduce Monero's ring-16 effective anonymity from 16 members to ~4.2 members (~52% efficiency).

Our simulation suggests cluster-aware decoy selection may provide better resistance to such attacks. However, direct comparison requires caution:

- **This is simulation data**, not real-world measurement
- Monero has 10+ years of adversarial analysis; these results are preliminary
- Different signature schemes and network conditions affect real-world privacy
- Monero is deploying FCMP++ which will fundamentally change their privacy model

## Running the Simulation

```bash
# Full privacy simulation
cargo run -p bth-cluster-tax --features cli --bin cluster-tax-sim --release -- \
    privacy -n 10000 --pool-size 100000

# Ring size comparison
cargo run -p bth-cluster-tax --features cli --bin cluster-tax-sim --release -- \
    ring-size --sizes 11,16,20 --simulate -n 2000
```

## Key Takeaways

1. Cluster-aware decoy selection achieves ~95% privacy efficiency in simulation
2. The cluster fingerprinting attack is the primary privacy threat
3. Larger ring sizes provide diminishing returns (0.07 bits/KB at ring-20)
4. Real-world validation is needed before making strong claims

## Committed Tag Vector Design Decision

### Cluster ID Visibility (v1)

**Decision**: Cluster IDs are **visible**, tag masses are **hidden** in commitments.

```
CommittedTagVector:
├── entries: Vec<CommittedTagMass>
│   ├── cluster_id: ClusterId       // VISIBLE - used for decoy selection
│   └── commitment: CompressedRistretto  // HIDDEN - mass in Pedersen commitment
└── total_commitment: CompressedRistretto  // HIDDEN - total attributed mass
```

### Rationale

**Why visible cluster IDs?**

1. **Decoy selection requirement**: Ring signatures require selecting decoys that appear plausible. With hidden cluster IDs, the wallet cannot determine which outputs are similar.

2. **Simpler implementation**: Visible IDs allow straightforward set-overlap similarity metrics without requiring zero-knowledge proofs for membership.

3. **Acceptable privacy cost**: Cluster IDs reveal *which* clusters an output is associated with, but not *how much* value is attributed to each. This is analogous to revealing transaction graph structure (which cryptocurrencies already do) while hiding amounts.

4. **Future extensibility**: Full cluster ID hiding is possible with fixed-size padding and more complex proofs, but adds significant complexity for marginal privacy gains.

### Privacy Implications

**Information leaked by visible cluster IDs:**

| Information | Leaked | Hidden |
|-------------|--------|--------|
| Which clusters output is associated with | ✓ | |
| Mass/weight per cluster | | ✓ |
| Total attributed mass | | ✓ |
| Background (unattributed) portion | | ✓ |

**Attack surface:**

- **Cluster set fingerprinting**: Adversary can identify outputs with identical cluster ID sets
- **Linkability by cluster set size**: Outputs with unusual numbers of clusters may be distinguishable
- **Mitigation**: Decoy selection prioritizes cluster set overlap, reducing fingerprinting effectiveness

### Decoy Selection with Committed Tags

With visible cluster IDs, decoy selection uses **Jaccard similarity** on cluster ID sets:

```
similarity(A, B) = |clusters(A) ∩ clusters(B)| / |clusters(A) ∪ clusters(B)|
```

This replaces the weighted cosine similarity used with plaintext tags (which required mass values).

**Selection algorithm:**
1. Extract cluster ID set from real output
2. Filter pool to outputs with Jaccard similarity ≥ 70%
3. Weight candidates by age distribution (gamma PDF)
4. Sample decoys using weighted random selection

### Quantified Privacy Impact

Based on simulation with committed tag vectors:

| Metric | Plaintext Tags | Committed Tags | Delta |
|--------|----------------|----------------|-------|
| Effective anonymity | 17.2 | 16.8 | -2.3% |
| Bits of privacy | 4.10 | 4.07 | -0.7% |
| ID rate (Combined) | 32.6% | 34.1% | +1.5% |

**Analysis**: Committed tags provide nearly identical privacy to plaintext tags because:
- Cluster set overlap is the dominant factor in adversary analysis
- Mass values provide minimal additional distinguishing information
- Age heuristics are unchanged

### Future Work: Full Cluster ID Hiding

For maximum privacy, cluster IDs could also be hidden:

1. **Fixed vector size**: All outputs have 8 cluster slots
2. **Zero-mass commitments**: Unused slots commit to 0 with random IDs
3. **Wallet decoy hints**: Wallet provides hint about which decoys are compatible
4. **Anonymized statistics**: Node maintains cluster distribution without specific IDs

This would eliminate cluster set fingerprinting but requires:
- ~2x commitment size per output
- Wallet-node coordination protocol
- More complex range proofs
