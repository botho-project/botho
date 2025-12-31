# Tokenomics

Botho (BTH) uses a two-phase emission model designed for long-term sustainability: an initial distribution phase with halvings, followed by perpetual tail emission targeting stable inflation.

## Overview

| Parameter | Value |
|-----------|-------|
| Token symbol | BTH |
| Internal precision | picocredits (10⁻¹² BTH) |
| Display unit | nanoBTH (10⁻⁹ BTH) |
| Pre-mine | None (100% mined) |
| Phase 1 supply | ~100 million BTH |
| Block time | 3-40 seconds (dynamic based on load) |
| Consensus | SCP (Stellar Consensus Protocol) |

## Unit System

BTH uses a **two-tier precision system** for optimal balance between accuracy and usability:

### Internal Precision (12 decimals)

Transaction amounts use picocredits for maximum accounting precision:

| Unit | Picocredits | BTH |
|------|-------------|-----|
| 1 picocredit | 1 | 0.000000000001 |
| 1 nanoBTH | 1,000 | 0.000000001 |
| 1 microBTH (µBTH) | 1,000,000 | 0.000001 |
| 1 milliBTH (mBTH) | 1,000,000,000 | 0.001 |
| 1 BTH | 1,000,000,000,000 | 1 |

### Display Precision (9 decimals)

User interfaces and fee calculations use nanoBTH for manageable numbers:

| Unit | nanoBTH | BTH |
|------|---------|-----|
| 1 nanoBTH | 1 | 0.000000001 |
| 1 microBTH (µBTH) | 1,000 | 0.000001 |
| 1 milliBTH (mBTH) | 1,000,000 | 0.001 |
| 1 BTH | 1,000,000,000 | 1 |

### Why Two Tiers?

- **Picocredits (10¹²)**: Used by bridge contracts, transaction amounts, and internal accounting. Provides sub-nanoBTH precision for exact calculations.
- **NanoBTH (10⁹)**: Used for supply tracking, fee calculations, and user display. Fits in u64 for 100M+ BTH totals.

**Conversion**: 1 nanoBTH = 1,000 picocredits

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

Botho has a multi-layered fee system combining size-based fees, progressive wealth taxation, and dynamic congestion pricing.

### Fee Formula

```
fee = dynamic_base × tx_size × cluster_factor + memo_fees
```

| Component | Range | Description |
|-----------|-------|-------------|
| `dynamic_base` | 1-100 nanoBTH/byte | Adjusts based on network congestion |
| `tx_size` | ~4-65 KB | Transaction size in bytes |
| `cluster_factor` | 1x-6x | Progressive multiplier based on sender's cluster wealth |
| `memo_fees` | 100 nanoBTH/memo | Additional fee for encrypted memos |

All transaction fees are **burned**, creating deflationary pressure that offsets tail emission.

### Size-Based Fees

Fees are proportional to transaction size, ensuring larger transactions pay more:

| Type | Ring Size | Typical Size | Base Fee (1x cluster) |
|------|-----------|--------------|----------------------|
| Standard-Private (CLSAG) | 20 | ~4 KB | ~4,000 nanoBTH |
| PQ-Private (LION) | 11 | ~65 KB | ~65,000 nanoBTH |
| Minting | — | ~1.5 KB | 0 (no fee) |

### Dynamic Congestion Pricing

The fee base adjusts based on network load using a **cascaded control system**:

1. **Supply-side adaptation** (primary): Block timing adjusts from 40s to 3s based on transaction rate
2. **Demand-side adaptation** (secondary): When at minimum block time and blocks are >75% full, fee base increases exponentially

| Block Fullness | Fee Multiplier | Effect |
|----------------|----------------|--------|
| ≤75% | 1x | Fees at minimum |
| 80% | ~1.5x | Gentle pressure |
| 90% | ~3.3x | Moderate pressure |
| 100% | ~7.4x | Strong back-pressure |

This ensures fees stay low during normal operation while providing strong congestion control under extreme load.

### Cluster-Based Progressive Fees

Botho implements a novel **progressive fee system** that taxes wealth concentration without enabling Sybil attacks.

