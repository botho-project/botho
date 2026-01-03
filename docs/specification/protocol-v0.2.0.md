# Botho Protocol Specification

**Version**: 0.2.0
**Status**: Draft
**Last Updated**: 2025-01-03

## Abstract

Botho is a privacy-preserving cryptocurrency protocol that combines Ring Confidential Transactions (RingCT) with hybrid post-quantum cryptography. This specification defines the complete protocol including transaction formats, consensus mechanism, network protocol, and cryptographic primitives.

## Table of Contents

1. [Introduction](#1-introduction)
2. [Notation and Conventions](#2-notation-and-conventions)
3. [Transaction Format](#3-transaction-format)
4. [Consensus Protocol (SCP)](#4-consensus-protocol-scp)
5. [Network Protocol](#5-network-protocol)
6. [Cryptographic Primitives](#6-cryptographic-primitives)
7. [Block Structure](#7-block-structure)
8. [Monetary System](#8-monetary-system)
9. [Network Configuration](#9-network-configuration)
10. [Security Considerations](#10-security-considerations)
11. [References](#11-references)
12. [Changelog](#12-changelog)

---

## 1. Introduction

### 1.1 Purpose

This document provides a formal specification of the Botho protocol, enabling:

- Third-party implementations and wallet integrations
- Security audits with complete protocol documentation
- Academic review of cryptographic primitives
- Developer onboarding and protocol understanding
- Regulatory compliance documentation

### 1.2 Scope

This specification covers:

- Wire formats for all protocol messages
- Cryptographic algorithms and parameters
- Consensus rules and state transitions
- Network topology and peer discovery
- Economic parameters and fee structures

### 1.3 Design Goals

1. **Privacy**: Sender, receiver, and amount privacy through ring signatures and commitments
2. **Quantum Resistance**: Hybrid security model with post-quantum stealth addresses
3. **Scalability**: Compact block relay and efficient validation
4. **Decentralization**: Byzantine fault-tolerant consensus via SCP

### 1.4 Transaction Types

Botho supports two transaction types:

| Type | Purpose | Signature | Stealth Address |
|------|---------|-----------|-----------------|
| **Minting** | Block rewards | ML-DSA-65 | ML-KEM-768 |
| **Private** | User transfers | CLSAG ring signature | ML-KEM-768 |

Both transaction types use ML-KEM-768 for post-quantum secure stealth addresses, protecting recipient privacy against future quantum attacks.

---

## 2. Notation and Conventions

### 2.1 Cryptographic Notation

| Symbol | Description |
|--------|-------------|
| $G$ | Ristretto255 basepoint |
| $H$ | Secondary generator for Pedersen commitments |
| $H_s(\cdot)$ | Hash-to-scalar function (SHA-512 with domain separation) |
| $\|$ | Byte concatenation |
| $[n]$ | Set of integers $\{0, 1, ..., n-1\}$ |
| $\mathbb{Z}_q$ | Scalar field of Ristretto255 ($q = 2^{252} + ...$) |

### 2.2 Data Types

```
u8      : unsigned 8-bit integer
u32     : unsigned 32-bit integer (little-endian)
u64     : unsigned 64-bit integer (little-endian)
[u8; N] : fixed-size byte array of length N
Vec<T>  : variable-length vector of type T
```

### 2.3 Encoding

All multi-byte integers are encoded in **little-endian** format unless otherwise specified. Structures are serialized using Protocol Buffers (prost) encoding.

---

## 3. Transaction Format

Botho supports two transaction types with distinct purposes:

| Type | Ring Signature | Ring Size | Max Inputs | Max Outputs | Max Size |
|------|---------------|-----------|------------|-------------|----------|
| Minting | None (ML-DSA) | N/A | 0 | 1 | 10 KB |
| Private | CLSAG | 20 | 16 | 16 | 100 KB |

### 3.1 Minting Transaction

Minting transactions create new coins as block rewards. They have no inputs and exactly one output.

#### 3.1.1 Minting Transaction Structure

```rust
struct MintingTx {
    block_height: u64,
    reward: u64,                 // in picocredits
    minter_view_key: [u8; 32],
    minter_spend_key: [u8; 32],
    target_key: [u8; 32],        // Stealth output
    public_key: [u8; 32],        // Ephemeral key for stealth
    ml_kem_ciphertext: [u8; 1088], // Post-quantum key encapsulation
    prev_block_hash: [u8; 32],
    difficulty: u64,
    nonce: u64,
    timestamp: u64,
    signature: MlDsaSignature,   // ML-DSA-65 signature
}
```

#### 3.1.2 Minting Validation

1. **Height**: `block_height == current_chain_height + 1`
2. **Reward**: Matches expected reward for height (see Section 8.2)
3. **PoW**: Valid proof-of-work (see Section 7.2)
4. **Signature**: Valid ML-DSA-65 signature over transaction data

### 3.2 Private Transaction

Private transactions transfer funds between users with full privacy (hidden sender, recipient, and amount).

#### 3.2.1 Transaction Structure

```rust
struct Transaction {
    prefix: TxPrefix,
    signature: ClsagSignature,  // Ring signature
}

struct TxPrefix {
    inputs: Vec<TxInput>,      // 1-16 inputs
    outputs: Vec<TxOutput>,    // 1-16 outputs
    fee: u64,                  // in picocredits
    tombstone_block: u64,      // expiry height
}
```

#### 3.2.2 Transaction Input

```rust
struct TxInput {
    ring: Vec<TxOutMembershipElement>,  // exactly 20 members
    pseudo_output_commitment: CompressedCommitment,
    key_image: KeyImage,                 // [u8; 32]
}
```

**Wire Format** (example, 1 input):

```
Offset  Size    Field
0x00    4       ring_size (u32) = 20
0x04    20*96   ring_members (20 * TxOutMembershipElement)
0x784   32      pseudo_output_commitment
0x7A4   32      key_image
```

#### 3.2.3 Transaction Output

```rust
struct TxOutput {
    amount: MaskedAmount,              // encrypted value
    target_key: CompressedRistretto,   // [u8; 32]
    public_key: CompressedRistretto,   // [u8; 32]
    ml_kem_ciphertext: [u8; 1088],     // Post-quantum stealth
    e_memo: Option<EncryptedMemo>,     // encrypted memo
}
```

#### 3.2.4 Stealth Address Derivation (Hybrid)

Botho uses a hybrid classical/post-quantum stealth address scheme:

**Recipient Setup**:
1. Generate classical keypairs: view $(a, A = a \cdot G)$, spend $(b, B = b \cdot G)$
2. Generate ML-KEM keypair: $(pk_{kem}, sk_{kem})$
3. Publish address containing $A$, $B$, and $pk_{kem}$

**Sender Creates Output**:
1. Generate random scalar $r \in \mathbb{Z}_q$
2. Encapsulate PQ shared secret: $(ct, ss_{pq}) = \text{ML-KEM.Encaps}(pk_{kem})$
3. Compute classical shared secret: $ss_c = r \cdot A$
4. Combine secrets: $ss = H(ss_c \| ss_{pq})$
5. Compute target key: $P = H_s(ss) \cdot G + B$
6. Compute public key: $R = r \cdot G$
7. Include $ct$ (1,088 bytes) in output

**Recipient Scans**:
1. Decapsulate: $ss_{pq} = \text{ML-KEM.Decaps}(ct, sk_{kem})$
2. Compute: $ss_c = a \cdot R$
3. Combine: $ss = H(ss_c \| ss_{pq})$
4. Compute candidate: $P' = H_s(ss) \cdot G + B$
5. If $P' = P$, output belongs to recipient

**Spending Key**:
$$x = H_s(ss) + b$$

#### 3.2.5 Masked Amount

```rust
struct MaskedAmountV2 {
    commitment: CompressedCommitment,  // [u8; 32]
    masked_value: u64,                  // XOR-encrypted
    masked_token_id: [u8; 8],           // XOR-encrypted
}
```

**Commitment**: $C = v \cdot H + b \cdot G$ where $v$ is value and $b$ is blinding factor.

**Masking**:
1. Derive mask: $\text{mask} = H_s(\text{"mc\_amount\_value"} \| ss)$
2. Masked value: $\text{masked\_value} = v \oplus \text{mask}[0..8]$

### 3.3 Transaction Validation

#### 3.3.1 Structural Validation

1. Input count: $1 \leq |\text{inputs}| \leq 16$
2. Output count: $1 \leq |\text{outputs}| \leq 16$
3. Ring size: exactly 20
4. Transaction size: $\leq 100$ KB
5. Tombstone: $\text{current\_height} < \text{tombstone\_block} \leq \text{current\_height} + 20160$

#### 3.3.2 Cryptographic Validation

1. **Key Images**: No duplicates in blockchain history
2. **Ring Signature**: Valid CLSAG signature
3. **Balance Proof**: $\sum \text{pseudo\_outputs} = \sum \text{outputs} + \text{fee} \cdot H$
4. **Range Proofs**: Valid Bulletproofs for all outputs

#### 3.3.3 Constants

| Parameter | Value | Notes |
|-----------|-------|-------|
| `RING_SIZE` | 20 | Decoy ring members |
| `MAX_INPUTS` | 16 | Per transaction |
| `MAX_OUTPUTS` | 16 | Per transaction |
| `MAX_TX_SIZE` | 100 KB | DoS protection |
| `MIN_TX_FEE` | 100,000,000 | 0.0001 BTH in picocredits |
| `MAX_TOMBSTONE_BLOCKS` | 20,160 | ~7 days at 30s blocks |

---

## 4. Consensus Protocol (SCP)

Botho uses the Stellar Consensus Protocol (SCP) for Byzantine fault-tolerant agreement.

### 4.1 Overview

SCP is a federated Byzantine agreement (FBA) protocol where nodes can choose their own quorum slices. It provides:

- Safety: Agreement on a single value
- Liveness: Progress under network partitions
- Decentralization: No central authority

### 4.2 Message Types

#### 4.2.1 Nominate

Propose candidate values for consensus:

```rust
struct NominatePayload<V> {
    X: BTreeSet<V>,  // Voted values
    Y: BTreeSet<V>,  // Accepted values
}
```

**Wire Format**:
```
[4 bytes] X_count (u32)
[X_count * sizeof(V)] X values
[4 bytes] Y_count (u32)
[Y_count * sizeof(V)] Y values
```

#### 4.2.2 Prepare

Vote on candidate ballots:

```rust
struct PreparePayload<V> {
    B: Ballot<V>,        // Current ballot
    P: Option<Ballot<V>>, // Highest accepted prepared
    PP: Option<Ballot<V>>, // Prepared prime (different value)
    CN: u32,              // Lowest ballot attempting to confirm
    HN: u32,              // Highest ballot with quorum preparation
}

struct Ballot<V> {
    counter: u32,         // INFINITY = u32::MAX
    value: V,
}
```

#### 4.2.3 Commit

Lock a value after quorum agreement:

```rust
struct CommitPayload<V> {
    B: Ballot<V>,         // Commit ballot
    PN: u32,              // Prepared counter
    CN: u32,              // Commit counter
    HN: u32,              // Highest counter seen
}
```

#### 4.2.4 Externalize

Announce final consensus:

```rust
struct ExternalizePayload<V> {
    C: Ballot<V>,         // Committed ballot
    HN: u32,              // Highest counter
}
```

### 4.3 Slot State Machine

Each consensus slot progresses through phases:

```
NOMINATING → PREPARING → CONFIRMING → EXTERNALIZED
     ↑           ↓
     ←──────────←─ (ballot failure)
```

#### 4.3.1 Phase Transitions

1. **NOMINATING**: Collect and vote on candidate values
2. **PREPARING**: Vote on ballots until one is prepared
3. **CONFIRMING**: Confirm prepared ballot until committed
4. **EXTERNALIZED**: Slot finalized, value immutable

### 4.4 Quorum Configuration

```rust
struct QuorumSet {
    threshold: u32,
    validators: Vec<NodeId>,
    inner_sets: Vec<QuorumSet>,
}
```

A quorum slice is satisfied when:
- At least `threshold` validators agree, AND
- At least `threshold` inner sets are satisfied

### 4.5 Slot Numbering

- `SlotIndex`: u64 representing block height
- Slots are processed sequentially
- Each slot produces exactly one block

---

## 5. Network Protocol

### 5.1 Transport Layer

Built on libp2p with:
- **Transport**: TCP with Noise encryption
- **Multiplexing**: Yamux
- **Discovery**: mDNS + Kademlia DHT

### 5.2 Gossipsub Topics

```rust
const BLOCKS_TOPIC: &str = "botho/blocks/1.0.0";
const TRANSACTIONS_TOPIC: &str = "botho/transactions/1.0.0";
const SCP_TOPIC: &str = "botho/scp/1.0.0";
const COMPACT_BLOCKS_TOPIC: &str = "botho/compact-blocks/1.0.0";
```

**Message Propagation**:
- Blocks: Full block on minting, compact otherwise
- Transactions: Full transaction on submission
- SCP: Consensus messages for current/next slot

### 5.3 Sync Protocol

Protocol ID: `/botho/sync/1.0.0`

#### 5.3.1 Request Messages

```rust
enum SyncRequest {
    GetStatus,
    GetBlocks { start_height: u64, count: u32 },
}
```

#### 5.3.2 Response Messages

```rust
enum SyncResponse {
    Status { height: u64, tip_hash: [u8; 32] },
    Blocks { blocks: Vec<Block>, has_more: bool },
    Error(String),
}
```

**Wire Format** (GetBlocks):
```
Offset  Size   Field
0x00    1      message_type = 0x01
0x01    8      start_height (u64 LE)
0x09    4      count (u32 LE)
```

### 5.4 Compact Block Relay

Efficient block propagation using short transaction IDs:

```rust
struct CompactBlock {
    header: BlockHeader,
    short_ids: Vec<ShortId>,      // 6-byte tx identifiers
    prefilled_txs: Vec<PrefilledTx>,
}

struct ShortId([u8; 6]);  // Truncated SipHash
```

**Protocol**:
1. Miner broadcasts `CompactBlock`
2. Peers identify missing transactions
3. Peers request via `GetBlockTxn`
4. Miner responds with `BlockTxn`

### 5.5 DDoS Protection

| Parameter | Value | Purpose |
|-----------|-------|---------|
| `MAX_REQUEST_SIZE` | 1 KB | Prevent memory exhaustion |
| `MAX_RESPONSE_SIZE` | 10 MB | ~100 blocks |
| `MAX_REQUESTS_PER_MINUTE` | 60 | Per-peer rate limiting |
| `BLOCKS_PER_REQUEST` | 100 | Batch size limit |
| `REQUEST_TIMEOUT` | 30s | Connection cleanup |
| `SYNC_BEHIND_THRESHOLD` | 10 | Blocks before sync mode |

### 5.6 Network Events

```rust
enum NetworkEvent {
    NewBlock(Block),
    NewTransaction(Transaction),
    ScpMessage(ScpMessage),
    NewCompactBlock(CompactBlock),
    GetBlockTxn { peer: PeerId, request: GetBlockTxnRequest },
    BlockTxn { txs: Vec<Transaction> },
    PeerDiscovered(PeerId),
    PeerDisconnected(PeerId),
    SyncRequest { peer: PeerId, request: SyncRequest },
    SyncResponse { peer: PeerId, response: SyncResponse },
}
```

---

## 6. Cryptographic Primitives

### 6.1 CLSAG Ring Signatures

Concise Linkable Spontaneous Anonymous Group signatures provide sender privacy.

#### 6.1.1 Parameters

- Curve: Ristretto255
- Ring size: 20
- Signature size: ~700 bytes per input

#### 6.1.2 Signature Structure

```rust
struct ClsagSignature {
    c_zero: CurveScalar,              // [u8; 32]
    responses: Vec<CurveScalar>,      // 20 * [u8; 32]
    key_image: KeyImage,              // [u8; 32]
    commitment_key_image: KeyImage,   // [u8; 32]
}
```

#### 6.1.3 Signing Algorithm

**Inputs**:
- Message $m$
- Ring of public keys $\{P_0, ..., P_{n-1}\}$
- Secret index $\pi$ and private key $x$ where $P_\pi = x \cdot G$
- Pseudo-output commitment and blinding

**Output**: $(c_0, \{r_0, ..., r_{n-1}\}, I)$

**Algorithm**:

1. Compute key image: $I = x \cdot H_p(P_\pi)$
2. Compute aggregation coefficients:
   - $\mu_P = H_s(\text{agg\_P} \| \{P_i\} \| I)$
   - $\mu_C = H_s(\text{agg\_C} \| \{P_i\} \| I)$
3. Generate random $\alpha \in \mathbb{Z}_q$
4. Compute $L_\pi = \alpha \cdot G$, $R_\pi = \alpha \cdot H_p(P_\pi)$
5. For $i = \pi + 1, ..., \pi - 1 \mod n$:
   - Generate random $r_i$
   - Compute $c_i$, $L_i$, $R_i$
6. Set $c_0 = c_{\pi+1 \mod n}$
7. Compute $r_\pi = \alpha - c_\pi \cdot (\mu_P \cdot x + \mu_C \cdot z)$

#### 6.1.4 Verification Algorithm

1. Recompute aggregation coefficients
2. For $i = 0, ..., n-1$:
   - $L_i = r_i \cdot G + c_i \cdot (\mu_P \cdot P_i + \mu_C \cdot C_i)$
   - $R_i = r_i \cdot H_p(P_i) + c_i \cdot (\mu_P \cdot I + \mu_C \cdot D)$
   - $c_{i+1} = H_s(\text{round} \| \{P\} \| m \| L_i \| R_i)$
3. Accept iff $c_0 = c_n$

#### 6.1.5 Domain Separators

```rust
const CLSAG_ROUND_HASH_DOMAIN_TAG: &[u8] = b"CLSAG_round";
const CLSAG_AGG_COEFF_P_DOMAIN_TAG: &[u8] = b"CLSAG_agg_P";
const CLSAG_AGG_COEFF_C_DOMAIN_TAG: &[u8] = b"CLSAG_agg_C";
```

### 6.2 ML-KEM-768 (Kyber)

Key encapsulation mechanism for post-quantum stealth addresses.

#### 6.2.1 Parameters (NIST Level 3)

| Parameter | Value |
|-----------|-------|
| Public Key | 1,184 bytes |
| Secret Key | 2,400 bytes |
| Ciphertext | 1,088 bytes |
| Shared Secret | 32 bytes |

#### 6.2.2 Usage in Botho

ML-KEM-768 provides post-quantum security for recipient privacy:

1. Recipient generates ML-KEM keypair alongside classical keys
2. Sender encapsulates shared secret using recipient's ML-KEM public key
3. Shared secret combined with classical ECDH for hybrid security
4. Ciphertext (1,088 bytes) included in transaction output

This protects recipient addresses against future quantum attacks. Transaction data is recorded permanently on-chain, so recipient privacy must be quantum-resistant from day one.

### 6.3 ML-DSA-65 (Dilithium)

Digital signatures for minting transaction authorization.

#### 6.3.1 Parameters (NIST Level 3)

| Parameter | Value |
|-----------|-------|
| Public Key | 1,952 bytes |
| Secret Key | 4,032 bytes |
| Signature | 3,309 bytes |

#### 6.3.2 Usage in Botho

ML-DSA-65 is used exclusively for minting transactions:

1. Minter generates ML-DSA keypair
2. Minting transaction signed with ML-DSA private key
3. Nodes verify signature against minter's public key in block header

### 6.4 Pedersen Commitments

Amount hiding with homomorphic properties.

#### 6.4.1 Commitment Scheme

$$C = v \cdot H + b \cdot G$$

where:
- $v \in [0, 2^{64})$ is the value
- $b \in \mathbb{Z}_q$ is the blinding factor
- $G, H$ are independent generators

#### 6.4.2 Properties

1. **Hiding**: Given $C$, cannot determine $v$ or $b$
2. **Binding**: Cannot find $(v', b') \neq (v, b)$ with same commitment
3. **Homomorphic**: $C_1 + C_2 = (v_1 + v_2) \cdot H + (b_1 + b_2) \cdot G$

### 6.5 Bulletproofs Range Proofs

Prove that committed values are in valid range without revealing them.

#### 6.5.1 Purpose

Prove $v \in [0, 2^{64})$ for each output commitment.

#### 6.5.2 Properties

- Proof size: $O(\log n)$ for $n$ range bits
- Aggregated: Single proof for multiple outputs
- Zero-knowledge: Reveals nothing about values

### 6.6 Domain Separators

All hash functions use domain separation to prevent cross-protocol attacks:

```rust
const AMOUNT_VALUE_DOMAIN_TAG: &[u8] = b"mc_amount_value";
const AMOUNT_TOKEN_ID_DOMAIN_TAG: &[u8] = b"mc_amount_token_id";
const AMOUNT_BLINDING_DOMAIN_TAG: &[u8] = b"mc_amount_blinding";
const BULLETPROOF_DOMAIN_TAG: &[u8] = b"mc_bulletproof_transcript";
const TXOUT_MERKLE_LEAF_DOMAIN_TAG: &[u8] = b"mc_tx_out_merkle_leaf";
const TXOUT_MERKLE_NODE_DOMAIN_TAG: &[u8] = b"mc_tx_out_merkle_node";
const EXTENDED_MESSAGE_DOMAIN_TAG: &[u8] = b"mc_extended_message";
```

---

## 7. Block Structure

### 7.1 Block Header

```rust
struct BlockHeader {
    version: u32,
    prev_block_hash: [u8; 32],
    tx_root: [u8; 32],          // Merkle root of transactions
    timestamp: u64,              // Unix seconds
    height: u64,
    difficulty: u64,
    nonce: u64,
    minter_view_key: [u8; 32],
    minter_spend_key: [u8; 32],
}
```

**Wire Format**:
```
Offset  Size   Field
0x00    4      version
0x04    32     prev_block_hash
0x24    32     tx_root
0x44    8      timestamp
0x4C    8      height
0x54    8      difficulty
0x5C    8      nonce
0x64    32     minter_view_key
0x84    32     minter_spend_key
-----
Total: 164 bytes
```

### 7.2 Proof of Work

#### 7.2.1 Hash Computation

```
pow_hash = SHA256(nonce || prev_block_hash || minter_view_key || minter_spend_key)
```

#### 7.2.2 Validity Condition

```
u64::from_be_bytes(pow_hash[0..8]) < difficulty
```

### 7.3 Genesis Block

#### 7.3.1 Magic Bytes

| Network | Magic | Hex |
|---------|-------|-----|
| Mainnet | `BOTHO_MAINNET_GENESIS_V1` | 0x4254484F_4D414954... |
| Testnet | `BOTHO_TESTNET_GENESIS_V1` | 0x4254484F_54455354... |

#### 7.3.2 Genesis Configuration

```
prev_block_hash = SHA256(magic_bytes)
height = 0
difficulty = INITIAL_DIFFICULTY
```

### 7.4 Block Body

```rust
struct Block {
    header: BlockHeader,
    minting_tx: MintingTx,
    transfer_txs: Vec<Transaction>,
}
```

### 7.5 Block Limits

| Parameter | Value | Notes |
|-----------|-------|-------|
| `MAX_TXS_PER_BLOCK` | 5,000 | Excluding minting tx |
| `MAX_BLOCK_SIZE` | 20 MB | Total serialized size |

---

## 8. Monetary System

### 8.1 Base Units

| Unit | Picocredits | Notation |
|------|-------------|----------|
| 1 BTH | 10^12 | BTH |
| 1 milliBTH | 10^9 | mBTH |
| 1 microBTH | 10^6 | uBTH |
| 1 nanoBTH | 10^3 | nBTH |
| 1 picocredit | 1 | pico |

The **picocredit** is the atomic unit used in all protocol calculations.

### 8.2 Supply Schedule

#### 8.2.1 Phase 1 Distribution

Total Phase 1 supply: **100,000,000 BTH**

Emission follows a halving schedule over approximately 10 years.

#### 8.2.2 Block Reward

Block rewards decrease according to:

```
reward(height) = base_reward / 2^(height / halving_interval)
```

### 8.3 Fee Structure

#### 8.3.1 Minimum Fee

```
MIN_TX_FEE = 100,000,000 picocredits = 0.0001 BTH
```

#### 8.3.2 Fee Calculation

Fees are proportional to transaction size:

```
fee = max(MIN_TX_FEE, size_in_bytes * fee_per_byte)
```

#### 8.3.3 Fee Blinding

```rust
const FEE_BLINDING: Scalar = Scalar::ZERO;
```

Fees are public (unblinded) to enable fee validation without range proofs.

### 8.4 Progressive Fees (Cluster Tax)

Botho implements provenance-based taxation to discourage wealth concentration:

1. Outputs tagged with cryptographic cluster identifiers
2. Fees increase for outputs with high cluster concentration
3. Sybil-resistant: Cannot reduce fees by splitting across addresses

---

## 9. Network Configuration

### 9.1 Network Parameters

| Network | Address Prefix | Gossip Port | RPC Port | Magic |
|---------|---------------|-------------|----------|-------|
| Mainnet | `botho://1/` | 7100 | 7101 | 0x4254484D |
| Testnet | `tbotho://1/` | 17100 | 17101 | 0x42544854 |

### 9.2 Address Format

```
botho://1/<base58check-encoded-keys>
```

Components:
- View public key (32 bytes)
- Spend public key (32 bytes)
- ML-KEM public key (1,184 bytes)

All addresses include ML-KEM public keys for post-quantum stealth address derivation.

### 9.3 Block Timing

| Parameter | Value |
|-----------|-------|
| Target Block Time | 30 seconds |
| Difficulty Adjustment | Every block |
| Epoch Length | 20,160 blocks (~7 days) |

---

## 10. Security Considerations

### 10.1 Threat Model

#### 10.1.1 Assumptions

1. Discrete logarithm problem is hard (classical security)
2. Module-LWE problem is hard (post-quantum security)
3. SHA-256 and SHA-512 are collision-resistant
4. Network adversary cannot control > 1/3 of consensus nodes

#### 10.1.2 Protected Against

- Transaction linkability (ring signatures)
- Amount disclosure (Pedersen commitments)
- Double-spending (key images)
- Quantum attacks on recipient privacy (ML-KEM stealth addresses)

### 10.2 Ring Signature Security

#### 10.2.1 Anonymity Set

With ring size 20, probability of identifying true signer is 5% per input.

For $n$ inputs: $P(\text{identify all}) = (1/20)^n$

#### 10.2.2 Decoy Selection

Decoys selected using gamma distribution weighted by:
- Recency (newer outputs preferred)
- Age uniformity (avoid timing analysis)

### 10.3 Key Image Security

Key images provide:
- **Uniqueness**: Each output can only be spent once
- **Unlinkability**: Key image reveals nothing about source output

### 10.4 Hybrid Post-Quantum Security

#### 10.4.1 Security Model

Botho uses a hybrid classical/post-quantum security model:

| Component | Classical | Post-Quantum | Rationale |
|-----------|-----------|--------------|-----------|
| **Recipient privacy** | ECDH | ML-KEM-768 | On-chain forever, must be PQ-safe |
| **Sender privacy** | CLSAG | — | Ephemeral value, classical sufficient |
| **Amount hiding** | Pedersen | — | Information-theoretic hiding |
| **Minting auth** | — | ML-DSA-65 | Block rewards need PQ signatures |

#### 10.4.2 Rationale

**Why hybrid stealth addresses?**

Transaction data is recorded permanently on-chain. A quantum attacker in 2045 could retroactively link recipients from 2025 transactions if we used only classical ECDH. By using ML-KEM alongside ECDH, recipient privacy is protected against future quantum attacks.

**Why classical ring signatures?**

Sender anonymity is ephemeral—its value degrades over time as economic context becomes historical. Post-quantum ring signatures (like LION) are ~50x larger than CLSAG, making blockchain growth unsustainable for desktop nodes. The tradeoff favors compact classical signatures for sender privacy.

See [ADR-0001](../decisions/0001-deprecate-lion-ring-signatures.md) for detailed analysis.

#### 10.4.3 Security Properties

Both classical AND post-quantum components must be broken to compromise recipient privacy:

```
recipient_compromised = break_ecdh(output) AND break_mlkem(output)
```

This provides defense-in-depth against implementation bugs in either scheme.

### 10.5 Denial of Service

#### 10.5.1 Transaction Size Limits

- Maximum: 100 KB
- Prevents memory exhaustion

#### 10.5.2 Rate Limiting

- 60 requests/minute per peer
- Protects against sync flooding

#### 10.5.3 Validation Ordering

1. Structural validation (cheap)
2. Key image check (cheap)
3. Signature verification (expensive)

---

## 11. References

1. **CLSAG**: "Tighter Security Proofs for Money-Grubbing Ring Signatures" - MRL-0011
2. **Bulletproofs**: Bunz et al., "Bulletproofs: Short Proofs for Confidential Transactions"
3. **SCP**: Mazieres, "The Stellar Consensus Protocol"
4. **ML-KEM**: NIST FIPS 203, "Module-Lattice-Based Key-Encapsulation Mechanism"
5. **ML-DSA**: NIST FIPS 204, "Module-Lattice-Based Digital Signature Algorithm"
6. **Ristretto**: "Ristretto: A Technique for Constructing Elliptic Curve Groups"

---

## 12. Changelog

### Version 0.2.0 (2025-01-03)

- **BREAKING**: Removed LION ring signatures and PQ-Private transaction type
- Simplified to two transaction types: Minting (ML-DSA) and Private (CLSAG)
- All addresses now include ML-KEM public key for post-quantum stealth
- Updated security model documentation to reflect hybrid approach
- Clarified rationale for classical sender privacy vs PQ recipient privacy
- Removed PQ transaction size limits (512 KB max no longer applicable)
- Updated address format documentation

### Version 0.1.0 (2024-12-31)

- Initial specification draft
- Complete transaction format documentation
- SCP consensus protocol specification
- Network protocol and gossipsub topics
- Cryptographic primitives (CLSAG, LION, ML-KEM, ML-DSA)
- Block structure and PoW specification
- Monetary system and fee structure
- Security considerations

---

*This specification is maintained in the Botho repository at `docs/specification/protocol-v0.2.0.md`.*
