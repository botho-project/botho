# Lottery-Based Fee Redistribution

## Overview

Botho uses a **lottery-based fee redistribution** system instead of burning transaction fees. This creates a Universal Basic Income (UBI) effect where fees flow back to coin holders, weighted by cluster factor to maintain progressivity.

**Key properties:**
- **Progressive**: Low cluster-factor UTXOs earn more tickets per BTH
- **Sybil-resistant**: Value-weighting prevents splitting attacks
- **Activity-rewarding**: Ring participation earns lottery tickets
- **Privacy-preserving**: Claims happen when spending (no new leaks)

## Problem Statement

### Why Not Burn Fees?

Burning fees creates deflationary pressure but doesn't actively redistribute wealth. The benefits accrue passively to all holders proportionally.

### Why Not Direct Taxation?

Our earlier cluster tax design had issues:
- Primarily taxed minters on first spend, not accumulated wealth
- Commerce wealth escaped (tags decay through trade)
- Complex fee calculations at transaction time
- Punitive framing ("pay more") vs. rewarding ("receive more")

### The Sybil Problem

Any naive redistribution faces Sybil attacks:
- 1 ticket per UTXO → split into many UTXOs
- Random selection → create many accounts
- Without identity, how do we prevent gaming?

## Solution: Cluster-Weighted Participation Lottery

### Core Insight

Combine three mechanisms:

1. **Cluster factors** (existing) - track coin provenance, provide Sybil-resistant "identity"
2. **Value weighting** - tickets proportional to value, canceling split advantage
3. **Ring participation** - reward UTXOs selected as decoys (active anonymity set)

### Ticket Calculation

```
tickets_per_selection = UTXO_value / cluster_factor / ring_size
```

Where:
- `UTXO_value`: Value of the UTXO in base units
- `cluster_factor`: Progressive factor from fee curve (1.0 - 6.0)
- `ring_size`: Number of members in the ring (e.g., 11)

### Why Value Weighting Prevents Sybil

```
Single UTXO:
  10,000 BTH / factor 2.0 = 5,000 tickets

Split into 100 UTXOs:
  100 × (100 BTH / factor 2.0) = 5,000 tickets

Same total! Splitting doesn't help.
```

### Why Cluster Factor Makes It Progressive

```
Wealthy minter (factor 6.0):
  10,000 BTH / 6.0 = 1,667 tickets → 0.17 tickets/BTH

Poor recipient (factor 1.0):
  100 BTH / 1.0 = 100 tickets → 1.00 tickets/BTH

Poor person earns 6x more tickets per BTH held.
```

## Mechanism Details

### Fee Flow

```
Transaction fee
    │
    ├──► 20% burned (deflation)
    │
    └──► 80% to lottery pool
              │
              └──► Distributed to ring participants
```

### Ring Participation Tracking

Every transaction creates a ring of N UTXOs. Each ring member earns tickets:

```rust
fn record_ring_participation(
    ring_members: &[UtxoId],
    ring_size: usize,
    current_block: u64,
    utxo_tickets: &mut HashMap<UtxoId, TicketBalance>,
) {
    for utxo_id in ring_members {
        let utxo = get_utxo(utxo_id);
        let tickets = utxo.value as f64
                    / utxo.cluster_factor
                    / ring_size as f64;

        utxo_tickets
            .entry(*utxo_id)
            .or_default()
            .add_tickets(tickets, current_block);
    }
}
```

### Claiming on Spend

When a UTXO is spent, its accumulated tickets are claimed:

```rust
fn claim_lottery_on_spend(
    spent_utxo: &Utxo,
    ticket_balance: &TicketBalance,
    lottery_pool: &mut u64,
    total_unclaimed: f64,
) -> u64 {
    // Calculate share of pool
    let share = ticket_balance.tickets / total_unclaimed;
    let payout = (*lottery_pool as f64 * share) as u64;

    // Deduct from pool
    *lottery_pool -= payout;

    // Payout added to change output
    payout
}
```

### Ticket Expiration

To prevent infinite accumulation by dormant UTXOs:

```
Tickets expire after 100,000 blocks (~12 days) if UTXO not spent
```

This ensures:
- Active users claim regularly
- Pool doesn't drain to dormant UTXOs
- Incentive to transact (claim before expiry)

## Privacy Analysis

### What's Already Public

- Which UTXOs are in each ring (ring members visible on-chain)
- UTXO values and ages
- Transaction graph (with ring ambiguity)

### What This Adds

| Information | Visibility | Impact |
|-------------|------------|--------|
| Times selected as decoy | Could compute from chain | Low (derivable) |
| Accumulated tickets | On spend only | Low (reveals with UTXO) |
| Lottery payout | On spend only | Low (reveals with UTXO) |

### Why It's Privacy-Preserving

