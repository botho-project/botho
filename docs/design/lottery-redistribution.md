# Lottery-Based Fee Redistribution

## Overview

Botho uses a **lottery-based fee redistribution** system instead of burning transaction fees. This creates a progressive wealth redistribution effect where fees flow back to coin holders.

**Recommended Design: Immediate Distribution with Superlinear Fees**

```
Fee = base × cluster_factor × outputs²
     └─► 80% immediately distributed to 4 random UTXOs
     └─► 20% burned
```

**Key properties:**
- **Progressive via population statistics**: Random UTXO selection favors the many (poor) over the few (rich)
- **Sybil-resistant**: Superlinear per-output fees make splitting prohibitively expensive
- **Simple**: No activity tracking, no ticket accumulation, immediate distribution
- **Effective**: Achieves up to 100% Gini reduction under realistic conditions

## The Core Insight

In an unequal wealth distribution:
- Many poor people, few rich people
- If each person has ~1 UTXO, random selection favors the majority
- The majority are poor
- **Therefore: uniform random selection is inherently progressive**

```
Population: 90% poor (10% of wealth), 10% rich (90% of wealth)

If each person has 1 UTXO:
- Random UTXO → 90% chance poor person wins
- But rich fund most of the pool (they transact more, pay higher fees)
- Poor win 90% of lottery with ~10% of contributions
- That's 9× progressive redistribution!
```

## Problem Statement

### Why Not Burn Fees?

Burning fees creates deflationary pressure but doesn't actively redistribute wealth. The benefits accrue passively to all holders proportionally.

### Why Not Direct Taxation?

Our earlier cluster tax design had issues:
- Primarily taxed minters on first spend, not accumulated wealth
- Commerce wealth escaped (tags decay through trade)
- Complex fee calculations at transaction time
- Punitive framing ("pay more") vs. rewarding ("receive more")

### The Sybil Problem

Any naive redistribution faces Sybil attacks:
- 1 ticket per UTXO → split into many UTXOs
- Random selection → create many accounts
- Without identity, how do we prevent gaming?

**Solution**: Make UTXO creation expensive enough that splitting is unprofitable.

## The Combined Design

### Components

1. **Progressive Fees via Cluster Factor**
   ```
   fee = base × cluster_factor
   ```
   - Rich (high factor) pay more per transaction
   - Poor (low factor) pay less

2. **Superlinear Per-Output Fees**
   ```
   fee = base × cluster_factor × outputs²

   Examples (base=100, factor=3.0):
   2 outputs: 100 × 3 × 4 = 1,200 (normal tx)
   5 outputs: 100 × 3 × 25 = 7,500 (3× per output)
   10 outputs: 100 × 3 × 100 = 30,000 (5× per output)
   ```
   - Makes splitting prohibitively expensive
   - Normal transactions (2 outputs) are unaffected

3. **Immediate Distribution to Random UTXOs**
   ```
   Each transaction:
   1. Pay fee F
   2. 80% of F distributed to 4 random UTXOs (uniform selection)
   3. 20% burned
   ```
   - No pool accumulation needed
   - No periodic drawings
   - Simple implementation

4. **No Cluster Tracking for Lottery Eligibility**
   - UTXOs selected uniformly regardless of cluster factor
   - Cluster factor only affects fees paid, not lottery chances
   - Simpler state management

### Why It Works

| Mechanism | Effect |
|-----------|--------|
| Cluster-factor fees | Rich fund the pool disproportionately |
| Uniform UTXO selection | Poor win disproportionately (more of them) |
| Superlinear output fees | Can't game by splitting (quadratic cost) |
| Immediate distribution | Simple, no state to track |

### Fee Flow

```
Transaction (2 outputs)
    │
    ├──► Calculate: fee = base × factor × 4
    │
    ├──► 20% burned (deflation)
    │
    └──► 80% to 4 random UTXOs
              │
              ├──► UTXO A (randomly selected)
              ├──► UTXO B (randomly selected)
              ├──► UTXO C (randomly selected)
              └──► UTXO D (randomly selected)
```

## Sybil Resistance

### Why Splitting Doesn't Help

With superlinear per-output fees:

```
Creating 10 UTXOs in one transaction:
- Fee = base × factor × 100 (quadratic)
- Cost per UTXO = 10× normal

Creating 10 UTXOs in 10 transactions:
- Fee = 10 × base × factor × 4
- Still 2× more expensive than 2 UTXOs total
```

### Break-Even Analysis

For splitting to be profitable, expected lottery winnings must exceed creation cost.

```
Expected winnings per UTXO per tx = (prize_per_winner × 4) / total_UTXOs
                                  = 0.8 × fee / total_UTXOs

With 100,000 UTXOs and fee=1000:
- Expected per UTXO per tx = 800 / 100,000 = 0.008

To break even on a 1000-fee creation cost:
- Need 1000 / 0.008 = 125,000 transactions
- At 10,000 tx/day = 12.5 days

With superlinear fees (creating 10 outputs costs 25× more):
- Need 25,000 / 0.008 = 3,125,000 transactions = 312 days
```

With superlinear fees, splitting takes nearly a year to break even. Most users won't bother.

## Simulation Results

### Design Comparison (ValueWeighted Transactions)

| Design | Init Gini | Final Gini | Change |
|--------|-----------|------------|--------|
| Pooled + FeeProportional | 0.71 | 0.44 | +38.7% |
| Pooled + PureValueWeighted | 0.71 | 0.29 | +59.9% |
| Pooled + UniformPerUtxo | 0.71 | 0.09 | +86.8% |
| **COMBINED (Immediate + Uniform + Superlinear)** | 0.71 | **0.00** | **+100%** |

