# Entropy-Weighted Decay: Design Specification

## Overview

This document specifies **entropy-weighted decay** as Phase 2 of Botho's cluster tag decay mechanism. It addresses the patient wash trading vulnerability in age-based decay by tying decay credit to genuine economic activity as measured by cluster entropy changes.

**Status**: Proposed (Phase 2)
**Prerequisite**: Age-based decay (Phase 1) - see [cluster-tag-decay.md](cluster-tag-decay.md)

## Problem Statement

### The Patient Wash Trading Attack

Age-based decay (Phase 1) blocks rapid wash trading by requiring UTXOs to be at least 720 blocks (~2 hours) old before decay applies. However, this only **slows** attacks:

| Attack Strategy | Time Required | Decay Events | Tag Remaining |
|-----------------|---------------|--------------|---------------|
| 100 rapid self-transfers | Minutes | 0 (blocked) | 100% |
| Patient attack (1 day) | 24 hours | Max 12 | 54% |
| **Patient attack (1 week)** | **7 days** | **84** | **1.35%** |

An attacker with patience can reduce their cluster factor from high (15% fees) to low (1% fees) in one week through automated self-transfers.

### The Core Problem

Current decay grants credit for **any** eligible spend:
```
decay_effect = 5% per eligible spend (age ≥ 720 blocks)
```

This doesn't distinguish between:
- **Wash trading**: A→B→C→A (self-transfers that contribute nothing to economy)
- **Real commerce**: A→B→C→D→... (genuine economic activity with diverse counterparties)

## The Solution: Entropy-Weighted Decay

### Key Insight

**Cluster entropy is decay-invariant and commerce-sensitive.**

The codebase already has `cluster_entropy()` in `tag.rs`:

```rust
pub fn cluster_entropy(&self) -> f64 {
    // Excludes background (decay)
    // Only counts cluster weights
    // Increases through genuine mixing with diverse sources
}
```

Properties that make it ideal for this use case:

| Event | Shannon Entropy | Cluster Entropy |
|-------|-----------------|-----------------|
| Decay (aging) | Increases (background grows) | **Unchanged** |
| Self-transfer | Unchanged | **Unchanged** |
| Commerce (new source) | Increases | **Increases** |

### Mathematical Model

#### Entropy Delta Calculation

When spending a UTXO to create outputs, we measure the entropy change:

```
entropy_before = cluster_entropy(input_tags)
entropy_after = cluster_entropy(combined_output_tags)
entropy_delta = max(0, entropy_after - entropy_before)
```

For transactions with multiple inputs:
```
entropy_before = weighted_average(cluster_entropy(input_i), value_i)
```

#### Entropy Delta Factor

The entropy delta is converted to a multiplicative factor:

```rust
fn entropy_delta_factor(delta: f64, config: &EntropyDecayConfig) -> f64 {
    if delta <= config.min_delta_threshold {
        return config.min_factor; // Minimal decay credit for wash trades
    }

    // Linear interpolation between min and max
    let normalized = (delta - config.min_delta_threshold)
                   / (config.full_credit_delta - config.min_delta_threshold);
    let clamped = normalized.clamp(0.0, 1.0);

    config.min_factor + clamped * (1.0 - config.min_factor)
}
```

#### Decay Application

```rust
fn apply_entropy_weighted_decay(
    tags: &mut TagVector,
    entropy_delta: f64,
    config: &EntropyDecayConfig,
) -> f64 {
    let factor = entropy_delta_factor(entropy_delta, config);
    let effective_decay = config.base_decay_rate * factor;

    // Apply decay to all cluster tags
    for (cluster_id, weight) in tags.iter_mut() {
        let decay_amount = (*weight as f64 * effective_decay) as TagWeight;
        *weight = weight.saturating_sub(decay_amount);
    }

    effective_decay
}
```

### Configuration Parameters

```rust
pub struct EntropyDecayConfig {
    /// Base decay rate per eligible transaction (from Phase 1)
    pub base_decay_rate: f64,           // 0.05 (5%)

    /// Minimum entropy delta to receive any decay credit
    pub min_delta_threshold: f64,        // 0.1 bits

    /// Entropy delta for full decay credit
    pub full_credit_delta: f64,          // 0.5 bits

    /// Minimum decay factor for zero-delta transactions
    pub min_factor: f64,                 // 0.1 (10% of base)

    /// Age requirement (preserved from Phase 1)
    pub min_age_blocks: u64,             // 720 blocks
}
```

### Parameter Rationale

| Parameter | Value | Rationale |
|-----------|-------|-----------|
| `base_decay_rate` | 5% | Backward compatible with Phase 1 |
| `min_delta_threshold` | 0.1 bits | Filter out noise from rounding |
| `full_credit_delta` | 0.5 bits | Typical entropy increase from one new source |
| `min_factor` | 0.1 | Allow minimal decay for pure consolidation |
| `min_age_blocks` | 720 | Preserve rapid-attack protection |

