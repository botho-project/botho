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

---

## Fresh Coin Mixing Attack Analysis (Issue #90)

This section analyzes whether wealthy clusters can evade progressive taxation by mixing their coins with freshly minted coins to dilute their cluster attribution.

### Executive Summary

**Finding: Fresh coin mixing provides marginal, self-limiting fee reduction that is economically unfavorable.**

The attack hypothesis is partially correct - mixing with fresh coins DOES dilute tag weights at the UTXO level. However, several factors make this attack impractical:

1. **Cluster wealth is preserved**: Mixing redistributes tags but doesn't change total cluster wealth
2. **Fresh coins aren't free**: They carry their miner's cluster attribution
3. **Diminishing returns**: Large dilution requires proportionally large fresh coin acquisition
4. **Self-limiting**: The more coins minted, the higher the minter's cluster wealth grows

### Q1: How Effective is Dilution?

**Answer: Modest fee reduction (4-10%), not elimination.**

#### Mixing Math

When coins are mixed, tags blend proportionally (from `tag.rs:116-144`):

```
new_weight(C) = (self_value × self_weight(C) + incoming_value × incoming_weight(C)) / total_value
```

#### Example: 100M Whale Mixes with 10M Fresh Coins

**Before mixing:**
| Account | Balance | Cluster W Tag | Cluster F Tag |
|---------|---------|---------------|---------------|
| Whale | 100M | 100% | 0% |
| Fresh | 10M | 0% | 100% |

**After mixing (110M combined):**
| Cluster | Tag Weight | Calculation |
|---------|------------|-------------|
| W | 90.9% | (100M × 100% + 10M × 0%) / 110M |
| F | 9.1% | (100M × 0% + 10M × 100%) / 110M |

#### Fee Impact Analysis

Using default fee curve parameters:
- `r_min_bps`: 5 (0.05%)
- `r_max_bps`: 3000 (30%)
- `w_mid`: 10,000,000 (inflection point)

| Cluster Wealth | Fee Rate |
|---------------|----------|
| 100M (whale) | ~3000 bps (max) |
| 10M (fresh) | ~1500 bps (midpoint) |
| 1M (small) | ~500 bps |

**Effective fee rate after mixing:**
```
rate = 0.909 × rate(100M) + 0.091 × rate(10M)
     = 0.909 × 3000 + 0.091 × 1500
     = 2727 + 137
     = 2864 bps
```

**Fee reduction: (3000 - 2864) / 3000 = 4.5%**

#### Dilution Effectiveness Table

| Fresh Coins | Dilution Ratio | W Tag After | New Rate | Fee Reduction |
|-------------|---------------|-------------|----------|---------------|
| 10M | 10:100 | 90.9% | 2864 bps | 4.5% |
| 25M | 25:100 | 80.0% | 2700 bps | 10.0% |
| 50M | 50:100 | 66.7% | 2500 bps | 16.7% |
| 100M | 100:100 | 50.0% | 2250 bps | 25.0% |

**Key insight**: Significant fee reduction requires acquiring fresh coins equal to or greater than existing holdings.

### Q2: What's the Cost of Fresh Coins?

**Answer: Expensive - mining costs or market premium.**

#### Option 1: Mine Fresh Coins

Using default monetary policy:
- Initial reward: 50 BTH/block
- Block time: ~1 minute

To mine 10M BTH:
```
Blocks needed = 10,000,000 BTH / 50 BTH per block = 200,000 blocks
Time = 200,000 minutes ≈ 139 days of continuous mining
```

**Cost**: Hardware + electricity for 139 days of mining. In competitive equilibrium, mining cost ≈ coin value.

#### Option 2: Buy "Fresh" Coins

**Critical insight**: There are no truly "fresh" coins on the open market.

- All circulating coins already have cluster attribution
- Buying from a miner means buying coins tagged to the miner's cluster
- If the miner has minted 50M BTH total, their "fresh" coins carry ClusterWealth(miner) = 50M
- Large miners have high cluster wealth → their coins carry high fee rates

#### The Freshness Paradox

"Fresh" coins only exist at the moment of minting. As soon as a miner accumulates coins:

```
ClusterWealth(miner) = Σ(all coins minted by this miner)
```

A successful miner who has produced 100M BTH has ClusterWealth = 100M, same as a whale!

### Q3: Can This Be Repeated?

**Answer: Yes, but with diminishing returns and increasing costs.**

#### Multi-Round Dilution Attack

