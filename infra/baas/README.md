# Botho BaaS — Node Bootstrap (`infra/baas/`)

Self-contained **cloud-init / EC2 user-data** that turns a fresh instance into a
working Botho **testnet mining node** with **zero manual SSH**. This is the
foundational, no-billing slice of the BaaS provisioner (issue #458, phase P6.1;
epic #441) — the automation #458 will invoke for each paid signup.

It is the scripted, idempotent, non-interactive form of the proven manual seed
recipe in `infra/seed/` and `infra/faucet/`:

| Manual asset (reused/transformed)            | Bootstrap step |
|----------------------------------------------|----------------|
| `infra/seed/deploy-botho.sh`                 | Step 2 — obtain the binary (fetch a published URL instead of building on-box) |
| `infra/faucet/faucet-config.toml.template`   | Step 3 — generate `~/.botho/testnet/config.toml` (mint on, faucet off) |
| `infra/seed/botho-seed.service`              | Step 4 — `botho.service` with `--mint` |
| `infra/seed/seed-nginx.conf`                 | Step 5 — nginx + Let's Encrypt + `/rpc` (and `/rpc/ws`) proxy |
| `infra/seed/reset-to-testnet.sh`             | Steps 4–6 — start, verify `network == botho-testnet`, read back |

## Files

- `node-bootstrap.sh` — the entire first-boot provisioner (paste as user-data).
- `bip39-english.txt` — BIP39 wordlist, used to generate a per-node wallet
  mnemonic non-interactively (the built-in `botho init` is interactive). It is
  the byte-identical wordlist the node's `tiny-bip39` dependency uses.

## What it does (idempotent, logged to `/var/log/botho-node-bootstrap.log`)

1. **Install deps** — nginx, certbot, curl, jq, ca-certificates.
2. **Download the prebuilt `botho` linux-aarch64 binary** — from
   `BOTHO_BINARY_URL` if set (the release tarball
   `botho-vX.Y.Z-linux-aarch64.tar.gz`, or a bare-binary mirror), else reuse an
   already-installed `/usr/local/bin/botho`, else **resolve the latest GitHub
   release automatically** and checksum-pin it. Never built from source on the
   box (slow / OOM-prone). Tarballs are extracted (`botho` member); optional
   `BOTHO_BINARY_SHA256` (the `botho` line of `checksums-linux-aarch64.txt`)
   is verified against the (extracted) binary, which is also checked to be
   `aarch64` and installed mode `0755`.
3. **Generate identity + config** — a per-node 24-word wallet mnemonic and
   `~/.botho/testnet/config.toml` (testnet, RandomX minting on, faucet off,
   `quorum.mode = recommended` / `min_peers = 1`, `bootstrap_peers = []`,
   DNS-seed discovery `seeds.testnet.botho.io` enabled). The wallet mnemonic is
   preserved across re-runs. The libp2p **node_key** (peer identity) is created
   and persisted automatically by the binary at `~/.botho/testnet/node_key`, so
   the peer id is stable across reboots with no extra handling.

   > **Peering note (learned during verification):** the node's libp2p
   > transport rejects bare `/dns4/host/tcp/port` entries in explicit
   > `bootstrap_peers` (`MultiaddrNotSupported` → 0 peers). The working path is
   > DNS-seed discovery (empty `bootstrap_peers` + `dns_seeds.enabled = true`),
   > which resolves the seed TXT records to `/ip4/.../p2p/<peer_id>` multiaddrs
   > the transport accepts. An explicit `BOOTSTRAP_PEERS` override must
   > therefore supply resolved `/ip4/.../tcp/<port>/p2p/<peer_id>` multiaddrs.
4. **Install + start** the `botho` systemd unit
   (`botho --testnet run --mint --mint-threads N`).
5. **nginx + TLS + `/rpc` proxy** for `NODE_HOSTNAME` (mirrors
   `seed-nginx.conf`: HTTP→HTTPS redirect, CORS de-duplication, `/rpc/ws`).
6. **Emit node info** to `~/node-info.txt` and install `node-status` for read-back.

## Inputs (env vars / user-data exports)

| Variable              | Required | Default            | Purpose |
|-----------------------|----------|--------------------|---------|
| `BOTHO_BINARY_URL`    | no       | latest GitHub release | URL to the prebuilt linux-aarch64 build: the release tarball `botho-vX.Y.Z-linux-aarch64.tar.gz` (canonical since v0.3.0; the `botho` member is extracted) or a bare `botho` binary (S3/R2 mirror, legacy). When unset, an existing `/usr/local/bin/botho` is reused (idempotent re-run), else the latest GitHub release's tarball is resolved via the GitHub API (see **Binary source** below). |
| `BOTHO_BINARY_SHA256` | no       | auto-pinned in latest-release mode | Expected sha256 of the `botho` **binary**; verified if set. Pass the `botho` line of the release asset `checksums-linux-aarch64.txt` (per-binary digests of the **extracted** files — the tarball's own digest is not published; do **not** use `SHA256SUMS.txt`, which mixes all platforms unlabelled). In bare-binary mode the downloaded file itself is verified. |
| `BOTHO_REPO`          | no       | `botho-project/botho` | GitHub `owner/repo` used for latest-release resolution. |
| `NODE_ID`              | no       | —                  | Short opaque node id (e.g. `abc123`). When set and `NODE_HOSTNAME` is unset, the hostname is derived as `node-<NODE_ID>.<NODE_DOMAIN>`. Recorded in `node-info.txt`. |
| `NODE_DOMAIN`          | no       | `testnet.botho.io` | Zone for `node-<NODE_ID>` hostnames; combined with `NODE_ID` to derive `NODE_HOSTNAME`. |
| `NODE_HOSTNAME`        | no       | —                  | Public hostname, e.g. `node-abc123.testnet.botho.io`. Takes precedence over `NODE_ID`/`NODE_DOMAIN`. If neither is set, public nginx/TLS is skipped (RPC still on `localhost:17101`). |
| `REGION`              | no       | —                  | AWS region the node launched in (informational; the provisioner picks it at run-instances time). Recorded in `node-info.txt`. |
| `TIER`                | no       | `t4g.medium`       | Instance type/tier (informational; MVP is t4g.medium-only). Recorded in `node-info.txt`. |
| `NETWORK`             | no       | `testnet`          | Only `testnet` is supported in this slice. |
| `BOOTSTRAP_PEERS`     | no       | DNS-seed discovery | Comma-separated **resolved** multiaddrs (`/ip4/.../tcp/<port>/p2p/<peer_id>`). Default empty -> DNS-seed discovery (the working path; bare `/dns4` is unsupported). |
| `MINT_THREADS`        | no       | `1`                | RandomX threads (t4g.medium = 2 vCPU). |
| `CERTBOT_EMAIL`       | no       | `admin@botho.io`   | Let's Encrypt registration email. |
| `TLS_MODE`            | no       | `webroot`          | `webroot` (needs nginx+DNS) / `standalone` / `skip` (HTTP-only, for local testing). |
| `NODE_WALLET_MNEMONIC` | no       | generated          | Bring-your-own wallet (#458 will decide BYO vs generated). |

## Outputs

- `/var/log/botho-node-bootstrap.log` — full provisioning log.
- `~ubuntu/.botho/testnet/config.toml` — node config incl. mnemonic (chmod 600).
- `~ubuntu/node-info.txt` — node id, region, tier, RPC URL, public IP, binary
  version, helper commands.
- `botho.service` running and mining; `node-status` read-back helper.

Read back at any time:

```bash
sudo node-status                 # network/height/peers/synced/mintingActive
cat ~ubuntu/node-info.txt
journalctl -u botho -f
```

## Dependencies the provisioner (#458) must satisfy

### 1. Binary source (`BOTHO_BINARY_URL`)

The node **downloads the prebuilt arm64 binary** — it never builds from source on
the box (t4g release builds are slow and RandomX-linked crates can OOM).

Since **v0.3.0** (2026-07-05) the canonical source is the **GitHub release
asset** published by `.github/workflows/release.yml`:

- `botho-vX.Y.Z-linux-aarch64.tar.gz` — gzip tarball with top-level members
  `botho`, `botho-wallet`, `botho-exchange-scanner` (mode `0644` inside the
  archive; the bootstrap extracts `botho` and installs it `0755`).
- `checksums-linux-aarch64.txt` — sha256 digests of each **extracted binary**
  (one per line). **The tarball's own digest is published nowhere**, so in
  tarball mode `BOTHO_BINARY_SHA256` is compared against the extracted `botho`.
- Do **not** verify against `SHA256SUMS.txt`: it is an unlabelled concatenation
  of every platform's checksums, so `botho` appears once per platform with
  conflicting digests. Always use `checksums-<platform>.txt`.

The bootstrap resolves the binary in this order (Step 2 of the script):

1. **Explicit `BOTHO_BINARY_URL`** — the release tarball URL, or a bare
   aarch64 `botho` binary (e.g. an S3/R2 mirror object; legacy path, still
   supported — in that mode the sha256 is of the downloaded file itself).
2. **Existing `/usr/local/bin/botho`** — reused on idempotent re-runs; no
   network access is attempted.
3. **Latest GitHub release fallback** — resolves the newest release of
   `BOTHO_REPO` via `https://api.github.com/repos/<repo>/releases/latest`,
   downloads `botho-<tag>-linux-aarch64.tar.gz`, and (when
   `BOTHO_BINARY_SHA256` is unset) auto-pins the `botho` digest from the same
   release's `checksums-linux-aarch64.txt`. If the GitHub API is unreachable,
   the script fails with instructions to pass `BOTHO_BINARY_URL` explicitly.

So the provisioner (#458 P6.2) may omit both variables entirely (track the
latest release), or pin an exact build:

```bash
export BOTHO_BINARY_URL="https://github.com/botho-project/botho/releases/download/v0.3.0/botho-v0.3.0-linux-aarch64.tar.gz"
# the `botho` line of that release's checksums-linux-aarch64.txt:
export BOTHO_BINARY_SHA256="019f31e8e29cf482567be1c51f65d499aeffda1b63f57098a99106a31053aab1"
```

> **Obsolete guidance (pre-v0.3.0):** earlier releases (≤ `v0.2.0`) shipped no
> downloadable assets, which forced an interim "copy the binary from a live
> seed to a temporary HTTP location" stand-in. That workaround is no longer
> needed and must not be used — release assets are the canonical source
> (see #638, "prefer release artifacts in deploys").

### 2. DNS pre-creation (`NODE_HOSTNAME` / `NODE_ID`)

The provisioner must create the `NODE_HOSTNAME` (or the derived
`node-<NODE_ID>.<NODE_DOMAIN>`) A record pointing at the
instance's public IP **before/at boot** so Let's Encrypt (`webroot`/
`standalone`) can validate. If DNS isn't ready when the script runs, certbot is
skipped gracefully and nginx serves HTTP-only `/rpc`; re-running the script
after DNS propagates issues the cert and switches to HTTPS (idempotent). For
DNS-less local testing use `TLS_MODE=skip`.

## Example user-data

```bash
#!/usr/bin/env bash
export NODE_ID="abc123"                 # -> node-abc123.testnet.botho.io
export REGION="us-west-2"
export TIER="t4g.medium"
# Binary: omit both exports to track the latest GitHub release (auto-pinned
# from checksums-linux-aarch64.txt), or pin an exact release build:
export BOTHO_BINARY_URL="https://github.com/botho-project/botho/releases/download/v0.3.0/botho-v0.3.0-linux-aarch64.tar.gz"
export BOTHO_BINARY_SHA256="<the 'botho' line of that release's checksums-linux-aarch64.txt>"
export MINT_THREADS=1
# ... then the contents of node-bootstrap.sh ...
```

(Pass `NODE_HOSTNAME` directly instead of `NODE_ID`/`NODE_DOMAIN` if you want a
fully custom hostname.)

(In practice the provisioner concatenates the exports + `node-bootstrap.sh` into
the instance's user-data. cloud-init runs it as root on first boot.)

## Target

- `t4g.medium` (arm64, 2 vCPU, ~4 GB; RandomX needs ~2 GB), Ubuntu 24.04 arm64.
- Security group must allow inbound `17100` (gossip), `80`/`443` (nginx/ACME),
  and `22` only for break-glass (the bootstrap needs no inbound SSH).

## Operator validation runbook (protocol 4.0.0 / release-asset flow)

> **Status: pending live validation.** This runbook is the end-to-end check for
> issue #652 — execute it after any change to the binary-acquisition flow (and
> after protocol resets) and record the results on the tracking issue. It
> requires AWS access and is **not** exercised by CI.

### 1. Launch

Launch a fresh instance (matching the provisioner's parameters, #502):

- **AMI**: Ubuntu 24.04 LTS arm64 (`ubuntu/images/hvm-ssd-gp3/ubuntu-noble-24.04-arm64-server-*`).
- **Type**: `t4g.medium`; root volume ≥ 16 GB gp3.
- **Security group**: inbound `17100/tcp` (gossip), `80`/`443` (only if testing
  TLS), `22` for break-glass.
- **User data**: the exports below followed by the full contents of
  `node-bootstrap.sh`:

```bash
#!/usr/bin/env bash
export TIER="t4g.medium"
export TLS_MODE="skip"       # DNS-less validation; use NODE_ID + webroot to also test TLS
# No BOTHO_BINARY_URL / BOTHO_BINARY_SHA256: exercises the latest-release
# resolution + auto checksum pinning (the default provisioner path).
# ... contents of node-bootstrap.sh ...
```

### 2. Verify provisioning

SSH in (break-glass) and check:

```bash
sudo tail -50 /var/log/botho-node-bootstrap.log
# Expect: "latest release: vX.Y.Z", "pinned sha256 from checksums-linux-aarch64.txt: ...",
#         "gzip tarball detected; extracting 'botho' member", "sha256 verified: ...",
#         "installed botho (aarch64)", "=== Botho node bootstrap complete ==="
ls -l /usr/local/bin/botho      # mode 0755
systemctl is-active botho       # active
```

### 3. Verify the node joined protocol-4.0.0 testnet and mints

```bash
# Network / sync / peers / minting (node_getStatus):
curl -s -X POST http://localhost:17101 -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"node_getStatus","params":{},"id":1}' \
  | jq '{network:.result.network, height:.result.chainHeight, peers:.result.peerCount,
         synced:.result.synced, syncStatus:.result.syncStatus,
         mintingActive:.result.mintingActive, nodeVersion:.result.nodeVersion}'

# Wire-protocol version (node_getIdentity — protocolVersion is NOT in node_getStatus):
curl -s -X POST http://localhost:17101 -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"node_getIdentity","params":{},"id":1}' \
  | jq '{protocolVersion:.result.protocolVersion, network:.result.network,
         dnsSeedDomain:.result.dnsSeedDomain}'
```

Pass criteria (record actual values on the tracking issue):

- [ ] `network` = `botho-testnet` (both calls).
- [ ] `protocolVersion` = `4.0.0`.
- [ ] `peerCount` ≥ 1 within a few minutes — peers discovered via DNS-seed
      discovery (`seeds.testnet.botho.io`; seed/seed2/faucet). No
      `BOOTSTRAP_PEERS` needed.
- [ ] `synced` = `true` and `chainHeight` tracks the live cluster tip
      (compare against a seed's `/rpc`).
- [ ] `mintingActive` = `true` (RandomX; 1 thread by default) and, after a
      while, `journalctl -u botho` shows minting activity with no restarts.

### 4. Idempotency spot-check

Re-run the script on the same box (`sudo bash node-bootstrap.sh` with the same
env): Step 2 must log `reusing existing /usr/local/bin/botho (idempotent
re-run)` (no download/API call), the wallet mnemonic must be preserved, and the
service must come back healthy.
