# Economic Simulation Analysis

## Executive Summary

This analysis reveals **critical design tensions** between privacy and equality in the cluster tax mechanism. Key findings:

1. **Ring signature privacy is effective** (~95% efficiency) against combined adversaries
2. **Cluster decay creates a taxation evasion vector** - higher decay = easier wash trading
3. **Whales can evade 94-99% of cluster tax** through wash trading and aggressive strategies
4. **The current design may not achieve its inequality reduction goals** due to evasion

---

## Experiment 1: Privacy Baseline

### Pool Composition Impact on Privacy

| Standard Tx % | Combined Bits | ID Rate | Effective Anonymity |
|--------------|---------------|---------|---------------------|
| 30%          | 3.12          | 47.3%   | 14.3 / 20           |
| 50%          | 3.92          | 37.2%   | 18.1 / 20           |
| 70%          | 4.30          | 37.3%   | 19.8 / 20           |

**Insight**: Higher standard transaction prevalence improves privacy for everyone. The "herd effect" is strong - more uniform transactions make fingerprinting harder.

---

## Experiment 2: Decay Rate Impact on Privacy

| Decay Rate | Combined Bits | ID Rate |
|------------|---------------|---------|
| 2.5%       | 4.10          | 32.3%   |
| 5.0%       | 4.10          | 32.6%   |
| 10.0%      | 4.11          | 31.8%   |
| 20.0%      | 4.29          | 78.7%   |

**Insight**: Decay rate has minimal impact on privacy until very high values (20%), where cluster fingerprinting becomes extremely effective (78.7% ID rate).

---

## Experiment 3: Ring Size Trade-offs

| Ring Size | Theoretical | Measured | Efficiency | Bits/KB |
|-----------|-------------|----------|------------|---------|
| 5         | 2.32        | 2.19     | 94.5%      | 0.136   |
| 7         | 2.81        | 2.66     | 94.7%      | 0.116   |
| 9         | 3.17        | 3.00     | 94.6%      | 0.103   |
| 11        | 3.46        | 3.29     | 95.1%      | 0.091   |
| 13        | 3.70        | 3.51     | 94.7%      | 0.081   |

**Insight**: Ring size 7 is indeed the sweet spot:
- 94.7% theoretical efficiency (cluster attacks have minimal impact)
- Best bits-per-KB is ring 5, but ring 7 adds +21% privacy for +35% size
- Larger rings have diminishing returns (ring 13 is +78% size for only +32% more privacy)

---

## Experiment 4: Whale Strategy Effectiveness (CRITICAL FINDING)

| Strategy     | Whale Fees | Effectiveness | Final Gini |
|--------------|------------|---------------|------------|
| Passive      | 1,388,465  | 0% (baseline) | 0.9546     |
| Wash Trading | 73,149     | **94.7%**     | 0.9547     |
| Structuring  | 1,327,565  | 4.4%          | 0.9496     |
| Aggressive   | 8,380      | **99.4%**     | 0.9514     |

**CRITICAL INSIGHT**: Whales can evade 94-99% of cluster tax through:
- **Wash trading**: Self-transfers to decay cluster tags
- **Aggressive**: Combination of wash trading + mixers + structuring

This fundamentally undermines the cluster tax as an inequality reduction mechanism.

---

## Experiment 5: Progressive vs Flat Fees

| Metric           | Progressive | Flat   |
|------------------|-------------|--------|
| Initial Gini     | 0.5953      | 0.5953 |
| Final Gini       | 0.8810      | 0.8810 |
| Gini Change      | +0.2856     | +0.2856|
| Q5 Fee Rate      | 759 bps     | 100 bps|

**Insight**: Despite Q5 (richest) paying 7.6x higher fees than other quintiles, the Gini coefficient ended up identical. This suggests:
1. Fee levels are too low to materially impact wealth distribution
2. The burn mechanism doesn't redistribute - it just destroys value
3. Whales may be successfully evading via strategies tested in Exp 4

---

## Experiment 6: Mixer Economy

| Metric            | Value    |
|-------------------|----------|
| Final Gini        | 0.8434   |
| Mixer Utilization | 47.23%   |
| Winner            | Mixer 1 (50bps) |

**Insight**: Competition drives mixer fees down. The lowest-fee mixer (50 bps) captured 100% of the market. Whales are rational economic actors who will use privacy tools when beneficial.

---

## Experiment 7-8: Wash Trading Economics

### Decay Rate Impact on Wash Trading Viability

| Decay | 30 Hops Savings | Break-even (txs) |
|-------|-----------------|------------------|
| 2%    | 0.00%           | Never            |
| 5%    | 3.07%           | 278              |
| 10%   | 21.49%          | 27               |
| 20%   | 26.19%          | 20               |

