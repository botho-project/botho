# Cadence CLI - Implementation Plan

## Overview

A single foreground process that syncs the blockchain, manages a wallet, and optionally mines.

## Architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│                              cadence                                  │
│                                                                       │
│  ┌─────────────┐  ┌─────────────┐  ┌──────────┐  ┌───────────────┐  │
│  │   Network   │  │   Wallet    │  │  Miner   │  │   Consensus   │  │
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

## Implementation Status

### CLI Commands

| Command | Status | Description |
|---------|--------|-------------|
| `cadence init` | ✅ Complete | Create wallet, generate 24-word mnemonic |
| `cadence init --recover` | ✅ Complete | Recover wallet from existing mnemonic |
| `cadence run` | ✅ Complete | Sync blockchain + scan wallet + network |
| `cadence run --mine` | ✅ Complete | Run with mining enabled |
| `cadence status` | ✅ Complete | Show sync status, balance, mining stats |
| `cadence send <addr> <amt>` | ✅ Complete | Send credits (saves to pending file) |
| `cadence balance` | ✅ Complete | Show wallet balance and UTXO count |
| `cadence address` | ✅ Complete | Show receiving address |

### Core Modules

| Module | Status | Notes |
|--------|--------|-------|
| CLI Entry Point | ✅ Complete | clap-based parser with 6 commands |
| Config Module | ✅ Complete | TOML config with secure file permissions |
| Wallet Module | ✅ Complete | BIP39 mnemonic, single account, Schnorr signatures |
| Ledger Storage | ✅ Complete | LMDB-backed with UTXO tracking |
| Mempool | ✅ Complete | Fee-based priority, double-spend detection |
| Miner | ✅ Complete | Multi-threaded PoW, work updates |
| Transaction Builder | ✅ Complete | UTXO selection, change outputs, signing |
| Network Discovery | ✅ Complete | libp2p gossipsub for peer discovery |
| Consensus Service | ⚠️ Partial | SCP wrapper exists, basic integration |
| Wallet Scanner | ❌ TODO | Scan blocks for incoming transactions |
| Block Sync | ❌ TODO | Sync historical blocks from peers |

## Config File

Location: `~/.cadence/config.toml`

```toml
[wallet]
# BIP39 mnemonic (24 words) - KEEP SECRET
mnemonic = "word1 word2 word3 ... word24"

[network]
# Port for gossip protocol
gossip_port = 8443

# Bootstrap peers for network discovery
bootstrap_peers = [
    "/ip4/192.168.1.100/tcp/8443",
    "/ip4/192.168.1.101/tcp/8443",
]

# Quorum configuration for consensus
[network.quorum]
threshold = 2  # Need 2 peers to agree

[mining]
enabled = false
threads = 0  # 0 = auto-detect CPU count
```

## Data Directory

```
~/.cadence/
├── config.toml      # Configuration + wallet mnemonic
├── ledger.db/       # Blockchain (LMDB)
│   ├── data.mdb
│   └── lock.mdb
└── pending_txs.bin  # Pending transactions (from send command)
```

## Block Structure (Implemented)

```rust
struct Block {
    header: BlockHeader,
    mining_tx: MiningTx,           // Coinbase transaction
    transactions: Vec<Transaction>, // User transactions
}

struct BlockHeader {
    version: u32,
    prev_block_hash: [u8; 32],
    tx_root: [u8; 32],             // Merkle root of transactions
    timestamp: u64,
    height: u64,
    difficulty: u64,
    nonce: u64,                    // PoW solution
    miner_view_key: [u8; 32],      // Recipient view public key
    miner_spend_key: [u8; 32],     // Recipient spend public key
}

struct MiningTx {
    block_height: u64,
    reward: u64,                   // In picocredits
    recipient_view_key: [u8; 32],
    recipient_spend_key: [u8; 32],
    output_key: [u8; 32],          // One-time output key
    prev_block_hash: [u8; 32],
    timestamp: u64,
    difficulty: u64,
    nonce: u64,
}

// Proof of work validation:
// SHA256(nonce || prev_block_hash || miner_view_key || miner_spend_key) < difficulty_target
```

