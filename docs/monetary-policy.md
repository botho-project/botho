# Monetary Policy Design

This document describes Botho's monetary policy framework, including how fee parameters, difficulty adjustment, and emission targeting interact, and how planned fork upgrades can adjust these parameters over time.

## Overview

Botho's monetary policy achieves predictable inflation through the interaction of three mechanisms:

1. **Emission**: New BTH created via minting transactions
2. **Burns**: Transaction fees removed from circulation
3. **Difficulty**: Controls the rate of successful minting

The key insight is that **block production and emission are decoupled**:

- **Block production**: Controlled by SCP consensus (~20 second target)
- **Emission rate**: Controlled by mining difficulty (how many blocks include minting rewards)

This separation allows precise monetary targeting without affecting transaction throughput.

### Block-Based Halving (Design Decision)

> **Block-Based Monetary Schedule**: The halving schedule is tied to **block height**,
> using a 5-second block assumption for monetary calculations.
>
> - Halving occurs every 12,614,400 blocks (~2 years at 5s blocks)
> - 5 halvings total before tail emission (~10 years at 5s blocks)
> - See `monetary.rs::mainnet_policy()` for the authoritative implementation
>
> **Adaptive Inflation**: Since actual block times vary (5-40s based on network load),
> effective inflation scales with network activity:
>
> | Block Time | Effective Inflation | Halving Period |
> |------------|--------------------:|---------------:|
> | 5s (high load) | 2.0%/year | 2 years |
> | 20s (normal) | 0.5%/year | 8 years |
> | 40s (idle) | 0.25%/year | 16 years |
>
> This creates a natural inflation dampener: busy network = full inflation,
> idle network = reduced inflation.

## The Economic Loop

```
┌─────────────────────────────────────────────────────────────────────────┐
│                         MONETARY POLICY LOOP                            │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                         │
│   ┌──────────────┐                                                      │
│   │ Target: 2%   │                                                      │
│   │ Net Inflation│                                                      │
│   └──────┬───────┘                                                      │
│          │                                                              │
│          ▼                                                              │
│   ┌──────────────┐      ┌──────────────┐      ┌──────────────┐        │
│   │   Compare    │      │   Adjust     │      │   Minting    │        │
│   │   to Actual  │─────▶│   Difficulty │─────▶│   Success    │        │
│   │   Net        │      │              │      │   Rate       │        │
│   └──────────────┘      └──────────────┘      └──────┬───────┘        │
│          ▲                                           │                 │
│          │                                           ▼                 │
│   ┌──────────────┐                           ┌──────────────┐         │
│   │ Net Emission │◀──────────────────────────│    Gross     │         │
│   │ = Gross -    │                           │   Emission   │         │
│   │   Burns      │                           │              │         │
│   └──────────────┘                           └──────────────┘         │
│          ▲                                                             │
│          │                                                              │
│   ┌──────────────┐      ┌──────────────┐      ┌──────────────┐        │
│   │  Fee Burns   │◀─────│ Transaction  │◀─────│     Fee      │        │
│   │              │      │   Volume     │      │  Parameters  │        │
│   └──────────────┘      └──────────────┘      └──────────────┘        │
│                                                                         │
└─────────────────────────────────────────────────────────────────────────┘
```

### Key Relationships

| If... | Then... | Because... |
|-------|---------|------------|
| Burns > expected | Net emission < target | Gross - Burns = Net |
| Net < target | Difficulty decreases | To increase gross emission |
| Difficulty decreases | More minting txs succeed | Lower threshold to meet |
| More minting success | Higher gross emission | More rewards distributed |
| Higher gross | Net rises toward target | Equilibrium restored |

The inverse applies when burns are lower than expected.

## Block Production vs. Emission

### SCP Block Production

SCP produces blocks on a regular schedule, independent of mining:

```
┌─────────┐     ┌─────────┐     ┌─────────┐     ┌─────────┐
│ Block N │────▶│Block N+1│────▶│Block N+2│────▶│Block N+3│
└─────────┘     └─────────┘     └─────────┘     └─────────┘
    20s             20s             20s             20s

Contents:       Contents:       Contents:       Contents:
- Minting tx    - Minting tx    - (no minting)  - Minting tx
- 5 transfers   - 3 transfers   - 8 transfers   - 2 transfers
- 50 BTH reward - 50 BTH reward - 0 BTH reward  - 50 BTH reward
```

Blocks are produced every ~20 seconds regardless of minting activity. A block without a valid minting transaction simply has no block reward.

### Difficulty Controls Minting Success

Difficulty determines what fraction of blocks include minting rewards:

| Difficulty Level | Minting Success Rate | Effective Emission |
|------------------|---------------------|-------------------|
| Very Low | ~100% of blocks | Maximum (follows halving schedule) |
| Target | ~100% of blocks | Near-maximum |
| High | ~80% of blocks | 80% of maximum |
| Very High | ~50% of blocks | 50% of maximum |

In Phase 1 (halving period), difficulty is calibrated to achieve near-100% minting success, giving predictable emission following the halving schedule.

In Phase 2 (tail emission), difficulty becomes the primary lever for targeting 2% net inflation.

## Slot-Based Block Production

SCP is a consensus protocol, not a timer. To achieve regular block production, Botho uses a **slot clock** that divides time into fixed intervals. SCP runs within each slot to reach consensus on block contents.

### The Slot Clock

Time is divided into 20-second slots, synchronized across nodes via wall clock:

```rust
pub struct SlotClock {
    /// Unix timestamp when slot 0 began (genesis)
    genesis_time_ms: u64,
    /// Duration of each slot in milliseconds
    slot_duration_ms: u64,  // 20,000 (20 seconds)
}

impl SlotClock {
    /// Deterministic slot number from wall clock
    pub fn current_slot(&self) -> u64 {
        let now = system_time_ms();
        (now - self.genesis_time_ms) / self.slot_duration_ms
    }

    /// When does a given slot start?
    pub fn slot_start_time(&self, slot: u64) -> u64 {
        self.genesis_time_ms + (slot * self.slot_duration_ms)
    }

    /// Time remaining until next slot boundary
    pub fn time_until_next_slot(&self) -> Duration {
        let current = self.current_slot();
        let next_start = self.slot_start_time(current + 1);
        Duration::from_millis(next_start.saturating_sub(system_time_ms()))
    }
}
```

All nodes agree on slot boundaries because they share the same genesis time and slot duration. Clock synchronization (via NTP) keeps nodes aligned within acceptable tolerances.

### Block Production Flow

Each slot follows this sequence:

```
┌─────────────────────────────────────────────────────────────────────────┐
│                     SLOT N BLOCK PRODUCTION                             │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                         │
│  1. COLLECTION PHASE (during slot)                                     │
│     ┌──────────────────────────────────────────────────────────┐       │
│     │  • Minters submit valid PoW solutions (MintingTx)        │       │
│     │  • Users submit transfer transactions                     │       │
│     │  • Node accumulates candidates in mempool                 │       │
│     └──────────────────────────────────────────────────────────┘       │
│                              │                                          │
│                              ▼                                          │
│  2. NOMINATION PHASE (at slot boundary)                                │
│     ┌──────────────────────────────────────────────────────────┐       │
│     │  • Each node nominates its preferred minting tx          │       │
│     │  • Nodes exchange nominations via SCP protocol           │       │
│     │  • Candidates are ranked (e.g., by PoW hash quality)     │       │
│     └──────────────────────────────────────────────────────────┘       │
│                              │                                          │
│                              ▼                                          │
│  3. BALLOT PHASE                                                       │
│     ┌──────────────────────────────────────────────────────────┐       │
│     │  • SCP quorum votes on nominated values                  │       │
│     │  • Nodes converge on a single winning minting tx         │       │
│     │  • (Or agree that no valid minting tx exists)            │       │
│     └──────────────────────────────────────────────────────────┘       │
│                              │                                          │
│                              ▼                                          │
│  4. EXTERNALIZE (finalize block)                                       │
│     ┌──────────────────────────────────────────────────────────┐       │
│     │  Block contents:                                          │       │
│     │  • Slot number and timestamp                              │       │
│     │  • Winning minting tx (if any) → block reward             │       │
│     │  • Transfer transactions from mempool                     │       │
│     │  • Previous block hash (chain link)                       │       │
│     └──────────────────────────────────────────────────────────┘       │
│                                                                         │
└─────────────────────────────────────────────────────────────────────────┘
```

### Consensus Service Implementation

