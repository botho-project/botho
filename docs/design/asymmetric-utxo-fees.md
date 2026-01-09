# Asymmetric UTXO Fees: Design Proposal

## Status

**Research** - Requires simulation validation

## Overview

This document proposes a combined mechanism for progressive redistribution in a privacy-preserving system:

1. **Asymmetric fees**: Splitting UTXOs is expensive, consolidating is cheap
2. **Value-weighted lottery with floor**: Tickets scale with value, but everyone gets at least one
3. **Lottery eligibility decay**: Inactive UTXOs lose lottery weight over time
4. **Minimum UTXO size**: Caps the maximum splitting advantage

Together, these create economic incentives where wealthy actors face a progressive disadvantage regardless of their strategy, while remaining Sybil-resistant without requiring identity.

### The Core Innovation

Traditional approaches to progressive taxation in privacy-preserving systems fail because:
- We cannot identify wealth without breaking privacy
- Any fee structure based on observable properties gets arbitraged to equilibrium
- Sybil attacks defeat lottery-based redistribution

This proposal creates a **strategy-proof progressive mechanism**:

```
Wealthy actor's dilemma:

Option A: Keep many UTXOs
├── Higher transaction fees (more inputs = bigger tx)
├── Must pay to create/maintain them
├── Lottery eligibility decays if parked
└── Result: PAYS MORE FEES

Option B: Consolidate into few UTXOs
├── Lower transaction fees
├── But fewer lottery tickets
├── But tickets scale with value (no splitting advantage)
└── Result: LOTTERY DISADVANTAGE

Either way: Progressive effect achieved
```

## The Primary Threat: Parking Attack

Before detailing the mechanism, we must understand the attack it's designed to counter.

### The Parking Attack

```
Day 0:   Attacker splits 1M BTH → 1000 × 1K UTXOs
         Cost: split_penalty (one-time)

Day 1-N: UTXOs sit idle ("parked"), collecting lottery
         No transaction fees (not moving)
         Expected winnings: 1000 × daily_lottery_rate

Day N+1: Consolidate when actually need to move
         Cost: cheap (consolidation discount)

Profit = (N × daily_winnings) - split_cost - consolidation_cost
```

**Key insight**: The attack exploits the gap between:
- Split cost (one-time, at creation)
- Lottery winnings (continuous, while parked)

If parking duration is long enough, lottery winnings exceed split cost.

### Why Transaction Frequency Matters

| Holder Type | Tx Frequency | Optimal Strategy | System Response |
|-------------|--------------|------------------|-----------------|
| Active trader | High | Consolidate | Low lottery, low fees |
| Normal user | Medium | Natural mix | Normal lottery |
| HODLer/Parker | Very low | Split and park | **MUST BE COUNTERED** |

The parking attack specifically targets low-frequency holders who can afford to wait.

## The Combined Mechanism

### Component 1: Asymmetric Fee Structure

| Transaction Type | Fee Treatment | Rationale |
|------------------|---------------|-----------|
| **Split** (1 → many outputs) | EXPENSIVE | Discourage artificial UTXO creation |
| **Consolidate** (many → 1 output) | CHEAP | Encourage rational consolidation |
| **Transfer** (1 → 1) | CHEAP | Don't penalize simple transfers |
| **Commerce** (N → 2) | NORMAL | Don't penalize normal payments |

```rust
fn calculate_structure_fee(tx: &Transaction) -> u64 {
    let base_fee = BASE_FEE_PER_BYTE * tx.size();
    let input_count = tx.inputs.len();
    let output_count = tx.outputs.len();

    if output_count > input_count + ALLOWED_EXTRA_OUTPUTS {
        // Splitting: penalize each extra output
        let extra = output_count - input_count - ALLOWED_EXTRA_OUTPUTS;
        base_fee * (1 + extra * SPLIT_PENALTY_MULTIPLIER)
    } else if output_count < input_count {
        // Consolidating: discount
        base_fee * CONSOLIDATION_DISCOUNT  // e.g., 0.3
    } else {
        // Normal: no adjustment
        base_fee
    }
}
```

