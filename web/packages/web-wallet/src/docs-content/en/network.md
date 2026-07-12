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

**Seed Discovery:**

Bootstrap peers are discovered via DNS TXT records rather than a hardcoded list:

| Network | DNS Seed Domain |
|---------|-----------------|
| Mainnet | seeds.botho.io |
| Testnet | seeds.testnet.botho.io |

When your node starts, it resolves the seed domain to learn about bootstrap peers (you can also pin explicit `bootstrap_peers` in the config). After initial discovery, your node maintains connections to multiple peers for redundancy.

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
| Block time | 3–40 seconds (load-adaptive) | Fast blocks under heavy traffic (3 s only at 20+ tx/s), slow blocks when idle |
| Max block size | 20 MB | Maximum serialized block size |
| Max transactions per block | 5,000 | Transaction count limit |

**Transaction Limits:**

| Parameter | Value | Description |
|-----------|-------|-------------|
| Max inputs | 16 | Maximum inputs per transaction |
| Max outputs | 16 | Maximum outputs per transaction |
| Ring size | 20 | Number of members in CLSAG ring signature |
| Max tx size | 100 KB | Maximum serialized transaction size |

**Fees:**

| Parameter | Value | Description |
|-----------|-------|-------------|
| Fee formula | per-byte rate × size × cluster factor × output penalty | Size-based, wealth-progressive |
| Cluster factor | 1x–6x | Progressive multiplier from coin provenance |
| Output penalty | quadratic, capped at 100x | Anti-UTXO-farming |
| Fee destination | 80% lottery / 20% burned | Redistribution plus deflationary pressure |

### Port Reference

Defaults differ by network (mainnet / testnet); all are configurable:

| Port (mainnet / testnet) | Protocol | Purpose |
|--------------------------|----------|---------|
| 7100 / 17100 | TCP | P2P gossip (libp2p) |
| 7101 / 17101 | HTTP + WebSocket | JSON-RPC API and real-time updates |
| 9090 / 19090 | HTTP | Prometheus metrics |

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
