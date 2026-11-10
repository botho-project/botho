# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

While Botho is pre-1.0, the leading `0.` indicates the on-chain protocol is
still mutable. Within the `0.x` line, a `MINOR` bump (`0.X.0`) signals
consensus-breaking changes that require a coordinated network reset; `PATCH`
bumps (`0.X.Y`) are backwards-compatible.

## Protocol 10.0.0 (unreleased, in progress)

Consensus-breaking; fresh chains from genesis (testnet reset).
`PROTOCOL_VERSION` and `MIN_SUPPORTED_PROTOCOL_VERSION` are both 10.0.0, so 9.x
peers are refused at the identify handshake. First item of the 10.0.0
consensus batch (#1683).

- **Structural transaction rules are consensus rules (#1684, #1772).** Ring
  size at least `MIN_RING_SIZE` (20), at most `MAX_OUTPUTS` (16) outputs per
  transaction, and no output below `DUST_THRESHOLD` (1,000,000 pico) were
  checked only at mempool admission, so a validator could include its own
  small-ring, dust or over-wide transaction in a block. Block application
  (`validate_ordinary_body`, new check C9, and the per-transaction
  `verify_transfer_for_block` used by the block builder and the SCP slot
  check) and SCP value validation (`validate_transfer_tx`) now reject them.
  All three call one predicate, `Transaction::check_consensus_structure`,
  which mempool admission also calls, so relay and consensus cannot drift.
  The constant values are unchanged. Coinbase (`MintingTx`) and lottery
  payouts are not `Transaction`s and are unaffected.
- **Bridge-import tag drop closed (#1687, #1740).** Under 7.x-9.x a spender
  could rewrite an import-tagged coin's output tags to nothing: C6 only
  rejected tag *inflation*, and the ADR-0007 import floor `F` = 1.5x was read
  from the output tags, so the spend was charged at 1x (Lean
  `Botho.Tags.import_floor_invariant_refuted`). Per the operator ruling
  (options 1 + 4), three new consensus rules, enforced by block validation
  (`verify_transfer_for_block`, used by the block builder and the SCP value
  check, and `validate_ordinary_body`) and mempool admission through the same
  functions:
  1. *The import floor is read from the inputs*
     (`bth_cluster_tax::import_floor_factor_from_inputs`, via
     `ValidationReads::import_floor_factor_from_inputs` inside
     `consensus_fee_floor`). Each input's value, its pseudo-output amount, is
     priced at the largest import share among its *spend candidates* (the ring
     members whose amount equals the pseudo-output amount, which C3 and the
     balance proof bind the real member to), blended with 1x. The output-tag
     blend is kept as a further `max`. Output tags no longer lower `F`, and a
     decoy cannot dilute it.
  2. *Retained-weight floor* (`first_tag_retention_violation`, via
     `ValidationReads::verify_cluster_tag_retention`). Every output keeps at
     least 95% of each cluster's weight that every spend candidate of a ring
     carries (value-weighted over inputs, less a rounding slack of
     `inputs + 1`). 95% is the documented 0.95 decay per transfer
     (whitepaper Appendix, `docs/design/cluster-tag-decay.md`) the stock
     wallets already apply: no new constant. Required weights below
     `MIN_STORED_WEIGHT` are not checked, and an output with 16 entries, all at
     least the required weight, is excused (honest truncation). Settlements
     are exempt (they must be background and pay the capitalized charge).
  3. *`created_at_height` bound (#1740).* A transfer dated after the including
     block's height is rejected (`is_future_dated`); the block builder skips
     such transactions and relay refuses them against the next height.

  Known limits: a decoy of the coin's exact amount without the cluster lowers
  the candidates' minimum to zero, so the retained-weight floor does not bind
  (the drop then still pays `F` from the inputs); the floor limits decay per
  spend, not per unit of time. Lean: `import_floor_honoured₁₀`,
  `tag_shed_rejected₁₀`, `same_amount_decoy_drop₁₀` (statement records added;
  listed by a follow-up), DRT legs `cluster_tag_retention` and
  `import_floor_from_inputs`. The honest first spend of a lone 1,000-BTH
  import now pays 0.492% instead of 0.442%, because `F` is read from the
  undecayed input. Wallets: the web/snap/mobile signer (`bth-wasm-signer`)
  wrote empty output tags and would now be rejected spending any tagged coin;
  it now inherits its inputs' tags decayed 5% like the node and CLI wallets.
  `chain_getOutputs` gains an additive `clusterTagsDec` field (cluster ids as
  decimal strings) because JavaScript numbers round u64 cluster ids.
- **`MIN_TX_FEE` stays relay policy (ADR 0014).** The consensus fee rule
  remains the size-based `consensus_fee_floor`; `MIN_TX_FEE` (1e8 pico) is
  still checked only at mempool admission. See
  `docs/decisions/0014-min-tx-fee-relay-policy.md`.

## Protocol 9.0.0 (unreleased, in progress)

Consensus-breaking; fresh chains from genesis (testnet reset).
`PROTOCOL_VERSION` and `MIN_SUPPORTED_PROTOCOL_VERSION` are both 9.0.0, so 8.x
peers are refused at the identify handshake. The release carries two fixes:
the #1876 exact fee-floor means and the #1875 exact demurrage charge.

- **Exact fee-floor weighted means (#1876).** The consensus fee floor takes
  three value-weighted means of cumulative cluster wealth: the ring-centroid
  floor (`ring_centroid_implied_factor`), each ring member's tag-weighted
  wealth (`ring_centroid_floored_factor`), and the output-tag claimed wealth
  (`effective_cluster_wealth_from_outputs`). In 8.x each one saturated its
  numerator at `u128::MAX` and then divided by the unsaturated total value,
  which lowered the result for whale-scale rings rather than pinning it to the
  maximum. For example, a 10M BTH coin in a cluster with 1B BTH cumulative
  wealth got a floor of 5919 instead of 6000 and paid 16,200 BTH too little on
  the #925 downgrade charge. All three now compute the exact rational floor
  over a 256-bit numerator (`bth_cluster_tax::wide`). The centroid is scale
  invariant, and every input the 8.x arithmetic did not overflow gives the same
  result. A result above `u128::MAX` is clamped at the end, which is
  conservative because it maps to the 6000 maximum factor. That case can
  only arise from malformed tag weights, which Rule 1 rejects. No storage
  format change. Mempool admission had its own copy of the claimed-wealth
  computation with the same saturation, so relay would have admitted whale
  transactions below the consensus floor. It now calls the ledger's exact
  function, so relay and consensus compute the same claimed wealth and
  relay stays at least as strict as consensus.
- **Demurrage is computed exactly (#1875).** `demurrage_charge` now takes one
  floor of the full rational charge,
  `value × rate × (factor − 1000) × elapsed / (10_000 × 5_000 × blocks_per_year)`,
  clamped to `u64::MAX`. The 8.x kernel first floored the elapsed time to
  `1e6` units of a year. At the 5 s year that zeroed every hold of 1 to 6
  blocks, for any value, and charged every 7 to 12 block hold the same
  amount. A wallet that kept its ring members young could churn every six
  blocks and pay no demurrage. Splitting a hold into n same-class spends now
  pays the unsplit charge less fewer than n pico. Honest charges rise only by
  the 8.x under-charge (for example, 1M BTH at 6x for 12 blocks: 0.038 BTH,
  was 0.02 BTH). The operator chose this fix after the simulation study in
  PR #1908. Rates, `SETTLEMENT_HORIZON_BLOCKS` and the emission schedule are
  unchanged. Downgrade (#925) and settlement (#831) charges call the same
  kernel and are exact too.

## Protocol 8.0.0

Consensus-breaking; fresh chains from genesis (testnet reset).
`PROTOCOL_VERSION` and `MIN_SUPPORTED_PROTOCOL_VERSION` are both 8.0.0, so 7.x
peers are refused at the identify handshake.

- **Output cluster tags are bounded (#1857).** Until now, consensus did not
  bound a transfer output's cluster tags. One spend could credit any other
  holder's cluster with an unlimited amount of wealth, which raised that
  cluster's fee and demurrage factor. Two rules now apply:
  - Rule 1: every output's tag vector must satisfy
    `ClusterTagVector::is_valid` (at most 16 sorted, unique entries totalling
    at most 100%). Block validation, SCP intrinsic validity and mempool
    admission all enforce it.
  - Rule 2: an output may name a cluster that no ring member carries only if
    it is the bridge-import cluster of the including block's epoch. Block
    validation enforces it, and mempool admission checks it at `tip + 1`.
    Admission also refuses an import-tagged tx whose inclusion window
    crosses an epoch.
  Honest wallets (`merge_weighted` inheritance), the web signer (empty tags),
  the bridge release (`single(import cluster)` plus empty change), coinbase,
  lottery and settlement outputs already satisfy both rules. No storage
  format change.
- **Block timestamps use median time past (#1848).** A block or minting tx
  extending height `h` is valid only if `timestamp > MTP(h)`. `MTP(h)` is the
  median of the last 11 non-genesis header timestamps, zero-padded on a short
  chain. This rule replaces the 7.x `timestamp >= parent` rule. Under the old
  rule, one tip forward-dated up to the future bound rejected every honest
  `now` stamp, so the forward-dater became the only valid minter (found by the
  Lean model, #1856). `ChainState` carries the 11-timestamp window. It is
  written in the block write txn and carried and verified by V2 snapshots.
  A ledger store past genesis without the window is refused at open, and
  pre-8.0.0 V2 snapshot files are refused as an unsupported schema.
- **Future bound 2 h → 60 s (#1848).** `MAX_FUTURE_TIMESTAMP_SECS` is now 60 in
  both the SCP/gossip path and block apply. Validator and minter clocks must
  be within about 60 s of real time.
- **Difficulty floors the observed gap (#1848).** The controller is fed
  `max(ts - parent, 3 s)` (`MIN_OBSERVED_BLOCK_TIME_SECS`, equal to the slot
  floor). Zero and backward gaps, which MTP now admits, cost one bounded 0.6x
  step. `compute_time_adjusted_difficulty` itself is unchanged.
- **Honest minters stamp `max(now, MTP + 1)` (#1848).** The minter's work
  carries `min_timestamp`, so an honest minting tx is never invalid on its
  timestamp alone. This holds even when a forward-dated majority of the window
  holds the median ahead of the clock.

## [Unreleased]

### Added
- **Consensus-breaking (protocol 7.0.0 reset):** LotteryV2 payouts are active on
  the production ledger from genesis (#1640, #1668), on the new
  `botho.ledger.2` schema with derivation-context storage (#1639, #1652) and
  versioned V2 snapshots (#1646, #1664). The draw and the #573 candidate window
  are seeded from the parent block's RandomX PoW output, not its header hash
  (#1676, #1679). Header checks run before the V2 draw, and add_block enforces
  the ML-KEM ciphertext shape (#1670, #1677, #1680). This builds on the LotteryV2
  primitives, storage and atomic ledger transitions from #1302, #1351, #1353,
  #1357 and #1365.
- Every wallet discovers and spends V2 lottery payouts: the native node wallet
  (#1644, #1661), `botho balance`/`send` via view-key scan (#1662, #1665), the
  WASM, web and mobile clients (#1645, #1658), the bridge reserve (#1657,
  #1666) and the stress harness (#1642, #1643, #1659, #1660). Detached native
  payout recovery and spending are authenticated (#1499, #1672), all consumers
  share one set of CLSAG payout helpers (#1667, #1671, #1673), and
  `chain_getOutputs` exposes V2 payout context and real cluster tags (#1641,
  #1655).
- P12 internal review of LotteryV2 payouts (`audits/`, #1647, #1678).
- Proposed confidential-transaction contract and integer-proof gates, bounded
  composition experiments, and resource/economic measurements (#1266, #1284,
  #1288, #1311, #1335, #1347, #1350, #1356). This is design and research
  evidence; current testnet amounts remain public.
- Durable Squads mint intents through the bridge engine, finalized execution
  recovery, and bounded retries with marker-rent validation (#1300, #1317,
  #1322), backed by a real localnet vault-mint CPI test (#1279).
- PR CI executes 15 Botho integration suites and the consensus regression
  suites; secp256k1 tests and metrics-daemon checks also run in CI (#1259,
  #1276, #1282, #1287).

### Fixed
- Wallet decoy fallback preserves unique targets; standalone wallet feature
  builds work again (#1338, #1369).
- Web wallet connection changes cancel stale work; the desktop production
  build uses Vite 8's native minifier (#1321, #1328).
- Ledger test concurrency respects the macOS SysV semaphore undo budget
  without changing host limits (`scripts/test-ledger.sh`, #1362).
- Chain sync handles bad peers without stalling or falsely reporting synced:
  stale request failures no longer demote a caught-up node, Discovery rounds
  exclude silent and banned peers under a bounded deadline, reputation bans
  expire (5 min, doubling to a 1 h cap) with an all-banned fallback, and block
  responses earn reputation only after they apply (#1431, #1722, #1723, #1802,
  #1813, #1815, #1819, #1827, #1832, #1834).

### Security
- Bumped zstd 0.13 → 0.14.3 to clear RUSTSEC-2026-0391, -0398, -0403 and -0410
  (#1617, #1654).
- Remediated HTTP/TLS and Bulletproofs dependency advisories, removed the
  vulnerable Solana native bigint conversion, and migrated DNS/libp2p/WebRTC
  dependencies with bounded connection cleanup (#1269, #1303, #1312, #1316,
  #1330).
- **Consensus-breaking (rides the protocol 7.0.0 reset):** `Transaction::hash`
  and `Transaction::signing_hash` now commit to every output field, including
  `e_memo`, `cluster_tags` and `kem_ciphertext`, under new domain separators.
  Those fields can no longer be altered without invalidating the signatures.
  There is no legacy signing mode; every producer signs through the shared
  function (#1638, part of #1286).

### Changed
- contracts/ethereum migrated to the Hardhat 3 toolchain (#1185).
- Mobile app migrated to Expo SDK 57, with a first mobile CI workflow
  (typecheck + jest + expo-doctor) gating `mobile/app/**` (#1184).
- Workspace crypto migrated to the dalek-5 family (curve25519 / ed25519 /
  x25519); a bridged `curve25519-dalek-v4` alias survives only for the
  bulletproofs-og range-proof boundary (#1188).
- rand family migrated to the rand_core 0.10 line;
  `bth_util_from_random::OsRng` replaces the retired `rand_core::OsRng`
  (#1193).

## [0.6.0] - 2026-07-16

### Changed (consensus-breaking — requires a coordinated testnet reset)
- Ships protocol 6.0.0 with universal hybrid ML-KEM output enforcement and
  bound lottery payouts (#974), plus value-free Path C lottery selection
  (#980).
- Address-format v2 and hybrid output construction/scanning are shared across
  native and browser wallet paths (#964, #967, #969, #971, #977, #984, #989).
  Transaction amounts remain public in this release.
- Added demurrage class-transition charging and the demurrage-settlement
  operation used by the bridge wrap on-ramp (#953, #956).

See the [published release notes](https://github.com/botho-project/botho/releases/tag/v0.6.0)
for the complete release diff.

## [0.4.0] - 2026-07-15

### Changed (consensus-breaking — requires a coordinated testnet reset)
- Ships protocol 5.0.0 (#949), including epoch-keyed bridge-import cluster
  tagging with the ADR 0007 factor floor (#942).

### Added
- Bridge reserve release with exactly-once claims and threshold attestation,
  federation envelope transport, and equivocation detection (#840, #847,
  #873, #891).

See the [published release notes](https://github.com/botho-project/botho/releases/tag/v0.4.0)
for the complete release diff. Dates above use the GitHub release publication
date in UTC.

## [0.3.2] - 2026-07-07

### Added
- Explorer-facing RPC enrichment (#696): `getBlockByHeight`/`getBlockByHash`
  gain additive `transactions` (hash/fee/ringSize — structure only, never
  amounts or recipients), `totalFees`, and `lottery` summary fields;
  `cluster_getAllWealth` gains a per-cluster `factor` from the live fee
  curve (single source of curve truth for the explorer's wealth histogram).
- Fleet metrics daemon (#697): multi-node collection with per-node SQLite
  rollups and a public history API for the wallet's /network dashboard.

### Fixed
- Nodes re-dial configured bootstrap peers whenever under-connected, not
  only at zero peers (#690) — rolling deploys can no longer strand the
  fleet in a star topology that wedges SCP externalization.

## [0.3.1] - 2026-07-07

### Fixed
- **Node: `tx_submit` now relays transactions to peers regardless of minting
  state** (#674). Previously a non-minting ingress node (the web wallet's
  default) was a black hole for RPC-submitted transactions — sends, claim
  links, and pay flows silently never confirmed. Accepted txs are gossiped and
  SCP-cache-registered at accept time, plus a 30s mempool re-announce tick.
- Web wallet: `/pay` and `/claim` inline wallet create/import now enforce the
  required-password policy — no more plaintext seeds from the link funnels
  (#672); WalletGate lands on the unlock form for locked wallets and
  overwriting a stored wallet requires explicit confirmation (#673).
- Web wallet: transaction history shows per-event rows with netted send
  amounts, real confirmations/status, and block heights instead of fabricated
  "just now" timestamps (#675).
- Two broken `network/privacy` doctests; doctests now run in CI (#670).
- Stale cluster-factor e2e expectations rescaled to the picocredit curve;
  `e2e_progressive_fees` now runs in CI (#669).

### Changed
- Release builds enable `overflow-checks` (curve25519-dalek exempted for
  performance); attacker-influenced sums on consensus paths saturate instead
  of silently wrapping (#663).

### Security
- `crossbeam-epoch` 0.9.18 → 0.9.20 (RUSTSEC-2026-0204); cargo-deny PR gate
  green again (#679).
- Cycle-7 internal audit report committed (`audits/2026-07-05-cycle7.md`).

## [0.3.0] - 2026-07-05

### Summary

Consensus-breaking release (requires a coordinated network reset). Ships two
protocol bumps — `2.0.0 → 3.0.0 → 4.0.0` — that pin the current testnet: a
deterministic consensus fee floor (H1) and a log-domain cluster-factor curve
with recalibrated `w_mid` and `u128` cluster-wealth accounting (#626). Also
lands the CLI thin wallet's real CLSAG ring transactions, regional testnet
seeds on by default, and a validated, reproducible release pipeline.

### Changed (consensus-breaking — requires network reset)

- **Deterministic consensus fee floor in `add_block`.** The minimum-fee rule is
  now enforced in the consensus path with integer-only arithmetic; the mempool
  relay fee may only tighten *above* the consensus floor, never below it (H1).
  Block fee-sum arithmetic rejects/saturates on overflow instead of wrapping.
  Protocol version bumped to `3.0.0` for the required reset. (#600, #601, #602,
  #603, #606, #608)
- **Log-domain cluster-factor curve in picocredits.** The cluster-tax factor is
  computed on a log-domain curve with a recalibrated `w_mid`, the
  `cluster_wealth_db` accumulator is widened `u64 → u128` to avoid overflow at
  full-supply scale, and the real curve is exercised by an M2 simulation
  harness. Protocol version bumped to `4.0.0` for the required reset. (#626,
  #627, #628, #629, #631)
- **M2 cumulative cluster-wealth semantic ratified** as a design decision
  (spend-time cumulative accrual). (#605, #630)
- **Cluster-wealth rebuild uses `saturating_add`** for determinism parity (M3).
  (#607)

### Added

- **Real CLSAG ring transactions in the CLI thin wallet** — the wallet now
  builds actual ring signatures instead of placeholder transactions. (#614,
  #620)
- **Live testnet regional seeds included in the fallback set by default**, so a
  fresh node can discover the network without manual seed configuration. (#613,
  #624)

### Fixed

- **Wallet RPC serde field-case drift** vs the current node repaired — restores
  wallet ↔ node compatibility. (#610, #617)
- **Decoy selection clamp panic** — phase-1 in-band draw is skipped when the age
  band is degenerate, avoiding an out-of-range clamp. (#611, #618)
- **Reproducible release pipeline portability.** Portable `sha256` wrapper for
  macOS runners (`sha256sum` is Linux-only), and the `botho-wallet` Windows
  file-ACL code fixed to compile against `windows` 0.58 (module paths, features,
  `WIN32_ERROR`). Validated across all five release targets. (#615, #619, #621,
  #622)

### Docs & Security

- **Threat model refreshed** for audit cycle-6 and the economic mechanism.
  (#616, #625)
- **Testnet runbook updated** for the protocol resets; stale `--relay` flag
  references removed; mainnet-blocker list refreshed. (#609, #612, #623)

## [0.2.0] - 2026-06-15

### Summary

Consensus-breaking release (requires a coordinated network reset — see #323).
Hardens supply integrity, sets the canonical emission schedule, and reconciles
all economic documentation with the shipped code. Follows an extended fuzzing
and audit pass that surfaced and closed a critical inflation vector.

### Changed (consensus-breaking — requires network reset)

- **Emission schedule set to ~1-year halvings.** `mainnet_policy()` halving
  interval is now `BLOCKS_PER_YEAR` (6,307,200 blocks, ~1 year at 5s), 5
  halvings → ~611M BTH Phase-1 supply, reaching the perpetual 2% net tail in
  ~5 years. Chosen from an agent-based emission-schedule sweep (#350/#352).
  (#362)
- **Supply accounting widened to `u128`.** `ChainState.total_mined` /
  `total_fees_burned`, the `EmissionController` mirrors, and the cumulative
  lottery-pool carryover are now `u128`; on-disk metadata grows 8→16 bytes.
  Per-amount fields (`TxOutput.amount`, reward, fee) remain `u64`, so the
  block/tx wire format is unchanged. RPC supply fields now emit as JSON
  strings (exceed JS 2^53). (#342, #346)
- **Per-input CLSAG balance verification** for multi-input transactions
  (audit I4). (#317)
- **Lottery payout UTXOs are minted** and burn accounting corrected (M4). (#323-era)

### Fixed

- **Critical: inflation via output-sum overflow.** `total_output()` summed
  output amounts with an unchecked `u64` that wrapped under
  `overflow-checks=false`, defeating the balance check; overflowing
  transactions are now rejected. Found by fuzzing. (#340)
- Cluster-tax simulation supply accumulators widened to `u128` to avoid
  overflow at full-supply scale. (#344)
- `agents_by_wealth()` made a total order (AgentId tie-break) to remove
  cross-process nondeterminism in simulations. (#360)
- Faucet `getStatus` no longer over-reports `dailyDispensed` across the UTC
  midnight boundary. (#366)
- Web wallet + explorer wired to a reachable seed RPC; connection and search
  bugs fixed. (#322)
- `totalMined` RPC field asserted as a parseable decimal string. (#348)

### Added

- **`getBlockByHash` RPC** and explorer block-by-hash / tx-by-hash lookups. (#367)
- **Emission-schedule sweep** simulation subcommand + comparison report under
  `experiments/results/`. (#352)
- **Fuzzing suite revived** against the current API with a PR build guard, plus
  4 consensus-critical fuzz targets (add_block, multi-input balance, lottery,
  cluster-tax math). (#338, #339)
- **Repo-wide rustfmt baseline + CI fmt check** on the pinned nightly. (#355)
- GTK/glib system deps for the workspace benchmark CI job. (#358)

### Documentation

- Whitepaper reconciled with the shipped ~1yr/611M/2%-tail schedule and 5s
  block timing; PDF rebuilt and deployed copy regenerated. (#321)
- README + `docs/` monetary parameters reconciled with shipped code; fee
  economics corrected to the 80/20 lottery+burn model. (#324, #364)

### Testing

- Privacy-metrics tests serialized to remove process-global-state races. (#359)
- Sybil-resistance lottery simulation made deterministic (seeded RNG). (#349)
- Explorer e2e made hermetic via a mocked `/rpc` endpoint. (#334)
- 5-node consensus and tx-lifecycle e2e tests re-enabled for the lottery era.

## [0.1.0] - 2026-06-12

### Summary

First tagged release of Botho — a privacy-first, anti-hoarding cryptocurrency
derived from MobileCoin with significant simplifications (no SGX, no Fog).
The node runs end-to-end on the pre-mainnet testnet with SCP consensus,
CLSAG ring signatures, ML-KEM-768 stealth addresses, ML-DSA-65 PQ
authorization, cluster-tagged progressive fees, redistribution lottery,
spend-time demurrage, and Onion Gossip network privacy.

This release marks the audit-hardened state after cycle 6 of the internal
security audit. All Critical block-acceptance findings from cycle 6 (C1–C4)
are resolved; one Critical (C5: float-based difficulty controller in dead
code) remains open and will be addressed in 0.2.0.

### Added — Consensus

- Stellar Consensus Protocol (SCP) with 3–5 s finality
- Parallel proof-of-work block minting
- Block-acceptance hardening (audit cycle 6, C1–C4):
  - Chain-expected difficulty enforced against the header
  - Block reward recomputed from the emission schedule
  - Timestamp monotonicity vs parent + 2 h future-skew bound
  - Transaction root re-derived and compared to `header.tx_root`
  - Every CLSAG ring member resolved against the UTXO set
- Lottery payout-suppression check: validators re-run the deterministic
  draw on no-winner blocks and reject suppressed payouts
- Cluster-tilted redistribution lottery with carryover pool
- Spend-time demurrage and progressive cluster-wealth fees
- Atomic block + lottery-pool state commit in one LMDB transaction
- Dynamic block timing (5–40 s, adapting to load)
- Block-height-based halving + perpetual tail emission

### Added — Cryptography

- Stealth addresses (ML-KEM-768) — recipient privacy, post-quantum
- CLSAG ring signatures (ring size 20) — sender privacy
- ML-DSA-65 — post-quantum transaction authorization
- Migrated `ml-dsa` to 0.1.1, fixing RUSTSEC-2025-0144 (timing
  side-channel in ML-DSA decomposition)
- Pedersen commitments — amount hiding, information-theoretically secure

### Added — Network

- libp2p networking with peer discovery
- Onion Gossip with 3-hop routing for transaction-origin privacy
- Compact-block relay with mempool reconstruction
- Protocol-version disconnect on consensus-incompatible peers
  (fails closed on unparseable agent strings)
- Pluggable transports (QUIC, WebRTC, TLS-tunnel) with negotiation
- Traffic-padding `TrafficNormalizer` for fingerprint resistance

### Added — Wallet & Tooling

- CLI wallet with 24-word BIP-39 mnemonic
- Stealth-address detection / UTXO scanning
- Progressive-fee estimation
- Faucet web UI with rate limits and per-address caps
- WebSocket subscriptions for live block / tx events
- JSON-RPC API with view-key registration

### Added — Build & Release

- Reproducible release builds for Linux x86_64, macOS Intel + ARM,
  Windows — verifiable by third parties via `./scripts/build-release.sh`
  and SHA256SUMS comparison
- Coverage / benchmarks / fuzz / e2e / security CI workflows

### Security

- Internal audits documented in [`audits/`](./audits/) (cycles 1–6)
- 9 of 15 `cargo audit` advisories resolved during cycle 6 (`ml-dsa`,
  `bytes`, `quinn-proto`, `rkyv`, `time`, `rustls-webpki`)

### Known Limitations

- Pre-mainnet — running on testnet only; mainnet launch not scheduled
- Cycle 6 finding C5 (float-based live difficulty controller) is open
- 6 `cargo audit` advisories remain (DoS / cert-validation in DNS &
  telemetry paths, gated behind upstream `libp2p` and `sentry` bumps)