```rust
pub struct ConsensusService {
    slot_clock: SlotClock,
    scp: ScpNode,
    pending_minting_txs: Vec<MintingTx>,
    mempool: Mempool,
    chain: Chain,
}

impl ConsensusService {
    /// Main block production loop
    pub async fn run(&mut self) -> Result<()> {
        loop {
            // Wait for next slot boundary
            tokio::time::sleep(self.slot_clock.time_until_next_slot()).await;

            let slot = self.slot_clock.current_slot();

            // Skip if we already have a block for this slot (e.g., after restart)
            if self.chain.has_block_for_slot(slot) {
                continue;
            }

            // Run consensus for this slot
            match self.run_slot_consensus(slot).await {
                Ok(block) => {
                    self.chain.append(block)?;
                    self.cleanup_after_block();
                }
                Err(ConsensusError::Timeout) => {
                    warn!("Slot {} consensus timeout", slot);
                    // Next iteration will try the next slot
                }
                Err(e) => return Err(e),
            }
        }
    }

    async fn run_slot_consensus(&mut self, slot: u64) -> Result<Block> {
        // Select our preferred minting tx (if we have valid candidates)
        let our_nomination = self.select_best_minting_tx();

        // SCP nomination phase
        self.scp.nominate(slot, our_nomination).await?;

        // SCP ballot phase - converge on winner
        let consensus_result = tokio::time::timeout(
            Duration::from_secs(15),  // Consensus timeout
            self.scp.ballot(slot)
        ).await??;

        // Build finalized block
        Ok(self.build_block(slot, consensus_result))
    }

    fn select_best_minting_tx(&self) -> Option<MintingTxHash> {
        // Among valid minting solutions, prefer the one with lowest PoW hash
        // (arbitrary but deterministic tie-breaker)
        self.pending_minting_txs
            .iter()
            .filter(|tx| self.validate_minting_tx(tx).is_ok())
            .min_by_key(|tx| tx.pow_hash())
            .map(|tx| tx.hash())
    }

    fn build_block(&self, slot: u64, winner: Option<MintingTxHash>) -> Block {
        let minting_tx = winner.and_then(|h| {
            self.pending_minting_txs.iter().find(|tx| tx.hash() == h)
        });

        Block {
            slot,
            height: self.chain.height() + 1,
            prev_hash: self.chain.tip_hash(),
            timestamp: self.slot_clock.slot_start_time(slot),
            minting_tx: minting_tx.cloned(),
            transactions: self.mempool.select_for_block(),
            // Block reward only if minting_tx is present
            reward: minting_tx.map(|_| self.current_block_reward()),
        }
    }
}
```

### Clock Synchronization Requirements

Nodes must maintain synchronized clocks within acceptable bounds:

| Requirement | Value | Rationale |
|-------------|-------|-----------|
| NTP sync | Required | Common time reference |
| Max clock drift | ±2 seconds | ~10% of slot duration |
| Sync check interval | 1 hour | Detect drift before it causes issues |

```rust
pub struct ClockHealth {
    last_ntp_check: Instant,
    ntp_offset_ms: i64,
}

impl ClockHealth {
    const MAX_OFFSET_MS: i64 = 2000;  // 2 seconds
    const CHECK_INTERVAL: Duration = Duration::from_secs(3600);  // 1 hour

    pub async fn check_and_sync(&mut self) -> Result<()> {
        if self.last_ntp_check.elapsed() > Self::CHECK_INTERVAL {
            let response = ntp::query("pool.ntp.org").await?;
            self.ntp_offset_ms = response.offset_ms;
            self.last_ntp_check = Instant::now();

            if self.ntp_offset_ms.abs() > Self::MAX_OFFSET_MS {
                warn!(
                    "Clock drift {}ms exceeds maximum {}ms",
                    self.ntp_offset_ms,
                    Self::MAX_OFFSET_MS
                );
                // Could pause consensus participation until corrected
            }
        }
        Ok(())
    }
}
```

### Handling Edge Cases

**Node starts mid-slot:**
```rust
// On startup, wait for next slot boundary before participating
let time_to_next = slot_clock.time_until_next_slot();
info!("Waiting {}ms for next slot boundary", time_to_next.as_millis());
tokio::time::sleep(time_to_next).await;
```

**Node was offline and missed slots:**
```rust
async fn catch_up(&mut self) -> Result<()> {
    let our_tip = self.chain.tip_slot();
    let network_tip = self.peers.best_slot().await;

    if network_tip > our_tip {
        info!("Catching up: our slot {} vs network slot {}", our_tip, network_tip);
        self.sync_blocks_from_peers(our_tip + 1, network_tip).await?;
    }
    Ok(())
}
```

**Consensus doesn't complete within slot:**
```rust
// If ballot phase times out, the slot is skipped
// This is rare but handled gracefully - next slot will proceed normally
match tokio::time::timeout(CONSENSUS_TIMEOUT, self.scp.ballot(slot)).await {
    Ok(Ok(result)) => Ok(result),
    Ok(Err(e)) => Err(e),
    Err(_) => {
        warn!("Slot {} consensus timeout after {:?}", slot, CONSENSUS_TIMEOUT);
        Err(ConsensusError::Timeout)
    }
}
```

### Why Slot-Based Timing?

