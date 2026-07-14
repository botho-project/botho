# Post-CT Analytics: Surface-by-Surface Fate Map

**Status**: Accepted design input (feeds #902 gadget selection)
**Date**: 2026-07-14
**Issue**: #908
**Related**: ADR 0006 (PQ & privacy ratification), #902 (CT ↔ economics spec
epic), #904 (CT + universal ML-KEM implementation epic), #616/#830 (external
audit scope), `docs/design/block-explorer-network-stats.md` (dashboard
architecture and privacy rules)

## Purpose

ADR 0006 Decision 1 ratifies **confidential amounts** (Pedersen commitments +
Bulletproofs) with **public fees** as the target state; the live chain's
public `amount: u64` is an implementation gap. Every analytics, explorer,
dashboard, and display surface that reads amounts today must therefore be
assigned a post-CT fate *before* #902 selects its verification gadgets —
otherwise the gadget spec could silently make a load-bearing aggregate
uncomputable.

This document inventories the **actual** surfaces (read from code, with each
node-side field verified as wired rather than hardcoded — the lesson of
#541–#544) and maps every one to exactly one of three fates:

- **(a) Survives via consensus-computable public aggregate** — the value
  remains derivable by any validator/observer from public ledger data under
  CT. The union of these is the "required public aggregates" contract that
  #902's gadget selection must preserve.
- **(b) Needs view-key-based opt-in disclosure** — the value concerns a
  specific party's holdings and survives only if that party voluntarily
  discloses (view key, attested opening, or self-published accounting).
- **(c) Deprecated by design** — the value ceases to exist as a public
  quantity under CT. Each (c) entry is an explicit product decision, not a
  silent omission.

### The disclosure model this doc enforces

Botho's privacy posture is a triad, and each leg is deliberate:

| Question | Answer | Mechanism |
|---|---|---|
| **Who** transacted | Hidden | CLSAG rings (sender) + one-time stealth outputs, universal ML-KEM per ADR 0006 Decision 2 (recipient) |
| **How much** | Hidden | Pedersen + Bulletproofs (ADR 0006 Decision 1); **fees public**; **minting/coinbase public** (the one transparent tx class) |
| **Lineage** (cluster provenance / wealth class) | **Public by design** | Cluster tags and per-cluster wealth aggregates — the substrate of demurrage factors, tilted lottery, and progressive fees |

Everything in fate (a) below is an instance of the third leg or of the public
fee/coinbase carve-outs of the second. Nothing in this document may weaken
the first two legs; several (c) decisions exist precisely to protect them.

---

## 1. Inventory and fate map

Wiring verification: all node-side fields cited below were traced to their
computing functions in `botho/src/rpc/mod.rs`, `botho/src/rpc/metrics.rs`,
`botho/src/ledger/store.rs`, `botho/src/mempool.rs`, and
`bridge/service/src/api.rs`. The node's observability layer is clean post
#541–#544: unwired optional handles default to `0`/`false`/`null` (never a
plausible fake constant). The two fabrication bugs found live in the **web
adapter**, not the node — see §5.

### 1.1 `/network` dashboard (`web/packages/features/src/network/`, page `web/packages/web-wallet/src/pages/network.tsx`)

