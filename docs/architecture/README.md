# Botho Architecture Overview

This directory contains detailed architecture documentation for external security auditors.

## Document Index

| Document | Description |
|----------|-------------|
| [Cryptographic Architecture](crypto.md) | Key hierarchy, ring signatures, post-quantum extensions |
| [Consensus Architecture](consensus.md) | SCP implementation, block production, validation |
| [Network Architecture](network.md) | P2P protocol, peer discovery, DDoS protections |
| [Wallet Architecture](wallet.md) | Key storage, transaction signing, output scanning |

## System Component Diagram

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                                    BOTHO NODE                                    │
│                                                                                  │
│  ┌────────────────────────────────────────────────────────────────────────────┐ │
│  │                           TRUST BOUNDARY: Network                          │ │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐  ┌───────────────┐  │ │
│  │  │   libp2p     │  │  Gossipsub   │  │    Sync      │  │     RPC       │  │ │
│  │  │  Transport   │  │   Topics     │  │   Protocol   │  │   Server      │  │ │
│  │  │  (TCP/QUIC)  │  │              │  │              │  │  (HTTP/WS)    │  │ │
│  │  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘  └───────┬───────┘  │ │
│  │         │                 │                 │                  │          │ │
│  │         └─────────────────┴─────────────────┴──────────────────┘          │ │
│  │                                    │                                       │ │
│  └────────────────────────────────────┼───────────────────────────────────────┘ │
│                                       │                                         │
│  ┌────────────────────────────────────┼───────────────────────────────────────┐ │
│  │                    TRUST BOUNDARY: Consensus                               │ │
│  │                                    ▼                                       │ │
│  │  ┌──────────────────────────────────────────────────────────────────────┐  │ │
│  │  │                         CONSENSUS SERVICE                             │  │ │
│  │  │  ┌────────────────┐  ┌────────────────┐  ┌────────────────────────┐  │  │ │
│  │  │  │   SCP Node     │  │ Block Builder  │  │  Transaction Validator │  │  │ │
│  │  │  │  (Byzantine    │  │ (Externalized  │  │  (Signature + Range    │  │  │ │
│  │  │  │   Fault Tol.)  │  │  Values→Block) │  │   Proof Verification)  │  │  │ │
│  │  │  └────────┬───────┘  └────────┬───────┘  └────────────┬───────────┘  │  │ │
│  │  │           │                   │                       │              │  │ │
│  │  │           └───────────────────┴───────────────────────┘              │  │ │
│  │  └──────────────────────────────────┬───────────────────────────────────┘  │ │
│  │                                     │                                      │ │
│  └─────────────────────────────────────┼──────────────────────────────────────┘ │
│                                        │                                        │
│  ┌─────────────────────────────────────┼────────────────────────────────────┐   │
│  │                     TRUST BOUNDARY: Storage                              │   │
│  │                                     ▼                                    │   │
│  │  ┌──────────────────┐  ┌──────────────────┐  ┌────────────────────────┐ │   │
│  │  │     MEMPOOL      │  │      LEDGER      │  │       WALLET          │ │   │
│  │  │  ┌────────────┐  │  │  ┌────────────┐  │  │  ┌──────────────────┐ │ │   │
│  │  │  │ Pending Tx │  │  │  │   LMDB     │  │  │  │   AccountKey     │ │ │   │
│  │  │  │ Key Images │  │  │  │  (Blocks,  │  │  │  │  (View + Spend)  │ │ │   │
│  │  │  │ Fee Queue  │  │  │  │   UTXOs)   │  │  │  │   Zeroizing      │ │ │   │
│  │  │  └────────────┘  │  │  └────────────┘  │  │  └──────────────────┘ │ │   │
│  │  └──────────────────┘  └──────────────────┘  └────────────────────────┘ │   │
│  │                                                                          │   │
│  └──────────────────────────────────────────────────────────────────────────┘   │
│                                                                                  │
│  ┌──────────────────────────────────────────────────────────────────────────┐   │
│  │                      TRUST BOUNDARY: Minting                             │   │
│  │  ┌──────────────────────────────────────────────────────────────────────┐│   │
│  │  │                           MINTER                                     ││   │
│  │  │  ┌────────────────┐  ┌────────────────┐  ┌────────────────────────┐ ││   │
│  │  │  │   PoW Search   │  │  MintingTx     │  │   Difficulty Adj.      │ ││   │
│  │  │  │  (Multi-thread)│  │  Construction  │  │   (per 1000 tx)        │ ││   │
│  │  │  └────────────────┘  └────────────────┘  └────────────────────────┘ ││   │
│  │  └──────────────────────────────────────────────────────────────────────┘│   │
│  └──────────────────────────────────────────────────────────────────────────┘   │
│                                                                                  │
└──────────────────────────────────────────────────────────────────────────────────┘
```

## Data Flow Overview

### Transaction Lifecycle

```
┌──────────────┐     ┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│   User       │     │   Wallet     │     │   Mempool    │     │   Network    │
│   Intent     │────▶│   Signs Tx   │────▶│   Validates  │────▶│   Gossipsub  │
│              │     │   (CLSAG)    │     │   & Queues   │     │   Broadcast  │
└──────────────┘     └──────────────┘     └──────────────┘     └──────────────┘
                                                                       │
                                                                       ▼
