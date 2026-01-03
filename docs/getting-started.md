# Getting Started

This guide walks you through building Botho, creating a wallet, and running a node.

## Prerequisites

- **Rust** (1.83.0 or later)
- **Cargo** (comes with Rust)

Install Rust via [rustup](https://rustup.rs/):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

## Building

Clone the repository and build:

```bash
git clone https://github.com/botho-project/botho.git
cd botho
cargo build --release
```

The binary will be at `target/release/botho`.

For development builds:

```bash
cargo build
```

## Creating a Wallet

Initialize a new wallet with a 24-word mnemonic:

```bash
botho init
```

This creates:
- `~/.botho/config.toml` - Configuration file containing your mnemonic
- `~/.botho/ledger.db/` - Blockchain database directory

**Important:** Back up your mnemonic phrase securely. It's the only way to recover your wallet.

### Recovering an Existing Wallet

To recover a wallet from an existing mnemonic:

```bash
botho init --recover
```

You'll be prompted to enter your 24-word mnemonic.

## Running a Node

Start the node to sync the blockchain and participate in the network:

```bash
botho run
```

The node will:
1. Connect to bootstrap peers
2. Sync the blockchain
3. Scan for transactions belonging to your wallet
4. Listen for new blocks and transactions

### Running with Minting

To run the node with minting enabled:

```bash
botho run --mint
```

Minting requires a satisfiable quorum (at least one other peer). Solo minting is not possible by design.

## Basic Commands

### Check Your Balance

```bash
botho balance
```

Shows your total balance and UTXO count.

### Get Your Receiving Address

```bash
botho address
```

Displays your public address for receiving funds.

### Send Funds

```bash
botho send <recipient_address> <amount>
```

Creates a transaction and saves it to pending. The transaction will be broadcast when the node is running.

### Check Node Status

```bash
botho status
```

Shows:
- Sync status (current height vs network height)
- Wallet balance
- Minting statistics (if minting)
- Connected peers

## Data Directory

All Botho data is stored in `~/.botho/`:

```
~/.botho/
├── config.toml      # Configuration + wallet mnemonic
├── ledger.db/       # Blockchain (LMDB)
│   ├── data.mdb
│   └── lock.mdb
└── pending_txs.bin  # Pending transactions
```

## Next Steps

- [Configuration Reference](operations/configuration.md) - Customize your node settings
- [Minting Guide](minting.md) - Learn about minting economics and setup
- [API Reference](api.md) - JSON-RPC and WebSocket API documentation
- [Architecture](concepts/architecture.md) - Understand how Botho works
- [Troubleshooting](operations/troubleshooting.md) - Common issues and solutions
