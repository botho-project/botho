# Cluster Tax: Progressive Economics for Botho

The `cluster-tax` crate implements Botho's economic layer: progressive transaction fees, provenance tracking, lottery-based redistribution, and monetary policy.

## Overview

Botho's economic system has four pillars:

1. **Tag Vectors** - Track coin provenance through cluster attribution
2. **Progressive Fees** - Size-based fees with wealth-dependent multipliers (1x-6x)
3. **Lottery Redistribution** - 80% of fees redistributed to random UTXOs, 20% burned
4. **Two-Phase Monetary Policy** - Halving schedule (years 0-10) then tail emission (2% target)

## Key Concepts

### Clusters and Tag Vectors

Every UTXO carries a **tag vector** tracking what fraction of its value traces back to each origin cluster:

```rust
use bth_cluster_tax::{TagVector, TAG_WEIGHT_SCALE};

// Fresh minted coin: 100% attributed to minter's cluster
let minted = TagVector::single(cluster_id);

// After trading: mixed provenance from multiple sources
let traded = TagVector::from_weights(&[
    (alice_cluster, 400_000),  // 40% from Alice's cluster
    (bob_cluster, 350_000),    // 35% from Bob's cluster
    // remaining 25% is "background" (fully diffused)
]);
```

**Key properties:**
- Weights sum to at most `TAG_WEIGHT_SCALE` (1,000,000 = 100%)
- Remainder is "background" - value that has fully diffused through commerce
- Maximum 32 tags per vector (oldest/smallest pruned)

### Decay Mechanisms

Tags decay over time, reflecting that provenance information becomes less relevant as coins change hands:

| Decay Type | Trigger | Effect |
|------------|---------|--------|
| **Age-based** | Time passing | Old coins lose cluster attribution |
| **Block-based** | New blocks mined | Attribution fades with chain growth |
| **AND-based** | Epochs (7 days) | Wash trading resistance via epoch caps |

```rust
use bth_cluster_tax::{AndDecayConfig, AndTagVector};

let config = AndDecayConfig::default();
let mut tags = AndTagVector::new(initial_tags, current_block);

// Apply decay when spending
tags.apply_decay(current_block, &config);
```

### Cluster Entropy

The **entropy** of a tag vector measures provenance diversity:

```rust
// Fresh mint: entropy = 0 (single source)
// Self-split: entropy ≈ 0.5-1.0 (same sources)
// Commerce coin: entropy ≈ 1.5-2.5 (mixed sources)
// Exchange coin: entropy ≈ 2.5-3.5 (highly mixed)

let entropy = tags.cluster_entropy(); // Decay-invariant
```

**Important:** Use `cluster_entropy()` (excludes background) not `shannon_entropy()` (includes background) for lottery selection. See [Provenance-Based Selection](../docs/design/provenance-based-selection.md).

## Fee Model

### Size-Based Progressive Fees

```
fee = fee_per_byte × tx_size × cluster_factor(sender_wealth)
```

| Component | Description |
|-----------|-------------|
| `fee_per_byte` | Base rate (default: 1 nanoBTH/byte) |
| `tx_size` | Transaction size in bytes |
| `cluster_factor` | 1x-6x based on sender's cluster wealth |

### Transaction Types

| Type | Ring Signature | Typical Size | Use Case |
|------|---------------|--------------|----------|
| **Hidden** (CLSAG) | ~700 bytes | 2-4 KB | Standard private transactions |
| **Minting** | N/A | ~1.5 KB | Block rewards (no fee) |

*Note: LION post-quantum ring signatures were deprecated in ADR-0001. Quantum resistance is provided via ML-KEM-768 stealth addresses and ML-DSA-65 transaction authorization.*

### Progressive Taxation

The cluster factor curve ensures wealthy clusters pay more:

```
cluster_factor(W) = 1 + 5 × sigmoid((W - w_mid) / steepness)
```

| Sender Wealth | Cluster Factor | Effect |
|---------------|----------------|--------|
| Small holder | ~1.0x | Base fee only |
| Medium holder | ~2-3x | Moderate premium |
| Large holder | ~6.0x | Maximum premium |

```rust
use bth_cluster_tax::{FeeConfig, TransactionType};

let config = FeeConfig::default();

// Small holder: pays base fee
let fee = config.compute_fee(TransactionType::Hidden, 2000, 1_000, 0);
// ≈ 2000 nanoBTH

// Large holder: pays 6x premium
let fee = config.compute_fee(TransactionType::Hidden, 2000, 100_000_000, 0);
// ≈ 12000 nanoBTH
```

## Fee Distribution: Lottery + Burn

Fees are split between lottery redistribution and burning:

```
┌─────────────────────────────────────────────────────────┐
│                    Transaction Fee                       │
├─────────────────────────────────────┬───────────────────┤
│         80% Lottery Pool            │    20% Burned     │
│  (redistributed to random UTXOs)    │  (deflationary)   │
└─────────────────────────────────────┴───────────────────┘
```

### Lottery Selection Modes

| Mode | Sybil Resistance | Progressive | Best For |
|------|------------------|-------------|----------|
| **Uniform** | Low (9x advantage) | High | N/A - vulnerable |
| **ValueWeighted** | High (1x) | None | Pure Sybil resistance |
| **EntropyWeighted** | Medium (6x) | Medium | Balanced approach |