┌──────────────┐     ┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│   Recipient  │     │   Ledger     │     │   Consensus  │     │   Minter     │
│   Scans      │◀────│   Stores     │◀────│   Finalizes  │◀────│   Includes   │
│   Outputs    │     │   Block      │     │   Block      │     │   in Block   │
└──────────────┘     └──────────────┘     └──────────────┘     └──────────────┘
```

### Block Production

```
         ┌─────────────────────────────────────────────────────────────┐
         │                    BLOCK PRODUCTION                         │
         │                                                             │
         │   ┌─────────────┐     ┌─────────────┐     ┌─────────────┐  │
         │   │   Minter    │────▶│   Submit    │────▶│    SCP      │  │
         │   │  Finds PoW  │     │  MintingTx  │     │   Quorum    │  │
         │   └─────────────┘     └─────────────┘     └──────┬──────┘  │
         │                                                   │         │
         │                         ┌─────────────────────────┘         │
         │                         ▼                                   │
         │   ┌─────────────┐     ┌─────────────┐     ┌─────────────┐  │
         │   │  Broadcast  │◀────│   Build     │◀────│ Externalize │  │
         │   │  to Peers   │     │   Block     │     │   Value     │  │
         │   └─────────────┘     └─────────────┘     └─────────────┘  │
         │                                                             │
         └─────────────────────────────────────────────────────────────┘
```

## Security-Critical Paths

### 1. Key Derivation Path (CRITICAL)

```
BIP39 Mnemonic (24 words)
         │
         ▼ PBKDF2(mnemonic, "TREZOR", 2048)
    ┌─────────┐
    │  Seed   │
    └────┬────┘
         │
         ▼ SLIP-10 Hardened Derivation
    ┌─────────────────────────────────────┐
    │         m/44'/866'/0'               │
    │  ┌─────────────┬─────────────┐      │
    │  │ View Key    │ Spend Key   │      │
    │  │ (a, A)      │ (b, B)      │      │
    │  └──────┬──────┴──────┬──────┘      │
    │         │             │             │
    │         ▼             ▼             │
    │    Subaddress Derivation            │
    │    D = B + Hs(a||i)*G               │
    │    C = A + Hs(a||i)*G               │
    └─────────────────────────────────────┘
