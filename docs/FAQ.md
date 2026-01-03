# Frequently Asked Questions

## General

### What is Botho?

Botho is a privacy-preserving cryptocurrency designed for fairness and long-term sustainability. The name comes from the Sesotho/Setswana word meaning "humanity" — reflecting the philosophy that money should serve community rather than concentrate power.

Key features:
- **Private by default** — Stealth addresses hide recipients, ring signatures hide senders
- **Anti-hoarding economics** — Progressive fees discourage wealth concentration
- **Sustainable security** — Perpetual tail emission ensures miners always have incentive
- **Fast finality** — Transactions confirm in seconds, not minutes

### How is Botho different from Bitcoin?

| Aspect | Bitcoin | Botho |
|--------|---------|-------|
| Privacy | Transparent (all transactions public) | Private by default |
| Finality | ~60 minutes (6 confirmations) | ~3-5 seconds |
| Fee system | Flat fees to miners | Progressive fees, burned |
| Long-term security | Relies on fees after 2140 | Perpetual tail emission |
| Consensus | Nakamoto (longest chain) | SCP (Byzantine agreement) |

### How is Botho different from Monero?

Both prioritize privacy, but differ in economics and consensus:

| Aspect | Monero | Botho |
|--------|--------|-------|
| Consensus | Proof-of-work (RandomX) | PoW + SCP hybrid |
| Finality | ~20 minutes (10 confirmations) | ~3-5 seconds |
| Fee destination | To miners | Burned |
| Wealth redistribution | None | Progressive cluster fees |
| Quantum resistance | Not yet | Hybrid (see below) |

### Is Botho quantum-resistant?

Yes, where it matters most:

| Component | Algorithm | Quantum Safety |
|-----------|-----------|----------------|
| **Recipient privacy** | ML-KEM-768 stealth addresses | ✓ PQ-safe |
| **Amount privacy** | Pedersen hiding (info-theoretic) | ✓ PQ-safe |
| **Sender anonymity** | CLSAG ring signatures | Classical |

**Why is sender anonymity classical?**

We prioritize PQ protection for recipient identity and amounts because these are permanent (on-chain forever). Sender anonymity uses classical CLSAG because:

1. **Network-level attacks dominate** — IP correlation and timing analysis are more practical threats today
2. **Compact transactions** — CLSAG (~700 bytes) keeps blockchain growth to ~100 GB/year, enabling desktop nodes
3. **Larger anonymity sets** — More users can run nodes, improving privacy for everyone