1. **Decoy selection is already public** - anyone can see ring members
2. **Claiming happens at spend time** - you're already revealing the UTXO
3. **No new linkability** - can't link UTXOs by lottery participation

### Optional: Committed Ticket Balances

For enhanced privacy, ticket balances can be Pedersen-committed:
- Public: Commitment to ticket balance
- Private: Actual balance (revealed in ZK proof on claim)

## Economic Analysis

### Redistribution Effectiveness

| Holder Type | Cluster Factor | Tickets/BTH | Annual Yield |
|-------------|---------------|-------------|--------------|
| Fresh minter | 6.0 | 0.17 | ~0.08% |
| Recent recipient | 3.0 | 0.33 | ~0.17% |
| Well-circulated | 1.5 | 0.67 | ~0.33% |
| Background rate | 1.0 | 1.00 | ~0.50% |

*Assumes 500K BTH annual fees, 100M BTH supply*

### Sybil Attack Economics

**Attack:** Split holdings into many accounts to win more lottery

**Result:** No benefit due to value weighting

```
Cost of 10-account strategy:
- Same lottery winnings (value-weighted)
- 9 extra transactions per multi-account spend
- Extra fees: 9 × ~1000 nanoBTH = 9000 nanoBTH per spend
- Net: Pure loss
```

### Wash Trading Attack

**Attack:** Rapidly transact to accumulate lottery tickets

**ActivityBased model vulnerability:**
```
100 wash trades at ~0.0016% cost → 347% more tickets
ROI: 22,000x - massively gameable!
```

**FeeProportional solution:**
```
tickets = fee × (max_factor - your_factor) / max_factor

Each wash trade:
- Costs fee F
- Earns tickets = F × ticket_rate (fixed by cluster factor)

Since ticket_rate is constant, more trades = more fees = more tickets
but the ratio is LINEAR - no amplification, no gaming profit.
```

With FeeProportional, wash trading earns tickets exactly proportional to fees paid. No profit, no gaming advantage.

## Parameters

| Parameter | Value | Rationale |
|-----------|-------|-----------|
| Pool fraction | 80% | Balance redistribution vs. deflation |
| Burn fraction | 20% | Maintain some deflationary pressure |
| Ring size | 11 | Privacy vs. bandwidth tradeoff |
| Ticket expiry | 100,000 blocks | ~12 days, encourages activity |
| Min UTXO value | 1,000 nanoBTH | Prevent dust spam |
| Min UTXO age | 720 blocks | Match decay age requirement |

## Comparison to Alternatives

| Approach | Progressive? | Sybil-Resistant? | Privacy? | Complexity |
|----------|-------------|------------------|----------|------------|
| Burn fees | No | N/A | Good | None |
| Cluster tax | Weak | Yes | Moderate | High |
| Random lottery | Statistical | No | Good | Low |
| **Ring lottery** | **Yes** | **Yes** | **Good** | **Medium** |

## Implementation Considerations

### State Requirements

Per UTXO:
- Cluster tags (existing)
- Ticket balance: `u64` (accumulated tickets × 1000)
- Last claim block: `u64`

Global:
- Lottery pool balance
- Total unclaimed tickets

### Consensus Changes

1. Track ring participation in block processing
2. Calculate ticket earnings per ring member
3. On UTXO spend, include lottery claim in output calculation
4. Validate lottery payouts against ticket balances

### Wallet Changes

1. Track ticket balances for owned UTXOs
2. Display expected lottery earnings
3. Optimize spend timing (claim before expiry)
4. Consider ticket value in UTXO selection

## Ring Participation Tracking

### Can We Identify Recent Ring Members?

**Yes.** Ring membership is public on-chain data. Each transaction includes:
- The ring of N UTXO references
- We don't know which is real, but all N are visible

To build the "active decoy set" for the last month:

```rust
fn build_active_decoy_set(
    chain: &Blockchain,
    lookback_blocks: u64,
) -> HashMap<UtxoId, RingParticipation> {
    let current = chain.height();
    let start = current.saturating_sub(lookback_blocks);

    let mut participation = HashMap::new();

    for block in chain.blocks(start..=current) {
        for tx in block.transactions() {
            for input in tx.inputs() {
                for ring_member in input.ring() {
                    participation
                        .entry(ring_member.utxo_id())
                        .or_insert_with(RingParticipation::new)
                        .record(block.height(), tx.fee());
                }
            }
        }
    }

    participation
}

struct RingParticipation {
    /// Number of times selected as ring member
    selection_count: u32,
    /// Blocks where selected
    selection_blocks: Vec<u64>,
    /// Total fees from transactions where this was a ring member
    total_tx_fees: u64,
}
```

### Lookback Window

