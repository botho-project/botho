# Testnet Reset & Multi-Seed Bootstrap (operator runbook)

This document covers the **coordinated testnet reset** onto current `main`
(protocol `6.0.0`) and the **multi-seed bootstrap** plan. It was written for
the #323 reset (protocol `2.0.0`), updated for the #606 reset (protocol
`3.0.0`, H1 consensus fee floor), then the #605/#626 reset (protocol `4.0.0`,
log-domain fee curve + u128 cluster wealth + ratified cluster-wealth decay),
and again for the **v0.6.0 / protocol `6.0.0` reset (2026-07-16)** — universal
ML-KEM minting outputs (#968/#973), Path C lottery (#955), and the
bridge-import cluster tagging batch. The 6.0.0 reset surfaced several
operational gotchas (zombie old-protocol nodes wedging the chain, a degenerate
solo-quorum config, minting-relay crash-loops); the new sections below capture
them so the next reset is clean.

> **Scope split.** The *code/config* prep (scripts, genesis reconciliation,
> multi-seed config scaffolding, release-build path) is in the repo. The
> *deploy/reset/DNS/faucet* steps below require AWS/DNS credentials and are
> performed by a human operator. Nothing in CI touches live infra.

## 1. Genesis / network-parameter reconciliation (verified)

Current `main` values that any reset must match:

| Parameter | Value | Source |
|-----------|-------|--------|
| Protocol version | `6.0.0` | `botho/src/network/discovery.rs` (`PROTOCOL_VERSION`, `MIN_SUPPORTED_PROTOCOL_VERSION`). Consensus-breaking resets require a **major** bump — `is_consensus_compatible` compares major only, so a minor bump would merely warn, not disconnect, old peers. |
| Testnet genesis magic | `BOTHO_TESTNET_GENESIS_V1` (32-byte, in `prev_block_hash`) | `botho/src/block.rs` (`TESTNET_GENESIS_MAGIC`) |
| Mainnet genesis magic | `BOTHO_MAINNET_GENESIS_V1` | `botho/src/block.rs` (`MAINNET_GENESIS_MAGIC`) |
| Testnet network magic | `BTHT` (`0x42 0x54 0x48 0x54`) | `transaction/types/src/constants.rs` (`Network::magic_bytes`) |
| Testnet gossip / RPC ports | `17100` / `17101` | `transaction/types/src/constants.rs` |
| RPC `network` string | `botho-testnet` | `botho/src/rpc/mod.rs` (`format!("botho-{}", network.name())`) |
| Data dir | `~/.botho/testnet/` (`ledger/`, `wallet/`, `config.toml`) | `botho/src/config.rs` (`data_dir`) |

> **Crate/release version vs. protocol version are SEPARATE by design.** The
> crate/release version (`0.6.0`, the git tag `v0.6.0` and the release
> artifact name) tracks the *software build*; the protocol version (`6.0.0` in
> `discovery.rs`) tracks *consensus compatibility*. They happen to share the
> "6" here, but they are not required to move in lockstep — a patch release
> (`v0.6.1`) can ship with an unchanged protocol `6.0.0`, and a
> consensus-breaking change bumps the protocol major without necessarily
> bumping the crate major. Always read the protocol version from
> `discovery.rs`, never infer it from the release tag.

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

Fixed in an earlier PR: it previously deleted stale `blocks/`, `state/`,
`peers.json` paths that the current node never writes, and targeted the wrong
service name. It now deletes the real artifacts (`ledger/`, `wallet/`) and
defaults to the `botho-seed` unit.

> **The wipe deletes `wallet/`.** After a wipe a node has no wallet. This is
> the source of the relay crash-loop gotcha in §4 — a node configured with
> `[minting] enabled = true` but no wallet exits with
> `Error: Cannot mine without a wallet`. Only the minter (faucet) should carry
> a `[wallet]` mnemonic; the seeds relay.

### `reset-to-testnet.sh` — run locally on the host
Runs **on** the seed host. Removes any stale `~/.botho/mainnet`, (re)installs
the `botho-seed` systemd unit, starts it, and verifies `network == botho-testnet`
via RPC on `localhost:17101`.

```bash
./reset-to-testnet.sh --dry-run
./reset-to-testnet.sh
```

### `reprovision-relay.sh` — re-provision ONE relay after a consensus-breaking reset

The composed operator command for the failure that stranded the eu/ap relays in
**#1114**: after a coordinated, consensus-breaking reset, a single regional
relay is left behind on the **old** chain. Its on-disk ledger is incompatible
with the new protocol, so the freshly deployed binary boots against a chain it
can never reconcile. `deploy-botho.sh` alone swaps the binary but never wipes
the ledger; `reset-chain.sh` alone wipes the ledger but never upgrades/pins the
binary. Neither verifies the relay actually re-peers and converges on the
fleet's tip. This script does all of it, in the only order that works:

1. stops the service,
2. wipes **only** `ledger/` + `wallet/` (preserving `config.toml` **and**
   `node_key` — see below),
3. deploys the pinned release (delegates to `deploy-botho.sh`),
4. restarts onto the fresh, empty ledger,
5. polls the relay's **LOCAL** RPC (§7) until `peerCount > 0` **and** its
   `tipHash` matches a known-good validator, with a bounded timeout.

```bash
./reprovision-relay.sh --dry-run ubuntu@eu.seed.botho.io          # preview, no SSH
RELEASE_TAG=v0.6.0 ./reprovision-relay.sh ubuntu@eu.seed.botho.io
RELEASE_TAG=v0.6.0 ./reprovision-relay.sh --service botho \
    --validator-rpc https://seed.botho.io/rpc ubuntu@ap.seed.botho.io
```

> **`node_key` survives the wipe automatically.** The stable peer identity lives
> at `~/.botho/<network>/node_key`, a **sibling** of `ledger/` and `wallet/`
> (`botho/src/config.rs` `node_key_path_from_config`), so removing `ledger/` +
> `wallet/` never touches it. The relay keeps its `PEER_ID` across the
> re-provision — no acceptance-criterion code change was needed for this, only
> verification.

> **Service-name drift (verify before running).** `deploy-botho.sh` historically
> hardcoded the `botho` unit, `reset-chain.sh` defaults to `botho-seed`, and
> `reset-to-testnet.sh` installs `botho-seed.service` — yet the live regional
> relays in #1114 were running under a `botho` unit. `reprovision-relay.sh`
> defaults `--service` to `botho` but **preflights the unit's existence and
> fails loudly** if the named unit is not installed (printing the actual
> `botho*` units present), so it can never silently restart the wrong service.
> Confirm the real unit first: `systemctl list-unit-files | grep botho`.

> **Firewall prerequisite (#1117).** A relay cannot re-peer if the validators'
> gossip port (`17100`) is dropping its IP. The validators carry an
> **un-persisted** iptables lockdown on `:17100` (VPC-only) even while the
> security group shows `0.0.0.0/0` — the SG lies, iptables is the real gate, and
> it silently strands external relays. On a `peerCount == 0` timeout the script
> explicitly points at #1117 as the most likely cause. Confirm the relay's
> public IP is `ACCEPT`ed on **every** validator's `:17100` before blaming the
> relay.

## 3. Deploy from the published release artifact (do NOT build on the seeds)

The seeds are **t4g.small** (1.8 GiB RAM, 0 swap) and a release build **OOMs**
on them. The default `deploy-botho.sh` path pulls the prebuilt, checksummed
release artifact from GitHub instead of building on the host:

```bash
# Deploy the tagged v0.6.0 release to a host (pulls the aarch64 artifact,
# verifies checksums, installs, restarts the service):
RELEASE_TAG=v0.6.0 ./infra/seed/deploy-botho.sh ubuntu@<host>
```

- The host's architecture is auto-detected; the aarch64 seeds pull
  `botho-v0.6.0-linux-aarch64.tar.gz` + `checksums-linux-aarch64.txt`, and the
  script runs `sha256sum -c` before installing.
- `RELEASE_TAG` defaults to the **latest** GitHub release when unset; pin it
  explicitly (`RELEASE_TAG=v0.6.0`) for a reproducible reset.
- `--build-on-host` is a **fallback only** (untagged commits). Do NOT use it on
  the t4g.small seeds — it OOMs. If you must deploy an untagged build, build
  once on the faucet (3.7 GiB + a temporary 4 GiB swapfile) and `scp` the
  identical aarch64 binary to the seeds (all boxes are Ubuntu 24.04.3 /
  glibc 2.39, so the binary is portable). See the "Building for low-RAM seed
  boxes" note in `infra/faucet/README.md`.

> **Non-blocking: the release's `Reproducibility Check` job may go red while
> all artifacts publish fine (#996).** The `v0.6.0` release published every
> platform artifact + checksums successfully even though the separate
> `Reproducibility Check` job failed. That job is a determinism audit, not a
> gate on artifact availability — a red check there does **not** block a
> testnet deploy. Verify the artifacts + checksums exist on the release page
> and proceed.

## 4. Single-seed vs multi-seed quorum

A lone seed cannot satisfy the default `recommended` quorum (needs >= 2 nodes),
so minting stalls at *"have 1, need 2"*.

- **Multi-seed (>= 2 nodes, the normal case):** use the committed
  `botho-seed.service` as-is (plain `run` with `[minting] enabled = false` —
  there is no `--relay` CLI flag; relay behavior is simply running without
  minting) and quorum `mode = "recommended"`. Run minting on a dedicated
  validator/faucet node. See the header comments in `botho-seed.service` for
  the exact `ExecStart` variants.

- **Genuine solo bring-up (mint from a self-quorum):** run the node with
  `--mint --mint-threads 1` and set an **explicit self-quorum** in
  `~/.botho/testnet/config.toml` — `members` must contain the node's **own**
  `PEER_ID` (a single self-member), NOT an empty list:

  ```toml
  [network.quorum]
  mode = "explicit"
  threshold = 1
  members = ["<this-node's-own-PEER_ID>"]   # single self-member
  ```

### Quorum config gotchas (learned the hard way, 6.0.0 reset)

- **NEVER use `members = []` with `mode = "explicit"`.** A config of
  `mode=explicit threshold=1 members=[]` is a **degenerate quorum** —
  `threshold` (1) exceeds the member count (0), so the slot can **never**
  externalize and the node sits in `NominatePrepare` forever. The chain
  produces zero blocks even though the minter logs "Submitting minting tx". For
  a multi-node fleet use `mode = "recommended"`; for a real solo node use the
  documented single-self-member config above (`is_solo_mode` needs one
  self-member, not an empty list).

- **Relays MUST set `[minting] enabled = false`.** After the wipe deletes the
  wallet (§2), a node with `[minting] enabled = true` and no wallet
  **crash-loops** on start with `Error: Cannot mine without a wallet`, and the
  systemd unit restarts it endlessly. The **faucet mints** (it has a
  `[wallet]` mnemonic in its config); the **seeds relay** (`enabled = false`).
  Double-check every seed's `config.toml` before restarting the fleet.

## 5. Blocking stale old-protocol nodes (zombie firewall) — #998 / #1000

**The load-bearing lesson of the 6.0.0 reset.** Zombie nodes left running from
a *prior* testnet on an *older* protocol (e.g. `4.0.0`) advertise a **higher
(old-chain) height**. That higher-height advertisement **holds the fresh
minter's propose-gate closed**: `should_propose_this_round` sees a peer
claiming a height above the fresh chain, concludes initial sync is incomplete,
and **withholds the minter's proposal** — so the chain **wedges producing zero
blocks even though the minter logs "Submitting minting tx"** every ~0.5s.

The node *does* disconnect these peers as consensus-incompatible (protocol
major mismatch), but their **connect-churn + height gossip stalls SCP first**,
before the disconnect takes effect. Every reconnect (~every 5s) also churns a
quorum reconfiguration.

**Best practice: decommission the old nodes BEFORE the reset.** As a
belt-and-suspenders measure, firewall gossip (port `17100`) to an explicit
peer allowlist on **ALL** validators. This is now scripted and persisted —
run `gossip-firewall.sh apply` on each validator host instead of hand-typing
`iptables` commands (the old copy-paste block was never persisted and never
updated for the eu/ap relays, which silently stranded them in #1114):

```bash
# ON each validator host (seed / seed2 / faucet), as an OPERATOR with sudo.
# Edit the allowlist first (see gossip-peers.conf), then apply + persist:
sudo ./gossip-firewall.sh apply

# Preview without touching iptables (no root needed, safe anywhere):
./gossip-firewall.sh --dry-run apply

# Is :17100 locked down AND durable across reboot?
./gossip-firewall.sh status

# Revert :17100 to the SG default (open), e.g. when decommissioning:
sudo ./gossip-firewall.sh remove
```

The allowlist lives in [`gossip-peers.conf`](./gossip-peers.conf) (edit that
one file when topology changes) and currently covers:

| Peer | IP | Notes |
|------|----|----|
| eu.seed.botho.io | `3.77.150.19` (public) | EU relay (Frankfurt), #613 — on a separate host, cannot reach the US validators over the VPC, so must be allowlisted by public IP |
| ap.seed.botho.io | `3.0.209.59` (public) | AP relay (Singapore), #613 — same reasoning |
| seed / seed2 / faucet | `172.31.x.y` (internal VPC) | The three US validators — internal IPs are VPC-private and rotation-prone, so they are **not** committed; the operator fills them into `gossip-peers.conf` before applying (see the file's comments) |

`127.0.0.1` is always allowed by the script. This drops all gossip from
non-allowlisted peers so zombies can no longer connect, churn, or gossip their
stale height. Leave RPC (`17101`) alone.

> **Scope.** Editing `gossip-peers.conf` and the script itself is repo work;
> running `apply`/`remove` against a live validator mutates the kernel firewall
> and is an **operator** action (SSH + sudo on the host), never done by CI —
> the same split this runbook's header calls out for the reset scripts.

> **Tracking / why keep it now that #1000 shipped.** #998 was the original
> wedge incident; #1000 (the propose-gate hardening that excludes a
> consensus-incompatible peer's height advertisement from the sync/propose
> determination) **shipped on 2026-07-16**, so the *specific* propose-gate
> wedge this firewall once mitigated is fixed in the node itself. The firewall
> is **retained deliberately** as defense-in-depth: the validators are private
> consensus infrastructure, so peer-allowlisting `:17100` keeps hostile/zombie
> gossip and connect-churn off the fleet in general (not just the #998 bug),
> and makes the iptables gate declarative + reboot-durable so it can no longer
> silently drift from the AWS SG (the #1114 / #1117 failure mode). Do **not**
> read "#1000 shipped" as "remove the firewall" — the policy is now
> peer-allowlist by design.

## 6. Wedge recovery (chain stops producing after a reset)

The exact procedure that recovered the 6.0.0 reset. If the chain stops
producing blocks after a reset:

1. **Diagnose via local RPC** (see §7 for the exact command). A wedge shows:
   - `chainHeight` frozen (not climbing),
   - `scpSlotPhase = NominatePrepare` (stuck nominating),
   - `lastExternalizedSecondsAgo` climbing unbounded,
   - `slotStalled` eventually `true`.
2. **Grep logs for stale old-protocol peers** — e.g.
   `journalctl -u botho-seed | grep -i "peer_version=4.0.0"`; the tell is
   `peer_version=4.0.0 ... disconnecting` (consensus-incompatible) repeating
   every few seconds.
3. **Firewall the zombies off on ALL validators** with the §5 gossip firewall
   (`sudo ./gossip-firewall.sh apply` on each host).
4. **Confirm no degenerate quorum** — verify no node has
   `[network.quorum] mode=explicit threshold=1 members=[]` (§4). A degenerate
   quorum wedges identically and will NOT be fixed by the firewall.
5. **Clean restart in order: faucet (hub) first, then the seeds** —
   `sudo systemctl restart botho-faucet` (or the faucet unit), wait for it to
   be up and minting, then restart each seed (`sudo systemctl restart
   botho-seed`).
6. **Verify recovery**: `chainHeight` climbs, and **all nodes agree on one
   `tipHash`** at the same height (query each host's local RPC per §7).

> **What the 6.0.0 reset taught us (#998, resolved).** The wedge that looked
> like a code bug was **entirely operational**: (a) the stale old-protocol
> zombie nodes above held the propose-gate closed, and (b) an incorrect
> `[network.quorum] members=[]` set during debugging is a *degenerate* quorum
> that can never externalize — which manufactured the misleading "stalls even
> solo" symptom. The 6.0.0 code is **not** at fault: an offline investigation
> reproduced a fresh 6.0.0 genesis minting 12+ successive blocks with no wedge
> (regression tests in #1001). So if steps 1-6 don't recover, the cause is
> almost certainly still operational — re-check for a degenerate/`members=[]`
> quorum and any un-firewalled old-protocol peer before suspecting the code.
> The one genuine code hardening from the incident is #1000 (a
> consensus-incompatible peer's height gossip should not stall the minter).

## 7. Verifying a node via LOCAL RPC (not public HTTPS)

**Always verify against the node's LOCAL RPC over SSH, not the public HTTPS
endpoint** — nginx and DNS can cache stale responses and mask a wedge or a
half-deployed binary:

```bash
ssh <host> "curl -s localhost:17101/rpc -H 'content-type: application/json' \
  -d '{\"jsonrpc\":\"2.0\",\"method\":\"node_getStatus\",\"params\":{},\"id\":1}'" | jq .result
```

Key fields to check:

| Field | Expect | Meaning |
|-------|--------|---------|
| `nodeVersion` | the deployed release (e.g. `0.6.0`) | confirms the new binary is actually running |
| `chainHeight` | climbing between polls | chain is producing blocks (frozen = wedge, see §6) |
| `synced` | `true` | node has caught up to the tip |
| `tipHash` | **identical across all nodes** at the same height | fleet agrees on one chain (divergence = fork) |
| `peerCount` | >= 1 on a multi-node fleet | node is connected to the fleet |
| `mintingActive` | `true` on the faucet, `false` on relays | only the faucet mints (§4) |
| `scpSlotPhase` | advancing (not stuck in `NominatePrepare`) | SCP is externalizing slots |

## 8. Multi-seed bootstrap config (scaffolding)

PLAN.md "Network Bootstrap Strategy" calls for >= 3 geographically diverse
seeds plus DNS-seed discovery and a hardcoded fallback.

What already exists in code:

- **DNS-seed discovery** — `botho/src/network/dns_seeds.rs`
  (`seeds.botho.io` / `seeds.testnet.botho.io` TXT records, TTL caching, DNS
  failure falls back to hardcoded seeds).
- **PEX** — `botho/src/network/pex.rs` (decentralized peer exchange).
- **Bootstrap order** — explicit `config.bootstrap_peers` -> DNS -> hardcoded
  fallback (`NetworkConfig::bootstrap_peers_async`).

Single source of truth for the hardcoded fallback:

- **`botho/src/network/seeds.rs`** — `config.rs` and `dns_seeds.rs` both
  delegate here, so the two lists can no longer drift. It defines regional seed
  scaffolding for three regions (`us`, `eu`, `ap`) for both networks, **gated
  off by default** behind `BOTHO_REGIONAL_SEEDS=1` because the regional DNS
  records are not yet live.

### Activating regional seeds (operator, when infra exists)

Preferred path (zero client release): publish the new seeds as
`seeds.testnet.botho.io` TXT records (`PEER_ID@host:port`); DNS discovery picks
them up automatically.

Fallback path: launch `us.seed.botho.io`, `eu.seed.botho.io`,
`ap.seed.botho.io` and start nodes with `BOTHO_REGIONAL_SEEDS=1` so the
hardcoded fallback includes them.

## 9. Reproducible release build (verified path)

- Workflow: `.github/workflows/release.yml` — triggers on `v*` tags, and
  supports `workflow_dispatch` with a `dry_run` input that builds without
  publishing a release.
- Script: `scripts/build-release.sh` — builds `botho`, `botho-wallet`,
  `botho-exchange-scanner` with pinned `SOURCE_DATE_EPOCH`, isolated
  `CARGO_HOME`, `LC_ALL=C.UTF-8`, `TZ=UTC`, `CARGO_INCREMENTAL=0` for
  reproducibility, then emits SHA256 checksums + `build-info.txt`.
- Deploy hosts are **linux-aarch64** (ARM64 Ubuntu); the release matrix builds
  that target natively (`ubuntu-24.04-arm`).
- Build prerequisites are `build-essential cmake pkg-config libssl-dev` plus a
  Rust toolchain — **`cmake` is mandatory** (`randomx-rs` compiles RandomX's
  C++ via a cmake build script and the build aborts without it).

The published `v0.6.0` release is the current artifact source (§3). **Cutting a
fresh tag is an operator action.** To validate the build path without
publishing, run `workflow_dispatch` with `dry_run=true`, or locally:
`./scripts/build-release.sh`. Note the `Reproducibility Check` job may fail
while all artifacts still publish (#996) — non-blocking for a testnet deploy.

## Operator steps remaining (NOT in this PR — require AWS/DNS/credentials)

> **The fleet node list — enumerate EVERY host, including the regional relays.**
> A coordinated reset must touch **all five** live nodes, not just "the seeds":
> the two SCP validators/faucet **and** the two regional relays
> `eu.seed.botho.io` and `ap.seed.botho.io`. In #1114 the eu/ap relays were left
> behind on the old chain because the runbook spoke generically of "the fleet" /
> "every host" without naming them — do not repeat that. If you only need to
> bring a **single** stranded relay back onto the current chain (rather than
> resetting the whole fleet), use `reprovision-relay.sh` (§2) instead of the
> full sequence below.

1. **Decommission any old-protocol nodes from the prior testnet BEFORE the
   reset** (§5). If you cannot, prepare the gossip firewall to apply on every
   fleet node at reset time — old zombies will otherwise wedge the fresh chain.
2. **Deploy the current release binary** to every host — the validators/faucet
   **and** `eu.seed.botho.io` / `ap.seed.botho.io` — from the published
   artifact: `RELEASE_TAG=v0.6.0 ./infra/seed/deploy-botho.sh ubuntu@<host>`
   (§3). Do NOT `--build-on-host` on the t4g.small seeds (OOM).
3. **Reset the chain** to fresh genesis on the current protocol version
   (`reset-chain.sh` / `reset-to-testnet.sh` against the live host). Confirm
   relays have `[minting] enabled = false` and no node has a degenerate
   `members = []` quorum (§4).
4. **Bring up the fleet in order** — faucet (hub, mints) first, then the seeds
   (relay). Switch quorum to `recommended` for a multi-node fleet, and either
   publish DNS TXT seeds or set `BOTHO_REGIONAL_SEEDS=1`.
5. **Verify via local RPC on every host** (§7): `nodeVersion` matches the
   deploy, `chainHeight` climbs, and all nodes agree on one `tipHash`. If the
   chain wedges, run the §6 wedge-recovery procedure.
6. **Restore the faucet service** (`infra/faucet/`) and confirm reachability.
7. **CloudWatch monitoring + Route53 DNS failover** (PLAN.md "Seed Node" /
   "Disaster Recovery").
8. **Point web wallet + ledger browser** at the new RPC; verify end-to-end.
</content>
</invoke>