## Transaction Structure (Implemented)

```rust
struct Transaction {
    version: u32,
    inputs: Vec<TxInput>,
    outputs: Vec<TxOutput>,
    fee: u64,                      // In picocredits
    created_at_height: u64,
}

struct TxInput {
    prev_tx_hash: [u8; 32],
    output_index: u32,
    signature: [u8; 64],           // Schnorr signature
}

struct TxOutput {
    amount: u64,                   // In picocredits
    recipient_view_key: [u8; 32],
    recipient_spend_key: [u8; 32],
    output_key: [u8; 32],          // One-time output key
}
```

## Mining Economics (Implemented)

### Emission Schedule (Monero-style)

```rust
const TOTAL_SUPPLY: u64 = 18_446_744_073_709_551_615; // ~18.4 quintillion picocredits
const EMISSION_SPEED_FACTOR: u64 = 20;                // Divide by 2^20 per block
const TAIL_EMISSION: u64 = 600_000_000_000;           // 0.6 credits per block
const PICOCREDITS_PER_CREDIT: u64 = 1_000_000_000_000; // 10^12
```

Block reward formula:
```
reward = max(TAIL_EMISSION, (TOTAL_SUPPLY - total_mined) >> EMISSION_SPEED_FACTOR)
```

### Difficulty Adjustment (Implemented)

Uses a simple ratio-based adjustment every 10 blocks:

```rust
const TARGET_BLOCK_TIME: u64 = 20;    // seconds
const ADJUSTMENT_WINDOW: u64 = 10;    // blocks
const MAX_ADJUSTMENT: u64 = 4;        // Maximum 4x change per adjustment

fn calculate_new_difficulty(
    current_difficulty: u64,
    window_start_time: u64,
    window_end_time: u64,
    blocks_in_window: u64,
) -> u64 {
    let actual_time = window_end_time - window_start_time;
    let expected_time = blocks_in_window * TARGET_BLOCK_TIME;

    // Ratio of expected to actual (clamped to prevent extreme swings)
    let ratio = (expected_time as f64) / (actual_time as f64);
    let clamped_ratio = ratio.clamp(1.0 / MAX_ADJUSTMENT, MAX_ADJUSTMENT);

    (current_difficulty as f64 * clamped_ratio) as u64
}
```

### Transaction Fees

- Minimum fee: 0.0001 credits (100,000,000 picocredits)
- Fees go to the miner who includes the transaction
- Mempool prioritizes by fee-per-byte

## Network Protocol (Implemented)

Uses libp2p with gossipsub for peer-to-peer communication:

```
Topics:
  cadence/blocks/1.0.0        - Block announcements
  cadence/transactions/1.0.0  - Transaction broadcasts
  cadence/scp/1.0.0           - SCP consensus messages
```

### Peer Discovery Flow

1. Node starts and dials bootstrap peers
2. Subscribes to gossip topics
3. Announces presence via gossipsub
4. Discovers additional peers through gossip
5. Maintains peer table with last-seen timestamps

## Consensus Integration (Partial)

The SCP (Stellar Consensus Protocol) integration provides Byzantine fault tolerance:

```rust
struct ConsensusValue {
    tx_hash: [u8; 32],
    is_mining_tx: bool,
    priority: u64,     // PoW priority for mining txs, 0 for regular txs
}

enum ConsensusEvent {
    SlotExternalized { slot_index: u64, values: Vec<ConsensusValue> },
    BroadcastMessage(ScpMessage),
    Progress { slot_index: u64, phase: String },
}
```

**Current Status**: Basic framework in place. ConsensusService wraps mc-consensus-scp crate. Full integration with run command needs work.

