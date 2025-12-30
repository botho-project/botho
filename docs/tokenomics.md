# Tokenomics

Botho (BTH) uses a two-phase emission model designed for long-term sustainability: an initial distribution phase with halvings, followed by perpetual tail emission targeting stable inflation.

## Overview

| Parameter | Value |
|-----------|-------|
| Token symbol | BTH |
| Smallest unit | nanoBTH (10⁻⁹ BTH) |
| Pre-mine | None (100% mined) |
| Phase 1 supply | ~100 million BTH |
| Target block time | 20 seconds |
| Consensus | SCP (Stellar Consensus Protocol) |

## Unit System

BTH uses a 9-decimal precision system:

| Unit | nanoBTH | BTH |
|------|---------|-----|
| 1 nanoBTH | 1 | 0.000000001 |
| 1 microBTH (µBTH) | 1,000 | 0.000001 |
| 1 milliBTH (mBTH) | 1,000,000 | 0.001 |
| 1 BTH | 1,000,000,000 | 1 |

## Emission Schedule

### Phase 1: Halving Period (Years 0-10)

Block rewards halve every ~2 years, distributing approximately 100 million BTH over 10 years.

| Period | Years | Block Reward | Cumulative Supply |
|--------|-------|--------------|-------------------|
| Halving 0 | 0-2 | 50 BTH | ~52.6M BTH |
| Halving 1 | 2-4 | 25 BTH | ~78.9M BTH |
| Halving 2 | 4-6 | 12.5 BTH | ~92.0M BTH |
| Halving 3 | 6-8 | 6.25 BTH | ~98.6M BTH |
| Halving 4 | 8-10 | 3.125 BTH | ~100M BTH |

**Halving interval**: 3,153,600 blocks (~2 years at 20-second blocks)

### Phase 2: Tail Emission (Year 10+)

After Phase 1, Botho transitions to perpetual tail emission targeting **2% annual net inflation**.

**Why tail emission?**

- **Security budget**: Ensures minters always have incentive to secure the network
- **Lost coin replacement**: Compensates for coins lost to forgotten keys, deaths, etc.
- **Predictable monetary policy**: 2% is below typical fiat inflation

**How it works:**

```
net_inflation = gross_emission - fees_burned

tail_reward = (target_net_inflation + expected_fee_burns) / blocks_per_year
```

At 100M BTH supply:
- Target net emission: 2% × 100M = 2M BTH/year
- Expected fee burns: 0.5% × 100M = 0.5M BTH/year
- Gross emission needed: 2.5M BTH/year
- Blocks per year: 1,576,800
- **Tail reward: ~1.59 BTH/block**

## Fee Structure

Botho has a multi-layered fee system combining minimum fees with progressive taxation based on wealth concentration.

### Minimum Transaction Fee

| Parameter | Value |
|-----------|-------|
| Minimum fee | 400 µBTH (0.0004 BTH) |
| Fee destination | Burned (removed from supply) |
| Priority | Higher fees = faster confirmation |

All transaction fees are **burned**, creating deflationary pressure that offsets tail emission.

### Cluster-Based Progressive Fees

In addition to the minimum fee, Botho implements a novel **progressive fee system** that taxes wealth concentration without enabling Sybil attacks.

**The Problem**: Traditional wealth taxes fail in cryptocurrency because users can split holdings across unlimited addresses.

**The Solution**: Tax based on coin *ancestry*, not account identity.

#### How It Works

1. **Clusters**: Each minting reward creates a unique "cluster" identity
2. **Tag Vectors**: Every UTXO carries a sparse vector tracking what fraction of its value traces back to each cluster origin
3. **Cluster Wealth**: Total value in the system tagged to a given cluster: `W = Σ(balance × tag_weight)`
4. **Progressive Rate**: Fee rate increases with cluster wealth via sigmoid curve

```
fee_rate = sigmoid(cluster_wealth)
         = min_rate + (max_rate - min_rate) / (1 + e^(-k(W - midpoint)))
```

#### Fee Parameters

| Parameter | Value | Description |
|-----------|-------|-------------|
| Minimum rate | 0.05% | Small/diffused clusters |
| Maximum rate | 30% | Large concentrated clusters |
| Midpoint | 10M BTH | Sigmoid inflection point |
| Decay rate | 5% per hop | Tag decay per transaction |

#### Why It's Sybil-Resistant

