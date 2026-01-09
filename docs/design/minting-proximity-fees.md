# Minting Proximity Fees: Design Rationale

## Status

**Active** - Core economic mechanism

## Overview

Botho implements a **minting proximity fee system** that makes freshly-minted coins more expensive to transact until they circulate through the economy. This addresses the "early adopter problem" where early miners could capture disproportionate value by hoarding coins minted when the network was young.

### What This System Is

- **Early adopter dilution**: Fresh coins from concentrated minting origins pay higher fees
- **Circulation incentive**: Fees naturally decrease as coins participate in real commerce
- **Sybil-resistant**: Splitting coins doesn't reduce their minting proximity
- **Privacy-preserving**: Tracks coin provenance, not user identity

### What This System Is NOT

- **NOT wealth taxation**: We don't know or track how much any person holds
- **NOT identity-based**: No accounts, no balances, no user tracking
- **NOT a wealth detector**: A user who buys 1M BTH from diverse sources faces normal fees

## The Early Adopter Problem

In traditional cryptocurrencies, early miners enjoy compounding advantages:

```
Year 1: Miner accumulates 100,000 coins (easy difficulty)
Year 5: Those coins now represent 10% of supply
Year 10: Network matures, those coins = concentrated economic power

Problem: Early minting advantage compounds indefinitely
```

### Why This Matters

Early adopter concentration creates:
1. **Governance risk**: Concentrated holders can influence protocol development
2. **Market manipulation**: Large holders can move markets
3. **Inequality lock-in**: Late adopters face permanent disadvantage
4. **Velocity reduction**: Hoarded coins don't participate in commerce

## The Solution: Proximity-Based Fees

Coins "remember" their minting origin through cluster tags. Fees are based on **how concentrated** that origin was and **how much** the coins have circulated since.

### Core Mechanism

```
Minting Event
    │
    ▼
┌─────────────────────────────────┐
│  Fresh Coins                    │
│  - cluster_id: unique to miner  │
│  - source_wealth: minting amount│
│  - fee_rate: HIGH (15%)         │
└─────────────────────────────────┘
    │
    │  Real Commerce (mixing with diverse sources)
    ▼
┌─────────────────────────────────┐
│  Circulated Coins               │
│  - cluster tags: blended        │
│  - source_wealth: averaged down │
│  - fee_rate: LOW (1-2%)         │
└─────────────────────────────────┘
```

### Why Splitting Doesn't Help

```
Attempt: Split 1M coins into 1000 × 1K coins
Result:  Each UTXO still has source_wealth = 1M (unchanged)
         All transactions still pay 15% rate
         Attack defeated
```

The fee is based on **origin**, not **denomination**. Splitting changes the value per UTXO but not the provenance.

### Why Sybil Shuffling Doesn't Help

```
Attempt: Transfer to 100 fake accounts
Result:  Each recipient inherits source_wealth from sender
         All sybil accounts still pay 15% rate
         Attack defeated
```

Provenance transfers to recipients. Creating shell accounts doesn't launder away minting proximity.

### What DOES Reduce Fees

**Genuine commerce** reduces fees because:

1. **Tag decay**: Each transaction reduces cluster attribution by ~5%
2. **Mixing**: Combining coins from diverse sources averages down source_wealth
3. **Time**: Age-gated decay prevents rapid wash trading

After 20+ hops through legitimate merchants, coins approach baseline fee rates.

## Conceptual Distinction: Minting vs Wealth

This is a crucial distinction that shapes the entire design:

| Concept | What It Tracks | Example |
|---------|----------------|---------|
| **Minting Proximity** | Where coins were originally created | "These coins came from a miner who accumulated 1M BTH" |
| **Wealth Concentration** | How much a person currently holds | "This person owns 1M BTH across various sources" |

### What We Track: Minting Proximity

- Coins carry provenance information about their origin
- A whale miner's coins have high source_wealth (from concentrated minting)
- Fees apply based on this origin, regardless of current holder

### What We DON'T Track: Wealth Concentration

- We have no concept of "accounts" or "balances"
- A person who buys 1M BTH from diverse sources faces normal fees
- We cannot identify "wealthy users" - only "coins with concentrated origins"

### Implications

| Scenario | Minting Proximity Fee | Would Wealth Tax Apply? |
|----------|----------------------|------------------------|
| Whale miner spends own coins | HIGH | HIGH |
| Rich person buys diverse coins | LOW | HIGH |
| Poor person receives from whale | HIGH (decays) | LOW |
| Exchange consolidates deposits | MIXED | HIGH |

The system treats these differently because **we can only see coin provenance, not user holdings**.

## Lottery Redistribution: Rewarding Circulation

The lottery mechanism complements proximity fees by rewarding coins that have circulated widely.

### Selection Weighting

Coins with high **cluster entropy** (many diverse sources blended together) receive higher lottery weight:

```
Fresh minted coin:      entropy ≈ 0 bits   → low lottery weight
Self-transferred coin:  entropy ≈ 0 bits   → low lottery weight (no new sources)
Commerce-cycled coin:   entropy ≈ 2-3 bits → high lottery weight
```

