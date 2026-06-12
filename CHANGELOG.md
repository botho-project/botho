# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

While Botho is pre-1.0, the leading `0.` indicates the on-chain protocol is
still mutable. Within the `0.x` line, a `MINOR` bump (`0.X.0`) signals
consensus-breaking changes that require a coordinated network reset; `PATCH`
bumps (`0.X.Y`) are backwards-compatible.

## [Unreleased]

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

