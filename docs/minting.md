# Minting Guide

Botho uses a **parallel proof-of-work** mechanism integrated with Stellar Consensus Protocol (SCP) for Byzantine fault tolerance.

## How Minting Works

### The Minting Process

1. **Find a Valid Nonce**: Minters search for a nonce that produces a hash below the difficulty target:

   ```
   SHA256(nonce || prev_block_hash || minter_view_key || minter_spend_key) < difficulty_target
   ```

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

### Phase 1: Halving Period (~10 years at 5s blocks)

| Parameter | Value |
|-----------|-------|
| Initial reward | 50 BTH |
| Halving interval | 12,614,400 blocks (~2 years at 5s blocks) |
| Number of halvings | 5 |
| Phase 1 supply | ~100 million BTH |

| Period | Years (at 5s) | Block Reward | Cumulative Supply |
|--------|---------------|--------------|-------------------|
| Halving 0 | 0-2 | 50 BTH | ~52.6M BTH |
| Halving 1 | 2-4 | 25 BTH | ~78.9M BTH |
| Halving 2 | 4-6 | 12.5 BTH | ~92.0M BTH |
| Halving 3 | 6-8 | 6.25 BTH | ~98.6M BTH |
| Halving 4 | 8-10 | 3.125 BTH | ~100M BTH |

**Note**: Actual halving periods depend on dynamic block timing. If average block time is 20s (typical for moderate load), halvings will occur ~4x slower (every ~8 years instead of ~2 years).

### Phase 2: Tail Emission (Year 10+)

After Phase 1, Botho transitions to perpetual tail emission targeting **2% annual net inflation**.

| Parameter | Value |
|-----------|-------|
| Target net inflation | 2% annually |
| Tail reward | ~0.40 BTH per block (at 5s blocks) |
| Fee burn offset | ~0.5% expected |

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
| Adjustment interval | 17,280 blocks (~24h at 5s blocks) |
| Maximum adjustment | ±25% per epoch |

### Algorithm

**Phase 1 (Halving)**:
```
adjustment_ratio = expected_time / actual_time
new_difficulty = current_difficulty × clamp(ratio, 0.75, 1.25)
```

**Phase 2 (Tail Emission)**:
Difficulty adjustment blends timing (30%) and monetary targeting (70%) to maintain 2% net inflation:
```
timing_ratio = expected_time / actual_time
monetary_ratio = actual_net_emission / target_net_emission
combined_ratio = timing_ratio × 0.3 + monetary_ratio × 0.7
new_difficulty = current_difficulty × clamp(combined_ratio, 0.75, 1.25)
```

This ensures the network can adapt block rate to hit inflation targets even when fee burns fluctuate.

## Transaction Fees

Transaction fees in Botho are **burned** (removed from circulation), creating deflationary pressure that offsets tail emission.

| Parameter | Value |
|-----------|-------|
| Minimum fee | 400 µBTH (0.0004 BTH) |
| Fee destination | Burned |
| Priority | Higher fees = faster confirmation |

Minters earn block rewards only—fees are not collected by minters but instead removed from the total supply.

### Net Supply Formula

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

### No GPU/ASIC Advantage (Currently)

Botho uses SHA-256 for proof-of-work. While this is ASIC-friendly, the small network size means CPU minting is currently viable.

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
| Halving interval | ~2 years (at 5s blocks) |
| Number of halvings | 5 |
| Tail emission | ~0.40 BTH/block |
| Tail inflation target | 2% net |
| Difficulty adjustment | Every 17,280 blocks (~24h at 5s) |
| Gossip port | 7100 |
| RPC port | 7101 |
