# Cluster Tax: Progressive Transaction Fees for Botho

Botho implements a **dual-incentive fee model** that prices privacy as a resource while applying progressive taxation to reduce wealth inequality.

## The Problem

Traditional cryptocurrencies face two challenges:

1. **Privacy is underpriced**: Private transactions impose real costs (verification work, storage) but are often priced the same as transparent transactions, leading to overuse or requiring arbitrary limits.

2. **Wealth concentrates over time**: Without intervention, cryptocurrency wealth follows power-law distributions where the rich get richer through compound effects.

## The Solution

Botho addresses both problems with a single fee structure:

```
fee_rate = base_rate × cluster_factor(sender_wealth)
```

Where:
- `base_rate` differs by transaction type (reflecting actual costs)
- `cluster_factor` ranges from 1x to 6x based on sender's cluster wealth

### Transaction Types

| Type | Base Rate | Min Fee | Max Fee | Description |
|------|-----------|---------|---------|-------------|
| **Plain** | 5 bps | 0.05% | 0.30% | Transparent transactions |
| **Hidden** | 20 bps | 0.20% | 1.20% | Private (ring signatures + bulletproofs) |
| **Minting** | 0 | 0% | 0% | Block reward claims |

### The 4x Privacy Premium

Hidden transactions cost 4x more than plain transactions. This reflects:

- **~10x verification cost**: Ring signature and bulletproof verification vs simple signature check
- **~10x storage cost**: ~2.5KB transaction size vs ~250 bytes
- **Averaged to 4x**: Keeps privacy accessible while pricing the resource fairly

### Progressive Taxation

Both transaction types apply the same cluster factor curve:

```
cluster_factor(W) = 1 + 5 × sigmoid((W - w_mid) / steepness)
```

This ensures:
- **Small holders pay ~1x** (just the base fee)
- **Large holders pay up to 6x** (heavily taxed)
- **Smooth transition** around the midpoint

**Critical insight**: Applying progressive fees to BOTH transaction types prevents large holders from avoiding taxation by choosing plain transactions.

## Economic Impact

Agent-based simulations show this fee structure can **reduce inequality by ~48% over 10 years** through the burn mechanism alone (no redistribution required).

![Botho Fee Model Simulation Results](gini_10yr/botho_fee_model.png)

| Metric | Result |
|--------|--------|
| Initial GINI | 0.788 |
| Final GINI | 0.409 |
| Reduction | 48.1% |

Compared to a flat 1% fee that achieves similar reduction, progressive fees are **~4.5x more efficient** - achieving the same inequality reduction while burning only 22% as many total fees.

See [scripts/README.md](scripts/README.md) for detailed simulation methodology and results.

## Implementation

The fee calculation is implemented in [`src/fee_curve.rs`](src/fee_curve.rs):

```rust
use cluster_tax::{FeeConfig, TransactionType};

let config = FeeConfig::default();

// Small holder (cluster_wealth = 1000): pays near-minimum rate
let (fee, net) = config.compute_fee(TransactionType::Plain, 10_000, 1_000);
// fee ≈ 8 (0.08%), net ≈ 9,992

// Large holder (cluster_wealth = 100M): pays near-maximum rate
let (fee, net) = config.compute_fee(TransactionType::Plain, 10_000, 100_000_000);
// fee ≈ 30 (0.30%), net ≈ 9,970

// Hidden transaction (4x base rate)
let (fee, net) = config.compute_fee(TransactionType::Hidden, 10_000, 100_000_000);
// fee ≈ 120 (1.20%), net ≈ 9,880
```

## Cluster Wealth Tracking

"Cluster wealth" is the total value associated with a sender's transaction graph cluster. This is tracked through:

1. **Output linking**: When outputs are spent together, they're linked to the same cluster
2. **Decay over time**: Cluster associations decay as coins change hands
3. **Privacy preservation**: For hidden transactions, cluster wealth is estimated from ring member analysis

## Design Rationale

### Why not just flat fees?

Flat fees treat all participants equally regardless of their impact on the network. Progressive fees recognize that:

- Large holders benefit more from network security (more to protect)
- Large holders generate more transaction volume
- Concentrated wealth creates systemic risks

### Why not redistribute fees?

While redistribution achieves faster inequality reduction, burning has advantages:

- **Simpler**: No complex distribution mechanism
- **Deflationary**: Creates scarcity, benefiting all holders proportionally
- **No gaming**: Can't farm redistribution by creating fake small accounts

### Why 1x-6x and not more aggressive?

The 6x maximum was chosen to:

- Achieve meaningful inequality reduction (~48% over 10 years)
- Avoid punitive fees that drive large holders away
- Keep the system usable for legitimate large transactions

Simulations show diminishing returns beyond 6x - a 10x maximum only improves reduction by ~0.7%.

## Configuration

Default parameters can be adjusted for different economic goals:

```rust
let config = FeeConfig {
    plain_base_fee_bps: 5,      // 0.05% base
    hidden_base_fee_bps: 20,    // 0.20% base (4x plain)
    cluster_curve: ClusterFactorCurve {
        factor_min: 1,          // 1x for small clusters
        factor_max: 6,          // 6x for large clusters
        w_mid: 10_000_000,      // Sigmoid midpoint
        steepness: 5_000_000,   // Transition smoothness
        ..Default::default()
    },
};
```

## Testing

```bash
cargo test -p bth-cluster-tax
```

## License

See repository root for license information.
