import { useState } from 'react'
import { Link, useLocation } from 'react-router-dom'
import { Logo } from '@botho/ui'
import { ArrowLeft, Book, Code, Shield, Zap, Globe, Terminal, Menu, X, Coins } from 'lucide-react'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'

const sections = [
  {
    id: 'getting-started',
    title: 'Getting Started',
    icon: Book,
    content: `
## Getting Started with Botho

Botho is a privacy-focused cryptocurrency designed for the post-quantum era. It combines **stealth addresses** for transaction privacy with the **Stellar Consensus Protocol (SCP)** for fast, energy-efficient consensus. Unlike proof-of-work cryptocurrencies, Botho achieves finality in seconds while maintaining strong privacy guarantees.

### What Makes Botho Different?

Traditional cryptocurrencies like Bitcoin have transparent blockchains where anyone can trace the flow of funds between addresses. Even "privacy coins" often rely on cryptographic assumptions that may be broken by future quantum computers.

Botho takes a different approach:

- **Stealth addresses** ensure that each payment you receive goes to a unique one-time address, making it impossible to link your transactions together by watching the blockchain
- **Optional post-quantum cryptography** protects your privacy against adversaries with quantum computers
- **Federated Byzantine Agreement** provides fast finality without energy-intensive mining
- **Fee burning** creates a deflationary monetary policy where transaction fees are permanently removed from circulation

### Creating a Wallet

Getting started with Botho takes just a few steps:

1. **Visit the Wallet page** - Click "Launch Wallet" from the homepage or navigate directly to the wallet
2. **Choose "Create New Wallet"** - You can also import an existing wallet if you have a recovery phrase
3. **Secure your recovery phrase** - You'll be shown a 12-word mnemonic phrase. Write this down on paper and store it in a safe place. This phrase is the **only way** to recover your funds if you lose access to your device
4. **Optional: Set a password** - Add an encryption password for additional security. You'll need this password each time you open the wallet in this browser

**Important:** Never share your recovery phrase with anyone. Anyone with these words can access your funds. Never store it digitally (no screenshots, no cloud storage, no password managers).

### Understanding Your Wallet Address

Your wallet address looks like this: \`botho://1/B62qk4nuKn2U5qsR...\`

This address format includes:
- **Protocol identifier** (\`botho://\`) - Identifies this as a Botho address
- **Network version** (\`1/\`) - Indicates the mainnet (testnet uses different versions)
- **Public key** - Your stealth address public key encoded in base58

You can safely share this address with anyone who wants to send you funds. Thanks to stealth addresses, each incoming transaction will be sent to a unique derived address that only you can spend from.

### Receiving Your First Payment

When someone sends you Botho credits:

1. They use your public address to derive a unique one-time address
2. The transaction is broadcast to the network and included in a block
3. Your wallet scans new blocks and detects payments addressed to you
4. The funds appear in your balance, typically within 20-30 seconds

### Sending Payments

To send credits to someone else:

1. Click the **Send** button in your wallet
2. Enter the recipient's Botho address
3. Enter the amount to send
4. Review the transaction details including the fee
5. Confirm the transaction

Transactions are final once confirmed—there are no chargebacks or reversals in Botho.

### Transaction Fees

Every Botho transaction requires a small fee. These fees serve two purposes:

1. **Spam prevention** - Fees make it expensive to flood the network with junk transactions
2. **Deflationary pressure** - All fees are permanently burned, reducing the total supply over time

The minimum fee is currently 1 microcredit (0.000001 BTH), though you can include a higher fee for priority during periods of high network activity.

### Security Best Practices

- **Back up your recovery phrase** on paper, stored in a secure location
- **Use a password** to encrypt your wallet in the browser
- **Consider running your own node** for maximum privacy
- **Verify addresses carefully** before sending funds—transactions cannot be reversed
    `,
  },
  {
    id: 'privacy',
    title: 'Privacy Features',
    icon: Shield,
    content: `
## Privacy Features

Privacy is not just a feature in Botho—it's a fundamental design principle. Every aspect of the protocol is designed to protect your financial privacy while maintaining the auditability properties needed for a sound monetary system.

### Why Privacy Matters

Financial privacy is essential for:

- **Personal security** - Public wealth makes you a target for criminals
- **Business confidentiality** - Competitors shouldn't see your supplier payments or revenue
- **Fungibility** - Money should be interchangeable; tainted coins create a two-tier system
- **Human dignity** - Your financial life is nobody's business but your own

### Stealth Addresses

Stealth addresses are the foundation of Botho's privacy model. Here's how they work:

**The Problem:** In Bitcoin, if you publish an address to receive donations, anyone can see every donation you've ever received by looking at that address on the blockchain.

**The Solution:** In Botho, your public address is not where funds are actually sent. Instead, each sender uses your public address to mathematically derive a unique one-time address. Only you can detect and spend from these derived addresses.

**Technical Details:**

1. Your wallet has a **view keypair** and a **spend keypair**
2. The sender generates a random value and combines it with your public keys
3. This produces a one-time address that appears random to everyone else
4. Your wallet uses your private view key to scan for payments addressed to you
5. To spend, you use your private spend key to sign the transaction

The result: Even if you publish your address publicly, no one watching the blockchain can determine how many payments you've received, when you received them, or how much they were for.

### Ring Signatures (Private Transactions)

When you choose a **Private transaction**, Botho uses **LION ring signatures** to hide which specific coins you're spending. Your transaction references 7 possible inputs (a "ring"), and the signature proves you own one of them without revealing which one.

LION (Lattice-based lInkable ring signatures fOr aNonymity) is a post-quantum ring signature scheme that provides both sender privacy AND quantum resistance.

This breaks the transaction graph that would otherwise allow tracing funds through the blockchain. An observer sees that *someone* in the ring spent *some* coins, but cannot determine which participant or which specific coins.

> **Note:** Standard transactions don't use ring signatures—the sender is visible but amounts remain hidden. Choose Private transactions when you need sender anonymity.

### Confidential Amounts

In **Standard** and **Private** transactions, amounts are hidden using **Pedersen commitments** with **Bulletproofs** range proofs. These cryptographic constructs allow the network to verify that transactions balance (inputs equal outputs plus fees) without revealing the actual amounts.

Validators can confirm:
- No new money is created from thin air
- The sender has sufficient funds
- The fee is at least the minimum required
- All amounts are positive (via Bulletproofs)

But they cannot determine:
- How much is being transferred
- The sender's total balance
- The recipient's total balance

> **Note:** Minting transactions (block rewards) have public amounts for supply auditability, but recipients are still hidden via stealth addresses.

### Post-Quantum Cryptography

Quantum computers pose a future threat to the cryptographic algorithms that secure most cryptocurrencies today. Botho is designed from the ground up with **pure post-quantum security**—all transactions use quantum-resistant algorithms by default.

**Algorithms Used:**

- **ML-KEM-768** (FIPS 203) - Post-quantum key encapsulation for stealth addresses (all transactions)
- **ML-DSA-65** (FIPS 204) - Post-quantum signatures for Standard and Minting transactions
- **LION** - Lattice-based ring signatures for Private transactions (~128-bit PQ security)

These algorithms were standardized by NIST after years of rigorous cryptanalysis.

**Transaction Types:**

| Type | Recipient | Amount | Sender | Use Case |
|------|-----------|--------|--------|----------|
| Minting | Hidden | Public | Known | Block rewards |
| Standard | Hidden | Hidden | Visible | Most transfers (~3-4 KB) |
| Private | Hidden | Hidden | Hidden | Maximum privacy (~22 KB) |

All transactions are quantum-safe from day one—no need to choose a "PQ mode."

### Privacy Best Practices

To maximize your privacy when using Botho:

1. **Run your own node** - This prevents revealing your addresses to third-party servers
2. **Use a new address for each context** - While stealth addresses protect received funds, using separate addresses for work vs personal adds another layer
3. **Enable post-quantum mode for savings** - Protect against harvest-now-decrypt-later attacks
4. **Be mindful of metadata** - Privacy on-chain doesn't help if you reveal information off-chain
    `,
  },
  {
    id: 'consensus',
    title: 'Consensus',
    icon: Zap,
    content: `
## Stellar Consensus Protocol

Botho uses the **Stellar Consensus Protocol (SCP)** for distributed consensus. SCP is a federated Byzantine agreement protocol that provides fast finality, energy efficiency, and flexible trust—without sacrificing decentralization.

### Why Not Proof-of-Work?

Proof-of-work (PoW) consensus, as used in Bitcoin, has significant drawbacks:

- **Energy waste** - PoW deliberately consumes massive amounts of electricity as a security mechanism
- **Slow finality** - Bitcoin transactions aren't truly final for an hour or more
- **Centralization pressure** - Mining economies of scale push toward industrial operations
- **51% attacks** - If an attacker controls majority hashpower, they can rewrite history

### Why Not Proof-of-Stake?

Proof-of-stake (PoS) improves on energy usage but introduces its own issues:

- **Nothing-at-stake** - Validators can cheaply vote on multiple chain forks
- **Wealth concentration** - The rich get richer through staking rewards
- **Long-range attacks** - Old keys can potentially rewrite history
- **Complexity** - PoS systems require intricate slashing and validator selection logic

### How SCP Works

SCP takes a fundamentally different approach based on **federated voting**:

**Quorum Slices:** Each node in the network defines its own "quorum slice"—a set of other nodes it trusts. A node will only accept a statement as final when its quorum slice agrees.

**Quorum Intersection:** The network is secure as long as all quorum slices share some nodes in common. This ensures that two conflicting statements cannot both achieve consensus.

**Federated Voting:** Consensus proceeds through a series of voting rounds:

1. **Nominate** - Nodes propose candidate values for the next block
2. **Prepare** - Nodes vote to prepare a specific value
3. **Commit** - Nodes vote to commit the prepared value
4. **Externalize** - Once committed, the value is final

**Key Insight:** Unlike PoW where you trust "the longest chain," in SCP you explicitly choose which nodes to trust. This makes the trust model transparent and auditable.

### Properties of SCP

**Decentralized Control:** No central authority determines consensus. Each node independently chooses its quorum slice based on its own assessment of trustworthiness.

**Low Latency:** Transactions reach finality in seconds (typically 3-5 seconds under normal conditions), compared to minutes or hours for PoW systems.

**Flexible Trust:** Participants can choose different quorum configurations based on their needs. Some may trust established institutions; others may trust a set of technical experts.

**Asymptotic Security:** As the network grows and quorum slices become more interconnected, the system becomes more resilient against Byzantine failures.

**Energy Efficiency:** SCP nodes only need to exchange messages and verify signatures—no computational puzzles, no energy waste.

### Safety vs. Liveness

SCP prioritizes **safety** over **liveness**:

- **Safety:** The network will never confirm conflicting transactions
- **Liveness:** The network should eventually make progress

If the quorum structure is disrupted (too many nodes go offline), SCP will halt rather than risk confirming conflicting transactions. This is the correct trade-off for a monetary system—it's better to pause than to have funds stolen.

### Quorum Configuration in Botho

The Botho network starts with a bootstrap quorum centered on the foundation's seed nodes. Over time, as more independent nodes join, the quorum structure will become increasingly decentralized.

Node operators can customize their quorum slice to trust:
- The foundation's seed nodes (default)
- Other known community nodes
- Nodes run by exchanges or businesses they trust
- Any combination of the above

The health of the network depends on sufficient quorum intersection. The Botho explorer shows real-time quorum topology to help operators make informed decisions.
    `,
  },
  {
    id: 'running-node',
    title: 'Running a Node',
    icon: Terminal,
    content: `
## Running a Botho Node

Running your own Botho node gives you the highest level of privacy and helps strengthen the network. When you run a node, your wallet connects directly to the blockchain without relying on third-party servers.

### Why Run a Node?

**Privacy:** When you use a light wallet or web wallet, you're trusting a server not to log your addresses or transactions. Running your own node means your wallet activity stays on your machine.

**Verification:** Your node independently validates every transaction and block. You don't have to trust anyone's claims about the state of the network.

**Network health:** More nodes make the network more resilient. Your node relays transactions and blocks, helping the network function.

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

\`\`\`bash
# Install Rust if you haven't already
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Clone the repository
git clone https://github.com/botho-project/botho.git
cd botho

# Build in release mode
cargo build --release

# The binary is at ./target/release/botho
\`\`\`

**First-Time Setup:**

\`\`\`bash
# Initialize a new wallet and configuration
./target/release/botho init

# This will:
# - Generate a 12-word recovery phrase
# - Create ~/.botho/config.toml
# - Create ~/.botho/wallet.db
\`\`\`

**IMPORTANT:** Write down your recovery phrase and store it securely!

### Running the Node

**Basic operation:**

\`\`\`bash
# Start the node and sync with the network
./target/release/botho run
\`\`\`

**With minting enabled:**

\`\`\`bash
# Start the node and participate in block production
./target/release/botho run --mint
\`\`\`

### CLI Commands Reference

**Wallet Commands:**

| Command | Description |
|---------|-------------|
| \`botho init\` | Create a new wallet with a 12-word mnemonic |
| \`botho balance\` | Show your current wallet balance |
| \`botho address\` | Display your receiving address |
| \`botho send <address> <amount>\` | Send credits to another address |

**Node Commands:**

| Command | Description |
|---------|-------------|
| \`botho run\` | Start the node and sync with the network |
| \`botho run --mint\` | Start with minting enabled |
| \`botho status\` | Show node sync status and peer count |

### Configuration

The configuration file is located at \`~/.botho/config.toml\`:

\`\`\`toml
[network]
# Seed nodes for initial peer discovery
seeds = ["seed.botho.io:8443"]

# Your node's listen address
listen = "0.0.0.0:8443"

[rpc]
# JSON-RPC API listen address
listen = "127.0.0.1:8080"

[minting]
# Enable block minting
enabled = false

# Number of CPU threads for minting
threads = 4
\`\`\`

### Firewall Configuration

If you want your node to accept incoming connections (recommended):

\`\`\`bash
# Allow P2P traffic
sudo ufw allow 8443/tcp

# Optional: Allow RPC access (only if needed externally)
# sudo ufw allow 8080/tcp
\`\`\`

### Troubleshooting

**Node won't sync:**
- Check your internet connection
- Verify firewall allows outbound connections on port 8443
- Try clearing the database: \`rm -rf ~/.botho/chain.db\`

**High memory usage:**
- Reduce the database cache size in config
- Consider adding swap space if RAM is limited

**Can't connect to peers:**
- Ensure port 8443 is open for incoming connections
- Check if you're behind a strict NAT
    `,
  },
  {
    id: 'api',
    title: 'API Reference',
    icon: Code,
    content: `
## JSON-RPC API

Botho nodes expose a JSON-RPC 2.0 API on port 8080. All requests use the standard JSON-RPC 2.0 format.

### Request Format

\`\`\`json
{
  "jsonrpc": "2.0",
  "method": "METHOD_NAME",
  "params": { ... },
  "id": 1
}
\`\`\`

---

## Node Methods

### node_getStatus

Get node status and sync information.

**Response:**
- \`version\` - Node software version
- \`network\` - Network name (e.g., "botho-mainnet")
- \`uptimeSeconds\` - Node uptime in seconds
- \`syncStatus\` - Current sync status
- \`chainHeight\` - Current blockchain height
- \`tipHash\` - Hash of the latest block
- \`peerCount\` - Number of connected peers
- \`mempoolSize\` - Transactions in mempool
- \`mintingActive\` - Whether minting is enabled

---

## Chain Methods

### getChainInfo

Get blockchain information.

**Response:**
- \`height\` - Current block height
- \`tipHash\` - Hash of the tip block
- \`difficulty\` - Current mining difficulty
- \`totalMined\` - Total coins mined
- \`mempoolSize\` - Number of pending transactions
- \`mempoolFees\` - Total fees in mempool

### getBlockByHeight

Get a block by its height.

**Parameters:**
- \`height\` (number) - Block height

**Response:**
- \`height\` - Block height
- \`hash\` - Block hash
- \`prevHash\` - Previous block hash
- \`timestamp\` - Block timestamp
- \`difficulty\` - Block difficulty
- \`nonce\` - Mining nonce
- \`txCount\` - Number of transactions
- \`mintingReward\` - Minting reward amount

### getMempoolInfo

Get mempool statistics.

**Response:**
- \`size\` - Number of transactions
- \`totalFees\` - Total fees from all transactions
- \`txHashes\` - Array of transaction hashes (up to 100)

### estimateFee

Estimate transaction fee.

**Parameters:**
- \`amount\` (number) - Transaction amount
- \`private\` (boolean) - Whether transaction uses privacy features (default: true)
- \`memos\` (number) - Number of memo fields

**Response:**
- \`minimumFee\` - Minimum required fee
- \`feeRateBps\` - Fee rate in basis points
- \`recommendedFee\` - Recommended fee for normal priority
- \`highPriorityFee\` - Fee for high priority confirmation

---

## Wallet Methods

### chain_getOutputs

Get transaction outputs for wallet sync.

**Parameters:**
- \`start_height\` (number) - Starting block height
- \`end_height\` (number) - Ending block height (max 100 blocks per request)

**Response:** Array of blocks, each containing:
- \`height\` - Block height
- \`outputs\` - Array of outputs with \`txHash\`, \`outputIndex\`, \`targetKey\`, \`publicKey\`, \`amountCommitment\`

### wallet_getBalance

Get wallet balance (requires local wallet).

**Response:**
- \`confirmed\` - Confirmed balance
- \`pending\` - Pending balance
- \`total\` - Total balance
- \`utxoCount\` - Number of unspent outputs

### wallet_getAddress

Get wallet keys and address info.

**Response:**
- \`viewKey\` - Public view key (hex)
- \`spendKey\` - Public spend key (hex)
- \`hasWallet\` - Whether node has a wallet configured

---

## Transaction Methods

### tx_submit / sendRawTransaction

Submit a signed transaction.

**Parameters:**
- \`tx_hex\` (string) - Hex-encoded serialized transaction

**Response:**
- \`txHash\` - Transaction hash

### pq_tx_submit

Submit a quantum-private transaction (requires PQ build).

**Parameters:**
- \`tx_hex\` (string) - Hex-encoded quantum-private transaction

**Response:**
- \`txHash\` - Transaction hash
- \`type\` - "quantum-private"
- \`size\` - Transaction size in bytes

---

## Minting Methods

### minting_getStatus

Get minting status.

**Response:**
- \`active\` - Whether minting is enabled
- \`threads\` - Number of minting threads
- \`hashrate\` - Current hashrate
- \`totalHashes\` - Total hashes computed
- \`blocksFound\` - Blocks mined by this node
- \`currentDifficulty\` - Current network difficulty
- \`uptimeSeconds\` - Minting uptime

---

## Network Methods

### network_getInfo

Get network connection information.

**Response:**
- \`peerCount\` - Total peer count
- \`inboundCount\` - Inbound connections
- \`outboundCount\` - Outbound connections
- \`bytesSent\` - Total bytes sent
- \`bytesReceived\` - Total bytes received
- \`uptimeSeconds\` - Connection uptime

### network_getPeers

Get list of connected peers.

**Response:**
- \`peers\` - Array of peer information
    `,
  },
  {
    id: 'network',
    title: 'Network',
    icon: Globe,
    content: `
## Network Information

This page provides technical details about the Botho network, including connection information, network parameters, and security model.

### Network Status

The Botho network is currently in **testnet** phase. This means:

- Coins have no monetary value
- The network may be reset during development
- Features are still being tested and refined
- Bug reports and feedback are welcome

Production mainnet launch will be announced when the network is stable.

### Connecting to the Network

**Seed Nodes:**

Seed nodes help your node discover other peers on the network:

| Address | Location | Status |
|---------|----------|--------|
| seed.botho.io:8443 | Primary | Active |

When your node starts, it connects to seed nodes to learn about other peers. After initial discovery, your node maintains connections to multiple peers for redundancy.

**Peer Discovery:**

Botho uses libp2p for networking, which supports multiple discovery mechanisms:

- **Bootstrap nodes** - Known seed nodes for initial connection
- **mDNS** - Local network discovery for development
- **Kademlia DHT** - Distributed peer discovery
- **Gossipsub** - Topic-based message propagation

### Network Parameters

**Block Production:**

| Parameter | Value | Description |
|-----------|-------|-------------|
| Target block time | 60 seconds | Average time between blocks |
| Max block size | 1 MB | Maximum serialized block size |
| Max transactions per block | 1000 | Transaction count limit |

**Transaction Limits:**

| Parameter | Value | Description |
|-----------|-------|-------------|
| Max inputs | 16 | Maximum inputs per transaction |
| Max outputs | 16 | Maximum outputs per transaction |
| Ring size | 7 | Number of members in LION ring signature |
| Max tx size | 100 KB | Maximum serialized transaction size |

**Fees:**

| Parameter | Value | Description |
|-----------|-------|-------------|
| Minimum fee | 1 nanoBTH | Floor fee for any transaction |
| Fee calculation | Size-based | Larger transactions pay more |
| Fee destination | Burned | All fees are permanently destroyed |

### Port Reference

| Port | Protocol | Purpose |
|------|----------|---------|
| 8443 | TCP | P2P gossip (libp2p) |
| 8080 | HTTP | JSON-RPC API |
| 8080 | WebSocket | Real-time updates |

### Network Security

**Sybil Resistance:**

The network resists Sybil attacks through:
- Quorum-based consensus (SCP)
- Reputation scoring for peers
- Resource requirements for block minting

**Eclipse Protection:**

Nodes protect against eclipse attacks by:
- Maintaining diverse peer connections
- Preferring peers with established history
- Regular peer rotation
- Multiple independent peer discovery methods

### Getting Involved

**For Developers:**
- Source code: [github.com/botho-project/botho](https://github.com/botho-project/botho)
- Report bugs via GitHub issues
- Contributions welcome (see CONTRIBUTING.md)

**For Node Operators:**
- Run a node to strengthen the network
- Enable minting if you have reliable uptime
- Monitor your node's quorum intersection

**For Users:**
- Test the wallet and report issues
- Provide feedback on user experience
- Help with documentation and translations
    `,
  },
  {
    id: 'tokenomics',
    title: 'Tokenomics',
    icon: Coins,
    content: `
## Tokenomics

Botho (BTH) uses a two-phase emission model designed for long-term sustainability: an initial distribution phase with halvings, followed by perpetual tail emission targeting stable inflation.

### Overview

| Parameter | Value |
|-----------|-------|
| Token symbol | BTH |
| Smallest unit | nanoBTH (10⁻⁹ BTH) |
| Pre-mine | None (100% mined) |
| Phase 1 supply | ~100 million BTH |
| Target block time | 60 seconds |

### Unit System

BTH uses 9-decimal precision:

- **1 nanoBTH** = 0.000000001 BTH (smallest unit)
- **1 microBTH (µBTH)** = 1,000 nanoBTH = 0.000001 BTH
- **1 milliBTH (mBTH)** = 1,000,000 nanoBTH = 0.001 BTH
- **1 BTH** = 1,000,000,000 nanoBTH

---

## Emission Schedule

### Phase 1: Halving Period (Years 0-10)

Minting rewards halve every ~2 years, distributing approximately 100 million BTH over 10 years.

| Period | Years | Minting Reward | Cumulative Supply |
|--------|-------|--------------|-------------------|
| Halving 0 | 0-2 | 50 BTH | ~52.6M BTH |
| Halving 1 | 2-4 | 25 BTH | ~78.9M BTH |
| Halving 2 | 4-6 | 12.5 BTH | ~92.0M BTH |
| Halving 3 | 6-8 | 6.25 BTH | ~98.6M BTH |
| Halving 4 | 8-10 | 3.125 BTH | ~100M BTH |

**Halving interval**: 1,051,200 blocks (~2 years at 60-second blocks)

### Phase 2: Tail Emission (Year 10+)

After Phase 1, Botho transitions to perpetual tail emission targeting **2% annual net inflation**.

**Why tail emission?**

- **Security budget** - Ensures minters always have incentive to secure the network
- **Lost coin replacement** - Compensates for coins lost to forgotten keys
- **Predictable monetary policy** - 2% is below typical fiat inflation

At 100M BTH supply, the tail minting reward works out to approximately **4.76 BTH per slot**.

---

## Fee Structure

### Transaction Fees

All transaction fees are **burned**, creating deflationary pressure that offsets tail emission.

| Parameter | Value |
|-----------|-------|
| Minimum fee | 400 µBTH (0.0004 BTH) |
| Fee destination | Burned (removed from supply) |
| Priority | Higher fees = faster confirmation |

### Cluster-Based Progressive Fees

Botho implements a novel **progressive fee system** that taxes wealth concentration without enabling Sybil attacks.

**The Problem:** Traditional wealth taxes fail in cryptocurrency because users can split holdings across unlimited addresses.

**The Solution:** Tax based on coin *ancestry*, not account identity.

**How It Works:**

1. Each minting reward creates a unique "cluster" identity
2. Every UTXO carries a tag vector tracking what fraction traces back to each cluster
3. Fee rate increases with cluster wealth via sigmoid curve
4. Tags decay ~5% per transaction hop, encouraging circulation

| Parameter | Value |
|-----------|-------|
| Minimum rate | 0.05% (diffused clusters) |
| Maximum rate | 30% (concentrated clusters) |
| Sigmoid midpoint | 10M BTH |
| Tag decay | 5% per hop |

**Why it's Sybil-resistant:** Splitting coins across addresses doesn't reduce fees because the fee rate depends on cluster wealth, not transaction size or account count.

---

## Supply Projections

### Long-Term Growth

| Year | Approximate Supply | Annual Inflation |
|------|-------------------|------------------|
| 2 | ~52.6M BTH | High (initial) |
| 5 | ~85M BTH | ~15% |
| 10 | ~100M BTH | ~3% |
| 20 | ~122M BTH | 2% |
| 50 | ~180M BTH | 2% |
| 100 | ~295M BTH | 2% |

---

## Economic Design Philosophy

### Why No Pre-mine?

- **Fair distribution** - Everyone starts equal; early minters take on risk
- **Credibility** - No insider advantage or founder enrichment
- **Decentralization** - No concentrated holdings from day one

### Why Burn Fees?

- **Deflationary pressure** - Offsets tail emission
- **Simple economics** - No complex fee distribution mechanisms
- **Predictable** - Net inflation = gross emission - burns

### Why Progressive Cluster Fees?

- **Reduce concentration** - Wealthy clusters pay more
- **Sybil-resistant** - Can't avoid by splitting accounts
- **Encourage circulation** - Moving coins diffuses tags, reducing fees
- **Privacy-compatible** - Works with ring signatures and stealth addresses
    `,
  },
]

