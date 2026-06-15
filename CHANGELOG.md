# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

While Botho is pre-1.0, the leading `0.` indicates the on-chain protocol is
still mutable. Within the `0.x` line, a `MINOR` bump (`0.X.0`) signals
consensus-breaking changes that require a coordinated network reset; `PATCH`
bumps (`0.X.Y`) are backwards-compatible.

## [Unreleased]

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

