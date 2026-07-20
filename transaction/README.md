## transaction

The Botho transaction stack, split into focused sub-crates. This directory is an
index; see each sub-crate's own README (where present) for detail.

### Sub-crates

- [`clsag`](./clsag/) — `bth-transaction-clsag`: self-contained CLSAG
  ring-signature transaction types (stealth addresses + ring signatures).
- [`core`](./core/README.md) — `bth-transaction-core`: core transaction logic.
- [`signer`](./signer/README.md) — `bth-transaction-signer`: transaction
  signing.
- [`types`](./types/README.md) — `bth-transaction-types`: shared transaction
  data types.

### Workspace fit

These crates provide the transaction primitives consumed by the `botho` node,
`botho-wallet`, and other clients that build or verify spends.
