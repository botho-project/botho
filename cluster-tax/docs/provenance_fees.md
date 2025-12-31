# Provenance-Based Progressive Fees

## The Problem

We want progressive transaction fees: wealthy users pay higher rates than poor users.

**Naive approach**: Fee rate based on transaction amount.
```
fee = amount × rate(amount)
```

**The attack**: Programmatic splitting defeats this instantly.
```
Whale has 1,000,000 coins.
Goal: Transfer to merchant.

Without splitting:
  1 tx of 1,000,000 at 10% rate = 100,000 fee

With splitting:
  1,000 txs of 1,000 at 1% rate = 10,000 total fee

Savings: 90% fee reduction via trivial split operation.
```

This is fatal. Any amount-based progressive fee can be gamed by splitting.

## The Solution: Provenance Tags

Coins remember where they came from. The fee is based on **source wealth**, not current denomination.

### Core Data Structure

Each UTXO carries:
```
UTXO {
    value: u64,           // Current amount
    source_wealth: u64,   // Wealth of original minter (persists through splits)
}
```

### Key Property: Splits Don't Help

```
Before split:
  UTXO A: value=1,000,000, source_wealth=1,000,000

After split into 1000 pieces:
  UTXO A1: value=1,000, source_wealth=1,000,000
  UTXO A2: value=1,000, source_wealth=1,000,000
  ...
  UTXO A1000: value=1,000, source_wealth=1,000,000

Fee calculation:
  rate = f(source_wealth) = f(1,000,000) = HIGH

Each 1,000-coin piece STILL pays the high rate because
source_wealth is unchanged by splitting.
```

**This is the entire point.** The provenance tag defeats splitting attacks.

## Detailed Mechanics

### 1. Minting (Coinbase/Initial Distribution)

When coins are minted, `source_wealth` equals the minter's total balance:
```
Agent with balance B mines reward R:
  New UTXO: value=R, source_wealth=B+R
```

This tags new coins with the wealth level of their origin.

### 2. Splitting (Self-Transfer)

When splitting a UTXO into pieces:
```
Input:  UTXO with value=V, source_wealth=S
Outputs: Multiple UTXOs, each with source_wealth=S (unchanged)
```

**source_wealth persists.** Splitting doesn't reduce it.

### 3. Transfer (Payment)

When paying someone else:
```
Inputs: UTXOs with various (value_i, source_wealth_i)
Outputs: Payment UTXO + Change UTXO

Blended source_wealth = Σ(value_i × source_wealth_i) / Σ(value_i)

Both outputs inherit the blended source_wealth.
```

**source_wealth blends** as a value-weighted average. This handles:
- Combining multiple inputs
- Mixing coins from different sources

### 4. Fee Calculation

```
effective_wealth = source_wealth  // Or source_wealth × tag_weight if using decay
fee_rate = lerp(r_min, r_max, effective_wealth / max_wealth)
fee = amount × fee_rate
```

The fee rate depends on **where the coins came from**, not their current size.

## Why This Works

### Attack 1: Splitting
```
Whale splits 1M into 1000 × 1K UTXOs.
Each UTXO: source_wealth = 1M (unchanged)
Each tx pays: rate(1M) = high
Total fees: same as unsplit
Result: Attack defeated.
```

### Attack 2: Sybil Shuffle
```
Whale creates 100 sybil accounts.
Transfers 10K to each sybil.
Each sybil's UTXO: source_wealth = 1M (inherited from whale)
Sybils pay: rate(1M) = high
Result: Attack defeated.
```

### Attack 3: Mixing with Poor Coins
```
Whale has UTXO: value=100K, source_wealth=1M
Poor person has UTXO: value=1K, source_wealth=1K

They combine into one transaction:
  Total value: 101K
  Blended source_wealth: (100K×1M + 1K×1K) / 101K ≈ 990,109

Result: source_wealth slightly diluted, but still ~99% of whale level.
         Attack provides minimal benefit.
```

### Legitimate Commerce: Natural Decay

Over many legitimate transactions, source_wealth naturally averages out:
```
Whale → Merchant → Supplier → Worker → Retailer → ...

Each hop blends with the recipient's existing coins.
After many hops through diverse parties, source_wealth approaches population average.
```

This is correct behavior: coins that have circulated widely through the economy
should pay lower fees than fresh coins from concentrated wealth.

## Optional: Tag Weight Decay

For faster normalization, add explicit decay:
```
UTXO {
    value: u64,
    source_wealth: u64,   // Fixed per source
    tag_weight: u32,      // Decays with transfers (0 to 1,000,000 scale)
}

effective_wealth = source_wealth × (tag_weight / 1,000,000)
```

On each transfer:
```
// Partial decay: only decay proportional to value transferred
transfer_fraction = amount_out / total_input_value
decay_applied = base_decay × transfer_fraction
new_tag_weight = old_tag_weight × (1 - decay_applied)
```

**Partial decay** prevents frequent transactors from losing their tags
when just moving change around.

## Correctness Checklist

| Property | Required Behavior | Verified? |
|----------|-------------------|-----------|
| Split resistance | source_wealth unchanged by splitting | |
| Sybil resistance | source_wealth inherited by sybil recipients | |
| Blend on combine | Multiple inputs → weighted average source_wealth | |
| Decay over time | Legitimate commerce reduces effective_wealth | |
| Whale pays more | High source_wealth → high fee rate | |
| Poor pays less | Low source_wealth → low fee rate | |

## Open Questions

1. **Privacy**: Does source_wealth leak information about transaction history?
   - If public: observers can track coin provenance
   - If private: how do validators verify fee correctness?

2. **Initial distribution**: How is source_wealth set for pre-mine or ICO coins?

3. **Mining rewards**: Should mining rewards have source_wealth = 0 (fresh money)
   or source_wealth = miner's total balance?

4. **Edge cases**: What happens with very old coins that have decayed to near-zero tag_weight?