## Remaining Work

### High Priority

1. **Wallet Scanning** - Scan incoming blocks for transactions addressed to wallet
   - Check each output against wallet view key
   - Update UTXO set when matches found
   - Required for receiving payments

2. **Full Consensus Integration** - Wire ConsensusService into main run loop
   - Mining transactions should go through SCP before being added to ledger
   - Block building from externalized values needs testing
   - Handle consensus failures and timeouts

### Medium Priority

3. **Historical Block Sync** - Download blocks from peers when joining network
   - Request blocks by height range
   - Validate and add to ledger
   - Required for new nodes joining existing network

4. **Transaction Validation** - Complete validation of incoming transactions
   - Verify signatures
   - Check UTXO existence and non-double-spend
   - Validate amounts and fees

### Lower Priority

5. **Ring Signatures** - Replace simple Schnorr with ring signatures for sender privacy
   - Currently uses plain Ed25519 signatures
   - Need to add decoy inputs and ring construction

6. **One-Time Addresses** - Generate proper one-time output keys
   - Currently marked as TODO in block.rs
   - Need Diffie-Hellman key exchange for stealth addresses

7. **Fee Market** - Dynamic fee based on mempool congestion
   - Currently uses fixed minimum fee

## Simplifications from MobileCoin

| MobileCoin | Cadence | Notes |
|------------|---------|-------|
| gRPC daemon | CLI foreground | No server, direct commands |
| Multi-monitor | Single wallet | One mnemonic, one account |
| SGX attestation | Simple TLS | No trusted enclaves |
| Fog integration | Removed | Not needed |
| T3 integration | Removed | Not needed |
| Watcher thread | Removed | Simplified |
| Complex config | Single TOML | Everything in one file |
| Ring signatures | Plain Ed25519 | Simplified (for now) |

## Code Reuse from MobileCoin

### Kept (mostly as-is)
- `mc-account-keys` - Key derivation
- `mc-crypto-*` - Cryptographic primitives
- `mc-consensus-scp` - Stellar Consensus Protocol
- `mc-common` - Shared types (NodeID, ResponderId)

### Heavily Modified
- `mobilecoind` → `cadence` - Complete rewrite as CLI

### Removed Entirely
- `fog/*` - Privacy-preserving mobile sync
- `sgx/*` - Intel SGX support
- `attest/*` - Remote attestation
- `mobilecoind-json` - HTTP wrapper
- `watcher/*` - Block verification service

## File Structure

```
cadence/src/
├── main.rs              # Entry point, CLI parser
├── config.rs            # TOML config loading/saving
├── block.rs             # Block, BlockHeader, MiningTx, emission schedule
├── transaction.rs       # Transaction, TxInput, TxOutput, UTXO types
├── wallet.rs            # BIP39 mnemonic, key derivation, signing
├── mempool.rs           # Transaction pool with fee-based priority
├── ledger/
│   ├── mod.rs           # Error types, ChainState
│   └── store.rs         # LMDB storage for blocks and UTXOs
├── node/
│   ├── mod.rs           # Node orchestrator
│   └── miner.rs         # Multi-threaded PoW mining
├── consensus/
│   ├── mod.rs           # Module exports
│   ├── service.rs       # SCP wrapper (ConsensusService)
│   ├── value.rs         # ConsensusValue type
│   ├── validation.rs    # Transaction validation stubs
│   └── block_builder.rs # Build blocks from consensus output
├── network/
│   ├── mod.rs           # Module exports
│   ├── discovery.rs     # libp2p gossip-based discovery
│   └── quorum.rs        # Quorum builder and validation
└── commands/
    ├── mod.rs
    ├── init.rs          # Wallet initialization
    ├── run.rs           # Main node loop
    ├── status.rs        # Status display
    ├── balance.rs       # Balance query
    ├── address.rs       # Address display
    └── send.rs          # Transaction creation
```
