# Provenance-Based Lottery Selection

## Abstract

This document analyzes lottery-based fee redistribution in privacy-preserving
cryptocurrencies and proposes provenance-based selection as a mechanism to
reduce (but not eliminate) Sybil attack advantage.

**Key findings:**
1. Pure uniform lottery selection has ~10× Sybil advantage
2. Provenance (entropy) weighting reduces this to ~3-6×
3. Network scale provides natural Sybil resistance (larger N = less profit per UTXO)
4. Superlinear output fees prevent mass splitting but not patient accumulation
5. True Sybil elimination requires either identity or acceptance of reduced progressivity

## The Fundamental Trade-Off

The literature establishes an impossibility result:

> "In a pseudonymous system without identity, you cannot have both:
> - Progressive redistribution (favoring the numerous over the wealthy)
> - Full Sybil resistance (preventing gaming via multiple accounts)"

**Source:** Kwon et al. "Impossibility of Full Decentralization" (2019)

This document explores how far we can push the boundary while remaining honest
about the limitations.

## Mechanism Overview

### Fee Structure

```
Transaction fee = base_rate × amount × outputs^exponent

Where:
  base_rate = 0.5% (proportional to transfer value)
  exponent = 2.0 (quadratic penalty for many outputs)
```

### Fee Distribution

```
80% → Lottery pool (redistributed to random UTXOs)
20% → Burned (deflationary pressure)
```

### Lottery Selection

Each UTXO has weight proportional to:
```
weight = f(value) × g(entropy)

Where:
  f(value) = value or sqrt(value) or capped_value
  g(entropy) = 1 + bonus × tag_entropy
```

## Sybil Attack Analysis

### Attack Vector 1: Mass Splitting

**Attack:** Create many UTXOs in a single transaction.

**Defense:** Quadratic output fees.

```
Creating 10 UTXOs in one transaction:
  Fee = 0.5% × amount × 10² = 0.5% × amount × 100
  Fee per UTXO = 5% × amount

Creating 2 UTXOs (normal):
  Fee = 0.5% × amount × 4 = 2% × amount
  Fee per UTXO = 1% × amount

Cost ratio: 5× more expensive per UTXO via mass splitting.
```

**Verdict:** Mass splitting is economically discouraged. ✓

### Attack Vector 2: Patient Accumulation

**Attack:** Do normal transactions, never consolidate change UTXOs.

**Defense:** ???

```
Attacker does 100 normal transactions over time.
Each transaction: pays normal fee, creates 1 change UTXO.
Result: 100 UTXOs, each paid only the normal transaction fee.

Marginal cost of NOT consolidating: ZERO
The attacker would have done these transactions anyway.
```

**Verdict:** Patient accumulation has no marginal cost. ✗

This is the fundamental problem. Normal economic activity naturally creates
UTXOs as a byproduct. An attacker simply keeps them separate instead of
consolidating.

### Attack Vector 3: UTXO Farming

**Attack:** Do gratuitous transactions purely to create UTXOs.

**Defense:** Fee exceeds expected lottery return.

```
For farming to be unprofitable:
  fee_per_tx > expected_lottery_return_per_utxo × lifetime

With our parameters:
  Fee per tx = 0.5% × 1000 BTH × 4 = 20 BTH

  At N = 1M UTXOs, P = 8400 BTH/day:
    Return per UTXO = 8400/1M × 365 = 3.07 BTH/year
    Break-even: 20 BTH / 3.07 BTH/year = 6.5 years

  At N = 100K UTXOs:
    Return per UTXO = 8400/100K × 365 = 30.7 BTH/year
    Break-even: 20 BTH / 30.7 BTH/year = 0.65 years (!)
```

**Verdict:** Depends critically on network size. ⚠️

| Network Size | Expected Return/UTXO/Year | Break-Even | Farming Profitable? |
|--------------|---------------------------|------------|---------------------|
| 100K UTXOs | 30.7 BTH | 0.65 years | YES - very profitable |
| 1M UTXOs | 3.07 BTH | 6.5 years | Marginal |
| 10M UTXOs | 0.31 BTH | 65 years | NO - unprofitable |

