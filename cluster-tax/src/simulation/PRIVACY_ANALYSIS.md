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
