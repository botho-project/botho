# Botho BaaS — Rig Bootstrap (`infra/baas/`)

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

- `rig-bootstrap.sh` — the entire first-boot provisioner (paste as user-data).
- `bip39-english.txt` — BIP39 wordlist, used to generate a per-rig wallet
  mnemonic non-interactively (the built-in `botho init` is interactive). It is
  the byte-identical wordlist the node's `tiny-bip39` dependency uses.

## What it does (idempotent, logged to `/var/log/botho-rig-bootstrap.log`)

1. **Install deps** — nginx, certbot, curl, jq, ca-certificates.
2. **Obtain the `botho` linux-aarch64 binary** from `BOTHO_BINARY_URL`
   (preferred over building on a t4g box, which is slow / OOM-prone). Optional
   `BOTHO_BINARY_SHA256` is verified; the file is checked to be `aarch64`.
3. **Generate identity + config** — a per-rig 24-word wallet mnemonic and
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
5. **nginx + TLS + `/rpc` proxy** for `RIG_HOSTNAME` (mirrors
   `seed-nginx.conf`: HTTP→HTTPS redirect, CORS de-duplication, `/rpc/ws`).
6. **Emit rig info** to `~/rig-info.txt` and install `rig-status` for read-back.

## Inputs (env vars / user-data exports)

| Variable              | Required | Default            | Purpose |
|-----------------------|----------|--------------------|---------|
| `BOTHO_BINARY_URL`    | yes\*    | —                  | URL to a linux-aarch64 `botho` binary (see **Binary source** below). \*Not required on an idempotent re-run if `/usr/local/bin/botho` already exists. |
| `BOTHO_BINARY_SHA256` | no       | —                  | Expected sha256; verified if set. |
| `RIG_HOSTNAME`        | no       | —                  | Public hostname, e.g. `rig-abc123.testnet.botho.io`. If unset, public nginx/TLS is skipped (RPC still on `localhost:17101`). |
| `NETWORK`             | no       | `testnet`          | Only `testnet` is supported in this slice. |
| `BOOTSTRAP_PEERS`     | no       | DNS-seed discovery | Comma-separated **resolved** multiaddrs (`/ip4/.../tcp/<port>/p2p/<peer_id>`). Default empty -> DNS-seed discovery (the working path; bare `/dns4` is unsupported). |
| `MINT_THREADS`        | no       | `1`                | RandomX threads (t4g.medium = 2 vCPU). |
| `CERTBOT_EMAIL`       | no       | `admin@botho.io`   | Let's Encrypt registration email. |
| `TLS_MODE`            | no       | `webroot`          | `webroot` (needs nginx+DNS) / `standalone` / `skip` (HTTP-only, for local testing). |
| `RIG_WALLET_MNEMONIC` | no       | generated          | Bring-your-own wallet (#458 will decide BYO vs generated). |

## Outputs

- `/var/log/botho-rig-bootstrap.log` — full provisioning log.
- `~ubuntu/.botho/testnet/config.toml` — node config incl. mnemonic (chmod 600).
- `~ubuntu/rig-info.txt` — RPC URL, public IP, binary version, helper commands.
- `botho.service` running and mining; `rig-status` read-back helper.

Read back at any time:

```bash
sudo rig-status                 # network/height/peers/synced/mintingActive
cat ~ubuntu/rig-info.txt
journalctl -u botho -f
```

## Dependencies the provisioner (#458) must satisfy

### 1. Binary source (`BOTHO_BINARY_URL`)

There is currently **no published GitHub release binary** — the latest release
(`v0.2.0`) ships **no assets**, and the release workflow builds artifacts but
the operator hasn't attached/uploaded one. So the bootstrap **cannot fetch a
binary on its own**; it consumes `BOTHO_BINARY_URL`.

**#458 must publish the current `linux-aarch64` `botho` release artifact and
pass its URL**, for example:

- Build via `scripts/build-release.sh` (or the `release.yml` workflow on a tag),
  then upload `target/.../botho` to an S3/R2 object the rig can `GET`, and pass
  that object URL as `BOTHO_BINARY_URL`; or
- Attach the binary to the GitHub release and use the asset's download URL.

Interim stand-in (used for the live verification of this PR): copy the binary
already running on a live seed (it is the exact network build) to a temporary
HTTP location and pass that URL — see the verification notes in the PR.

### 2. DNS pre-creation (`RIG_HOSTNAME`)

The provisioner must create the `RIG_HOSTNAME` A record pointing at the
instance's public IP **before/at boot** so Let's Encrypt (`webroot`/
`standalone`) can validate. If DNS isn't ready when the script runs, certbot is
skipped gracefully and nginx serves HTTP-only `/rpc`; re-running the script
after DNS propagates issues the cert and switches to HTTPS (idempotent). For
DNS-less local testing use `TLS_MODE=skip`.

## Example user-data

```bash
#!/usr/bin/env bash
export RIG_HOSTNAME="rig-abc123.testnet.botho.io"
export BOTHO_BINARY_URL="https://artifacts.botho.io/botho/<rev>/botho-aarch64"
export BOTHO_BINARY_SHA256="<sha256>"
export MINT_THREADS=1
# ... then the contents of rig-bootstrap.sh ...
```

(In practice the provisioner concatenates the exports + `rig-bootstrap.sh` into
the instance's user-data. cloud-init runs it as root on first boot.)

## Target

- `t4g.medium` (arm64, 2 vCPU, ~4 GB; RandomX needs ~2 GB), Ubuntu 24.04 arm64.
- Security group must allow inbound `17100` (gossip), `80`/`443` (nginx/ACME),
  and `22` only for break-glass (the bootstrap needs no inbound SSH).
