# Botho Protocol Specification

This directory contains the formal specification of the Botho protocol.

## Documents

| Version | Status | Date | Description |
|---------|--------|------|-------------|
| [v0.1.0](protocol-v0.1.0.md) | Draft | 2024-12-31 | Initial specification |

## Overview

The Botho protocol specification provides complete documentation for:

- **Transaction Format**: Classical and post-quantum transaction structures
- **Consensus (SCP)**: Stellar Consensus Protocol implementation
- **Network Protocol**: P2P messaging and synchronization
- **Cryptographic Primitives**: CLSAG, ML-KEM, ML-DSA, Bulletproofs (LION deprecated per ADR-0001)
- **Block Structure**: Headers, PoW, and minting transactions
- **Monetary System**: Units, supply schedule, and fee structure
- **Network Configuration**: Ports, addresses, and parameters

## Purpose

This specification enables:

1. **Interoperability**: Third-party wallet and node implementations
2. **Security Audits**: Complete protocol documentation for auditors
3. **Academic Review**: Peer review of cryptographic constructions
4. **Developer Onboarding**: Comprehensive protocol reference
5. **Compliance**: Regulatory documentation requirements

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
