## bth-gossip

A gossip protocol layer for Botho that provides peer discovery and topology
sharing.

### Role

Enables nodes to find each other without static configuration, share their
quorum sets so new nodes can learn the network structure, and stay in sync via a
hybrid push-pull approach (gossipsub for real-time updates plus periodic pulls).

### Key modules (`src/`)

- `service` — the top-level `GossipService`.
- `behaviour` — the libp2p network behaviour.
- `messages` — gossip message types.
- `store` — peer/topology store.
- `analyzer` — `TopologyAnalyzer` for reasoning about observed topology.
- `consensus_integration` — bridges gossiped topology into consensus quorum
  strategy.
- `rate_limit` — inbound message rate limiting.
- `config`, `error` — configuration and error types.

### Workspace fit

A networking crate consumed by the `botho` node and by the `bth-discover` tool
to observe and configure network topology.