**The Problem**: Traditional wealth taxes fail in cryptocurrency because users can split holdings across unlimited addresses.

**The Solution**: Tax based on coin *ancestry*, not account identity.

#### How It Works

1. **Clusters**: Each minting reward creates a unique "cluster" identity
2. **Tag Vectors**: Every UTXO carries a sparse vector tracking what fraction of its value traces back to each cluster origin
3. **Cluster Wealth**: Total value in the system tagged to a given cluster: `W = Σ(balance × tag_weight)`
4. **Progressive Multiplier**: Fee multiplier increases with cluster wealth via sigmoid curve

```
cluster_factor = 1 + 5 × sigmoid((W - midpoint) / steepness)
```

#### Fee Parameters

| Parameter | Value | Description |
|-----------|-------|-------------|
| Minimum multiplier | 1x | Small/diffused clusters |
| Maximum multiplier | 6x | Large concentrated clusters |
| Midpoint | 10M BTH | Sigmoid inflection point |
| Decay rate | 5% per hop | Tag decay per transaction |

#### Why It's Sybil-Resistant

Splitting coins across addresses doesn't reduce fees because:

- Fee multiplier depends on **cluster wealth**, not transaction size or account count
- All UTXOs tracing to the same minting origin pay the same rate
- The only way to reduce fees is genuine economic activity that diffuses coins

#### Tag Decay

Tags decay by ~5% per transaction hop:

- Coins that circulate widely pay lower fees over time
- Hoarded coins retain high cluster attribution → higher fees
- ~14 transaction hops to halve a tag's weight

**Economic effect**: Encourages velocity of money and discourages extreme wealth accumulation.

## Block Timing

Botho uses **dynamic block timing** that adapts to network load, providing faster finality under high load while conserving resources when idle.

### Dynamic Timing Levels

| Transaction Rate | Block Time | Capacity |
|------------------|------------|----------|
| 20+ tx/s | 3 seconds | ~600 tx/min |
| 5+ tx/s | 5 seconds | ~300 tx/min |
| 1+ tx/s | 10 seconds | ~100 tx/min |
| 0.2+ tx/s | 20 seconds | ~50 tx/min |
| <0.2 tx/s | 40 seconds | ~25 tx/min |

This provides **13x capacity scaling** between idle and high-load conditions without protocol changes.

### Why Dynamic Timing?

- **Efficiency**: Slow blocks when idle reduce storage overhead
- **Responsiveness**: Fast blocks under load improve user experience
- **Congestion control**: Combined with dynamic fees, manages demand spikes

## Difficulty Adjustment

Botho uses **transaction-based difficulty adjustment** that targets monetary policy goals rather than block timing (which is handled by dynamic timing above).

### Parameters

| Parameter | Value |
|-----------|-------|
| Adjustment epoch | 10,000 transactions |
| Min difficulty | 1,000 |
| Max adjustment | ±25% per epoch |

### Phase 1: Emission-Tracking Adjustment

During the halving period, difficulty adjusts to maintain the target emission schedule:

```
epoch_target_emission = halving_reward × target_blocks_per_epoch
adjustment_ratio = epoch_target_emission / actual_epoch_emission
new_difficulty = old_difficulty × clamp(ratio, 0.75, 1.25)
```

### Phase 2: Monetary-Aware Adjustment

After Phase 1, difficulty targets 2% net inflation by balancing gross emission against fee burns:

```
target_gross = target_net_inflation + expected_fee_burns
adjustment_ratio = target_gross / actual_gross
new_difficulty = old_difficulty × clamp(ratio, 0.75, 1.25)
```

This ensures:
- If net emission is too high → difficulty increases → fewer minting rewards
- If net emission is too low → difficulty decreases → more minting rewards
- Fee burn variations are automatically compensated

## Transaction Constraints

| Parameter | Standard-Private | PQ-Private |
|-----------|------------------|------------|
| Max transactions per block | 100 | 100 |
| Max inputs per transaction | 16 | 8 |
| Max outputs per transaction | 16 | 16 |
| Ring size | 20 (CLSAG) | 11 (LION) |
| Max transaction size | 100 KB | 512 KB |

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
