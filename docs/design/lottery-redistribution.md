# Lottery-Based Fee Redistribution

## Overview

Botho uses a **lottery-based fee redistribution** system instead of burning transaction fees. This creates a progressive wealth redistribution effect where fees flow back to coin holders.

**Design: Immediate Distribution with Superlinear Fees**

```
Fee = base × cluster_factor × outputs²
     └─► 80% immediately distributed to 4 random UTXOs
     └─► 20% burned
```

**Intended properties:**
- **Progressive via population statistics**: Random UTXO selection favors the many (poor) over the few (rich)
- **Sybil-resistant**: Superlinear per-output fees make splitting prohibitively expensive
- **Simple**: No activity tracking, no ticket accumulation, immediate distribution

## ⚠️ Critical Finding: The Progressivity-Sybil Trade-off

Stress testing revealed a **fundamental tension** between progressivity and Sybil resistance:

| Selection Mode | Sybil Advantage | Progressive? | Verdict |
|----------------|-----------------|--------------|---------|
| **Uniform** | 9.3x | ✅ Yes | ❌ VULNERABLE |
| **ValueWeighted** | 1.04x | ❌ No | ✅ Sybil-resistant |
| **SqrtWeighted** | 5.0x | Partial | ❌ Still vulnerable |
| **LogWeighted** | 8.5x | Partial | ❌ Nearly as bad |

**Key insight**: In a privacy-preserving system without identity, **you cannot have both**:
- Uniform selection is progressive but allows 9.3x gaming via UTXO splitting
- Value-weighted selection is Sybil-resistant but not progressive (proportional to wealth)
- Hybrid approaches (sqrt, log) don't successfully balance both properties

## The Core Insight (and Its Limitation)

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

**The problem**: The assumption "each person has ~1 UTXO" is fragile:
- Anyone can create multiple UTXOs over time through normal transactions
- An attacker with 10 UTXOs gets ~9.3x more lottery winnings
- Superlinear output fees only prevent *single-transaction* splitting
- Patient accumulation over time bypasses this protection

## Stress Test Results

### UTXO Accumulation Attack

```
BASELINE (1 UTXO):   Winnings=3.3M, Fees=13.1M, Net=-9.8M
GAMING (10 UTXOs):   Winnings=31.3M, Fees=39.5M, Net=-8.2M

Winnings ratio: 9.4x more with 10 UTXOs
Net result: Gaming strategy loses LESS money
```

While both strategies are net negative (wealthy pay more fees), the attacker
with multiple UTXOs **loses less**, creating an unfair advantage.

### Selection Mode Comparison

We tested alternative selection modes to mitigate gaming:

| Mode | 1 UTXO Winnings | 10 UTXO Winnings | Ratio | Verdict |
|------|-----------------|------------------|-------|---------|
| Uniform | 3.3M | 31.1M | 9.3x | VULNERABLE |
| ValueWeighted | 16.7M | 17.4M | 1.04x | Sybil-resistant |
| SqrtWeighted | 5.3M | 26.7M | 5.0x | Still vulnerable |
| LogWeighted | 3.6M | 30.4M | 8.5x | Nearly as bad |

**SqrtWeighted** was expected to give sqrt(10)≈3.16x advantage, but in practice
shows 5x due to interaction with value-weighted transaction selection.

### Exchange Entity Impact

When exchanges hold funds for many users in few UTXOs:

```
Scenario: 3 exchanges (50% of funds, 3 UTXOs) vs 1000 retail (50%, 1000 UTXOs)

Result:
- Retail wins 99.7% of lottery (matches UTXO proportion)
- Exchanges pay fees from user funds but win little
- Net redistribution FROM exchanges TO retail users
```

This is actually a **positive** finding - the lottery redistributes from
custodial entities to self-custody users.

### Why "100% Gini Reduction" is Misleading

The simulations show 100% Gini reduction because they run until wealth is
essentially consumed through fees + 20% burn. In practice:

