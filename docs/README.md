# Botho Documentation

Welcome to the Botho documentation. Botho is a privacy-preserving, mined cryptocurrency built on proven cryptographic foundations.

## Quick Links

### Getting Started
| Document | Description |
|----------|-------------|
| [Getting Started](getting-started.md) | Build, install, and run your first node |
| [FAQ](FAQ.md) | Frequently asked questions |
| [Glossary](glossary.md) | Technical terms explained |

### Understanding Botho
| Document | Description |
|----------|-------------|
| [Why Botho?](concepts/comparison.md) | Comparison with Bitcoin, Monero, Zcash |
| [Architecture](concepts/architecture.md) | System design and component overview |
| [Transaction Types](concepts/transactions.md) | Minting, Plain, Standard-Private, and PQ-Private transactions |
| [Privacy](concepts/privacy.md) | Privacy features and cryptography |
| [Tokenomics](concepts/tokenomics.md) | Supply, emission, fees, and economics |
| [Progressive Fees](concepts/progressive-fees.md) | Provenance-based taxation that defeats Sybil attacks |
| [Monetary Policy](concepts/monetary-policy.md) | Difficulty adjustment, epochs, and fork upgrades |
| [Security](concepts/security.md) | Security model and best practices |

### Running a Node
| Document | Description |
|----------|-------------|
| [Configuration](operations/configuration.md) | Complete configuration reference |
| [Minting](minting.md) | Mining setup and economics |
| [Deployment](operations/deployment.md) | Production deployment (systemd, Docker) |
| [Monitoring](operations/monitoring.md) | Metrics, alerting, and dashboards |
| [Troubleshooting](operations/troubleshooting.md) | Common issues and solutions |

### Backup & Recovery
| Document | Description |
|----------|-------------|
| [Backup](operations/backup.md) | Wallet backup procedures |
| [Disaster Recovery](operations/disaster-recovery.md) | Recovery procedures and runbooks |
| [Seed Node Backup](operations/seed-node-backup.md) | Seed node specific backup |

### For Developers
| Document | Description |
|----------|-------------|
| [API Reference](api.md) | JSON-RPC and WebSocket API |
| [Developer Guide](developer-guide.md) | Build applications with Botho |
| [Testing Guide](testing.md) | Run and write tests |

### Ecosystem
| Document | Description |
|----------|-------------|
| [Exchange Integration](exchange-integration.md) | List BTH on your exchange |
| [Merchant Guide](merchant-guide.md) | Accept BTH payments |

### Design & Research
| Document | Description |
|----------|-------------|
| [Design Documents](design/README.md) | Proposals and roadmaps |
| [Network Privacy](design/traffic-privacy-roadmap.md) | Traffic analysis resistance design |
| [Research](research/README.md) | Analysis and comparisons |
| [Decisions](decisions/0001-deprecate-lion-ring-signatures.md) | Architecture Decision Records |

### Protocol Specification
| Document | Description |
|----------|-------------|
| [Specification](specification/README.md) | Formal protocol specification |
| [Protocol v0.1.0](specification/protocol-v0.1.0.md) | Complete protocol reference |

### Architecture Deep Dives
| Document | Description |
|----------|-------------|
| [Component Overview](architecture/README.md) | Architecture component index |
| [Consensus](architecture/consensus.md) | SCP implementation details |
| [Cryptography](architecture/crypto.md) | Key hierarchy and signatures |
| [Network](architecture/network.md) | P2P protocol and discovery |
| [Wallet](architecture/wallet.md) | Key storage and scanning |

### Bridge (Experimental)
| Document | Description |
|----------|-------------|
| [Bridge Architecture](bridge/architecture.md) | Cross-chain bridge design |
| [Bridge Security](bridge/security.md) | Hot wallet security |

### Security
| Document | Description |
|----------|-------------|
| [Threat Model](security/threat-model.md) | Adversary model and mitigations |

---

## What is Botho?

Botho combines:

- **Proof-of-Work Minting**: SHA-256 minting with variable difficulty
- **Two Transaction Types**: Minting (block rewards) and Private (CLSAG ring signatures)
- **Hybrid Post-Quantum Security**: ML-KEM stealth addresses, CLSAG ring signatures
- **Confidential Amounts**: Pedersen commitments with Bulletproofs range proofs
- **Byzantine Fault Tolerance**: Stellar Consensus Protocol (SCP) for consensus
- **Progressive Fees**: Cluster-based taxation that discourages wealth concentration
- **Network Privacy**: Onion Gossip for transaction origin hiding (planned)

The native currency unit is **BTH** (1 BTH = 1,000,000,000 nanoBTH).

## Quick Start

```bash
# Build
cargo build --release

# Initialize wallet
botho init

# Run node
botho run

# Run with minting
botho run --mint
```

## Commands

| Command | Description |
|---------|-------------|
| `botho init` | Create wallet with 24-word mnemonic |
| `botho init --recover` | Recover wallet from existing mnemonic |
| `botho run` | Sync blockchain and scan wallet |
| `botho run --mint` | Run with minting enabled |
| `botho status` | Show sync status, balance, minting stats |
| `botho balance` | Show wallet balance |
| `botho address` | Show receiving address |
| `botho send <addr> <amt>` | Send BTH |

## Documentation Structure

```
docs/
├── getting-started.md      # First steps
├── FAQ.md                  # Common questions
├── glossary.md             # Term definitions
├── api.md                  # API reference
├── developer-guide.md      # Building with Botho
├── testing.md              # Testing guide
├── minting.md              # Mining setup
├── merchant-guide.md       # Accept payments
├── exchange-integration.md # Exchange listing
│
├── concepts/               # Core concepts (what/why)
│   ├── architecture.md
│   ├── privacy.md
│   ├── transactions.md
│   ├── tokenomics.md
│   ├── monetary-policy.md
│   ├── progressive-fees.md
│   ├── security.md
│   └── comparison.md
│
├── operations/             # Running nodes (how)
│   ├── configuration.md
│   ├── deployment.md
│   ├── monitoring.md
│   ├── backup.md
│   ├── disaster-recovery.md
│   ├── troubleshooting.md
│   └── runbooks/
│
├── architecture/           # Component deep-dives
├── design/                 # Proposals and roadmaps
├── research/               # Analysis and comparisons
├── specification/          # Formal protocol spec
├── decisions/              # ADRs
├── security/               # Threat model
└── bridge/                 # Cross-chain bridge
```

## Project Status

Botho is in active development. See the main [README](../README.md) for current status.

## Contributing

See [CONTRIBUTING.md](../CONTRIBUTING.md) for contribution guidelines.

## License

See the [LICENSE](../LICENSE) file for details.
