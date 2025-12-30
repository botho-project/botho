# Botho

*"Motho ke motho ka batho"* — A person is a person through other people.

**A global currency designed for privacy, fairness, and the long term.**

## The Name

**Botho** (pronounced BOH-toh) comes from the Sesotho and Setswana languages of Southern Africa, meaning *humanity*, *humaneness*, or *ubuntu*. It is a national principle of Botswana and a core philosophy across many African cultures.

The opening proverb—*"Motho ke motho ka batho"*—translates to "a person is a person through other people." It expresses the idea that our humanity is defined by our relationships and responsibilities to one another, not by individual accumulation.

In currency design, this philosophy rejects the "number go up" mentality. Instead, Botho asks: *how can money serve community rather than concentrate power?*

## Why Botho?

The world needs a digital currency that works for everyone—not just early adopters and whales. Botho is built on three principles that set it apart:

### Privacy as a Human Right

Every transaction is private by default. No surveillance, no tracking, no exceptions.

- **Stealth addresses** ensure recipients can't be linked across transactions
- **Confidential amounts** hide transaction values from observers
- **Post-quantum ready** with ML-KEM-768 and ML-DSA-65 hybrid cryptography

Unlike "transparent by default" cryptocurrencies, Botho treats financial privacy the way cash does—as the baseline, not a premium feature.

### Anti-Hoarding Economics

Most cryptocurrencies reward early accumulators and punish late adopters. Botho inverts this with **progressive transaction fees** based on wealth concentration:

| Wealth Level | Fee Rate |
|:--|:--|
| Widely circulated coins | 0.05% |
| Moderately concentrated | 1-5% |
| Heavily hoarded | up to 30% |

The system tracks coin *ancestry*, not identities. Coins that circulate through the economy pay less; coins that sit in whale wallets pay more. This is Sybil-resistant—splitting your wallet doesn't help because fees are based on where coins came from, not where they are now.

### Sustainable Security

Bitcoin's security budget will eventually depend entirely on transaction fees. Botho ensures permanent security through **perpetual tail emission**:

- Initial reward: 50 BTH per block
- ~2-year halving schedule (5 halvings over 10 years)
- Perpetual tail emission: ~1.59 BTH per block targeting 2% annual inflation

This guarantees minters are always incentivized to secure the network, without relying on ever-increasing transaction volume.

## Technical Foundation

Botho combines proven cryptographic building blocks in a novel architecture:

| Component | Technology | Benefit |
|:--|:--|:--|
| Consensus | Stellar Consensus Protocol (SCP) | 3-5 second finality, Byzantine fault tolerance |
| Minting | Parallel proof-of-work | Fair block selection, not fastest-propagation-wins |
| Privacy | CryptoNote stealth addresses | Unlinkable transactions |
| Quantum safety | ML-KEM-768 + ML-DSA-65 | Future-proof key exchange and signatures |
| Fee system | Cluster-tagged progressive fees | Economic equality without identity |

### Fast Finality

Unlike Bitcoin's probabilistic finality (wait 6 blocks = 60 minutes to be "sure"), Botho transactions are final in seconds. The SCP quorum reaches consensus, and that's it—no reorgs, no double-spend risk.

### Block Parameters

| Parameter | Value |
|:--|:--|
| Block time | 20 seconds |
| Difficulty adjustment | Every 1,440 blocks (~8 hours) |
| Phase 1 supply | ~100 million BTH (10 years of halvings) |
| Tail emission | ~1.59 BTH/block (2% net annual inflation) |
| Native unit | BTH (9 decimal places) |

## Philosophy in Practice

The Botho philosophy manifests in every design decision:

- **Decentralized consensus** — decisions require community agreement
- **Privacy** — respecting individual dignity
- **Progressive fees** — prioritizing circulation over accumulation
- **Tail emission** — sustainable security for future generations

## Getting Started

### Run a Node

```bash
# Clone and build
git clone https://github.com/botho-project/botho.git
cd botho
cargo build --release

# Initialize wallet (generates 24-word mnemonic)
./target/release/botho init

# Start node
./target/release/botho run

# Start node with minting
./target/release/botho run --mint
```

### CLI Commands

| Command | Description |
|:--|:--|
| `botho init` | Create wallet with recovery phrase |
| `botho run` | Start node and sync blockchain |
| `botho run --mint` | Start node with minting enabled |
| `botho status` | Show sync and wallet status |
| `botho balance` | Show wallet balance |
| `botho address` | Show receiving address |
| `botho send <addr> <amt>` | Send BTH |

### Web Wallet

Visit [botho.io](https://botho.io) to use the web wallet without running a node.

## Documentation

| Document | Description |
|:--|:--|
| [Getting Started](./docs/getting-started.md) | Build, install, and run your first node |
| [FAQ](./docs/FAQ.md) | Frequently asked questions |
| [Why Botho?](./docs/comparison.md) | Comparison with Bitcoin, Monero, Zcash |
| [Architecture](./docs/architecture.md) | System design and component overview |
| [Tokenomics](./docs/tokenomics.md) | Emission schedule, fees, and supply |
| [Monetary Policy](./docs/monetary-policy.md) | Difficulty adjustment, epochs, and upgrades |
| [Minting](./docs/minting.md) | Mining setup and economics |
| [Privacy](./docs/privacy.md) | Privacy features and cryptography |
| [Configuration](./docs/configuration.md) | Node configuration options |
| [API Reference](./docs/api.md) | JSON-RPC and WebSocket API |
| [Developer Guide](./docs/developer-guide.md) | Build applications with Botho |
| [Testing](./docs/testing.md) | Run and write tests |
| [Troubleshooting](./docs/troubleshooting.md) | Common issues and solutions |
| [Glossary](./docs/glossary.md) | Technical terms explained |

## Project Status

Botho is in active development. Current focus areas:

- Core node implementation with SCP consensus
- Stealth address transaction privacy
- Progressive fee mechanism
- Web and desktop wallet applications

### Origins

Botho is derived from [MobileCoin](https://github.com/mobilecoinfoundation/mobilecoin), with significant simplifications. We removed SGX enclaves, Fog, and mobile-specific infrastructure to focus on a clean, auditable implementation for desktop and server environments.

## Repository Structure

| Directory | Description |
|:--|:--|
| [botho](./botho) | Main node binary with RPC server |
| [botho-wallet](./botho-wallet) | CLI wallet implementation |
| [cluster-tax](./cluster-tax) | Progressive fee mechanism and monetary policy |
| [consensus/scp](./consensus/scp) | Stellar Consensus Protocol implementation |
| [crypto](./crypto) | Cryptographic primitives (ring signatures, keys) |
| [transaction](./transaction) | Private transaction construction and signing |
| [ledger](./ledger) | Blockchain state and LMDB storage |
| [gossip](./gossip) | libp2p networking and peer discovery |
| [web](./web) | Web wallet, landing page, and UI components |

## Links

- **Website**: [botho.io](https://botho.io)
- **Documentation**: [botho.io/docs](https://botho.io/docs)

## License

See the [LICENSE](./LICENSE) file for details.

## Cryptography Notice

This software includes cryptographic components. Check your local laws regarding the use of cryptographic software before downloading or using.
