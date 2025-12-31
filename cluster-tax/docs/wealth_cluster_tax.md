# Wealth Cluster Tax: Design Rationale

## Core Insight

**We tax wealth CLUSTERS, not individuals.**

Coins from wealthy sources carry a fee burden that persists through transfers and decays over time. This is fundamentally different from taxing wealthy *people* - it's taxing wealthy *money*.

## Why This Design?

### The Sybil Problem

Without provenance tracking:
```
Whale → Sybil1 → Sybil2 → ... → spend with 0% fee
```
The whale escapes progressive taxation by laundering through sybil accounts.

With cluster-based fees:
```
Whale → Sybil1 → Sybil2 → ... → still pays whale-level fee (decayed)
```
Coins remember their origin. Sybil shuffling doesn't help.

### The Key Mechanism

1. **Coins inherit provenance**: When a whale pays someone, those coins carry the whale's "cluster wealth" tag
2. **Tags decay per hop**: Each transfer reduces the cluster_wealth attribution by ~5%
3. **Fee based on cluster wealth**: Higher cluster_wealth → higher fee rate
4. **Mixing accelerates decay**: Legitimate commerce blends money from multiple sources, averaging down cluster_wealth faster than pure decay

## Economic Effects

### What Gets Taxed

| Scenario | Fee Level | Why |
|----------|-----------|-----|
| Whale spends directly | High | Fresh whale money |
| Whale's recipient spends | High (decayed) | Still linked to whale cluster |
| After 20 hops through economy | Low | Tags have decayed/mixed |
| Poor person spends their own | Low | Low cluster_wealth origin |

### Anti-Sybil Properties

Sybil accounts cannot escape the cluster tax because:
- Coins retain their cluster_wealth attribution
- Shuffling through empty accounts only applies decay (slow)
- No mixing with other sources (pure exponential decay)
- Still ~902x higher fees after 3 sybil hops vs normal origin

### Natural Decay Through Commerce

When money flows through the "real economy":
- Merchants accumulate from many sources
- Each payment blends tags from diverse origins
- Cluster_wealth averages toward normal levels
- "Whale tax" fades through legitimate activity

## Simulation Results

### Progressive vs Flat Fees (500 rounds, 100 agents)

| Metric | Progressive (1-30%) | Flat (5%) |
|--------|---------------------|-----------|
| ΔGini | **-0.0668** | -0.0205 |
| Supply Burned | 16.0% | 43.2% |

**Progressive is 3.3x more effective** at reducing inequality while burning **less** total supply.

### Sybil Resistance (3 hops through sybil chain)

```
Whale coins after 3 hops:  cluster_wealth = 857,375
Normal coins (1K origin):  cluster_wealth = 950

Ratio: 902x higher fee burden persists
```

### Decay Through Commerce (19 legitimate hops)

```
Initial:  cluster_wealth = 1,000,000
Final:    cluster_wealth = 5,133 (0.51%)

Faster than pure 5% decay (expected 37.74%)
because legitimate commerce MIXES sources.
```

## Implementation

### Tag Structure

Each UTXO carries:
- `value`: Amount of coins
- `cluster_wealth`: Effective wealth of the source (decays over hops)

Or with full provenance (multi-cluster):
- `tags: Dict[cluster_id, weight]`: Weighted attribution to source clusters
- Fee based on `max(tag_weight × cluster_wealth)` across all clusters

### Transfer Logic

```python
def transfer(sender, receiver, amount):
    # 1. Compute effective wealth from input UTXOs
    effective_wealth = weighted_average(input.cluster_wealth for input in inputs)

    # 2. Calculate fee based on cluster wealth
    fee_rate = progressive_rate(effective_wealth, max_wealth)
    fee = amount * fee_rate

    # 3. Apply decay to cluster_wealth for outputs
    decayed_wealth = effective_wealth * (1 - DECAY_RATE)

    # 4. Create outputs with decayed cluster_wealth
    # Both payment and change get decayed tags (can't distinguish with stealth addresses)
    create_utxo(receiver, amount, cluster_wealth=decayed_wealth)
    create_utxo(sender, change, cluster_wealth=decayed_wealth)

    # 5. Burn fee
    burn(fee)
```

### Privacy Considerations

**Key constraint**: With stealth addresses, we cannot distinguish payment from change.

Therefore:
- All outputs must receive the same tag treatment
- Decay applies to ALL outputs, not just "payments"
- This is intentional for privacy, and the cluster tax model still works

## Why Not Tax Individuals?

Taxing individuals (rather than clusters) would require:
1. Identifying the sender's total wealth
2. Linking UTXOs to identities
3. Either revealing identity (privacy loss) or trusting self-declaration

Cluster-based taxation avoids this by:
- Taxing the *coins themselves* based on their provenance
- No identity required - just track where coins came from
- Privacy preserved - only coin history matters, not who holds them now

## UTXO Simulation Results

### Simple Model: source_wealth + tag_weight (500 rounds, 100 agents)

Each UTXO tracks:
- `source_wealth`: Wealth of original minter (FIXED, blends on transfer)
- `tag_weight`: Attribution weight (DECAYS with partial rate)
- `effective_wealth = source_wealth × tag_weight`

**Key fix**: Partial decay based on transfer fraction:
```
decay_applied = base_decay × (amount_transferred / total_input)
```
This prevents frequent transactors from losing their tag weight when moving change.

| Config | ΔGini | Burned | Whale EW | Retail EW |
|--------|-------|--------|----------|-----------|
| Flat 5% | -0.2353 | 9.1% | 921,373 | 890,383 |
| Prog 0.5%-10% | -0.2345 | 8.9% | 887,691 | 851,441 |
| **Prog 1%-15%** | **-0.2396** | 12.8% | 882,487 | 836,911 |

**Key Observations:**

1. **Progressive reduces Gini more**: -0.2396 vs -0.2353 (+1.8% better)
2. **Reasonable burn rate**: 12.8% for best progressive vs 9.1% flat
3. **Tag weights preserved**: ~66% whale, ~88% retail (partial decay works)
4. **Whale/Retail ratio maintained**: ~1.1x (differentiation preserved)

## Conclusion

The wealth cluster tax creates a system where:
- Fresh money from wealthy sources is expensive to spend
- Well-circulated money approaches baseline fees
- Sybil accounts cannot launder away the tax burden
- Legitimate commerce naturally normalizes fees over time
- Fee differentiation by wealth tier is achieved

### Recommended Implementation

**UTXO Structure:**
```
UTXO {
    value: u64,
    source_wealth: u64,  // Wealth of minter, blends on transfer
    tag_weight: u32,     // Attribution (0-1M scale), decays partially
}

effective_wealth = source_wealth × (tag_weight / 1_000_000)
fee_rate = lerp(r_min, r_max, effective_wealth / max_wealth)
```

**Partial Decay Formula:**
```
transfer_fraction = amount / sum(input_values)
decay_applied = base_decay × transfer_fraction
new_tag_weight = avg_tag_weight × (1 - decay_applied)
```

**Recommended Parameters:**
- Base decay: 5% per transfer
- Fee range: 1% (min) to 15% (max)
- This achieves ~2% better Gini reduction than flat fees with ~40% more burning
