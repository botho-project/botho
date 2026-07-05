# Block Explorer + Network Stats: Architecture Proposal

## Status

**Proposed** — Architecture decision document for two user-facing surfaces
(public block explorer and network stats/health dashboard) that have no home
today. Written 2026-07-05 as the handoff focus (`NEXT_SESSION.md`) shifts from
mainnet-blockers to user-facing surfaces. No code changes yet; this document
makes the placement and storage decisions and hands off a phased implementation
breakdown. Resolves the design deliverable in #633.

## Summary

Botho has no public block explorer and no cross-node stats dashboard. This
proposal makes three decisions and grounds them in the **actual repo state as it
exists on `main` today** (verified against source and the live EU node
`http://3.77.150.19:17101` on 2026-07-05):

1. **Explorer placement** → **Ship the explorer that already exists inside
   `@botho/web-wallet`.** It is not "code to be built" or "code to be placed" —
   it is already wired at `/explorer`, `/explorer/block/:hash`,
   `/explorer/tx/:hash` in `web/packages/web-wallet/src/pages/explorer.tsx`,
   backed by `@botho/features` explorer components and a `RemoteNodeAdapter`
   data source, with a working `NetworkSelector`. The work is
   surfacing/linking + privacy hardening, **not** a new package and **not**
   extending the vanilla-JS seed page.

2. **Stats / health dashboard placement** → **Extend the existing
   `infra/faucet/metrics-daemon` into a multi-node aggregator, and render it as
   a `/stats` route inside `@botho/web-wallet`.** Keep Grafana as the
   operator-facing surface (unchanged). Do not stand up a new service.

3. **History storage** → **Server-side, in the metrics-daemon SQLite store,
   extended to poll all five nodes and to capture block-times, fee-floor,
   cluster-wealth snapshots, and lottery events.** Reject client-side fan-out
   as the source of history (it cannot retain state and multiplies RPC load by
   viewer count).

The rest of this document justifies each decision, gives the package/data-flow
layout, states the privacy rules the explorer must obey, specifies the
M2-relevant views, places the chain-height staleness indicator, and closes with
a filed-ready phased issue breakdown.

---

## Ground Truth: What Already Exists

This section is the load-bearing part of the proposal. The placement decisions
follow directly from it. Everything here was verified against source on `main`
and, where marked (live), against the EU node RPC on 2026-07-05.

### The explorer is already built AND already wired into the web wallet

`web/packages/features/src/explorer/` is a complete React explorer library:
`Explorer`, `SearchBar`, `BlockList`, `BlockDetail`, `TransactionDetail`,
`ErrorMessage`, `DetailRow`, plus `ExplorerProvider`/`useExplorer` context
(loading, error, pagination, real-time block subscription, search) and a
pluggable `ExplorerDataSource` interface (`getRecentBlocks`, `getBlock`,
`getTransaction`, optional `onNewBlock`).

Crucially, it is **not** an unplaced library. `@botho/web-wallet` already
consumes it:

- `web/packages/web-wallet/src/App.tsx` registers three routes:
  `/explorer`, `/explorer/block/:hash`, `/explorer/tx/:hash`.
- `web/packages/web-wallet/src/pages/explorer.tsx` (76 lines) wires the
  `ExplorerProvider` to `adapter.getRecentBlocks/getBlock/getTransaction/onNewBlock`,
  drives path-based URL sync on view changes, and includes the `NetworkSelector`
  so a viewer can switch which node the explorer reads.

`@botho/web-wallet` is a deployable Vite SPA that publishes to Cloudflare Pages
(`package.json` → `"deploy": "wrangler pages deploy dist --project-name=botho-wallet"`)
and already depends on `@botho/adapters`, `@botho/features`, `@botho/core`,
`@botho/ui`, and `@botho/wasm-signer`. The explorer therefore already shares the
adapter layer with the wallet — the exact sharing the issue asked us to
"consider."

Implication: the explorer's "never been deployed as a public page" status is a
**surfacing** gap (it is not linked from the landing/nav, and it depends on
`isConnected`), not a build or placement gap.

### Seed status page is minimal vanilla JS