**Key insight:** The lottery is naturally Sybil-resistant at scale, but
vulnerable during the bootstrap phase when N is small.

## Provenance-Based Mitigation

### The Core Insight

When an attacker splits UTXOs, the resulting UTXOs have **identical tag vectors**:

```
Before: 1 UTXO with tags {A: 80%, B: 15%, bg: 5%}
After:  10 UTXOs each with tags {A: 80%, B: 15%, bg: 5%}  ← IDENTICAL

Tag entropy before = tag entropy after = 0.89
```

Legitimate commerce creates UTXOs with **diverse provenance**:

```
Alice receives from Bob:   tags from Bob's history
Alice receives from Carol: tags from Carol's history
Alice receives from Dave:  tags from Dave's history

Each UTXO has different tag vector → different entropy
```

### Entropy-Weighted Selection

```rust
fn selection_weight(utxo: &Utxo, config: &Config) -> f64 {
    // IMPORTANT: Use cluster_entropy(), not shannon_entropy()
    // cluster_entropy() is decay-invariant - see Implementation section
    let entropy = cluster_entropy(&utxo.tags);
    utxo.value as f64 * (1.0 + config.entropy_bonus * entropy)
}
```

### Quantitative Effect

| Source | Typical Entropy | Weight (bonus=0.5) | Relative |
|--------|-----------------|---------------------|----------|
| Fresh mint | 0.0 | value × 1.0 | 1.0× |
| Self-split | 0.5-1.0 | value × 1.25-1.5 | 1.25-1.5× |
| Commerce coin | 1.5-2.5 | value × 1.75-2.25 | 1.75-2.25× |
| Exchange coin | 2.5-3.5 | value × 2.25-2.75 | 2.25-2.75× |

### Sybil Advantage With Entropy Weighting

Without entropy weighting (pure uniform):
```
10 Sybil UTXOs vs 1 legitimate UTXO (same total value):
Sybil advantage = 10× (each UTXO = 1 lottery ticket)
```

With entropy weighting (bonus = 0.5):
```
Assume: Sybil entropy = 0.6, Legitimate entropy = 2.0

Sybil weight = 10 × (1 + 0.5 × 0.6) = 10 × 1.3 = 13
Legit weight = 1 × (1 + 0.5 × 2.0) = 1 × 2.0 = 2

Sybil advantage = 13/2 = 6.5× (reduced from 10×)
```

**Entropy weighting reduces Sybil advantage by ~35%, but does not eliminate it.**

### Why Can't We Eliminate It?

To fully eliminate Sybil advantage, we would need weight to be **inversely**
proportional to UTXO count per owner. But:

1. We can't identify "owners" (privacy requirement)
2. We can only use on-chain observable properties
3. Tag entropy is preserved across splits, not increased

The best we can do is make Sybil UTXOs relatively less valuable than
legitimate commerce coins.

## Honest Assessment

### What This Mechanism Achieves

| Property | Status | Notes |
|----------|--------|-------|
| Progressive (wealthy pay more) | ✓ YES | Proportional fees |
| Progressive (poor win more) | ⚠️ PARTIAL | Statistical, not guaranteed |
| Mass splitting prevented | ✓ YES | Quadratic fees |
| Patient accumulation prevented | ✗ NO | No marginal cost |
| UTXO farming prevented | ⚠️ DEPENDS | On network size |
| Sybil advantage reduced | ✓ YES | ~35% reduction via entropy |
| Sybil advantage eliminated | ✗ NO | Still 6-7× advantage |

### What This Mechanism Does NOT Achieve

1. **True Sybil resistance** - Attackers still gain advantage from multiple UTXOs
2. **Guaranteed progressivity** - Depends on UTXO distribution assumptions
3. **Identity-free equality** - Cannot achieve "one person, one vote" without identity

### When This Mechanism Works Best

1. **Large networks** (N > 1M) - Low lottery value per UTXO discourages farming
2. **High fee environments** - Farming becomes unprofitable faster
3. **Diverse economy** - Legitimate users have high-entropy coins

### When This Mechanism Fails

