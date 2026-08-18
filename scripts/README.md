# scripts

Repo-level shell scripts: bridge end-to-end drivers, testnet provisioning, and
build/maintenance helpers. All are run from the repository root
(`./scripts/<name>.sh`) and print their own usage.

Node/host deployment lives in [`../infra/`](../infra/); per-crate helpers live
next to their crate (e.g. [`../cluster-tax/scripts/`](../cluster-tax/scripts/)).

## Bridge end-to-end drivers

Each drives the **real** bridge engine (`OrderProcessor::process_pending_orders`)
rather than a mock, and tears its nodes down on exit. The first three need no
credentials at all.

| Script | What it runs |
|--------|--------------|
| [`bridge-e2e-local.sh`](bridge-e2e-local.sh) | Hermetic Ethereum-leg happy path: local Hardhat node (chain id 31337) + the `#[ignore]`d Rust fork tests — attestation → Safe-wrapped `bridgeMint` → confirmation → `bridgeBurn` → watcher scan. No testnet access or secrets. |
| [`bridge-e2e-fork.sh`](bridge-e2e-fork.sh) | The same pipeline against a **Sepolia fork** (`anvil --fork-url <rpc>`), so real Sepolia state is in play. Takes an RPC URL; accounts are funded on the fork via `*_setBalance`. |
| [`bridge-e2e-full-loop.sh`](bridge-e2e-full-loop.sh) | The full wrap → mint wBTH → burn → release BTH round trip with **both** chains live locally (Hardhat + a `botho-testnet` node), asserting the peg, custody, proof-of-reserves and federation properties. |
| [`bridge-e2e-defi-fork.sh`](bridge-e2e-defi-fork.sh) | The DeFi round trip on a Sepolia fork — mint BTH → wrap to wBTH → seed a Uniswap v3 wBTH/WETH pool → swap → burn the proceeds → release native BTH to a fresh stealth output. This is the mainnet liquidity-launch rehearsal. |
| [`bridge-e2e-defi-solana.sh`](bridge-e2e-defi-solana.sh) | The Solana analog of the above, on devnet: real Ed25519 t-of-n mint submission → seed an Orca Whirlpool with the freshly bridged wBTH → swap → burn → release. |

## Bridge testnet provisioning

| Script | Purpose |
|--------|---------|
| [`bridge-testnet-accounts.sh`](bridge-testnet-accounts.sh) | Generates the Sepolia (secp256k1) and Solana-devnet (ed25519) keypairs the live wBTH deploy needs. Idempotent; keys are written 0600 into a gitignored `.secrets/bridge-testnet/` and never printed. Testnet only. |
| [`bridge-testnet-federation.sh`](bridge-testnet-federation.sh) | Stands up a real t-of-n federation — N independent `bth-bridge` processes exchanging signed attestations over `POST /api/attest` — against live betanet, Sepolia and Solana devnet, and drives Phase C of the runbook. |
| [`deploy-safe-fork-test.sh`](deploy-safe-fork-test.sh) | Fork-tests the custody bring-up before the live run: deploys a 2-of-3 Safe and `WrappedBTH` on a Sepolia fork and asserts the Safe holds admin/minter/pauser. |

The manual live-testnet procedure these scripts rehearse is
[`docs/bridge/testnet-e2e-runbook.md`](../docs/bridge/testnet-e2e-runbook.md).

## Build and maintenance

| Script | Purpose |
|--------|---------|
| [`build-release.sh`](build-release.sh) | Reproducible release build — pins the environment factors that affect codegen and emits `dist/` binaries with SHA-256 checksums, optionally GPG-signed (`--sign`, `GPG_KEY_ID`). |
| [`version.sh`](version.sh) | Reads/checks/bumps the version across the botho-native crates and npm packages (`check`, `bump patch|minor|major`, `set X.Y.Z [--tag]`). Deliberately excludes the MobileCoin-fork crates pinned at the upstream `7.1.0`. |
| [`fuzz.sh`](fuzz.sh) | Wrapper around the [`../fuzz/`](../fuzz/) targets — `list`, plus timed runs (quick/medium/long/overnight) with corpus and log management. |
| [`join-betanet.sh`](join-betanet.sh) | Smoke test: launches a throwaway local node, points it at the live betanet seed over the public internet, and verifies it peers and syncs. Ops/manual only — it depends on live infrastructure and must not be wired into PR CI. |
