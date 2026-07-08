# Tokenomics

Botho (BTH) uses a two-phase emission model designed for long-term sustainability: an initial distribution phase with halvings, followed by perpetual tail emission targeting stable inflation.

## Overview

| Parameter | Value |
|-----------|-------|
| Token symbol | BTH |
| Base unit | picocredits (10⁻¹² BTH) |
| Display unit | BTH (formatted from picocredits at the UI edge) |
| Pre-mine | None (100% mined) |
| Phase 1 supply | ~611 million BTH (~5 years of halvings) |
| Block time | 5-40 seconds (dynamic based on load); 5s baseline for monetary calculations |
| Consensus | SCP (Stellar Consensus Protocol) |

## Unit System

BTH uses a **single base unit** — the **picocredit** (10⁻¹² BTH). Every amount in
the protocol (transaction values, fees, emission, and monetary policy) is
denominated in picocredits. Amounts are formatted into BTH, or convenient
multiples like milliBTH and microBTH, only at the user-interface edge.

| Unit | Picocredits | BTH |
|------|-------------|-----|
| 1 picocredit | 1 | 0.000000000001 |
| 1 microBTH (µBTH) | 1,000,000 | 0.000001 |
| 1 milliBTH (mBTH) | 1,000,000,000 | 0.001 |
| 1 BTH | 1,000,000,000,000 | 1 |

Picocredits provide 12 decimals of precision. Aggregate supply exceeds u64 when
expressed in picocredits (100M BTH = 10²⁰ picocredits), so supply totals are
tracked in **u128**; individual transaction amounts fit comfortably in u64.

## Emission Schedule

### Phase 1: Halving Period (Years 0-5)

Block rewards halve every ~1 year, distributing approximately 611 million BTH over ~5 years.

| Period | Years | Block Reward | Cumulative Supply |
|--------|-------|--------------|-------------------|
| Halving 0 | 0-1 | 50 BTH | ~315.4M BTH |
| Halving 1 | 1-2 | 25 BTH | ~473.0M BTH |
| Halving 2 | 2-3 | 12.5 BTH | ~551.9M BTH |
| Halving 3 | 3-4 | 6.25 BTH | ~591.3M BTH |
| Halving 4 | 4-5 | 3.125 BTH | ~611.0M BTH |

**Halving interval**: 6,307,200 blocks (~1 year at the 5-second monetary baseline; proportionally longer at slower actual block times)

### Phase 2: Tail Emission (Year 5+)

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

Here `fees_burned` is only the **20% burn share** of fees that is actually destroyed (audit cycle 6, M4). The 80% lottery-redistributed share and the emission share routed into the lottery pool both remain in circulating supply, so they do **not** count toward `fees_burned`.

At the ~611M BTH tail-onset supply:
- Target net emission: 2% × 611M = ~12.2M BTH/year
- Expected fee burns (20% share only): ~0.5% × 611M = ~3.1M BTH/year
- Gross emission needed: ~15.3M BTH/year
- Blocks per year (5s baseline): 6,307,200
- **Tail reward: supply-dependent, ~2.4 BTH/block gross (~1.9 BTH/block net) at this supply**