See [Why This Architecture?](privacy.md#why-this-architecture) for detailed rationale.

---

## Getting Started

### How do I get BTH?

Currently, there are two ways:

1. **Mine it** — Run a node with `--mint` flag
2. **Receive it** — Get your address with `botho address` and have someone send you BTH

As the network grows, exchanges and other acquisition methods will become available.

### Do I need to run a full node?

No. You can use the web wallet at [botho.io](https://botho.io) which connects to public nodes. However, running your own node gives you:

- Full privacy (your node scans for your transactions)
- Network participation
- Ability to mine

### What are the system requirements?

**Minimum (sync only):**
- 2 GB RAM
- 10 GB disk space
- Broadband internet

**Recommended (mining):**
- 4+ CPU cores
- 4 GB RAM
- SSD storage
- Stable internet connection

### How long does initial sync take?

With a fresh network, sync is nearly instant. As the blockchain grows, initial sync will take longer depending on your connection speed and hardware.

---

## Privacy

### Are all transactions private?

All transactions hide the **recipient** (via ML-KEM stealth addresses). Other privacy depends on type:

| Type | Recipient | Amount | Sender |
|------|-----------|--------|--------|
| Minting | Hidden | Public | Known (minter) |
| Private | Hidden | Hidden | Hidden (20-member ring) |

**Sender privacy** depends on transaction type:
- **Minting**: Sender is visible (ML-DSA signature)
- **Private**: Sender hidden via CLSAG ring signatures (ring size 20)

### Can I see my transaction on a block explorer?

You can see that a transaction exists, but:

- **Recipient**: Always hidden (stealth addresses)
- **Amount**: Hidden except for Minting transactions
- **Sender**: Hidden for Private transactions (ring signatures)

For Private transactions, sender, recipient, and amount are all hidden.

### What information is NOT hidden?

- Transaction existence and approximate size
- Timing of transactions
- Your IP address (use Tor for network-level privacy)

### Can Botho be traced?

Botho provides strong cryptographic privacy, but privacy is never absolute:

- **Timing analysis** may reveal patterns if you transact predictably
- **IP tracking** is possible without Tor/VPN
- **Exchange KYC** links your identity to addresses you deposit to/withdraw from

For maximum privacy, follow the [privacy best practices](privacy.md#privacy-best-practices).

---

## Mining (Minting)

### How does mining work in Botho?

Botho uses a unique hybrid approach:

1. **Proof-of-Work**: Miners find valid nonces (SHA-256)
2. **SCP Consensus**: The network agrees on which miner's block is accepted

This means network latency doesn't determine winners — the quorum does.

### Can I mine solo?

No. Solo mining is impossible by design. You need at least one other peer to form a consensus quorum. This prevents network fragmentation and ensures all miners contribute to the same chain.

### Is mining profitable?

Profitability depends on:

- Your hardware (more CPU cores = higher hashrate)
- Electricity costs
- Network difficulty
- BTH market value (when trading exists)

Currently, with low network difficulty, CPU mining is viable.

### What hardware should I use?

Botho uses SHA-256, which is ASIC-friendly. However, while the network is small, regular CPUs work fine. As the network grows, dedicated hardware may become necessary to remain competitive.

---

## Economics

### What is the total supply?

Botho has no hard cap. Instead:

- **Phase 1 (years 0-10)**: ~100 million BTH via halvings
- **Phase 2 (year 10+)**: ~2% annual tail emission

This ensures permanent mining incentives while maintaining predictable monetary policy.

### Why are fees burned instead of paid to miners?

Burning fees creates deflationary pressure that offsets tail emission, keeping net inflation around 2%. It also:

- Simplifies economics (no complex fee distribution)
- Prevents fee-based miner centralization
- Creates predictable monetary policy: `net_supply = minted - burned`

### What are "cluster fees" and why do they exist?

Cluster fees are progressive transaction fees based on coin ancestry:

- Coins that circulate widely pay low fees (~0.05%)
- Coins that stay concentrated pay high fees (up to 30%)

This discourages hoarding and encourages economic activity — without tracking identities. The system is Sybil-resistant because fees are based on where coins came from, not how many wallets you have.

### How do cluster fees preserve privacy?

Cluster tracking happens at the UTXO level, not the account level. The system knows "this coin traces back to minting event X" but doesn't know "this coin belongs to person Y." Ring signatures further obscure the connection.

### Why do I pay higher fees than others?

Fees are based on **source wealth** — where your coins originated. Coins traced back to large mining clusters pay higher fees (up to 15%), while well-circulated coins pay lower fees (~1%). This is Sybil-resistant because splitting coins doesn't change their origin.

### How can I reduce my cluster attribution?

Through legitimate economic activity. When you spend coins and they mix with others' coins in transactions, the cluster tags naturally decay. Key factors:

- **Age**: UTXOs must be at least ~2 hours old before any decay applies
- **Mixing**: Combining with coins from different sources dilutes tags
- **Rate limit**: Maximum ~12 decay events per day, regardless of transaction count

After ~10-20 hops through real commerce, original cluster attribution becomes negligible.

### What is the maximum fee I can pay?

The progressive fee curve caps at **15%** for the wealthiest clusters (those controlling 70%+ of maximum tracked wealth). Most users pay between 1-10% based on coin provenance.

### How does trading affect my privacy?

Trading improves privacy over time:

- Each legitimate transaction allows tags to decay by 5%
- After ~10-20 hops through real commerce, original cluster attribution is negligible
- Ring signatures hide which specific input was spent
- The age-based decay mechanism doesn't add any new trackable metadata (uses only the UTXO creation block, which is already public)

---

## Technical

### What is SCP (Stellar Consensus Protocol)?

SCP is a Byzantine fault-tolerant consensus protocol that allows nodes to agree on transactions even if some nodes are malicious or offline. Key properties:

- **Fast finality**: Transactions are final in seconds
- **No forks**: Once agreed, blocks can't be reorganized
- **Decentralized trust**: Nodes choose who they trust

### What is a "quorum"?

A quorum is the set of nodes that must agree for consensus to proceed. In Botho:

- **Recommended mode**: Automatically trusts discovered peers
- **Explicit mode**: You specify exactly which nodes to trust

A healthy quorum tolerates `f` failures where `f = (n-1)/3` for `n` nodes.

### Why does Botho use both PoW and SCP?

PoW provides:
- Fair coin distribution (anyone can mine)
- Sybil resistance (mining costs resources)

SCP provides:
- Fast finality (no waiting for confirmations)
- No selfish mining attacks
- Fair block selection (not fastest-propagation-wins)

Together, they get the benefits of both approaches.

### What ports does Botho use?

| Port | Purpose |
|------|---------|
| 7100 | P2P gossip (libp2p) |
| 7101 | JSON-RPC API |

---

## Troubleshooting

### My node won't connect to peers

1. Check your internet connection
2. Verify bootstrap peers in `~/.botho/config.toml`
3. Ensure port 7100 isn't blocked by firewall
4. Try restarting the node

See [Troubleshooting Guide](troubleshooting.md) for more.

### My balance shows 0 but I received funds

1. Wait for your node to fully sync (`botho status`)
2. Verify you're using the correct wallet (check `botho address`)
3. Confirm the sender's transaction was confirmed

### Mining says "waiting for quorum"

You need at least one other peer to mine. Check:

1. Are you connected to peers? (`botho status`)
2. Is the bootstrap peer online?
3. Try lowering `min_peers` in config (not recommended for production)

---

## Project

### Who created Botho?

Botho is an open-source project derived from [MobileCoin](https://github.com/mobilecoinfoundation/mobilecoin), simplified for desktop/server use without SGX enclaves.

### Is there a pre-mine or founder allocation?

No. 100% of BTH is mined through proof-of-work. There is no pre-mine, no founder reward, and no special allocation.

### How can I contribute?

- **Code**: See [CONTRIBUTING.md](../CONTRIBUTING.md)
- **Testing**: Run a node, report bugs
- **Documentation**: Improve the docs
- **Community**: Help answer questions

### Where can I get help?

- **Documentation**: [docs/](.)
- **GitHub Issues**: [github.com/botho-project/botho/issues](https://github.com/botho-project/botho/issues)
- **GitHub Discussions**: [github.com/botho-project/botho/discussions](https://github.com/botho-project/botho/discussions)
