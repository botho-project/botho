# Botho

*"Motho ke motho ka batho"* — A person is a person through other people.

A privacy-preserving, mined cryptocurrency built on proven cryptographic foundations.

## What is Botho?

**Botho** (Sesotho/Setswana: humanity, humaneness) is a cryptocurrency that embodies the African philosophy of interconnectedness—the idea that we exist through our relationships with others. Just as the Botho philosophy emphasizes community over individualism, this currency is designed for collective benefit rather than concentrated wealth.

Botho combines:

- **Proof-of-Work Mining**: Bitcoin-style mining with variable difficulty
- **Full Transaction Privacy**: Stealth addresses, ring signatures, and confidential transactions
- **Anti-Inequality Design**: Progressive fees that discourage wealth concentration
- **Simple Design**: Focused on the essentials, removing unnecessary complexity

The native currency unit is the **credit** (symbol: **BTH**).

## Philosophy

Botho is a national principle of Botswana, describing "a person who has a well-rounded character, who is well-mannered, courteous and disciplined, and realises his or her full potential."

In the context of cryptocurrency:
- **Decentralized consensus** reflects "a person is a person through other people"
- **Privacy** respects individual dignity
- **Progressive fees** prioritize community over accumulation
- **Tail emission** ensures sustainable, shared security

## How Mining Works

Botho uses a **parallel proof-of-work** mechanism integrated with Stellar Consensus Protocol (SCP) for Byzantine fault tolerance.

### The Mining Process

1. **Find a Valid Nonce**: Miners search for a nonce that produces a hash below the difficulty target:
   ```
   SHA256(nonce || prev_block_hash || miner_address) < difficulty_target
   ```

2. **Submit Mining Transaction**: Valid proofs are wrapped in a `MiningTx` and submitted to the consensus network

3. **SCP Decides the Winner**: Multiple miners may find valid solutions simultaneously—the SCP quorum determines which block is accepted, providing Byzantine fault tolerance

### Why Parallel Mining?

Unlike Bitcoin where the first valid block to propagate "wins," Botho separates proof-of-work from block selection:

- **Multiple Valid Solutions**: Any miner who finds a valid nonce can submit a mining transaction
- **Consensus-Based Selection**: The SCP quorum (not network propagation speed) determines which miner's block is included
- **Byzantine Fault Tolerance**: Even if some nodes are malicious or offline, consensus proceeds correctly
- **Fair Selection**: Network latency doesn't determine the winner—the quorum does

### Emission Schedule

Mining rewards follow a smooth decay curve with perpetual tail emission:

| Parameter | Value |
| :-- | :-- |
| Initial reward | 50 BTH |
| Halving period | ~6,307,200 blocks (~4 years at 20-sec blocks) |
| Tail emission | 0.6 BTH per block (perpetual) |
| Total supply | ~18 million BTH (pre-tail) |

### Difficulty Adjustment

- **Target block time**: 20 seconds
- **Adjustment window**: Every 10 blocks
- **Smooth adjustment**: Prevents large difficulty jumps (max 4x change per adjustment)

## Privacy Features

Botho inherits battle-tested privacy technology from the CryptoNote protocol:

- **Stealth Addresses**: Each transaction creates a unique one-time destination address
- **Ring Signatures**: Hide the sender among a group of possible signers (planned)
- **RingCT (Ring Confidential Transactions)**: Amounts are cryptographically hidden (planned)

These features ensure that transactions cannot be traced or linked, providing cash-like privacy.

## Progressive Transaction Fees

Botho implements a novel **cluster-based progressive fee** system designed to reduce wealth concentration without sacrificing privacy or enabling Sybil attacks.

### How It Works

Transaction fees are based on coin *ancestry*, not account identity:

1. **Clusters**: Each coin-creation event (mining reward) spawns a new "cluster" identity
2. **Tag Vectors**: Every account carries a sparse vector of weights indicating what fraction of its coins trace back to each cluster origin
3. **Cluster Wealth**: The total value in the system tagged to a given cluster (W = Σ balance × tag_weight)
4. **Progressive Fees**: Fee rate increases with cluster wealth via a sigmoid curve—larger clusters pay higher rates

```
Fee Rate = sigmoid(cluster_wealth) → ranges from 0.05% to 30%
```

### Why It's Sybil-Resistant

Splitting transactions or creating multiple accounts doesn't reduce fees because:
- Fee rate depends on **cluster wealth**, not transaction size or account count
- All accounts holding coins from the same mining origin pay the same rate
- The only way to reduce fees is through genuine economic activity that diffuses coins across the economy

### Tag Decay

Tags decay by ~5% per transaction hop, gradually converting cluster attribution into "background" (fully diffused) wealth. This means:
- Coins that circulate widely pay lower fees over time
- Hoarded coins retain high cluster attribution and pay higher fees
- ~14 transaction hops to halve a tag's weight

### Default Parameters

| Parameter | Value | Description |
| :-- | :-- | :-- |
| Minimum fee | 0.05% | Small/diffused clusters |
| Maximum fee | 30% | Large concentrated clusters |
| Decay rate | 5% per hop | Tag decay per transaction |
| Midpoint | 10M credits | Sigmoid inflection point |

## Project Status

This project is in early development. We are actively:

- Implementing the mining mechanism
- Simplifying the codebase (removing SGX dependencies, Fog, and other MobileCoin-specific components)
- Developing and validating the progressive fee mechanism through economic simulation

### Simplifications from MobileCoin

By removing SGX enclaves, we eliminate the need for:

- **Oblivious database access patterns**: MobileCoin used ORAM and other techniques to hide which records were accessed inside enclaves. Without SGX, standard database access is fine.
- **Remote attestation**: No need for Intel attestation infrastructure or verification.
- **Sealed storage**: Encryption keys can use standard key management instead of SGX sealing.

This allows significant code simplification throughout the codebase.

## Building

The workspace can be built with `cargo build` and tested with `cargo test`.

```bash
cargo build --release
cargo test
```

## Documentation

Detailed documentation is available in the [docs](./docs) directory:

| Document | Description |
| :-- | :-- |
| [Getting Started](./docs/getting-started.md) | Build, install, and run your first node |
| [Architecture](./docs/architecture.md) | System design and component overview |
| [Configuration](./docs/configuration.md) | Complete configuration reference |
| [Mining](./docs/mining.md) | Mining setup, economics, and troubleshooting |
| [Privacy](./docs/privacy.md) | Privacy features and cryptography |

## Repository Structure

| Directory | Description |
| :-- | :-- |
| [cluster-tax](./cluster-tax) | Progressive fee mechanism and economic simulation |
| [common](./common) | Shared utilities and types |
| [consensus](./consensus) | Block validation and consensus |
| [crypto](./crypto) | Cryptographic primitives |
| [ledger](./ledger) | Blockchain storage |
| [transaction](./transaction) | Private transaction construction |
| [util](./util) | Miscellaneous utilities |

## Origins

Botho is derived from [MobileCoin](https://github.com/mobilecoinfoundation/mobilecoin), with significant simplifications to focus on mined, private transactions.

**Note**: Unlike MobileCoin, Botho is designed for desktop and server environments only. There are no plans for mobile device support. We have removed MobileCoin's SGX enclave infrastructure, Fog (the privacy-preserving mobile sync service), and other components designed for resource-constrained devices.

## Links

- **Website**: [botho.io](https://botho.io)

## License

See the LICENSE file for details.

## Cryptography Notice

This software includes cryptographic components. Check your local laws regarding the use of cryptographic software before downloading or using.
