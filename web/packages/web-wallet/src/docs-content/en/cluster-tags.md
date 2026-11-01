## Cluster Tags

Cluster tags are Botho's novel mechanism for tracking coin provenance without compromising privacy. They enable **Sybil-resistant progressive fees**, **lottery-based redistribution**, and **privacy-preserving ring signatures**.

### The Problem: Wealth Taxation Without Identity

Traditional progressive taxation requires identity. In cryptocurrency:

- **Amount-based fees** fail instantly — split 1M into 1000×1K and pay lower rates
- **Account-based taxation** is impossible — anyone can create unlimited addresses
- **Transaction counting** doesn't work — bots can create artificial activity

**The core challenge:** How do you tax wealth concentration when you can't identify who owns what?

### The Solution: Provenance Tracking

Instead of tracking *who* owns coins, we track *where* they came from. Every coin carries a memory of its origin.

**Key insight:** Splitting coins doesn't change where they came from.

### How Cluster Tags Work

**1. Clusters Are Born at Minting**

Each block reward creates a unique "cluster" — an identity for coins minted by a specific minter. The minting reward carries a 100% tag for that cluster.

```
Block 1000: Minter A receives 50 BTH
  → Tag: {cluster_A: 100%}

Block 1001: Minter B receives 50 BTH
  → Tag: {cluster_B: 100%}
```

**2. Tags Inherit on Transfer**

When coins move, the recipient's UTXO inherits the sender's tags:

```
Minter A → Merchant → Customer
  100%   →   95%    →   90%   (A's cluster factor)
```

**3. Tags Blend on Combination**

When multiple inputs are spent together, output tags are a value-weighted average:

```
Input 1: 70 BTH {cluster_A: 100%}
Input 2: 30 BTH {cluster_B: 100%}
─────────────────────────────────
Output: 100 BTH {cluster_A: 70%, cluster_B: 30%}
```

**4. Tags Decay as Coins Move**

