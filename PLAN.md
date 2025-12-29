# Cadence CLI - Implementation Plan

## Overview

A single foreground process that syncs the blockchain, manages a wallet, and optionally mines.

## Architecture

```
┌─────────────────────────────────────────────────┐
│                 cadence                          │
│                                                  │
│  ┌─────────────┐  ┌─────────────┐  ┌──────────┐ │
│  │ LedgerSync  │  │   Wallet    │  │  Miner   │ │
│  │             │  │   Scanner   │  │          │ │
│  └──────┬──────┘  └──────┬──────┘  └────┬─────┘ │
│         │                │               │       │
│         └────────────────┼───────────────┘       │
│                          │                       │
│                   ┌──────┴──────┐                │
│                   │   Storage   │                │
│                   │   (LMDB)    │                │
│                   └─────────────┘                │
└─────────────────────────────────────────────────┘
```

## CLI Commands

```bash
cadence init                  # Create wallet, generate mnemonic
cadence init --recover        # Recover wallet from mnemonic

cadence run                   # Sync blockchain + scan wallet
cadence run --mine            # Also mine

cadence status                # Show sync status, balance, mining stats
cadence send <address> <amt>  # Send credits
cadence address               # Show receiving address
```

## Config File

Location: `~/.cadence/config.toml` (or `./cadence.toml` if present)

```toml
[wallet]
# BIP39 mnemonic (24 words) - KEEP SECRET
mnemonic = "word1 word2 word3 ... word24"

[network]
# Peers to sync from (SCP quorum slice)
peers = [
    "cadence://node1.example.com:8443",
    "cadence://node2.example.com:8443",
]

# Quorum configuration
[network.quorum]
threshold = 2  # Need 2 of 3 peers to agree
peers = ["node1", "node2", "node3"]

[mining]
enabled = false
threads = 4  # Number of mining threads (0 = auto)
```

## Data Directory

```
~/.cadence/
├── config.toml      # Configuration + wallet secret
├── ledger/          # Blockchain (LMDB)
│   └── data.mdb
└── wallet/          # Wallet state (LMDB)
    └── data.mdb
```

## Components to Build

### 1. CLI Entry Point
- Parse commands with `clap`
- Load config from file
- Initialize logging

### 2. Config Module
- Load/save TOML config
- Validate settings
- Handle missing config (prompt for init)

### 3. Wallet Module (simplified from mobilecoind)
- Single account (no monitors)
- BIP39 mnemonic → account keys
- UTXO tracking
- Balance calculation
- Address derivation

### 4. Ledger Sync (simplified)
- Connect to peers (simple TCP/TLS, no SGX)
- Download blocks
- Validate blocks
- Store in LedgerDB

### 5. Wallet Scanner
- Scan new blocks with view key
- Update UTXO set
- Mark spent outputs

### 6. Miner (NEW)
- Mining loop in separate thread(s)
- Find nonce: `hash(nonce || prev_block_hash || address) < target`
- Create mining transaction when solution found
- Broadcast to peers
- Difficulty adjustment based on block times

### 7. Transaction Builder (reuse existing)
- Build standard transactions
- Ring signature construction
- Fee calculation

## Mining Transaction

A mining transaction is special:
- No inputs (creates new coins)
- Single output to miner's address
- Contains proof of work:
  - `nonce`: the solution
  - `prev_block_hash`: links to chain
  - `target`: difficulty at time of mining
- Validated by: `hash(nonce || prev_block_hash || miner_address) < target`

## Simplifications from mobilecoind

| mobilecoind | cadence | Notes |
|-------------|---------|-------|
| gRPC daemon | CLI foreground | No server, direct commands |
| Multi-monitor | Single wallet | One mnemonic, one account |
| SGX attestation | Simple TLS | No trusted enclaves |
| Fog integration | Removed | Not needed |
| T3 integration | Removed | Not needed |
| Watcher thread | Removed | Simplify |
| Complex config | Single TOML | Everything in one file |

## Implementation Order

