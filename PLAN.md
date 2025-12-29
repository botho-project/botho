# Botho CLI - Implementation Plan

## Overview

A single foreground process that syncs the blockchain, manages a wallet, and optionally mines.

## Architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│                               botho                                   │
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
| `botho init` | ✅ Complete | Create wallet, generate 24-word mnemonic |
| `botho init --recover` | ✅ Complete | Recover wallet from existing mnemonic |
| `botho run` | ✅ Complete | Sync blockchain + scan wallet + network |
| `botho run --mine` | ✅ Complete | Run with mining enabled |
| `botho status` | ✅ Complete | Show sync status, balance, mining stats |
| `botho send <addr> <amt>` | ✅ Complete | Send credits (saves to pending file) |
| `botho balance` | ✅ Complete | Show wallet balance and UTXO count |
| `botho address` | ✅ Complete | Show receiving address |

### Core Modules

| Module | Status | Notes |
|--------|--------|-------|
| CLI Entry Point | ✅ Complete | clap-based parser with 6 commands |
| Config Module | ✅ Complete | TOML config with secure file permissions |
| Wallet Module | ✅ Complete | BIP39 mnemonic, single account, stealth addresses |
| Ledger Storage | ✅ Complete | LMDB-backed with UTXO tracking |
| Mempool | ✅ Complete | Fee-based priority, double-spend detection |
| Miner | ✅ Complete | Multi-threaded PoW, work updates |
| Transaction Builder | ✅ Complete | UTXO selection, change outputs, stealth outputs |
| Network Discovery | ✅ Complete | libp2p gossipsub for peer discovery |
| Stealth Addresses | ✅ Complete | CryptoNote-style one-time keys |
| Wallet Scanner | ✅ Complete | TxOutput::belongs_to() with view key scanning |
| Block Sync | ✅ Complete | Request-response protocol with DDoS protection |
| Peer Reputation | ✅ Complete | EMA latency tracking, success/failure scoring |
| Transaction Validation | ⚠️ Partial | Mining tx PoW verified; transfer tx signature verification TODO |
| JSON-RPC Server | ✅ Complete | Full JSON-RPC 2.0 API for thin wallets |
| Consensus Service | ⚠️ Partial | SCP wrapper exists, needs run loop integration |

## Config File

Location: `~/.botho/config.toml`

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
mode = "recommended"  # or "explicit"
min_peers = 1         # For recommended mode: minimum peers before mining
threshold = 2         # For explicit mode: required agreement count
members = []          # For explicit mode: list of trusted peer IDs

[mining]
enabled = false
threads = 0  # 0 = auto-detect CPU count
```

## Data Directory

```
~/.botho/
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
  botho/blocks/1.0.0        - Block announcements
  botho/transactions/1.0.0  - Transaction broadcasts
  botho/scp/1.0.0           - SCP consensus messages
```

### Peer Discovery Flow

1. Node starts and dials bootstrap peers
2. Subscribes to gossip topics
3. Announces presence via gossipsub
4. Discovers additional peers through gossip
5. Maintains peer table with last-seen timestamps

## Network Launch Strategy

### Overview

Botho uses a self-organizing quorum model based on Stellar's Federated Byzantine Agreement (FBA). Each node chooses its own quorum configuration, and the network's consensus cluster emerges from overlapping trust relationships.

**Key principle**: Solo nodes cannot mine. Mining requires a satisfiable quorum with at least one other peer.

### Launch Phases

```
Phase 0: Bootstrap Server Only
┌─────────────────────────────────────────────────────────────────┐
│   Bootstrap (hosted by Botho team)                              │
│   - Participates in consensus (no mining)                       │
│   - Provides peer discovery                                     │
│   - Suggests default quorum configuration                       │
└─────────────────────────────────────────────────────────────────┘

Phase 1: First Miner Joins (2-of-2)
┌─────────────────┐       ┌─────────────────┐
│   Bootstrap     │◄─────►│    Miner 1      │
│   (no mining)   │       │   (mining)      │
└─────────────────┘       └─────────────────┘
         └── 2-of-2 consensus, mining starts ──┘

Phase 2: Network Grows (n-of-N)
┌─────────────────┐       ┌─────────────────┐       ┌─────────────────┐
│   Bootstrap     │◄─────►│    Miner 1      │◄─────►│    Miner 2      │
│   (may retire)  │       │                 │       │                 │
└─────────────────┘       └─────────────────┘       └─────────────────┘
                                   │
                          ┌────────┴────────┐
                          │    Miner 3      │
                          │                 │
                          └─────────────────┘
         └── Threshold scales with BFT formula ──┘
```

### Quorum Configuration Modes

Users can configure their quorum in `config.toml`:

#### Recommended Mode (Default)

Automatically trusts discovered peers and calculates BFT threshold:

```toml
[network.quorum]
mode = "recommended"
min_peers = 1  # Minimum peers before mining can start
```

- Trusts all connected peers
- Threshold calculated as `ceil(2n/3)` for BFT safety
- Good for most users who want to join the main network

#### Explicit Mode

User explicitly lists trusted peer IDs:

```toml
[network.quorum]
mode = "explicit"
threshold = 2
members = [
  "12D3KooWBootstrap...",  # Bootstrap server
  "12D3KooWMiner1...",     # Specific trusted miner
]
```

- Only counts listed peers toward quorum
- Threshold is fixed (user-defined)
- Good for private networks or specific trust relationships

### BFT Threshold Calculation

The threshold uses the formula `n = 3f + 1` where `f` = failures tolerated:

| Nodes | Threshold | Fault Tolerance | Notes |
|-------|-----------|-----------------|-------|
| 2     | 2-of-2    | 0               | Any failure halts chain |
| 3     | 2-of-3    | 1               | Minimum recommended |
| 4     | 3-of-4    | 1               | |
| 5     | 4-of-5    | 1               | |
| 6     | 4-of-6    | 2               | |
| 7     | 5-of-7    | 2               | Stellar's current Tier 1 |
| 13    | 9-of-13   | 4               | Stellar's 2025 target |

```rust
fn calculate_threshold(n: usize) -> usize {
    let f = (n - 1) / 3;  // Failures tolerated
    n - f                  // Threshold = n - f ≈ ceil(2n/3)
}
```

### Dynamic Mining Eligibility

Mining is gated by quorum satisfiability:

```
┌─────────────────────────────────────────────────────────────────┐
│                    Mining State Machine                          │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│   ┌──────────┐  peer connects    ┌──────────┐                   │
│   │ WAITING  │ ───────────────► │  MINING  │                   │
│   │          │  quorum met       │          │                   │
│   └──────────┘ ◄─────────────── └──────────┘                   │
│                  peer disconnects                                │
│                  quorum lost                                     │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