**Effect**: Creates one-time cost for splitting, making lottery gaming require upfront investment.

### Component 2: Value-Weighted Lottery with Floor

This is the key innovation that dramatically reduces the splitting advantage.

```rust
const TICKET_THRESHOLD: u64 = 1_000_000_000_000; // 1000 BTH in picocredits
const MIN_TICKETS: u64 = 1;

fn lottery_tickets(utxo: &Utxo) -> u64 {
    max(MIN_TICKETS, utxo.value / TICKET_THRESHOLD)
}
```

**Why this works:**

```
Scenario: 1M BTH holdings

Unsplit (1 UTXO × 1M BTH):
    tickets = max(1, 1M / 1K) = 1000 tickets

Split into 1000 × 1K BTH:
    tickets = 1000 × max(1, 1K / 1K) = 1000 tickets

SAME NUMBER OF TICKETS - no splitting advantage above threshold!
```

**But still progressive:**

```
Poor holder (100 BTH, below threshold):
    tickets = max(1, 100/1000) = 1 ticket
    tickets_per_BTH = 1/100 = 0.01

Wealthy holder (1M BTH):
    tickets = max(1, 1M/1000) = 1000 tickets
    tickets_per_BTH = 1000/1M = 0.001

Poor gets 10x more lottery weight per BTH held!
```

### Component 3: Lottery Eligibility Decay

Counters the parking attack by reducing lottery weight for inactive UTXOs.

```rust
const DECAY_RATE_PER_DAY: f64 = 0.03;  // 3% daily decay
const MIN_ELIGIBILITY: f64 = 0.1;      // Floor at 10% weight

fn lottery_eligibility(utxo: &Utxo, current_block: u64) -> f64 {
    let age_days = (current_block - utxo.last_activity_block) / BLOCKS_PER_DAY;
    let decay = (1.0 - DECAY_RATE_PER_DAY).powf(age_days as f64);
    max(MIN_ELIGIBILITY, decay)
}

fn effective_lottery_tickets(utxo: &Utxo, current_block: u64) -> f64 {
    lottery_tickets(utxo) as f64 * lottery_eligibility(utxo, current_block)
}
```

**Effect on parking attack:**

```
Day 0:   Attacker parks 1000 UTXOs
         Eligibility: 100%

Day 30:  Eligibility: (0.97)^30 ≈ 40%
         Effective tickets: 40% of original

Day 100: Eligibility: (0.97)^100 ≈ 5% (hits floor at 10%)
         Effective tickets: 10% of original
```

To maintain full eligibility, must transact (which costs fees).

### Component 4: Minimum UTXO Size

Caps the maximum splitting advantage from the floor.

```rust
const MIN_UTXO_VALUE: u64 = 100_000_000_000; // 100 BTH in picocredits
```

**Why this matters:**

Without minimum UTXO:
```
Attack: Split 1M BTH into 1,000,000 × 1 BTH
Each gets floor of 1 ticket
Total: 1,000,000 tickets (vs 1000 unsplit)
1000x advantage!
```

With minimum UTXO of 100 BTH:
```
Maximum split: 1M BTH → 10,000 × 100 BTH
Each gets floor of 1 ticket
Total: 10,000 tickets (vs 1000 unsplit)
Only 10x advantage - much easier to counter with split penalty
```

## Combined Mechanism Analysis

### The Math Improves Dramatically

**Pure uniform lottery (original vulnerability):**
```
Split 1M into 1000 UTXOs: 1000x ticket advantage
Required penalty: Must overcome 1000x expected lottery value
Penalty needed: ~2,374 BTH per output (very high)
```

**Value-weighted with floor + min UTXO:**
```
Split 1M into 10,000 × 100 BTH: 10x ticket advantage
Required penalty: Must overcome 10x expected lottery value
Penalty needed: ~24 BTH per output (100x smaller!)
```

