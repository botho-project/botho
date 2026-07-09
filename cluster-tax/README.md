# Cluster Tax: Progressive Economics for Botho

The `cluster-tax` crate implements Botho's economic layer: progressive transaction fees, provenance tracking, lottery-based redistribution, and monetary policy.

## Overview

Botho's economic system has four pillars:

1. **Tag Vectors** - Track coin provenance through cluster attribution
2. **Progressive Fees** - Size-based fees with wealth-dependent multipliers (1x-6x)
3. **Lottery Redistribution** - 80% of fees redistributed via a cluster-tilted lottery, 20% burned
4. **Two-Phase Monetary Policy** - Halving schedule (5 yearly epochs, ~611M BTH) then tail emission (2% target)

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

| Decay Type | Trigger | Status |
|------------|---------|--------|
| **Age-based** | Spending a UTXO ≥ 720 blocks old (5% per eligible spend) | **Production** — stateless, no extra metadata leaked |
| **Block-based** | New blocks mined | Alternative kept for simulation |
| **AND-based** | Epochs (7 days) | Superseded by age-based decay (equivalent epoch caps emerge naturally) |

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
fee = fee_per_byte × tx_size × cluster_factor(sender_wealth) × output_penalty + memo_fees
```

| Component | Description |
|-----------|-------------|
| `fee_per_byte` | Base rate (default: 1 picocredit/byte) |
| `tx_size` | Transaction size in bytes |
| `cluster_factor` | 1x-6x based on sender's cluster wealth |
| `output_penalty` | `min(outputs, 10)²` — quadratic anti-UTXO-farming penalty, capped at 100x |
| `memo_fees` | Flat fee per encrypted memo (default: 100 picocredits) |

### Transaction Types

| Type | Ring Signature | Typical Size | Use Case |
|------|---------------|--------------|----------|
| **Hidden** (CLSAG) | ~700 bytes | 2-4 KB | Standard private transactions |
| **Minting** | N/A | ~1.5 KB | Block rewards (no fee) |

*Note: LION post-quantum ring signatures were deprecated in ADR-0001. Quantum resistance is provided via ML-KEM-768 stealth addresses and ML-DSA-65 transaction authorization.*

### Progressive Taxation

The cluster factor curve is a sigmoid in the **logarithm** of cluster wealth (integer-only fixed point for consensus determinism), with its midpoint pinned at 100,000 BTH:

```
cluster_factor(W) = 1 + 5 × sigmoid((log2(W) − log2(w_mid)) / width),  w_mid = 100,000 BTH
```

| Sender's Cluster Wealth | Cluster Factor | Effect |
|-------------------------|----------------|--------|
| ≲ 1K BTH | ~1.0x | Base fee only |
| ~100K BTH (midpoint) | ~3.5x | Moderate premium |
| ≳ 10M BTH | ~6.0x | Maximum premium |

```rust
use bth_cluster_tax::{FeeConfig, TransactionType};

const PICO: u128 = 1_000_000_000_000; // picocredits per BTH

let config = FeeConfig::default();

// Small cluster (1K BTH): pays base fee (2-output penalty = 4x)
let fee = config.compute_fee(TransactionType::Hidden, 2000, 1_000 * PICO, 0);

// Whale cluster (10M BTH): pays the 6x premium on top
let fee = config.compute_fee(TransactionType::Hidden, 2000, 10_000_000 * PICO, 0);
```

## Fee Distribution: Lottery + Burn

Fees are split between lottery redistribution and burning:

```
┌─────────────────────────────────────────────────────────┐
│                    Transaction Fee                       │
├─────────────────────────────────────┬───────────────────┤
│         80% Lottery Pool            │    20% Burned     │
│   (cluster-tilted redistribution)   │  (deflationary)   │
└─────────────────────────────────────┴───────────────────┘
```

### Lottery Selection Modes

| Mode | Sybil Resistance | Progressive | Status |
|------|------------------|-------------|--------|
| **ClusterWeighted** | High (value-based) | Yes (tilt via cluster factor) | **Production default** |
| **ValueWeighted** | High (1x) | None | Baseline for comparison |
| **Uniform** | Low (~9x gaming advantage) | High | Simulation only — vulnerable |
| **Hybrid** | Low-medium (α-dependent) | Medium | Simulation only |
| **EntropyWeighted** | Medium | Medium | Simulation only |

**Production mode is `ClusterWeighted`:** winner weight is `value ÷ cluster_factor`. It is the only mode whose progressive term is **split-invariant** — weights are value-based (splitting a position never increases total weight) and the tilt depends on cluster provenance, which inherits through splits. Adversarial simulation showed that per-UTXO weight terms (Uniform, the α component of Hybrid) are subsidies to whoever splits hardest: a strategic whale splitting into 1,000 UTXOs captured the payout stream (~300x weight gain under Hybrid α=0.3). See [Cluster-Tilted Redistribution](../docs/design/cluster-tilted-redistribution.md) and `experiments/ANALYSIS.md`.

## Monetary Policy

### Two-Phase Model

**Phase 1: Halving (~5 years at full load; canonical schedule #351)**
- Initial reward: 50 BTH/block
- Halving interval: 6,307,200 blocks (~1 year at the 5-second full-load block time)
- 5 halvings total → ~611M BTH
- Monetary math assumes 5-second blocks; idle-network blocks are slower, stretching the schedule proportionally

```
Epoch 1:   50.00 BTH/block  → ~315.4M BTH
Epoch 2:   25.00 BTH/block  → ~157.7M BTH
Epoch 3:   12.50 BTH/block  →  ~78.8M BTH
Epoch 4:    6.25 BTH/block  →  ~39.4M BTH
Epoch 5:    3.125 BTH/block →  ~19.7M BTH
────────────────────────────────────────
Total Phase 1:              611.01M BTH
```

**Phase 2: Tail Emission**
- Supply-dependent tail reward (~1.94 BTH/block at tail onset)
- Target: 2% annual NET inflation
- Difficulty adjusts to hit inflation target
- Fee burns reduce effective inflation

**Demurrage** (stock-level component): wealthy-cluster coins accrue a holding charge paid at spend time — 2%/yr at the maximum 6x factor, scaled by `(factor − 1)/5`, so factor-1 coins pay zero. Disabled during the first halving epoch (bootstrap); proceeds flow into the lottery pool. See `src/demurrage.rs`.

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
    fee_per_byte: 1,               // 1 picocredit per byte
    fee_per_memo: 100,             // 100 picocredits per memo
    output_fee_exponent_scaled: 2000, // 2.0 = quadratic output penalty
    output_count_cap: 10,          // caps the penalty at 10² = 100x
    min_output_value: 1_000_000,   // dust floor (1e-6 BTH)
    cluster_curve: ClusterFactorCurve::default(),
    // The default curve: 1x-6x, log-domain sigmoid with its midpoint
    // pinned at w_mid_pico = 100,000 BTH (see ClusterFactorCurve docs;
    // the midpoint is a module constant, not a free parameter).
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