- On peer connect: Re-evaluate quorum, start mining if satisfied
- On peer disconnect: Re-evaluate quorum, stop mining if lost
- Solo mining is impossible by design

### Self-Organizing Quorum

Nodes with bad quorum configurations self-punish:

| Bad Config | Consequence |
|------------|-------------|
| Trust only yourself | Isolated, mine worthless blocks |
| Trust too few nodes | Chain halts when they go offline |
| Trust nodes outside main cluster | Follow a minority fork |
| Threshold too low | Accept blocks others reject |
| Threshold too high | Halt more often |

**Economic incentive**: Mined coins are only valuable if the main cluster accepts your blocks. Bad configuration = wasted electricity.

### Bootstrap Node Retirement

The bootstrap node can eventually retire:

1. Network reaches critical mass (e.g., 7+ stable miners)
2. Bootstrap announces retirement (gives warning period)
3. Miners update quorum configs to remove bootstrap
4. Bootstrap stops participating
5. Network continues with miner-only quorum

### Example Configurations

**For bootstrap server (no mining)**:
```toml
[mining]
enabled = false

[network.quorum]
mode = "recommended"
min_peers = 1
```

**For first miner joining network**:
```toml
[mining]
enabled = true
threads = 4

[network.quorum]
mode = "explicit"
threshold = 2
members = ["12D3KooWBootstrapPeerIdHere..."]
```

**For established miner (recommended mode)**:
```toml
[mining]
enabled = true

[network.quorum]
mode = "recommended"
min_peers = 2  # Wait for at least 2 peers
```

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

### Critical (Security) - FIXED

1. ~~**Transaction Signature Verification**~~ ✅ FIXED
   - Added `TxInput::verify_signature()` in `transaction.rs`
   - Mempool now verifies Schnorrkel signatures against UTXO target_key
   - Uses same domain separator ("botho-tx-v1") as signing

2. ~~**Fix Weak RNG in Wallet**~~ ✅ FIXED
   - `botho-wallet/src/transaction.rs:70` now uses `OsRng.fill_bytes()`
   - Stealth address entropy is cryptographically secure

### High Priority

3. ~~**Full Consensus Integration**~~ ✅ IMPROVED
   - ConsensusService fully wired into main run loop
   - Mining transactions go through SCP before ledger
   - Block building from externalized values implemented
   - Fixed: QuorumSet now uses real PeerIds
   - Fixed: NodeID derived from local PeerId
   - Fixed: Slot index syncs with ledger height

4. **Windows File Permissions** - Security issue for Windows users
   - `botho-wallet/src/storage.rs:150-153` has no permission restrictions on Windows
   - Need Windows ACL API integration

5. ~~**Amount Parsing Safety**~~ ✅ FIXED
   - `commands/send.rs` now uses explicit rounding and bounds checking
   - Maximum amount limit (18,000 credits) prevents overflow

### Medium Priority

6. ~~**Timestamp Monotonicity**~~ ✅ FIXED
   - Added `tip_timestamp` field to ChainState
   - Mining tx validation now checks `timestamp >= parent_timestamp`
   - Rejects blocks with timestamps before their parent

7. **Peer Reputation Integration** - Wire reputation into peer selection
   - `network/reputation.rs` fully implemented with EMA tracking
   - Needs integration into sync peer selection logic
   - Ban peers with < 25% success rate

8. **Ring Signatures** - Replace simple Schnorr with ring signatures for sender privacy
   - Currently uses plain Ed25519 signatures
   - Need to add decoy inputs and ring construction
   - Would complete Monero-style sender unlinkability

9. **Error Handling Cleanup** - Replace unwrap() calls
   - 40+ `unwrap()` calls in hot paths (`rpc/mod.rs`, `node/mod.rs`, `run.rs`)
   - Can cause panics/DoS if invariants violated
   - Replace with proper error handling

### Lower Priority

10. **Fee Market** - Dynamic fee based on mempool congestion
    - Currently uses fixed minimum fee (0.0001 credits)
    - Could add fee estimation based on recent blocks

11. **Web Dashboard Polish** - Improve the web-based dashboard
    - Currently has mining, network, wallet, ledger pages
    - Add real-time updates via WebSocket
    - Improve mobile responsiveness

12. **Transaction Size Limits** - Add max size validation before deserialization

## Recently Completed

### Stealth Addresses (One-Time Keys)
- ✅ CryptoNote-style protocol in `transaction.rs`
- ✅ `TxOutput::new()` generates random ephemeral key, computes target_key and public_key
- ✅ `TxOutput::belongs_to()` checks ownership using view key
- ✅ `TxOutput::recover_spend_key()` recovers one-time private key for spending
- ✅ Uses `mc_crypto_ring_signature::onetime_keys` for key derivation

