## bridge

The BTH bridge, which moves value between BTH and external chains (Ethereum and
Solana) via wrapped BTH (wBTH). This directory groups two workspace-member
sub-crates.

### Sub-crates

- [`core`](./core/) — `bth-bridge-core`: core types and logic for the bridge
  (order records, peg accounting, and shared primitives).
- [`service`](./service/) — `bth-bridge-service`: the runnable bridge service
  that relays between BTH and Ethereum / Solana.

### Workspace fit

`service` builds on `core` to produce the bridge relayer binary. Related
on-chain contracts live under [`../contracts`](../contracts/) (Ethereum, Solana,
and the Wormhole NTT deployment for wBTH).