1. **Small networks** (N < 100K) - High lottery value per UTXO attracts farming
2. **Low fee environments** - Farming break-even is too fast
3. **Concentrated economy** - Few sources means low entropy even for legitimate users

## The Bootstrap Problem

The mechanism is most vulnerable when the network is young:

```
Year 1: N = 50K UTXOs
  - Return per UTXO = 61.3 BTH/year
  - Farming break-even = 0.33 years
  - Sybil is VERY profitable

Year 5: N = 500K UTXOs
  - Return per UTXO = 6.1 BTH/year
  - Farming break-even = 3.3 years
  - Sybil is marginally profitable

Year 10: N = 5M UTXOs
  - Return per UTXO = 0.61 BTH/year
  - Farming break-even = 33 years
  - Sybil is unprofitable
```

### Potential Bootstrap Mitigations

1. **Lower lottery fraction during bootstrap** - Reduce pool_fraction from 80% to 20%
2. **Higher minimum UTXO value** - Increase capital lockup cost
3. **Longer eligibility delay** - Delay lottery participation for new UTXOs
4. **Adaptive parameters** - Adjust based on network size

## Comparison With Alternatives

| Approach | Sybil Resistance | Progressive | Privacy | Complexity |
|----------|------------------|-------------|---------|------------|
| Pure burning | N/A | NO | Full | Simple |
| Value-weighted lottery | 1.0× | NO | Full | Simple |
| Uniform lottery | 10× | YES | Full | Simple |
| Entropy-weighted lottery | 6.5× | YES | Partial | Moderate |
| Identity-based lottery | 1.0× | YES | NONE | Complex |

**The entropy-weighted approach is a compromise:** It sacrifices some privacy
(~1 bit revealed by entropy) and accepts reduced (not eliminated) Sybil
resistance in exchange for maintaining progressivity.

## Implementation Considerations

### Required Changes

1. **Add `tag_entropy` to UTXO structure** - Store or compute entropy per UTXO
2. **Implement entropy calculation** - Shannon entropy of tag distribution
3. **Add SelectionMode::EntropyWeighted** - New selection mode
4. **Track tag vectors through splits** - Preserve tags when splitting

### Critical: Use cluster_entropy(), NOT shannon_entropy()

**WARNING:** There are two entropy calculations. Using the wrong one creates a
gaming opportunity where old coins gain unfair lottery advantage.

| Method | Formula | Decay Behavior | Use Case |
|--------|---------|----------------|----------|
| `shannon_entropy()` | All sources including background | INCREASES with age | DO NOT USE for lottery |
| `cluster_entropy()` | Cluster sources only, renormalized | STABLE with age | USE for lottery |

The problem with `shannon_entropy()`:
- Background (decayed weight) counts as a source
- As coins age, background grows, adding "entropy" from decay
- Old coins get unfair lottery advantage just by waiting

The solution with `cluster_entropy()`:
- Background is excluded from calculation
- Cluster weights are renormalized to sum to 1.0
- Entropy only increases through genuine commerce (mixing sources)

```rust
/// CORRECT: Decay-invariant entropy for lottery selection
fn cluster_entropy(tags: &TagVector) -> f64 {
    let total_cluster = tags.total_attributed();
    if total_cluster == 0 {
        return 0.0; // Fully decayed = no diversity = 0 entropy
    }

    let scale = total_cluster as f64;
    let mut entropy = 0.0;

    // Renormalize cluster weights, excluding background
    for (_, weight) in tags.iter() {
        if weight > 0 {
            let p = weight as f64 / scale;
            entropy -= p * p.log2();
        }
    }

    entropy // bits
}
```

### Computational Cost

Cost: O(number of tags per UTXO) = O(32) = O(1) per UTXO

### Privacy Implications

| Information | Without Entropy | With Entropy | Leakage |
|-------------|-----------------|--------------|---------|
| UTXO value | Hidden | Hidden | 0 bits |
| UTXO age | Hidden | Partially revealed | ~0.5 bits |
| Provenance complexity | Hidden | Revealed | ~1 bit |
| Specific origins | Hidden | Hidden | 0 bits |

The entropy reveals whether a coin has "simple" (fresh/split) or "complex"
(traded) provenance, but not the specific clusters involved.