### Wallet Scanning
- ✅ `TxOutput::belongs_to(account)` returns subaddress index if owned
- ✅ Checks default subaddress (index 0) and change subaddress (index 1)
- ✅ Integrates with AccountKey view/spend key pairs

### Block Sync Protocol
- ✅ `network/sync.rs` (771 lines) with full implementation
- ✅ DDoS protections: rate limiting (60 req/min), size limits (10MB response)
- ✅ Request-response pattern via libp2p
- ✅ Batched block requests (100 blocks per request)

### Transaction Validation
- ✅ `consensus/validation.rs` with TransactionValidator
- ✅ Mining tx: PoW, height, difficulty, reward, timestamp validation
- ✅ Transfer tx: structural validation (inputs, outputs, amounts)
- ✅ Batch validation support

### Peer Reputation
- ✅ `network/reputation.rs` with full implementation
- ✅ EMA latency tracking (alpha=0.3)
- ✅ Success/failure counting
- ✅ Score calculation: `successes / (successes + failures) * (1000 / latency_ms)`
- ✅ Peer selection API with score-based ordering

### JSON-RPC Server
- ✅ `rpc/mod.rs` with JSON-RPC 2.0 API
- ✅ Methods: getBlockByHeight, getBlockByHash, getChainInfo, getMempoolInfo, etc.
- ✅ Static file serving for web dashboard
- ✅ CORS support for browser clients

## Simplifications from MobileCoin

| MobileCoin | Botho | Notes |
|------------|-------|-------|
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
- `bt-account-keys` - Key derivation
- `bt-crypto-*` - Cryptographic primitives
- `bt-consensus-scp` - Stellar Consensus Protocol
- `bt-common` - Shared types (NodeID, ResponderId)

### Heavily Modified
- `mobilecoind` → `botho` - Complete rewrite as CLI

### Removed Entirely
- `fog/*` - Privacy-preserving mobile sync
- `sgx/*` - Intel SGX support
- `attest/*` - Remote attestation
- `mobilecoind-json` - HTTP wrapper
- `watcher/*` - Block verification service

## File Structure

```
botho/src/
├── main.rs              # Entry point, CLI parser
├── config.rs            # TOML config loading/saving
├── block.rs             # Block, BlockHeader, MiningTx, emission schedule
├── transaction.rs       # Transaction, TxInput, TxOutput with stealth addresses
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
│   ├── validation.rs    # Mining tx and transfer tx validation
│   └── block_builder.rs # Build blocks from consensus output
├── network/
│   ├── mod.rs           # Module exports
│   ├── discovery.rs     # libp2p gossip-based discovery
│   ├── sync.rs          # Block sync protocol with DDoS protection
│   ├── quorum.rs        # Quorum builder and validation
│   └── reputation.rs    # Peer latency/reliability tracking (EMA)
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

## Dependency Modernization

### Completed

| Old | New | Status |
|-----|-----|--------|
| `scoped_threadpool` | Removed (unused) | ✅ Removed |
| `json` | Removed (unused) | ✅ Removed |
| `rjson` | Removed (unused) | ✅ Removed |
| `lazy_static` | `std::sync::LazyLock` | ✅ Migrated in botho/ crate |
| `once_cell` | `std::sync::OnceLock` | ✅ Migrated in botho/ crate |

### In Progress

| Old | New | Status |
|-----|-----|--------|
| `lazy_static` | `std::sync::LazyLock` | ⚠️ Inherited Botho crates still use lazy_static |
| `yaml-rust` | Removed (unused) | ⚠️ Still in workspace, unused |
| `serde_cbor` | `ciborium` | ⚠️ Both present, migration incomplete |

### Recently Completed

| Old | New | Status |
|-----|-----|--------|
| `grpcio` | `tonic` | ✅ Complete - watcher/SGX excluded, all active crates use tonic |

### Pending

| Old | New | Status |
|-----|-----|--------|
| `rusoto_*` | `aws-sdk-*` | ⏳ Used in ledger/distribution |
| `slog` | `tracing` | ⏳ botho/ already migrated, inherited crates remain |
| `mbedtls` | `rustls` | ⏳ Low priority |
| `lmdb-rkv` | `heed` or `redb` | ⏳ Low priority |
| `protobuf 2` | `prost` | ⏳ Already using prost in many places |

**Notes**:
- `transaction/core` is `#![no_std]` and must continue using `lazy_static`
- Many inherited Botho crates still use older patterns

## Security Audit

**Last Audit:** 2025-12-29
**Full Report:** `docs/SECURITY_AUDIT.md`

### Summary

| Severity | Count | Status |
|----------|-------|--------|
| Critical | 2 | ✅ Fixed |
| High | 2 | 1 fixed, 1 open |
| Medium | 4 | Should fix |
| Low | 2 | Nice to fix |

### Critical Issues

| Issue | Location | Status |
|-------|----------|--------|
| Signature verification not implemented | `mempool.rs`, `transaction.rs` | ✅ Fixed |
| Weak RNG in wallet stealth outputs | `botho-wallet/src/transaction.rs` | ✅ Fixed |

### High Issues

| Issue | Location | Status |
|-------|----------|--------|
| No Windows file permissions | `botho-wallet/src/storage.rs:150-153` | ❌ Open |
| Float precision loss in amounts | `commands/send.rs` | ✅ Fixed |

### Positive Findings

- No `unsafe` code in botho/
- ChaCha20-Poly1305 + Argon2id for wallet encryption
- DDoS protections (rate limiting, size limits)
- Double-spend detection in mempool
- Overflow protection with `checked_add()`
- `OsRng` used correctly in main node (issue is wallet crate only)

## Quantum Resistance

### Threat Model

