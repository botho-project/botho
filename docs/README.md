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
| [Why Botho?](comparison.md) | Comparison with Bitcoin, Monero, Zcash |
| [Architecture](architecture.md) | System design and component overview |
| [Transaction Types](transactions.md) | Minting, Standard, and Private transactions |
| [Privacy](privacy.md) | Privacy features and cryptography |
| [Tokenomics](tokenomics.md) | Supply, emission, fees, and economics |
| [Monetary Policy](monetary-policy.md) | Difficulty adjustment, epochs, and fork upgrades |

### Running a Node
| Document | Description |
|----------|-------------|
| [Configuration](configuration.md) | Complete configuration reference |
| [Minting](minting.md) | Mining setup and economics |
| [API Reference](api.md) | JSON-RPC and WebSocket API |
| [Troubleshooting](troubleshooting.md) | Common issues and solutions |

### For Developers
| Document | Description |
|----------|-------------|
| [Developer Guide](developer-guide.md) | Build applications with Botho |
| [Testing Guide](testing.md) | Run and write tests |

### Operations & Security
| Document | Description |
|----------|-------------|
| [Deployment](deployment.md) | systemd, Docker, monitoring |
| [Security](security.md) | Key management, threat model |
| [Backup & Recovery](backup.md) | Wallet backup procedures |

### Ecosystem
| Document | Description |
|----------|-------------|
| [Exchange Integration](exchange-integration.md) | List BTH on your exchange |
| [Merchant Guide](merchant-guide.md) | Accept BTH payments |

## What is Botho?

Botho combines:

- **Proof-of-Work Minting**: SHA-256 minting with variable difficulty
- **Three Transaction Types**: Minting (block rewards), Standard (hidden amounts), Private (hidden sender + amounts)
- **Pure Post-Quantum Security**: ML-KEM stealth addresses, ML-DSA signatures, LION ring signatures
- **Confidential Amounts**: Pedersen commitments with Bulletproofs range proofs
- **Byzantine Fault Tolerance**: Stellar Consensus Protocol (SCP) for consensus
- **Progressive Fees**: Cluster-based taxation that discourages wealth concentration

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

## Project Status

Botho is in active development. See the main [README](../README.md) for current status and the [PLAN.md](../PLAN.md) for implementation details.

## Contributing

See [CONTRIBUTING.md](../CONTRIBUTING.md) for contribution guidelines.

## License

See the [LICENSE](../LICENSE) file for details.
