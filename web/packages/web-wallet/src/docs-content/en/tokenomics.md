## Tokenomics

Botho (BTH) uses a two-phase emission model designed for long-term sustainability: an initial distribution phase with halvings, followed by perpetual tail emission targeting stable inflation.

### Overview

| Parameter | Value |
|-----------|-------|
| Token symbol | BTH |
| Smallest unit | picocredit (10⁻¹² BTH) |
| Pre-mine | None (100% mined) |
| Phase 1 supply | ~611 million BTH |
| Block time | 3–40 seconds (load-adaptive; 5 s monetary baseline) |

### Unit System

BTH uses 12-decimal precision. The picocredit is the single base unit — every amount on the wire (balances, fees, cluster wealth) is denominated in picocredits, and formatting into BTH happens only in the display layer:

- **1 picocredit** = 0.000000000001 BTH (smallest unit)
- **1 microBTH (µBTH)** = 1,000,000 picocredits = 0.000001 BTH
- **1 milliBTH (mBTH)** = 1,000,000,000 picocredits = 0.001 BTH
- **1 BTH** = 1,000,000,000,000 picocredits

---

## Emission Schedule

All monetary math assumes the 5-second high-load block time (actual blocks range 3–40 s, dropping to 3 s only at very high load, 20+ tx/s). When the network is idle and blocks slow down (up to 40 s), emission stretches proportionally — a natural dampener: a busy network emits at the full schedule, an idle one emits less.

### Phase 1: Halving Period (~5 years at full load)

The minting reward starts at 50 BTH and halves every **6,307,200 blocks** (one year of 5-second blocks). After five halving epochs, Phase 1 has distributed **611,010,000 BTH**:

| Epoch | Minting Reward | Cumulative Supply |
|-------|----------------|-------------------|
| 1 | 50 BTH | ~315.4M BTH |
| 2 | 25 BTH | ~473.0M BTH |
| 3 | 12.5 BTH | ~552.0M BTH |
| 4 | 6.25 BTH | ~591.3M BTH |
| 5 | 3.125 BTH | ~611.0M BTH |

(This is the canonical schedule ratified in issue #351, locked by regression tests in the node.)

### Phase 2: Tail Emission

After Phase 1, Botho transitions to perpetual tail emission targeting **2% annual net inflation** (gross emission minus fee burns), with difficulty adjusting to hit the target.

**Why tail emission?**

- **Security budget** - Ensures minters always have incentive to secure the network
- **Lost coin replacement** - Compensates for coins lost to forgotten keys
- **Predictable monetary policy** - 2% is below typical fiat inflation

At the ~611M BTH Phase-1 supply and full load, 2% works out to roughly **2 BTH per block**, growing slowly with supply.

---

## Fee Structure

### Transaction Fees

Every fee is split two ways: **80% is redistributed** to holders through the cluster-tilted lottery, and **20% is burned**, creating deflationary pressure that offsets tail emission.

```
fee = per-byte rate × transaction size × cluster factor × output penalty + memo fees
```

| Parameter | Value |
|-----------|-------|
| Fee basis | Transaction size (bytes), not amount |
| Cluster factor | 1x–6x progressive multiplier |
| Output penalty | Quadratic in output count, capped at 100x |
| Memo fee | Flat per encrypted memo |
| Fee destination | 80% lottery pool / 20% burned |
| Priority | Higher fees = faster confirmation |

### Cluster-Based Progressive Fees

Botho implements a novel **progressive fee system** that taxes wealth concentration without enabling Sybil attacks.

**The core innovation:** Instead of taxing based on transaction amount (easily gamed by splitting), fees are based on coin *provenance* — where coins originally came from.

| Parameter | Value |
|-----------|-------|
| Cluster factor range | 1x to 6x multiplier |
| Curve shape | Sigmoid in log(cluster wealth), midpoint 3.5x at 100K BTH |
| Tag decay | 5% per eligible hop |

**Why it's Sybil-resistant:** Splitting coins doesn't change their origin. A whale's coins carry the same cluster tag whether held in 1 UTXO or 1000.

### Demurrage

Transaction fees are a consumption tax — they can't touch wealth that never moves. Demurrage closes that gap: a **holding charge on wealthy-cluster coins, paid when they are eventually spent**.

| Parameter | Value |
|-----------|-------|
| Rate | 2% per year at the maximum (6x) cluster factor |
| Scaling | Proportional to (factor − 1) — factor-1 coins pay **zero** |
| Bootstrap | Disabled during the first halving epoch |
| Proceeds | Flow into the lottery redistribution pool |

Everyday coins never pay demurrage; it binds only concentrated, idle wealth. Churning doesn't escape it — spending pays the accrued charge first, so the total paid over any holding period is the same no matter how often you self-transfer (and each transfer adds fees on top).

> **See the [Cluster Tags](#cluster-tags) section** for a complete explanation of how provenance tracking, progressive fees, lottery redistribution, and ring signature privacy work together.

---

## Supply Projections

### Long-Term Growth

At sustained full load (5-second blocks; slower when the network is idle):

| Epoch | Approximate Supply | Annual Inflation |
|-------|-------------------|------------------|
| 1 | ~315M BTH | High (initial) |
| 3 | ~552M BTH | ~17% |
| 5 | ~611M BTH | ~3% |
| Tail (perpetual) | +2%/year net of burns | 2% |

---

## Economic Design Philosophy

### Why No Pre-mine?

- **Fair distribution** - Everyone starts equal; early minters take on risk
- **Credibility** - No insider advantage or founder enrichment
- **Decentralization** - No concentrated holdings from day one

### Why Split Fees 80/20?

- **Redistribution** - 80% flows back to holders via the cluster-tilted lottery, favoring well-circulated coins
- **Deflationary pressure** - The 20% burn offsets tail emission
- **Predictable** - Net inflation = gross emission − burns

### Why Progressive Cluster Fees?

- **Reduce concentration** — Wealthy clusters pay more
- **Sybil-resistant** — Can't avoid by splitting accounts
- **Encourage circulation** — Moving coins diffuses tags, reducing fees
- **Privacy-compatible** — Works with ring signatures and stealth addresses

> **Deep dive:** See the [Cluster Tags](#cluster-tags) documentation for the complete technical explanation.