```
┌─────────────────────────────────────────────────────────────────────────┐
│                    HARVEST NOW, DECRYPT LATER                           │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                         │
│   2025: Adversary archives blockchain                                   │
│         ↓                                                               │
│   2035: Adversary obtains quantum computer                              │
│         ↓                                                               │
│   Shor's algorithm breaks ECDLP:                                        │
│   - Recover view private keys → de-anonymize ALL historical outputs     │
│   - Recover spend private keys → steal funds from old addresses         │
│                                                                         │
│   PRIVACY LOSS IS IRREVERSIBLE. THEFT IS MITIGABLE VIA HARD FORK.       │
│                                                                         │
└─────────────────────────────────────────────────────────────────────────┘
```

### Strategy: Quantum-Safe Private Transactions

**Key Insight**: Only private transactions need post-quantum protection NOW.

| Concern | Urgency | Mitigation |
|---------|---------|------------|
| Privacy | **Immediate** | PQ crypto on private tx (harvest-now threat) |
| Theft | Deferrable | Hard fork when QC imminent (require new addresses) |

Standard transactions continue using classical crypto. When quantum computers
become practical, a hard fork will require:
1. New PQ-safe addresses for all users
2. Migration of funds from classical addresses
3. Deadline after which classical signatures rejected

### Cryptographic Primitives

**NIST Post-Quantum Standards (2024):**

| Primitive | Algorithm | Sizes | Use Case |
|-----------|-----------|-------|----------|
| KEM | ML-KEM-768 (Kyber) | pk: 1184 B, ct: 1088 B, ss: 32 B | Stealth address key exchange |
| Signature | ML-DSA-65 (Dilithium) | pk: 1952 B, sig: 2420 B | Transaction signing |

**Rust Crates:**
- `pqcrypto-kyber` - ML-KEM implementation
- `pqcrypto-dilithium` - ML-DSA implementation
- Both from the `pqcrypto` family (wraps PQClean)

### Transaction Types

```rust
enum TransactionType {
    /// Classical stealth addresses, Schnorr signatures
    /// Quantum-vulnerable but upgradeable via hard fork
    Standard = 0,

    /// Hybrid classical + post-quantum
    /// Both crypto layers must verify
    /// Privacy protected against quantum adversaries
    QuantumPrivate = 1,
}
```

### Hybrid Private Transaction Structure

```rust
/// Quantum-safe transaction output
struct QuantumPrivateTxOutput {
    // === Classical Layer (72 bytes) ===
    amount: u64,                    // 8 B - plaintext for now (RingCT later)
    target_key: [u8; 32],           // Ristretto one-time spend key
    public_key: [u8; 32],           // Ephemeral ECDH key (R = r*G)

    // === Post-Quantum Layer (1088 bytes) ===
    pq_ciphertext: [u8; 1088],      // ML-KEM-768 encapsulation

    // Total: 1160 bytes per output (vs 72 classical)
}

/// Quantum-safe transaction input
struct QuantumPrivateTxInput {
    // === Reference (36 bytes) ===
    tx_hash: [u8; 32],
    output_index: u32,

    // === Classical Signature (64 bytes) ===
    schnorr_sig: [u8; 64],          // Signs with classical one-time key

    // === Post-Quantum Signature (2420 bytes) ===
    dilithium_sig: [u8; 2420],      // Signs with PQ one-time key

    // Total: 2520 bytes per input (vs 100 classical)
}
```

**Validation Rule**: Both signatures MUST verify. Breaking either layer fails the tx.

### Extended Address Format

Private addresses include PQ public keys for receiving:

```rust
struct QuantumSafeAddress {
    // Classical (64 bytes)
    view_public_key: [u8; 32],      // For output scanning
    spend_public_key: [u8; 32],     // For ownership verification

    // Post-Quantum (3136 bytes)
    pq_view_public_key: [u8; 1184], // ML-KEM-768 for key encapsulation
    pq_spend_public_key: [u8; 1952],// ML-DSA-65 for signature verification

    // Total: 3200 bytes (~4.3 KB base58 encoded)
}
```

**Encoding**: `botho-pq://1/<base58(view||spend||pq_view||pq_spend)>`

Standard addresses remain unchanged: `botho://1/<base58(view||spend)>`

### Key Derivation

PQ keys derived from same mnemonic (no additional backup required):

```rust
impl QuantumSafeAccountKeys {
    fn from_mnemonic(mnemonic: &str) -> Self {
        // Classical derivation (existing)
        let classical = AccountKey::from_mnemonic(mnemonic);

        // PQ derivation (new path)
        let pq_seed = HKDF::new(
            mnemonic.as_bytes(),
            b"botho-pq-v1",
            64  // 64 bytes for both keypairs
        );

        let pq_view_keypair = MlKem768::keygen_from_seed(&pq_seed[0..32]);
        let pq_spend_keypair = MlDsa65::keygen_from_seed(&pq_seed[32..64]);

        Self { classical, pq_view_keypair, pq_spend_keypair }
    }
}
```

### Stealth Address Protocol (Quantum-Safe)

**Sender creates output:**

```
Classical (existing):
  1. Generate ephemeral scalar r
  2. Shared secret: s = H(r * view_public_key)
  3. Target key: P = s*G + spend_public_key
  4. Public key: R = r*G

Post-Quantum (new):
  5. Encapsulate to recipient's PQ view key:
     (pq_ciphertext, pq_shared_secret) = ML_KEM.Encaps(pq_view_public_key)
  6. PQ one-time public key: pq_P = ML_DSA.DerivePublic(H(pq_shared_secret))
```

**Recipient scans outputs:**

```
Classical:
  1. Compute s' = H(view_private * R)
  2. Check if P - s'*G == spend_public_key

Post-Quantum:
  3. Decapsulate: pq_ss = ML_KEM.Decaps(pq_view_private, pq_ciphertext)
  4. Derive expected PQ public key from pq_ss
  5. Verify it matches stored pq_P (implicit in signature verification later)
```