```

### 2. Transaction Signing Path (CRITICAL)

```
┌────────────────────────────────────────────────────────────────────────┐
│                      TRANSACTION SIGNING                                │
│                                                                         │
│  ┌──────────────┐     ┌──────────────┐     ┌──────────────────────┐   │
│  │  Select      │     │  Compute     │     │  Generate            │   │
│  │  Ring (20)   │────▶│  Key Image   │────▶│  CLSAG Signature     │   │
│  │  Decoys      │     │  I = x*Hp(P) │     │  (challenge chain)   │   │
│  └──────────────┘     └──────────────┘     └──────────────────────┘   │
│         │                                              │              │
│         │              ┌───────────────────────────────┘              │
│         ▼              ▼                                              │
│  ┌──────────────────────────────────────────────────────────────┐    │
│  │                    RING SIGNATURE                             │    │
│  │                                                               │    │
│  │  For i in 0..ring_size:                                      │    │
│  │    L[i] = s[i]*G + c[i]*P[i]                                 │    │
│  │    R[i] = s[i]*Hp(P[i]) + c[i]*I                             │    │
│  │    c[i+1] = H(L[i], R[i], message)                           │    │
│  │                                                               │    │
│  │  Verify: c[0] == c[ring_size] (loop closure)                 │    │
│  └──────────────────────────────────────────────────────────────┘    │
│                                                                         │
└────────────────────────────────────────────────────────────────────────┘
```

### 3. Consensus Path (CRITICAL)

```
┌────────────────────────────────────────────────────────────────────────┐
│                        SCP CONSENSUS                                    │
│                                                                         │
│  ┌──────────────┐     ┌──────────────┐     ┌──────────────┐           │
│  │  NOMINATE    │────▶│   PREPARE    │────▶│   COMMIT     │           │
│  │  (propose)   │     │  (vote)      │     │  (accept)    │           │
│  └──────────────┘     └──────────────┘     └──────┬───────┘           │
│                                                    │                   │
│                                                    ▼                   │
│                                           ┌──────────────┐            │
│                                           │ EXTERNALIZE  │            │
│                                           │  (finalize)  │            │
│                                           └──────────────┘            │
│                                                                         │
│  SAFETY GUARANTEE: No two honest nodes externalize different           │
│  values for the same slot (Byzantine fault tolerant)                   │
│                                                                         │
└────────────────────────────────────────────────────────────────────────┘
```

## Trust Boundaries

| Boundary | Components Inside | Threats Outside | Mitigations |
|----------|------------------|-----------------|-------------|
| **Network** | libp2p, Gossipsub, RPC | Malicious peers, DDoS, Eclipse attacks | Rate limiting, peer reputation, connection limits |
| **Consensus** | SCP Node, Validators | Byzantine nodes (up to f < n/3) | Quorum intersection, ballot ordering |
| **Storage** | LMDB, Mempool, UTXO set | Data corruption, injection | Input validation, checksums |
| **Wallet** | Private keys, Mnemonic | Key extraction, side channels | Zeroization, memory protection |

## Key Security Properties

| Property | Guarantee | Implementation |
|----------|-----------|----------------|
| **Double-Spend Prevention** | Key images are unique per output | UTXO model + key image tracking |
| **Amount Hiding** | Transaction amounts are hidden | Pedersen commitments + Bulletproofs |
| **Sender Privacy** | Sender identity is hidden | Ring signatures (CLSAG) with 20 decoys |
| **Recipient Privacy** | Recipient address not on chain | Stealth addresses (one-time keys) |
| **Consensus Safety** | No conflicting finalization | SCP with quorum intersection |
| **Post-Quantum Ready** | PQ stealth addresses and minting | ML-KEM-768, ML-DSA-65 |

## Related Documentation

- [AUDIT.md](../../AUDIT.md) - Security audit preparation checklist
- [docs/privacy.md](../concepts/privacy.md) - Privacy model and guarantees
- [docs/security.md](../concepts/security.md) - Security considerations
- [docs/transactions.md](../concepts/transactions.md) - Transaction format details
