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
| Fee system | Flat fees to miners | Progressive fees: 80% redistributed, 20% burned |
| Long-term security | Relies on fees after 2140 | Perpetual tail emission |
| Consensus | Nakamoto (longest chain) | SCP (Byzantine agreement) |

### How is Botho different from Monero?

Both prioritize privacy, but differ in economics and consensus:

| Aspect | Monero | Botho |
|--------|--------|-------|
| Consensus | Proof-of-work (RandomX) | PoW + SCP hybrid |
| Finality | ~20 minutes (10 confirmations) | ~3-5 seconds |
| Fee destination | To miners | 80% redistribution lottery, 20% burned |
| Wealth redistribution | None | Progressive cluster fees |
| Quantum resistance | Not yet | Hybrid (see below) |

### Is Botho quantum-resistant?

Yes, where it matters most:

| Component | Algorithm | Quantum Safety |
|-----------|-----------|----------------|
| **Recipient privacy** | ML-KEM-768 stealth addresses | ✓ PQ-safe |
| **Amount privacy** | Pedersen hiding (info-theoretic) | ✓ PQ-safe |
| **Sender anonymity** | CLSAG ring signatures | Classical |
| **Minting attribution** | PoW-preimage binding (hash-based) | ✓ PQ-safe |

> **Status**: Amount privacy (Pedersen commitments + Bulletproofs) is the
> ratified design ([ADR 0006](decisions/0006-pq-architecture-ratification.md))
> and is in development (#904). On the current testnet, amounts are public.

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
| Private | Hidden | Hidden (in development — see status note above) | Hidden (20-member ring) |

**Sender privacy** depends on transaction type:
- **Minting**: Minter is known (the PoW preimage commits to the minter's public keys)
- **Private**: Sender hidden via CLSAG ring signatures (ring size 20)

### Can I see my transaction on a block explorer?

You can see that a transaction exists, but:

- **Recipient**: Always hidden (stealth addresses)
- **Amount**: Hidden by design for Private transactions (confidential amounts are in development — public on the current testnet); always public for Minting
- **Sender**: Hidden for Private transactions (ring signatures)

For Private transactions, sender and recipient are hidden today; amounts will be hidden once confidential transactions ship (#904).

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

1. **Proof-of-Work**: Miners find valid nonces (RandomX, the CPU-egalitarian hash also used by Monero)
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

Botho uses RandomX, which is deliberately ASIC- and GPU-resistant: ordinary CPUs compete on near-equal footing by design. A desktop with spare CPU cores is a reasonable mining setup, and dedicated hardware offers no dramatic advantage. Note that RandomX keeps a large in-memory dataset, so allow ~2-3 GB of RAM for mining.

---

## Economics

### What is the total supply?

Botho has no hard cap. Instead:

- **Phase 1 (years 0-5)**: ~611 million BTH via 5 halvings (~1 year each)
- **Phase 2 (year 5+)**: ~2% annual net tail emission (supply-dependent per-block reward, ~1.9 BTH/block at the ~611M tail-onset supply)

This ensures permanent mining incentives while maintaining predictable monetary policy.

### What happens to transaction fees?

Fees are not paid to miners. Each block, collected fees are split **80% into a cluster-tilted redistribution lottery** (paid back out to randomly selected, mostly small/well-circulated holders) and **20% burned**. Only the burned 20% is deflationary and partially offsets tail emission, keeping net inflation around 2%. This split also:

- Recycles wealth-concentration charges back into the economy rather than destroying them
- Prevents fee-based miner centralization
- Keeps monetary policy predictable: only the burned portion reduces supply (`net_supply = minted - burned`)

### What are "cluster fees" and why do they exist?

Cluster fees are progressive transaction fees based on coin ancestry. Fees are size-based, and the multiplier depends on where your coins came from:

- Coins that circulate widely pay the base rate (1x)
- Coins from concentrated clusters pay up to 6x the base rate

This discourages hoarding and encourages economic activity — without tracking identities. The system is Sybil-resistant because fees are based on where coins came from, not how many wallets you have.

### How do cluster fees preserve privacy?

Cluster tracking happens at the UTXO level, not the account level. The system knows "this coin traces back to minting event X" but doesn't know "this coin belongs to person Y." Ring signatures further obscure the connection.

### Why do I pay higher fees than others?

Fees are based on **source wealth** — where your coins originated. Coins traced back to large mining clusters pay up to 6x the base size-based fee, while well-circulated coins pay 1x. This is Sybil-resistant because splitting coins doesn't change their origin.

### How can I reduce my cluster attribution?

Through legitimate economic activity. When you spend coins and they mix with others' coins in transactions, the cluster tags naturally decay. Key factors:

- **Age**: UTXOs must be at least ~2 hours old before any decay applies
- **Mixing**: Combining with coins from different sources dilutes tags
- **Rate limit**: Maximum ~12 decay events per day, regardless of transaction count

Through sustained real commerce — decay plus blending with counterparties' coins — original cluster attribution becomes negligible.

### What is the maximum fee I can pay?

The progressive multiplier caps at **6x** the base size-based fee, saturating for the wealthiest clusters (≳10M BTH of cluster wealth). Well-circulated coins pay 1x; the curve's midpoint (3.5x) sits at 100,000 BTH of cluster wealth.

### How does trading affect my privacy?

Trading improves privacy over time:

- Each eligible transaction hop decays tags by 5%, and blending with others' coins dilutes attribution faster still
- With sustained real commerce, original cluster attribution becomes negligible
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

See [Troubleshooting Guide](operations/troubleshooting.md) for more.

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
