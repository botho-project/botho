# Consensus Architecture

This document describes Botho's consensus mechanism, block production, and validation for external security auditors.

## Table of Contents

1. [SCP Overview](#scp-overview)
2. [Ballot Protocol](#ballot-protocol)
3. [Quorum Configuration](#quorum-configuration)
4. [Block Production](#block-production)
5. [Block Validation](#block-validation)
6. [Byzantine Fault Tolerance](#byzantine-fault-tolerance)

---

## SCP Overview

Botho uses the **Stellar Consensus Protocol (SCP)** for Byzantine fault-tolerant consensus. Unlike Bitcoin's probabilistic finality, SCP provides deterministic finality once a value is externalized.

### Key Properties

| Property | Guarantee |
|----------|-----------|
| **Safety** | No two honest nodes externalize different values for the same slot |
| **Liveness** | Consensus eventually terminates under partial synchrony |
| **Byzantine Tolerance** | Tolerates up to f < n/3 malicious nodes |
| **Deterministic Finality** | Once externalized, value is final |

### Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           CONSENSUS SERVICE                                  │
│                                                                              │
│  ┌───────────────────────────────────────────────────────────────────────┐  │
│  │                           SCP NODE                                    │  │
│  │                                                                       │  │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  │  │
│  │  │   Slot      │  │   Ballot    │  │   Quorum    │  │   Message   │  │  │
│  │  │   Manager   │  │   Protocol  │  │   Set       │  │   Handler   │  │  │
│  │  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘  │  │
│  │         │                │                │                │         │  │
│  │         └────────────────┴────────────────┴────────────────┘         │  │
│  │                                    │                                  │  │
│  └────────────────────────────────────┼──────────────────────────────────┘  │
│                                       │                                     │
│                                       ▼                                     │
│  ┌───────────────────────────────────────────────────────────────────────┐  │
│  │                      TRANSACTION VALIDATOR                            │  │
│  │  ┌──────────────────┐  ┌──────────────────┐  ┌────────────────────┐  │  │
│  │  │  Mint Tx         │  │  Transfer Tx     │  │  Range Proof       │  │  │
│  │  │  Validation      │  │  Validation      │  │  Verification      │  │  │
│  │  └──────────────────┘  └──────────────────┘  └────────────────────┘  │  │
│  └───────────────────────────────────────────────────────────────────────┘  │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

**Code Reference:** `consensus/scp/src/`

---

## Ballot Protocol

SCP uses a federated voting protocol with ballots as the voting unit.

### Ballot Structure

```rust
struct Ballot<V> {
    counter: u32,    // N: Ballot number (more significant)
    values: Vec<V>,  // X: Proposed values (must be sorted)
}
```

**Ordering:** Ballots are ordered by (N, X) where N is more significant. This ensures:
- Higher counter always wins
- For same counter, lexicographic value comparison

### Message Types

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           SCP MESSAGE FLOW                                   │
│                                                                              │
│  ┌──────────────┐     ┌──────────────┐     ┌──────────────┐                │
│  │  NOMINATE    │────▶│   PREPARE    │────▶│   COMMIT     │                │
│  │              │     │              │     │              │                │
│  │ voted: X     │     │ ballot: B    │     │ ballot: B    │                │
│  │ accepted: Y  │     │ prepared: P  │     │ cn, ch       │                │
│  │              │     │ prepared': P'│     │              │                │
│  └──────────────┘     └──────────────┘     └──────┬───────┘                │
│                                                    │                        │
│                                                    ▼                        │
│                                           ┌──────────────┐                 │
│                                           │ EXTERNALIZE  │                 │
│                                           │              │                 │
│                                           │ commit: C    │                 │
│                                           │ height: H    │                 │
│                                           └──────────────┘                 │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Message Payloads

| Message | Fields | Purpose |
|---------|--------|---------|
| **Nominate** | `voted: Vec<V>`, `accepted: Vec<V>` | Propose values for slot |
| **Prepare** | `ballot: B`, `prepared: P`, `prepared_prime: P'`, `cn: u32`, `hn: u32` | Vote to prepare ballot |
| **Commit** | `ballot: B`, `cn: u32`, `hn: u32` | Vote to commit ballot |
| **Externalize** | `commit: C`, `height: u32` | Finalize value |

**Code Reference:** `consensus/scp/src/msg.rs`

### State Machine

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           SLOT STATE MACHINE                                 │
│                                                                              │
│  ┌────────────┐    ┌────────────┐    ┌────────────┐    ┌────────────┐      │
│  │            │    │            │    │            │    │            │      │
│  │   IDLE     │───▶│ NOMINATING │───▶│ PREPARING  │───▶│ COMMITTING │      │
│  │            │    │            │    │            │    │            │      │
│  └────────────┘    └────────────┘    └─────┬──────┘    └─────┬──────┘      │
│                                            │                  │             │
│                                            │  (abort)         │             │
│                                            ▼                  ▼             │
│                                     ┌────────────┐    ┌────────────┐       │
│                                     │            │    │            │       │
│                                     │ BUMP BALLOT│    │EXTERNALIZED│       │
│                                     │            │    │  (FINAL)   │       │
│                                     └────────────┘    └────────────┘       │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

**Code Reference:** `consensus/scp/src/slot.rs`

---

## Quorum Configuration

### Quorum Set Structure

```rust
struct QuorumSet {
    threshold: u32,           // Minimum nodes required
    members: Vec<NodeId>,     // Direct members
    inner_sets: Vec<QuorumSet>, // Nested quorum sets
}
```

### Configuration Modes

| Mode | Description | Use Case |
|------|-------------|----------|
| **Recommended** | Auto-trust discovered peers, threshold = ceil(2n/3) | Default for new networks |
| **Explicit** | User specifies trusted peer IDs and threshold | Production networks |

### Quorum Intersection

For safety, all quorum slices must have non-empty intersection:

```
∀ Q1, Q2 ∈ quorum_slices: Q1 ∩ Q2 ≠ ∅
```

This is verified at runtime when quorum configuration changes.

**Code Reference:** `consensus/scp/src/quorum_set_ext.rs`

---

## Block Production

### Overview

Botho separates proof-of-work from block selection:

1. Minters find valid PoW nonces
2. Multiple valid solutions may exist
3. SCP quorum decides which block wins
4. Winner's block is built and broadcast

### PoW Formula

```
SHA256(nonce || prev_block_hash || minter_view_key || minter_spend_key) < difficulty_target
```

### Block Building Flow

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           BLOCK PRODUCTION                                   │
│                                                                              │
│  ┌───────────────────────────────────────────────────────────────────────┐  │
│  │                           MINTER                                      │  │
│  │                                                                       │  │
│  │  1. Get current chain tip from ledger                                 │  │
│  │  2. Construct candidate MintingTx with nonce search                   │  │
│  │  3. Multi-threaded PoW search until valid nonce found                 │  │
│  │  4. Submit MintingTx to consensus service                             │  │
│  │                                                                       │  │
│  └──────────────────────────────────────┬────────────────────────────────┘  │
│                                         │                                    │
│                                         ▼                                    │
│  ┌───────────────────────────────────────────────────────────────────────┐  │
│  │                      CONSENSUS SERVICE                                │  │
│  │                                                                       │  │
│  │  1. Validate MintingTx (PoW check, key validity)                     │  │
│  │  2. Propose MintingTx to SCP slot                                    │  │
│  │  3. Wait for quorum agreement (Nominate → Prepare → Commit)          │  │
│  │  4. On Externalize: Build block from winning value                   │  │
│  │                                                                       │  │
│  └──────────────────────────────────────┬────────────────────────────────┘  │
│                                         │                                    │
│                                         ▼                                    │
│  ┌───────────────────────────────────────────────────────────────────────┐  │
│  │                       BLOCK BUILDER                                   │  │
│  │                                                                       │  │
│  │  1. Extract winning MintingTx from externalized value                │  │
│  │  2. Select transactions from mempool (by fee priority)               │  │
│  │  3. Construct block header with merkle root                          │  │
│  │  4. Compute block hash                                               │  │
│  │  5. Return BuiltBlock for storage and broadcast                      │  │
│  │                                                                       │  │
│  └───────────────────────────────────────────────────────────────────────┘  │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

**Code Reference:** `botho/src/consensus/block_builder.rs`

---

## Block Validation

### Block Header

```rust
struct BlockHeader {
    version: u32,
    previous_hash: Hash,
    merkle_root: Hash,
    timestamp: u64,
    height: u64,
    difficulty: u64,
    nonce: u64,
    minter_view_key: PublicKey,
    minter_spend_key: PublicKey,
}
```

### Validation Rules

| Check | Rule | Location |
|-------|------|----------|
| PoW | `hash(header) < difficulty_target` | `validation.rs` |
| Height | `height == parent.height + 1` | `validation.rs` |
| Previous | `previous_hash == parent.hash` | `validation.rs` |
| Timestamp | `timestamp <= now + 2 hours` | `validation.rs` |
| Merkle | `merkle_root == compute_merkle(transactions)` | `validation.rs` |
| Difficulty | Within adjustment bounds | `validation.rs` |

### Genesis Block

Genesis blocks are identified by magic bytes:

```rust
const MAINNET_GENESIS_MAGIC: &[u8] = b"BOTHO_MAINNET_GENESIS_V1";
const TESTNET_GENESIS_MAGIC: &[u8] = b"BOTHO_TESTNET_GENESIS_V1";
```

**Code Reference:** `botho/src/consensus/validation.rs`

---

## Byzantine Fault Tolerance

### Guarantees

| Scenario | Behavior |
|----------|----------|
| f < n/3 Byzantine nodes | Consensus proceeds normally |
| f = n/3 Byzantine nodes | Safety preserved, liveness may stall |
| f > n/3 Byzantine nodes | No guarantees (network compromised) |

### Attack Resistance

| Attack | Mitigation |
|--------|------------|
| **Equivocation** | Message signatures tie statements to sender |
| **Value Injection** | Only valid MintingTx can be proposed |
| **Stalling** | Ballot bumping allows progress |
| **Replay** | Slot numbers prevent message replay |

### Message Validation

All SCP messages are validated before processing:

```rust
fn validate(&self) -> Result<(), ValidationError> {
    // Check ballot values are sorted
    self.ballot.is_values_sorted()?;

    // Check signature
    self.verify_signature()?;

    // Check slot is current or adjacent
    self.validate_slot_range()?;

    Ok(())
}
```

---

## Difficulty Adjustment

### Transaction-Based Adjustment

Botho adjusts difficulty based on transaction throughput:

```
Every 1000 transactions:
  actual_time = time_since_last_adjustment
  expected_time = 1000 * target_block_time / expected_tx_per_block
  adjustment = clamp(expected_time / actual_time, 0.75, 1.25)
  new_difficulty = old_difficulty * adjustment
```

### Dynamic Block Timing

| Transaction Rate | Block Time |
|------------------|------------|
| 20+ tx/s | 3 seconds |
| 5+ tx/s | 5 seconds |
| 1+ tx/s | 10 seconds |
| 0.2+ tx/s | 20 seconds |
| < 0.2 tx/s | 40 seconds |

**Code Reference:** `botho/src/block.rs:dynamic_timing`

---

## Security Audit Checklist

### SCP Implementation
- [ ] Ballot ordering is consistent across all nodes
- [ ] Value ordering in ballots is enforced
- [ ] Quorum intersection is verified
- [ ] Message replay protection works

### Block Validation
- [ ] PoW verification is correct
- [ ] Height sequence is monotonic
- [ ] Timestamp bounds are enforced
- [ ] Difficulty adjustment is bounded

### Block Building
- [ ] Only valid MintingTx can produce blocks
- [ ] Transaction selection respects fee priority
- [ ] Merkle root computation is correct

### Concurrency
- [ ] State locks prevent data races
- [ ] Lock poisoning is handled
- [ ] No deadlock potential in critical paths
