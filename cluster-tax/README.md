# Cluster Tax: Progressive Transaction Fees for Botho

Botho implements a **size-based progressive fee model** that scales with transaction size while applying progressive taxation to discourage wealth concentration.

## The Problem

Traditional cryptocurrencies face two challenges:

1. **Transaction size externalities**: Larger transactions consume more network resources (bandwidth, storage, verification) but flat fees don't reflect this cost difference.

2. **Wealth concentrates over time**: Without intervention, cryptocurrency wealth follows power-law distributions where the rich get richer through compound effects.

## The Solution

Botho addresses both problems with a single fee structure:

```
fee = fee_per_byte × tx_size × cluster_factor(sender_wealth)
```

Where:
- `fee_per_byte` is the base rate per byte (e.g., 1 nanoBTH/byte)
- `tx_size` is the transaction size in bytes
- `cluster_factor` ranges from 1x to 6x based on sender's cluster wealth

### Transaction Types

| Type | Ring Signature | Typical Size | Description |
|------|---------------|--------------|-------------|
| **Standard-Private** | CLSAG (~700B) | ~2-3 KB | Efficient classical ring signatures |
| **PQ-Private** | LION (~63 KB) | ~65-70 KB | Post-quantum ring signatures |
| **Minting** | N/A | ~1 KB | Block reward claims (no fee) |

### Size-Based Pricing

PQ-Private transactions naturally cost more because they're larger:
- CLSAG signature: ~700 bytes per input
- LION signature: ~63 KB per input

This ensures fair pricing - you pay for what you use.

### Progressive Taxation

All transaction types apply the same cluster factor curve:

```
cluster_factor(W) = 1 + 5 × sigmoid((W - w_mid) / steepness)
```

This ensures:
- **Small holders pay ~1x** (just the size-based fee)
- **Large holders pay up to 6x** (progressively taxed)
- **Smooth transition** around the midpoint

## Economic Impact

Agent-based simulations show this fee structure can **reduce inequality by ~48% over 10 years** through the burn mechanism alone (no redistribution required).

![Botho Fee Model Simulation Results](gini_10yr/botho_fee_model.png)

| Metric | Result |
|--------|--------|
| Initial GINI | 0.788 |
| Final GINI | 0.409 |
| Reduction | 48.1% |

See [scripts/README.md](scripts/README.md) for detailed simulation methodology and results.

## Implementation

The fee calculation is implemented in [`src/fee_curve.rs`](src/fee_curve.rs):

```rust
use bth_cluster_tax::{FeeConfig, TransactionType};

let config = FeeConfig::default();

// Small Standard-Private transaction (2 KB, small holder)
let fee = config.compute_fee(TransactionType::Hidden, 2000, 1_000, 0);
// fee ≈ 2000 nanoBTH (1x cluster factor)

// Same transaction, large holder (cluster_wealth = 100M)
let fee = config.compute_fee(TransactionType::Hidden, 2000, 100_000_000, 0);
// fee ≈ 12000 nanoBTH (6x cluster factor)

// PQ-Private transaction (~65 KB, small holder)
let fee = config.compute_fee(TransactionType::PqHidden, 65000, 1_000, 0);
// fee ≈ 65000 nanoBTH

// Minting transactions are free
let fee = config.compute_fee(TransactionType::Minting, 1000, 0, 0);
// fee = 0
```

## Cluster Wealth Tracking

"Cluster wealth" is the total value associated with a sender's transaction graph cluster. This is tracked through:

1. **Output linking**: When outputs are spent together, they're linked to the same cluster
2. **Decay over time**: Cluster associations decay as coins change hands
3. **Privacy preservation**: For private transactions, cluster wealth is estimated from ring member analysis

## Design Rationale

### Why size-based fees?

Size-based fees naturally capture the cost differences between transaction types:
- Larger transactions use more network bandwidth
- Larger transactions require more storage
- Larger signatures take longer to verify

This is fairer than flat fees and avoids arbitrary rate tables.

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
    fee_per_byte: 1,              // 1 nanoBTH per byte
    fee_per_memo: 100,            // 100 nanoBTH per memo
    cluster_curve: ClusterFactorCurve {
        factor_min: 100,          // 1x minimum (100 = 1.00x)
        factor_max: 600,          // 6x maximum (600 = 6.00x)
        w_mid: 10_000_000,        // Sigmoid midpoint
        steepness: 5_000_000,     // Transition smoothness
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