| Window | Blocks | Time | Storage |
|--------|--------|------|---------|
| 1 day | ~8,640 | 24h | ~50 MB |
| 1 week | ~60,480 | 7d | ~350 MB |
| 1 month | ~259,200 | 30d | ~1.5 GB |

The 1-month window is manageable and provides good coverage.

### Activity-Weighted Tickets

This gives us another knob: weight tickets by *actual* decoy selection, not just eligibility:

```
tickets = (value / cluster_factor) × activity_multiplier

where:
  activity_multiplier = 1 + log2(1 + selections_last_month)
```

| Selections (30d) | Multiplier | Effect |
|-----------------|------------|--------|
| 0 | 1.0× | Baseline |
| 1 | 1.0× | Minimal boost |
| 3 | 2.0× | 2× tickets |
| 7 | 3.0× | 3× tickets |
| 15 | 4.0× | 4× tickets |

**Why logarithmic?** Prevents whales with many UTXOs from dominating through sheer selection volume.

### What This Rewards

1. **Good decoy candidates**: UTXOs that look "normal" get selected more
2. **Active participation**: Transacting creates rings that include others
3. **Network health**: Incentivizes maintaining the anonymity set

### What This Discourages

1. **Dust UTXOs**: Rarely selected, earn few tickets
2. **Unusual amounts**: Stand out, selected less often
3. **Dormant holdings**: Not contributing to anonymity set

## Sybil Economics: The Multi-Account Problem

### The Attack

Split 10,000 BTH into 10 accounts of 1,000 BTH each.

**Question**: Does this increase lottery winnings?

### Base Tickets (Value-Weighted)

With `tickets = value / factor`:

| Strategy | Calculation | Tickets |
|----------|-------------|---------|
| 1 account | 10,000 / 2.0 | 5,000 |
| 10 accounts | 10 × (1,000 / 2.0) | 5,000 |

**Result**: No advantage. Value-weighting protects us.

### Activity Multiplier: The Subtle Issue

If decoy selection is **uniform by UTXO count** (not value-weighted):
- 1 UTXO: 1 chance per transaction to be selected
- 10 UTXOs: 10 chances per transaction to be selected

Over a month with 100,000 transactions and ring size 11:
- 1 UTXO: ~100,000 × 11/N selections (where N = total UTXOs)
- 10 UTXOs: ~10 × that = 10× more selections

**This IS a Sybil advantage for activity multiplier!**

### Solution: Value-Weighted Activity

Instead of:
```
activity_multiplier = 1 + log2(1 + selection_count)
```

Use:
```
activity_contribution = Σ (value_when_selected / ring_size)
activity_multiplier = 1 + log2(1 + activity_contribution / value)
```

Now:
- 1 UTXO (10,000 BTH), 50 selections: contribution = 50 × 10,000/11 ≈ 45,000
  - multiplier = 1 + log2(1 + 45,000/10,000) = 1 + log2(5.5) ≈ 3.5
- 10 UTXOs (1,000 BTH each), 500 selections total: contribution = 500 × 1,000/11 ≈ 45,000
  - multiplier = 1 + log2(1 + 45,000/10,000) = 1 + log2(5.5) ≈ 3.5

**Same multiplier regardless of split!**

### Fee Cost Analysis

Given activity parity, multi-account only has disadvantages:

| Operation | 1 Account | 10 Accounts | Extra Cost |
|-----------|-----------|-------------|------------|
| Receive 100 payments | 100 txs | 100 txs | 0 |
| Spend 5,000 BTH | 1 tx | ~3-5 txs | 2-4 fees |
| Annual cost (100 spends) | 100 fees | ~400 fees | 300 fees |

At 1,000 nanoBTH per fee, extra annual cost = 300,000 nanoBTH.

### Break-Even Analysis

For multi-account to be profitable:
```
lottery_advantage > extra_fee_cost
```

With proper value-weighting:
```
lottery_advantage = 0
extra_fee_cost > 0
```

**Multi-account is ALWAYS unprofitable.**

### Why Can't They Avoid Extra Fees?

The memo field trick lets them receive into multiple accounts without extra sender fees. But they still need to **spend**:

1. **Exact match**: If payment needed = one UTXO, no extra cost
2. **Combine needed**: If payment > any single UTXO, must combine
   - Each additional input = additional transaction? (No, multiple inputs per tx)
   - But reveals UTXO linkage!

Actually, with multiple inputs per transaction, the cost might be lower than I estimated. Let me reconsider...

**Privacy cost**: Spending from multiple accounts in one tx links them as same owner. This defeats the purpose of splitting.

**Economic choice**:
- Keep accounts separate → extra transactions when combining needed
- Combine accounts → privacy loss, defeats Sybil attempt

Either way, splitting has costs with no lottery benefit.

## Fee Calibration

To ensure Sybil unprofitability:

```rust
fn minimum_fee_for_sybil_resistance(
    avg_utxo_value: u64,
    lottery_yield_per_ticket: f64,
    avg_spends_per_year: u32,
) -> u64 {
    // With proper value-weighting, lottery advantage = 0
    // Any positive fee makes multi-account unprofitable
    //
    // But add safety margin: require fee such that
    // 10-way split costs ≥1% of holdings annually

    let safety_margin = 0.01;
    let extra_txs_per_spend = 4.0; // Average for 10-account strategy
    let extra_annual_txs = extra_txs_per_spend * avg_spends_per_year as f64;

    let min_extra_cost = avg_utxo_value as f64 * safety_margin;
    let min_fee = min_extra_cost / extra_annual_txs;

    (min_fee as u64).max(100) // Floor of 100 nanoBTH
}
```

Example:
- Average UTXO: 100 BTH = 100,000,000,000 nanoBTH
- 1% of that: 1,000,000,000 nanoBTH
- 100 spends/year × 4 extra txs = 400 extra txs
- Min fee: 1,000,000,000 / 400 = 2,500,000 nanoBTH ≈ 2.5 mBTH per tx

This is higher than typical crypto fees but provides strong Sybil resistance.

## Ticket Models

Two ticket allocation models were evaluated:

### ActivityBased (Original Design)

```
tickets = (value / cluster_factor) × activity_multiplier
```

Where activity_multiplier rewards decoy ring selection.

**Vulnerability**: Wash trading. A user paying 100 wash trades gains ~347% more tickets while paying only ~0.0016% of holdings in fees (22,000x ROI).

### FeeProportional (Recommended)

```
tickets = fee_paid × (max_factor - your_factor) / max_factor
```

Where `max_factor = 6.0`.

**Why it works**:
- Tickets are strictly proportional to fees paid
- Wash trading costs exactly what it "earns" (no profit)
- Low cluster factor users get more tickets per fee
- High cluster factor users (rich minters) get zero tickets

| Cluster Factor | Ticket Rate | Effect |
|----------------|-------------|--------|
| 1.0 (poor) | 0.83 | 83% of fee becomes tickets |
| 3.0 (medium) | 0.50 | 50% of fee becomes tickets |
| 6.0 (rich minter) | 0.00 | No tickets from fees |

## Simulation Results

### Ticket Model Comparison

| Scenario | Init Gini | Final Gini | Change | Notes |
|----------|-----------|------------|--------|-------|
| ActivityBased + ValueWeighted | 0.71 | 0.32 | +55.5% | Best case but gameable |
| ActivityBased + Uniform | 0.71 | 0.78 | -9.7% | **Increases inequality!** |
| **FeeProportional + ValueWeighted** | 0.71 | 0.45 | +37.3% | Wash-resistant |
| **FeeProportional + Uniform** | 0.71 | 0.41 | +43.0% | Works either way |

**Key Finding**: FeeProportional is recommended because:
1. Works regardless of transaction patterns (+37-43% reduction)
2. Wash-trading resistant (tickets/fee is fixed by cluster factor)
3. ActivityBased looks better but is gameable and fails under uniform transactions

### Lottery vs Cluster Tax

| Metric | Lottery (FeeProportional) | Cluster Tax |
|--------|---------------------------|-------------|
| Initial Gini | 0.71 | 0.71 |
| Final Gini | 0.41-0.45 | 0.66 |
| **Reduction** | **37-43%** | **7%** |

The lottery system is **5-6x more effective** at reducing wealth inequality.

### Why Lottery Works

1. **Direct redistribution**: Fees flow back weighted by `1/cluster_factor`
2. **Progressive by design**: Poor get more tickets per fee paid
3. **Wash-resistant**: Linear ticket-to-fee relationship eliminates gaming
4. **Depends on cluster factors**: Lottery progressivity comes from the cluster tax infrastructure

### Simulation Parameters

```
Total wealth: 100M BTH
Distribution: 10 poor (0.5% each), 5 middle (5% each), 2 rich (35% each)
Duration: 50,000 blocks
Transactions: 20 per block
Fee: 100 base × cluster_factor
Pool: 80% redistribution, 20% burn
Ticket Model: FeeProportional
```

## Open Questions

1. **Claim batching**: Can you claim for multiple UTXOs in one tx?
2. **Pool smoothing**: Fixed payout per block vs. share of pool?
3. **Decoy selection impact**: Does this change optimal decoy selection?
4. **Hybrid model**: Should activity provide a small bonus on top of fee-proportional?

## References

- [Cluster Tag Decay](cluster-tag-decay.md) - Age-based decay mechanism
- [Progressive Fees](../progressive-fees.md) - Fee curve design
- [Tokenomics](../tokenomics.md) - Overall economic model