**With eligibility decay:**
```
Parked UTXOs lose ~3% weight per day
After 30 days: 40% of original weight
Effective advantage: 10x × 0.4 = 4x
Penalty needed: Even smaller
```

### Strategy-Proof Properties

| Wealthy Strategy | Fee Impact | Lottery Impact | Net Effect |
|------------------|------------|----------------|------------|
| Consolidate | Low fees | Few tickets, but proportional | Lottery disadvantage |
| Keep many UTXOs | High fees (big txs) | More tickets, but capped | Fee disadvantage |
| Split aggressively | Split penalty | 10x max advantage | Penalty > advantage |
| Park for lottery | One-time split cost | Eligibility decays | Decays to unprofitable |

**No winning strategy for concentrated wealth.**

### Progressive Redistribution Flow

```
Fee Collection:
├── Wealthy (consolidated): Low fees per tx, but large values
├── Wealthy (fragmented): High fees per tx (many inputs)
├── Small holders: Normal fees
└── Result: Wealthy contribute disproportionately

Lottery Distribution:
├── Weighted by tickets (value / threshold, floor of 1)
├── Small holders: 1 ticket per UTXO (high tickets/BTH ratio)
├── Wealthy: tickets proportional to value (low tickets/BTH ratio)
└── Result: Small holders win disproportionately

Net: Wealth flows from concentrated to distributed holdings
```

## Fee Formula Design

### Recommended Implementation

```rust
fn total_fee(tx: &Transaction) -> u64 {
    let base = BASE_FEE_PER_BYTE * tx.size();

    // Factor 1: Minting proximity (existing mechanism)
    let cluster_factor = calculate_cluster_factor(tx);

    // Factor 2: UTXO structure (new mechanism)
    let structure_factor = calculate_structure_factor(tx);

    base * cluster_factor * structure_factor
}

fn calculate_structure_factor(tx: &Transaction) -> f64 {
    let input_count = tx.inputs.len();
    let output_count = tx.outputs.len();

    if output_count > input_count + ALLOWED_EXTRA_OUTPUTS {
        // Splitting
        let extra = output_count - input_count - ALLOWED_EXTRA_OUTPUTS;
        1.0 + (extra as f64 * SPLIT_PENALTY_MULTIPLIER)
    } else if output_count < input_count {
        // Consolidating
        CONSOLIDATION_DISCOUNT
    } else {
        1.0
    }
}
```

### Parameter Recommendations

| Parameter | Recommended Value | Rationale |
|-----------|-------------------|-----------|
| `SPLIT_PENALTY_MULTIPLIER` | 0.5 - 2.0 | Only needs to overcome ~10x advantage |
| `CONSOLIDATION_DISCOUNT` | 0.3 | 70% discount encourages consolidation |
| `ALLOWED_EXTRA_OUTPUTS` | 1 | Allows payment + change without penalty |
| `TICKET_THRESHOLD` | 1000 BTH | Balance between granularity and splitting incentive |
| `MIN_UTXO_VALUE` | 100 BTH | Caps splitting to 10x advantage |
| `DECAY_RATE_PER_DAY` | 0.03 | Significant decay over weeks, not days |
| `MIN_ELIGIBILITY` | 0.1 | Floor prevents complete exclusion |

## Lottery Design

### Selection Algorithm

