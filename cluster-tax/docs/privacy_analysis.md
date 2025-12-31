# Privacy Analysis: Provenance-Based Progressive Fees

## The Fundamental Tension

Progressive fees based on coin provenance create an inherent tension:

- **For fee correctness**: Need to know source_wealth (where coins came from)
- **For privacy**: Need to hide transaction history

This document analyzes how Botho resolves this tension.

## Privacy Threats from Provenance Tags

### Threat 1: Direct Provenance Tracking

If `source_wealth` is publicly visible on UTXOs:
```
UTXO A: value=100K, source_wealth=1,000,000  ← "This is whale money"
UTXO B: value=100K, source_wealth=10,000     ← "This is small-holder money"
```

**Impact**: Observers can:
- Track coins back to their origin
- Identify wealthy cluster members by their spending patterns
- Link transactions across time via source_wealth fingerprints

### Threat 2: Cluster Fingerprinting Attack

Even with ring signatures, tag distribution can identify the real spend:
```
Ring member 1: [cluster_1: 90%, cluster_2: 10%]  ← Unusual pattern
Ring member 2: [cluster_1: 50%, background: 50%]
Ring member 3: [cluster_1: 50%, background: 50%]
```

The unique tag fingerprint (90%/10% split) narrows the anonymity set.

**Simulation results** (from PRIVACY_ANALYSIS.md):
- Cluster fingerprinting attack: **59.5% identification rate**
- Combined attack: 32.6% identification rate
- This is the primary privacy threat

### Threat 3: Fee Amount Analysis

Fee amount correlates with source_wealth:
```
Transaction A: 10K transfer, 1,500 fee (15%)  ← Whale-origin money
Transaction B: 10K transfer, 114 fee (1.1%)   ← Small-holder money
```

Even without seeing tags, the fee rate reveals provenance.

## Privacy Solutions

### Phase 1: Public Tags (Current Simulation)

Tags are visible on-chain. Simple but privacy-limited.

**Validation**:
```
validate_tag_inheritance(inputs, outputs, fee, ...)
  → Check value conservation
  → Check tag mass conservation (with decay)
  → Check fee sufficiency
```

**Privacy level**: Low. Tags are visible, provenance is trackable.

**Use case**: Testing, proof of concept, trusted environments.

### Phase 2: Committed Tags (Cryptographic Privacy)

Tags are hidden using Pedersen commitments. The implementation in `src/crypto/committed_tags.rs`:

```rust
// Each UTXO commits to its tag masses without revealing them
CommittedTagMass {
    cluster_id: ClusterId,
    commitment: CompressedRistretto,  // C = mass * H_k + blinding * G
}
```

**How it works**:

1. **Pedersen Commitments**: Each cluster's tag mass is committed:
   ```
   C_k = mass_k * H_k + blinding_k * G
   ```
   - `H_k` is a unique generator per cluster (hash-to-curve)
   - The commitment hides `mass_k` but allows verification

2. **Tag Conservation Proof**: ZK proof that:
   ```
   sum(output_mass_k) ≤ (1 - decay) * sum(input_mass_k)
   ```
   Uses Schnorr proofs on the blinding factor differences.

3. **Homomorphic Property**: Commitments can be added:
   ```
   C1 + C2 = (m1 + m2) * H_k + (b1 + b2) * G
   ```
   This allows validators to check mass conservation without decryption.

**Privacy level**: High. Only commitment values are on-chain.

### Ring Signatures with Cluster-Aware Decoy Selection

To prevent cluster fingerprinting, decoys are selected to have similar tag distributions:

```
Real spend: [cluster_1: 70%, cluster_2: 30%]
Decoy 1:    [cluster_1: 68%, cluster_2: 32%]  ← Similar distribution
Decoy 2:    [cluster_1: 72%, cluster_2: 28%]  ← Similar distribution
...
```

**Simulation results**:
| Ring Size | Theoretical Bits | Measured Bits | Efficiency |
|-----------|-----------------|---------------|------------|
| 11 | 3.46 | 3.30 | 95.3% |
| 16 | 4.00 | 3.78 | 94.5% |
| 20 | 4.32 | 4.10 | 94.8% |

~95% of theoretical anonymity is preserved despite tag fingerprinting.

## Privacy-Preserving Fee Verification

The critical question: How do validators verify fees without seeing source_wealth?

### Approach 1: Range Proofs on Fee Rate

Prove that fee_paid ≥ required_fee without revealing either:
```
required_fee = transfer_amount × rate(effective_wealth)
fee_paid ≥ required_fee

// ZK proof that:
// 1. fee_paid is committed correctly
// 2. rate was computed correctly from committed effective_wealth
// 3. fee_paid ≥ transfer_amount × rate
```

This requires bulletproof-style range proofs.

### Approach 2: Committed Fee Curve Evaluation

The fee curve `rate(wealth)` can be evaluated on committed values:
```
rate_commitment = f(wealth_commitment)

// Prove correct evaluation without revealing wealth
```

This is more complex but allows full privacy.

### Current Implementation Status

The `committed_tags.rs` implementation focuses on tag conservation proofs. Fee verification with committed values would extend this with:

1. Committed effective_wealth computation
2. Range proof that fee ≥ rate(committed_wealth) × amount
3. This is feasible but adds proof complexity

## Natural Privacy Through Decay

An important observation: **decay provides natural privacy over time**.

```
Hop  0: source_wealth = 1,000,000
Hop  5: source_wealth = 236,145
Hop 10: source_wealth = 103,894
```

After sufficient commerce:
- Tags blend toward population average
- Original provenance becomes statistically diluted
- Old coins have naturally obscured history

This means:
- Fresh whale money has low privacy (high source_wealth is distinctive)
- Well-circulated money has high privacy (source_wealth ≈ average)

**This aligns incentives**: Privacy improves as money participates in the real economy.

## Summary: Privacy Tradeoffs

| Aspect | Phase 1 (Public) | Phase 2 (Committed) |
|--------|-----------------|---------------------|
| Tag visibility | Visible | Hidden |
| Provenance tracking | Easy | Not possible |
| Fee verification | Direct | ZK proof required |
| Ring signature efficiency | ~95% | ~95% |
| Implementation complexity | Low | High |
| Proof size overhead | None | ~2KB per cluster |

## Open Questions

1. **Fee commitment complexity**: How much does committed fee verification add to tx size?

2. **Cluster ID visibility**: In Phase 2, are cluster_ids visible? If so, this leaks some information (number and identity of source clusters).

3. **Background weight handling**: The "unattributed" portion of value needs careful handling in commitments.

4. **Interaction with stealth addresses**: Both outputs (payment and change) get the same tags. Does this create any privacy leakage?

5. **Long-term tag accumulation**: As more clusters are created, does tag vector size become a privacy/efficiency concern?

## Recommendations

1. **Ship Phase 1 for testnet** with public tags to validate correctness
2. **Implement Phase 2 for mainnet** with committed tags
3. **Prioritize cluster-aware decoy selection** as the primary ring signature improvement
4. **Accept that fresh whale money has lower privacy** - this is a feature, not a bug
5. **Document the privacy model clearly** so users understand the tradeoffs

## Conclusion

The provenance-based progressive fee system CAN achieve strong privacy through:
- Pedersen commitments hiding tag values
- ZK proofs verifying tag conservation and fee sufficiency
- Ring signatures with cluster-aware decoy selection
- Natural decay obscuring history over time

The implementation in `committed_tags.rs` provides the cryptographic foundation. The key remaining work is:
- Committed fee verification proofs
- Integration with ring signature system
- Performance optimization for multi-cluster tags
