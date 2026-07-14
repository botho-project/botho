# External Security Audit — Engagement Scope

**Status**: DRAFT for firm selection (issue #616, mainnet blocker 2).
**Prepared**: 2026-07-08. All four "settle the audit surface" preconditions
cleared 2026-07-07 (#581 cluster-tag bound, #532 view-change decision,
RandomX ratification, H4 disposition #715).
**Audience**: candidate audit firms (shortlist per #50: Trail of Bits,
NCC Group, Cure53) and the engaging operator (2amlogic).

This document defines what is in and out of scope, the freeze artifact, the
prior-work package auditors receive, and the known/accepted findings we are
explicitly NOT asking to be rediscovered.

---

## 1. Engagement parameters (operator fills in)

| Parameter | Value |
|---|---|
| Firm | TBD (shortlist: Trail of Bits, NCC Group, Cure53) |
| Budget | TBD (2amlogic) |
| Audit window | TBD (firm lead time 4–8+ weeks expected) |
| Freeze artifact | A tagged reproducible release — `v0.3.2` or a fresh `v0.3.x` cut for the engagement (`release.yml` produces the artifacts; see §3) |
| Point of contact | TBD |
| Test environment | Live public testnet (5 nodes, 3 continents, protocol 4.0.0) + dedicated audit nodes on request; everything pre-mainnet is disposable, so destructive testing on provided infrastructure is acceptable by prior arrangement |

## 2. Project summary for auditors

Botho is a Rust CPU-mineable (RandomX) cryptocurrency with SCP-based
federated consensus and a novel anti-concentration economic mechanism:
cluster-tilted lottery redistribution, demurrage, and a deterministic
consensus fee floor, all enforced at block acceptance. Privacy uses CLSAG
ring signatures with RingCT/Bulletproofs and Pedersen commitments, extended
with cluster tags for the economic mechanism. Mining is economically coupled
but consensus-decoupled: PoW earns rewards, SCP quorums decide finality, and
PoW weight never influences consensus.

Design pillar the audit should stress: **no hard forks, ever**. Every
consensus-relevant computation must be bit-deterministic across nodes
(integer/`BTreeMap`-only math in the acceptance path). Anything an attacker
can use to make two honest nodes disagree about a block's validity is a
Critical.

## 3. Freeze artifact and build

- Audit a **tagged release**, not a moving branch. `release.yml` builds
  reproducible artifacts (verified dry-run + real tags `v0.3.0`–`v0.3.2`).
- Workspace layout: `botho/` (node), `botho-wallet/` (wallet lib + CLI),
  `blockchain/types/`, `consensus/{scp,quorum-sim}/`, `transaction/{clsag,
  core,signer,types}/`, `ledger/`, `web/packages/*` (web wallet, BaaS
  worker), `infra/`.
- Release profile ships `overflow-checks = true` with one documented
  exemption (`curve25519-dalek`, upstream-audited constant-time limb math;
  rationale and benchmarks in root `Cargo.toml` §profile.release).

## 4. In-scope areas

### 4.1 Cryptography
- CLSAG ring signatures: `transaction/clsag/`.
- RingCT / Bulletproofs / Pedersen commitments: `transaction/core/src/ring_ct/`
  (incl. `rct_bulletproofs.rs`, generator cache).
- Cluster-tag commitments and conservation proofs (Botho-specific extension —
  highest-value crypto target: it is novel, consensus-enforced, and has no
  external prior art).
- Domain separation: `transaction/types/src/domain_separators.rs`.
- Key hierarchies: BIP39 wallet derivation, one-time output keys, Ed25519
  libp2p node identity.
- RandomX integration (parameterization and verification path only; RandomX
  itself is externally audited upstream).

### 4.2 Consensus
- SCP implementation and integration: `consensus/scp/`, node-side driving
  logic in `botho/src/consensus/` and `botho/src/commands/run.rs`.
- Quorum promotion gate: `gated_scp_quorum_set` /
  `symmetric_quorum_has_intersection` (`botho/src/commands/run.rs`), backed
  by the brute-force FBAS analyzer in `consensus/quorum-sim/` — the gate
  refuses candidate quorum sets admitting disjoint quorums. Adversarial
  question: can any input sequence (peer churn, config, operator actions)
  install a non-intersecting or degenerate quorum set?
- Competing-coinbase model (no view-change by design — decision record on
  #532 with Phase-0 simulation + live evidence).

### 4.3 Block acceptance & economic consensus rules
`Ledger::add_block_inner` (`botho/src/ledger/store.rs`) enforces, per block,
the seven consensus gates (C1–C7 from internal cycles 6–7): declared
difficulty, reward recompute + timestamp bounds, ring-member/UTXO binding,
tx_root recompute, integer difficulty controller, cluster-tag inheritance
bound (per-ring maxima, #581/PR #713), and the deterministic consensus fee
floor. Plus:
- Lottery redistribution: seed-rotated candidate window (#573), tilted
  selection, payout accounting; H4 grindability disposition (#715) —
  accepted as economically inert, auditors should test that acceptance.
- Demurrage: max-quantile ring age + centroid-floored cluster factor
  (decoy-resistant, #596/#582).
- u64→u128 cluster-wealth widening: fail-closed on-disk decoding, saturating
  math pinned to the conservative consensus direction (#626).
- Emission schedule (5yr/2%/~611M, #351) and crash-atomicity: block +
  emission + difficulty state in a single LMDB write txn.

### 4.4 Transaction validation & mempool
Key-image double-spend checks (fail-closed on DB errors), fee/overflow
arithmetic, mempool admission vs consensus-rule parity (a tx admitted to the
mempool but rejected at acceptance must never split the network).

### 4.5 Network
libp2p stack (gossipsub, DNS seeds, mdns), peer discovery and reconnect
logic, message parsing on untrusted input (no-panic posture), rate limiting
and connection caps, transport security (see `docs/security/
transport-security.md` and the phase-1 onion-gossip audit). Known
node-reachable dependency advisories are already tracked (#659 hickory,
#661 sentry/rustls) — status at freeze time will be stated in the handoff.

### 4.6 Wallet stacks
- Web wallet (`web/packages/`): vault at-rest crypto (AES-256-GCM +
  PBKDF2-SHA256 600k), claim links, wasm signer, RPC trust boundaries.
- Wallet library + CLI (`botho-wallet/`).
- The RPC surface nodes expose to wallets, incl. exchange-endpoint HMAC auth
  (`botho/src/rpc/auth.rs`).

### 4.7 Product control planes
- BaaS worker (`web/packages/baas-worker/`): Stripe webhook signature
  verification, magic-link status tokens, EC2 provisioning with
  least-privilege IAM, reconciliation cron. (Stripe TEST mode only; LIVE
  billing is out of scope, gated separately on #722.)
- Operator dashboard read surface (#707): read-token verification,
  per-peer quorum classification exposure.
- Quorum write path: the security design is
  `docs/security/quorum-write-path.md` (review-gated on #708). If the
  implementation (#709) lands before freeze, it is in scope as the highest-
  privilege remote surface; if not, the design doc itself is offered for
  review comment.

### 4.8 Bridge proof-of-reserve under confidential amounts (forward flag)

The BTH↔wBTH bridge audit is scoped separately (#830) and remains the first
call on the external-audit budget. One item is flagged here because it
couples to the core-protocol roadmap: once confidential amounts land
(ADR 0006 Decision 1, epics #902/#904), third parties can no longer read the
bridge reserve balance from public ledger amounts, and proof-of-reserve
becomes a **federation view-key / attested-opening disclosure protocol**
(see `docs/design/post-ct-analytics.md` §3). The audit must cover: what is
disclosed and by whom, key rotation/revocation, and whether a spoofed or
stale disclosure can fake solvency. This is in scope regardless of whether
CT lands before or after the engagement, because the disclosure design
constrains how the reserve address is structured today.

## 5. Prior-work package (provided to auditors)

- `docs/security/threat-model.md` — refreshed through cycle 7 + the
  2026-07-07 hardening; every behavioral claim code-verified at review time.
- `audits/` — seven internal audit cycles (2025-12-30 → 2026-07-05), with
  per-finding disposition. Cycle 6 (`2026-06-11-cycle6.md`) is the deepest
  single document; cycle 7 (`2026-07-05-cycle7.md`) verifies its closures.
- `docs/design/` — mechanism design docs (lottery redistribution,
  cluster-tilted redistribution, ring-signature tag propagation and privacy
  analysis, asymmetric fees, entropy-proof analyses).
- `consensus/quorum-sim/README.md` — FBAS safety/liveness framing and the
  verified 2-of-4 fork counterexample.
- Transport security audits (`docs/security/transport-security-audit-2024.md`,
  `onion-gossip-phase1-audit.md`).

## 6. Known findings and accepted risks (do not re-report as new)

| Item | Disposition |
|---|---|
| H4 lottery candidate-cap grindability | **Accepted** as economically inert (#715, analysis in threat model). In scope to *challenge the acceptance*, not to rediscover. |
| M2 cluster wealth = cumulative volume, not holdings | **Ratified design decision** (#605/#630, 4–11× dGini margin without decay). |
| Cycle-6 M3–M6, L1, L3 | Open, tracked, low-priority; list in cycle-6 report. |
| Lottery payout privacy (winners visible on-chain) | Known testnet watch item; phase-2 (Pedersen payout blinding) not yet scheduled. |
| "Everyone parks" demurrage drift | Countermeasure designed (eligibility decay), not yet implemented; watch item. |
| Node-reachable dep advisories | Tracked #659 (hickory 0.26 via libp2p), #661 (sentry → rustls 0.23). CI `cargo deny` gate active with justified ignores. |
| `curve25519-dalek` overflow-checks exemption | Documented, benchmarked (#663). |

## 7. Out of scope

- Testnet operations infrastructure (faucet service, nginx configs, seed-node
  hosting, metrics daemon) — disposable pre-mainnet plumbing.
- Stripe LIVE-mode billing and legal/regulatory posture (#722, separate
  workstream).
- Development tooling (Loom orchestration, Anvil, CI beyond the release
  pipeline's supply-chain properties).
- RandomX internals (upstream), rust-libp2p internals (upstream) — their
  *integration* is in scope per §4.1/§4.5.

## 8. Deliverables requested from the firm

1. Findings report with severity ratings and per-finding reproduction.
2. Explicit verdicts on the two Botho-novel surfaces: the cluster-tag
   commitment scheme (§4.1) and the economic-consensus gates (§4.3),
   including determinism/fork-risk review of both.
3. A statement on fitness of the quorum promotion gate as the sole
   constructor of SCP quorum sets (§4.2).
4. Re-test pass after remediation of Critical/High findings.
