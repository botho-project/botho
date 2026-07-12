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

**4. Tags Decay Over Time**

Each transaction hop decays the tag by 5%, spreading attribution across the economy. But decay only applies if the UTXO is at least 720 blocks old (one to a few hours, depending on the load-adaptive block time), preventing wash trading attacks.

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

**How winners are selected (cluster-weighted):**

A UTXO's winning weight is its **value divided by its cluster factor**. This is the only progressive weighting that is split-invariant:

- Weights are value-based, so splitting a position into many UTXOs never increases total weight
- The tilt comes from cluster provenance, which inherits through splits
- Well-circulated (low-factor) coins win proportionally more; whale-cluster coins win less

To participate, a UTXO must be at least 720 blocks old and worth at least 1 µBTH.

### Ring Signatures and Tag Privacy

Cluster tags work seamlessly with CLSAG ring signatures:

**The challenge:** Ring signatures hide which input is real among 20 ring members. How do we calculate the correct fee?

**The solution:** Centroid-based validation

1. All ring members' tags are publicly known
2. The fee derives from the value-weighted *centroid* of the ring's tags, with floors that stop cheap background decoys from dragging the factor down
3. The claimed output tags must be at least 70% similar (cosine similarity) to the ring centroid, or the transaction is rejected

This prevents fee evasion — you can't cherry-pick low-factor decoys to cut your fee, because implausible ring compositions fail validation.

### Decay Mechanism Details

To prevent wash trading (sending to yourself repeatedly to decay tags):

**Age-Based Gating:**
- Decay only applies to UTXOs at least 720 blocks old
- New outputs from rapid self-transfers don't decay
- The age gate naturally caps decay at ~12 events per day

**Natural rate limiting:**

| Attack | Result |
|--------|--------|
| 100 rapid self-transfers | 0% decay (all outputs too young) |
| Patient attack (1 day) | ~46% max decay (only ~12 eligible hops) |
| Patient attack (1 week) | ~99% decay — but you've paid ~84 transaction fees |
| Holding without transacting | 0% decay |

### Privacy Considerations

**Phase 1 (Current):** Tags are public on UTXOs. This enables direct fee verification but reveals some provenance information.

**Phase 2 (Planned):** Tags will be hidden using Pedersen commitments with zero-knowledge proofs. Validators verify correct fees without seeing actual tag values.

### Economic Incentives

The cluster tag system creates aligned incentives:

| Behavior | Effect on Tags | Incentive |
|----------|----------------|-----------|
| **Circulate coins** | Tags decay and blend | Lower fees, higher lottery weight |
| **Hoard wealth** | Tags remain concentrated | Higher fees, lower lottery weight, demurrage |
| **Split into many UTXOs** | Tags unchanged | No benefit — fees and lottery weight are provenance- and value-based |

### Technical Parameters

| Parameter | Value | Purpose |
|-----------|-------|---------|
| Decay rate | 5% per eligible hop | Gradual tag diffusion |
| Min UTXO age (decay + lottery) | 720 blocks | Wash trading prevention |
| Min UTXO value (lottery) | 1 µBTH | Dust exclusion |
| Cluster factor range | 1x–6x (midpoint 3.5x at 100K BTH) | Progressive fees |
| Ring size | 20 | Privacy set for tag propagation |
| Lottery winners | 4 per drawing | Redistribution granularity |
| Burn rate | 20% of fees | Deflationary pressure |
| Pool rate | 80% of fees | Redistribution amount |
| Demurrage | 2%/yr at max factor (see Tokenomics) | Reaches idle concentrated wealth |

### Summary

Cluster tags solve the "Sybil-resistant progressive taxation" problem that plagues cryptocurrency:

1. **Track provenance, not identity** — Coins remember their origin
2. **Resist splitting attacks** — Cluster wealth is fixed at minting
3. **Enable progressive fees** — Wealthy clusters pay more
4. **Power fair redistribution** — Lottery weight tilts toward well-circulated coins
5. **Preserve privacy** — Works with ring signatures
6. **Encourage circulation** — Tags decay through commerce

This makes Botho the first cryptocurrency with a credible mechanism for wealth-based fees that can't be trivially evaded.
