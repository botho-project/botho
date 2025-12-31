# Cluster Tag Decay: Design Specification

## Overview

This document specifies the **AND-based decay with epoch cap** mechanism for cluster tag decay in Botho's progressive fee system. This design prevents wash trading attacks while ensuring wealthy clusters cannot passively reduce their tax burden.

## Problem Statement

### Background

Botho uses cluster tags to track the lineage of coins back to their minting origin. Wealthy clusters (those controlling large amounts of minted value) pay higher transaction fees via a progressive fee curve.

### Attack Vector: Wash Trading

Under naive hop-based decay (X% decay per transfer), an attacker can:
1. Send coins to themselves repeatedly
2. Each self-transfer decays the cluster tag
3. After N transfers: tag_remaining = (1 - decay_rate)^N
4. With 5% decay: 100 transfers → 0.6% tag remaining → 85% fee reduction

This attack is cheap (only base fees) and fast (can execute in minutes).

### Design Goals

1. **Resist rapid wash trading** - Self-transfers in quick succession should not accelerate decay
2. **Resist patient wash trading** - Spreading attacks over time should have bounded effect
3. **No passive decay** - Holding coins without trading should not reduce cluster attribution
4. **Enable legitimate privacy** - Real economic activity should still enable tag diffusion

## Mathematical Model

### Definitions

| Symbol | Definition | Example Value |
|--------|------------|---------------|
| `d` | Decay rate per eligible hop | 0.05 (5%) |
| `Δt_min` | Minimum blocks between decay events | 720 (~2 hours) |
| `N_epoch` | Maximum decays per epoch | 12 |
| `T_epoch` | Epoch length in blocks | 8,640 (~1 day) |
| `w(t)` | Tag weight at time t | [0, 1] |
| `n(t)` | Cumulative transfer count by time t | ≥ 0 |

### Decay Eligibility

A transfer at block `b` triggers decay if and only if:

```
eligible(b) = (b - b_last ≥ Δt_min) AND (decays_epoch < N_epoch)
```

Where:
- `b_last` = block of last decay event
- `decays_epoch` = number of decays in current epoch

### Tag Evolution

After a sequence of transfers at blocks `{b_1, b_2, ..., b_n}`:

```
w(b_n) = w(0) × (1 - d)^E(b_n)
```

Where `E(b_n)` is the count of eligible decay events:

```
E(b_n) = |{i : eligible(b_i) = true}|
```

### Bounds

**Theorem 1 (Epoch Bound)**: In any epoch of length `T_epoch`, at most `N_epoch` decay events can occur.

*Proof*: By definition, `decays_epoch` is reset at epoch boundaries and `eligible()` returns false when `decays_epoch ≥ N_epoch`. □

**Theorem 2 (Time Bound)**: Over any time period `T`, at most `⌈T / T_epoch⌉ × N_epoch` decay events can occur.

*Proof*: By Theorem 1, each epoch contributes at most `N_epoch` events. The number of epochs in period T is at most `⌈T / T_epoch⌉`. □

**Theorem 3 (Rate Bound)**: Within any epoch, consecutive decay events are separated by at least `Δt_min` blocks.

*Proof*: The condition `b - b_last ≥ Δt_min` must hold for eligibility. After each decay, `b_last` is updated to current block. □

**Corollary (Maximum Daily Decay)**: With parameters `d=0.05, N_epoch=12`, maximum daily decay is:

```
1 - (1 - 0.05)^12 = 1 - 0.95^12 ≈ 0.46 (46%)
```

## Attack Analysis

### Attack 1: Rapid Wash Trading

**Strategy**: Execute N self-transfers in rapid succession (within minutes).

**Outcome**:
- At most 1 decay event occurs (first transfer may be eligible)
- Rate limit blocks subsequent decays
- Tag remaining: ≈ 95% (vs 0.6% under hop-based)

**Resistance**: ✅ Fully mitigated by rate limiting

### Attack 2: Patient Wash Trading

**Strategy**: Execute transfers spaced by `Δt_min`, continuously over time T.

**Outcome**:
- Maximum decays = `⌈T / T_epoch⌉ × N_epoch`
- Over 1 week (7 epochs): max 84 decays
- Tag remaining: 0.95^84 ≈ 1.35%

**Resistance**: ✅ Bounded by epoch cap (vs 0.02% under rate-limiting alone)

### Attack 3: Epoch Boundary Gaming

**Strategy**: Time transactions to maximize decays across epoch boundaries.

**Outcome**:
- Can achieve `N_epoch` in last hour of epoch E
- Can achieve `N_epoch` in first hour of epoch E+1
- But bounded to `2 × N_epoch` in any 2-epoch window

**Resistance**: ✅ Bounded, not exploitable beyond normal rate

### Attack 4: Passive Holding

