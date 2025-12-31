# Architecture

Botho is a privacy-preserving, mined cryptocurrency built as a single binary that handles all node operations.

## System Overview

```
┌──────────────────────────────────────────────────────────────────────┐
│                               botho                                   │
│                                                                       │
│  ┌─────────────┐  ┌─────────────┐  ┌──────────┐  ┌───────────────┐  │
│  │   Network   │  │   Wallet    │  │  Minter   │  │   Consensus   │  │
│  │  Discovery  │  │             │  │          │  │    Service    │  │
│  └──────┬──────┘  └──────┬──────┘  └────┬─────┘  └───────┬───────┘  │
│         │                │               │                │          │
│         └────────────────┼───────────────┼────────────────┘          │
│                          │               │                           │
│                   ┌──────┴───────┐  ┌────┴─────┐                     │
│                   │    Ledger    │  │  Mempool │                     │
│                   │    (LMDB)    │  │          │                     │
│                   └──────────────┘  └──────────┘                     │
└──────────────────────────────────────────────────────────────────────┘
```

## Core Components

### Network Layer

The network layer uses **libp2p** with gossipsub for peer-to-peer communication.

**Gossip Topics:**
- `botho/blocks/1.0.0` - Block announcements
- `botho/transactions/1.0.0` - Transaction broadcasts
- `botho/scp/1.0.0` - SCP consensus messages

**Peer Discovery Flow:**
1. Node starts and dials bootstrap peers
2. Subscribes to gossip topics
3. Announces presence via gossipsub
4. Discovers additional peers through gossip
5. Maintains peer table with last-seen timestamps

**Peer Reputation:**
- EMA (Exponential Moving Average) latency tracking
- Success/failure counting for reliability scoring
- Peers with <25% success rate are banned

### Consensus Layer

Botho uses the **Stellar Consensus Protocol (SCP)** for Byzantine fault tolerance. Unlike Bitcoin where the first valid block to propagate wins, Botho separates proof-of-work from block selection.

**How it works:**
1. Minters find valid PoW nonces and submit minting transactions
2. Multiple valid solutions may exist simultaneously
3. The SCP quorum determines which block is accepted
4. Byzantine fault tolerance ensures consensus even with malicious nodes

**Quorum Configuration:**
- **Recommended mode**: Automatically trusts discovered peers, calculates BFT threshold as `ceil(2n/3)`
- **Explicit mode**: User specifies trusted peer IDs and threshold

### Wallet

The wallet uses **BIP39 mnemonics** (24 words) for key derivation with a single account model.

**Features:**
- CryptoNote-style stealth addresses (one-time keys)
- View key for scanning incoming transactions
- Spend key for signing outgoing transactions
- Change subaddress for privacy

### Minter

Multi-threaded proof-of-work minting integrated with the consensus layer.

**PoW Algorithm:**
```
SHA256(nonce || prev_block_hash || minter_view_key || minter_spend_key) < difficulty_target
```

**Key Properties:**
- Minting requires a satisfiable quorum (solo minting is impossible)
- Minting automatically pauses when quorum is lost
- Work updates when new blocks arrive

### Ledger

LMDB-backed storage for the blockchain.

**Stored Data:**
- Block headers and full blocks
- UTXO (Unspent Transaction Output) set
- Block height index
- Transaction hash index

### Mempool

Transaction pool with fee-based priority ordering.

**Features:**
- Double-spend detection
- Fee-per-byte prioritization
- Automatic expiration of old transactions

## Data Flow

### Block Production

```
Minting Transaction Found
         │
         ▼
┌─────────────────┐
│   Submit to     │
│   Consensus     │
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│  SCP Quorum     │
│  Decides Winner │
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│  Block Built    │
│  from Consensus │
│  Output         │
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│  Broadcast to   │
│  Network        │
└─────────────────┘
```

### Transaction Flow

```
User creates transaction
         │
         ▼
┌─────────────────┐
│  Add to Local   │
│  Mempool        │
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│  Broadcast via  │
│  Gossipsub      │
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│  Minter includes │
│  in Block       │
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│  Block reaches  │
│  Consensus      │
└─────────────────┘
```

## Directory Structure