| Surface / stat | Source (verified wired) | Amounts-dependent | **Fate** |
|---|---|---|---|
| Fleet summary: consensus height, nodes in sync, fleet mempool, avg block spacing (`components/fleet-summary.tsx`) | `node_getStatus` → `chainHeight`/`mempoolSize`; `getBlockByHeight` timestamps | No (counts, timings) | **(a)** — unaffected |
| Node cards: height, mempool, peers, SCP peers, minting flag, blocks-behind, slot-stalled, version (`components/node-card.tsx`) | `node_getStatus` (all live: atomics, ledger state, SCP slot snapshots) | No | **(a)** — unaffected |
| History charts: chain height, mempool depth (`components/history-chart.tsx`) | metrics-daemon `GET /api/metrics/history` (`infra/faucet/metrics-daemon/src/api.rs`) → `height`, `mempoolSize`, `peerCount` | No | **(a)** — unaffected |
| **Reserve proof card**: locked reserve, total wrapped, ETH/SOL wBTH supply, drift, peg-healthy (`components/reserve-proof-card.tsx`) | metrics-daemon `GET /api/metrics/reserve` ← bridge service reserve snapshot (`bridge/service/src/api.rs`, live transports per #853/#880) | **Yes** (picocredit balances) | **(b)** — see §3 (bridge view-key disclosure; audit-scope flag) |

### 1.2 Block explorer (`web/packages/features/src/explorer/`, tabs at `/explorer` — blocks / wealth distribution / lottery, `components/explorer.tsx:26-27`)

| Surface / stat | Source (verified wired) | Amounts-dependent | **Fate** |
|---|---|---|---|
| Block list: height, hash, timestamp, tx count, **block reward** (`components/block-list.tsx`) | `getRecentBlocks` → `getBlockByHeight` → `block.minting_tx.reward` | Reward: yes | **(a)** — coinbase is the transparent tx class; reward stays public (aggregate A2) |
| Block detail: header fields, **reward**, **total fees**, **per-tx fee**, ring size, lottery summary (`components/block-detail.tsx`) | `getBlockByHeight`/`getBlockByHash` → `mintingReward`, `totalFees` (= `block.total_fees()`), `transactions[].fee`, `lottery.*` | Yes | **(a)** — fees public (A1), coinbase public (A2), lottery summary public (A3) |
| Transaction detail: fee, status, confirmations, ring size, block height (`components/transaction-detail.tsx`) | `getTransaction` → `fee` (public), structural fields | Fee: yes | **(a)** — fee stays public (A1) |
| Transaction detail: **"Amount" row** (`transaction-detail.tsx:64`) | **Fabricated** — adapter hardcodes `amount: BigInt(0)` (`web/packages/adapters/src/remote.ts:489`); the node deliberately does not expose per-tx amounts even today | Yes | **(c)** — deprecation D1 |
| Wealth distribution tab: total tracked wealth, cluster count, median factor, log-bucket histogram with factor bands (`components/cluster-wealth.tsx`, `wealth.ts`) | `cluster_getAllWealth` → per-cluster `wealth` (u128, live from `cluster_wealth_db`) + node-computed `factor` | **Yes** — the whole view is the per-cluster wealth vector | **(a) — conditional on #902** (aggregate A4, the load-bearing ask) |
| Lottery tab: per-block payout count, payout total, pool distributed, burned, fees (`components/lottery-feed.tsx`, `lottery.ts`) | `Block.lottery` (`lottery_summary` + `total_lottery_payouts()`, wired) | **Yes** | **(a)** — lottery event quantities stay public (A3); winner identity stays hidden |

### 1.3 Node RPC / metrics endpoints (no web renderer today; consumed by operators, Grafana, future dashboards)

| Surface / field | Source (verified wired) | Amounts-dependent | **Fate** |
|---|---|---|---|
| `getChainInfo` / `getSupplyInfo`: `totalMined`, `totalFeesBurned`, `circulatingSupply`, `lotteryPool` | Ledger meta counters (`META_TOTAL_MINED`, `META_FEES_BURNED`, `META_LOTTERY_POOL`), live | **Yes** | **(a)** — all four are sums/differences of public coinbase, public fees, and public lottery events (A1+A2+A3 ⇒ A5) |
| Prometheus `botho_total_minted`, `botho_total_fees_burned` (`rpc/metrics.rs`) | Same ledger counters | **Yes** | **(a)** — same derivation |
| `cluster_getAllWealth`, `cluster_getWealth` | `ledger.get_all_cluster_wealth()` / `cluster_wealth_db` | **Yes** | **(a) — conditional on #902** (A4) |
| `cluster_getWealthByTargetKeys` (wallet's own cluster wealth + own UTXO total) | `ledger.compute_cluster_wealth_for_utxos(target_keys)` over public amounts | **Yes** | **(b)** — becomes wallet-local computation over the wallet's own (view-key-decrypted) outputs; the node can no longer compute it for arbitrary target keys |
| `fee_estimate` / `tx_estimateFee`, `fee_getRate` | Per-byte schedule + dynamic base + cluster factor (`transaction/src/fees.rs`, `mempool.rs`) — size-based, amount used only for structure estimation | Indirectly | **(a)** — fees are public by design; per-byte pricing was chosen for exactly this (ADR 0006 / #904 note). Sender-side factor input moves to the wallet (see §1.5) |
| `getTransaction` → `fee` | `tx.fee` | Yes | **(a)** (A1) |
| `node_getStatus`, `network_getInfo`, `network_getPeers`, `minting_getStatus`, `/health`, `/metrics` (non-monetary fields) | Live atomics / snapshots; zero/null defaults only when handles unwired (anti-#541 gates) | No | **(a)** — unaffected |

### 1.4 Faucet (testnet-only surface)

| Surface / stat | Source | Amounts-dependent | **Fate** |
|---|---|---|---|
| Web/desktop faucet buttons: "received N BTH" (`web/packages/web-wallet/src/components/FaucetButton.tsx`, `web/packages/desktop/src/components/faucet-button.tsx`) | `faucet_request` response `amount` (`botho/src/rpc/faucet.rs`) | Yes | **(b)** — trivially: the faucet is the **sender** and knows its own drip amounts; self-disclosure by construction |
| Legacy faucet page: drip amount, max/daily limit, today-dispensed + progress bar (`infra/faucet/web/js/faucet.js`) | Config constants + faucet's own `AtomicU64` accounting (`FaucetState::stats()`) | Yes | **(b)** — the faucet publishes its own accounting; under CT it continues via its own wallet keys. (Faucet is disposable testnet plumbing regardless — §7 of the audit scope) |
| Minting-paused-at-10k-BTH logic | Faucet's own balance scan with its own keys | Yes | **(b)** — self-scan already view-key-shaped; unaffected in structure |

### 1.5 Fee-estimation and wallet display flows

| Surface / stat | Source | Amounts-dependent | **Fate** |
|---|---|---|---|
| Send modal: estimated fee, custom fee, cluster factor display, total (`web/packages/features/src/wallet/components/send-modal.tsx`) | `estimateFee` (size-based) + `clusterFactorDisplay` (node-computed today from sender wealth) | Yes | **(a)** for the fee itself (public, size-priced). The **factor input** becomes wallet-computed: the wallet derives its own cluster wealth from view-key-decrypted outputs and the public per-cluster aggregates (A4), then the validator checks the committed demurrage/fee relation per #902's demurrage gadget |
| Wallet balance / tx history (`balance-card.tsx`, `transaction-row.tsx`) | Wallet's own scan (view key → note decryption → amount opening under CT) | Yes | **(b)** — own-key territory by definition; structurally unaffected by CT |

---

## 2. Required public aggregates — input to #902 gadget selection

This is the contract. Whatever gadget set #902 selects (committed demurrage
terms + range proofs, tag-blend proofs, Merkle-sum lottery tree), it **must
keep the following consensus-computable and publicly verifiable**, or the
fate map above collapses and additional surfaces silently fall into (c):

- **A1 — Per-transaction public fee.** Already ratified (ADR 0006 Decision
  1). Downstream sums that must therefore remain exact public scalars:
  per-block `totalFees`, ledger `totalFeesBurned`, mempool `totalFees`,
  fee-floor/fee-rate history. Note for the §9 leakage table update in #902:
  the committed-demurrage design must bound what the public fee reveals
  about the hidden value (the documented `fee = k·v` inversion risk).
- **A2 — Per-block public minting reward.** Minting stays the one
  transparent transaction class (coinbase amounts public by construction,
  attribution via PoW preimage per ADR 0006 Decision 3). Downstream:
  `totalMined`, block-list/detail reward display.
- **A3 — Per-block public lottery event quantities**: `payoutCount`,
  `payoutTotal`, `poolDistributed`, `amountBurned`, lottery `totalFees`,
  and the running `lotteryPool` balance. The Merkle-sum weighted-sampling
  gadget must produce payout **amounts** as public consensus quantities
  while keeping the **winner** hidden (payout to a one-time output; the
  known "winners visible on-chain" watch item is a separate phase-2
  blinding decision and must not be conflated — if payout amounts ever get
  blinded too, the lottery feed and `lotteryPool` accounting move to (c),
  which is a product decision nobody has made).
- **A4 — The per-cluster wealth vector** (`cluster_id → wealth` totals, as
  served by `cluster_getAllWealth` today). This is the **load-bearing
  aggregate** and the hard requirement on #902's tag-blend gadget: cluster
  lineage and cluster wealth are *public by design* (the disclosure triad's
  third leg), because validators must derive demurrage/fee factors and the
  lottery tilt from them, and because the entire M2 observability program
  (wealth histogram, Gini, top-10 share, factor bands, the merchant-3.53×
  long-horizon watch from #605) reads this vector. A blend-correctness
  gadget that hides per-cluster totals — or a "value-free tag propagation
  redesign" that stops maintaining them — would break consensus economics
  *and* every wealth view at once. If #902 chooses a design where cluster
  wealth is maintained as homomorphic commitment sums, it must additionally
  specify how the **scalar totals** are published and verified (e.g.
  validator-checked running openings), not just that the sums exist.
- **A5 — Supply identities** (derived, no new gadget): `totalMined` −
  `totalFeesBurned` = `circulatingSupply`; A1+A2+A3 suffice. #902 should
  state this identity survives CT verbatim so auditors can check supply
  without any view key.

Anti-requirement, for completeness: **no gadget may introduce a public
per-output or per-transaction amount**. The only public value quantities
after CT are A1–A5.

## 3. Bridge proof-of-reserve: the view-key obligation (flag to #616/#830)

The reserve-proof card verifies `lockedReserve ≥ totalWrapped` by comparing
the bridge reserve balance **read directly from public Botho amounts** (live
transport, #880) against transparent-chain wBTH supplies. Under CT that read
becomes impossible for a third party: deposits to the bridge reserve are
hidden like any other output.

Post-CT, proof-of-reserve therefore **requires the bridge federation to
disclose**: publish the reserve wallet's view key (or per-epoch attested
openings / a proof-of-liabilities-style commitment sum) so that the
metrics daemon — and anyone else — can recompute `lockedReserve`. Notes:

- ADR 0004 already accepts amount revelation at the bridge boundary (lock
  reveals the amount; the wrapped side is public), so this disclosure adds
  no *new* leakage class — it institutionalizes the existing boundary.
- **Audit-scope action**: this disclosure mechanism (what exactly is
  disclosed, by whom, revocation/rotation, and whether a spoofed disclosure
  can fake solvency) is added to the external-audit scope
  (`docs/security/external-audit-scope.md` §4.8, this PR) and belongs to
  the bridge audit's "reserve accounting / peg" line in #830. It must be in
  scope **whenever the audit and CT land in either order**, because the
  disclosure design constrains the reserve-address structure chosen now.
- Fate: **(b)** — and it is the one (b) entry where the disclosing party is
  *obligated*, not opting in: a bridge that declines to disclose has no
  peg-health story. #902/#904 should treat "reserve view-key disclosure
  protocol" as a named deliverable of the CT implementation epic.

## 4. Deprecations — explicit product decisions

- **D1 — Explorer per-transaction "Amount" row is removed.** The node has
  never exposed per-tx amounts to the explorer endpoint; the row today
  renders an adapter-fabricated `0 BTH` (`remote.ts:489` →
  `transaction-detail.tsx:64`) — a #541-class fabricated display. Decision:
  delete the row (do it now, ahead of CT; tracked as a follow-up fix, §5).
  Under CT this is not a loss — it is the design.
- **D2 — No BTH-denominated velocity / transfer-volume analytics, ever.**
  Never shipped (the dashboard has no such stat; `getNetworkStats` is
  consumerless), and under CT the sum of transferred value is not a public
  quantity by construction. Decision: transaction-**count** velocity
  (mempool throughput, `totalTransactions`, tx/block) is the supported
  metric family; value-velocity requests are rejected as
  incompatible-by-design, not "not yet implemented".
- **D3 — No per-address balances, rich lists, or flow tracing —
  reaffirmed.** Already prohibited by the explorer privacy rules
  (`block-explorer-network-stats.md` "Privacy Constraints"); CT upgrades
  the prohibition from policy to cryptographic impossibility. The wealth
  view stays **cluster-granular** (A4) and must never resolve buckets to
  addresses.
- **D4 — Public per-output amounts on the live testnet are a transitional
  artifact.** No new surface may be built against them (they disappear with
  #904). Anything wanting amounts must cite an A1–A5 aggregate or a (b)
  disclosure path.

Everything else inventoried above survives; there are no silent omissions —
if a surface is not listed in §1, it displays no amounts (verified for every
component under `features/src/network/`, `features/src/explorer/`, the
faucet surfaces, and the send flow).

## 5. Wiring findings (the #541–#544 sweep results)

Verified clean and **wired** node-side: all `node_getStatus` /
`getChainInfo` / `getSupplyInfo` / `cluster_*` / `fee_*` / lottery /
Prometheus fields trace to live ledger state, atomics, or consensus
snapshots; unwired optional handles yield `0`/`false`/`null` by explicit
anti-fabrication design (miner health, network stats fallback, SCP slot
`null`s). The bridge `lockedReserve` is a live ledger read (#880), not a
constant.

Fabrications found in the **web adapter** (`web/packages/adapters/src/remote.ts`),
to fix as a follow-up (filed from this PR):

1. `remote.ts:489` — `getTransaction` returns `amount: BigInt(0)`, rendered
   as a real "Amount: 0 BTH" (`transaction-detail.tsx:64`). Fix = D1.
2. `remote.ts:494` — `timestamp: Date.now()` fabricates the tx timestamp as
   "now"; render "—" or resolve the block timestamp instead.
3. `remote.ts:488` — `type: 'receive'` hardcoded for every transaction.
4. `remote.ts:269` — `getNetworkStats` still carries the `hashRate: '0'`
   stub. Currently dormant (no consumer renders it) but a standing trap;
   derive-or-"n/a" per the dashboard design doc if ever surfaced.

## 6. Summary

| Fate | Count (surfaces from §1) | Members |
|---|---|---|
| **(a)** public aggregate | 13 | fleet summary; node cards; history charts; block list; block detail; tx-detail structural+fee; wealth tab*; lottery tab; supply RPC fields; Prometheus supply metrics; `cluster_getAllWealth`*; fee-estimation flow; non-monetary RPC/status/metrics (*conditional on #902 preserving A4) |
| **(b)** view-key disclosure | 6 | bridge reserve-proof card (**obligatory**, →#616/#830); `cluster_getWealthByTargetKeys` (wallet-local); faucet buttons; faucet status page; faucet pause logic; wallet balance/history |
| **(c)** deprecated by design | 4 | D1 explorer tx "Amount" row; D2 value-velocity analytics; D3 per-address balances/rich lists (reaffirmed); D4 transitional public amounts |

#902 owns: A1–A5 preservation, the §9 leakage-table update for the
committed demurrage term, and the A3 winner-hidden/amount-public split.
#904 owns: the reserve view-key disclosure protocol as a named deliverable,
and D4 enforcement when the output format flips.
