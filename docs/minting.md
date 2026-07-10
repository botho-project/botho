# Minting Guide

Botho uses a **parallel proof-of-work** mechanism (RandomX, CPU-egalitarian) integrated with Stellar Consensus Protocol (SCP) for Byzantine fault tolerance.

## How Minting Works

### The Minting Process

1. **Find a Valid Nonce**: Minters search for a nonce whose RandomX hash satisfies the difficulty target:

   ```
   RandomX(seed_key, nonce || prev_block_hash || minter_keys) < difficulty_target
   ```

   The preimage binds `prev_block_hash`, so a miner cannot start working on block N+1 before seeing block N — preserving a fair latency edge for well-connected nodes while keeping CPU mining egalitarian.

2. **Submit Minting Transaction**: Valid proofs are wrapped in a `MintingTx` and submitted to the consensus network

3. **SCP Decides the Winner**: Multiple minters may find valid solutions simultaneously—the SCP quorum determines which block is accepted

### Why Parallel Minting?

Unlike Bitcoin where the first valid block to propagate "wins," Botho separates proof-of-work from block selection:

- **Multiple Valid Solutions**: Any minter who finds a valid nonce can submit a minting transaction
- **Consensus-Based Selection**: The SCP quorum (not network propagation speed) determines which minter's block is included
- **Byzantine Fault Tolerance**: Even if some nodes are malicious or offline, consensus proceeds correctly
- **Fair Selection**: Network latency doesn't determine the winner—the quorum does

## Quorum Requirements

**Solo minting is impossible by design.** Minting requires a satisfiable quorum with at least one other peer.

In recommended mode the quorum threshold follows the configured **fault model**:

- `crash` (default): a 2f+1 simple majority (`floor(n/2)+1`) — high-availability
  / liveness for trusted homogeneous clusters. A single crashed or lagging node
  (including a peer that is still catching up) cannot stall consensus. At n=3
  the quorum is 2-of-3.
- `bft`: a 3f+1 Byzantine quorum (`n - floor((n-1)/3)`) for an adversarial
  posture. Genuine Byzantine tolerance needs **at least 4 nodes**; at n<=3 it
  degenerates to unanimity.