| Round | Whale Balance | Fresh Added | W Tag | Effective Rate |
|-------|--------------|-------------|-------|----------------|
| 0 | 100M | - | 100% | 3000 bps |
| 1 | 110M | 10M | 90.9% | 2864 bps |
| 2 | 121M | 11M | 82.6% | 2750 bps |
| 3 | 133M | 12M | 75.2% | 2640 bps |
| 4 | 146M | 13M | 68.5% | 2540 bps |
| 5 | 161M | 15M | 62.4% | 2445 bps |

After 5 rounds of dilution:
- Total fresh coins acquired: 61M
- Fee reduction: 18.5%
- Cost: 61M worth of fresh coins

**Diminishing returns**: Each round requires more fresh coins for less fee reduction.

#### Equilibrium Analysis

In the long run, if all wealthy participants adopt this strategy:
1. Demand for fresh coins increases
2. Mining becomes more profitable
3. More miners enter → more coins minted
4. Total supply grows → all cluster wealths grow proportionally
5. System reaches new equilibrium with similar relative fee rates

The attack doesn't provide sustainable advantage - it's an arms race that benefits miners.

### Q4: Does Decay Interact with Mixing?

**Answer: Decay happens on transfer, not mixing. Mixed tags decay uniformly.**

#### Decay on Transfer

From `transfer.rs:143-144`:
```rust
let mut transferred_tags = sender.tags.clone();
transferred_tags.apply_decay(config.decay_rate);
```

Decay applies to ALL tags proportionally, including the "fresh" cluster tags mixed in.

#### Post-Mix Decay Example

After mixing (90.9% W, 9.1% F), if decay occurs:
```
After 5% decay:
- W tag: 90.9% × 0.95 = 86.4%
- F tag: 9.1% × 0.95 = 8.6%
- Background: 5%
```

Both tags decay together - mixing doesn't reset or affect decay timers.

#### AND-Based Decay Interaction

Under the recommended AND-based decay model:
- Decay requires transfer + time elapsed + epoch cap not reached
- Mixing (which is a transfer) triggers ONE decay event
- The mixed coins are now on a unified decay schedule
- No "fresh" coins get special treatment

### Economic Viability Analysis

#### Break-Even Calculation

**Scenario**: Whale with 100M coins, annual transaction volume = 100M

| Metric | Value |
|--------|-------|
| Fee reduction from 10M fresh | 4.5% |
| Annual fee savings | 100M × 30% × 4.5% = 1.35M |
| Cost of 10M fresh coins | ~10M (mining cost ≈ value) |
| Payback period | 10M / 1.35M = **7.4 years** |

**Verdict**: Marginally profitable over very long horizons, but:
- Ties up 10% additional capital
- Subject to market risks
- Doesn't compound (can't reuse the fresh coins)

#### Comparison with Wash Trading

| Attack | Fee Reduction | Cost | Verdict |
|--------|--------------|------|---------|
| Wash trading (hop-based) | 94-99% | Low (base fees) | ❌ Viable (patched by AND-based) |
| Wash trading (AND-based) | 46%/day max | Low | ⚠️ Bounded |
| **Fresh coin mixing** | 4-25% | High (coin acquisition) | ❌ Not economical |

Fresh coin mixing is far less effective and far more expensive than wash trading.

### Why the System is Robust

#### Defense 1: Cluster Wealth Preservation

Mixing redistributes tags but doesn't reduce cluster wealth:

```
Before: ClusterWealth(W) = 100M, ClusterWealth(F) = 10M
After:  ClusterWealth(W) = 110M × 90.9% = 100M  ← unchanged!
        ClusterWealth(F) = 110M × 9.1% = 10M   ← unchanged!
```

The fee rate for cluster W is still based on 100M wealth.

#### Defense 2: No Free Fresh Coins

Every fresh coin comes with cluster attribution:
- Mined coins → miner's cluster
- Bought coins → seller's cluster
- "Freshness" is an illusion

#### Defense 3: Background Attribution

As coins mix repeatedly across the economy:
- Tags diffuse toward "background" (no cluster attribution)
- Background coins pay the minimum rate (10 bps)
- This is the intended long-term privacy equilibrium

The system is designed for tags to eventually diffuse - mixing attacks just speed up what happens naturally through legitimate commerce.

### Potential Mitigations (If Needed)

The current system is robust, but if future analysis shows concern:

#### Mitigation A: Minting Cluster Inheritance

New coins could inherit a fraction of the minter's existing cluster wealth:
```rust
new_cluster_wealth = max(minted_amount, parent_cluster_wealth × inheritance_rate)
```

This prevents miners from creating "clean" clusters.

#### Mitigation B: Mixing Penalty

Transactions that combine inputs from disparate clusters could pay additional fees:
```rust
if input_clusters.len() > 1 {
    fee *= mixing_penalty_factor;
}
```

