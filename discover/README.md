## bth-discover

A CLI tool for discovering Botho network topology and generating node
configurations.

### Role

Connects to the gossip network, discovers peers, and helps operators configure
their nodes — suggesting quorum sets based on observed trust patterns.

### Structure

A single-binary crate (`bth-discover`, entry point `src/main.rs`). It drives the
`bth-gossip` service (`GossipService`, `TopologyAnalyzer`, `QuorumStrategy`) to
observe the live topology and emit configuration.

### Workspace fit

An operator-facing tool built on top of `gossip` (`bth-gossip`) and the
consensus quorum-set types. It complements the `botho` node by helping new nodes
join and configure themselves.