1. **Scaffold** - CLI structure, config loading, data directories
2. **Wallet** - Init command, mnemonic generation, key derivation
3. **Storage** - LedgerDB + WalletDB setup
4. **Sync** - Basic peer connection, block download
5. **Scanner** - Process blocks, find wallet transactions
6. **Balance** - UTXO aggregation, balance display
7. **Mining** - PoW loop, mining transactions
8. **Send** - Transaction building, broadcast

## Code Reuse

Keep these crates mostly as-is:
- `mc-account-keys` - Key derivation
- `mc-crypto-*` - All cryptography
- `mc-transaction-core` - Transaction primitives
- `mc-transaction-builder` - Tx construction
- `mc-ledger-db` - Blockchain storage
- `mc-blockchain-types` - Block structures

Heavily modify:
- `mobilecoind` → `cadence` - Complete rewrite of main binary

Remove entirely:
- `fog/*` - All fog components
- `sgx/*` - SGX support
- `attest/*` - Attestation
- `mobilecoind-json` - HTTP wrapper
- `mobilecoind-dev-faucet` - Test faucet
- `consensus/*` - Old consensus (we use mining now)
- `watcher/*` - Block verification service

## Mining Economics

### Genesis
Empty ledger. No pre-mine, no initial distribution. All credits come from mining.

### Emission Schedule (Monero-style)

**Main curve**: Block reward decreases smoothly until reaching the tail emission.

```
reward = max(TAIL_EMISSION, (TOTAL_SUPPLY - already_mined) >> EMISSION_SPEED_FACTOR)
```

**Tail emission**: Once main curve depletes, constant small reward per block forever.
- Ensures perpetual mining incentive
- Provides baseline network security
- Slight inflation but predictable

Example parameters (to be tuned):
```rust
const TOTAL_SUPPLY: u64 = 18_446_744_073_709_551_615; // Near u64::MAX
const EMISSION_SPEED_FACTOR: u64 = 20;
const TAIL_EMISSION: u64 = 600_000_000_000; // 0.6 credits per block (in picocredits)
const TARGET_BLOCK_TIME: u64 = 120; // 2 minutes
```

### Difficulty Adjustment (PID Control)

Rather than Bitcoin's step-wise adjustment every N blocks, use continuous PID control:

```rust
struct DifficultyController {
    kp: f64,  // Proportional gain
    ki: f64,  // Integral gain
    kd: f64,  // Derivative gain
    target_yield: f64,  // Target blocks per time period
    integral: f64,
    last_error: f64,
}

impl DifficultyController {
    fn adjust(&mut self, actual_yield: f64) -> f64 {
        let error = self.target_yield - actual_yield;

        self.integral += error;
        let derivative = error - self.last_error;
        self.last_error = error;

        let adjustment = self.kp * error
                       + self.ki * self.integral
                       + self.kd * derivative;

        adjustment
    }
}
```

**Benefits of PID control:**
- Smooth adjustments (no sudden jumps)
- Resists oscillation
- Adapts quickly to hashrate changes
- Can target yield directly rather than block time

**Tuning:**
- Recalculate every block (or every N blocks)
- Use rolling window of recent block times
- Clamp adjustment factor to prevent extreme swings

### Transaction Fees

- Fees go to the miner who includes the transaction
- Minimum fee based on transaction size (bytes)
- Fee market during congestion (higher fee = priority)
- Mining reward = block_reward + sum(tx_fees)

### Block Structure

```rust
struct Block {
    header: BlockHeader,
    transactions: Vec<Transaction>,
}

struct BlockHeader {
    version: u32,
    prev_block_hash: Hash,
    merkle_root: Hash,      // Root of transaction tree
    timestamp: u64,
    difficulty: u64,
    nonce: u64,             // PoW solution
    miner_address: PublicAddress,
}

// Proof of work validation:
// hash(nonce || prev_block_hash || miner_address) < difficulty_target
```

### Mining Transaction (Coinbase)

First transaction in each block is the mining reward:
```rust
struct MiningTransaction {
    block_height: u64,
    reward: Amount,         // block_reward + fees
    recipient: PublicAddress,
    // No inputs - creates new credits
    // Still uses one-time address for privacy
}
```