1. Transaction volumes are much lower relative to total supply
2. Most wealth sits idle (doesn't transact)
3. Convergence would take decades, not days

The simulation's "1 day to equilibrium" result uses 864K tx/day with only
17 participants - an unrealistic ratio.

## The Design

Despite the limitations, the lottery mechanism still provides value:

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
   - Makes single-transaction splitting expensive
   - Normal transactions (2 outputs) are unaffected

3. **Immediate Distribution to Random UTXOs**
   ```
   Each transaction:
   1. Pay fee F
   2. 80% of F distributed to 4 random UTXOs
   3. 20% burned
   ```
   - No pool accumulation needed
   - No periodic drawings
   - Simple implementation

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

## Sybil Resistance Analysis

### What Works

**Single-transaction splitting** is expensive:
```
Creating 10 UTXOs in one transaction:
- Fee = base × factor × 100 (quadratic)
- Cost per UTXO = 10× normal
```

### What Doesn't Work

**Gradual accumulation** bypasses protections:
- Normal 2-output transactions create 1 new UTXO (change)
- Over time, an active user accumulates many UTXOs
- No additional cost beyond normal transaction fees
- Each UTXO provides additional lottery chances

### Break-Even Analysis (Revised)

```
With 100,000 UTXOs and fee=1000:
- Expected per UTXO per tx = 800 / 100,000 = 0.008

To break even on a 1000-fee creation cost:
- Need 1000 / 0.008 = 125,000 transactions
- At 10,000 tx/day = 12.5 days

This is NOT "nearly a year" - it's under two weeks.
```

For small networks, break-even is even faster, making the lottery
more vulnerable during bootstrap phase.

## Selection Mode Options

The implementation supports multiple selection modes:

### Uniform (Default)
```rust
SelectionMode::Uniform
```
- Each UTXO has equal probability
- Most progressive, most vulnerable to gaming

### Value-Weighted
```rust
SelectionMode::ValueWeighted
```
- Probability proportional to UTXO value
- Sybil-resistant but not progressive
- Equivalent to passive holding benefit

### Sqrt-Weighted
```rust
SelectionMode::SqrtWeighted
```
- Probability proportional to sqrt(value)
- Theoretical 3.16x gaming advantage
- In practice shows ~5x (still vulnerable)

### Log-Weighted
```rust
SelectionMode::LogWeighted
```
- Probability proportional to 1 + log2(value)
- Shows ~8.5x gaming advantage
- Nearly as vulnerable as uniform

## Implementation

### Transaction Processing

```rust
fn process_transaction(
    tx: &Transaction,
    utxo_set: &mut UtxoSet,
    cluster_factors: &ClusterFactors,
    selection_mode: SelectionMode,
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
        let winner = select_winner(utxo_set, selection_mode);
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

## Parameters

| Parameter | Value | Rationale |
|-----------|-------|-----------|
| Pool fraction | 80% | Balance redistribution vs. deflation |
| Burn fraction | 20% | Deflationary pressure |
| Winners per tx | 4 | Balance variance vs. complexity |
| Output exponent | 2.0 | Quadratic discourages batched splitting |
| Base fee | 100 | Baseline transaction cost |
| Selection mode | Uniform | Progressive (accept gaming risk) |

## Honest Assessment

### What the Lottery Achieves

1. **Redistributes FROM exchanges TO self-custody users** - Positive!
2. **Provides income to long-term holders** - Incentivizes holding
3. **Burns 20% of fees** - Deflationary pressure
4. **Simple implementation** - No complex state tracking

### What It Doesn't Achieve

1. **True Sybil resistance** - UTXO accumulation provides ~9x advantage
2. **Guaranteed progressive redistribution** - Depends on assumptions about UTXO distribution
3. **Wealth taxation** - Cannot tax accumulated wealth without identity

### The Fundamental Trade-off

In a privacy-preserving system without identity:

| Property | Uniform Selection | Value Selection |
|----------|-------------------|-----------------|
| Progressive | ✅ Yes | ❌ No |
| Sybil-resistant | ❌ No | ✅ Yes |
| Privacy-preserving | ✅ Yes | ✅ Yes |

**You must choose between progressivity and Sybil resistance.**

The current design chooses progressivity, accepting that sophisticated users
can game the system by accumulating UTXOs.

## Privacy-Progressivity Pareto Frontier

Extensive simulation testing explored whether we can trade a small amount of privacy
for meaningful progressivity improvements. The results reveal several Pareto-optimal
configurations:

### Hybrid Mode (α Parameter)

The `Hybrid { alpha }` mode interpolates between uniform and value-weighted:
```
weight = α + (1 - α) × normalized_value
```

| α | Gaming Ratio | Gini Δ% | Assessment |
|---|-------------|---------|------------|
| 0.0 | 0.98x | 59.9% | ★ Sybil-resistant |
| 0.1 | 2.06x | 60.3% | GOOD trade-off |
| 0.2 | 2.95x | 65.2% | GOOD trade-off |
| 0.3 | 3.84x | 69.3% | ACCEPTABLE |
| 0.4 | 4.66x | 72.5% | ACCEPTABLE |
| 0.5+ | >5x | >77% | POOR - too gameable |

**Finding**: α ≤ 0.2 provides meaningful progressivity (~65% Gini reduction)
while keeping gaming ratio under 3x.

### Age-Weighted Mode

Older UTXOs get higher lottery weight, discouraging rapid UTXO accumulation:
```rust
SelectionMode::AgeWeighted { max_age_blocks: 100_000, age_bonus: 5.0 }
```

| Age Bonus | Old/New Win Ratio | Privacy Cost |
|-----------|-------------------|--------------|
| 0x (uniform) | 0.99x | 0 bits |
| 2x | 1.97x | ~0.5 bits |
| 5x | 2.65x | ~0.5 bits |
| 10x | 3.08x | ~0.5 bits |

**Finding**: Age weighting provides ~3x preference for established coins over
freshly split UTXOs, but doesn't fully solve Sybil resistance.

### Cluster-Weighted Mode (Conditionally Progressive)

Uses existing cluster factor information—coins with lower cluster factors
(more commercial activity) get higher lottery weight:
```rust
SelectionMode::ClusterWeighted
```

**⚠️ CRITICAL CAVEAT**: ClusterWeighted progressivity depends entirely on
the correlation between wealth and cluster factor.

| Scenario | Poor Factor | Rich Factor | Poor Δ | Rich Δ | Progressive? |
|----------|-------------|-------------|--------|--------|--------------|
| Same factors | 3.0 | 3.0 | -47% | -48% | ❌ NO |
| Ideal (poor=low) | 1.5 | 5.0 | +928% | -99% | ✅ YES |
| **Adverse (poor=high)** | **5.0** | **1.5** | **-98%** | **-21%** | **❌ REGRESSIVE** |

**The problem**: In practice, new minters start with HIGH cluster factors (6.0)
and only reduce them through trade. This means:
- **New entrants (often poor)**: Start with high factors
- **Established traders (potentially wealthy)**: Have low factors from commerce

**ClusterWeighted could be REGRESSIVE** if factor-wealth correlation doesn't hold.

**Finding**: ClusterWeighted is Sybil-resistant (1.0x gaming) but:
- Progressivity depends on factor-wealth assumptions
- NOT recommended without empirical validation of factor distribution

### Pareto-Optimal Summary

| Mode | Privacy Cost | Gaming Ratio | Gini Δ% | Score |
|------|--------------|--------------|---------|-------|
| ValueWeighted | 0 bits | 1.04x | 59.4% | 56.9 |
| Hybrid(0.3) | 0 bits | 3.83x | 70.5% | 18.4 |
| ClusterWeighted | 1.5 bits | 1.00x | 40.5% | 16.2 |

**Recommendations**:
- **Maximum privacy**: Use `Hybrid { alpha: 0.3 }` - 0 bits cost, ~3.8x gaming
- **Balanced approach**: Use `ValueWeighted` - 0 bits cost, 1x gaming, 59% Gini
- **Maximum progressivity**: Use `ClusterWeighted` - 1.5 bits cost, 1x gaming, 41% Gini

### The Key Insight

ClusterWeighted works because it leverages **existing information** (cluster factor)
rather than requiring new privacy leakage. The cluster factor already reveals
something about coin origins; using it for lottery selection doesn't leak
additional bits beyond what's already exposed by the fee structure.

## Potential Mitigations (Implemented)

1. **UTXO age weighting** - Older UTXOs get more weight, discouraging rapid accumulation
   - Implemented as `SelectionMode::AgeWeighted`
   - Privacy cost: ~0.5 bits (reveals approximate age)

2. **Cluster factor weighting** - Commerce-origin coins win more often
   - Implemented as `SelectionMode::ClusterWeighted`
   - Privacy cost: ~1.5 bits (but uses existing public information)

3. **Hybrid interpolation** - Tunable balance between uniform and value-weighted
   - Implemented as `SelectionMode::Hybrid { alpha }`
   - Privacy cost: 0 bits (no new information revealed)

## Other Potential Mitigations (Not Implemented)

1. **Minimum UTXO value** - Small UTXOs from splitting don't qualify
2. **Output count limits** - Cap maximum outputs per transaction
3. **Identity layer** - Defeats privacy, not recommended

## Philosophical Notes

### What We Learned

The original insight (population statistics → progressivity) is mathematically
sound but practically limited:

1. The assumption "1 person ≈ 1 UTXO" doesn't hold in practice
2. Patient actors can accumulate UTXOs without paying splitting fees
3. Privacy and Sybil resistance are fundamentally in tension

### What This System Actually Does

This is not progressive wealth redistribution. It's closer to:

1. **Transaction fee recycling** - 80% of fees go back to random holders
2. **Anti-custodial incentive** - Redistributes from exchanges to individuals
3. **Holding reward** - Each UTXO earns lottery income over time

### Realistic Expectations

- Don't expect dramatic Gini coefficient changes
- Do expect modest redistribution from active transactors to passive holders
- Do expect redistribution from custodial services to self-custody
- Accept that sophisticated users will accumulate UTXOs for advantage

## Future Work: Provenance-Based Selection

See [Provenance-Based Selection](provenance-based-selection.md) for a novel
approach that may escape the progressivity-Sybil trade-off by leveraging
cluster tag **distributions** (entropy) rather than scalar factors.

Key insight: **Sybil splits preserve tag entropy** while requiring real
economic activity to increase it. This creates a cost structure aligned
with legitimate participation rather than gaming.

## References

- [Provenance-Based Selection](provenance-based-selection.md) - Novel entropy-weighted approach
- [Cluster Tag Decay](cluster-tag-decay.md) - How cluster factors decay through trade
- [Progressive Fees](../progressive-fees.md) - Fee curve design
- [Tokenomics](../tokenomics.md) - Overall economic model