## Attack Analysis

### Attack 1: Patient Wash Trading (Primary Target)

**Strategy**: Automated self-transfers spaced by 720 blocks over one week.

**Without entropy weighting (Phase 1)**:
```
84 decay events × 5% each = 97% decay
Tag remaining: 1.35%
Fees reduced: 15% → ~1%
```

**With entropy weighting (Phase 2)**:
```
Self-transfer: entropy_delta ≈ 0 → factor = 0.1
84 decay events × (5% × 0.1) = 84 × 0.5% = 42% decay
Tag remaining: 58%
Fees reduced: 15% → ~9% (still paying significant fees)
```

**Improvement**: Attack effectiveness reduced from 97% to 42%.

### Attack 2: Fake Commerce (Secondary Target)

**Strategy**: Create multiple "merchant" wallets, send funds in a circuit.

**Scenario**: A→B→C→D→A with 4 controlled wallets.

**Analysis**:
- First hop (A→B): entropy_delta = 0 (same source)
- All hops: Tags identical, no new sources introduced
- `cluster_entropy()` unchanged throughout circuit

**Result**: Same as wash trading—minimal decay credit.

### Attack 3: Entropy Purchasing

**Strategy**: Buy small amounts from many real merchants to increase entropy.

**Cost**: Each purchase requires:
1. Finding willing counterparty
2. Paying market rate + fees
3. Waiting for age requirement

**Analysis**:
- Expensive (must actually purchase goods/services)
- Creates genuine economic activity (not really an attack)
- Legitimate use of the system

**Verdict**: Not an attack—this IS the intended behavior.

### Attack 4: Entropy Mining via Dust

**Strategy**: Receive many tiny dust payments from diverse sources.

**Defense**: Entropy is weighted by value, not count:
```rust
fn cluster_entropy_weighted(&self) -> f64 {
    // Weight each source by its contribution
    // Dust sources contribute minimally
}
```

**Result**: Receiving 1000 dust payments provides less entropy than one meaningful transaction.

### Attack Comparison Summary

| Attack | Phase 1 (Age-Based) | Phase 2 (Entropy-Weighted) |
|--------|---------------------|----------------------------|
| Rapid wash (100 txs) | 0% decay | 0% decay |
| Patient wash (1 week) | 97% decay | ~42% decay |
| Fake commerce circuit | 97% decay | ~42% decay |
| Entropy purchasing | 64% decay | 64% decay |
| Dust mining | 97% decay | ~10% decay |
| **Real commerce (20 hops)** | **64% decay** | **64% decay** |

## Privacy Analysis

### Information Leakage

| Information | Phase 1 | Phase 2 | Additional Leakage |
|-------------|---------|---------|-------------------|
| UTXO creation block | Public | Public | None |
| Transaction structure | Public | Public | None |
| Input values | Public | Public | None |
| Tag weights | Private | Private | None |
| **Entropy delta** | N/A | **Computed** | **~0 bits** |

**Key insight**: Entropy delta is computed from tag vectors, but the **result** (decay amount) is private—it modifies internal tag state that is not revealed on-chain.

### Why Zero Privacy Cost?

Unlike lottery selection (which reveals entropy-based preferences publicly), decay happens **internally**:

1. Transaction is processed
2. Entropy delta computed from private tag data
3. Decay applied to private tag weights
4. Only the transaction outputs are visible

An observer sees the same transaction structure regardless of entropy delta.

### Ring Signature Considerations

For ring signatures, decay eligibility must be verifiable:

```rust
pub struct RingDecayProof {
    /// Zero-knowledge proof that entropy delta meets threshold
    pub entropy_threshold_proof: ZkProof,

    /// Commitment to the actual entropy delta (for future auditing)
    pub entropy_commitment: PedersenCommitment,
}
```

**Challenge**: Proving entropy relationships without revealing tag vectors.

**Approach**: Range proofs on entropy delta:
- Prove `entropy_delta >= min_threshold` without revealing exact value
- Verifier accepts proof → full decay credit
- Verifier rejects → minimal decay credit

