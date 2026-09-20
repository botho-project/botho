# External Security Audit — Engagement Scope

**Status**: DRAFT for firm selection (issue #616, core mainnet gate).
**Prepared**: 2026-07-08; implementation inventory refreshed 2026-09-19.
The four original "settle the audit surface" preconditions cleared in July
(#581, #532, RandomX, H4/#715). Subsequent changes and pending confidential
amounts require a new freeze-specific scope reconciliation; that historical
milestone does not freeze the current code.
**Audience**: candidate audit firms (shortlist per #50: Trail of Bits,
NCC Group, Cure53) and the engaging operator (2amlogic).

This document defines what is in and out of scope, the freeze artifact, the
prior-work package auditors receive, and the known/accepted findings we are
asking auditors to reassess against the freeze. The
[mainnet readiness checklist](../operations/mainnet-readiness.md) separates
implemented code, historical/live evidence and missing sign-offs. This draft
is not an engagement agreement or a finding of mainnet fitness.

---

## 1. Engagement parameters (operator fills in)

| Parameter | Value |
|---|---|
| Firm | TBD (shortlist: Trail of Bits, NCC Group, Cure53) |
| Budget | TBD (2amlogic) |
| Audit window | TBD (firm lead time 4–8+ weeks expected) |
| Freeze artifact | TBD: exact tagged release, commit, build features and checksums; record any CT rollout and subsequent review window explicitly (`release.yml`, §3) |
| Point of contact | TBD |
| Test environment | Operator to inventory deployed versions, hosts, genesis and permissions at freeze. Source advertises protocol 6.0.0; no current fleet observation is asserted here. Dedicated/destructive testing requires prior arrangement on designated disposable infrastructure. |

## 2. Project summary for auditors

Botho is a Rust CPU-mineable (RandomX) cryptocurrency with SCP-based
federated consensus and a novel anti-concentration economic mechanism:
redistribution, demurrage, and a deterministic consensus fee floor, enforced
at block acceptance. The active transaction format uses CLSAG ring signatures
and hybrid ML-KEM recipient outputs, but **amounts remain public**.
`transaction/clsag/src/lib.rs` publishes `TxOutput.amount` and
`ClsagRingInput.pseudo_output_amount`, using zero-blinded amount commitments.
Amount matching can eliminate decoys: do not infer sender anonymity from ring
size. RingCT/Bulletproof helpers exist, but confidential amounts are a pending
integration under #902/#904, not a shipped property of this transaction path.
Mining is economically coupled but consensus-decoupled: PoW earns rewards, SCP quorums decide finality, and
PoW weight never influences consensus.

Design pillar the audit should stress: **no hard forks, ever**. Every
consensus-relevant computation must be bit-deterministic across nodes
(integer/`BTreeMap`-only math in the acceptance path). Anything an attacker
can use to make two honest nodes disagree about a block's validity is a
Critical.

## 3. Freeze artifact and build

- Audit a **tagged release**, not a moving branch. Attach build features,
  checksums and independent reproducibility results for that tag. Historical
  release work (#615 and RandomX follow-up #640) is closed; it does not prove
  the selected future artifact reproduces. See [release verification](../operations/reproducible-builds.md).
- Workspace layout: `botho/` (node), `botho-wallet/` (wallet lib + CLI),
  `consensus/{scp,quorum-sim}/`, `transaction/{clsag,
  core,signer,types}/`, `botho/src/ledger/`, `web/packages/*` (web wallet, BaaS
  worker), `infra/`.
- Release profile ships `overflow-checks = true` with one documented
  exemption (`curve25519-dalek`, upstream-audited constant-time limb math;
  rationale and benchmarks in root `Cargo.toml` §profile.release).

## 4. In-scope areas

### 4.1 Cryptography: active path versus target

| Surface | Inventory and audit treatment |
|---|---|
| CLSAG and public amounts | `transaction/clsag/src/lib.rs`: active ring inputs/outputs, signing and amount balance checks. Audit amount-matching and public-tag leakage; do not describe current amounts as confidential. |
| Hybrid recipient outputs | Same crate plus `botho/src/consensus/validation.rs`: ML-KEM ciphertext support and enforcement at protocol major ≥6 with `pq` enabled. Pin release features and audit wallet scanning/decapsulation. #904 records the shipped rollout; encrypted-memo confidentiality has a separate classical-only caveat. |
| CT target and supporting primitives | `transaction/core/src/ring_ct/` (including `rct_bulletproofs.rs` and generator cache) and `transaction/core/src/range_proofs/` contain supporting code. Their existence is not proof of active-path integration. #902/#904 and proposed ADR 0009 govern the pending economics specification and rollout. Scope the final integration, proof soundness/conservation, public-fee leakage budget and CT-compatible factor/demurrage verification explicitly. |
| Cluster tags/economics | Review actual enforcement in `botho/src/ledger/store.rs` and `transaction/core/src/validation/validate.rs`, distinguishing public tags/bounds from any target commitment/proof scheme. Do not assume hidden amounts or exact blend proofs are consensus-enforced today. |
| Domain separation and keys | `transaction/types/src/domain_separators.rs`, BIP39 account derivation, hybrid one-time output keys and Ed25519 node identity. |
| RandomX integration | Parameterization, PoW attribution binding and verification; upstream RandomX internals are separately audited. |

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
- Lottery redistribution: candidate eligibility, current selection mode,
  reward cap/carry-forward and payout accounting. #902/ADR 0009 record
  Path C work (#955/#980); do not use the historical tilted-selection model
  as a substitute for reading the freeze. Reassess H4's historical accepted
  disposition (#715) against the actual mechanism.
- Demurrage: max-quantile ring age + centroid-floored cluster factor
  (decoy-resistant, #596/#582). `consensus_fee_floor` uses
  `ring_elapsed_quantile` for age, while the factor path still uses
  `ring_centroid_floored_factor`; distinguish those from proposed CT economics.
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
transport-security.md` and the phase-1 onion-gossip audit). Record dependency
advisories and justified ignores at the actual freeze;
closed historical dependency issues do not prove the frozen dependency graph
is clear. Current follow-up tracking includes #813 and #1254.

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
- Quorum write path: `docs/security/quorum-write-path.md`,
  `botho/src/operator_action.rs` and the application/gating path in
  `botho/src/commands/run.rs` are in scope. Cycle 8 reviewed this surface;
  include subsequent changes in the freeze assessment.

### 4.8 Bridge proof-of-reserve under confidential amounts (forward flag)

The BTH↔wBTH bridge audit is scoped separately (#830) and remains the first
call on the external-audit budget. Its Solana custody inventory must distinguish
`squads.rs` assembly primitives from the missing production lifecycle:
`SolMinter::prepare_mint`/`check_confirmation` still do not call them, and
Squads configuration plus durable order→proposal mapping remain unimplemented.
[#1267](https://github.com/botho-project/botho/issues/1267) supplies the real-program
local CPI harness; [#1268](https://github.com/botho-project/botho/issues/1268)
supplies minter/engine integration and retry persistence. These are code
prerequisites before #1086 authority rotation/drill, not operator-only setup.
Review the implemented lifecycle and execution evidence at the freeze. One item is flagged here because it
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
- `audits/` — internal cycles through cycle 8, plus dedicated bridge and
  Snap reports; see `audits/README.md` for dates/scopes. Cycle 6 captures the
  block-acceptance findings, cycle 7 verifies closures, and cycle 8 covers
  the operator write path. They are prior-work records, not reviews of all
  later commits.
- `docs/design/` — mechanism design docs (lottery redistribution,
  cluster-tilted redistribution, ring-signature tag propagation and privacy
  analysis, asymmetric fees, entropy-proof analyses).
- `consensus/quorum-sim/README.md` — FBAS safety/liveness framing and the
  verified 2-of-4 fork counterexample.
- Transport security audits (`docs/security/transport-security-audit-2024.md`,
  `onion-gossip-phase1-audit.md`).

## 6. Historical findings and accepted risks: reconcile at freeze

The following dispositions came from earlier reports. Validate applicability
and issue status against the freeze, especially after CT/economic changes;
"accepted" is not a request to exclude a material risk from the assessment.

| Item | Disposition |
|---|---|
| H4 lottery candidate-cap grindability | **Accepted** as economically inert (#715, analysis in threat model). In scope to *challenge the acceptance*, not to rediscover. |
| M2 cluster wealth = cumulative volume, not holdings | **Ratified design decision** (#605/#630, 4–11× dGini margin without decay). |
| Cycle-6 M3–M6, L1, L3 | Historical report entries; reconcile each closure/residual at freeze rather than treating this row as a current open count. |
| Lottery payout privacy (winners visible on-chain) | Known testnet watch item; phase-2 (Pedersen payout blinding) not yet scheduled. |
| "Everyone parks" demurrage drift | Historical watch item; reassess against the current circulation window and reward-cap mechanism, rather than carrying forward an old implementation claim. |
| Dependency advisories | Inventory the frozen lockfile, `deny.toml` exceptions and actual check results; see current follow-up #813/#1254. No blanket clean-dependency assertion. |
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
2. Explicit verdicts on Botho's actual cluster-tag/economic-consensus
   enforcement (§4.1/§4.3), including determinism/fork risk; separately
   identify any pending CT design reviewed and which implementation was
   tested. Do not certify an unimplemented target as deployed behavior.
3. A statement on fitness of the quorum promotion gate as the sole
   constructor of SCP quorum sets (§4.2).
4. Re-test pass after remediation of Critical/High findings.
