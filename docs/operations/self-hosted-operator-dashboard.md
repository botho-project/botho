# Self-Hosting the Operator Dashboard

This runbook explains how a quorum operator can **build, verify, and self-host**
the Botho operator dashboard (`/operator`) instead of trusting the shared
Cloudflare Pages deploy at `botho.io` / `wallet.botho.io`.

It is the operator-facing companion to
[`docs/security/quorum-write-path.md`](../security/quorum-write-path.md) §8.3.1
(bundle-integrity hardening) and parallels the node-binary procedure in
[`reproducible-builds.md`](./reproducible-builds.md).

## Why this exists

The operator dashboard imports your operator Ed25519 signing key into the
browser (encrypted under a mandatory passphrase, session-only, never sent to any
host) and signs quorum-curation envelopes client-side. The serious residual risk
(§8.3) is a **malicious bundle**: a compromised host could serve JavaScript that
prompts you to sign attacker-chosen bytes.

Three mitigations reduce this to near-zero for a careful operator:

1. **Browser-enforced SRI (option (a), #772), on by default.** `/operator` is
   its own build entry (`operator.html`) whose HTML pins `integrity="sha384-…"`
   hashes for every JS/CSS chunk it loads. Any tampered chunk fails the browser's
   own integrity check and never executes — no operator action required. This
   protects the operator entry's *sub-resources* even on the shared Pages deploy.
   Its one residual gap: the browser does not integrity-check the top-level
   `operator.html` document itself (SRI never does), so mitigations 2–3 remain
   worthwhile.
2. **Verify the bundle** you are about to run against a maintainer-published
   hash, *before* you import your key. A tampered bundle produces a different
   hash and is caught. This covers the top-level document that SRI cannot.
3. **Self-host** that verified bundle from infrastructure you control, so the
   shared Pages host is no longer in your trust path at all.

This runbook covers (2) and (3); (1) is automatic. Doing (2) alone already
closes most of the risk on top of (1); (3) removes the host entirely and is
recommended for mainnet.

## Prerequisites

- Git
- Node.js + [pnpm](https://pnpm.io/) (the repo uses pnpm workspaces under `web/`)
- A trusted machine to build on
- The maintainer-published bundle hash for the release you intend to run
  (see [Where to find the published hash](#where-to-find-the-published-hash))

## Step 1: Clone and check out a pinned commit

Never build from a moving branch. Check out the exact tag/commit whose bundle
hash the maintainer published.

```bash
git clone https://github.com/botho-project/botho.git
cd botho
git checkout <tag-or-commit>   # e.g. v0.3.2
git rev-parse HEAD             # record this — it's what you're trusting
```

## Step 2: Build the bundle

```bash
cd web
pnpm install --frozen-lockfile

# Build the operator dashboard bundle. The build is multi-page: the marketing/
# wallet SPA is `dist/index.html`, and the operator dashboard is its own entry,
# `dist/operator.html`, whose referenced chunks carry SRI integrity hashes.
# Include the in-browser signer wasm so the signing path actually works:
pnpm --filter @botho/web-wallet build:all
```

The build output lands in `web/packages/web-wallet/dist/`. Two HTML entry
documents are emitted: `dist/index.html` (main SPA) and `dist/operator.html`
(the standalone, SRI-pinned operator dashboard — this is what `/operator`
serves).

> `build:all` runs `build:wasm` then `build`. If you only need to *inspect*
> the bundle hash and not sign, plain `pnpm --filter @botho/web-wallet build`
> is enough — but self-hosting for real use requires the wasm signer, so
> prefer `build:all`.

## Step 3: Verify the bundle hash

Compute the aggregate bundle hash and compare it to the maintainer-published
value **before you trust the bundle with your key**:

```bash
# From the repo root (or web/):
web/scripts/verify-operator-bundle.sh --expected sha256-<published-value>
```

Or via the package script:

```bash
pnpm --filter @botho/web-wallet verify:operator-bundle -- --expected sha256-<published-value>
```

The script hashes every file in `dist/` (excluding source maps), then computes a
single SHA-256 over the sorted per-file checksum list. On a match it prints
`MATCH` and exits 0; on a mismatch it prints `MISMATCH`, tells you **not** to
import your key, and exits 2.

To see the full per-file listing (useful for auditing exactly what is in the
bundle):

```bash
web/scripts/verify-operator-bundle.sh --manifest
```

### Confirm the operator entry's browser-enforced SRI (option (a))

Independently of the aggregate hash, you can confirm the operator entry pins a
correct `sha384` integrity hash on each chunk it references — the scriptable
equivalent of the check the browser performs on load:

```bash
web/scripts/verify-operator-bundle.sh --verify-sri
# or: pnpm --filter @botho/web-wallet verify:operator-sri
```

It parses `dist/operator.html`, recomputes the `sha384` of each referenced chunk
from disk, and exits 3 if any reference is missing an `integrity` attribute or
the hash does not match. This is what protects you on the *shared* deploy even
without self-hosting: a compromised host that swaps a chunk cannot match the
pinned hash, and the browser refuses to run it.

### Reproducibility

The bundle hash is deterministic: building the same pinned commit in two clean
checkouts on the same toolchain produces the identical aggregate hash. If your
hash does not match the published value:

1. Confirm `git rev-parse HEAD` matches the published commit.
2. Confirm `git status` is clean (no local modifications).
3. Confirm you ran `pnpm install --frozen-lockfile` (lockfile-pinned deps).
4. Re-run the build in a fresh `dist/` (`rm -rf web/packages/web-wallet/dist`).

If it still differs, **do not proceed** — treat it as a potentially tampered
source tree or a toolchain drift and report it.

## Step 4: Serve the verified bundle

Serve `web/packages/web-wallet/dist/` as static files from infrastructure you
control. Any static file server works; the dashboard is a client-side SPA that
talks to a node's read RPC / `operator_submitAction` endpoint over the network.

Two examples:

```bash
# Option A: local-only preview (quickest — binds localhost)
cd web
pnpm --filter @botho/web-wallet preview   # serves dist/ on http://localhost:4173

# Option B: any static file server you trust, on infra you control
#   (nginx, caddy, `python3 -m http.server`, an internal Pages/Netlify you own, …)
#   Point it at web/packages/web-wallet/dist/. See the routing note below for
#   how to map /operator to operator.html and everything else to index.html.
```

Because the build is now multi-page, configure your server's client-side-routing
fallback carefully:

- `/operator` (and any `/<locale>/operator`, e.g. `/es/operator`) must serve
  **`operator.html`** — the standalone, SRI-pinned operator document.
- All other unknown paths serve **`index.html`** — the main SPA fallback.

Serving `index.html` for `/operator` would load the un-SRI-pinned main SPA route
instead of the split operator entry, silently discarding the browser-enforced
integrity. Example nginx:

```nginx
location = /operator      { try_files /operator.html =404; }
location ~ ^/[a-z]{2}/operator$ { try_files /operator.html =404; }
location /                { try_files $uri /index.html; }
```

> **RPC reachability.** The bundle reaches a node's RPC directly. For local
> `pnpm preview`, the Vite preview proxy forwards `/rpc` to a seed node (see
> `vite.config.ts`); for a custom static host you must ensure the browser can
> reach your node's RPC (same-origin proxy or CORS), exactly as the shared
> deploy does.

## Step 5: Handle PWA auto-update (important)

The main SPA registers a service worker with `registerType: 'autoUpdate'`
(`vite.config.ts`). On the shared deploy this silently fetches and activates a
**newer** bundle after each deploy — which would defeat the whole point of
pinning a verified bundle.

> **Note (option (a), #772):** the operator entry is deliberately kept *out* of
> the service worker. `operator-main.tsx` does not register the SW, and
> `operator.html` is excluded from the Workbox precache
> (`globIgnores: ['operator.html']`), so the operator document is always fetched
> fresh over the network and cannot be silently swapped from the SW cache. The
> guidance below still matters for the *rest* of the bundle and for the chunks
> the operator entry loads.

When self-hosting, keep the served `dist/` **fixed**: serve one verified build
and do not roll a newer one behind the same origin. With no newer bundle
available, the service worker has nothing to pull, so your verified bundle stays
in force. The service-worker files (`sw.js`, `workbox-*.js`) are themselves part
of the verified trust set — `verify-operator-bundle.sh` hashes them — so a
tampered service worker changes the aggregate hash and is caught by Step 3.

If you *intentionally* upgrade to a newer release:

1. Repeat Steps 1–3 at the new pinned commit against the new published hash.
2. Replace the served `dist/` only after the new bundle verifies.
3. Clear the old service worker / hard-refresh so the new bundle takes control.

## Step 6: Confirm the dashboard works

Load your self-hosted URL, open `/operator`, and confirm:

- The **Actions** tab loads and lets you import your operator key (under a
  passphrase).
- The **dry-run preview** renders the canonical envelope before any signature
  (this is the §8.3 mitigation — always read it and confirm the bytes are what
  you intend before signing).
- A dry-run against your real node returns the expected verdict.

Only import your operator key into a bundle whose hash you verified in Step 3.

## Where to find the published hash

Maintainers publish the operator bundle hash for a release alongside the release
artifacts (the same channel as `SHA256SUMS.txt` for node binaries — see
[`reproducible-builds.md`](./reproducible-builds.md)). To publish one yourself as
a maintainer:

```bash
git checkout <tag>
cd web && pnpm install --frozen-lockfile && pnpm --filter @botho/web-wallet build:all
web/scripts/verify-operator-bundle.sh     # prints: operator bundle hash: sha256-<...>
```

Publish that `sha256-<...>` value next to the release for the pinned commit.

## What this does and does not remove from the trust path

**Removed** by browser-enforced SRI (automatic, even on the shared deploy):

- Silent tampering of the operator entry's JS/CSS **chunks** — the browser
  refuses any chunk whose bytes don't match the pinned `sha384` hash.

**Removed** by verify + self-host:

- Trust in the shared Cloudflare Pages host serving an honest top-level
  `operator.html` document (the one thing SRI cannot pin).
- Silent bundle replacement via PWA auto-update (when you serve a fixed build).

**Still trusted** (out of scope for this runbook):

- The pinned source commit and its dependency lockfile.
- Your build machine and the toolchain on it.
- The node you submit actions to (its gate + audit log — see §4, §6).

Whether the in-browser key path is the right posture for mainnet at all —
versus an air-gapped signer or hardware key — is a separate security-review
decision tracked as a §8.3 follow-up, not something this runbook settles.
