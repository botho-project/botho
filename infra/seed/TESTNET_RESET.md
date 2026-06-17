# Testnet Reset & Multi-Seed Bootstrap (operator runbook)

This document covers the **coordinated testnet reset** onto current `main`
(protocol `2.0.0`) and the **multi-seed bootstrap** plan. It pairs with the
automatable-prep PR for issue #323.

> **Scope split.** The *code/config* prep (scripts, genesis reconciliation,
> multi-seed config scaffolding, release-build path) is in the repo. The
> *deploy/reset/DNS/faucet* steps below require AWS/DNS credentials and are
> performed by a human operator. Nothing in CI touches live infra.

## 1. Genesis / network-parameter reconciliation (verified)

Current `main` values that any reset must match:

| Parameter | Value | Source |
|-----------|-------|--------|
| Protocol version | `2.0.0` | `botho/src/network/discovery.rs` (`PROTOCOL_VERSION`, `MIN_SUPPORTED_PROTOCOL_VERSION`) |
| Testnet genesis magic | `BOTHO_TESTNET_GENESIS_V1` (32-byte, in `prev_block_hash`) | `botho/src/block.rs` (`TESTNET_GENESIS_MAGIC`) |
| Mainnet genesis magic | `BOTHO_MAINNET_GENESIS_V1` | `botho/src/block.rs` (`MAINNET_GENESIS_MAGIC`) |
| Testnet network magic | `BTHT` (`0x42 0x54 0x48 0x54`) | `transaction/types/src/constants.rs` (`Network::magic_bytes`) |
| Testnet gossip / RPC ports | `17100` / `17101` | `transaction/types/src/constants.rs` |
| RPC `network` string | `botho-testnet` | `botho/src/rpc/mod.rs` (`format!("botho-{}", network.name())`) |
| Data dir | `~/.botho/testnet/` (`ledger/`, `wallet/`, `config.toml`) | `botho/src/config.rs` (`data_dir`) |

These are consistent across `block.rs`, the network constants, and the RPC
layer — **no genesis/param drift to fix on `main`** as of this PR. A fresh
genesis is produced automatically the first time a node starts with an empty
`~/.botho/testnet/ledger` (the reset scripts clear it).

## 2. Reset scripts

Both scripts support `--help` and `--dry-run` (the dry run prints every
command without contacting any host — safe to run anywhere, no credentials).

### `reset-chain.sh` — wipe chain data over SSH
Runs from a workstation, connects to the seed host over SSH, stops the
service, deletes `~/.botho/<network>/{ledger,wallet}` (preserving
`config.toml`), and restarts.

```bash
./reset-chain.sh --dry-run                 # preview, no SSH
./reset-chain.sh ubuntu@seed.botho.io      # interactive confirm
./reset-chain.sh --force ubuntu@seed.botho.io
./reset-chain.sh --network testnet --service botho-seed ubuntu@seed.botho.io
```

Fixed in this PR: it previously deleted stale `blocks/`, `state/`,
`peers.json` paths that the current node never writes, and targeted the wrong
service name. It now deletes the real artifacts (`ledger/`, `wallet/`) and
defaults to the `botho-seed` unit.

### `reset-to-testnet.sh` — run locally on the host
Runs **on** the seed host. Removes any stale `~/.botho/mainnet`, (re)installs
the `botho-seed` systemd unit, starts it, and verifies `network == botho-testnet`
via RPC on `localhost:17101`.

```bash
./reset-to-testnet.sh --dry-run
./reset-to-testnet.sh
```

## 3. Single-seed vs multi-seed quorum

A lone seed cannot satisfy the default `recommended` quorum (needs >= 2 nodes),
so minting stalls at *"have 1, need 2"*.

- **Single-seed bring-up (mint from a self-quorum):** run the node with
  `--mint --mint-threads 1` and set in `~/.botho/testnet/config.toml`:

  ```toml
  [network.quorum]
  mode = "explicit"
  threshold = 1
  members = []
  ```

