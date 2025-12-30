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

### Phase 1: Halving Period (~10 years)

| Parameter | Value |
|-----------|-------|
| Initial reward | 50 BTH |
| Halving interval | 3,153,600 blocks (~2 years at 20s blocks) |
| Number of halvings | 5 |
| Phase 1 supply | ~100 million BTH |

| Period | Years | Block Reward | Cumulative Supply |
|--------|-------|--------------|-------------------|
| Halving 0 | 0-2 | 50 BTH | ~52.6M BTH |
| Halving 1 | 2-4 | 25 BTH | ~78.9M BTH |
| Halving 2 | 4-6 | 12.5 BTH | ~92.0M BTH |
| Halving 3 | 6-8 | 6.25 BTH | ~98.6M BTH |
| Halving 4 | 8-10 | 3.125 BTH | ~100M BTH |

### Phase 2: Tail Emission (Year 10+)

After Phase 1, Botho transitions to perpetual tail emission targeting **2% annual net inflation**.

| Parameter | Value |
|-----------|-------|
| Target net inflation | 2% annually |
| Tail reward | ~1.59 BTH per block |
| Fee burn offset | ~0.5% expected |

**Why tail emission?**
- Ensures minters always have incentive to secure the network
- Compensates for coins lost to forgotten keys
- Provides predictable, sustainable monetary policy

## Block Timing

| Parameter | Value |
|-----------|-------|
| Target block time | 20 seconds |
| Minimum block time | 15 seconds |
| Maximum block time | 30 seconds |

## Difficulty Adjustment

| Parameter | Value |
|-----------|-------|
| Adjustment interval | 1,440 blocks (~24 hours) |
| Maximum adjustment | ±25% per epoch |

### Algorithm

```
adjustment_ratio = expected_time / actual_time
new_difficulty = current_difficulty × clamp(ratio, 0.75, 1.25)
```

In Phase 2, difficulty also factors in inflation targeting to maintain 2% net emission.

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
| Block time | 20 seconds |
| Initial reward | 50 BTH |
| Halving interval | ~2 years |
| Number of halvings | 5 |
| Tail emission | ~1.59 BTH/block |
| Tail inflation target | 2% net |
| Difficulty adjustment | Every 24 hours |
| Gossip port | 7100 |
| RPC port | 7101 |
