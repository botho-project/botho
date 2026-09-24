# Snow fork testnet canary — September 24, 2026 UTC

**Result: bounded live compatibility check passed; original EU binary restored.**

The existing `eu.seed.botho.io` node ran Botho candidate `340cf303814e5ac82f5deb0b6d1e9dd24ceba9b2`, based on the deployed `6dbcb92463214694f3122a05c9c48986d4f4f1b7` revision with only the Snow dependency override and its lockfile changes. Snow was pinned to `rjwalters/snow@8192d73c676ecdc93f91c5621caaa95245969ccc`, the follow-up in [snow#219](https://github.com/mcginty/snow/pull/219). The branch-only build workflow produces the experimental ARM64 artifact and publishes no release.

[Linux ARM64 build and validation](https://github.com/botho-project/botho/actions/runs/35938080677) passed all **18 network integration tests**. Separate local Snow checks passed default/XChaCha and both ring configurations, including all three vector corpora, sparse cipher configurations, configured feature checks, and clippy. Full Snow platform/MSRV CI remains unverified.

## Live evidence

- Run `snow-20260924T002911Z`; EU preserved its existing node identity, configuration, genesis and ledger. Other nodes kept the original binary.
- Candidate binary SHA-256: `920c65de319c1378de81655b7eaf3843d5287e4c1de09cc21f15d2a4940c0a62`.
- The forked node reconnected, remained synced, and shared the baseline ancestry with all five nodes. No handshake/decryption errors appeared in the sampled EU logs.
- Exactly one normal **1 BTH** grant was requested at 00:30:08 UTC. Transaction `aa9aa3c44139347188f7e862d7531cf2fc36592f03c4eb9548e311787afd3dd1` propagated to all five mempools and confirmed across the fleet at 00:32:35 UTC: **147.378 seconds** after the submission marker.
- Inclusion block **2646**: `1a4f2e7d8e4dc4d85d29d21a1d605b1e8b9bc45e5617ccb5861090cbd013abd2`. Fee **100,000,000 picocredits (0.0001 BTH)**.
- The protected wallet restore/scan verified one owned output worth exactly 1 BTH using the EU node's inclusion-block outputs. This check covers that block; it does not prove a full-chain rescan or subsequent spend.
- A persistent 30-minute rollback timer was armed before replacement. After payment verification, explicit rollback restored the original running binary SHA-256 `88c169423fdc003a5dbc6d688852fdbcda26b78f85cd371de29d9192571c674f`. The final fleet check passed with every node back on the deployed base and the payment block preserved. The timer was disabled after successful restoration.

The one-shot helper initially compared the faucet's decimal-string `amount` with an integer and classified the successful reply as uncertain. It did **not** resubmit. Read-only faucet log/mempool reconciliation recovered the unique full transaction hash, then independent fleet receipts and wallet scanning established the result. The helper now accepts the documented decimal string and durably records the response before validation. The original uncertainty evidence was retained.

## Limits and follow-up

Native `libp2p-noise` selects Snow's ring backend. This live run therefore establishes Botho/native integration and wire compatibility; Snow's RustCrypto vector tests exercise the migrated AEAD implementations directly. One payment and a short mixed-version run do not establish sustained-load, web/Snap, confidential-amount, or 72-hour acceptance.

The previous 72-hour controller never reached T0. Its funding loop halted because submission jitter made a fixed offer arrive before the preceding submission plus 900 seconds. A separate local patch defers within the original admission slot, retaining expiry and hard caps; all 25 controller tests pass. That patch is not deployed, and no new long-running workload was started here. Setup must still establish mature outputs, production decoys, signer probes, rescan agreement and exact accounting.

The fork permits testing ahead of an upstream release, but does not complete #1429: wallet/WebRTC consumers still retain the older ChaCha dependency. Neither the experimental branch nor a fleet-wide fork deployment is merged into main.

[Sanitized machine-readable evidence](testnet-snow-canary-evidence/2026-09-24.json) records exact sources, checksums, fleet snapshots and receipts. Wallet recovery material stays in a protected directory on the worker; no secrets are included.
