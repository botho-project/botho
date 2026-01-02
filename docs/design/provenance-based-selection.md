# Provenance-Based Lottery Selection: A Novel Approach

## Abstract

This document proposes a novel approach to Sybil-resistant progressive mechanisms
in privacy-preserving cryptocurrencies. By leveraging **provenance information**
(cluster tag vectors) rather than identity or account count, we may achieve
properties previously thought impossible in pseudonymous systems.

## The Impossibility Triangle

Existing literature establishes a fundamental trade-off:

```
            Identity
               │
               │ ← Proof of Personhood
               │
Privacy ───────┼─────── Sybil Resistance
               │
               │ ← UTXO counting
               │
          No Identity
```

**Established result** (Kwon et al., 2019):
> "When Sybil cost is zero, the probability of achieving good decentralization
> approaches zero, given substantial wealth inequality."

All known solutions require one of:
1. Identity verification (Worldcoin, Proof of Humanity) - defeats privacy
2. Value-weighting (proportional to holdings) - not progressive
3. Accept gaming (Uniform selection) - 9x Sybil advantage

## The Provenance Dimension

Botho's cluster tag system tracks **coin provenance** without requiring identity:

```rust
/// Sparse vector of cluster tags for a UTXO.
/// Maps cluster IDs to weights indicating what fraction of value
/// traces back to each cluster's origin.
pub struct TagVector {
    tags: HashMap<ClusterId, TagWeight>,  // Up to 32 clusters
    // Remainder is "background" (fully decayed)
}
```

### Tag Evolution Through Commerce