`infra/seed/web/js/status.js` (~10 KB, deployed at https://seed.botho.io) is a
static vanilla-JS page. It calls `node_getStatus` and `getChainInfo` only — no
block browser, no search, no history, no multi-node view. Embedding the React
explorer here would mean shipping a compiled bundle as a static asset or
converting the page to a build step — reinventing the deploy path that
`@botho/web-wallet` already has.

### Metrics daemon: single-node history, automatic rollup

`infra/faucet/metrics-daemon/` (Rust, default port 17102) polls `node_getStatus`
every 5 minutes from **one** node (the faucet node at :17101). SQLite schema
(`src/db.rs`) with automatic rollup:

- `metrics_5min` — raw 5-min samples, 24h retention
- `metrics_hourly` — hourly aggregates, 30d retention
- `metrics_daily` — daily aggregates, 1y retention

Collected today: `height`, `peer_count`, `scp_peer_count`, `mempool_size`,
`tx_delta`, `uptime_seconds`, `minting_active`.

Not collected today: block timestamps / block-time intervals, lottery payout
events, cluster-wealth distribution, protocol versions, fee-floor history, and
**any node other than the faucet node**.

API (`src/api.rs`, CORS `*`):
`GET /api/metrics/history?metric=<...>&period=<...>&granularity=<...>`,
`GET /api/metrics/latest`, `GET /health`.

### Grafana: operator-facing, Prometheus-based

`infra/grafana/` has a Prometheus dashboard (`dashboards/botho-node.json`) and
alert rules (`provisioning/alerting/botho-alerts.yaml`), including a
`Block Height Stale (Critical)` alert (no new blocks in 10 min) and
`Low Peer Count`. These are single-instance Prometheus alerts requiring a
scrape of `--metrics-port 9090` on each node. Operator-facing; not a user
surface. **Cross-node** staleness alerting (deferred from #613) does not exist
here or anywhere.

### RPC surface (verified live 2026-07-05, EU node)

All methods the issue names are present and wired in `botho/src/rpc/mod.rs`.
Live responses from `http://3.77.150.19:17101/rpc`:

| Method | Verified response fields (live) |
|--------|--------------------------------|
| `node_getStatus` | `chainHeight`, `tipHash`, `peerCount`, `scpPeerCount`, `mempoolSize`, `mintingActive`, `mintingThreads`, `totalTransactions`, `version`, `nodeVersion`, `network`, `synced`, `syncStatus`, `syncProgress`, `quorumFaultTolerant`, `quorumDegenerate`, `minerStalled`, `uptimeSeconds`, `buildTime`, `gitCommit`, `gitCommitShort` |
| `getChainInfo` | `height`, `tipHash`, `difficulty` (number), `totalMined` (string), `totalFeesBurned` (string), `circulatingSupply` (string), `mempoolSize`, `mempoolFees` |
| `getSupplyInfo` | `height`, `totalMined` (str), `totalFeesBurned` (str), `circulatingSupply` (str), `lotteryPool` (str) |
| `getBlockByHeight` / `getBlockByHash` | `height`, `hash`, `prevHash`, `timestamp` (unix secs), `difficulty`, `nonce`, `txCount`, `mintingReward` (number picocredits) |
| `getMempoolInfo` | `size`, `totalFees`, `txHashes[]` (≤100) |
| `getTransaction` / `tx_get` | `txHash`, `status`, `blockHeight`, `confirmations`, `inMempool`, `type`, `fee` |
| `network_getPeers` | `peerCount`, `peers[]` (`peerId`, `address`, `protocolVersion`, `versionWarning`, `lastSeenSecs`) |
| `cluster_getAllWealth` | `count`, `total_tracked_wealth` (str u128), `clusters[]` (`cluster_id` str, `wealth` str) |
| `cluster_getWealthByTargetKeys` | requires `target_keys[]`; returns per-cluster `cluster_factor`, `cluster_factor_display` (e.g. `"3.53x"`) |

**Hardcoded-observability caveat (lesson from #541–#544).** `hashRate` is NOT in
any RPC response. `RemoteNodeAdapter.getNetworkStats()`
(`web/packages/adapters/src/remote.ts:199`) returns `hashRate: '0' // Not
provided by RPC`. Any hashrate shown to a user today is a stub. This proposal
must not treat it as real (see RPC Gaps below).

**Lottery payout events** are not a dedicated endpoint. Only the `lotteryPool`
balance (via `getSupplyInfo`) and per-block `mintingReward` (via
`getBlockByHeight`) are on-chain-visible. On the live node right now
`lotteryPool: "0"` and every `mintingReward` sample is the flat block subsidy
(`50000000000000` pc) — i.e. no lottery payouts have fired yet, which is exactly
why a derived feed (not a fabricated one) is required.

**Monetary amounts** (`totalMined`, `totalFeesBurned`, `circulatingSupply`,
`lotteryPool`, cluster `wealth`) are u128 picocredits serialized as decimal
strings and can exceed JS safe-integer range (since the #626 u128 migration).
All arithmetic on them must use BigInt.

---

## Decision 1 — Explorer Placement

**Recommendation: keep the explorer in `@botho/web-wallet` and surface it
publicly. Do not create `web/packages/explorer`. Do not extend the seed page.**

### Options considered

| Option | Cost | Verdict |
|--------|------|---------|
| **A. New `web/packages/explorer` SPA** | New package + Vite/router/build config + a second Cloudflare Pages project + duplicate adapter wiring already present in web-wallet | Rejected — rebuilds plumbing that exists; splits the adapter/network-selector surface across two apps |
| **B. Extend `infra/seed/web` vanilla JS** | Port React explorer to a compiled static bundle, or convert the seed page to a build step; re-solve deployment | Rejected — reinvents web-wallet's deploy path for a strictly weaker page |
| **C. Ship the explorer already wired into `@botho/web-wallet`** | Surface/link it, replace mock-friendly fields with live-verified ones, add privacy hardening | **Chosen** |

### Rationale

- The explorer already lives in the one web app that (a) deploys publicly
  (Cloudflare Pages `botho-wallet`) and (b) already shares `@botho/adapters`
  with the wallet — the sharing the issue explicitly asked us to consider.
- Routes, data source, real-time subscription, and node selection are already
  implemented. Standing up a new package throws that away and creates a second
  deploy target to operate.
- The remaining work is genuinely small and product-facing: link the explorer
  from the wallet's nav/landing so it is discoverable, and confirm/replace the
  fields the explorer renders against the live RPC (no stubbed values).

### `ExplorerDataSource`: used as-is for the explorer, extended for stats

For the **explorer** the interface is sufficient as written
(`getRecentBlocks`, `getBlock`, `getTransaction`, `onNewBlock`) — it maps 1:1
onto `RemoteNodeAdapter` RPC calls already implemented. **Do not** widen
`ExplorerDataSource` for stats. Stats views (histograms, time-series) have a
different shape (aggregate + historical, multi-node) and belong to a **separate
stats data source** that reads the metrics-daemon HTTP API, not the per-node
JSON-RPC. Keeping the two interfaces separate prevents the explorer's clean
single-node contract from being polluted with aggregation concerns.

---

## Decision 2 — Stats / Health Dashboard Placement

**Recommendation: extend `infra/faucet/metrics-daemon` into a multi-node
aggregator (the data plane) and render a new `/stats` route inside
`@botho/web-wallet` (the view plane). Leave Grafana operator-facing and
unchanged.**

### Options considered

| Option | Verdict |
|--------|---------|
| **Fold stats into the explorer with client-side fan-out** | Rejected as the *history* source — a browser cannot retain history, and N viewers × 5 nodes × poll-rate multiplies RPC load. Fine for the *live snapshot* row only. |
| **Repurpose Grafana as the user surface** | Rejected — Grafana is operator-facing, needs Prometheus scraping `:9090` on each node and an auth story we do not want to expose publicly. Keep it for operators. |
| **New standalone stats service** | Rejected — the metrics-daemon already is that service (Rust, SQLite, rollup, HTTP+CORS). Duplicating it is waste. |
| **Extend metrics-daemon + `/stats` page in web-wallet** | **Chosen** — reuses the existing daemon, rollup, retention, and CORS API; renders in the app that already deploys. |

### Rationale

- The metrics-daemon already solves the hard parts: periodic polling, a tiered
  SQLite store with automatic 5min→hourly→daily rollup, retention windows, and a
  CORS-enabled read API. The only missing capabilities are *multi-node* and
  *additional metrics* — additive changes, not a rewrite.
- Rendering in `@botho/web-wallet` reuses the deploy path and lets the stats
  page and explorer share chrome, the `NetworkSelector`, and a future
  staleness banner.
- Grafana stays where it belongs (operators), so we do not couple a public page
  to a Prometheus/auth footprint.

### Live snapshot vs history — split the responsibility

- **Live "right now" row** (current height per node, peers, minting, sync,
  version, quorum flags): the web-wallet `/stats` page MAY fan out client-side
  to each node's `node_getStatus` on load/refresh. Bounded (5 nodes, on demand),
  no state needed.
- **History and aggregates** (height-over-time, block-times, mempool depth,
  fee-floor, cluster-wealth snapshots, lottery events): served exclusively by
  the metrics-daemon HTTP API. The browser never reconstructs history.

---

## Decision 3 — History Storage

**Recommendation: store history server-side in the metrics-daemon SQLite store,
extended to (a) poll all five nodes and (b) capture the new metrics. Keep the
existing 5min/hourly/daily rollup + retention model.**

### Schema extension

Add a node dimension and new metric columns/tables. Two viable shapes; the
proposal recommends the **per-node-row** shape for minimal churn:

- Add `node_id TEXT` to `metrics_5min` / `metrics_hourly` / `metrics_daily`
  and make the primary key `(timestamp, node_id)`. The collector writes one row
  per node per interval; existing single-node consumers read
  `node_id = 'faucet'`.
- New network-scoped tables (not per node, since these are chain-global or
  derived once per interval from the healthiest node):
  - `block_times` — `(height, timestamp, interval_secs, difficulty, tx_count,
    minting_reward)`, one row per new block observed; source of block-time,
    fee-floor context, and lottery-event derivation.
  - `fee_floor` — `(timestamp, height, fee_floor_pc)`, sampled each interval
    (derived; see RPC Gaps).
  - `cluster_wealth_snapshot` — `(timestamp, height, cluster_id, wealth_pc)` or
    a bucketed histogram row set, sampled at a coarse cadence (e.g. hourly) from
    `cluster_getAllWealth`; drives the M2 wealth histogram over time.
  - `lottery_events` — `(height, timestamp, amount_pc)`, appended when a
    payout is detected (see RPC Gaps for detection).
- Store all monetary values as `TEXT` (decimal picocredit strings) to preserve
  u128 range; parse with BigInt client-side.

### Why not client-side / why not a new store

- A browser cannot retain 1-year history; the daemon already does, with rollup.
- Fan-out from every viewer would scale RPC load with audience size and produce
  inconsistent, gap-ridden series. Server-side single-writer polling gives one
  clean, deduplicated series.
- SQLite + rollup is already proven in this exact daemon; a new datastore adds
  ops burden for no capability gain at testnet scale (5 nodes, 5-min cadence).

### Multi-node collector

Replace the single faucet-node poll with a fan-out poller over the five node
endpoints (us×3 / eu=`3.77.150.19` / ap=`3.0.209.59`, all HTTP `:17101`). Node
list belongs in daemon config, not hardcoded. Each interval: poll every node's
`node_getStatus` (+ the healthiest node's `getChainInfo` / `getSupplyInfo` /
`cluster_getAllWealth` for chain-global metrics), write per-node rows, derive and
append block-time / fee-floor / lottery / cluster-wealth rows.

---

## Data Flow

```
                         ┌───────────────────────────────────────────────┐
                         │  Botho testnet nodes (5)                       │
                         │  us×3          eu 3.77.150.19   ap 3.0.209.59  │
                         │  each exposes JSON-RPC on HTTP :17101          │
                         └───────────────────────────────────────────────┘
                            ▲  ▲                         ▲            ▲
        per-node RPC on     │  │ server-side poll        │            │
        demand (live row)   │  │ every 5 min (all nodes) │            │
                            │  └───────────────┐         │            │
                            │                  │         │            │
             ┌──────────────┴───────┐   ┌──────┴─────────┴────────────┴──────┐
             │  @botho/web-wallet   │   │  metrics-daemon (Rust, :17102)      │
             │  (Cloudflare Pages)  │   │  ─ multi-node fan-out collector     │
             │                      │   │  ─ SQLite: 5min/hourly/daily +      │
             │  /explorer  ─────────┼──▶│    block_times, fee_floor,          │
             │   (per-node RPC via  │   │    cluster_wealth_snapshot,         │
             │    RemoteNodeAdapter)│   │    lottery_events                   │
             │                      │   │  ─ HTTP API (CORS *): /api/metrics  │
             │  /stats  ────────────┼──▶│    history + latest + /health       │
             │   live row: per-node │   └─────────────────────────────────────┘
             │   RPC fan-out        │
             │   history/aggregates:│         ┌──────────────────────────────┐
             │   metrics-daemon API │         │  Grafana (unchanged)          │
             └──────────────────────┘         │  operator-facing, Prometheus  │
                                              │  :9090 scrape + alerts        │
                                              └──────────────────────────────┘
```

Two data sources, deliberately separate:

```
ExplorerDataSource  ── single node, live structure ──▶ RemoteNodeAdapter (RPC)
StatsDataSource     ── multi-node, historical/aggregate ─▶ metrics-daemon HTTP API
                       (+ optional on-demand per-node RPC for the live row)
```

---

## Privacy Constraints (Non-Negotiable)

Botho transactions use CLSAG ring signatures + stealth addresses; amounts and
recipients are confidential. The explorer shows **structure only**.

**Shows (on-chain, non-linking):**
- Block: height, hash, prevHash, timestamp, difficulty, nonce, txCount,
  `mintingReward` (the coinbase/subsidy amount — public by construction).
- Transaction: txHash, status, confirmations, block height, type, fee, ring
  size (structural).
- Aggregates: cluster-wealth distribution (histogram + factor bands),
  circulating supply, total mined, fees burned, lottery pool balance.
- Coinbase / lottery **events** as amount + height (the payout amount is a
  protocol quantity, not a user balance).

**Must NOT show / must NOT build:**
- Address balances or per-address holdings.
- Transaction flows, sender→recipient linkage, or any de-anonymization aid.
- Ring-member "real spend" guessing, output-tracing, or lottery-winner→address
  linkage tooling. Lottery payout visibility is a known watch-item (per issue):
  display what is on-chain (amount, height), never who received it.
- Any UI that correlates cluster-wealth movements to specific addresses.

Rule of thumb: if a view could help someone deanonymize a user, it does not
ship, regardless of technical feasibility.

---

## RPC Gaps and How This Proposal Handles Them

### hashRate — absent from RPC (do not fabricate)

`hashRate` is not in any RPC response; the adapter hardcodes `'0'`. Options:

- **Recommended (short term): client-side derivation, clearly labelled
  "estimated."** hashrate ≈ `difficulty / mean_block_time`, using `difficulty`
  from `getChainInfo` and mean block-time from the daemon's `block_times`
  series. Never present a hardcoded `0` as if it were measured (the #541–#544
  trap). If it cannot be derived, render "n/a", not "0".
- **Optional (later): a real `hashRate` RPC field** computed node-side over a
  recent window, if an estimate proves insufficient. File as its own issue; do
  not block the explorer on it.

### Lottery payout events — not a dedicated endpoint (derive, don't invent)

No endpoint emits payout events. Detect them server-side in the daemon:

- Primary signal: a `lotteryPool` **drop** between consecutive `getSupplyInfo`
  samples indicates a payout of the drop amount at that height → append to
  `lottery_events`.
- Cross-check: a `mintingReward` in `block_times` above the flat subsidy
  (`50000000000000` pc) at the same height corroborates the payout.
- Until a payout fires (live `lotteryPool` is currently `"0"`), the feed is
  legitimately empty — show "no lottery payouts yet", never a placeholder.
- A dedicated `getLotteryEvents` RPC is a possible later optimization; not
  required for v1.

### Cluster factor bands — needs target keys

`cluster_getAllWealth` gives the wealth distribution directly (histogram input).
`cluster_getWealthByTargetKeys` returns `cluster_factor` / `cluster_factor_display`
but **requires** `target_keys`. For the M2 "factor bands via the live curve"
view, compute factor bands from the live redistribution curve parameters applied
to the wealth histogram, and use `cluster_getWealthByTargetKeys` only for
spot-checking specific clusters (e.g. verifying the merchant 3.53x prediction
from #605) — not as the bulk histogram source.

---

## Chain-Height Staleness Alerting (deferred from #613)

Two layers, both cheap:

1. **User-facing indicator (primary, no new infra):** the `/stats` page (and
   optionally the explorer header) computes "last block N minutes ago" from the
   max observed height's timestamp. Banner states:
   - green: last block < ~2× expected interval
   - amber: 2×–10× (slow)
   - red: > 10 min with no new block (matches the Grafana critical threshold)
   This is pure client-side logic over data already fetched — ship it in v1.

2. **Server-side cross-node alert (fills the #613 gap):** the metrics-daemon,
   now polling all five nodes, is the natural home for **cross-node** staleness
   detection (e.g. any node's height lagging the network max by > K, or the
   network max not advancing for > T). It can expose a status field on
   `/api/metrics/latest` and, if desired, emit an alert (webhook/log) — the
   user-facing indicator reads this so the browser and operators agree. This is
   the piece that does not exist in Grafana (which is single-instance).

---

## M2-Relevant Views

These double as the long-horizon merchant-band observation deferred at #605
closure (including the merchant 3.53x prediction).

1. **Cluster-wealth distribution — histogram + factor bands.**
   Source: `cluster_getAllWealth` for the live histogram; daemon
   `cluster_wealth_snapshot` for evolution over time. Overlay factor bands
   computed from the live redistribution curve. Spot-check specific clusters via
   `cluster_getWealthByTargetKeys` to validate the 3.53x merchant prediction.
   Privacy: aggregate/bucketed only; never resolve a bucket to addresses.

2. **Fee-floor over time.**
   Source: daemon `fee_floor` series (derived per interval). Shows the
   progressive fee floor's trajectory as the chain grows — the empirical
   companion to the fee-curve design docs.

3. **Lottery payout events.**
   Source: daemon `lottery_events` (derived from `lotteryPool` deltas +
   `mintingReward` corroboration). Amount + height only. Empty state until the
   first payout fires.

---

## Phased Issue Breakdown (filed-ready)

Each item below is written to be filed directly as a `loom:triage` issue. They
are ordered by dependency; Phase 1 delivers user-visible value with zero new
infrastructure.

### Phase 1 — Surface the explorer (no new infra)

**Issue 1.1 — Link the block explorer from the web-wallet nav/landing**
- The explorer exists at `/explorer` in `@botho/web-wallet` but is not
  discoverable. Add a nav/landing entry so users can reach it.
- Verify against the live node that every field `BlockDetail` /
  `TransactionDetail` render is populated by real RPC (no stubbed values);
  fix any that read from mock-only fields.
- Acceptance: explorer reachable from the wallet UI; block list, block detail,
  tx lookup, and search all work against a live testnet node; no field shows a
  hardcoded placeholder.

**Issue 1.2 — Explorer privacy audit + hardening**
- Audit every explorer view against the privacy rules in this doc. Ensure no
  balance/flow/linkage data is shown and none can be derived from the UI.
- Confirm `mintingReward`, fees, ring size are the only value-adjacent fields
  and are structural/public.
- Acceptance: documented pass over each component; any offending field removed
  or aggregated.

**Issue 1.3 — Client-side chain-height staleness banner**
- Add a "last block N minutes ago" indicator (green/amber/red per this doc) to
  the explorer header and/or `/stats`, computed client-side from observed tip
  timestamp. No backend.
- Acceptance: banner turns red when tip is > 10 min old; no new services.

### Phase 2 — Multi-node metrics backend

**Issue 2.1 — metrics-daemon: multi-node fan-out collector**
- Move the node list to config; poll all five nodes' `node_getStatus` each
  interval. Add `node_id` to the metrics tables (PK `(timestamp, node_id)`).
  Preserve rollup + retention. Keep single-node consumers working via a default
  `node_id`.
- Acceptance: history API returns per-node series for all five nodes; rollup and
  retention unchanged.

**Issue 2.2 — metrics-daemon: block-time + fee-floor capture**
- Add `block_times` and `fee_floor` tables; populate each interval from
  `getBlockByHeight`/`getChainInfo`. Expose via history API.
- Acceptance: block-time and fee-floor series queryable over 24h/30d/1y tiers.

**Issue 2.3 — metrics-daemon: lottery-event + cluster-wealth capture**
- Add `lottery_events` (derived from `lotteryPool` deltas, corroborated by
  `mintingReward` spikes) and `cluster_wealth_snapshot` (from
  `cluster_getAllWealth`, coarse cadence). All monetary values stored as decimal
  strings.
- Acceptance: lottery-event feed appends on pool drops; cluster-wealth snapshots
  queryable over time; empty states correct when nothing has fired.

**Issue 2.4 — metrics-daemon: cross-node staleness status + alert (closes #613 gap)**
- Compute cross-node staleness (node height lag vs network max; network max
  stall duration). Expose on `/api/metrics/latest`; optional webhook/log alert.
- Acceptance: `/api/metrics/latest` reports per-node lag and a network-stall
  flag; alert fires when the network max stalls beyond threshold.

### Phase 3 — Stats page in web-wallet

**Issue 3.1 — `/stats` route with live per-node row + network history**
- Add a `StatsDataSource` reading the metrics-daemon history API; add a `/stats`
  route in `@botho/web-wallet`. Live row via on-demand per-node RPC fan-out;
  history/aggregates via the daemon API. Wire the staleness banner (Issue 1.3)
  to the daemon's cross-node status (Issue 2.4).
- Acceptance: `/stats` shows all five nodes live + height/peers/mempool/block-time
  history; staleness banner reflects cross-node status.

**Issue 3.2 — M2 views: cluster-wealth histogram + factor bands, fee-floor chart**
- Render the cluster-wealth histogram with factor bands from the live curve
  (BigInt math on picocredit strings); render fee-floor over time. Spot-check
  the merchant 3.53x band via `cluster_getWealthByTargetKeys`.
- Acceptance: histogram + factor bands render from live/daemon data; fee-floor
  chart renders; all monetary math is BigInt; privacy rules honored.

**Issue 3.3 — Lottery payout events view**
- Render the derived lottery-event feed (amount + height) with a correct empty
  state. No winner/address linkage.
- Acceptance: feed renders daemon `lottery_events`; empty state until first
  payout.

### Phase 4 — Optional follow-ups (file only if needed)

**Issue 4.1 — Real `hashRate` RPC field** (only if the client-side estimate
proves insufficient). Node-side hashrate over a recent window; adapter reads it
instead of deriving. Removes the `hashRate: '0'` stub.

**Issue 4.2 — Dedicated `getLotteryEvents` RPC** (only if derivation proves
lossy). Node-side event log; daemon reads it directly instead of inferring from
pool deltas.

---

## Open Questions for the Maintainer

1. Should the public explorer/stats live under the existing `botho-wallet`
   Cloudflare Pages project (recommended — one deploy) or a separate subdomain?
2. Should the metrics-daemon's node list and the web app's node list share a
   single source of truth (e.g. a checked-in `nodes.json`) to avoid drift?
3. Is a coarse (hourly) cluster-wealth snapshot cadence acceptable for the M2
   histogram, or is finer resolution wanted near merchant-band validation?

---

## Acceptance Criteria Coverage (from #633)

- [x] Explorer placement decision + rationale — Decision 1 (keep in web-wallet)
- [x] Stats/dashboard placement decision + rationale — Decision 2 (extend
  metrics-daemon + `/stats` in web-wallet)
- [x] Whether `ExplorerDataSource` is used as-is or extended — as-is for
  explorer; separate `StatsDataSource` for stats
- [x] Multi-node history data source + retention design — Decision 3 (server-side
  SQLite, per-node rows, new tables)
- [x] RPC gap handling: hashRate + lottery events — RPC Gaps section
- [x] Chain-height staleness home — client banner + daemon cross-node status
- [x] Privacy handling: what is / is not shown — Privacy Constraints section
- [x] M2 views: cluster-wealth histogram + factor bands, fee-floor, lottery
  events — M2-Relevant Views section
- [x] Phased issue breakdown for implementation — Phased Issue Breakdown section