See [Configuration → network.quorum](operations/configuration.md#networkquorum)
for the full threshold tables.

### Minting State Machine

```
┌──────────┐  peer connects    ┌──────────┐
│ WAITING  │ ───────────────► │  MINING  │
│          │  quorum met       │          │
└──────────┘ ◄─────────────── └──────────┘
              peer disconnects
              quorum lost
```

- On peer connect: Re-evaluate quorum, start minting if satisfied
- On peer disconnect: Re-evaluate quorum, stop minting if lost

## Starting Minting

### Basic Setup

1. Initialize your wallet if you haven't:
   ```bash
   botho init
   ```

2. Edit `~/.botho/config.toml`:
   ```toml
   [minting]
   enabled = true
   threads = 0  # 0 = auto-detect CPU count
   ```

3. Run with minting:
   ```bash
   botho run --mint
   ```

### Thread Configuration

| threads | Behavior |
|---------|----------|
| 0       | Auto-detect (uses all available CPUs) |
| 1-N     | Use exactly N threads |

For dedicated minting machines, leave at 0. For machines doing other work, set to fewer threads than your CPU count.

## Emission Schedule

Minting rewards follow a two-phase model: halvings followed by perpetual tail emission.

### Phase 1: Halving Period (~5 years at 5s blocks)

| Parameter | Value |
|-----------|-------|
| Initial reward | 50 BTH |
| Halving interval | 6,307,200 blocks (~1 year at 5s blocks) |
| Number of halvings | 5 |
| Phase 1 supply | ~611 million BTH |

Each halving epoch is 6,307,200 blocks; the cumulative supply is the running sum
of `reward × 6,307,200` per epoch (e.g. 50 × 6,307,200 = 315.36M BTH in epoch 0):

| Period | Years (at 5s) | Block Reward | Cumulative Supply |
|--------|---------------|--------------|-------------------|
| Halving 0 | 0-1 | 50 BTH | ~315.4M BTH |
| Halving 1 | 1-2 | 25 BTH | ~473.0M BTH |
| Halving 2 | 2-3 | 12.5 BTH | ~551.9M BTH |
| Halving 3 | 3-4 | 6.25 BTH | ~591.3M BTH |
| Halving 4 | 4-5 | 3.125 BTH | ~611.0M BTH |

**Note**: Actual halving periods depend on dynamic block timing. If average block time is 20s (typical for moderate load), halvings will occur ~4x slower (every ~4 years instead of ~1 year).

### Phase 2: Tail Emission (Year 5+)

After Phase 1, Botho transitions to perpetual tail emission targeting **2% annual net inflation**.

| Parameter | Value |
|-----------|-------|
| Target net inflation | 2% annually |
| Tail reward | Supply-dependent: ~1.9 BTH per block at the ~611M tail-onset supply, growing with supply (at 5s blocks) |
| Fee burn offset | ~0.5% expected |

The tail reward is **not a fixed constant** — it is recomputed from circulating
supply each block to target 2% net annual inflation. The gross per-block reward
is `supply × (2% + 0.5% expected burns) / 6,307,200`; at the ~611M tail-onset
supply this is ~2.4 BTH/block gross (~1.9 BTH/block net), and it scales upward as
supply grows.

**Why tail emission?**
- Ensures minters always have incentive to secure the network
- Compensates for coins lost to forgotten keys
- Provides predictable, sustainable monetary policy

**Note**: The tail reward calculation assumes 5s blocks (6.3M blocks/year). With slower actual blocks, fewer rewards are paid, naturally dampening inflation.

## Block Timing

Botho uses **dynamic block timing** that adapts to network load while maintaining fixed emission calculations based on a 5-second baseline.

### Monetary Baseline (5s)

All monetary calculations (halving schedule, emission rate) assume 5-second blocks:

| Parameter | Value |
|-----------|-------|
| Assumed block time | 5 seconds |
| Minimum (consensus floor) | 3 seconds |
| Maximum (policy ceiling) | 60 seconds |

### Dynamic Timing (Actual)

Actual block production timing adjusts based on transaction rate:

| Network Load | Transaction Rate | Block Time |
|--------------|------------------|------------|
| Very high | 20+ tx/s | 3s |
| High | 5+ tx/s | 5s |
| Medium | 1+ tx/s | 10s |
| Low | 0.2+ tx/s | 20s |
| Idle | < 0.2 tx/s | 40s |

**Why dynamic timing?**
- Faster blocks when the network is busy (better UX)
- Slower blocks when idle (reduced overhead)
- Emission rate self-adjusts: idle network = lower effective inflation

See [Architecture: Block Timing](architecture.md#block-timing-architecture) for detailed design.

## Difficulty Adjustment

| Parameter | Value |
|-----------|-------|
| Adjustment cadence | Every block (M5, #554) |
| Signal | Observed inter-block time vs the 5 s target |
| Per-step clamp | 0.5x–2x |

### Algorithm

```
observed > target (blocks too slow) → ease PoW
observed < target (blocks too fast) → harden PoW

new_difficulty = current_difficulty × clamp(observed / target, 0.5, 2.0)
```

The signal deliberately ignores transaction count, so a block producer cannot skew difficulty by stuffing or starving blocks. Tail-phase inflation targeting comes from the supply-dependent tail *reward* (recomputed each block for 2% net), not from difficulty.

## Transaction Fees

Transaction fees in Botho are **split 80% / 20%**: each block, 80% of collected
fees fund the cluster-tilted redistribution lottery (paid back out to randomly
selected, mostly small/well-circulated holders) and 20% are **burned** (removed
from circulation). Only the burned 20% is deflationary; it partially offsets tail
emission. See [Cluster-Tilted Redistribution](design/cluster-tilted-redistribution.md).

| Parameter | Value |
|-----------|-------|
| Fee formula | per-byte rate × tx size × cluster factor (1x–6x) × output penalty |
| Fee destination | 80% redistribution lottery, 20% burned |
| Priority | Higher fees = faster confirmation |

Minters earn block rewards only—fees are not collected by minters but instead routed to the redistribution lottery (80%) and burned (20%).

### Net Supply Formula

Only the burned portion (20% of fees) reduces total supply:

```
net_supply = total_mined - total_fees_burned
```

## Monitoring

### Check Minting Status

```bash
botho status
```

Shows:
- Current hashrate
- Blocks found
- Network difficulty
- Quorum status

### RPC API

Query minting status programmatically:

```bash
curl -X POST http://localhost:7101/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"minting_getStatus","params":{},"id":1}'
```

## Profitability Considerations

### Factors Affecting Profitability

1. **Hardware**: More CPU cores = higher hashrate
2. **Electricity costs**: PoW minting consumes significant power
3. **Network difficulty**: Adjusts based on total network hashrate
4. **Quorum stability**: Unstable peers can interrupt minting

### No GPU/ASIC Advantage (By Design)

Botho uses RandomX for proof-of-work — a memory-hard, CPU-optimized hash deliberately designed to resist ASICs and GPUs. Ordinary CPUs compete on near-equal footing, and that remains true as the network grows. Budget ~2-3 GB of RAM for the RandomX dataset when mining.

## Troubleshooting

### "Waiting for quorum"

Your node doesn't have enough peers to satisfy the quorum requirements.

**Solutions:**
- Check your internet connection
- Verify bootstrap peers are correct in config
- Lower `min_peers` in recommended mode
- Ensure firewall allows port 7100

### "Minting paused - peer disconnected"

A peer required for your quorum went offline.

**Solutions:**
- Wait for peer to reconnect
- Add more bootstrap peers for redundancy
- Use recommended mode with `min_peers = 2` for better fault tolerance

### Low Hashrate

**Solutions:**
- Increase `threads` in config
- Close other CPU-intensive applications
- Check for thermal throttling

## Security Considerations

### Bad Quorum Configurations

| Bad Config | Consequence |
|------------|-------------|
| Trust only yourself | Isolated, mine worthless blocks |
| Trust too few nodes | Chain halts when they go offline |
| Trust nodes outside main cluster | Follow a minority fork |
| Threshold too low | Accept blocks others reject |
| Threshold too high | Halt more often |

**Economic incentive**: Mined coins are only valuable if the main cluster accepts your blocks. Bad configuration = wasted electricity.

## Quick Reference

| Parameter | Mainnet Value |
|-----------|---------------|
| Block time (monetary baseline) | 5 seconds |
| Block time (actual range) | 3-40 seconds (dynamic) |
| Initial reward | 50 BTH |
| Halving interval | 6,307,200 blocks (~1 year at 5s blocks) |
| Number of halvings | 5 |
| Phase 1 supply | ~611 million BTH (~5 years) |
| Tail emission | Supply-dependent (~1.9 BTH/block at ~611M tail-onset supply) |
| Tail inflation target | 2% net |
| Difficulty adjustment | Every block, time-based (0.5x–2x per step) |
| Gossip port | 7100 |
| RPC port | 7101 |