| Approach | Pros | Cons |
|----------|------|------|
| **Slot-based (chosen)** | Deterministic timing, simple mental model, handles empty blocks naturally | Requires clock sync |
| Leader-based | No clock sync needed | Leader selection complexity, single point of failure per slot |
| Minting-triggered | Natural PoW integration | Unpredictable timing, complexity with no-minting scenarios |

The slot-based approach matches Stellar's proven model and provides the predictability needed for monetary policy.

## Difficulty Adjustment Algorithm

### Parameters

```rust
pub struct DifficultyParams {
    /// How often to adjust (in blocks)
    pub adjustment_interval: u64,        // 1,440 blocks (~8 hours)

    /// Maximum adjustment per epoch
    pub max_adjustment: f64,             // ±25%

    /// Weight for timing component
    pub timing_weight: f64,              // 0.30 (30%)

    /// Weight for monetary component
    pub monetary_weight: f64,            // 0.70 (70%)

    /// Target block time in milliseconds
    pub target_block_time_ms: u64,       // 20,000 (20 seconds)

    /// Bounds on block time
    pub min_block_time_ms: u64,          // 15,000 (15 seconds)
    pub max_block_time_ms: u64,          // 30,000 (30 seconds)
}
```

### Phase 1: Timing-Focused Adjustment

During the halving period, we want predictable emission following the schedule:

```rust
fn phase1_adjustment(metrics: &EpochMetrics, params: &DifficultyParams) -> f64 {
    // Primary goal: maintain target block time
    let timing_ratio = params.target_block_time_ms as f64
        / metrics.avg_block_time_ms as f64;

    // Secondary: ensure minting is happening
    let minting_ratio = if metrics.minting_success_rate < 0.95 {
        // Minting success too low - need to lower difficulty
        0.95 / metrics.minting_success_rate
    } else {
        1.0
    };

    // Blend with timing dominant
    let adjustment = 0.8 * timing_ratio + 0.2 * minting_ratio;

    adjustment.clamp(1.0 - params.max_adjustment, 1.0 + params.max_adjustment)
}
```

### Phase 2: Monetary-Focused Adjustment

After tail emission begins, we target net inflation:

```rust
fn phase2_adjustment(
    metrics: &EpochMetrics,
    params: &DifficultyParams,
    monetary: &MonetaryParams,
) -> f64 {
    // Calculate target net emission per block
    let annual_target = monetary.target_inflation_rate * metrics.circulating_supply as f64;
    let blocks_per_year = 365.25 * 24.0 * 3600.0 * 1000.0 / params.target_block_time_ms as f64;
    let target_net_per_block = annual_target / blocks_per_year;

    // Calculate actual net emission per block
    let actual_gross_per_block = metrics.total_emission as f64 / metrics.block_count as f64;
    let actual_burns_per_block = metrics.total_burns as f64 / metrics.block_count as f64;
    let actual_net_per_block = actual_gross_per_block - actual_burns_per_block;

    // Monetary ratio: how much to adjust emission
    let monetary_ratio = if actual_net_per_block > 0.0 {
        target_net_per_block / actual_net_per_block
    } else {
        // Deflationary - need more emission
        1.0 + params.max_adjustment
    };

    // Timing ratio: maintain block time
    let timing_ratio = params.target_block_time_ms as f64
        / metrics.avg_block_time_ms as f64;

    // Blend: monetary dominant
    let raw_adjustment = params.timing_weight * timing_ratio
        + params.monetary_weight * monetary_ratio;

    // Clamp to bounds
    raw_adjustment.clamp(1.0 - params.max_adjustment, 1.0 + params.max_adjustment)
}
```

### Understanding the Adjustment Direction

When net emission is **too high** (burns lower than expected):

```
actual_net > target_net
monetary_ratio = target / actual < 1.0
adjustment < 1.0
new_difficulty = old_difficulty / adjustment  →  INCREASES
higher difficulty  →  fewer minting successes  →  lower gross emission
lower gross  →  lower net  →  converges to target
```

When net emission is **too low** (burns higher than expected):

```
actual_net < target_net
monetary_ratio = target / actual > 1.0
adjustment > 1.0
new_difficulty = old_difficulty / adjustment  →  DECREASES
lower difficulty  →  more minting successes  →  higher gross emission
higher gross  →  higher net  →  converges to target
```

## Monetary Epochs

Each planned fork defines a "monetary epoch" with all economic parameters:

```rust
/// Defines monetary policy for a range of block heights
#[derive(Clone, Debug)]
pub struct MonetaryEpoch {
    /// Block height when this epoch activates
    pub activation_height: u64,

    // ─── Fee Parameters ───────────────────────────────────────────

    /// Minimum transaction fee in nanoBTH
    pub min_fee: u64,

    /// Cluster fee curve: min rate in basis points (5 = 0.05%)
    pub cluster_fee_min_bps: u16,

    /// Cluster fee curve: max rate in basis points (3000 = 30%)
    pub cluster_fee_max_bps: u16,

    /// Cluster wealth midpoint (sigmoid inflection) in nanoBTH
    pub cluster_midpoint: u64,

    /// Tag decay per transaction hop in basis points (500 = 5%)
    pub cluster_decay_bps: u16,

    // ─── Emission Parameters ──────────────────────────────────────

    /// Target net inflation in basis points per year (200 = 2%)
    pub target_net_inflation_bps: u16,

    /// Override tail emission per block (0 = use halving formula)
    pub tail_emission_override: u64,

    // ─── Difficulty Adjustment ────────────────────────────────────

    /// Weight for timing component in basis points (3000 = 30%)
    pub timing_weight_bps: u16,

    /// Weight for monetary component in basis points (7000 = 70%)
    pub monetary_weight_bps: u16,

    // ─── Transition Parameters ────────────────────────────────────

    /// Blocks to transition from previous epoch's expectations
    pub transition_blocks: u64,

    /// Expected burn rate per block in nanoBTH (for transition calibration)
    pub expected_burn_rate: u64,
}
```

## Planned Epoch Schedule

| Epoch | Height | ~Year | Min Fee | Block Reward | Notes |
|-------|--------|-------|---------|--------------|-------|
| 0 | 0 | 0 | 400 µBTH | 50 BTH | Genesis |
| 1 | 3,153,600 | 2 | 200 µBTH | 25 BTH | First halving |
| 2 | 6,307,200 | 4 | 100 µBTH | 12.5 BTH | Second halving |
| 3 | 9,460,800 | 6 | 50 µBTH | 6.25 BTH | Third halving |
| 4 | 12,614,400 | 8 | 25 µBTH | 3.125 BTH | Fourth halving |
| 5 | 15,768,000 | 10 | 12.5 µBTH | ~1.59 BTH | Tail emission begins |
| 6 | 31,536,000 | 20 | 10 µBTH | ~1.59 BTH | Maturity (possible inflation reduction) |

### Epoch 0: Genesis

```rust
MonetaryEpoch {
    activation_height: 0,

    // Fees
    min_fee: 400_000,                              // 400 µBTH
    cluster_fee_min_bps: 5,                        // 0.05%
    cluster_fee_max_bps: 3000,                     // 30%
    cluster_midpoint: 10_000_000_000_000_000_000,  // 10M BTH in nanoBTH
    cluster_decay_bps: 500,                        // 5% per hop

    // Emission
    target_net_inflation_bps: 200,                 // 2% (informational in Phase 1)
    tail_emission_override: 0,                     // Use halving formula

    // Difficulty (timing-focused for Phase 1)
    timing_weight_bps: 8000,                       // 80%
    monetary_weight_bps: 2000,                     // 20%

    // Transition
    transition_blocks: 0,                          // Genesis
    expected_burn_rate: 0,                         // Will observe
}
```

### Epoch 5: Tail Emission Transition

```rust
MonetaryEpoch {
    activation_height: 15_768_000,                 // ~10 years

    // Fees (halved 5 times from genesis)
    min_fee: 12_500,                               // 12.5 µBTH
    cluster_fee_min_bps: 5,
    cluster_fee_max_bps: 3000,
    cluster_midpoint: 10_000_000_000_000_000_000,
    cluster_decay_bps: 500,

    // Emission (tail begins)
    target_net_inflation_bps: 200,                 // 2% - now actively targeted
    tail_emission_override: 1_590_000_000,         // ~1.59 BTH per block

    // Difficulty (monetary-focused for Phase 2)
    timing_weight_bps: 3000,                       // 30%
    monetary_weight_bps: 7000,                     // 70%

    // Transition (major shift - longer period)
    transition_blocks: 43_200,                     // ~10 days
    expected_burn_rate: /* based on epoch 4 observation */,
}
```

## Transition Mechanics

When parameters change at a fork, the difficulty adjustment needs to handle the discontinuity gracefully.

### The Problem

```
Before fork: burns averaging 1000 nanoBTH/block
Fork activates: fee floor halves
After fork: burns might be 500-2000 nanoBTH/block (unknown)

If we immediately use actual burns:
- First epoch after fork sees huge deviation
- Difficulty swings wildly
- System oscillates before converging
```

### The Solution: Blended Expectations

During the transition period, blend from pre-fork observations to post-fork expectations:

```rust
fn transition_burn_estimate(
    blocks_since_fork: u64,
    transition_length: u64,
    pre_fork_burn_rate: u64,
    expected_post_fork_rate: u64,
) -> u64 {
    let blend = blocks_since_fork as f64 / transition_length as f64;
    let blended = (1.0 - blend) * pre_fork_burn_rate as f64
        + blend * expected_post_fork_rate as f64;
    blended as u64
}
```

```
Transition Timeline:

Fork ─────────────────────────────────────────────────▶ Steady State
  │                                                           │
  │   ┌─────────────────────────────────────────────────┐    │
  │   │          Transition Period                       │    │
  │   │                                                  │    │
  │   │  Expected burns: blend(pre_fork → post_fork)    │    │
  │   │  Difficulty: uses blended expectations          │    │
  │   │                                                  │    │
  │   └─────────────────────────────────────────────────┘    │
  │                                                           │
  │   100% pre-fork ────────────────────▶ 100% post-fork     │
  │   expectations                        expectations        │
                                                              │
                                          Uses actual observed
                                          burns (self-correcting)
```

### Transition Algorithm

```rust
impl MonetaryEpoch {
    pub fn calculate_adjustment(
        &self,
        height: u64,
        prev_epoch: Option<&MonetaryEpoch>,
        metrics: &ChainMetrics,
    ) -> f64 {
        let blocks_since_fork = height.saturating_sub(self.activation_height);

        // Always use actual timing
        let timing_ratio = self.target_block_time_ms() as f64
            / metrics.recent_block_time_ms as f64;

        // Monetary ratio depends on transition state
        let monetary_ratio = if blocks_since_fork < self.transition_blocks {
            self.transition_monetary_ratio(blocks_since_fork, prev_epoch, metrics)
        } else {
            self.steady_state_monetary_ratio(metrics)
        };

        // Blend and clamp
        let timing_weight = self.timing_weight_bps as f64 / 10000.0;
        let monetary_weight = self.monetary_weight_bps as f64 / 10000.0;

        let raw = timing_weight * timing_ratio.clamp(0.75, 1.33)
            + monetary_weight * monetary_ratio.clamp(0.80, 1.25);

        raw.clamp(0.75, 1.25)
    }

    fn transition_monetary_ratio(
        &self,
        blocks_since_fork: u64,
        prev_epoch: Option<&MonetaryEpoch>,
        metrics: &ChainMetrics,
    ) -> f64 {
        // Blend from pre-fork actual to post-fork expected
        let blend = blocks_since_fork as f64 / self.transition_blocks as f64;

        let pre_fork_burns = metrics.burn_rate_before(self.activation_height);
        let expected_burns = lerp(
            pre_fork_burns as f64,
            self.expected_burn_rate as f64,
            blend,
        );

        // Calculate target vs expected net
        let target_net = self.calculate_target_net(metrics.circulating_supply);
        let gross = self.current_block_reward(metrics.height) as f64;
        let expected_net = gross - expected_burns;

        if expected_net > 0.0 {
            target_net / expected_net
        } else {
            1.25 // Push toward more emission
        }
    }

    fn steady_state_monetary_ratio(&self, metrics: &ChainMetrics) -> f64 {
        let target_net = self.calculate_target_net(metrics.circulating_supply);
        let actual_net = metrics.recent_gross_emission as f64
            - metrics.recent_burns as f64;

        if actual_net > 0.0 {
            target_net / actual_net
        } else {
            1.25
        }
    }
}
```

## Estimating Post-Fork Burns

When planning a fork that changes fee parameters, we need to estimate how burns will change:

### Transaction Demand Elasticity

```rust
/// Estimate how transaction volume responds to fee changes
pub fn estimate_volume_change(
    fee_ratio: f64,      // new_fee / old_fee (e.g., 0.5 for halving)
    elasticity: f64,     // demand elasticity (e.g., -1.5)
) -> f64 {
    // Volume change = (1/fee_ratio)^(-elasticity)
    (1.0 / fee_ratio).powf(-elasticity)
}

/// Estimate post-fork burn rate
pub fn estimate_post_fork_burns(
    current_burns: u64,
    fee_ratio: f64,
    elasticity: f64,
) -> u64 {
    let volume_change = estimate_volume_change(fee_ratio, elasticity);
    let burn_change = fee_ratio * volume_change;
    (current_burns as f64 * burn_change) as u64
}
```

### Elasticity Scenarios

For a fee floor halving (fee_ratio = 0.5):