## Parameter Tuning: entropy_bonus

The `entropy_bonus` parameter controls how much lottery weight advantage
commerce coins get over fresh mints. The formula is:

```
weight = value × (1 + entropy_bonus × cluster_entropy)
```

### Effect of Different Values

| entropy_bonus | Commerce Advantage | Weight Range | Fresh Mint Return |
|---------------|-------------------|--------------|-------------------|
| 0.25 | 1.5× | 1.0× - 1.75× | -17% vs avg |
| **0.50** | **2.0×** | **1.0× - 2.50×** | **-29% vs avg** |
| 0.75 | 2.5× | 1.0× - 3.25× | -38% vs avg |
| 1.00 | 3.0× | 1.0× - 4.00× | -44% vs avg |

*Commerce Advantage = heavy commerce (2 bits) vs fresh mint (0 bits)*
*Fresh Mint Return assumes network mix: 40% fresh, 30% light, 20% medium, 10% heavy*

### Key Insight: Sybil Resistance is Independent of entropy_bonus

The entropy_bonus value does NOT affect Sybil resistance. For any bonus:

```
Before split: weight = V × (1 + b × E)
After split:  weight = 10 × (V/10) × (1 + b × E) = V × (1 + b × E)
```

Total weight is preserved regardless of `b`. The bonus only affects the
relative advantage of commerce coins vs fresh mints.

### Recommended Starting Value: 0.5

**Why 0.5:**
- Commerce coins get 2× lottery weight advantage (meaningful but not extreme)
- Fresh mints are disadvantaged but not excluded (-29% vs average)
- Weight variance is moderate (1.0× to 2.5×)
- Good balance of progressivity vs predictability

**When to adjust:**
- **Increase to 0.75-1.0** if >80% of UTXOs have entropy < 0.5 (weak commerce incentive)
- **Decrease to 0.25** if lottery winners are too concentrated in high-entropy coins

### Monitoring Metrics

Track these in production to guide parameter adjustment:

1. **Entropy distribution** - Histogram of cluster_entropy across all UTXOs
2. **Winner concentration** - Gini coefficient of lottery winnings
3. **Commerce participation** - % of transactions that increase entropy
4. **Fresh mint competitiveness** - Win rate of fresh mints vs their weight share

## Conclusions

### The Honest Summary

We cannot solve the Sybil problem in a privacy-preserving lottery without
identity verification. What we CAN do:

1. **Make mass splitting expensive** via superlinear fees (effective)
2. **Make farming unprofitable at scale** via proportional fees (effective at large N)
3. **Reduce Sybil advantage** via entropy weighting (~35% reduction)
4. **Accept partial Sybil resistance** as the cost of privacy + progressivity

### The Novel Contribution

The insight that **tag entropy is preserved across splits** suggests a new
point in the design space. While not a complete solution, it provides
meaningful Sybil resistance reduction without requiring identity.

### Recommendations

1. **Use entropy-weighted selection** - Better than pure uniform, preserves progressivity
2. **Start with low lottery fraction** - 20-30% during bootstrap, increase as N grows
3. **Monitor Sybil behavior** - Track entropy distribution in live network
4. **Adjust parameters** - Be prepared to tune as network characteristics evolve

### What We Are NOT Claiming

- ~~We solved the Sybil problem~~ We reduced it
- ~~Lottery is fully progressive~~ It's statistically progressive
- ~~No gaming is possible~~ Gaming is expensive but possible
- ~~This is a breakthrough~~ It's an incremental improvement

## References

- Kwon et al. (2019). "Impossibility of Full Decentralization in Permissionless
  Blockchains." ACM AFT. https://arxiv.org/abs/1905.05158
- Buterin, Hitzig, Weyl. "Quadratic Funding."
- Frontiers in Blockchain. "Who Watches the Watchmen? A Review of Subjective
  Approaches for Sybil-Resistance in Proof of Personhood Protocols."
- Gitcoin. "How to Attack and Defend Quadratic Funding."
- Ethereum Foundation. "On Inflation, Transaction Fees and Cryptocurrency
  Monetary Policy."