**Recommended:** Entropy-weighted selection reduces Sybil advantage by ~35% while maintaining progressivity. See [Provenance-Based Selection](../docs/design/provenance-based-selection.md).

### Entropy Bonus Parameter

The `entropy_bonus` parameter controls lottery weight advantage for high-entropy (commerce) coins:

```
weight = value × (1 + entropy_bonus × cluster_entropy)
```

| `entropy_bonus` | Commerce Advantage | Fresh Mint Penalty |
|-----------------|-------------------|-------------------|
| 0.25 | 1.5x | -17% vs average |
| **0.50** (default) | 2.0x | -29% vs average |
| 1.00 | 3.0x | -44% vs average |

## Monetary Policy

### Two-Phase Model

**Phase 1: Halving (Years 0-10)**
- Initial reward: 50 BTH/block
- Halves every 2 years (5 halvings total)
- Total emission: ~100M BTH
- Difficulty targets 5-second block time

```
Year 0-2:   50.00 BTH/block  → ~52.6M BTH
Year 2-4:   25.00 BTH/block  → ~26.3M BTH
Year 4-6:   12.50 BTH/block  → ~13.1M BTH
Year 6-8:    6.25 BTH/block  →  ~6.6M BTH
Year 8-10:   3.125 BTH/block →  ~3.3M BTH
────────────────────────────────────────
Total Phase 1:               ~101.9M BTH
```

**Phase 2: Tail Emission (Year 10+)**
- Fixed tail reward (~1.94 BTH/block)
- Target: 2% annual NET inflation
- Difficulty adjusts to hit inflation target
- Fee burns reduce effective inflation

```rust
use bth_cluster_tax::{MonetaryPolicy, MonetaryState};

let policy = MonetaryPolicy::default();
let state = MonetaryState::genesis();

let reward = policy.block_reward(current_height);
let is_tail = policy.is_tail_emission(current_height);
```

### Key Insight

Instead of variable rewards (unpredictable for minters), difficulty adjusts to hit monetary targets:

```
net_inflation = gross_emission - fees_burned
              = (reward × blocks) - fees_burned

blocks_needed = (target_net + fees_burned) / reward
→ difficulty adjusts to produce this many blocks
```

## Simulation Framework

The crate includes comprehensive agent-based simulations:

```bash
# Run lottery simulation
cargo run --bin cluster-tax-sim --features cli -- lottery

# Run with custom parameters
cargo run --bin cluster-tax-sim --features cli -- lottery \
    --epochs 100 \
    --selection-mode entropy-weighted \
    --entropy-bonus 0.5
```

### Agent Types

| Agent | Behavior | Economic Role |
|-------|----------|---------------|
| **Retail** | Small, frequent transactions | Typical users |
| **Whale** | Large, infrequent transactions | Wealthy holders |
| **Merchant** | Receives many small payments | Commerce |
| **Minter** | Claims block rewards | Supply creation |
| **Mixer** | Consolidates/splits UTXOs | Privacy |
| **MarketMaker** | High-frequency trading | Liquidity |

### Metrics Tracked

- GINI coefficient (wealth inequality)
- Lottery win distribution
- Fee burden by wealth class
- Sybil attack profitability
- Network size effects

## Design Documents

For detailed analysis and rationale:

- **[Cluster Tag Decay](../docs/design/cluster-tag-decay.md)** - AND-based decay mechanism for wash trading resistance
- **[Lottery Redistribution](../docs/design/lottery-redistribution.md)** - Lottery design and Sybil analysis
- **[Provenance-Based Selection](../docs/design/provenance-based-selection.md)** - Entropy-weighted lottery selection

## Configuration

```rust
use bth_cluster_tax::{FeeConfig, ClusterFactorCurve};

let config = FeeConfig {
    fee_per_byte: 1,              // 1 nanoBTH per byte
    fee_per_memo: 100,            // 100 nanoBTH per memo
    cluster_curve: ClusterFactorCurve {
        factor_min: 100,          // 1.00x minimum
        factor_max: 600,          // 6.00x maximum
        w_mid: 10_000_000,        // Sigmoid midpoint
        steepness: 5_000_000,     // Transition smoothness
        ..Default::default()
    },
};
```

## Module Structure

| Module | Description |
|--------|-------------|
| `tag` | Tag vectors and cluster attribution |
| `fee_curve` | Progressive fee calculation |
| `monetary` | Two-phase monetary policy |
| `dynamic_fee` | Congestion-based fee adjustments |
| `transfer` | UTXO transfers with tag propagation |
| `crypto` | Committed tags for privacy |
| `simulation` | Agent-based economic simulations |
| `validate` | Transaction validation |

## Testing

```bash
# Unit tests
cargo test -p bth-cluster-tax

# With simulation tests (slower)
cargo test -p bth-cluster-tax --features cli

# Benchmarks
cargo bench -p bth-cluster-tax
```

## Economic Impact

Simulations show the combined fee + lottery system can significantly reduce wealth inequality:

| Metric | Value |
|--------|-------|
| Initial GINI | 0.788 |
| Final GINI (10 years) | 0.409 |
| Reduction | 48.1% |

The lottery mechanism provides additional redistribution beyond pure fee burning, though with trade-offs around Sybil resistance. See design documents for detailed analysis.

## License

See repository root for license information.