### Transaction Pattern Sensitivity

| Design | ValueWeighted | Uniform |
|--------|---------------|---------|
| FeeProportional | +38.7% | +43.5% |
| PureValueWeighted | +59.9% | -9.8% |
| UniformPerUtxo | +86.8% | -23.1% |
| **COMBINED** | **+100%** | +100%* |

*Standard inequality. Extreme inequality with uniform transactions still shows some regression.

### Key Finding

The combined design achieves **perfect equality** (Gini 0.0) under realistic conditions (ValueWeighted transactions). Even with uniform transactions, it achieves 100% Gini reduction for standard inequality distributions.

## Alternative Ticket Models

We evaluated multiple ticket models before arriving at the combined design:

### ActivityBased (Original)

```
tickets = (value / cluster_factor) × activity_multiplier
```

**Problem**: Gameable through wash trading (22,000× ROI).

### FeeProportional

```
tickets = fee_paid × (max_factor - your_factor) / max_factor
```

**Better**: Wash-resistant, but requires ticket state tracking.

### PureValueWeighted

```
tickets = value / cluster_factor
```

**Simpler**: No state tracking, but fails under uniform transactions.

### UniformPerUtxo (Population Statistics)

```
tickets = 1 (per UTXO, regardless of value)
```

**Insight**: Uses population statistics for progressivity. Works incredibly well (+86.8%) but still requires pooled distribution.

### Combined (Recommended)

```
No tickets! Just immediate distribution to random UTXOs.
```

**Best**: Simplest implementation, best results, uses population statistics insight.

## Implementation

### Transaction Processing

```rust
fn process_transaction(
    tx: &Transaction,
    utxo_set: &mut UtxoSet,
    cluster_factors: &ClusterFactors,
) {
    let spender = &tx.inputs[0];
    let factor = cluster_factors.get(spender);
    let num_outputs = tx.outputs.len() as u64;

    // Superlinear fee: base × factor × outputs²
    let fee = BASE_FEE * factor * num_outputs * num_outputs;

    // Deduct from spender
    utxo_set.deduct(spender, fee);

    // Burn 20%
    let burned = fee / 5;

    // Distribute 80% to 4 random UTXOs
    let to_distribute = fee - burned;
    let per_winner = to_distribute / 4;

    for _ in 0..4 {
        let winner = utxo_set.random_utxo();
        utxo_set.add(winner, per_winner);
    }
}
```

### State Requirements

Per UTXO:
- Value (existing)
- Cluster factor (existing)

Global:
- None! No pool, no tickets, no tracking.

### Consensus Changes

1. Calculate fee with superlinear output component
2. Select 4 random UTXOs deterministically (using block hash as seed)
3. Distribute 80% of fee to selected UTXOs
4. Burn remaining 20%

### Wallet Changes

1. Display expected lottery income based on UTXO count
2. Warn when creating many outputs (superlinear fee)
3. No ticket tracking needed

## Parameters

| Parameter | Value | Rationale |
|-----------|-------|-----------|
| Pool fraction | 80% | Balance redistribution vs. deflation |
| Burn fraction | 20% | Deflationary pressure, Sybil protection |
| Winners per tx | 4 | Balance variance vs. gas cost |
| Output exponent | 2.0 | Quadratic makes splitting expensive |
| Base fee | 100 | Baseline transaction cost |

## Privacy Analysis

### What's Already Public

- UTXO values
- Transaction outputs
- Ring members

### What This Adds

| Information | Visibility | Impact |
|-------------|------------|--------|
| Random winners | Deterministic from block hash | Low (anyone can compute) |
| Lottery income | Added to UTXO value | Low (already visible) |

### Why It's Privacy-Preserving

1. Winner selection is deterministic from public data (block hash)
2. No new information revealed about UTXO ownership
3. No activity tracking creates no new linkability

## Comparison to Alternatives

| Approach | Progressive? | Sybil-Resistant? | Simple? | Effective? |
|----------|-------------|------------------|---------|------------|
| Burn fees | No | N/A | Yes | No redistribution |
| Cluster tax | Weak | Yes | No | 7% Gini reduction |
| Pooled lottery | Yes | Mostly | No | 38-60% reduction |
| **Combined** | **Yes** | **Yes** | **Yes** | **100% reduction** |

## Philosophical Notes

### Why This Works

Traditional progressive systems require:
1. **Identity**: Know who has what
2. **Measurement**: Track wealth
3. **Enforcement**: Compel payment

We achieve progressivity without identity by:
1. Using **population statistics**: More poor people = more poor UTXOs
2. Using **cluster factors**: Rich pay higher fees
3. Using **economic incentives**: Splitting is unprofitable

### Limitations

The system depends on **rich transacting proportionally more** than poor. This is realistic:
- Wealthy entities have more economic activity
- Businesses transact more than individuals
- Investment activity scales with wealth

Under pathological conditions (everyone transacts equally, extreme inequality), the system can still increase inequality. But this is unlikely in practice.

### What We're Redistributing

This is not a wealth tax. We can't tax accumulated wealth without identity.

This is **seigniorage redistribution**: the privilege of money creation (minting) is taxed through cluster factors, and those taxes are redistributed to all participants weighted by population.

## References

- [Cluster Tag Decay](cluster-tag-decay.md) - How cluster factors decay through trade
- [Progressive Fees](../progressive-fees.md) - Fee curve design
- [Tokenomics](../tokenomics.md) - Overall economic model
