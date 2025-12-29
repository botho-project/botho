# Cadence

A privacy-preserving, mined cryptocurrency built on proven cryptographic foundations.

## What is Cadence?

Cadence is a new cryptocurrency that combines:

- **Proof-of-Work Mining**: Bitcoin-style mining with variable difficulty
- **Full Transaction Privacy**: Ring signatures, one-time addresses, and confidential transactions
- **Simple Design**: Focused on the essentials, removing unnecessary complexity

The native currency unit is the **credit**.

## How Mining Works

Cadence uses a **parallel proof-of-work** mechanism integrated with Stellar Consensus Protocol (SCP) for Byzantine fault tolerance.

### The Mining Process

1. **Find a Valid Nonce**: Miners search for a nonce that produces a hash below the difficulty target:
   ```
   SHA256(nonce || prev_block_hash || miner_address) < difficulty_target
   ```

2. **Submit Mining Transaction**: Valid proofs are wrapped in a `MiningTx` and submitted to the consensus network

3. **SCP Decides the Winner**: Multiple miners may find valid solutions simultaneously—the SCP quorum determines which block is accepted, providing Byzantine fault tolerance

### Why Parallel Mining?

Unlike Bitcoin where the first valid block to propagate "wins," Cadence separates proof-of-work from block selection:

- **Multiple Valid Solutions**: Any miner who finds a valid nonce can submit a mining transaction
- **Consensus-Based Selection**: The SCP quorum (not network propagation speed) determines which miner's block is included
- **Byzantine Fault Tolerance**: Even if some nodes are malicious or offline, consensus proceeds correctly
- **Fair Selection**: Network latency doesn't determine the winner—the quorum does

### Emission Schedule

Mining rewards follow a smooth decay curve with perpetual tail emission:

| Parameter | Value |
| :-- | :-- |
| Initial reward | 50 CAD |
| Halving period | ~6,307,200 blocks (~4 years at 20-sec blocks) |
| Tail emission | 0.6 CAD per block (perpetual) |
| Total supply | 21 million CAD |

### Difficulty Adjustment

- **Target block time**: 20 seconds
- **Adjustment window**: Every 10 blocks
- **Smooth adjustment**: Prevents large difficulty jumps (max 4x change per adjustment)

## Privacy Features

Cadence inherits battle-tested privacy technology from the CryptoNote protocol:

- **Ring Signatures**: Hide the sender among a group of possible signers
- **One-Time Addresses**: Each transaction creates a unique destination address
- **RingCT (Ring Confidential Transactions)**: Amounts are cryptographically hidden

These features ensure that transactions cannot be traced or linked, providing cash-like privacy.

## Progressive Transaction Fees

Cadence implements a novel **cluster-based progressive fee** system designed to reduce wealth concentration without sacrificing privacy or enabling Sybil attacks.

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

## Building

The workspace can be built with `cargo build` and tested with `cargo test`.

```bash
cargo build --release
cargo test
```

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

Cadence is derived from [MobileCoin](https://github.com/mobilecoinfoundation/mobilecoin), with significant simplifications to focus on mined, private transactions.

**Note**: Unlike MobileCoin, Cadence is designed for desktop and server environments only. There are no plans for mobile device support. We have removed MobileCoin's SGX enclave infrastructure, Fog (the privacy-preserving mobile sync service), and other components designed for resource-constrained devices.

## License

See the LICENSE file for details.

## Cryptography Notice

This software includes cryptographic components. Check your local laws regarding the use of cryptographic software before downloading or using.
