# Security Audit Preparation Guide

This document outlines security-critical areas requiring careful review before engaging third-party auditors. Use this as both a preparation checklist and a guide for auditors.

---

## Table of Contents

1. [Critical Priority: Cryptographic Implementations](#1-critical-priority-cryptographic-implementations)
2. [Critical Priority: Key Derivation & Management](#2-critical-priority-key-derivation--management)
3. [Critical Priority: Consensus Protocol](#3-critical-priority-consensus-protocol)
4. [High Priority: Transaction Validation](#4-high-priority-transaction-validation)
5. [High Priority: Privacy Analysis](#5-high-priority-privacy-analysis)
6. [High Priority: Network Security](#6-high-priority-network-security)
7. [Medium Priority: Wallet Security](#7-medium-priority-wallet-security)
8. [Medium Priority: Unsafe Rust Code](#8-medium-priority-unsafe-rust-code)
9. [Medium Priority: Dependencies](#9-medium-priority-dependencies)
10. [Known Issues & TODOs](#10-known-issues--todos)
10.5. [Ring Structure & Minting Dynamics](#105-ring-structure--minting-dynamics-updated-2025-12)
11. [Security Claims to Verify](#11-security-claims-to-verify)
12. [Pre-Audit Checklist](#12-pre-audit-checklist)
13. [Internal Audit Session - 2025-12-30](#13-internal-audit-session---2025-12-30)

---

## 1. Critical Priority: Cryptographic Implementations

### 1.1 MLSAG Ring Signatures

**Location:** `crypto/ring-signature/src/ring_signature/mlsag*.rs`

**Review Focus:**
- [ ] Challenge computation loop closure correctness
- [ ] Value conservation check: `sum(inputs) == sum(outputs)`
- [ ] Key image computation: `I = x * Hp(P)`
- [ ] Ring size validation (reject `real_index >= ring_size`)
- [ ] Modified MLSAG omitting key image term for amount commitments
- [ ] Scalar arithmetic edge cases (zero, overflow)
- [ ] Point decompression error handling
- [ ] Zeroization of decompressed rings and challenges

**Test Coverage:**
- [ ] Verify proptest coverage of 6+ scenarios
- [ ] Add test vectors from reference implementations
- [ ] Fuzz signing/verification roundtrips

### 1.2 CLSAG Ring Signatures

**Location:** `crypto/ring-signature/src/ring_signature/clsag.rs`

**Review Focus:**
- [ ] Aggregation coefficient derivation (`mu_P`, `mu_C`)
- [ ] Domain separation in Blake2b-512 hashing
- [ ] Auxiliary key image `D = z * Hp(P)` computation
- [ ] Commitment balance validation: `difference == z * G`
- [ ] Ring loop closure and challenge chain integrity
- [ ] Empty ring handling
- [ ] Batch verification correctness

**Critical Math:**
```
W = mu_P * P + mu_C * Z  (aggregated public key)
```

### 1.3 LION Post-Quantum Signatures

**Location:** `crypto/lion/src/`

**Review Focus:**
- [ ] Lattice basis reduction and LWE hardness assumptions
- [ ] Rejection sampling for uniform distribution
- [ ] Module-LWE/SIS parameter selection (128-bit PQ security)
- [ ] Key image linkability in lattice setting
- [ ] Signature size (~36KB per input) acceptability

**Parameters (verify security level):**
```
N = 256, Q = 8380417, K = L = 4, RING_SIZE = 11
```

**Ring Size Rationale (updated 2025-12):**
- Ring size 11 provides 3.30 bits of measured privacy (95.3% efficiency)
- Each ring member adds 3,072 bytes to signature
- Ring 11: ~36KB signature vs Ring 20: ~63.5KB (+27KB overhead)
- Ring 11 still exceeds Monero's effective anonymity (~4.2 of 16)

### 1.4 Pedersen Commitments

**Location:** `crypto/ring-signature/src/amount/`

**Review Focus:**
- [ ] Generator pair (G, H) independence
- [ ] Commitment formula: `V = v * H + r * G`
- [ ] Blinding factor uniqueness per commitment
- [ ] Point compression/decompression correctness
- [ ] Additive homomorphism verification

### 1.5 Bulletproofs Range Proofs

**Location:** `transaction/core/src/ring_ct/rct_bulletproofs.rs`

**Review Focus:**
- [ ] Range proof soundness: `0 <= amount < 2^64`
- [ ] Proof aggregation for multiple outputs
- [ ] Verification completeness
- [ ] No value overflow/underflow possible

---

## 2. Critical Priority: Key Derivation & Management

### 2.1 Account Key Hierarchy

**Location:** `account-keys/src/`

**Review Focus:**
- [ ] BIP39 mnemonic to seed (PBKDF2, "TREZOR" salt)
- [ ] SLIP-10 hardened/non-hardened derivation paths
- [ ] Domain separator consistency across all hash operations
- [ ] View key isolation from spend key
- [ ] Subaddress derivation correctness

**Key Hierarchy:**
```
RootIdentity (mnemonic) → AccountKey (SLIP-10) → SubAddress (view+spend)
                        → QuantumSafeAccountKey (classical+PQ)
```

### 2.2 One-Time Keys (Stealth Addresses)

**Location:** `crypto/ring-signature/src/onetime_keys.rs`

**Review Focus:**
- [ ] Stealth address: `P = Hs(r * C) * G + D`
- [ ] Ephemeral public key: `R = r * D`
- [ ] Private key recovery: `x = Hs(a * R) + d`
- [ ] DH shared secret symmetry
- [ ] Hash-to-scalar domain separators
- [ ] Ephemeral key uniqueness per output

### 2.3 Domain Separators

**Locations:**
- `crypto/ring-signature/src/domain_separators.rs`
- `account-keys/src/domain_separators.rs`
- `transaction/types/src/domain_separators.rs`

**Review Focus:**
- [ ] No domain separator collisions across codebase
- [ ] All hash operations use appropriate separators
- [ ] Separator format consistency

---

## 3. Critical Priority: Consensus Protocol

### 3.1 SCP Implementation

**Location:** `consensus/scp/src/`

**Review Focus:**
- [ ] Byzantine fault tolerance guarantees
- [ ] Quorum intersection properties
- [ ] Liveness under partial synchrony
- [ ] Safety: no two nodes externalize different values for same slot
- [ ] Ballot ordering constraints (multiple TODOs noted)
- [ ] Value nomination ordering (TODO: "reject incorrectly ordered values")
- [ ] Message replay protection
- [ ] Slot progression logic

**Known TODOs (must resolve before audit):**
```
slot.rs: "TODO: Reject messages with incorrectly ordered values"
slot.rs: "TODO: reject a message if it contains a ballot containing incorrectly ordered"
```

### 3.2 Block Validation

**Location:** `botho/src/consensus/validation.rs`

**Review Focus:**
- [ ] PoW hash verification: `hash < difficulty`
- [ ] Previous block hash linkage
- [ ] Block height sequence monotonicity
- [ ] Difficulty adjustment algorithm
- [ ] Block reward calculation
- [ ] Timestamp bounds (2-hour future tolerance)
- [ ] Transaction ordering within block

### 3.3 Consensus Service Integration

**Location:** `botho/src/consensus/service.rs`

**Review Focus:**
- [ ] Transaction validator callback security
- [ ] Shared state locking (Arc<RwLock<...>>)
- [ ] Lock poisoning handling
- [ ] Transaction cache coherency with mempool

---

## 4. High Priority: Transaction Validation

### 4.1 Transaction Structure

**Location:** `botho/src/transaction.rs`, `transaction/core/src/validation/`

**Review Focus:**
- [ ] Input existence verification in UTXO set
- [ ] Key image uniqueness enforcement (double-spend)
- [ ] Signature validity (MLSAG/CLSAG/LION dispatch)
- [ ] Value conservation: `sum(inputs) >= sum(outputs) + fee`
- [ ] Output amount validity (no negative, no overflow)
- [ ] Input/output canonical ordering
- [ ] Cluster tag inheritance validation

**Ring Parameters:**
```
Ring size: 20 (19 decoys + real input)
```

### 4.2 Mempool

**Location:** `botho/src/mempool.rs`

**Review Focus:**
- [ ] Double-spend detection via key image tracking
- [ ] Fee validation using cluster-tax
- [ ] Size limits (1,000 transactions)
- [ ] Age limits (3,600 seconds)
- [ ] Eviction policy (lowest fee first)
- [ ] Race condition handling for concurrent spends

**Known Issue:**
```rust
// mempool.rs:99
cluster_wealth = 0 // cluster tracking not yet implemented
```

### 4.3 Fee Calculation

**Location:** `cluster-tax/src/fee_curve.rs`

**Review Focus:**
- [ ] Progressive fee curve correctness
- [ ] Transaction type classification (Plain, Hidden, PqHidden)
- [ ] Encrypted memo count factor
- [ ] Cluster wealth impact (currently disabled)
- [ ] Minimum fee enforcement

---

## 5. High Priority: Privacy Analysis

### 5.1 Decoy Selection (OSPEAD)

**Location:** `botho/src/decoy_selection.rs`

**Review Focus:**
- [ ] Gamma distribution implementation correctness
- [ ] Age-weighted selection matches observed spending patterns
- [ ] Cluster-aware selection prevents fingerprinting
- [ ] Effective anonymity set >= 10 plausible members
- [ ] No bias toward recently created outputs
- [ ] No timing side-channels in selection

**Gamma Parameters:**
```
shape (k) = 19.28, scale (θ) = 1.61 days
Mean spending age: ~31 days
```

**Cluster Similarity:**
```
Metric: 60% cosine similarity + 40% dominant cluster matching
Threshold: MIN_CLUSTER_SIMILARITY = 0.7
```

### 5.2 Privacy Simulation

**Location:** `cluster-tax/src/simulation/privacy.rs`

**Review Focus:**
- [ ] Ring size effectiveness analysis
- [ ] Statistical attack resistance
- [ ] Temporal analysis resistance
- [ ] Cluster analysis resistance

### 5.3 Stealth Address Privacy

**Review Focus:**
- [ ] Output unlinkability across transactions
- [ ] Sender privacy (no public address on chain)
- [ ] Recipient can uniquely detect their outputs
- [ ] Ephemeral key doesn't leak recipient identity

---

## 6. High Priority: Network Security

### 6.1 P2P Protocol

**Location:** `botho/src/network/`

**Review Focus:**
- [ ] Eclipse attack resistance (peer diversity)
- [ ] Sybil attack mitigation
- [ ] Rate limiting effectiveness
- [ ] Bandwidth amplification prevention
- [ ] Message validation before processing
- [ ] Peer reputation management

**Rate Limits:**
```
MAX_REQUESTS_PER_MINUTE, MAX_REQUEST_SIZE, MAX_RESPONSE_SIZE
```

### 6.2 Chain Synchronization

**Location:** `botho/src/network/sync.rs`

**Review Focus:**
- [ ] Block request rate limiting
- [ ] Invalid block rejection
- [ ] Checkpoint verification
- [ ] Reorg depth limits
- [ ] Compact block validation

### 6.3 RPC Security

**Location:** `botho/src/rpc/`

**Review Focus:**
- [ ] HMAC-SHA256 authentication correctness
- [ ] Timestamp replay window (check tolerance)
- [ ] Rate limiting per API key
- [ ] IP whitelist enforcement
- [ ] Input validation on all endpoints
- [ ] View key handling (information disclosure)
- [ ] CORS configuration

**Known Issue:**
```
rpc/mod.rs: "TODO: Full validation requires checking..."
```

---

## 7. Medium Priority: Wallet Security

### 7.1 Key Storage

**Location:** `botho/src/wallet.rs`, `botho-wallet/src/storage.rs`

**Review Focus:**
- [ ] Mnemonic protection in memory
- [ ] Private key zeroization after use
- [ ] No plaintext key logging
- [ ] Secure random number generation

### 7.2 Windows Credential Storage

**Location:** `botho-wallet/src/storage.rs`

**Contains unsafe code for Windows DPAPI integration.**

**Review Focus:**
- [ ] DPAPI usage correctness
- [ ] Credential Manager integration
- [ ] Token cleanup and zeroization
- [ ] Cross-user access prevention
- [ ] Handle lifecycle management

### 7.3 Transaction Signing

**Review Focus:**
- [ ] Signing isolation (no key leakage)
- [ ] UTXO spent key recovery correctness
- [ ] Decoy selection during signing
- [ ] Fee calculation accuracy

---

## 8. Medium Priority: Unsafe Rust Code

### 8.1 LMDB FFI

**Location:** `botho/src/ledger/store.rs:71-76`

```rust
let env = unsafe {
    EnvOpenOptions::new()
        .max_dbs(6)
        .map_size(1024 * 1024 * 1024)
        .open(path)
}
```

**Review Focus:**
- [ ] LMDB version safety
- [ ] Memory-mapped file bounds
- [ ] Concurrent access safety
- [ ] Error handling on open failure

### 8.2 LRU Cache

**Location:** `common/src/lru.rs`

**Review Focus:**
- [ ] Pointer validity across iterations
- [ ] Entry index bounds checking
- [ ] Lifetime safety
- [ ] No use-after-free

### 8.3 Windows API Calls

**Location:** `botho-wallet/src/storage.rs`

**Review Focus:**
- [ ] Handle cleanup on error paths
- [ ] Buffer size validation
- [ ] Error code handling

---

## 9. Medium Priority: Dependencies

### 9.1 Cryptographic Libraries

| Crate | Version | Purpose | Action |
|-------|---------|---------|--------|
| `curve25519-dalek` | 4 | Ristretto points | Verify no CVEs |
| `blake2` | 0.10 | BLAKE2 hashing | Verify no CVEs |
| `sha2` | 0.10 | SHA-256 | Verify no CVEs |
| `aes` | 0.8 | AES-256-CTR | Verify no CVEs |
| `rand_core` | 0.6 | CSPRNG | Verify no CVEs |
| `zeroize` | 1 | Secure clear | Verify no CVEs |

### 9.2 Dependency Audit

- [x] Run `cargo audit` and resolve all advisories
- [ ] Review transitive dependencies for crypto usage
- [ ] Check for yanked versions
- [ ] Verify feature flags don't expose internals

### 9.3 Vulnerabilities Found (2025-12-30)

**Fixed:**
- [x] `crossbeam-channel` 0.5.12 → 0.5.15 (RUSTSEC-2025-0024: double free)
- [x] `tracing-subscriber` 0.3.6 → 0.3.22 (RUSTSEC-2025-0055: ANSI escape poisoning)

**Remaining (low severity, transitive dependency):**

| Crate | Version | Issue | Status |
|-------|---------|-------|--------|
| `ring` | 0.16.20 | RUSTSEC-2025-0009: AES panic (DoS) | Requires upstream updates to jsonwebtoken/rustls |

*Note: ring 0.16 is required by ethers (bridge) and sentry (logging). The vulnerability is DoS-only, requires overflow-checks enabled, and affects QUIC or 64GB+ encryption chunks. Low impact for node operation.*

**Warnings (yanked crates):**
- `xml-rs` 0.8.14 (via igd-next → libp2p-upnp)

---

## 10. Known Issues & TODOs

### Must Fix Before Audit

1. **SCP Ballot Ordering** (FIXED)
   - Location: `consensus/scp/src/slot.rs:382`
   - Issue: TODO said "Reject messages with incorrectly ordered values"
   - Fix Applied: Added `Ballot::is_values_sorted()` and validation in `Msg::validate()`
   - All ballot values in Prepare, Commit, and Externalize messages are now validated

2. **RPC Validation**
   - Location: `botho/src/rpc/mod.rs`
   - Issue: Incomplete transaction validation noted

3. **Cluster Wealth Tracking**
   - Location: `botho/src/mempool.rs:99`
   - Issue: Always returns 0, disabling progressive fees

### Document for Auditors

1. **LION Transaction Creation**
   - `wallet.rs`: "TODO: Implement LION ring signature transaction creation"
   - Status: PQ signatures available but not integrated in wallet

2. **Deprecated APIs**
   - `Wallet::sign_transaction()` marked deprecated
   - Simple transactions removed (privacy-by-default)

---

## 10.5 Ring Structure & Minting Dynamics (Updated 2025-12)

### Ring Sizes by Transaction Type

| Transaction Type | Ring Size | Signature Size | Privacy Bits | Rationale |
|------------------|-----------|----------------|--------------|-----------|
| **CLSAG (Standard-Private)** | 20 | ~700 bytes | 4.32 | Default for all txs, larger than Monero (16) |
| **LION (PQ-Private)** | 11 | ~36 KB | 3.30 | Optimized for lattice signature overhead |

**Key Constants:**
- `transaction/types/src/constants.rs`: `RING_SIZE = 20`
- `botho/src/transaction.rs`: `DEFAULT_RING_SIZE = 20`, `MIN_RING_SIZE = 20`
- `crypto/lion/src/params.rs`: `RING_SIZE = 11`
- `botho/src/transaction.rs`: `PQ_RING_SIZE = 11`, `MIN_PQ_RING_SIZE = 11`

**Review Focus:**
- [ ] Ring size enforcement at transaction validation
- [ ] No bypass allowing smaller rings
- [ ] Decoy selection doesn't repeat real input

### Two-Phase Monetary Policy

**Location:** `cluster-tax/src/monetary.rs`

| Phase | Duration | Mechanism | Target |
|-------|----------|-----------|--------|
| **Phase 1: Halving** | Years 0-10 | 5 halvings every ~2 years | ~100M BTH distributed |
| **Phase 2: Tail** | Year 10+ | Difficulty-adjusted minting | 2% NET annual inflation |

**Phase 1 Emission Schedule:**
```
Halving 0 (years 0-2):  ~50 BTH/block    → ~26.3M BTH
Halving 1 (years 2-4):  ~25 BTH/block    → ~13.1M BTH
Halving 2 (years 4-6):  ~12.5 BTH/block  → ~6.6M BTH
Halving 3 (years 6-8):  ~6.25 BTH/block  → ~3.3M BTH
Halving 4 (years 8-10): ~3.125 BTH/block → ~1.6M BTH
────────────────────────────────────────────────────
Total Phase 1:                           ~100M BTH
```

**Phase 2 Dynamics:**
- Block reward is fixed at transition
- Difficulty adjusts to achieve NET 2% inflation
- Fee burns reduce effective inflation (all cluster taxes burned)
- Block time floats within 45-90 second bounds

### Dynamic Block Timing

**Location:** `botho/src/block.rs:dynamic_timing`

| Transaction Rate | Block Time | Purpose |
|------------------|------------|---------|
| 20+ tx/s | 3 seconds | Very high load |
| 5+ tx/s | 5 seconds | High load |
| 1+ tx/s | 10 seconds | Medium load |
| 0.2+ tx/s | 20 seconds | Low load |
| <0.2 tx/s | 40 seconds | Idle |

**Review Focus:**
- [ ] Block time computation is deterministic from chain state
- [ ] All validators compute same target for given tip
- [ ] No timestamp manipulation attacks
- [ ] Difficulty adjustment bounds (25% max change per epoch)

### Difficulty Adjustment

**Two parallel systems exist (review interaction):**

1. **Transaction-based** (`block.rs:difficulty`):
   - Adjusts every 1000 transactions
   - Ties monetary policy to network usage
   - `ADJUSTMENT_TX_COUNT = 1000`

2. **Block-based** (`node/mod.rs`):
   - Legacy: adjusts every `ADJUSTMENT_WINDOW = 180` blocks
   - Used for network block synchronization

**Review Focus:**
- [ ] No conflict between adjustment mechanisms
- [ ] Overflow safety verified for 270+ years
- [ ] Difficulty cannot be manipulated for advantage

---

## 11. Security Claims to Verify

| Claim | Description | Verification Method |
|-------|-------------|---------------------|
| Privacy by Default | CLSAG uses ring 20, LION uses ring 11 | Code review, test |
| Double-Spend Prevention | Key images are unique and tracked | Formal analysis |
| Amount Hiding | Pedersen commitments hide values | Cryptographic proof |
| Recipient Privacy | Stealth addresses unlinkable | Protocol analysis |
| Post-Quantum Ready | LION + ML-KEM-768 available | Feature flag test |
| Consensus Safety | No conflicting values externalized | SCP formal proofs |
| Consensus Liveness | Progress guaranteed | SCP formal proofs |
| Decoy Privacy | 19 decoys (CLSAG), 10 decoys (LION) | Statistical analysis |
| Fee Burns | All cluster taxes burned (deflationary) | Code review |
| Inflation Control | 2% NET target in Phase 2 | Simulation |

---

## 12. Pre-Audit Checklist

### Documentation

- [ ] Architecture diagram
- [ ] Threat model document
- [ ] Key ceremony procedures
- [ ] Cryptographic specification
- [ ] Protocol specification

### Code Quality

- [ ] All TODOs in critical paths resolved
- [ ] `cargo clippy` passes with no warnings
- [ ] `cargo fmt` applied
- [ ] All tests passing
- [ ] No `unwrap()` in production code paths
- [ ] Error handling reviewed

### Testing

- [ ] Unit test coverage > 80% for crypto code
- [ ] Integration tests for transaction lifecycle
- [ ] Fuzz testing on serialization/deserialization
- [ ] Fuzz testing on signature verification
- [ ] Property-based testing for crypto operations

### Security Hardening

- [ ] `#![deny(unsafe_code)]` on all crypto crates
- [ ] Dependency audit clean
- [ ] No hardcoded secrets
- [ ] Logging doesn't expose sensitive data
- [ ] Rate limiting on all public endpoints

### Audit Logistics

- [ ] Provide auditors with read access to repository
- [ ] Designated point of contact for questions
- [ ] Commit hash frozen for audit period
- [ ] Test environment available for auditors
- [ ] Build instructions documented

---

## 13. Internal Audit Process

Internal security audits are tracked in the [`audits/`](audits/) directory.

### Process Overview

1. **Audit reports** are stored as dated markdown files (e.g., `audits/2025-12-30.md`)
2. **Template** available at `audits/TEMPLATE.md`
3. **Progress** tracked in `audits/README.md`

### Path to External Audit

External audit will be commissioned when:
- 3+ consecutive full audits with no Critical/High findings
- All Medium findings from previous audits resolved
- Test coverage > 80% on crypto code
- Fuzz testing infrastructure operational

### Current Status

| Audit | Date | Critical | High | Medium | Status |
|-------|------|----------|------|--------|--------|
| [Cycle 3](audits/2025-12-30-cycle3.md) | 2025-12-30 | **0** | **1** | 3 | **Significant Progress** |
| [Cycle 2](audits/2025-12-30-cycle2.md) | 2025-12-30 | 3 | 7 | 15+ | Issues Found |
| [Cycle 1](audits/2025-12-30.md) | 2025-12-30 | 1 (fixed) | 1 (fixed) | 2 | Issues Found |

### Issues Resolved in Cycle 3

| Issue | Severity | Resolution |
|-------|----------|------------|
| Mnemonic zeroization | CRITICAL | `Zeroizing<String>` wrapper in keys.rs |
| Tauri mnemonic exposure | CRITICAL | Session-based architecture |
| ring v0.16.20 vulnerability | CRITICAL | Updated to v0.17.14 |
| Test mnemonic detection | HIGH | `validate_not_test_mnemonic()` |
| LRU unsafe documentation | HIGH | SAFETY comments added |
| Crypto deny(unsafe_code) | MEDIUM | 10/10 crates now enforced |

### Remaining Issues

| Issue | Severity | Location | Notes |
|-------|----------|----------|-------|
| Wallet decryption rate limiting | HIGH | `storage.rs` | Argon2id mitigates |
| LION rejection sampling margin | MEDIUM | `lion/params.rs` | Needs crypto review |
| Gossipsub rate limiting | MEDIUM | `network/` | Defense in depth |
| Empty cluster tags similarity | LOW | `decoy_selection.rs:136` | Bootstrap edge case |

---

## Appendix: File Location Quick Reference

| Component | Location |
|-----------|----------|
| MLSAG | `crypto/ring-signature/src/ring_signature/mlsag*.rs` |
| CLSAG | `crypto/ring-signature/src/ring_signature/clsag.rs` |
| LION | `crypto/lion/src/` |
| Account Keys | `account-keys/src/` |
| One-Time Keys | `crypto/ring-signature/src/onetime_keys.rs` |
| Commitments | `crypto/ring-signature/src/amount/` |
| SCP Consensus | `consensus/scp/src/` |
| Block Validation | `botho/src/consensus/validation.rs` |
| Transaction | `botho/src/transaction.rs` |
| Mempool | `botho/src/mempool.rs` |
| Decoy Selection | `botho/src/decoy_selection.rs` |
| Network | `botho/src/network/` |
| RPC | `botho/src/rpc/` |
| Wallet | `botho/src/wallet.rs` |
| Ledger | `botho/src/ledger/store.rs` |