| Event | Tag Change |
|-------|------------|
| **Mint** | 100% attribution to new cluster |
| **Transfer** | Tags decay by 5% per hop |
| **Receive** | Weighted average with incoming tags |
| **Hold** | No change (AND-decay: time alone doesn't decay) |

### Key Properties

1. **Deterministic**: Tags are computed from transaction history
2. **Verifiable**: Can be validated on-chain
3. **Privacy-preserving**: Reveals provenance patterns, not identity
4. **Already tracked**: Required for cluster-based fees

## The Novel Insight: Splits Preserve Provenance

When a Sybil attacker splits UTXOs:

```
Before: 1 UTXO
  value: 10,000,000
  tags: { cluster_A: 80%, cluster_B: 15%, background: 5% }
  entropy: 0.89

After split: 10 UTXOs (each)
  value: 1,000,000
  tags: { cluster_A: 80%, cluster_B: 15%, background: 5% }  ← IDENTICAL
  entropy: 0.89  ← UNCHANGED
```

**This is fundamentally different from UTXO counting:**

| Metric | Before Split | After Split | Change |
|--------|--------------|-------------|--------|
| UTXO count | 1 | 10 | 10x (gameable!) |
| Total value | 10M | 10M | 1x |
| Tag entropy | 0.89 | 0.89 | 1x (preserved!) |
| Tag concentration | 80% | 80% | 1x (preserved!) |
| Background % | 5% | 5% | 1x (preserved!) |

## Proposed Selection Modes

### 1. Entropy-Weighted Selection

```rust
SelectionMode::EntropyWeighted { entropy_bonus: f64 }

fn weight(utxo: &Utxo) -> f64 {
    let entropy = shannon_entropy(&utxo.tags);
    utxo.value as f64 * (1.0 + entropy_bonus * entropy)
}

fn shannon_entropy(tags: &TagVector) -> f64 {
    let mut entropy = 0.0;
    for (_, weight) in tags.iter() {
        let p = weight as f64 / TAG_WEIGHT_SCALE as f64;
        if p > 0.0 {
            entropy -= p * p.log2();
        }
    }
    // Include background contribution
    let bg = tags.background() as f64 / TAG_WEIGHT_SCALE as f64;
    if bg > 0.0 {
        entropy -= bg * bg.log2();
    }
    entropy
}
```

**Properties:**
- Fresh mint: entropy = 0 (single cluster) → minimum weight
- Traded coin: entropy > 0 (multiple sources) → bonus weight
- Sybil splits: same entropy → no advantage

### 2. Background-Weighted Selection

```rust
SelectionMode::BackgroundWeighted { background_bonus: f64 }

fn weight(utxo: &Utxo) -> f64 {
    let bg_fraction = utxo.tags.background() as f64 / TAG_WEIGHT_SCALE as f64;
    utxo.value as f64 * (1.0 + background_bonus * bg_fraction)
}
```

**Properties:**
- Fresh mint: 0% background → minimum weight
- Traded coin: >0% background (decayed) → bonus weight
- Sybil splits: same background → no advantage

### 3. Diversity-Weighted Selection

```rust
SelectionMode::DiversityWeighted { min_clusters: usize }

fn weight(utxo: &Utxo) -> f64 {
    if utxo.tags.len() < min_clusters {
        0.0  // Don't qualify
    } else {
        utxo.value as f64
    }
}
```

**Properties:**
- Requires provenance from N distinct clusters to participate
- Fresh mints and Sybil splits don't qualify
- Only coins with diverse economic history participate

## Comparison with Existing Approaches

| Approach | Sybil-Resistant | Progressive | Privacy | Gaming Vector |
|----------|-----------------|-------------|---------|---------------|
| **Uniform** | ✗ 9.3x | ✓ By count | ✓ Full | Split UTXOs |
| **ValueWeighted** | ✓ 1.0x | ✗ Proportional | ✓ Full | None |
| **ClusterWeighted** | ✓ 1.0x | ? Conditional | Partial | Factor manipulation |
| **EntropyWeighted** | ✓ 1.0x | ✓ By activity | Partial | Buy diverse coins |
| **BackgroundWeighted** | ✓ 1.0x | ✓ By age+trade | Partial | Wait + trade |
| **Proof of Personhood** | ✓ 1.0x | ✓ Per-person | ✗ None | Biometric spoofing |

## Gaming Analysis

### Can attackers artificially increase entropy?

To increase tag entropy, an attacker must:

1. **Trade with others holding different clusters**
   - Requires real counterparties
   - Incurs transaction fees
   - Is... legitimate economic activity!

2. **Mine multiple clusters**
   - Expensive (PoW required)
   - Each new cluster starts at factor 6.0
   - Diminishing returns

3. **Buy from exchanges**
   - Exchanges have highly mixed tags
   - Receiving from exchange increases entropy
   - But exchange coins are already in circulation

**Key insight**: Unlike Sybil attacks (which are free), increasing entropy requires
either real economic participation or real resource expenditure.

### Cost-Benefit Analysis

```
Entropy increase via self-trading:
- Each trade costs: base_fee × cluster_factor
- Each trade provides: ~5% decay (increases background, may increase entropy)
- Expected entropy gain per trade: ~0.1-0.2 bits
- Cost to gain 1 bit of entropy: ~5-10 transaction fees

If lottery_bonus_per_entropy_bit < 5-10 × base_fee:
  → Entropy gaming is negative EV
```

## Relationship to ClusterWeighted

ClusterWeighted uses the **factor** (derived from dominant cluster's wealth):
```
weight = value × (max_factor - factor + 1) / max_factor
```

**Problem identified**: Factor-wealth correlation is uncertain.
- New minters (often poor) start with HIGH factors (6.0)
- Established traders (possibly wealthy) have LOW factors (decayed)
- This could make ClusterWeighted REGRESSIVE

EntropyWeighted uses the **tag distribution** (intrinsic to the coin):
```
weight = value × (1 + entropy_bonus × tag_entropy)
```

**Advantage**: Entropy is an intrinsic property of economic history, not
dependent on external wealth correlations.

## Who Benefits?

| Participant | Tag Pattern | Entropy | Benefit Level |
|-------------|-------------|---------|---------------|
| Fresh minter | Single cluster 100% | 0 | Minimum |
| HODLer | Concentrated, stable | Low | Low |
| Active trader | Mixed, some background | Medium | Medium |
| Merchant | Many sources | High | High |
| Long-term holder | High background | High | High |
| **Exchange** | **Extreme diversity** | **Very High** | **Very High (!)** |

### The Exchange Problem

Exchanges receive deposits from many users with diverse provenance.
Their UTXOs would have maximum entropy.

**Potential mitigations:**
1. Cap entropy bonus at some threshold
2. Use background-weighted instead (exchanges may not hold long enough)
3. Combine with value caps
4. Accept that exchanges provide liquidity value

## Privacy Implications

| Mode | Information Revealed | Privacy Cost |
|------|---------------------|--------------|
| Uniform | Nothing | 0 bits |
| ValueWeighted | Approximate value (in lottery context) | ~0.5 bits |
| EntropyWeighted | Tag distribution complexity | ~1 bit |
| BackgroundWeighted | Approximate age/trade history | ~0.5 bits |
| ClusterWeighted | Coin origin category | ~1.5 bits |

EntropyWeighted reveals less than ClusterWeighted because it only exposes
the *complexity* of provenance, not the specific clusters involved.

## The Novel Contribution

### What This Is

A mechanism that:
1. **Rewards economic participation** (high entropy = more trades/sources)
2. **Is Sybil-resistant** (splits preserve entropy)
3. **Is privacy-preserving** (no identity required)
4. **Uses existing infrastructure** (cluster tags already tracked)

### What This Is NOT

This is NOT:
- Progressive wealth redistribution (doesn't directly target wealth)
- Identity-based (no proof of personhood)
- Perfect (exchanges benefit, gaming possible via real trades)

### The Honest Framing

> "Provenance-based selection rewards coins with diverse economic history.
> This correlates with economic participation (merchants, active users)
> rather than wealth accumulation (HODLers, minters). While not strictly
> progressive, it may better align incentives with network utility."

## Related Work

### Quadratic Funding (Gitcoin)
- Also faces Sybil problem with "number of contributors" weighting
- Solution: Identity verification (Gitcoin Passport)
- Our approach: Provenance verification (no identity needed)

### Proof of Personhood (Worldcoin, etc.)
- Solves Sybil via biometric/social verification
- Defeats privacy by linking identity
- Our approach: Provenance is verifiable without identity

### Impossibility Results (Kwon et al.)
- Proves decentralization impossible without Sybil costs
- Our insight: Provenance manipulation HAS costs (fees, PoW)
- Not a refutation, but a different cost structure

## Validation Requirements

Before claiming breakthrough status, test:

1. **Sybil resistance**: Confirm splits preserve entropy in simulation
2. **Participation correlation**: Does high entropy correlate with smaller holders?
3. **Exchange dominance**: Quantify exchange advantage
4. **Gaming cost**: Calculate break-even for artificial entropy increase
5. **vs. ClusterWeighted**: Direct comparison under controlled conditions
6. **Real-world validation**: Monitor entropy distribution in live network

## Implementation Path

1. Add `tag_vector` or `tag_entropy` field to `LotteryUtxo`
2. Implement `SelectionMode::EntropyWeighted`
3. Add simulation tests comparing to other modes
4. Analyze exchange/merchant/retail distributions
5. Parameter tuning for `entropy_bonus`

## Conclusion

Provenance-based selection may offer a novel point in the design space:
Sybil-resistant without identity, activity-rewarding without being gameable.

The key insight is that **tag entropy is preserved across splits** while
**requiring real economic activity to increase**. This creates a cost
structure that aligns with legitimate participation rather than gaming.

Further validation is needed, but this represents a potentially novel
contribution to the literature on mechanism design in private payment systems.

## References

- Kwon et al. (2019). "Impossibility of Full Decentralization in Permissionless
  Blockchains." ACM AFT. https://arxiv.org/abs/1905.05158
- Buterin, Hitzig, Weyl. "Quadratic Funding." https://github.com/gitcoinco/quadratic-funding
- Frontiers in Blockchain. "Who Watches the Watchmen? A Review of Subjective
  Approaches for Sybil-Resistance in Proof of Personhood Protocols."
  https://www.frontiersin.org/articles/10.3389/fbloc.2020.590171/full
- Gitcoin. "How to Attack and Defend Quadratic Funding."
  https://www.gitcoin.co/blog/how-to-attack-and-defend-quadratic-funding
- Ethereum Foundation. "On Inflation, Transaction Fees and Cryptocurrency
  Monetary Policy." https://blog.ethereum.org/2016/07/27/inflation-transaction-fees-cryptocurrency-monetary-policy
