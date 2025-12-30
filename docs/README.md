# Botho Documentation

Welcome to the Botho documentation. Botho is a privacy-preserving, mined cryptocurrency built on proven cryptographic foundations.

## Quick Links

| Document | Description |
|----------|-------------|
| [Getting Started](getting-started.md) | Build, install, and run your first node |
| [Architecture](architecture.md) | System design and component overview |
| [Configuration](configuration.md) | Complete configuration reference |
| [Mining](mining.md) | Mining setup and guide |
| [Tokenomics](tokenomics.md) | Supply, emission, fees, and economics |
| [Privacy](privacy.md) | Privacy features and cryptography |

## What is Botho?

Botho combines:

- **Proof-of-Work Mining**: SHA-256 mining with variable difficulty
- **Full Transaction Privacy**: Stealth addresses, ring signatures (MLSAG), and encrypted memos
- **Post-Quantum Security**: Hybrid classical + ML-KEM/ML-DSA cryptography
- **Byzantine Fault Tolerance**: Stellar Consensus Protocol (SCP) for consensus
- **Progressive Fees**: Cluster-based taxation that discourages wealth concentration

The native currency unit is the **credit** (BTH).

## Quick Start

```bash
# Build
cargo build --release

# Initialize wallet
botho init

# Run node
botho run

# Run with mining
botho run --mine
```

## Commands

| Command | Description |
|---------|-------------|
| `botho init` | Create wallet with 24-word mnemonic |
| `botho init --recover` | Recover wallet from existing mnemonic |
| `botho run` | Sync blockchain and scan wallet |
| `botho run --mine` | Run with mining enabled |
| `botho status` | Show sync status, balance, mining stats |
| `botho balance` | Show wallet balance |
| `botho address` | Show receiving address |
| `botho send <addr> <amt>` | Send credits |

## Project Status

Botho is in active development. See the main [README](../README.md) for current status and the [PLAN.md](../PLAN.md) for implementation details.

## Contributing

See [CONTRIBUTING.md](../CONTRIBUTING.md) for contribution guidelines.

## License

See the [LICENSE](../LICENSE) file for details.
