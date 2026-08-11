## consensus

The Botho consensus stack (Stellar Consensus Protocol), split into focused
sub-crates. This directory is an index; see each sub-crate's own README (where
present) for detail.

### Sub-crates

- [`scp`](./scp/) — `bth-consensus-scp`: Stellar Consensus Protocol
  implementation, with its own sub-crates
  [`types`](./scp/types/) (`bth-consensus-scp-types`: shared SCP data types)
  and [`play`](./scp/play/) (`bth-consensus-scp-play`: SCP playground /
  simulation harness).
- [`quorum-sim`](./quorum-sim/) — `bth-quorum-sim`: static quorum-health
  analyzer for Botho's curated FBAS federation.

### Workspace fit

These crates provide the consensus layer consumed by the `botho` node.