**Strategy**: Wait for tags to decay naturally without transacting.

**Outcome**:
- Under AND-based: 100% tag retained indefinitely
- Decay only triggers on transfer

**Resistance**: ✅ No passive decay (vs 50%/week under block-based)

## Parameter Selection

### decay_rate_per_hop = 5% (50,000 ppm)

**Rationale**:
- Matches existing hop-based design for backward compatibility
- Provides meaningful decay per legitimate trade
- 20 trades = 36% remaining (reasonable privacy gain)

**Sensitivity**:
- Lower (2%): Slower privacy gain, harder to reduce high tags
- Higher (10%): Faster privacy, but also faster attack decay

### min_blocks_between_decays = 720 (~2 hours)

**Rationale**:
- Long enough to prevent rapid attacks
- Short enough for active traders to see regular decay
- 12 opportunities per day aligns with epoch cap

**Sensitivity**:
- Lower (360 = 1 hour): More decay opportunities, weaker protection
- Higher (1440 = 4 hours): Stronger protection, slower legitimate decay

### max_decays_per_epoch = 12

**Rationale**:
- Provides clear daily bound (46% max decay)
- Aligns with 2-hour rate limit (12 × 2 = 24 hours)
- Predictable behavior for users

**Sensitivity**:
- Lower (6): Stronger protection (27% max daily decay), slower privacy
- Higher (24): Weaker protection (71% max daily decay), faster privacy

### epoch_blocks = 8,640 (~1 day)

**Rationale**:
- Natural time unit for users to understand
- Long enough for epoch cap to be meaningful
- Aligns with standard day/week/month calculations

## Implementation Notes

### State Requirements

Each UTXO must track:
```rust
struct TagDecayState {
    tag_weights: HashMap<ClusterId, TagWeight>,
    last_decay_block: u64,
    decays_this_epoch: u32,
    epoch_start_block: u64,
}
```

### Epoch Reset Logic

```rust
fn check_epoch_reset(&mut self, current_block: u64, config: &AndDecayConfig) {
    if current_block - self.epoch_start_block >= config.epoch_blocks {
        self.epoch_start_block = current_block;
        self.decays_this_epoch = 0;
    }
}
```

### Decay Application

```rust
fn try_apply_decay(&mut self, current_block: u64, config: &AndDecayConfig) -> bool {
    self.check_epoch_reset(current_block, config);

    let time_eligible = current_block - self.last_decay_block >= config.min_blocks_between_decays;
    let epoch_eligible = self.decays_this_epoch < config.max_decays_per_epoch;

    if time_eligible && epoch_eligible {
        self.apply_decay(config.decay_rate_per_hop);
        self.last_decay_block = current_block;
        self.decays_this_epoch += 1;
        true
    } else {
        false
    }
}
```

### Chain Reorganization

On reorg, UTXO metadata (including decay state) is reconstructed from the canonical chain. The deterministic decay rules ensure consistent state regardless of which fork is followed.

## Comparison with Alternatives

| Property | Hop-Based | Block-Based | Rate-Limited | AND + Epoch |
|----------|-----------|-------------|--------------|-------------|
| Rapid wash trading | ❌ Vulnerable | ✅ Resistant | ✅ Resistant | ✅ Resistant |
| Patient wash trading | ❌ Vulnerable | ✅ Resistant | ⚠️ Unbounded | ✅ Bounded |
| Passive decay | ✅ None | ❌ Occurs | ✅ None | ✅ None |
| Complexity | Simple | Simple | Medium | Medium |
| State required | Tags only | Tags + block | Tags + block | Tags + block + epoch |

## Verification

### Simulation Commands

```bash
# Four-way comparison
./target/release/cluster-tax-sim decay-compare-four \
  --wealth 100000000 \
  --hop-decay 5.0 \
  --min-blocks 720 \
  --max-per-day 12 \
  --wash-txs 1000 \
  --blocks 60480

# Verify bounds hold
./target/release/cluster-tax-sim decay-compare-four \
  --wealth 100000000 \
  --hop-decay 5.0 \
  --min-blocks 720 \
  --max-per-day 12 \
  --wash-txs 10000 \
  --blocks 259200  # 30 days
```

### Expected Results

| Scenario | Expected Tag Remaining |
|----------|----------------------|
| 100 rapid txs (100 blocks) | 100% (0 eligible) |
| 1000 txs over 1 day | 54% (12 eligible) |
| 1000 txs over 1 week | 1.35% (84 eligible) |
| 0 txs over 1 year | 100% (no passive decay) |

## References

- `cluster-tax/src/block_decay.rs` - Implementation
- `experiments/ANALYSIS.md` - Experimental results
- GitHub Issue #85 - Research tracking

## Changelog

- 2024-12-31: Initial design specification