export function DocsPage() {
  const location = useLocation()
  const hash = location.hash.slice(1) || 'getting-started'
  const currentSection = sections.find((s) => s.id === hash) || sections[0]
  const [mobileMenuOpen, setMobileMenuOpen] = useState(false)

  const handleNavClick = () => {
    setMobileMenuOpen(false)
  }

  return (
    <div className="min-h-screen flex flex-col md:flex-row">
      {/* Mobile header */}
      <header className="md:hidden sticky top-0 z-50 bg-abyss/95 backdrop-blur border-b border-steel">
        <div className="flex items-center justify-between px-4 py-3">
          <Link to="/" className="flex items-center gap-2">
            <Logo size="sm" showText={false} />
            <span className="font-display text-base font-semibold">Botho</span>
          </Link>
          <button
            onClick={() => setMobileMenuOpen(!mobileMenuOpen)}
            className="p-2 -mr-2 text-ghost hover:text-light transition-colors"
            aria-label={mobileMenuOpen ? 'Close menu' : 'Open menu'}
          >
            {mobileMenuOpen ? <X size={24} /> : <Menu size={24} />}
          </button>
        </div>
      </header>

      {/* Mobile menu overlay */}
      {mobileMenuOpen && (
        <div
          className="md:hidden fixed inset-0 z-40 bg-void/80 backdrop-blur-sm"
          onClick={() => setMobileMenuOpen(false)}
        />
      )}

      {/* Mobile slide-out menu */}
      <aside
        className={`
          md:hidden fixed top-0 left-0 bottom-0 z-50 w-72 bg-abyss border-r border-steel
          transform transition-transform duration-300 ease-in-out overflow-y-auto
          ${mobileMenuOpen ? 'translate-x-0' : '-translate-x-full'}
        `}
      >
        <div className="p-4 border-b border-steel flex items-center justify-between">
          <Link to="/" className="flex items-center gap-2" onClick={handleNavClick}>
            <Logo size="sm" showText={false} />
            <span className="font-display text-base font-semibold">Botho Docs</span>
          </Link>
          <button
            onClick={() => setMobileMenuOpen(false)}
            className="p-2 -mr-2 text-ghost hover:text-light transition-colors"
          >
            <X size={20} />
          </button>
        </div>
        <nav className="p-4 space-y-1">
          {sections.map((section) => (
            <Link
              key={section.id}
              to={`/docs#${section.id}`}
              onClick={handleNavClick}
              className={`flex items-center gap-3 px-3 py-2.5 rounded-lg transition-colors ${
                currentSection.id === section.id
                  ? 'bg-pulse/10 text-pulse'
                  : 'text-ghost hover:text-light hover:bg-steel/50'
              }`}
            >
              <section.icon size={18} />
              {section.title}
            </Link>
          ))}
        </nav>
        <div className="p-4 border-t border-steel mt-auto">
          <Link
            to="/"
            onClick={handleNavClick}
            className="flex items-center gap-2 text-ghost hover:text-light transition-colors text-sm"
          >
            <ArrowLeft size={16} />
            Back to home
          </Link>
        </div>
      </aside>

      {/* Desktop sidebar */}
      <aside className="hidden md:block w-64 border-r border-steel bg-abyss/50 fixed top-0 bottom-0 left-0 overflow-y-auto">
        <div className="p-6">
          <Link to="/" className="flex items-center gap-3 mb-8">
            <Logo size="md" showText={false} />
            <span className="font-display text-lg font-semibold">Botho</span>
          </Link>
          <nav className="space-y-1">
            {sections.map((section) => (
              <Link
                key={section.id}
                to={`/docs#${section.id}`}
                className={`flex items-center gap-3 px-3 py-2 rounded-lg transition-colors ${
                  currentSection.id === section.id
                    ? 'bg-pulse/10 text-pulse'
                    : 'text-ghost hover:text-light hover:bg-steel/50'
                }`}
              >
                <section.icon size={18} />
                {section.title}
              </Link>
            ))}
          </nav>
        </div>
        <div className="p-6 border-t border-steel">
          <Link
            to="/"
            className="flex items-center gap-2 text-ghost hover:text-light transition-colors text-sm"
          >
            <ArrowLeft size={16} />
            Back to home
          </Link>
        </div>
      </aside>

      {/* Main content */}
      <main className="flex-1 md:ml-64">
        <div className="max-w-3xl mx-auto px-4 sm:px-8 md:px-12 py-8 md:py-16">
          <div className="flex items-center gap-3 mb-6 md:mb-8">
            <currentSection.icon className="text-pulse shrink-0" size={28} />
            <h1 className="font-display text-2xl md:text-3xl font-bold">{currentSection.title}</h1>
          </div>
          <div className="prose prose-invert max-w-none prose-headings:font-display prose-h2:text-xl prose-h2:mt-8 prose-h2:mb-4 prose-h3:text-lg prose-h3:mt-6 prose-h3:mb-3 prose-p:text-ghost prose-p:leading-relaxed prose-li:text-ghost prose-code:bg-steel/50 prose-code:px-1.5 prose-code:py-0.5 prose-code:rounded prose-code:text-pulse prose-code:before:content-none prose-code:after:content-none prose-pre:bg-void prose-pre:border prose-pre:border-steel prose-strong:text-light">
            <ReactMarkdown remarkPlugins={[remarkGfm]}>{currentSection.content.trim()}</ReactMarkdown>
          </div>
        </div>
      </main>
    </div>
  )
}