**Recipient spends:**

```
Classical:
  1. One-time private key: x = H(view_private * R) + spend_private
  2. Sign with Schnorr(x, message)

Post-Quantum:
  3. Derive PQ one-time private key from pq_shared_secret
  4. Sign with ML_DSA(pq_x, message)
```

### Size Overhead Analysis

```
Standard Transaction (2 inputs, 2 outputs):
  Inputs:  2 × 100 B  =   200 B
  Outputs: 2 ×  72 B  =   144 B
  Header:              ≈   50 B
  ─────────────────────────────
  Total:              ≈   394 B

Quantum-Private Transaction (2 inputs, 2 outputs):
  Inputs:  2 × 2520 B =  5040 B
  Outputs: 2 × 1160 B =  2320 B
  Header:              ≈    50 B
  ─────────────────────────────
  Total:              ≈  7410 B  (~19x larger)
```

**Ledger Growth Projection** (assuming 1 tx/block, 20s blocks):

| Scenario | Annual Growth |
|----------|---------------|
| 100% Standard tx | ~620 MB |
| 100% Quantum-Private tx | ~11.7 GB |
| 10% Quantum-Private tx | ~1.7 GB |

### Implementation Phases

#### Phase 1: Cryptographic Foundation

**New crate: `crypto/pq/`**

```
crypto/pq/
├── Cargo.toml          # pqcrypto-kyber, pqcrypto-dilithium deps
├── src/
│   ├── lib.rs
│   ├── kem.rs          # ML-KEM-768 wrapper
│   ├── sig.rs          # ML-DSA-65 wrapper
│   └── derive.rs       # Deterministic key derivation from seed
```

Tasks:
- [ ] Add `pqcrypto` dependencies to workspace
- [ ] Implement `MlKem768` wrapper with Encaps/Decaps
- [ ] Implement `MlDsa65` wrapper with Sign/Verify
- [ ] Implement deterministic keygen from 32-byte seed
- [ ] Unit tests for all primitives
- [ ] Benchmark: keygen, encaps, sign latency

#### Phase 2: Key Management

**Extend `account-keys/`**