### What This Achieves

- **Rewards participation**: Coins that participate in real commerce get redistribution
- **Sybil resistant**: Self-transfers don't increase entropy (no new sources mixed)
- **Not poverty-based**: We're not identifying "poor users" - we're identifying "well-circulated coins"

### The Conceptual Frame

Think of it as:
- **NOT**: "Redistribute from rich to poor" (we can't identify rich/poor)
- **YES**: "Redistribute from stagnant to circulating" (we CAN identify circulation)

## Privacy Model

### What's Visible

- Cluster tags (for fee calculation)
- UTXO creation block (for age-gated decay)
- Transaction structure

### What's Hidden

- Recipient identity (stealth addresses)
- Sender identity (ring signatures)
- User holdings (no account model)

### No Wealth-Conditional Privacy

Since we're tracking **minting proximity**, not **wealth**, there's no justification for reducing privacy based on source_wealth. A high source_wealth coin might be held by:

- The original miner (wealthy)
- Someone who just received it (any wealth level)
- A merchant who accepted it (any wealth level)

Reducing privacy for high source_wealth coins would harm innocent recipients, not just "wealthy users."

## Economic Effects

### Who Pays Higher Fees

| Scenario | Fee Level | Reason |
|----------|-----------|--------|
| Early miner spending hoarded coins | HIGH | Concentrated minting origin |
| Recent miner spending new coins | HIGH | Fresh minting origin |
| Person who bought from diverse sources | LOW | Diverse, averaged provenance |
| Anyone after 20+ commerce hops | LOW | Tags decayed through circulation |

### Incentive Alignment

The system creates these incentives:

1. **Circulate, don't hoard**: Holding coins doesn't reduce fees; commerce does
2. **Participate in economy**: Fees decrease through genuine economic activity
3. **Mining isn't special**: Fresh coins from any miner face the same fee pressure

### What This Doesn't Do

- Identify wealthy individuals
- Tax accumulated holdings
- Prevent wealth accumulation through diverse purchases
- Reduce privacy for any users

## Effectiveness Analysis: Why There's No Escape

A critical property of the minting proximity system is that **tags persist until commerce, not until time passes**. This creates unavoidable pressure on concentrated minting origins.

### No Escape Through Holding

Unlike time-based taxes or demurrage, holding doesn't decay tags:

```
Year 0:  Early miner accumulates 1M BTH (source_wealth = 1M)
Year 10: Still holding → source_wealth = 1M (unchanged)
Year 20: Still holding → source_wealth = 1M (unchanged)
Year 50: Finally moves coins → STILL pays 15% fee
```

**There is no patience escape.** Tags only decay through genuine commerce that mixes with diverse sources. A Satoshi-like figure holding coins for decades would still face full fees when those coins eventually move.

### No Escape Through Borrowing

Using concentrated-origin coins as collateral doesn't escape the mechanism:

```
Whale's 1M BTH (source_wealth = 1M)
         │
         ▼
    Posts as collateral for loan
         │
         ▼
    Receives fiat/stablecoins
         │
         ▼
    Lives off loan proceeds for years
         │
         ▼
    Eventually: loan repaid OR liquidated
         │
         ▼
    Coins move → STILL 15% fee on movement
```

The tags follow the coins, not the debt structure. Borrowing lets you defer the fee but not avoid it. When the collateral is eventually released or liquidated, the high source_wealth persists.

### Exchange Mixing Has Real Costs

Exchanges provide a "tag laundering" service by mixing deposits from many users, but this comes at a cost:

```
Whale deposits 1M BTH to exchange
    → Pays 15% deposit fee (150K BTH extracted)

Exchange mixes with other deposits (9M BTH from diverse sources)
    → Creates withdrawal UTXOs with averaged source_wealth
    → (1M × 1M + 9M × 10K) / 10M ≈ 109K average

Whale withdraws "clean" coins
    → But already paid 150K BTH to access mixing
    → Real wealth transfer occurred
```

The exchange route is not free. The mechanism extracts value during the laundering process.

### Market Discount: Immediate Wealth Reduction

Rational market participants would price in expected fee costs, creating an **immediate discount** on high source_wealth coins:

| UTXO Characteristic | Market Implication |
|---------------------|-------------------|
| source_wealth = 10K (circulated) | Full market value |
| source_wealth = 1M (concentrated) | Discounted by expected fee costs |

**Example valuation:**

A buyer evaluating 1,000 BTH with high source_wealth:
- Expected fee on first move: ~150 BTH (15%)
- Expected fees on subsequent moves: ~14% premium each
- Rational discount: Present value of fee differential

This means concentrated minting wealth is **effectively reduced immediately**, not just when transacting. The market prices in the encumbrance.

### The "Satoshi Problem" in Botho

If Bitcoin had launched with minting proximity fees:

| Scenario | Bitcoin (Actual) | Botho-style Bitcoin |
|----------|------------------|---------------------|
| Satoshi holds 1M BTC | Worth ~$60B, fully liquid | Worth ~$60B nominal, ~15% effective discount |
| Satoshi moves to exchange | 0% fee | 15% fee (~$9B at current prices) |
| Satoshi spends directly | Normal fees | 15% per transaction |
| Satoshi borrows against | Full collateral value | Full collateral, but fees on any movement |
| Satoshi sells OTC | Full price | Buyer demands discount for expected fees |

**There is no free exit for concentrated minting wealth.**

## Honest Assessment: What This Addresses

### Bitcoin Concentration Sources and Botho's Impact

| Concentration Source | % of BTC Concentration | Botho Impact |
|---------------------|------------------------|--------------|
| Early miner accumulation (Satoshi-like) | ~5% of supply | ✅ **Strong** - 15% fee on any movement |
| Mining pool concentration | Ongoing | ✅ **Strong** - Pool withdrawals face high fees |
| Early buyer advantage (bought at $0.01) | Significant | ⚠️ **None** - Bought from diverse sources |
| HODL culture | Behavioral | ✅ **Moderate** - Holding doesn't escape, but doesn't cost either |
| Exchange concentration | ~10% of supply | ⚠️ **Mixed** - Exchanges mix provenance |
| Institutional buying (ETFs, corps) | Growing | ⚠️ **None** - Buy from diverse sources |

### What Gets Addressed

1. **Early miner wealth**: The Satoshi problem is directly addressed. Large minting accumulations face permanent fee encumbrance.

2. **Mining centralization**: Pool operators face higher fees when extracting rewards, creating modest decentralization pressure.

3. **Velocity incentive**: Coins must circulate through real commerce to reduce fees, encouraging economic participation.

4. **No patience exploit**: Unlike many economic mechanisms, this one doesn't reward patient waiting.

### What Doesn't Get Addressed

1. **Secondary market accumulation**: A wealthy buyer acquiring coins from diverse sources faces normal fees. This is the primary gap.

2. **Price appreciation wealth**: Someone who bought 1,000 BTH at $0.01 and holds until $10,000 gains 1,000,000× regardless of fee structure.

3. **True wealth taxation**: Without identity, we cannot tax based on total holdings.

### Net Gini Impact Assessment

**Sources addressed by minting proximity:**
- Early miner concentration (significant in early cryptocurrency history)
- Mining pool concentration (ongoing issue)

**Sources not addressed:**
- Secondary market accumulation
- Price appreciation gains

**Realistic expectation**: Meaningful reduction in miner-originated inequality, minimal impact on buyer-originated inequality. Given that early miner concentration (Satoshi, early GPU miners, etc.) represents a significant portion of cryptocurrency wealth concentration, this is more impactful than it might initially appear.

### Comparison with Alternative Mechanisms

| Mechanism | Miner Concentration | Buyer Concentration | Privacy | Feasibility |
|-----------|--------------------|--------------------|---------|-------------|
| **Minting proximity fees** | ✅ Strong | ❌ None | ✅ Preserved | ✅ Implemented |
| Demurrage (holding tax) | ✅ Strong | ✅ Strong | ✅ Preserved | ⚠️ Controversial |
| Identity-based wealth tax | ✅ Strong | ✅ Strong | ❌ Broken | ❌ Defeats purpose |
| No mechanism (Bitcoin) | ❌ None | ❌ None | ✅ Preserved | ✅ Default |

Minting proximity fees represent the **strongest mechanism possible** for addressing miner concentration while preserving privacy. The secondary market gap is a fundamental limitation of privacy-preserving systems, not a design flaw.

## Terminology Update

Throughout the codebase, we're updating terminology for clarity:

| Old Term | New Term | Reason |
|----------|----------|--------|
| "Wealth taxation" | "Minting proximity fees" | Not tracking wealth |
| "Tax wealthy users" | "Fee on concentrated minting origins" | Not user-based |
| "Whale vs Poor" | "Fresh vs Circulated" | Origin-based, not holder-based |
| "Wealth-conditional privacy" | (Removed) | Not justified by design |
| "Redistribute to poor" | "Reward circulation" | We identify circulation, not poverty |

## Summary

Botho's minting proximity fee system:

1. **Addresses early adopter capture** by making fresh-minted coins expensive until circulated
2. **Preserves privacy** by tracking coin provenance, not user identity
3. **Resists Sybil attacks** because provenance persists through splits and transfers
4. **Incentivizes commerce** because fees naturally decrease through legitimate economic activity
5. **Does NOT identify wealth** - we see coin origins, not user holdings

This is a coherent mechanism for diluting early minting advantage, not a general wealth tax.

## References

- [Progressive Fees](../concepts/progressive-fees.md) - Fee curve implementation
- [Cluster Tag Decay](cluster-tag-decay.md) - How provenance decays over time
- [Lottery Redistribution](lottery-redistribution.md) - How fees are redistributed
- [Privacy](../concepts/privacy.md) - Privacy model details
