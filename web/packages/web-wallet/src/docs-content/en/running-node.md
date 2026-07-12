## Running a Botho Node

Running your own Botho node gives you the highest level of privacy and helps strengthen the network. When you run a node, your wallet connects directly to the blockchain without relying on third-party servers.

### Why Run a Node?

**Privacy:** When you use a light wallet or web wallet, you're trusting a server not to log your addresses or transactions. Running your own node means your wallet activity stays on your machine.

**Verification:** Your node independently validates every transaction and block. You don't have to trust anyone's claims about the state of the network.

**Network health:** More nodes make the network more resilient. Your node relays transactions and blocks, helping the network function.

**Minting advantage:** Running your own node gives you lower latency in the minting competition. Nodes that receive new blocks faster can begin working on the next block sooner, increasing their chances of earning minting rewards.

**Participation:** If you want to mint new blocks or participate in consensus, you need a full node.

### System Requirements

**Minimum Requirements:**
- 2 CPU cores
- 4 GB RAM
- 50 GB SSD storage
- 10 Mbps internet connection

**Recommended:**
- 4+ CPU cores
- 8 GB RAM
- 100 GB NVMe SSD
- 100 Mbps internet connection

The blockchain is currently small, but storage requirements will grow over time.

### Installation

**From Source (Recommended):**

```bash
# Install Rust if you haven't already
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Clone the repository
git clone https://github.com/botho-project/botho.git
cd botho

# Build in release mode
cargo build --release

# The binary is at ./target/release/botho
```

**First-Time Setup:**

```bash
# Initialize a new wallet and configuration
./target/release/botho init

# This will:
# - Generate a 24-word recovery phrase
# - Create your config and wallet under ~/.botho/

# Variants:
#   botho init --recover   # restore a wallet from an existing mnemonic
#   botho init --relay     # relay/seed node config with no wallet
```

**IMPORTANT:** Write down your recovery phrase and store it securely!

### Running the Node

**Basic operation:**

```bash
# Start the node and sync with the network
./target/release/botho run
```

**With minting enabled:**

```bash
# Start the node and participate in block production
./target/release/botho run --mint
```

### CLI Commands Reference

**Wallet Commands:**

| Command | Description |
|---------|-------------|
| `botho init` | Create a new wallet with a 24-word mnemonic |
| `botho balance` | Show your current wallet balance |
| `botho address` | Display your receiving address (`--save` writes it to a file) |
| `botho send <address> <amount>` | Send BTH (amount in BTH; `--quantum` for post-quantum crypto, `--memo` to attach an encrypted note) |

All sends use CLSAG ring signatures — sender privacy is on by default, not a flag.

**Node Commands:**

| Command | Description |
|---------|-------------|
| `botho run` | Start the node and sync with the network |
| `botho run --mint` | Start with minting enabled (`--mint-threads N` to limit CPU use) |
| `botho status` | Show node sync status and peer count |
| `botho snapshot` | Manage UTXO snapshots for fast initial sync |

### Configuration

The configuration file lives under `~/.botho/`. All ports have network-specific defaults, so a minimal config works out of the box:

```toml
# "mainnet" or "testnet"
network_type = "testnet"

[network]
# Defaults: gossip 7100 (mainnet) / 17100 (testnet)
#           RPC    7101 (mainnet) / 17101 (testnet)
#           metrics 9090 (mainnet) / 19090 (testnet), 0 disables
# gossip_port = 17100
# rpc_port = 17101
# metrics_port = 19090

# Optional explicit bootstrap peers (multiaddr format).
# If unset, peers are discovered via DNS seed TXT records
# (seeds.botho.io / seeds.testnet.botho.io).
# bootstrap_peers = ["/dns4/eu.seed.botho.io/tcp/7100/p2p/<peer-id>"]

[minting]
enabled = false
threads = 0   # 0 = use all CPU cores
```

### Firewall Configuration

If you want your node to accept incoming connections (recommended):

```bash
# Allow P2P gossip traffic (17100 on testnet, 7100 on mainnet)
sudo ufw allow 17100/tcp

# Optional: Allow RPC access (only if needed externally)
# sudo ufw allow 17101/tcp
```

### Troubleshooting

**Node won't sync:**
- Check your internet connection
- Verify firewall allows outbound connections on the gossip port
- As a last resort, clear the chain database under `~/.botho/` and resync (testnet only — this rescans from genesis)

**High memory usage:**
- Reduce minting threads (RandomX keeps a large in-memory dataset)
- Consider adding swap space if RAM is limited

**Can't connect to peers:**
- Ensure your gossip port (17100 testnet / 7100 mainnet) is open for incoming connections
- Check if you're behind a strict NAT