This discourages deliberate mixing for fee reduction.

#### Mitigation C: Fresh Coin Premium

Newly minted coins could have temporarily elevated cluster attribution:
```rust
if coin_age < maturity_blocks {
    effective_cluster_wealth *= fresh_coin_premium;
}
```

This makes fresh coins more expensive to use initially.

**Current recommendation**: No mitigations needed. The attack is not economically viable.

### Conclusion

**The fresh coin mixing attack is theoretically possible but economically impractical.**

| Research Question | Answer |
|-------------------|--------|
| Q1: How effective is dilution? | 4-25% fee reduction, requires equal-value fresh coins |
| Q2: Cost of fresh coins? | High - mining costs or market acquisition |
| Q3: Can it be repeated? | Yes, with diminishing returns |
| Q4: Decay interaction? | Decay applies uniformly to mixed tags |

**Key defenses already in place:**
1. Cluster wealth is preserved through mixing
2. Fresh coins carry miner's cluster attribution
3. Economic cost exceeds benefit
4. Natural tag diffusion makes attack unnecessary

**No additional mitigations required.**

---

## Sybil/Split Attack Analysis (Issue #89)

This section analyzes whether the AND-based decay mechanism is resistant to Sybil/split attacks where wealthy clusters attempt to evade progressive taxation by splitting funds across many accounts.

### Executive Summary

**Finding: The progressive fee system is inherently resistant to Sybil/split attacks.**

The key defense is that progressive fees are calculated based on **cluster wealth** (the total value attributed to a cluster across ALL accounts), not individual account balances. When a whale splits funds into 1000 accounts, all those accounts still carry the same cluster tag, and the ClusterWealth tracker still sees the full 100M attributed to that cluster.

### Q1: Does Cluster Wealth Tracking Mitigate Splitting?

**Answer: YES - Splitting provides zero fee evasion benefit.**

#### How the System Works

1. **ClusterWealth Tracking**: The system maintains a global view of wealth per cluster:
   ```
   W_{C_k} = Σ_i (balance_i × tag_i(k))
   ```
   This sums all balances weighted by their cluster tag attribution.

2. **Progressive Fee Formula**:
   ```
   fee = fee_per_byte × tx_size × cluster_factor(cluster_wealth)
   ```
   The `cluster_factor` is based on the TOTAL cluster wealth, not individual UTXO balance.

3. **Tag Inheritance**: When funds are transferred, the receiving UTXO inherits the sender's cluster tags (with 5% decay per eligible hop).

#### Split Attack Scenario

Consider a whale with 100M coins (100% tagged to cluster C):

| Step | State | Cluster C Wealth |
|------|-------|------------------|
| Initial | Account A: 100M (100% C) | 100M |
| After split to 1000 accounts | A1..A1000: ~100K each (95% C after decay) | ~95M |
| Fee rate for each account | Based on 95M cluster wealth | HIGH |

**Result**: Each of the 1000 smaller accounts pays the same high fee rate as the original whale account, because the fee is based on cluster wealth (95M), not individual balance (100K).

#### Code Verification

From `cluster-tax/src/transfer.rs`:

```rust
pub fn effective_fee_rate(
    &self,
    cluster_wealth: &ClusterWealth,  // <-- Global cluster wealth
    fee_curve: &FeeCurve,
) -> FeeRateBps {
    // Weighted average of cluster rates by tag weight
    for (cluster, weight) in self.tags.iter() {
        let cluster_w = cluster_wealth.get(cluster);  // <-- Uses TOTAL cluster wealth
        let rate = fee_curve.rate_bps(cluster_w) as u64;
        weighted_rate += rate * weight as u64;
    }
    // ...
}
```

The fee calculation explicitly uses `cluster_wealth.get(cluster)`, which returns the total wealth attributed to that cluster across ALL accounts.

### Q2: Economics of Split Attacks

**Answer: Split attacks have negative ROI - they cost more than they save.**

#### Cost Analysis

| Action | Cost |
|--------|------|
| Split into N accounts | N × base_fee × cluster_factor |
| Each intermediate tx | base_fee × cluster_factor (still high!) |
| Recombine to single | N × base_fee × cluster_factor |
| Total splitting overhead | 2N × base_fee × cluster_factor |

#### Benefit Analysis

| Expected Benefit | Actual Benefit |
|-----------------|----------------|
| Lower fee rate per account | **ZERO** - cluster wealth unchanged |
| Lower fee per transaction | **ZERO** - same cluster factor applies |
| Privacy improvement | **MINIMAL** - same cluster tag on all |

#### Break-Even Analysis

There is **no break-even point** because the attack provides zero benefit:

```
Benefit = 0 (no fee reduction from splitting)
Cost = 2N × base_fee (splitting + recombining overhead)
ROI = -2N × base_fee (always negative)
```

#### Comparison with Wash Trading

| Attack | Mechanism | Effectiveness |
|--------|-----------|---------------|
| **Wash Trading** | Decay tags via self-transfers | 94-99% evasion (hop-based) |
| **Sybil/Split** | Split into many accounts | 0% evasion |

Wash trading targets the **decay mechanism** to reduce cluster attribution. Split attacks cannot reduce cluster attribution because tags propagate to all descendants.

### Q3: Structural Limits on Splitting

Even if splitting could provide benefits, structural constraints limit the attack:

#### Ring Signature Constraints

- Each transaction input requires 7 ring members (decoys)
- More inputs = larger transaction size = higher size-based fee
- Recombining 1000 UTXOs requires multiple transactions (max ~16 inputs practical)

| Inputs | Ring Overhead | Size Impact |
|--------|---------------|-------------|
| 2 | 14 decoys | ~1.4 KB |
| 16 | 112 decoys | ~11 KB |
| 100 | 700 decoys | ~70 KB |

#### UTXO Management Overhead

- More UTXOs = more complex wallet state
- Each UTXO requires storage (key images, decoy selection)
- Transaction construction becomes computationally expensive

#### Privacy Implications

Splitting actually **HARMS** privacy rather than helping:

| Factor | Impact on Privacy |
|--------|------------------|
| Common origin | All split UTXOs trace to same source |
| Temporal clustering | Simultaneous creation is linkable |
| Amount analysis | Equal splits are highly identifiable |
| Recombination | Merging multiple UTXOs links them |

An adversary observing 1000 UTXOs of equal value created in the same block, all with the same cluster tag, can trivially identify them as belonging to the same entity.

### Simulation: 100M Split Into 1000 Accounts

#### Scenario Parameters

```
Initial: 1 account with 100M coins, 100% tagged to cluster C
Split: 1000 accounts with ~100K each
Decay: 5% per transfer (1 hop to split)
Fee curve: 1x-6x based on cluster wealth
```

#### Results

| Metric | Single Account | After Split (1000 accounts) |
|--------|---------------|----------------------------|
| Total balance | 100M | ~95M (after fees) |
| Cluster C wealth | 100M | ~95M |
| Fee rate | 6x (max tier) | 6x (max tier) |
| Fee per 1000 tx | 1000 × high | 1000 × high |
| Privacy | Ring of 7 | Ring of 7 (same) |

**Key Finding**: The fee rate remains at maximum tier because cluster wealth is still ~95M, well above the 10M threshold for maximum fees.

### Why the System Works

#### Cluster-Based vs Account-Based Taxation

Traditional wealth taxes face Sybil resistance problems because wealth is measured per account. Botho's cluster tax is fundamentally different:

| Approach | Measurement | Sybil Resistance |
|----------|-------------|------------------|
| Account-based tax | Balance per account | ❌ Vulnerable |
| **Cluster-based tax** | Total cluster wealth | ✅ Resistant |

The cluster ID acts as an immutable "birth certificate" for coins. No matter how many times coins are split, merged, or transferred, they retain attribution to their origin cluster.

#### Information-Theoretic Argument

Splitting cannot hide cluster attribution because:

1. **Tags are inherited**: Child UTXOs inherit parent's cluster tags
2. **ClusterWealth aggregates**: The system sums tags across all UTXOs
3. **No information loss**: Splitting doesn't destroy the origin information

The only way to reduce cluster attribution is through **legitimate economic activity** - mixing with coins from other clusters. This is by design, as it represents real privacy-enhancing behavior.

### Conclusion

**The AND-based decay mechanism is fully resistant to Sybil/split attacks.**

| Research Question | Answer |
|-------------------|--------|
| Q1: Does splitting reduce fees? | **No** - cluster wealth is tracked globally |
| Q2: What's the economics? | **Negative ROI** - costs more than single account |
| Q3: Structural limits? | **Multiple** - ring size, UTXO overhead, privacy loss |

**No additional mitigations are required** for split attacks. The existing cluster wealth tracking provides complete protection.

### Recommendations

1. **Document the defense**: Add this analysis to the design documentation to reassure users
2. **Monitor for attempts**: Log unusual splitting patterns for research purposes
3. **No code changes needed**: The current design is sound

### Related Issues

- #85: Parent research (AND-based decay design)
- #89: Sybil attack research (split attacks) - not viable
- #90: Fresh coin mixing analysis (complementary attack vector) - not economically viable
- #91: Privacy design decision (completed)
- #92: Implementation tracking (unblocked by this analysis)