```
botho/src/
├── main.rs              # Entry point, CLI parser
├── config.rs            # TOML config loading/saving
├── block.rs             # Block, BlockHeader, MintingTx, emission schedule
├── transaction.rs       # Transaction, TxInput, TxOutput with stealth addresses
├── wallet.rs            # BIP39 mnemonic, key derivation, signing
├── mempool.rs           # Transaction pool with fee-based priority
├── ledger/
│   ├── mod.rs           # Error types, ChainState
│   └── store.rs         # LMDB storage for blocks and UTXOs
├── node/
│   ├── mod.rs           # Node orchestrator
│   └── minter.rs         # Multi-threaded PoW minting
├── consensus/
│   ├── mod.rs           # Module exports
│   ├── service.rs       # SCP wrapper (ConsensusService)
│   ├── value.rs         # ConsensusValue type
│   ├── validation.rs    # Minting tx and transfer tx validation
│   └── block_builder.rs # Build blocks from consensus output
├── network/
│   ├── mod.rs           # Module exports
│   ├── discovery.rs     # libp2p gossip-based discovery
│   ├── sync.rs          # Block sync protocol with DDoS protection
│   ├── quorum.rs        # Quorum builder and validation
│   └── reputation.rs    # Peer latency/reliability tracking
├── rpc/
│   └── mod.rs           # JSON-RPC 2.0 server for thin wallets
└── commands/
    ├── mod.rs
    ├── init.rs          # Wallet initialization
    ├── run.rs           # Main node loop
    ├── status.rs        # Status display
    ├── balance.rs       # Balance query
    ├── address.rs       # Address display
    └── send.rs          # Transaction creation
```

## Cryptographic Foundations

Botho inherits battle-tested cryptography from MobileCoin:

| Component | Implementation |
|-----------|----------------|
| Key derivation | BIP39 + custom derivation |
| Signatures | Ed25519 (Schnorr) |
| Stealth addresses | CryptoNote protocol |
| Hashing | SHA-256 (PoW), Blake2b (general) |

## Block Timing Architecture

Botho uses a **dual timing system** that separates economic calculations from network efficiency:

### 1. Monetary Policy Block Time (5s assumed)

**Location**: `botho/src/monetary.rs` → `mainnet_policy()`

The monetary policy assumes **5-second blocks** for all economic calculations:
- Halving schedule
- Block reward calculations
- Inflation rate projections
- Difficulty adjustment epochs

This 5s assumption represents the **minimum block time under high load**. All emission schedules are calibrated to this baseline.

```rust
// From mainnet_policy():
target_block_time_secs: 5,    // Assumed for monetary calculations
min_block_time_secs: 3,       // Absolute floor (consensus needs time)
max_block_time_secs: 60,      // Absolute ceiling
halving_interval: 12_614_400, // ~2 years at 5s blocks
```

### 2. Dynamic Block Timing (5-40s actual)

**Location**: `botho/src/block.rs` → `dynamic_timing` module

Actual block production timing **adapts to network load**:

| Transaction Rate | Block Time | Use Case |
|------------------|------------|----------|
| 20+ tx/s | 3s | Very high load |
| 5+ tx/s | 5s | High load |
| 1+ tx/s | 10s | Medium load |
| 0.2+ tx/s | 20s | Low load |
| < 0.2 tx/s | 40s | Idle network |

### How They Interact

These systems are **complementary**, not competing:

```
┌─────────────────────────────────────────────────────────────────┐
│                    MONETARY POLICY (5s assumed)                  │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  Halving schedule, emission rate, difficulty targets     │   │
│  │  All calculations assume busy network (5s blocks)        │   │
│  └──────────────────────────────────────────────────────────┘   │
│                              │                                   │
│                              ▼                                   │
│                     ┌────────────────┐                          │
│                     │ Actual blocks  │                          │
│                     │ 5-40s dynamic  │                          │
│                     └────────────────┘                          │
│                              │                                   │
│                              ▼                                   │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  EFFECTIVE INFLATION = scheduled emission × (5s/actual)  │   │
│  └──────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

### Natural Inflation Dampening

This design creates a **self-regulating inflation dampener**:

| Actual Block Time | Effective Inflation | Halving Period |
|-------------------|---------------------|----------------|
| 5s (high load) | 2.0%/year (full) | ~2 years |
| 20s (normal) | 0.5%/year | ~8 years |
| 40s (idle) | 0.25%/year | ~16 years |

**Benefits**:
- Busy network (high utility) → Full emission rewards participants
- Idle network (low utility) → Reduced inflation preserves value
- No manual intervention required

### Why Not Just 60s?

The `cluster-tax` library provides a **configurable default** of 60s blocks:

```rust
// cluster-tax/src/monetary.rs - library defaults
target_block_time_secs: 60,
min_block_time_secs: 45,
max_block_time_secs: 90,
```

Botho's **mainnet policy overrides** this with 5s baseline because:
1. SCP consensus is faster than Bitcoin's probabilistic finality
2. Dynamic timing provides efficiency without sacrificing finality latency
3. Under load, users get faster confirmations

The library default exists for projects that want simpler, more predictable timing.

## Differences from MobileCoin

| MobileCoin | Botho | Notes |
|------------|---------|-------|
| gRPC daemon | CLI foreground | No server process |
| Multi-monitor | Single wallet | One mnemonic, one account |
| SGX attestation | Simple TLS | No trusted enclaves |
| Fog integration | Removed | Not needed |
| Ring signatures | Plain Ed25519 | Simplified (ring sigs planned) |