| Demand Elasticity | Volume Change | Burn Change | Interpretation |
|-------------------|---------------|-------------|----------------|
| -0.5 (inelastic) | 1.41× | 0.71× | Burns decrease 29% |
| -1.0 (unit) | 2.0× | 1.0× | Burns unchanged |
| -1.5 (elastic) | 2.83× | 1.41× | Burns increase 41% |
| -2.0 (very elastic) | 4.0× | 2.0× | Burns double |

For a new network, demand is likely elastic (-1.5 or higher), so fee reductions may actually increase total burns.

### Handling Uncertainty

Set `transition_blocks` based on uncertainty:

| Confidence | Transition Length | Rationale |
|------------|-------------------|-----------|
| High (±20% estimate) | 4,320 blocks (~1 day) | Quick convergence |
| Medium (±50% estimate) | 17,280 blocks (~4 days) | Standard transition |
| Low (±100% estimate) | 43,200 blocks (~10 days) | Extended observation |
| Major change | 86,400 blocks (~20 days) | Maximum caution |

## Self-Correction Properties

The system is inherently self-correcting regardless of burn estimates:

### Convergence Proof (Informal)

1. **Bounded adjustment**: Each epoch can only change difficulty by ±25%
2. **Correct direction**: monetary_ratio always pushes toward target
3. **Finite deviation**: Any burn rate produces a finite deviation
4. **Geometric convergence**: Each adjustment closes the gap by up to 25%

After N adjustment epochs:
- Maximum remaining deviation: `initial_deviation × 0.75^N`
- 90% convergence in: `log(0.1) / log(0.75) ≈ 8` epochs (~2.7 days)
- 99% convergence in: `log(0.01) / log(0.75) ≈ 16` epochs (~5.3 days)

### Stability Analysis

The system has a single stable equilibrium where:
- `actual_net = target_net`
- `monetary_ratio = 1.0`
- Difficulty constant (modulo hashrate changes)

Perturbations (hashrate changes, demand shocks, fee adjustments) cause temporary deviations that self-correct.

## Safety Rails

### Block Time Bounds

Even with monetary pressure, block times stay within bounds:

```rust
fn enforce_block_time_bounds(
    raw_adjustment: f64,
    current_block_time: u64,
    min_block_time: u64,
    max_block_time: u64,
) -> f64 {
    // Note: adjustment < 1 means difficulty INCREASES (blocks slower to find)
    //       adjustment > 1 means difficulty DECREASES (blocks faster to find)

    let projected_time = current_block_time as f64 * raw_adjustment;

    if projected_time < min_block_time as f64 {
        // Would be too fast - limit the decrease
        current_block_time as f64 / min_block_time as f64
    } else if projected_time > max_block_time as f64 {
        // Would be too slow - limit the increase
        current_block_time as f64 / max_block_time as f64
    } else {
        raw_adjustment
    }
}
```

### Emergency Fallback

If burns deviate extremely (>50% from expected), temporarily increase timing weight:

```rust
fn calculate_weights(
    epoch: &MonetaryEpoch,
    metrics: &ChainMetrics,
) -> (f64, f64) {
    let deviation = (metrics.recent_burns as f64 - epoch.expected_burn_rate as f64).abs()
        / epoch.expected_burn_rate.max(1) as f64;

    if deviation > 0.5 {
        // Burns way off - rely more on timing until stabilized
        (0.6, 0.4)  // 60% timing, 40% monetary
    } else {
        let t = epoch.timing_weight_bps as f64 / 10000.0;
        let m = epoch.monetary_weight_bps as f64 / 10000.0;
        (t, m)
    }
}
```

### Invariants

Every epoch must satisfy:

```rust
pub fn validate_epoch(epoch: &MonetaryEpoch) -> Result<(), Error> {
    // Weights sum to 100%
    ensure!(epoch.timing_weight_bps + epoch.monetary_weight_bps == 10000);

    // Block time bounds are sensible
    ensure!(epoch.min_block_time_ms >= 10_000);   // At least 10s
    ensure!(epoch.max_block_time_ms <= 120_000);  // At most 2min
    ensure!(epoch.min_block_time_ms < epoch.target_block_time_ms);
    ensure!(epoch.target_block_time_ms < epoch.max_block_time_ms);

    // Fees are valid
    ensure!(epoch.min_fee > 0);  // No free transactions
    ensure!(epoch.cluster_fee_min_bps < epoch.cluster_fee_max_bps);
    ensure!(epoch.cluster_fee_max_bps <= 5000);  // Max 50%

    // Inflation is bounded
    ensure!(epoch.target_net_inflation_bps > 0);
    ensure!(epoch.target_net_inflation_bps <= 500);  // Max 5%

    // Transition is reasonable
    if epoch.activation_height > 0 {
        ensure!(epoch.transition_blocks >= 1440);    // At least ~8 hours
        ensure!(epoch.transition_blocks <= 259_200); // At most ~60 days
    }

    Ok(())
}
```