Tasks:
- [ ] Add `QuantumSafeAccountKeys` struct
- [ ] Derive PQ keys from mnemonic via HKDF
- [ ] Extended address format with PQ public keys
- [ ] Address encoding/decoding (botho-pq:// scheme)
- [ ] Wallet storage for PQ keypairs
- [ ] Migration path for existing wallets (derive PQ keys on unlock)

#### Phase 3: Transaction Types

**Extend `botho/src/transaction.rs`**

Tasks:
- [ ] Define `QuantumPrivateTxOutput` and `QuantumPrivateTxInput`
- [ ] Transaction version byte to distinguish types
- [ ] PQ stealth output creation (sender side)
- [ ] PQ output scanning (recipient side)
- [ ] PQ one-time key derivation for spending
- [ ] Dual signature creation (Schnorr + Dilithium)
- [ ] Serialization/deserialization

#### Phase 4: Validation & Consensus

**Extend `botho/src/consensus/validation.rs`**

Tasks:
- [ ] Validate both classical and PQ signatures
- [ ] Reject if either signature fails
- [ ] Mempool size limits for larger tx
- [ ] Fee calculation based on tx size (PQ tx pay more)
- [ ] Block size limits accounting for PQ overhead

#### Phase 5: Wallet Integration

**Extend `botho/src/wallet.rs` and `botho-wallet/`**

Tasks:
- [ ] CLI flag: `--quantum-private` for send command
- [ ] Display PQ address in `address` command
- [ ] Scan both classical and PQ outputs
- [ ] Build quantum-private transactions
- [ ] Show tx type in history

#### Phase 6: Testing & Hardening

Tasks:
- [ ] Integration tests: full send/receive flow for PQ tx
- [ ] Test vectors from NIST PQC standards
- [ ] Fuzzing for deserialization
- [ ] Cross-platform testing (keygen determinism)
- [ ] Performance benchmarks under load
- [ ] Security review of key derivation

## Performance Optimization

### Baseline Benchmarks (Pre-Optimization)

| Nodes | k | Throughput | p50 (ms) | p95 (ms) | p99 (ms) |
|-------|---|------------|----------|----------|----------|
| 3 | 2 | 8014 tx/s | 0.00 | 117.01 | 117.01 |
| 5 | 3 | 6819 tx/s | 12.47 | 96.63 | 96.73 |
| 7 | 4 | 6258 tx/s | 27.69 | 75.56 | 83.03 |

### Post-Optimization Benchmarks (Release Build, 10k values)

| Nodes | k | Throughput | Notes |
|-------|---|------------|-------|
| 1 | 0 | 13,438 tx/s | Single node baseline |
| 2 | 1 | 12,192 tx/s | Minimal consensus |
| 3 | 1 | 8,090 tx/s | Low threshold |
| 3 | 2 | 7,126 tx/s | Standard quorum |
| 4 | 3 | 5,002 tx/s | Higher threshold |
| 5 | 3 | 3,699 tx/s | 5-node network |
| 5 | 4 | 2,326 tx/s | Near-unanimous |

### Optimizations Implemented

| Priority | Optimization | Effort | Impact | Status |
|----------|--------------|--------|--------|--------|
| 1 | Replace nodes_map Mutex with DashMap | Low | High | ✅ Done |
| 2 | Blocking channel receive (recv_timeout) | Low | Medium | ✅ Done |
| 3 | Arc<Msg> instead of cloning | Medium | Medium | ⏳ Pending |
| 4 | Quorum HashSet backtracking | High | High | ✅ Done |
| 5 | Cache to_propose BTreeSet | Low | Low | ✅ Done |

### Implementation Details

| Priority | Optimization | Effort | Impact | Status |
|----------|--------------|--------|--------|--------|
| 1 | Replace nodes_map Mutex with DashMap | Low | High | ✅ Done |
| 2 | Blocking channel receive | Low | Medium | ✅ Done |
| 3 | Arc<Msg> instead of cloning | Medium | Medium | ⏳ Pending |
| 4 | Quorum HashSet optimization | High | High | ✅ Done |
| 5 | Cache to_propose BTreeSet | Low | Low | ✅ Done |

#### 1. Lock Contention in Message Broadcasting

**Location:** `consensus/scp/tests/scp_sim.rs:598-612` - `broadcast_msg()`

**Problem:** Global `Mutex<HashMap<NodeID, SimNode>>` is locked for every broadcast. With N nodes × M messages, this creates O(N×M) sequential lock acquisitions.

**Solution Options:**
- Replace with `DashMap` (concurrent HashMap)
- Use per-node channels directly (eliminate lookup during broadcast)
- Pre-compute peer sender handles at startup

**Estimated Impact:** 30-50% throughput improvement

#### 2. Busy-Wait Loop in Node Thread

**Location:** `consensus/scp/tests/scp_sim.rs:542-544`

```rust
Err(crossbeam_channel::TryRecvError::Empty) => {
    thread::yield_now();  // CPU spinning
}
```

**Problem:** Wastes CPU cycles spinning when no messages available.

**Solution:** Use `recv_timeout()` or `select!` macro for blocking receive.

**Estimated Impact:** Lower CPU usage, better latency under load

#### 3. Quorum Finding Recursion with HashSet Cloning

**Location:** `consensus/scp/src/quorum_set_ext.rs:155-156`

```rust
let mut nodes_so_far_with_N = nodes_so_far.clone();  // Clone on every branch!
nodes_so_far_with_N.insert(N.clone());
```

**Problem:** `findQuorumHelper` creates new HashSet clones at each recursive step. Worst case O(2^n) memory allocations.

**Solution Options:**
- Use a mutable `&mut HashSet` with backtracking
- Use a bitset representation for small node sets
- Cache quorum computation results

**Estimated Impact:** 20-40% reduction in allocation overhead

### Medium Impact Optimizations

#### 4. Message Cloning in Protocol

**Location:** `consensus/scp/src/slot.rs:333, 385`

```rust
self.handle_messages(&[msg.clone()])  // Clone on every message
self.M.insert(msg.sender_id.clone(), msg.clone());
```

**Solution:** Use `Arc<Msg<V>>` throughout to share ownership without cloning.

#### 5. Repeated BTreeSet Creation

**Location:** `consensus/scp/tests/scp_sim.rs:550-554`

```rust
let to_propose: BTreeSet<TxValue> = pending_values
    .iter()
    .take(max_slot_values)
    .cloned()
    .collect();  // Rebuilt every loop iteration!
```

**Solution:** Only rebuild when `pending_values` changes.

#### 6. Message Sorting on Every Batch

**Location:** `consensus/scp/src/slot.rs:359`

**Solution:** Use a priority queue or maintain sorted order incrementally.

### Low Impact Optimizations

#### 7. Value Validation Redundancy

**Location:** `consensus/scp/src/slot.rs:374-378`

Values are validated on every incoming message, even if seen before.

**Solution:** Cache validation results per value hash.

#### 8. SmallVec for Small Collections

Many `Vec<V>` allocations for small node sets could use `SmallVec<[V; 8]>` to avoid heap allocation.

### Rollout Strategy

```
Phase    Network State                    Action
─────    ─────────────────────────────    ──────────────────────────────
  0      Classical only                   Current state

  1      PQ-aware nodes                   Deploy PQ-capable nodes
         Classical consensus              PQ tx valid but optional

  2      Majority PQ-aware                Encourage PQ for privacy
         Soft preference for PQ           Higher reputation for PQ nodes

  3      Quantum threat imminent          Hard fork announcement
         (10+ years from now?)            6-month migration window

  4      Post-fork                        Classical signatures rejected
         All tx require PQ                Old addresses frozen/burned
```

### Open Questions

1. **Ring signatures + PQ**: When we add ring signatures for sender privacy,
   how do PQ ring signatures work? Research needed (lattice-based ring sigs
   exist but are larger).

2. **Amount hiding + PQ**: Pedersen commitments are also ECDLP-based.
   Need PQ-safe commitment scheme for RingCT. Lattice commitments?

3. **Address size**: 3.2 KB addresses are unwieldy. Options:
   - QR codes (works fine)
   - Address registry service (centralization tradeoff)
   - Hybrid: short classical + derivable PQ (needs research)

4. **Signature aggregation**: Can we aggregate Dilithium signatures to
   reduce per-input overhead? (Probably not without new research)

### Dependencies

```toml
# Cargo.toml additions
[dependencies]
pqcrypto-kyber = "0.8"       # ML-KEM-768
pqcrypto-dilithium = "0.5"   # ML-DSA-65
pqcrypto-traits = "0.3"      # Common traits
```

### References

- NIST FIPS 203: ML-KEM (Kyber) Standard
- NIST FIPS 204: ML-DSA (Dilithium) Standard
- NIST FIPS 205: SLH-DSA (SPHINCS+) Standard (backup option)
- CryptoNote v2.0 Whitepaper (stealth address protocol)
- "Post-Quantum Cryptography for Blockchain" (IEEE S&P 2024)

## AWS Deployment

### Infrastructure Overview

```
┌─────────────────────────────────────────────────────────────────────────┐
│                              AWS Account                                 │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│   ┌─────────────────────┐              ┌─────────────────────┐          │
│   │   Route 53          │              │   CloudWatch        │          │
│   │   DNS for botho.io  │              │   Monitoring/Alerts │          │
│   └──────────┬──────────┘              └──────────┬──────────┘          │
│              │                                    │                      │
│   ┌──────────┴──────────────────────────────────┴──────────┐           │
│   │                                                          │           │
│   │   ┌─────────────────┐         ┌─────────────────────┐   │           │
│   │   │  Amplify        │         │  EC2 (t3.large)     │   │           │
│   │   │  botho.io       │         │  seed.botho.io      │   │           │
│   │   │                 │         │                     │   │           │
│   │   │  - Static site  │         │  - Botho node       │   │           │
│   │   │  - Web wallet   │         │  - P2P networking   │   │           │
│   │   │  - Docs         │         │  - Ledger DB        │   │           │
│   │   │  - Free tier    │         │  - Elastic IP       │   │           │
│   │   └─────────────────┘         └─────────────────────┘   │           │
│   │                                         │                │           │
│   │                               ┌─────────┴─────────┐     │           │
│   │                               │  EBS Volume       │     │           │
│   │                               │  100GB gp3        │     │           │
│   │                               │  (Ledger storage) │     │           │
│   │                               └───────────────────┘     │           │
│   │                                                          │           │
│   └──────────────────────────────────────────────────────────┘          │
│                                                                          │
└─────────────────────────────────────────────────────────────────────────┘
```

### Cost Estimate

| Service | Spec | Monthly Cost |
|---------|------|--------------|
| EC2 t3.large | 2 vCPU, 8GB RAM | ~$60 (on-demand) |
| EC2 t3.large | 1-yr reserved | ~$38 |
| EBS gp3 | 100GB | ~$8 |
| Elastic IP | 1 (attached) | $0 |
| Amplify Hosting | Static site | Free tier |
| Route 53 | Hosted zone | ~$0.50 |
| Data transfer | ~100GB/mo | ~$9 |
| **Total** | | **~$55-80/mo** |

### EC2 Setup for seed.botho.io

#### Instance Configuration

```
AMI:           Ubuntu 24.04 LTS (arm64 for cost savings, or x86_64)
Instance Type: t3.large (2 vCPU, 8GB RAM)
Storage:       100GB gp3 EBS (can expand later)
Region:        us-east-1 (or closest to target users)
```

#### Security Group Rules

```
Inbound:
  - SSH (22)           : Your IP only (or bastion)
  - P2P Gossip (8443)  : 0.0.0.0/0 (required for network)
  - JSON-RPC (8080)    : 0.0.0.0/0 (optional, for public API)

Outbound:
  - All traffic        : 0.0.0.0/0
```

#### CLI Commands

```bash
# 1. Create key pair
aws ec2 create-key-pair \
  --key-name botho-seed \
  --query 'KeyMaterial' \
  --output text > ~/.ssh/botho-seed.pem
chmod 400 ~/.ssh/botho-seed.pem

# 2. Create security group
aws ec2 create-security-group \
  --group-name botho-seed-sg \
  --description "Botho seed node security group"

SG_ID=$(aws ec2 describe-security-groups \
  --group-names botho-seed-sg \
  --query 'SecurityGroups[0].GroupId' \
  --output text)

# 3. Configure security group rules
aws ec2 authorize-security-group-ingress \
  --group-id $SG_ID \
  --protocol tcp --port 22 --cidr YOUR_IP/32

aws ec2 authorize-security-group-ingress \
  --group-id $SG_ID \
  --protocol tcp --port 8443 --cidr 0.0.0.0/0

aws ec2 authorize-security-group-ingress \
  --group-id $SG_ID \
  --protocol tcp --port 8080 --cidr 0.0.0.0/0

# 4. Launch instance
aws ec2 run-instances \
  --image-id ami-0c7217cdde317cfec \
  --instance-type t3.large \
  --key-name botho-seed \
  --security-group-ids $SG_ID \
  --block-device-mappings '[{"DeviceName":"/dev/sda1","Ebs":{"VolumeSize":100,"VolumeType":"gp3"}}]' \
  --tag-specifications 'ResourceType=instance,Tags=[{Key=Name,Value=botho-seed}]'

# 5. Allocate and associate Elastic IP
aws ec2 allocate-address --domain vpc
aws ec2 associate-address \
  --instance-id i-XXXXX \
  --allocation-id eipalloc-XXXXX
```

#### Server Setup Script

```bash
#!/bin/bash
# Run on EC2 instance after launch

# Update system
sudo apt update && sudo apt upgrade -y

# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source ~/.cargo/env

# Install build dependencies
sudo apt install -y build-essential pkg-config libssl-dev

# Clone and build
git clone https://github.com/user/botho.git
cd botho
cargo build --release --bin botho

# Create data directory
mkdir -p ~/.botho

# Copy binary to /usr/local/bin
sudo cp target/release/botho /usr/local/bin/

# Initialize wallet (generates mnemonic)
botho init

# IMPORTANT: Backup ~/.botho/config.toml mnemonic!
```

#### Systemd Service

```ini
# /etc/systemd/system/botho.service
[Unit]
Description=Botho Seed Node
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=ubuntu
Group=ubuntu
ExecStart=/usr/local/bin/botho run
Restart=always
RestartSec=10
StandardOutput=journal
StandardError=journal

# Security hardening
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths=/home/ubuntu/.botho

# Resource limits
LimitNOFILE=65535
MemoryMax=6G

[Install]
WantedBy=multi-user.target
```

```bash
# Enable and start service
sudo systemctl daemon-reload
sudo systemctl enable botho
sudo systemctl start botho

# View logs
journalctl -u botho -f
```

### Amplify Setup for botho.io

#### Project Structure

```
web/
├── pnpm-workspace.yaml
├── landing/              # Marketing site
│   ├── package.json
│   └── src/
├── wallet/               # Web wallet app
│   ├── package.json
│   └── src/
└── docs/                 # Documentation
    ├── package.json
    └── src/
```

#### Amplify Configuration

```yaml
# amplify.yml
version: 1
frontend:
  phases:
    preBuild:
      commands:
        - npm install -g pnpm
        - pnpm install
    build:
      commands:
        - pnpm build
  artifacts:
    baseDirectory: dist
    files:
      - '**/*'
  cache:
    paths:
      - node_modules/**/*
```

#### CLI Commands

```bash
# 1. Install Amplify CLI
npm install -g @aws-amplify/cli

# 2. Initialize Amplify (in web/ directory)
amplify init

# 3. Add hosting
amplify add hosting
# Select: Hosting with Amplify Console
# Select: Continuous deployment

# 4. Connect to GitHub repo
amplify hosting configure

# 5. Deploy
amplify publish
```

#### Custom Domain Setup

```bash
# In Amplify Console or via CLI:
# 1. Add custom domain: botho.io
# 2. Amplify will provision SSL certificate
# 3. Add DNS records to Route 53 (or update nameservers)
```

### Route 53 DNS Configuration

```bash
# Create hosted zone
aws route53 create-hosted-zone \
  --name botho.io \
  --caller-reference $(date +%s)

# Get hosted zone ID
ZONE_ID=$(aws route53 list-hosted-zones-by-name \
  --dns-name botho.io \
  --query 'HostedZones[0].Id' \
  --output text | cut -d'/' -f3)

# Add seed node A record
aws route53 change-resource-record-sets \
  --hosted-zone-id $ZONE_ID \
  --change-batch '{
    "Changes": [{
      "Action": "CREATE",
      "ResourceRecordSet": {
        "Name": "seed.botho.io",
        "Type": "A",
        "TTL": 300,
        "ResourceRecords": [{"Value": "ELASTIC_IP_HERE"}]
      }
    }]
  }'

# Amplify handles botho.io automatically
```

### CloudWatch Monitoring

#### Key Metrics to Monitor

| Metric | Threshold | Action |
|--------|-----------|--------|
| CPU Utilization | > 80% for 5 min | Alert |
| Memory Usage | > 90% | Alert + investigate |
| Disk Usage | > 80% | Alert + expand EBS |
| Network In/Out | Anomaly | Investigate |
| StatusCheckFailed | Any | Auto-recover |

#### CloudWatch Alarm Setup

```bash
# CPU alarm
aws cloudwatch put-metric-alarm \
  --alarm-name botho-seed-cpu \
  --alarm-description "High CPU on seed node" \
  --metric-name CPUUtilization \
  --namespace AWS/EC2 \
  --statistic Average \
  --period 300 \
  --threshold 80 \
  --comparison-operator GreaterThanThreshold \
  --dimensions Name=InstanceId,Value=i-XXXXX \
  --evaluation-periods 2 \
  --alarm-actions arn:aws:sns:us-east-1:ACCOUNT:alerts

# Auto-recovery alarm
aws cloudwatch put-metric-alarm \
  --alarm-name botho-seed-recovery \
  --alarm-description "Auto-recover seed node" \
  --metric-name StatusCheckFailed \
  --namespace AWS/EC2 \
  --statistic Maximum \
  --period 60 \
  --threshold 1 \
  --comparison-operator GreaterThanOrEqualToThreshold \
  --dimensions Name=InstanceId,Value=i-XXXXX \
  --evaluation-periods 2 \
  --alarm-actions arn:aws:automate:us-east-1:ec2:recover
```

### Deployment Checklist

#### Pre-Launch

- [ ] AWS account created and billing configured
- [ ] IAM user with appropriate permissions
- [ ] AWS CLI configured locally (`aws configure`)
- [ ] SSH key pair created
- [ ] Domain registered (botho.io)

#### Seed Node (seed.botho.io)

- [ ] EC2 instance launched
- [ ] Security group configured
- [ ] Elastic IP allocated and associated
- [ ] Botho binary built and installed
- [ ] Wallet initialized (mnemonic backed up securely!)
- [ ] Systemd service enabled
- [ ] DNS A record created
- [ ] Firewall ports verified open
- [ ] CloudWatch alarms configured

#### Web Hosting (botho.io)

- [ ] Amplify app created
- [ ] GitHub repo connected
- [ ] Build settings configured
- [ ] Custom domain added
- [ ] SSL certificate provisioned
- [ ] DNS configured (via Route 53 or registrar)

#### Post-Launch

- [ ] Verify seed node is discoverable (`botho status` from another machine)
- [ ] Verify website loads at https://botho.io
- [ ] Test P2P connectivity from external network
- [ ] Monitor CloudWatch metrics for 24 hours
- [ ] Document Elastic IP and instance IDs

### Backup Strategy

#### What to Backup

| Data | Location | Frequency | Method |
|------|----------|-----------|--------|
| Wallet mnemonic | ~/.botho/config.toml | Once (at init) | Manual, offline |
| Ledger DB | ~/.botho/ledger.db/ | Daily | EBS Snapshots |
| Config | ~/.botho/config.toml | On change | Git or S3 |

#### EBS Snapshot Automation

```bash
# Create snapshot
aws ec2 create-snapshot \
  --volume-id vol-XXXXX \
  --description "Botho ledger backup $(date +%Y-%m-%d)"

# Or use AWS Backup for automated retention
```

### Scaling (Future)

When network grows, add additional seed nodes:

```
seed.botho.io      → Round-robin DNS to multiple IPs
seed-us.botho.io   → US-East node
seed-eu.botho.io   → EU-West node
seed-ap.botho.io   → AP-Southeast node
```

Each region uses the same setup with region-specific EC2 instances.
