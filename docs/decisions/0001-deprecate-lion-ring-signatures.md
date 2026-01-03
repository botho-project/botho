# ADR 0001: Deprecate LION Ring Signatures

**Status**: Accepted
**Date**: 2026-01-03
**Decision Makers**: Core Team

## Context

Botho originally implemented two ring signature schemes:

1. **CLSAG** (Classical): ~700 bytes/input, ring size 20
2. **LION** (Post-Quantum): ~36 KB/input, ring size 11

The goal was to offer users a choice between efficient classical signatures and quantum-resistant signatures for sender anonymity.

## Problem Statement

LION ring signatures create significant challenges:

### 1. Transaction Size

| Metric | CLSAG | LION |
|--------|-------|------|
| Signature size per input | ~700 bytes | ~36,000 bytes |
| Key image size | 32 bytes | 1,312 bytes |
| 2-in/2-out transaction | ~4 KB | ~75 KB |

### 2. Blockchain Growth

At moderate transaction throughput:
- **CLSAG-only**: ~100 GB/year (desktop-friendly)
- **LION-heavy**: ~1.8 TB/year (datacenter-only)

A privacy system that requires datacenter infrastructure centralizes the network and reduces the anonymity set.

### 3. Key Image Storage

LION key images are 1,312 bytes each (vs 32 bytes for CLSAG). These must be stored forever to prevent double-spending. After 50 million spends:
- CLSAG key images: 1.6 GB
- LION key images: 65 GB

### 4. Network-Level Attacks Dominate

Even with perfect cryptographic sender anonymity, nation-state adversaries can deanonymize senders through:

- IP address correlation with transaction broadcast timing
- Transaction propagation analysis
- ISP/AS-level traffic analysis
- Tor exit node correlation

LION provides no protection against these attacks, which are the dominant threat today.

### 5. Sender Privacy is Ephemeral

Unlike recipient identity (permanent on-chain), sender anonymity value degrades over time:
- Economic context becomes historical
- UTXOs get spent, reducing ring membership relevance
- Chain analysis becomes less actionable

### 6. Quantum Timeline

Cryptographically-relevant quantum computers are estimated to be 20-30 years away. The quantum threat to sender anonymity doesn't justify 50x larger transactions today.

## Decision

**Deprecate LION ring signatures and use CLSAG exclusively for all private transactions.**

Maintain post-quantum protection where it provides lasting value:
- **Recipient privacy**: ML-KEM-768 stealth addresses (PQ-safe)
- **Amount privacy**: Pedersen commitments with information-theoretic hiding (PQ-safe)
- **Sender anonymity**: CLSAG ring signatures (classical)

## Consequences

### Positive

1. **Desktop-friendly nodes**: ~100 GB/year blockchain growth
2. **Larger anonymity sets**: More users can run full nodes
3. **Simpler architecture**: One ring signature scheme to maintain
4. **Lower fees**: Smaller transactions = lower size-based fees
5. **Faster validation**: No lattice operations in transaction verification

### Negative

1. **Classical sender anonymity**: Sender identity could theoretically be deanonymized by future quantum computers
2. **Marketing narrative**: Cannot claim "fully quantum-resistant"

### Neutral

1. **Recipient privacy remains PQ**: ML-KEM-768 stealth addresses protect the most important data
2. **Amount privacy remains PQ**: Pedersen hiding is information-theoretic
3. **Network upgrade path**: If quantum computers advance faster than expected, a future hard fork could introduce a new PQ ring signature scheme

## Alternatives Considered

### 1. Keep Both Tiers (Status Quo)

- Pro: User choice
- Con: LION anonymity set would be tiny (few users would pay 10x fees)
- Con: Complexity of two signature schemes

### 2. LION-Only

- Pro: Full quantum resistance
- Con: ~1.8 TB/year blockchain growth
- Con: Datacenter-only nodes
- Con: Smaller anonymity set due to centralization

### 3. Hybrid with LION Key Storage

Store LION public keys (1,312 bytes) on all outputs so any output can be a LION ring member.

- Pro: Full LION anonymity set
- Con: TxOutput grows from ~120 bytes to ~1,432 bytes
- Con: All transactions (including CLSAG) pay the storage cost
- Con: Blockchain growth still ~175 TB/year

### 4. Prunable LION Keys

Store LION keys only while outputs are unspent, prune after spending.

- Pro: Reduces long-term storage
- Con: UTXO set still grows ~12x
- Con: LION key images (1,312 bytes) must be stored forever
- Con: Complexity of pruning infrastructure

## Implementation

1. Update documentation to reflect single-tier architecture
2. Remove LION code from `crypto/lion/`
3. Update transaction validation to remove LION support
4. Clean up Cargo.toml dependencies
5. Update wallet CLI to remove `--quantum-private` flag

## References

- [Why This Architecture?](../privacy.md#why-this-architecture) - Detailed rationale
- [ML-KEM (FIPS 203)](https://csrc.nist.gov/pubs/fips/203/final) - Post-quantum key encapsulation
- [CLSAG Paper](https://eprint.iacr.org/2019/654.pdf) - Ring signature specification
