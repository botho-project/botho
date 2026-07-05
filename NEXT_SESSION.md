# Next-session prompt — web wallet UI + network visualization

Copy-paste the prompt below to start the next session.

---

## Prompt

The mainnet-blocker engineering push is complete (see PLAN.md and the memory
index): the 5-node testnet runs protocol 4.0.0 (`main @ 4f944e0`+, fresh
genesis 2026-07-05) with the recalibrated log-domain fee curve, u128 cluster
wealth, and ratified cumulative M2 semantics; v0.3.0 is tagged and released.
This session shifts focus to **user-facing surfaces**: the web wallet UI and
network-level visualization (block explorer + network stats). Use
`/loom:sweep` for issue lifecycles; work is on the public testnet, which is
ephemeral (no backup ceremony).

### Priority 1 — Web wallet UI polish

The PWA at https://wallet.botho.io (`web/packages/web-wallet`, Vite + React +
wasm-signer, deployed via Cloudflare Pages — deploy notes in the
`project_live_testnet_infra` memory) is functional but rough. Survey it
first (run it locally against the live testnet, walk every page), file issues
for what you find, then sweep them. Known context:

- The RPC surface changed recently: cluster-wealth values are now
  **string-encoded u128** (#628) and the fee curve returns smooth factors
  1.00x–6.00x (#626) — verify the wallet renders fees/factors correctly and
  handles the string encoding everywhere (the #610 serde-drift bug class:
  check the adapters in `web/packages/adapters` against actual node JSON).
- Wallet security posture: at-rest encryption issues #474/#475/#476 were
  fixed — don't regress them; threat model is docs/security/threat-model.md.
- UX debt worth checking: seed-phrase onboarding flow, send-flow fee
  display (should show the cluster factor + why), claim-link flow, network
  selector (5 nodes now — us×3/eu/ap; eu/ap are plain HTTP :17101, no TLS
  yet — decide whether to add them as ingress options or put TLS in front
  first), error states when a node is unreachable.
- Mobile app (`mobile/app`, React Native) exists in parallel — UI work here
  should keep API/adapter changes shared where possible.

### Priority 2 — Block explorer

No explorer exists today. The seed status page (`infra/seed/web/`, served at
https://seed.botho.io) shows only node status + chain info. Design and build
a block explorer as a new web package (`web/packages/explorer` or extend the
status page — architect should decide with a proposal issue first):

- RPC methods available: `getBlockByHeight`, `getBlockByHash`,
  `getChainInfo`, `getTransaction`/`getTransactionStatus`,
  `getMempoolInfo`, `chain_getOutputs`, `cluster_getAllWealth`,
  `node_getStatus`, `network_getPeers` (all on any node's /rpc; see
  botho/src/rpc/mod.rs for the full list and exact response shapes — verify
  shapes against a LIVE node, not just code, per the hardcoded-observability
  lesson in memory).
- Privacy caveat to respect in the UI: amounts/recipients are confidential
  (CLSAG + stealth addresses); an explorer shows structure (blocks, tx
  hashes, ring sizes, fees, cluster-wealth aggregates, coinbase/lottery
  events), NOT balances or flows. Lottery payout visibility is a known
  privacy watch-item — display what's on-chain but don't build linkage
  tooling.
- Include the M2-relevant views: cluster-wealth distribution (histogram +
  factor bands via the live curve), fee-floor over time, lottery payout
  events. These double as the long-horizon merchant-band observation the
  M2 closure deferred (#605's merchant 3.53x prediction).

### Priority 3 — Network stats / health dashboard

Aggregate view across the 5 nodes (heights, peer counts, block times,
mempool depth, minting state, protocol versions) with history. Options to
weigh: extend `infra/faucet/metrics-daemon` (exists, v0.3.0-bumped, check
what it already collects), a Grafana route (infra/grafana exists), or fold
into the explorer. CloudWatch auto-recovery alarms exist (#613) but
chain-height staleness alerting was deferred — this dashboard is the natural
home for it. Regional nodes: eu=3.77.150.19, ap=3.0.209.59 (HTTP :17101).

### Process guidance

- Start with an architect proposal for the explorer/stats architecture (one
  issue), and a survey pass over the wallet that files concrete issues.
  Champion-evaluate, then sweep in waves.
- Sub-agent security-review filter: do Botho security review in the main
  session (memory: `feedback_subagent_filter_security`).
- Left open intentionally: #615 (two remaining operator tasks: two-builder
  repro check + runbook prefer-artifacts update — check whether the v0.3.0
  release run 28742206174 published artifacts and close accordingly), #616
  (external audit engagement — operator-only), #613 (ops-hardening
  residuals). The five loom:architect design issues (#532/#458/#441/#427/
  #581) are parked pending product decisions; #427 has a close
  recommendation awaiting the operator.
- BaaS/billing context if wallet work touches account flows: Stripe under
  2amlogic, test mode on testnet (memory: `project_billing_stripe_2amlogic`).

---

## State snapshot at handoff (2026-07-05)

| Area | State |
|---|---|
| Testnet | 5 nodes / 3 continents, protocol 4.0.0, fresh genesis 2026-07-05, all synced |
| Release | v0.3.0 tagged on `003eec0`; release run 28742206174 was queued at handoff — verify artifacts |
| Mainnet blockers | 1/3/4/5 done; 2 (external audit) is operator-only |
| M2/#605 | CLOSED — cumulative semantics ratified, live-verified 6/6 golden vectors |
| Open loom work | none in flight; no sweep checkpoints |