## Fork Activation Process

### Timeline

```
T-6 months    Proposal published
              - New epoch parameters
              - Economic rationale
              - Simulation results
              - Activation height

T-3 months    Code released
              - Epoch baked into node software
              - Activation height locked
              - Nodes can begin upgrading

T-1 month     Testnet activation
              - Observe transition behavior
              - Validate convergence
              - Refine expected_burn_rate if needed

T-0           Mainnet activation
              - New parameters active at specified height
              - Transition period begins

T+transition  Steady state
              - System uses actual observed burns
              - Convergence complete
```

### Governance

Fee and monetary parameter changes should follow:

1. **Proposal**: Core developers or community propose changes
2. **Discussion**: Public debate on rationale and impacts
3. **Modeling**: Simulation of expected effects
4. **Code**: Implementation in reference client
5. **Testnet**: Live testing
6. **Activation**: Mainnet deployment at agreed height

For routine halvings (Epochs 1-4), the process can be streamlined since parameters are predictable.

For novel changes (Epoch 5 tail transition, Epoch 6 potential inflation reduction), extended discussion is warranted.

## Monitoring and Observability

### Key Metrics

Nodes should track and expose:

```rust
pub struct MonetaryMetrics {
    // Emission
    pub gross_emission_per_block: f64,      // EMA
    pub minting_success_rate: f64,          // Fraction of blocks with minting

    // Burns
    pub burns_per_block: f64,               // EMA
    pub burn_rate_deviation: f64,           // vs expected

    // Net
    pub net_emission_per_block: f64,        // EMA
    pub actual_vs_target_inflation: f64,    // Ratio

    // Difficulty
    pub current_difficulty: u64,
    pub last_adjustment: f64,               // Multiplier
    pub adjustment_direction: Direction,    // Increasing/Decreasing/Stable

    // Transition
    pub in_transition: bool,
    pub transition_progress: f64,           // 0.0 to 1.0
    pub current_epoch: u8,
}
```

### Alerts

Operators should alert on:

| Condition | Threshold | Action |
|-----------|-----------|--------|
| Minting success rate | < 80% | Check hashrate, difficulty |
| Net inflation deviation | > 50% from target | Monitor convergence |
| Block time deviation | Outside 15-30s | Check network health |
| Difficulty changing rapidly | > 10% per epoch | Investigate cause |
| Burns deviating | > 100% from expected | Extend transition if needed |

## Future Considerations

### Adaptive Tail Emission

If tighter inflation control is desired, tail emission itself could become adaptive:

```rust
fn adaptive_tail_emission(
    target_inflation: f64,
    supply: u64,
    recent_burns: u64,
    blocks_per_year: f64,
) -> u64 {
    let target_net = target_inflation * supply as f64 / blocks_per_year;
    let required_gross = target_net + recent_burns as f64;
    required_gross as u64
}
```

This would provide exact inflation targeting but adds complexity.

### Dynamic Fee Floor

The fee floor could adjust automatically based on difficulty:

```rust
fn dynamic_fee_floor(
    base_fee: u64,
    current_difficulty: u64,
    genesis_difficulty: u64,
) -> u64 {
    let ratio = (genesis_difficulty as f64 / current_difficulty as f64).sqrt();
    (base_fee as f64 * ratio).max(MIN_ABSOLUTE_FEE) as u64
}
```

This would provide inter-halving fee adjustment without requiring forks.

### Inflation Target Adjustment

As the network matures, the community may decide to reduce the inflation target:

| Year | Possible Target | Rationale |
|------|-----------------|-----------|
| 0-10 | N/A (halvings) | Distribution phase |
| 10-20 | 2.0% | Initial tail emission |
| 20-30 | 1.5% | Reduced as network stabilizes |
| 30+ | 1.0% | Long-term maintenance |

Such changes would require careful community consensus and extended transition periods.

## Summary

Botho's monetary policy achieves predictable inflation through:

1. **Decoupled block production**: SCP produces blocks regularly; minting success controls emission
2. **Difficulty targeting**: Adjusts to maintain target net inflation (gross - burns)
3. **Epoch-based upgrades**: Planned forks can adjust parameters with smooth transitions
4. **Self-correction**: System converges to target regardless of burn rate estimates
5. **Safety rails**: Block time bounds, emergency fallbacks, validated invariants

This framework allows the network to evolve its economic parameters over time while maintaining stability and predictability.