The tail reward is **not a fixed constant**: it is recomputed from circulating supply each block to target 2% net annual inflation, so it grows as supply grows. (The exact tail-reward figure also depends on the assumed block time; cross-source numeric reconciliation is tracked separately in issue #321.)

### Emission Routing into the Lottery Pool

Not all of the block reward goes to the miner's coinbase. A height-scheduled fraction of each block reward is routed into the redistribution lottery pool (`MonetaryPolicy::lottery_emission_bps` / `lottery_emission_share` in `botho/src/monetary.rs` and `cluster-tax/src/monetary.rs`):

- **Bootstrap epoch (epoch 0)**: 0% routed — miners keep the full reward while mining seeds the network and the only lottery-eligible UTXOs would be miner coinbases.
- **Each subsequent halving epoch**: +1,000 bps (+10%) routed to the pool.
- **Cap**: 5,000 bps (50%) — at least half of emission is always preserved as the mining security budget.

The miner receives `reward − emission_share`; the routed share joins the fee pool share and any carryover as the amount available for that block's lottery draw (capped at one block reward). Cluster demurrage activates on the same boundary: zero during epoch 0, then 2%/year at maximum cluster factor (`demurrage_rate_bps`).

## Fee Structure

Botho has a multi-layered fee system combining size-based fees, progressive wealth taxation, and dynamic congestion pricing.

### Fee Formula

```
fee = dynamic_base × tx_size × cluster_factor + memo_fees
```

| Component | Range | Description |
|-----------|-------|-------------|
| `dynamic_base` | 1-100 picocredits/byte | Adjusts based on network congestion |
| `tx_size` | ~4-65 KB | Transaction size in bytes |
| `cluster_factor` | 1x-6x | Progressive multiplier based on sender's cluster wealth |
| `memo_fees` | 100 picocredits/memo | Additional fee for encrypted memos |

#### Fee Destination: Redistribution Lottery + Burn (80/20)

Transaction fees are **not** all burned. Each block, collected fees are split deterministically (`LotteryFeeConfig` in `botho/src/consensus/lottery.rs`, 800 permille):

- **80% → redistribution lottery pool**, paid back out to randomly selected UTXO holders. The draw is cluster-tilted, favoring smaller, well-circulated holders over concentrated clusters.
- **20% → burned**, providing deflationary pressure that partially offsets tail emission.

The lottery pool is consensus state. A per-block payout cap (one block reward) plus carryover make seed-grinding unprofitable: undistributed pool funds carry over to future blocks rather than being destroyed. The burn share (the 20%) is the **only** portion of fees subtracted from supply; the redistributed 80% stays in circulation as new lottery-payout UTXOs.

In addition, **cluster demurrage** levies a small spend-time holding charge on coins in concentrated clusters (factor-1 / well-circulated coins pay zero). The charge is added to a transaction's minimum fee and flows through the same 80/20 split into the lottery pool — so idle-wealth charges are redistributed, not burned.

> **See also**: [Cluster-Tilted Redistribution](../design/cluster-tilted-redistribution.md) (the validated mechanism), [Lottery-Based Fee Redistribution](../design/lottery-redistribution.md) (background analysis), and [Entropy-Weighted Decay](../design/entropy-weighted-decay.md) (tag-decay hardening).

### Size-Based Fees

Fees are proportional to transaction size, ensuring larger transactions pay more:

| Type | Ring Size | Typical Size | Base Fee (1x cluster) |
|------|-----------|--------------|----------------------|
| Private (CLSAG) | 20 | ~4 KB | ~4,000 picocredits |
| Minting | — | ~1.5 KB | 0 (no fee) |

### Dynamic Congestion Pricing

The fee base adjusts based on network load using a **cascaded control system**:

1. **Supply-side adaptation** (primary): Block timing adjusts from 40s (idle) down to the 5s baseline (high load) based on transaction rate
2. **Demand-side adaptation** (secondary): When at minimum block time and blocks are >75% full, fee base increases exponentially

| Block Fullness | Fee Multiplier | Effect |
|----------------|----------------|--------|
| ≤75% | 1x | Fees at minimum |
| 80% | ~1.5x | Gentle pressure |
| 90% | ~3.3x | Moderate pressure |
| 100% | ~7.4x | Strong back-pressure |

This ensures fees stay low during normal operation while providing strong congestion control under extreme load.

### Cluster-Based Progressive Fees

Botho implements a novel **provenance-based progressive fee system** that taxes wealth concentration without enabling Sybil attacks.

![Progressive Fee System](images/cluster-tax/system_overview.png)

**The Problem**: Traditional wealth taxes fail in cryptocurrency because users can split holdings across unlimited addresses.

**The Solution**: Tax based on coin *ancestry* (source_wealth), not account identity. Splitting doesn't help because provenance tags persist.

![Split Resistance](images/cluster-tax/split_resistance.png)

#### How It Works

1. **Source Wealth**: Every UTXO tracks the wealth of its original minter
2. **Persistence**: Splitting doesn't change source_wealth—all pieces retain the original tag
3. **Blending**: Combining UTXOs creates a value-weighted average source_wealth
4. **Progressive Rate**: Fee rate increases with source_wealth via 3-segment curve

![Fee Curves](images/cluster-tax/fee_curves_comparison.png)

#### Fee Parameters

| Parameter | Value | Description |
|-----------|-------|-------------|
| Poor segment | 0-15% of max | 1% flat rate |
| Middle segment | 15-70% of max | 2% to 10% linear |
| Rich segment | 70%+ of max | 15% flat rate |
| Decay rate | 5% per eligible hop | Tag decay when UTXO is old enough |
| Min UTXO age | 720 blocks (~2 hours) | UTXOs must be this old before decay applies |

#### Age-Based Decay

Not every transaction triggers decay. To prevent **wash trading attacks** (rapid self-transfers to reduce fees), Botho uses **age-based decay gating**:

- **Age requirement**: UTXOs must be at least 720 blocks (~2 hours) old before decay applies
- **Rate limit**: This naturally caps decay to ~12 eligible transactions per day
- **Privacy preserved**: Uses only the UTXO creation block (already public), no extra metadata

This means a wash trader executing 100 rapid self-transfers gets 0% decay (all outputs too young), while legitimate commerce over time allows natural tag diffusion.

#### Simulation Results

![Gini Reduction](images/cluster-tax/gini_reduction_comparison.png)

The 3-segment model achieves **-0.2399 Gini reduction** (0.3% better than sigmoid) with **12.4% burn rate**.

#### Why It's Sybil-Resistant

Splitting coins across addresses doesn't reduce fees because:

- Fee rate depends on **source_wealth**, not transaction size or account count
- All UTXOs from the same origin retain the same source_wealth tag
- The only way to reduce fees is genuine economic activity that diffuses coins

#### Natural Decay Through Commerce

![Provenance Decay](images/cluster-tax/provenance_decay.png)

Tags decay through legitimate commerce:

- Coins that circulate widely pay lower fees over time
- Hoarded coins retain high source_wealth → higher fees
- ~10-20 transaction hops through merchants reduces source_wealth by 90%+
- Each hop must meet the 2-hour age requirement to trigger decay

**Maximum decay rates** (due to age-based gating):
- Per day: ~46% (12 eligible decays × 5% each)
- Per week: ~97% (84 eligible decays)
- Holding without transacting: 0% decay (requires spending)

**Economic effect**: Encourages velocity of money and discourages extreme wealth accumulation.

> **See also**: [Progressive Fees](progressive-fees.md) for detailed analysis, attack resistance proofs, and implementation details.

## Block Timing

Botho uses **dynamic block timing** that adapts to network load, providing faster finality under high load while conserving resources when idle.

### Dynamic Timing Levels

| Transaction Rate | Block Time | Capacity |
|------------------|------------|----------|
| 5+ tx/s | 5 seconds | ~300 tx/min |
| 1+ tx/s | 10 seconds | ~100 tx/min |
| 0.2+ tx/s | 20 seconds | ~50 tx/min |
| <0.2 tx/s | 40 seconds | ~25 tx/min |

The 5-second floor is also the baseline assumed by all monetary calculations (see `mainnet_policy` in `botho/src/monetary.rs`). This provides **8x capacity scaling** between the 40s idle interval and the 5s high-load floor without protocol changes.

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

| Parameter | Private | Minting |
|-----------|---------|---------|
| Max transactions per block | 100 | 1 |
| Max inputs per transaction | 16 | 0 |
| Max outputs per transaction | 16 | 16 |
| Ring size | 20 (CLSAG) | — |
| Max transaction size | 100 KB | 10 KB |

## Supply Projections

### Long-Term Supply Growth

| Year | Approximate Supply | Annual Inflation |
|------|-------------------|------------------|
| 0 | 0 | N/A |
| 1 | ~315.4M BTH | High (initial distribution) |
| 2 | ~473.0M BTH | High (initial distribution) |
| 5 | ~611M BTH (tail onset) | ~2% from here |
| 10 | ~674M BTH | 2% |
| 20 | ~822M BTH | 2% |
| 50 | ~1.49B BTH | 2% |
| 100 | ~4.0B BTH | 2% |

### Overflow Safety

- Phase 1 completion (~year 5): ~611M BTH = ~6.11 × 10²⁰ picocredits
- Maximum representable (u128): ~3.4 × 10³⁸ picocredits
- Growth capacity: ~18 orders of magnitude above tail-onset supply
- At 2% annual inflation: **no practical overflow risk** — all monetary accounting uses picocredits in u128, which retains ~16 orders of magnitude of headroom above the entire projected supply

## Economic Design Philosophy

### Why No Pre-mine?

- **Fair distribution**: Everyone starts equal; early minters take on risk
- **Credibility**: No insider advantage or founder enrichment
- **Decentralization**: No concentrated holdings from day one

### Why Redistribute Most Fees (and Burn the Rest)?

- **Structural Gini reduction**: Returning 80% of fees (plus demurrage charges and a height-scheduled emission share) to small, well-circulated holders via the cluster-tilted lottery actively reduces wealth concentration — burning alone cannot redistribute. See [Cluster-Tilted Redistribution](../design/cluster-tilted-redistribution.md).
- **Deflationary pressure**: The 20% burn share still offsets part of tail emission.
- **Predictable**: Net inflation = gross emission − (20% fee burn share).
- **Grinding-resistant**: A per-block payout cap and pool carryover make manipulating the verifiable lottery draw unprofitable.

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
| Fee destination | 80% redistributed (lottery), 20% burned | To minters | To minters | Partially burned |
| Progressive fees | Yes (cluster-based) | No | No | No |
| Block time | 5-40s (dynamic; 5s baseline) | 600s | 120s | 12s |

## Technical References

- [Bitcoin Halving Model](https://en.bitcoin.it/wiki/Controlled_supply) - Inspiration for Phase 1
- [Monero Tail Emission](https://www.getmonero.org/resources/moneropedia/tail-emission.html) - Inspiration for Phase 2
