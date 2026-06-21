# Botho Web

Monorepo for Botho's web apps:

- **`packages/web-wallet`** (`@botho/web-wallet`) — the public web wallet and
  ledger browser / block explorer served at `https://botho.io`. Routes:
  `/` (landing), `/wallet`, `/explorer`, `/explorer/block/:hash`,
  `/explorer/tx/:hash`, `/docs`.
- **`packages/desktop`** (`@botho/desktop`) — the desktop (Tauri) wallet UI.
- **`packages/features`** — shared feature components, including the explorer
  (`src/explorer/*`).
- **`packages/adapters`** — node adapters, including the JSON-RPC
  `RemoteNodeAdapter`.
- **`packages/core`**, **`packages/ui`** — shared core logic and UI components.

## Prerequisites

- Node.js 20+
- [pnpm](https://pnpm.io/) (the workspace uses pnpm workspaces)

## Install & build

```bash
cd web
pnpm install
pnpm build        # builds all packages
pnpm typecheck    # type-checks all packages
pnpm test:run     # vitest unit tests
```

## RPC endpoint configuration

The web wallet and explorer read chain data over JSON-RPC 2.0. The default
testnet endpoint is the seed node's CORS-enabled read RPC:

```
https://seed.botho.io/rpc
```

This is configured in `packages/web-wallet/src/config/networks.ts` and can be
overridden at build time:

```bash
# Point the wallet/explorer at a different RPC (absolute or same-origin path)
VITE_RPC_ENDPOINT=https://my-node.example/rpc pnpm build:web

# Use a same-origin proxy path (recommended for local dev / e2e to avoid CORS).
# The wallet's vite config proxies /rpc -> https://seed.botho.io.
VITE_RPC_ENDPOINT=/rpc pnpm build:web
```

See `docs/api.md` ("Public testnet endpoint") for endpoint details and the CORS
requirements enforced by `infra/seed/seed-nginx.conf`.

The `RemoteNodeAdapter` accepts both absolute endpoints
(`https://seed.botho.io/rpc`) and relative same-origin paths (`/rpc`); the
latter is resolved against the page origin.

## End-to-end tests (Playwright)

E2E specs live under `e2e/` (smoke, wallet, explorer, faucet).

The `web-wallet` Playwright project (`e2e/tests/wallet/**`) covers the recently
shipped wallet flows: wallet create/import, **request → pay**, **share my
address → pay**, the **contacts** manager (add/edit/delete/search + labeling),
the Send-form contact picker, and a Receive-QR smoke check. These run against the
**hermetic mock RPC** (`e2e/serve-rpc-mock.mjs`) — **no live node or faucet
required**. Because the mock has no `tx_submit`, the pay/contacts specs assert the
**pre-fill + confirm UI** (recipient/amount populated, the Pay button primed)
rather than on-chain settlement; real submission against a live node is covered by
the full-stack send spec (see below). Wallets are encrypted by default (#475), so
the specs always set a password when creating/importing
(`e2e/fixtures/wallet-setup.ts`).

```bash
# From web/ — install browsers once, then run only the wallet flows:
npx playwright install chromium chromium-headless-shell
pnpm test:e2e --project=web-wallet
```

These are runnable in CI without external infra (they build + preview the wallet
and mock `/rpc`), so the `web-wallet` project can join the existing
`pnpm test:e2e` gate. The **full-stack** send spec (`e2e/tests/fullstack/**`) is
the only wallet e2e that needs a real local node + wasm build and stays gated
behind `E2E_FULLSTACK=1` — it is **local-run**, not in default CI.

By default the suite is **self-contained**: it builds the web wallet (with
`VITE_RPC_ENDPOINT=/rpc`), serves it via `vite preview` on
`http://localhost:4173`, serves the faucet static site
(`infra/faucet/web`) on `http://localhost:4174`, and points the wallet/explorer
at the live seed node through the same-origin `/rpc` proxy. This means the suite
does not depend on `https://botho.io` / `https://faucet.botho.io` being up.

```bash
# Install the Playwright browser once
npx playwright install chromium

# Run the smoke suite
pnpm test:e2e:smoke

# Run everything
pnpm test:e2e
```

### Running options (env vars)

| Variable | Default | Purpose |
| --- | --- | --- |
| `E2E_WEB_BASE_URL` | `http://localhost:4173` | Point wallet/explorer specs at a remote deployment (e.g. `https://botho.io`). Disables the local web server. |
| `E2E_FAUCET_BASE_URL` | `http://localhost:4174` | Point faucet specs at a remote deployment. Disables the local faucet server. |
| `E2E_BROWSER_CHANNEL` | _(bundled chromium)_ | Set to `chrome` to use the system-installed Google Chrome instead of Playwright's bundled browser (useful where the bundled browser cannot be downloaded). |
| `E2E_VIDEO` | `retain-on-failure` | Set to `off` in environments without Playwright's ffmpeg build. |

Example (run against the live deployment using system Chrome):

```bash
E2E_WEB_BASE_URL=https://botho.io \
E2E_FAUCET_BASE_URL=https://faucet.botho.io \
E2E_BROWSER_CHANNEL=chrome \
  pnpm test:e2e
```

### Live-smoke suite (deployed wallet + live testnet)

The hermetic suite above can never catch a **deploy** regression (it builds the
app fresh and mocks `/rpc`). The live-smoke suite (`e2e/tests/live/smoke.spec.ts`)
drives the **deployed** wallet at `https://wallet.botho.io` against the **live**
testnet SCP nodes, catching exactly the class of bug a hermetic run misses — e.g.
the wasm-404 when a build lands on a Pages preview instead of production and
`/pkg/bth_wasm_signer_bg.wasm` stops being served.

It is **opt-in** and uses a **separate** config
(`e2e/playwright.live.config.ts`) that starts **no** local servers. It is NOT in
default CI (it hits live infra + the faucet's rate limits), and the default
`pnpm test` / `pnpm test:e2e` never run it (the default config ignores
`tests/live/**`).

```bash
# From web/
npx playwright install chromium   # once

BOTHO_LIVE=1 npx playwright test --config e2e/playwright.live.config.ts
```

It asserts: page + title load (HTTP 200), the WASM crypto module loads with no
wasm console error / no failed `/pkg/*` fetch, the PWA manifest + service worker
are served, the 3-node ingress picker lists seed/seed2/faucet and switching the
selected node updates (and persists) state, wallet create + import work entirely
client-side (address renders), and `/claim` loads its empty/invalid state.

| Variable | Default | Purpose |
| --- | --- | --- |
| `BOTHO_LIVE` | _(unset)_ | Must be `1` or the whole live suite is skipped. |
| `BOTHO_LIVE_URL` | `https://wallet.botho.io` | Override the deployed wallet URL under test. |

No default on-chain steps run (testnet mints on demand + faucet rate limits +
~30–80s block time make a full faucet→send→claim cycle flaky); any such step
should be gated separately and kept tolerant/skippable.

## Deploy

```bash
pnpm deploy           # build web wallet + deploy to Cloudflare Pages
pnpm deploy:preview   # deploy a preview build
```