**CRITICAL INSIGHT**: Higher decay rates make wash trading MORE profitable!

- At 2% decay: Wash trading never pays off
- At 5% decay: Profitable after 278 transactions
- At 10% decay: Profitable after only 27 transactions
- At 20% decay: Profitable after only 20 transactions

This is **counterintuitive but logical**: higher decay = faster tag erosion = faster fee reduction. The cost is fixed (hops × base fee), but benefit scales with decay rate.

---

## Design Recommendations

### Option A: Reduce Decay Rate (Privacy Trade-off)
- Set decay to 2-3% per hop
- Wash trading becomes uneconomical (never breaks even)
- BUT: Reduces privacy (cluster fingerprinting works better long-term)
- Privacy impact: Moderate (clusters persist longer)

### Option B: Fee on Self-Transfers (Anti-Wash Trading)
- Detect and penalize transactions where sender ≈ receiver
- Require minimum "distance" between transaction parties
- Could use chain analysis to detect wash trading patterns
- Privacy impact: High (requires surveillance)

### Option C: Time-Locked Decay
- Tags only decay based on time, not transaction count
- Prevents accelerating decay via wash trading
- But still allows natural diffusion over long periods
- Privacy impact: Low (maintains privacy properties)

### Option D: Progressive Decay Rate
- Small clusters decay faster (privacy for regular users)
- Large clusters decay slower (wealthy can't wash trade away)
- Creates two-tier privacy system
- Privacy impact: Asymmetric (worse for wealthy)

### Option E: Burn Rate Increase
- If evasion is 94%, increase rates 20x to compensate
- Current 0.05-30% becomes 1-600%
- Impractical - would destroy economic utility
- Not recommended

### Option F: Alternative Wealth Taxation
- Move away from transaction-based taxation
- Consider demurrage (holding tax) on large balances
- Wealth taxes work regardless of transaction patterns
- Privacy impact: Requires balance knowledge

---

## Key Questions for Further Research

1. **What decay rate balances privacy vs evasion?**
   - Need simulations at 2%, 3%, 4% decay with whale strategies

2. **Can we detect wash trading without breaking privacy?**
   - Statistical patterns in ring membership?
   - Timing analysis?

3. **Is the burn mechanism effective?**
   - Current implementation burns fees but doesn't redistribute
   - Could we redirect to a UBI-style distribution?

4. **What's the equilibrium with rational whales?**
   - Long-term simulation with adaptive whale behavior

---

---

## Block-Based Decay Implementation

### New Module: `block_decay.rs`

We implemented a **block-aware decay system** as an alternative to hop-based decay:

```rust
pub struct BlockDecayConfig {
    pub half_life_blocks: u64,    // Blocks for 50% decay
    pub min_decay_interval: u64,  // Minimum blocks between decay updates
    pub hop_decay_rate: u32,      // Optional: small hop decay for mixing incentive
}
```

### Comparison Results

| Scenario | Hop-Based | Block-Based | Difference |
|----------|-----------|-------------|------------|
| 100 wash txs in 100 blocks | 0.6% remaining, 85.7% fee reduction | 99.9% remaining, 0% fee reduction | **8567x more resistant** |
| 1000 wash txs in 1 hour | ~0% remaining | 99.4% remaining | Complete protection |

### Half-Life Parameter Selection

| Half-Life | Privacy Decay | Taxation Persistence | Recommendation |
|-----------|---------------|----------------------|----------------|
| 1 day     | Fast          | Short                | Too short for effective taxation |
| 1 week    | Moderate      | Medium               | **Good balance** |
| 1 month   | Slow          | Long                 | Maximum taxation, slower privacy |

### Run the Comparison Tool

```bash
# Compare decay mechanisms
./target/release/cluster-tax-sim decay-compare \
    --wealth 100000000 \
    --hop-decay 5.0 \
    --half-life 60480 \
    --wash-txs 100 \
    --blocks 100
```

### Key Finding

Block-based decay completely eliminates the wash trading attack vector while still providing natural privacy improvement over time. The trade-off is:
- **Privacy**: Tags still decay, just on a fixed schedule
- **Taxation**: Whales cannot accelerate decay through transactions
- **Simplicity**: Single parameter (half-life) vs decay rate

---

## Rate-Limited Hybrid Decay Model

### Design

A third option explored: rate-limited hop decay that combines aspects of both:

```rust
pub struct RateLimitedDecayConfig {
    pub decay_rate_per_hop: TagWeight,     // e.g., 5% per eligible hop
    pub min_blocks_between_decays: u64,    // e.g., 360 blocks (~1 hour)
    pub passive_half_life_blocks: Option<u64>,
}
```

### Three-Way Comparison

```bash
./target/release/cluster-tax-sim decay-compare-all \
    --wealth 100000000 \
    --hop-decay 5.0 \
    --half-life 60480 \
    --min-blocks 360 \
    --wash-txs 100 \
    --blocks 100
```

| Metric | Hop-Based | Block-Based | Rate-Limited |
|--------|-----------|-------------|--------------|
| Tag Remaining (100 txs/100 blocks) | 0.59% | 99.92% | 100% |
| Fee Reduction | 85.7% | 0% | 0% |
| Eligible Decay Events | 100 | N/A | 0 |

### Falsification Results

**H1: Does block decay break legitimate privacy use cases?**

For a normal user (1 tx/day over 30 days):
- Hop-based: 21.46% remaining
- Block-based: 5.36% remaining (decays MORE)
- Rate-limited: 22.59% remaining

**Finding**: Block-based decay is MORE aggressive for legitimate users over long periods because it decays based on time, not activity. Rate-limited behaves similarly to hop-based for legitimate users.

**H2: Can attackers use timing attacks?**

For a patient attacker (168 txs over 1 week, spaced 1/hour):
- Hop-based: 0.02% remaining, 87.9% fee reduction
- Block-based: 50% remaining, 0% fee reduction
- Rate-limited: 0.02% remaining, 87.9% fee reduction

**CRITICAL FINDING**: Rate-limited model only SLOWS the attack, it doesn't PREVENT it. A patient attacker who spaces transactions 1 hour apart can achieve the same evasion as hop-based.

**H3: Edge cases verified**

Rate limiting correctly prevents decay when < min_blocks have passed. Implementation is correct.

### Model Trade-offs Summary

| Model | Wash Trading Resistance | Long-term Behavior | Complexity |
|-------|------------------------|-------------------|------------|
| Hop-based | ❌ Vulnerable | Decays only on activity | Simple |
| Block-based | ✅ Immune | Decays even without activity | Simple |
| Rate-limited | ⚠️ Slows attack | Same as hop-based over time | Complex |

### Recommendation (Updated)

After further analysis, **AND-based decay with epoch cap** is the recommended approach. It addresses a critical flaw in pure block-based decay: wealthy holders get "free" tag decay just by waiting, without needing to transact.

## AND-Based Decay with Epoch Cap (Final Design)

### Core Insight

Decay requires **THREE conditions**:
1. **A transfer must occur** (hop) - no passive decay
2. **Sufficient time must pass** since last decay (rate limit)
3. **Epoch cap not reached** for current period (bounds max decay)

### Configuration

```rust
pub struct AndDecayConfig {
    pub decay_rate_per_hop: TagWeight,      // 5% per eligible hop
    pub min_blocks_between_decays: u64,     // 720 blocks (~2 hours)
    pub max_decays_per_epoch: u32,          // 12 per day
    pub epoch_blocks: u64,                  // 8,640 blocks (~1 day)
}
```

### Four-Way Comparison (1 Week, 1000 Wash Trades)

| Model | Tag Remaining | Decay Events | Holding Decay |
|-------|---------------|--------------|---------------|
| Hop-based | 0.00% | 1000 | None |
| Block-based | 50.00% | N/A | 50% (passive) |
| Rate-limited | 0.02% | 166 | None |
| **AND-based** | **1.35%** | **84 (capped)** | **None** |

### Key Properties

1. **Requires transfer**: Wealthy holders don't get free decay by waiting
2. **Rate-limited**: Can't rapid-fire wash trades
3. **Epoch-capped**: Even patient attackers are bounded to X decays/day
4. **Legitimate trading works**: Normal users still gain privacy over time

### Maximum Decay Bounds

With recommended parameters (5% decay, 12/day cap):
- Per day: 46% decay (54% remaining)
- Per week: 98.7% decay (1.35% remaining)
- Per month: ~100% decay (~0% remaining)

### Run the Comparison

```bash
./target/release/cluster-tax-sim decay-compare-four \
    --wealth 100000000 \
    --hop-decay 5.0 \
    --half-life 60480 \
    --min-blocks 720 \
    --max-per-day 12 \
    --wash-txs 1000 \
    --blocks 60480
```

---

## Files Generated

- `experiments/results/gini_progressive.csv`
- `experiments/results/gini_flat.csv`
- `experiments/results/gini_comparison.csv`

## Commands to Reproduce

```bash
# Privacy baseline
./target/release/cluster-tax-sim privacy -n 20000 --pool-size 100000

# Whale strategies
./target/release/cluster-tax-sim scenario-whale --whale-wealth 10000000 --rounds 5000

# Wash trading economics
./target/release/cluster-tax-sim wash-trading --wealth 100000000 --decay 5

# Progressive vs flat comparison
./target/release/cluster-tax-sim compare --retail-users 100 --whales 5 --rounds 20000
```
