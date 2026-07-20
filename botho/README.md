## botho

The main Botho node — a privacy-preserving, mined cryptocurrency. This crate
provides the core node library plus the binaries that run and simulate a node.

### Role

Ties the workspace together into a running node: it assembles blockchain types,
networking, consensus, the ledger, and wallet support into the `botho` daemon.

### Key modules (`src/`)

- `block`, `transaction`, `monetary` — block, transaction, and unit primitives.
- `ledger` — chain state and block application.
- `consensus` — SCP-based consensus integration.
- `network` — peer networking and message handling.
- `node` — node lifecycle and orchestration.
- `mempool` — pending-transaction pool.
- `pow` — proof-of-work / mining logic.
- `rpc` — JSON-RPC interface used by wallets and tools.
- `decoy_selection` — ring-signature decoy selection.
- `operator_action`, `operator_key`, `operator_nonce` — operator-signed actions.
- `config`, `telemetry`, `address`, `wallet` — node config, metrics, address
  handling, and embedded wallet support.

### Binaries

- `botho` — the node daemon (`main.rs`).
- `botho-testnet` — helper for spinning up local testnets.
- `scp_sim` — SCP consensus simulator.

### Workspace fit

The top-level application crate. It depends on the workspace's component crates
(`consensus`, `ledger`, `transaction/*`, `gossip`, `crypto`, `common`, and
others) and is the primary consumer of them.