The standard wallets decay tag weights by 5% on every transfer, spreading attribution across the economy: a weight halves after about 13.5 transfers. This is a wallet rule, not a consensus rule. There is no age gate, and the network itself applies no decay; it only rejects outputs that claim *more* cluster weight than their inputs. What the protocol should guarantee about tag persistence is an open decision (#1687).

### Why Splitting Doesn't Work

This is what makes cluster tags special:

```
Whale splits 1,000,000 BTH into 1000 × 1000 BTH

Before: 1 UTXO with {whale_cluster: 100%}
After:  1000 UTXOs, each with {whale_cluster: 100%}

Fee rate: unchanged (based on cluster wealth, not UTXO count)
```

The "source wealth" of a cluster is the total value minted by that minter — splitting doesn't reduce it.

### Progressive Fee Curve

The cluster factor determines how much you pay. It follows a smooth sigmoid curve in the *logarithm* of cluster wealth, with its midpoint at 100,000 BTH:

| Cluster Wealth | Fee Multiplier |
|----------------|----------------|
| Small clusters (≤ ~1K BTH) | ~1x (base rate) |
| Mid-size clusters (~100K BTH) | ~3.5x (curve midpoint) |
| Whale clusters (≥ ~10M BTH) | ~6x (saturates) |

The multiplier applies to a size-based fee (`per-byte rate × transaction size`), so the same transfer costs a whale cluster up to 6× what it costs well-circulated coins — and no amount of splitting changes that.

### Lottery-Based Redistribution

80% of all transaction fees are redistributed to eligible UTXOs via a lottery. 20% are burned.

**How it works:**

1. Each transaction pays a fee based on cluster factor
2. 80% of the fee is split among 4 winners drawn with verifiable randomness
3. 20% is permanently burned (deflationary)

**How winners are selected (uniform draw, as built):**

One drawing runs per block. Winners are drawn **uniformly** among eligible outputs: neither a UTXO's value nor its cluster factor changes its chance of being picked. (Earlier designs weighted by value ÷ cluster factor; that was retired because it read amounts directly and, in simulation, routed much of the pool back to whales.)

- **Eligibility:** at least 720 blocks old, worth at least 1 µBTH (10^6 picoBTH), and inside the most recent 10,000-block window. At most 10,000 eligible candidates are considered per draw, starting from a seeded offset.
- **Winners:** 4 per drawing, chosen without replacement using a seed bound to proof-of-work.
- **Payout:** the pool is the 80% fee share, plus the block's lottery emission share (a slice of the block reward routed to the pool instead of the miner: 0% in the bootstrap epoch, then 10% more per halving, capped at 50%), plus anything carried forward from earlier blocks; each drawing pays out the lesser of what is available and the per-block cap, and the remainder carries forward.

Any progressivity has to come from the *intake* side (progressive fees and demurrage), not from the draw. At production parameters the mechanism does **not** measurably reduce inequality ([#1790](https://github.com/botho-project/botho/issues/1790)). Splitting a position into many outputs is currently profitable ([#1761](https://github.com/botho-project/botho/issues/1761), [#1682](https://github.com/botho-project/botho/issues/1682)), and past winners compound their share ([#1690](https://github.com/botho-project/botho/issues/1690)); see also [#1689](https://github.com/botho-project/botho/issues/1689) and [#1691](https://github.com/botho-project/botho/issues/1691). A redesign is under study ([#1801](https://github.com/botho-project/botho/issues/1801)). Treat this section as describing the mechanism as built.

### Ring Signatures and Tag Privacy

Cluster tags work seamlessly with CLSAG ring signatures:

**The challenge:** Ring signatures hide which input is real among 20 ring members. How do we calculate the correct fee?

**The solution:** Centroid-based validation

1. All ring members' tags are publicly known
2. The fee derives from the value-weighted *centroid* of the ring's tags, with floors that stop cheap background decoys from dragging the factor down
3. The claimed output tags must be at least 70% similar (cosine similarity) to the ring centroid, or the transaction is rejected

This prevents fee evasion — you can't cherry-pick low-factor decoys to cut your fee, because implausible ring compositions fail validation.

### Decay Mechanism Details

How decay works today (as built):

- **Wallet policy:** the standard wallets (node wallet and CLI wallet) blend input tags by value, then decay the result by 5% on every transfer
- **No age gate:** decay applies however young the spent coin is, so a weight halves after about 13.5 transfers no matter how quickly they are made
- **Consensus:** applies no decay and only rejects tag *inflation* (outputs claiming more cluster weight than the inputs). Lower or empty output tags are accepted, so a modified wallet can drop tags in one transaction

**Wash trading (sending to yourself repeatedly to decay tags) is not rate-limited by the protocol.** Its cost is the transaction fees and the demurrage class-downgrade charge, not time. Earlier versions of these docs described a 720-block age gate capping decay at ~12 events per day; that design is not enforced by the network. Tag persistence is an open decision (#1687).

| Scenario | Result (stock wallet) |
|----------|-----------------------|
| One transfer | 5% decay |
| ~14 transfers (any speed) | ~50% decay |
| Holding without transacting | 0% decay |

### Privacy Considerations

**Phase 1 (Current):** Tags are public on UTXOs. This enables direct fee verification but reveals some provenance information.

**Phase 2 (Planned):** Tags will be hidden using Pedersen commitments with zero-knowledge proofs. Validators verify correct fees without seeing actual tag values.

### Economic Incentives

The cluster tag system creates aligned incentives:

| Behavior | Effect on Tags | Incentive |
|----------|----------------|-----------|
| **Circulate coins** | Tags decay and blend | Lower fees |
| **Hoard wealth** | Tags remain concentrated | Higher fees, demurrage |
| **Split into many UTXOs** | Tags unchanged | Fees are provenance-based, so no fee benefit; as built, extra outputs buy lottery tickets, and splitting is currently profitable ([#1761](https://github.com/botho-project/botho/issues/1761), [#1682](https://github.com/botho-project/botho/issues/1682)) |

### Technical Parameters

| Parameter | Value | Purpose |
|-----------|-------|---------|
| Decay rate | 5% per transfer (wallet policy; consensus applies none) | Gradual tag diffusion |
| Min UTXO age (lottery) | 720 blocks | Lottery eligibility |
| Min UTXO value (lottery) | 1 µBTH | Dust exclusion |
| Cluster factor range | 1x–6x (midpoint 3.5x at 100K BTH) | Progressive fees |
| Ring size | 20 | Privacy set for tag propagation |
| Lottery winners | 4 per drawing (uniform draw) | Redistribution granularity |
| Lottery window | 10,000 blocks, up to 10,000 candidates | Bounds the per-draw work |
| Burn rate | 20% of fees | Deflationary pressure |
| Pool rate | 80% of fees | Redistribution amount |
| Demurrage | 2%/yr at max factor (see Tokenomics) | Reaches idle concentrated wealth |

### Summary

Cluster tags solve the "Sybil-resistant progressive taxation" problem that plagues cryptocurrency:

1. **Track provenance, not identity** — Coins remember their origin
2. **Resist splitting attacks** — Cluster wealth is fixed at minting
3. **Enable progressive fees** — Wealthy clusters pay more
4. **Fund a lottery** — 80% of fees flow back to holders through a uniform lottery (progressivity comes from fees and demurrage); a measurable reduction in inequality is not demonstrated (see Tokenomics)
5. **Preserve privacy** — Works with ring signatures
6. **Encourage circulation** — Tags decay through commerce

This makes Botho the first cryptocurrency with a credible mechanism for wealth-based fees that can't be trivially evaded.