See [Phase 2 Implementation](#phase-2-implementation) for ZK circuit details.

## Implementation

### State Requirements

No additional state required beyond Phase 1:

```rust
struct TagDecayState {
    tag_weights: HashMap<ClusterId, TagWeight>,
    // entropy computed on-demand from tag_weights
}
```

### Core Algorithm

```rust
impl TagVector {
    pub fn apply_entropy_weighted_decay(
        &mut self,
        other_inputs: &[TagVector],
        current_block: u64,
        utxo_creation_block: u64,
        config: &EntropyDecayConfig,
    ) -> DecayResult {
        // Phase 1 check: age requirement
        if current_block.saturating_sub(utxo_creation_block) < config.min_age_blocks {
            return DecayResult::NotEligible;
        }

        // Compute entropy before (this input + others)
        let entropy_before = self.combined_entropy(other_inputs);

        // Compute entropy after (post-decay state)
        // Note: This is estimated from input mix, actual may vary
        let entropy_after = self.estimate_output_entropy(other_inputs);

        let entropy_delta = (entropy_after - entropy_before).max(0.0);
        let factor = config.entropy_delta_factor(entropy_delta);

        // Apply weighted decay
        let effective_rate = config.base_decay_rate * factor;
        self.apply_decay(effective_rate);

        DecayResult::Applied {
            entropy_delta,
            factor,
            effective_rate,
        }
    }
}
```

### Entropy Calculation

Using the existing `cluster_entropy()` which excludes background:

```rust
impl TagVector {
    /// Cluster entropy - decay invariant, commerce sensitive
    pub fn cluster_entropy(&self) -> f64 {
        let total_cluster = self.total_attributed();
        if total_cluster == 0 {
            return 0.0;
        }

        let scale = total_cluster as f64;
        let mut entropy = 0.0;

        for (_, weight) in self.weights.iter() {
            if *weight > 0 {
                let p = *weight as f64 / scale;
                entropy -= p * p.log2();
            }
        }

        entropy
    }

    /// Combined entropy from multiple inputs
    fn combined_entropy(&self, others: &[TagVector]) -> f64 {
        if others.is_empty() {
            return self.cluster_entropy();
        }

        // Weighted average by total attributed weight
        let self_weight = self.total_attributed() as f64;
        let other_weights: Vec<f64> = others.iter()
            .map(|t| t.total_attributed() as f64)
            .collect();

        let total_weight = self_weight + other_weights.iter().sum::<f64>();
        if total_weight == 0.0 {
            return 0.0;
        }

        let weighted_sum = self.cluster_entropy() * self_weight
            + others.iter().zip(other_weights.iter())
                .map(|(t, w)| t.cluster_entropy() * w)
                .sum::<f64>();

        weighted_sum / total_weight
    }
}
```

## Migration Path

### Phase 1 → Phase 2 Transition

1. **Deploy Phase 2 code** with feature flag disabled
2. **Soft fork activation** at predetermined block height
3. **Gradual ramp-up** of `min_factor`:
   - Week 1: min_factor = 0.8 (20% reduction for wash trades)
   - Week 2: min_factor = 0.5 (50% reduction)
   - Week 3: min_factor = 0.25 (75% reduction)
   - Week 4+: min_factor = 0.1 (90% reduction, full effect)

### Backward Compatibility

- Transactions remain valid regardless of entropy delta
- Only decay credit changes, not transaction validity
- Nodes running old software see normal transactions

## Verification

### Simulation Commands

```bash
# Compare Phase 1 vs Phase 2 under patient wash attack
./target/release/cluster-tax-sim entropy-decay-compare \
  --wealth 100000000 \
  --base-decay 5.0 \
  --min-age-blocks 720 \
  --wash-txs 1000 \
  --blocks 60480 \
  --min-factor 0.1

# Test entropy purchasing scenario
./target/release/cluster-tax-sim entropy-commerce \
  --wealth 100000000 \
  --commerce-hops 20 \
  --sources-per-hop 1
```

### Expected Results

| Scenario | Phase 1 Remaining | Phase 2 Remaining |
|----------|-------------------|-------------------|
| Patient wash (1 week) | 1.35% | 58% |
| Real commerce (20 hops) | 36% | 36% |
| Mixed (10 wash + 10 commerce) | 10% | 28% |

## Open Questions

### Q1: Should consolidation receive any decay credit?

**Current design**: Yes, min_factor = 0.1 allows 10% decay credit.

**Alternative**: min_factor = 0.0 for pure consolidation (no entropy increase).

**Trade-off**: Stricter is more secure but may punish legitimate consolidation.

### Q2: How to handle entropy from decoy selection in rings?

**Challenge**: Ring members contribute to apparent entropy but may be decoys.

**Approach**: Use only the real input's entropy (requires ZK proof).

### Q3: Should entropy bonus stack with decay?

**Context**: Lottery already uses entropy for selection weighting.

**Decision**: Keep mechanisms independent—entropy affects both lottery AND decay, creating consistent incentive for genuine commerce.

## Related Documents

- [Cluster Tag Decay](cluster-tag-decay.md) - Phase 1 specification and vulnerability acknowledgment
- [Provenance-Based Selection](provenance-based-selection.md) - Entropy-weighted lottery (parallel mechanism)
- [Lottery Redistribution](lottery-redistribution.md) - Fee redistribution overview
- [Progressive Fees](../concepts/progressive-fees.md) - Overall fee curve design

## References

- GitHub Issue #257 - Patient wash trading vulnerability analysis
- GitHub Issue #258 - Related entropy analysis
- GitHub Issue #259 - Implementation tracking (blocked by this spec)
- GitHub Issue #232 - Phase 2 committed tags

## Changelog

- 2026-01-06: Initial specification (resolves #257)