```rust
fn select_lottery_winner(utxo_set: &UtxoSet, current_block: u64) -> UtxoId {
    // Calculate effective tickets for each UTXO
    let mut cumulative_tickets = 0.0;
    let mut entries: Vec<(UtxoId, f64)> = Vec::new();

    for utxo in utxo_set.iter() {
        let tickets = effective_lottery_tickets(utxo, current_block);
        cumulative_tickets += tickets;
        entries.push((utxo.id, cumulative_tickets));
    }

    // Select winner
    let roll = random::<f64>() * cumulative_tickets;
    for (id, cumulative) in entries {
        if roll <= cumulative {
            return id;
        }
    }
    unreachable!()
}

fn effective_lottery_tickets(utxo: &Utxo, current_block: u64) -> f64 {
    let base_tickets = lottery_tickets(utxo) as f64;
    let eligibility = lottery_eligibility(utxo, current_block);
    base_tickets * eligibility
}

fn lottery_tickets(utxo: &Utxo) -> u64 {
    max(1, utxo.value / TICKET_THRESHOLD)
}

fn lottery_eligibility(utxo: &Utxo, current_block: u64) -> f64 {
    let age_blocks = current_block.saturating_sub(utxo.last_activity_block);
    let age_days = age_blocks / BLOCKS_PER_DAY;
    let decay = (1.0 - DECAY_RATE_PER_DAY).powf(age_days as f64);
    max(MIN_ELIGIBILITY, decay)
}
```

### Distribution

```rust
fn distribute_lottery(fee: u64, utxo_set: &UtxoSet, current_block: u64) {
    let pool = fee * LOTTERY_FRACTION;  // e.g., 80%
    let burned = fee - pool;

    let per_winner = pool / WINNERS_PER_TX;

    for _ in 0..WINNERS_PER_TX {
        let winner = select_lottery_winner(utxo_set, current_block);
        utxo_set.add_value(winner, per_winner);
        utxo_set.update_activity(winner, current_block);  // Refresh eligibility
    }

    // Burn remainder
    total_burned += burned;
}
```

## Edge Cases and Concerns

### 1. Dust and Small UTXOs

**Problem**: Very small UTXOs could be created to farm the floor ticket.

**Solutions**:
- Minimum UTXO size (100 BTH) prevents this
- UTXOs below threshold still get 1 ticket (progressive)
- Dust consolidation is cheap

### 2. Merchant Behavior

**Scenario**: Merchants receive many small payments.