Splitting coins across addresses doesn't reduce fees because:

- Fee rate depends on **cluster wealth**, not transaction size or account count
- All UTXOs tracing to the same minting origin pay the same rate
- The only way to reduce fees is genuine economic activity that diffuses coins

#### Tag Decay

Tags decay by ~5% per transaction hop:

- Coins that circulate widely pay lower fees over time
- Hoarded coins retain high cluster attribution → higher fees
- ~14 transaction hops to halve a tag's weight

**Economic effect**: Encourages velocity of money and discourages extreme wealth accumulation.

## Difficulty Adjustment

Botho uses adaptive difficulty adjustment with different strategies for each phase.

### Parameters

| Parameter | Value |
|-----------|-------|
| Target block time | 20 seconds |
| Minimum block time | 15 seconds |
| Maximum block time | 30 seconds |
| Adjustment interval | 1,440 blocks (~24 hours) |
| Max adjustment | ±25% per epoch |

### Phase 1: Time-Based Adjustment

Standard difficulty adjustment targeting consistent block times:

```
adjustment_ratio = expected_time / actual_time
new_difficulty = old_difficulty × clamp(ratio, 0.75, 1.25)
```

### Phase 2: Monetary-Aware Adjustment

Blends timing consistency (30%) with inflation targeting (70%):

```
timing_ratio = expected_time / actual_time
monetary_ratio = target_net_emission / actual_net_emission

blended_ratio = 0.3 × timing_ratio + 0.7 × monetary_ratio
new_difficulty = old_difficulty × clamp(blended_ratio, 0.75, 1.25)
```

This ensures:
- If net emission is too high → difficulty increases → fewer blocks → less emission
- If net emission is too low → difficulty decreases → more blocks → more emission
- Block times stay within 45-90 second bounds regardless of monetary pressure

## Transaction Constraints

| Parameter | Value |
|-----------|-------|
| Max transactions per block | 250 |
| Max inputs per transaction | 16 |
| Max outputs per transaction | 16 |
| Ring size | 7 (for private transactions) |
| Max tombstone | 20,160 blocks (~14 days) |

## Supply Projections

### Long-Term Supply Growth

| Year | Approximate Supply | Annual Inflation |
|------|-------------------|------------------|
| 0 | 0 | N/A |
| 2 | ~52.6M BTH | High (initial distribution) |
| 5 | ~85M BTH | ~15% |
| 10 | ~100M BTH | ~3% |
| 20 | ~122M BTH | 2% |
| 50 | ~180M BTH | 2% |
| 100 | ~295M BTH | 2% |

### Overflow Safety

- Phase 1 completion: 100M BTH = 10¹⁷ nanoBTH
- Maximum representable (u64): ~1.84 × 10¹⁹ nanoBTH
- Growth capacity: ~184× current supply
- At 2% annual inflation: **~260 years before overflow**

## Economic Design Philosophy

### Why No Pre-mine?

- **Fair distribution**: Everyone starts equal; early minters take on risk
- **Credibility**: No insider advantage or founder enrichment
- **Decentralization**: No concentrated holdings from day one

### Why Burn Fees?

- **Deflationary pressure**: Offsets tail emission
- **Simple economics**: No complex fee distribution mechanisms
- **Predictable**: Net inflation = gross emission - burns

### Why Progressive Cluster Fees?

- **Reduce concentration**: Wealthy clusters pay more
- **Sybil-resistant**: Can't avoid by splitting accounts
- **Encourage circulation**: Moving coins diffuses tags, reducing fees
- **Privacy-compatible**: Works with ring signatures and stealth addresses

## Comparison with Other Cryptocurrencies

| Aspect | Botho | Bitcoin | Monero | Ethereum |
|--------|-------|---------|--------|----------|
| Max supply | Unlimited (2% tail) | 21M | Unlimited (0.8% tail) | Unlimited |
| Pre-mine | None | None | None | ~72M ETH |
| Fee destination | Burned | To minters | To minters | Partially burned |
| Progressive fees | Yes (cluster-based) | No | No | No |
| Block time | 20s | 600s | 120s | 12s |

## Technical References

- [Bitcoin Halving Model](https://en.bitcoin.it/wiki/Controlled_supply) - Inspiration for Phase 1
- [Monero Tail Emission](https://www.getmonero.org/resources/moneropedia/tail-emission.html) - Inspiration for Phase 2
