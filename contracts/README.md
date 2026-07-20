## contracts

On-chain contracts and deployment tooling for wrapped BTH (wBTH) — the
external-chain leg of the [BTH bridge](../bridge/). These are not Rust workspace
members (this directory is excluded from the Cargo workspace); each sub-dir has
its own README with build and deployment detail.

### Sub-directories

- [`ethereum`](./ethereum/README.md) — `WrappedBTH.sol`, the ERC-20 wBTH token
  minted 1:1 against BTH locked in the bridge.
- [`solana`](./solana/README.md) — the `wbth` Anchor program that mints the wBTH
  SPL token 1:1.
- [`wbth-ntt`](./wbth-ntt/README.md) — Wormhole NTT deployment config for
  bridging wBTH (Sepolia ↔ HyperEVM).

### Workspace fit

These contracts are the counterpart to the [`bridge`](../bridge/) service, which
relays lock/mint and burn/unlock events between BTH and these chains.