- **Multi-seed (>= 2 nodes):** use the committed `botho-seed.service` as-is
  (`run --relay`, no minting on seeds) and quorum `mode = "recommended"`.
  Run minting on a dedicated validator/faucet node.

See the header comments in `botho-seed.service` for the exact `ExecStart`
variants.

## 4. Multi-seed bootstrap config (scaffolding)

PLAN.md "Network Bootstrap Strategy" calls for >= 3 geographically diverse
seeds plus DNS-seed discovery and a hardcoded fallback.

What already exists in code:

- **DNS-seed discovery** — `botho/src/network/dns_seeds.rs`
  (`seeds.botho.io` / `seeds.testnet.botho.io` TXT records, TTL caching, DNS
  failure falls back to hardcoded seeds).
- **PEX** — `botho/src/network/pex.rs` (decentralized peer exchange).
- **Bootstrap order** — explicit `config.bootstrap_peers` -> DNS -> hardcoded
  fallback (`NetworkConfig::bootstrap_peers_async`).

Added in this PR:

- **`botho/src/network/seeds.rs`** — single source of truth for hardcoded
  fallback seeds. `config.rs` and `dns_seeds.rs` now both delegate here, so the
  two lists can no longer drift. It defines regional seed scaffolding for three
  regions (`us`, `eu`, `ap`) for both networks, **gated off by default** behind
  `BOTHO_REGIONAL_SEEDS=1` because the regional DNS records are not yet live.

### Activating regional seeds (operator, when infra exists)

Preferred path (zero client release): publish the new seeds as
`seeds.testnet.botho.io` TXT records (`PEER_ID@host:port`); DNS discovery picks
them up automatically.

Fallback path: launch `us.seed.botho.io`, `eu.seed.botho.io`,
`ap.seed.botho.io` and start nodes with `BOTHO_REGIONAL_SEEDS=1` so the
hardcoded fallback includes them.

## 5. Reproducible release build (verified path, not cut here)

- Workflow: `.github/workflows/release.yml` — triggers on `v*` tags, and
  supports `workflow_dispatch` with a `dry_run` input that builds without
  publishing a release.
- Script: `scripts/build-release.sh` — builds `botho`, `botho-wallet`,
  `botho-exchange-scanner` with pinned `SOURCE_DATE_EPOCH`, isolated
  `CARGO_HOME`, `LC_ALL=C.UTF-8`, `TZ=UTC`, `CARGO_INCREMENTAL=0` for
  reproducibility, then emits SHA256 checksums + `build-info.txt`.
- Deploy hosts are **linux-aarch64** (ARM64 Ubuntu); the release matrix builds
  that target natively (`ubuntu-24.04-arm`).

The v0.1.0 tag is stale (predates cycle-6 / I4). **Cutting a fresh tag is an
operator action** (see below). To validate the build path without publishing,
run `workflow_dispatch` with `dry_run=true`, or locally:
`./scripts/build-release.sh`.

## Operator steps remaining (NOT in this PR — require AWS/DNS/credentials)

1. **Build/publish a current release binary** — either push a fresh `v0.2.x`
   tag (triggers `release.yml`) or build `linux-aarch64` and copy to the host.
2. **Deploy the binary** to `seed.botho.io` (`infra/seed/deploy-botho.sh`,
   needs SSH key).
3. **Reset the chain** to fresh genesis on protocol 2.0.0
   (`reset-chain.sh` / `reset-to-testnet.sh` against the live host).
4. **Bring up >= 1 additional regional seed** to retire `peerCount: 0`, switch
   quorum back to `recommended`, and either publish DNS TXT seeds or set
   `BOTHO_REGIONAL_SEEDS=1`.
5. **Restore the faucet service** (`infra/faucet/`) and confirm reachability.
6. **CloudWatch monitoring + Route53 DNS failover** (PLAN.md "Seed Node" /
   "Disaster Recovery").
7. **Point web wallet + ledger browser** at the new RPC; verify end-to-end.