**Analysis**: This is fine:
- Receiving payments: Free (recipient doesn't pay)
- Natural accumulation of UTXOs: Not penalized
- Consolidation when needed: Cheap
- Many UTXOs from commerce: Get lottery tickets (intended!)

### 3. Exchange Behavior

**Scenario**: Exchanges consolidate deposits, then create many withdrawals.

**Analysis**:
- Consolidation: Cheap (fine)
- Many withdrawal outputs: Triggers split penalty

**Mitigation options**:
- Accept that exchanges pay (passed to users as withdrawal fee)
- Batch withdrawals to minimize outputs
- No special treatment (maintains permissionlessness)

### 4. Privacy Impact

**Concern**: If wealthy consolidate, fewer UTXOs overall.

**Mitigation**:
- Small holders maintain many UTXOs (no incentive to consolidate)
- Minimum UTXO prevents extreme consolidation
- Natural commerce creates UTXOs continuously
- Value-binned anonymity sets for decoy selection

**Monitoring required**: Track UTXO count in simulation.

### 5. Activity Refresh Gaming

**Concern**: Could someone refresh eligibility cheaply?

**Analysis**:
- Self-transfer refreshes eligibility
- But costs transaction fee
- And doesn't change lottery tickets
- Net effect: Pays fees to maintain eligibility (intended!)

## Interaction with Minting Proximity

The two mechanisms are complementary:

```
Minting proximity fees:
├── Addresses: Early miner concentration
├── Mechanism: Tags persist from minting origin
├── Effective against: Wealth from mining

Asymmetric UTXO fees + lottery:
├── Addresses: Secondary market accumulation
├── Mechanism: Behavioral incentives + redistribution
├── Effective against: Wealth from buying

Combined coverage:
├── Early miners: High fees from source_wealth
├── Secondary accumulators: Lottery disadvantage + fee pressure
├── Both concentration types addressed
└── Mechanisms don't conflict
```

## Validation Requirements

### Simulation Goals

1. **Confirm split penalty threshold**: At what penalty is splitting unprofitable?
2. **Measure Gini reduction**: How much redistribution occurs?
3. **Track privacy metrics**: How many UTXOs remain?
4. **Test parking attack**: Does eligibility decay defeat it?
5. **Verify merchant viability**: Are merchants penalized?

### Key Parameters to Sweep

```python
# Split penalty (now much smaller range needed)
SPLIT_PENALTY_MULTIPLIER = [0.1, 0.5, 1.0, 2.0, 5.0]

# Lottery parameters
TICKET_THRESHOLD = [100, 500, 1000, 5000]  # BTH
MIN_UTXO_VALUE = [10, 50, 100, 500]        # BTH

# Eligibility decay
DECAY_RATE_PER_DAY = [0.01, 0.03, 0.05, 0.10]
MIN_ELIGIBILITY = [0.0, 0.1, 0.2]
```

### Success Criteria

```
1. Gini reduction: Δgini > 0.05 (meaningful redistribution)

2. Sybil resistance:
   - Parking attack ROI < 1.0 (unprofitable)
   - Splitting attack ROI < 1.0 (unprofitable)

3. Privacy: min_anonymity_set > 20 (ring size)

4. Commerce: merchant_overhead < 2x baseline

5. Stability: UTXO count doesn't collapse
```

## Comparison with Alternatives

| Mechanism | Miner Wealth | Buyer Wealth | Privacy | Complexity | Gaming Resistance |
|-----------|--------------|--------------|---------|------------|-------------------|
| Minting proximity only | ✅ Strong | ❌ None | ✅ Full | Medium | ✅ Strong |
| Pure split penalty | ✅ Strong | ⚠️ Moderate | ⚠️ Reduced | Medium | ⚠️ Moderate |
| **Combined mechanism** | ✅ Strong | ✅ Strong | ⚠️ Reduced | High | ✅ Strong |
| Demurrage | ✅ Strong | ✅ Strong (flat) | ✅ Full | Low | ✅ Strong |
| Identity-based | ✅ Strong | ✅ Strong | ❌ Broken | High | ✅ Strong |

### Advantages of Combined Approach

1. **Strategy-proof**: No winning strategy for concentrated wealth
2. **Moderate penalties**: 10x advantage (not 1000x) means smaller penalties work
3. **Parking-resistant**: Eligibility decay defeats long-term gaming
4. **Privacy-compatible**: No identity required
5. **Complementary**: Works with minting proximity fees

### Disadvantages

1. **Complexity**: Four interacting mechanisms to understand
2. **Parameter sensitivity**: Requires careful tuning
3. **Privacy tradeoff**: Potentially fewer UTXOs (must monitor)
4. **Exchange friction**: Legitimate splits are expensive

## Implementation Path

### Phase 1: Simulation
- Build simulation with all four components
- Test parameter ranges
- Identify viable configurations
- Measure privacy impact

### Phase 2: Analysis
- Formal game-theoretic analysis
- Privacy impact assessment
- Security review of interactions

### Phase 3: Specification
- Detailed protocol specification
- Consensus rule changes
- Migration plan from current system

### Phase 4: Implementation
- Code implementation
- Comprehensive testing
- Testnet deployment and observation

## Open Questions

1. **Optimal ticket threshold?** Balance between progressivity and splitting incentive.

2. **Decay rate tuning?** Fast enough to defeat parking, slow enough for normal users.

3. **Minimum UTXO floor?** Balance between splitting cap and commerce usability.

4. **Privacy impact magnitude?** Need simulation to quantify UTXO distribution.

5. **Exchange accommodation?** Accept fees or create special treatment?

## References

- [Minting Proximity Fees](minting-proximity-fees.md) - Complementary mechanism for miner wealth
- [Lottery Redistribution](lottery-redistribution.md) - Current lottery analysis
- [Progressive Fees](../concepts/progressive-fees.md) - Fee curve implementation
- [Asymmetric Fees Simulation](asymmetric-fees-simulation.md) - Simulation specification

## Changelog

- 2026-01-09: Initial proposal
- 2026-01-09: Added parking attack analysis, value-weighted lottery with floor, eligibility decay, minimum UTXO size. Significantly reduced required split penalty.
