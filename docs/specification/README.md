# Botho Protocol Specification

This directory contains protocol specification documents for Botho.

## Current protocol

The implemented wire protocol version is **4.1.0** (minimum supported peer
version: **4.0.0**), as defined by `PROTOCOL_VERSION` and
`MIN_SUPPORTED_PROTOCOL_VERSION` in
[`botho/src/network/discovery.rs`](../../botho/src/network/discovery.rs).

**No consolidated specification document exists yet for the current
protocol.** The v0.x documents below are historical snapshots of an early
design; the protocol has since gone through several breaking revisions. A
current consolidated spec snapshot is future work. Until it exists, the
authoritative sources are the code and the concept docs.

### Major deltas since the v0.2.0 snapshot

| Area | Change | Authoritative source |
|------|--------|---------------------|
| Proof of work | RandomX (CPU-egalitarian) replaced the earlier PoW design (#443) | [`botho/src/pow.rs`](../../botho/src/pow.rs) |
| Monetary unit | Single-unit picocredit migration; nanoBTH fee/display tier retired (#694) | [`transaction/types/src/constants.rs`](../../transaction/types/src/constants.rs) |
| Emission schedule | ~611M BTH total over 5 yearly halvings (#351) | [`botho/src/monetary.rs`](../../botho/src/monetary.rs) |
| Difficulty | M5 time-based difficulty controller (#554) | [`botho/src/block.rs`](../../botho/src/block.rs) |
| Fees & lottery | 80/20 fee split; cluster-weighted lottery selection | [`botho/src/consensus/lottery.rs`](../../botho/src/consensus/lottery.rs), [`cluster-tax/src/lottery.rs`](../../cluster-tax/src/lottery.rs) |
| Demurrage | Cluster-tax demurrage on idle balances | [`cluster-tax/src/demurrage.rs`](../../cluster-tax/src/demurrage.rs) |
| Block timing | Dynamic block timing (3-40s range) | [`botho/src/block.rs`](../../botho/src/block.rs) |
| Prose documentation | Concept-level descriptions of the current design | [`docs/concepts/`](../concepts/README.md) |

## Historical documents

| Version | Status | Date | Description |
|---------|--------|------|-------------|
| [v0.2.0](protocol-v0.2.0.md) | Historical snapshot | 2025-01-03 | Early draft spec (LION removed); does **not** describe the current wire protocol |
| [v0.1.0](protocol-v0.1.0.md) | Historical snapshot | 2024-12-31 | Initial spec (includes deprecated LION); does **not** describe the current wire protocol |

These documents are preserved for design history. **Do not implement against
them.** A third-party implementer or auditor should treat the code paths in
the table above as normative.

## Overview

The v0.x specification snapshots covered:

- **Transaction Format**: Minting and Private transaction structures
- **Consensus (SCP)**: Stellar Consensus Protocol implementation
- **Network Protocol**: P2P messaging and synchronization
- **Cryptographic Primitives**: CLSAG, ML-KEM, ML-DSA, Bulletproofs
- **Block Structure**: Headers, PoW, and minting transactions
- **Monetary System**: Units, supply schedule, and fee structure
- **Network Configuration**: Ports, addresses, and parameters

## Purpose

A complete, current specification would enable:

1. **Interoperability**: Third-party wallet and node implementations
2. **Security Audits**: Complete protocol documentation for auditors
3. **Academic Review**: Peer review of cryptographic constructions
4. **Developer Onboarding**: Comprehensive protocol reference
5. **Compliance**: Regulatory documentation requirements

Until a current snapshot is produced, these needs are served by the code
references in the [Current protocol](#current-protocol) section and the
[concept docs](../concepts/README.md).

## Versioning

Specifications use semantic versioning:

- **Major**: Breaking protocol changes
- **Minor**: Backward-compatible additions
- **Patch**: Clarifications and corrections

## Contributing

To propose specification changes:

1. Create an issue describing the change
2. Submit a PR with updated specification
3. Ensure implementation matches specification
4. Request review from at least 2 contributors

## Related Documentation

- [Architecture](../concepts/architecture.md): System design overview
- [Transactions](../concepts/transactions.md): User-facing transaction guide
- [Privacy](../concepts/privacy.md): Privacy features and cryptography
- [API Reference](../api.md): JSON-RPC and WebSocket API
