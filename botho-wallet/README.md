## botho-wallet

A standalone thin wallet client for Botho. It manages its own keys locally and
communicates with untrusted Botho nodes over JSON-RPC — private keys never leave
the wallet.

### Key modules (`src/`)

- `keys` — key management.
- `storage` — encrypted local wallet storage.
- `secmem` — secure (zeroizing) in-memory secret handling.
- `transaction`, `transaction_legacy` — transaction construction.
- `ring_builder`, `decoy_selection` — ring-signature assembly and decoy choice.
- `fee_estimation` — fee estimation.
- `discovery`, `rpc_pool` — node discovery and pooled JSON-RPC connections to
  untrusted nodes.
- `commands` — CLI subcommands (`main.rs` is the entry point).

### Workspace fit

A client-facing binary crate. It talks to a `botho` node's RPC interface rather
than embedding node logic, and reuses the workspace's transaction and crypto
crates for constructing spends.
