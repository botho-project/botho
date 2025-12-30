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

Minting rewards follow a smooth decay curve with perpetual tail emission:

| Parameter | Value |
|-----------|-------|
| Initial reward | 50 BTH |
| Halving period | ~6,307,200 blocks (~4 years at 20-sec blocks) |
| Tail emission | 0.6 BTH per block (perpetual) |
| Total supply | ~18 million BTH (pre-tail) |

### Reward Formula

```
reward = max(TAIL_EMISSION, (TOTAL_SUPPLY - total_mined) >> EMISSION_SPEED_FACTOR)
```

Where:
- `TOTAL_SUPPLY` = ~18.4 quintillion picocredits
- `EMISSION_SPEED_FACTOR` = 20
- `TAIL_EMISSION` = 0.6 credits per block

## Difficulty Adjustment

| Parameter | Value |
|-----------|-------|
| Target block time | 20 seconds |
| Adjustment window | Every 10 blocks |
| Maximum adjustment | 4x change per window |

### Algorithm

```
new_difficulty = current_difficulty * (expected_time / actual_time)
```

Clamped to prevent extreme swings (max 4x up or down per adjustment).

## Transaction Fees

Minters collect transaction fees in addition to block rewards.

| Parameter | Value |
|-----------|-------|
| Minimum fee | 0.0001 credits |
| Fee allocation | 100% to block minter |
| Priority | Fee-per-byte |

The mempool prioritizes transactions by fee-per-byte, so higher-fee transactions get included first.

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

### Web Dashboard

When running with `botho run`, a web dashboard is available showing:
- Minting statistics
- Network topology
- Wallet information
- Recent blocks

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
- Verify bootstrap peers are correct
- Lower `min_peers` in recommended mode
- Ensure firewall allows port 8443

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
