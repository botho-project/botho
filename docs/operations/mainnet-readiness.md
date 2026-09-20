# Mainnet readiness: requirements and evidence

**Status: not ready. Evidence review: 2026-09-20.** The historical review
baseline remains `65687275d394eec0265dc7f2a59566d47ca2bd87` (2026-09-19).
This refresh checks merged source at
[`4ef0af75603ec6c6fb58ec06031a8c38823bccad`](https://github.com/botho-project/botho/tree/4ef0af75603ec6c6fb58ec06031a8c38823bccad)
and public issue/PR evidence. Open branches are identified separately below;
they are not part of that merged baseline. No new live probes, deployments,
audit engagements or launch approvals are claimed. Issue closure and CI results
do not establish that a later launch candidate passes every gate.

This is the current launch checklist linked from [PLAN](../../PLAN.md) and
[the external audit scope](../security/external-audit-scope.md). Existing
acceptance criteria and ADRs govern the gates; this document neither ratifies
proposed decisions nor waives a requirement. **Incomplete** means required
work remains; **unproven** means the required execution/sign-off evidence is
missing here; **implemented** describes code, not production verification.

**Current work scope (2026-09-20): audit preparation, without commissioning
an external audit.** Complete implementation, independent internal review,
adversarial testing, wallet interoperability and launch/recovery rehearsals
before investing in an engagement. Resolve known substantive findings and
retain reproducible evidence against an exact release candidate. Firm
selection, engagement and spending are deferred; the external assessment
remains a later launch gate. Internal testing cannot guarantee that an
external reviewer will find no further defects.

## Core protocol release

| Gate and requirement | Current evidence/status | Evidence needed to close; owner/action |
|---|---|---|
| CT economics specification ([#902](https://github.com/botho-project/botho/issues/902)) | **Incomplete.** Merged [#1311](https://github.com/botho-project/botho/pull/1311) closes the specification checkpoint [#1301](https://github.com/botho-project/botho/issues/1301), publishing the [proposed, inactive CT1 contract](../design/ct-transaction-contract.md). D2/EpochOrigin is ratified; [ADR 0009](../decisions/0009-confidential-amounts-economics.md) remains Proposed. D1 fee buckets, D3 circulation policy and base-fee decisions are not complete acceptance. | Maintainer/protocol owners: resolve the remaining policy choices and independent construction review. [#1306](https://github.com/botho-project/botho/issues/1306) measures origin/decoy economics; [#1307](https://github.com/botho-project/botho/issues/1307) covers combined proofs. A merged candidate document does not activate CT or close #902/#904. |
| Confidential transaction implementation ([#904](https://github.com/botho-project/botho/issues/904)) | **Incomplete.** [Active CLSAG types](../../transaction/clsag/src/lib.rs) publish `TxOutput.amount` and `ClsagRingInput.pseudo_output_amount`, with zero-blinded amount commitments. The [#902](https://github.com/botho-project/botho/issues/902) escalation documents amount-matching elimination of decoys; sender anonymity cannot be inferred from ring size alone. RingCT helpers existing elsewhere are not integration evidence. | Protocol/wallet engineers: after the governing specification, implement hidden amounts, balance/range/demurrage proofs and value-free economics across validation, ledger, RPC and wallet stacks; test conservation, invalid proofs, overflow, double spending, fee leakage and cross-wallet compatibility. Record the coordinated testnet reset and confirmed private spends; submit the resulting implementation for security review. |
| Universal ML-KEM recipient outputs ([#904](https://github.com/botho-project/botho/issues/904) completed half) | **Implemented; historical live rollout recorded on [#904](https://github.com/botho-project/botho/issues/904).** [CLSAG outputs](../../transaction/clsag/src/lib.rs) implement hybrid outputs; [consensus validation](../../botho/src/consensus/validation.rs) enforces ciphertext presence/length for protocol major ≥6 with the `pq` feature. [Source protocol](../../botho/src/network/discovery.rs) is 6.0.0. This review has not queried the live fleet. | Release owner: pin build features and verify enforcement plus send/scan/spend on every supported wallet for the launch artifact. Preserve hybrid privacy through CT. [#904](https://github.com/botho-project/botho/issues/904) also records a classical-only encrypted-memo caveat; review its confidentiality claims and disposition explicitly. |
| Lottery payout spendability ([#1286](https://github.com/botho-project/botho/issues/1286)) | **Confirmed blocker; incomplete.** Public issue evidence records hybrid payout index mismatch and source/payout key-image reuse. Merged [#1302](https://github.com/botho-project/botho/pull/1302) at [`3c5adc12`](https://github.com/botho-project/botho/commit/3c5adc121e0d8699f765303026d1adca369332fb) supplies inactive V2 primitives and native/WASM vectors only. Merged [#1336](https://github.com/botho-project/botho/pull/1336) adds a read-only offline inventory of nominal legacy claims; it does not repair or establish spendable balances. | Protocol/storage/wallet owners: complete reviewed versioned consensus, storage/snapshot and RPC/client integration; prove independent accepted spends of sources and repeated/nested awards with replay rejection. Record explicit legacy-claim and genesis/migration disposition. No silent wire change, balance omission or reset; inactive primitives do not close #1286. |
| External security assessment ([#616](https://github.com/botho-project/botho/issues/616)) | **Deferred engagement; assessment incomplete.** [Scope draft](../security/external-audit-scope.md), [threat model](../security/threat-model.md) and [internal reports](../../audits/README.md) exist. Earlier scope conditions clearing is not external completion. | Finish engineering and reproducible internal evidence first. Owner instruction defers firm selection, commissioning and spending; no provider/budget request is pending from this checklist. Before launch, independent assessment of the frozen final scope, findings/dispositions and required remediation/retest remain mandatory. |
| Release and operational acceptance | **Historical delivery exists; launch candidate unproven.** [#613](https://github.com/botho-project/botho/issues/613) (regional seeds), [#605](https://github.com/botho-project/botho/issues/605) (economic disposition), [#614](https://github.com/botho-project/botho/issues/614) (CLI port), [#615](https://github.com/botho-project/botho/issues/615)/[#640](https://github.com/botho-project/botho/issues/640) (release/reproducibility) are closed. [Release verification](reproducible-builds.md), [disaster recovery](disaster-recovery.md) and [reset runbook](../../infra/seed/TESTNET_RESET.md) exist. | Release/operator owners: identify the actual genesis/configuration and frozen artifact, attach independent reproducibility results and wallet/consensus/sync validation; verify seed diversity/discovery, monitoring, recovery and rollback against that deployment. Carry forward documented residual decisions rather than silently reopening or waiving historical work. |

The live age path already uses value-free `ring_elapsed_quantile`; the factor
path still uses `ring_centroid_floored_factor` in
[ledger/store.rs](../../botho/src/ledger/store.rs). The merged candidate now
records this distinction. EpochOrigin propagation and hidden amounts are not
deployed. Open [#1308](https://github.com/botho-project/botho/issues/1308)
tracks the candidate hybrid opening codec and remains blocked with no implementation delivered,
[#1309](https://github.com/botho-project/botho/issues/1309) covers consumers and
versioned ledger migration, and
[#1310](https://github.com/botho-project/botho/issues/1310) covers cross-platform
rehearsal and activation evidence. These are unfinished implementation gates.

## Merged engineering evidence at this review

The following changes are in the pinned merged baseline. Their linked PRs retain
validation scope and limitations; none establishes launch readiness alone.

- [#1269](https://github.com/botho-project/botho/pull/1269): HTTP/TLS dependency
  repairs and the pinned Bulletproofs cleanup migration with compatibility
  evidence. Remaining dependency exceptions still require review.
- [#1291](https://github.com/botho-project/botho/pull/1291): pinned Rust build
  compatibility and the heed test-helper repair; this defers the Alloy 2 migration.
- [#1276](https://github.com/botho-project/botho/pull/1276) and
  [#1287](https://github.com/botho-project/botho/pull/1287): actual integration and
  consensus test execution, including production-wallet hybrid test recovery.
  Ordinary wallet coverage does not establish lottery payout recovery.
- [#1298](https://github.com/botho-project/botho/pull/1298),
  [#1303](https://github.com/botho-project/botho/pull/1303) and
  [#1305](https://github.com/botho-project/botho/pull/1305): reproducible Solana npm
  installation, removal of native bigint conversion from the dependency path,
  and a compiler gate with a narrowly pinned declaration repair.
- [#1275](https://github.com/botho-project/botho/pull/1275): mobile SDK dependency
  alignment and drift checks. This is not device send/scan/spend acceptance.

- [#1288](https://github.com/botho-project/botho/pull/1288) and
  [#1335](https://github.com/botho-project/botho/pull/1335): actual inactive
  single-input and combined arithmetic proofs. The combined pilot has 14 passing
  tests and measurements for 1/16 inputs and outputs, including shared commitment
  variables, exact aggregate bucket bounds and conservation. Actual CLSAG
  ownership linkage, authenticated state, wallet witness generation and full
  transaction admission remain outstanding under #1307. Its arithmetic byte
  subtotal excludes the full transaction; workstation timings do not establish
  cross-platform block-interval liveness.
- [#1315](https://github.com/botho-project/botho/pull/1315): actual selector and
  bounded accounting evidence, with full failed-selection denominators. These
  finite experiments are not calibrated steady-state economics or policy approval.
- [#1316](https://github.com/botho-project/botho/pull/1316) and
  [#1330](https://github.com/botho-project/botho/pull/1330): reviewed network
  dependency updates and real local WebRTC connection/teardown tests. Public
  STUN/TURN and browser interoperability are not established by loopback tests.
- [#1328](https://github.com/botho-project/botho/pull/1328): restored desktop
  release builds. Declared platform minimums still need reconciliation and actual
  minimum-runtime acceptance under [#1327](https://github.com/botho-project/botho/issues/1327).
- [#1333](https://github.com/botho-project/botho/pull/1333): optimized complete
  transaction-core CI execution, preserving debug assertions and overflow checks.
  Fuzz build checks elsewhere establish compilation, not completed fuzz campaigns.

### Fee modeling direction and pending reviewed work

The owner requests the smallest feasible payment supported by steady-state
network-wide electricity and compute costs, with floating BTH value. Modeling
must expose throughput/utilization, resource costs, security expenditure, fee
recipient shares/burn and exchange-value sensitivity. A fixed minimum BTH
payment or affordability percentage has not been supplied; the model should
produce a tradeoff frontier rather than invent one. The proposed 0.5 BTH base
for a typical payment with change remains unapproved. Cost recovery alone does
not establish privacy, anti-spam adequacy or incentive equilibrium; these remain
separate acceptance questions on #1306 and the CT policy issues.

The following is an open-PR snapshot at this review, not merged evidence:

| PR / exact head | Outstanding scope |
|---|---|
| [#1317](https://github.com/botho-project/botho/pull/1317) / `41d488987ee7186700958444ad639e8423f61c3b` | Archived Squads recovery; rebased onto the merged engine, fresh CI pending. |
| [#1322](https://github.com/botho-project/botho/pull/1322) / `6a7692dccbc41dc6877ebe24d6db7fcf1bbbca22` | Durable retry/fee budgets and marker rent; stacked on the prior recovery head and requires reconciliation. |
| [#1320](https://github.com/botho-project/botho/pull/1320) / `4e24f2f473c710b12e32d0a66a7b336d5111ffdb` | Fixed-capital controls; fresh checks after inheriting the shallow-checkout reporting correction. |
| [#1331](https://github.com/botho-project/botho/pull/1331) / `f443bfa80a9809152282df01bd1aa6d2f332433b` | Funded-payment/public-ticket model; needs parent reconciliation and passing CI. Local eight-test evidence is not final hosted acceptance. |
| [#1338](https://github.com/botho-project/botho/pull/1338) / `c54bc534e9b598bd65be8c177d8d3e9237201176` | Reviewed fallback target-key uniqueness fix, published with owner approval; CI pending. It does not resolve lottery ownership or migration. |

Historical experiment artifacts retain their recorded source hashes and
assumptions. Rebasing or changing a selector does not silently recalibrate those
results; new claims require new observations and explicit provenance.

## Bridge activation: no mainnet value until its gates close

These requirements apply when activating the bridge, independently of whether
optional DeFi demonstrations or hosting products launch with the core network.

| Gate and requirement | Current evidence/status | Evidence needed to close; owner/action |
|---|---|---|
| Internal and external bridge audits ([#830](https://github.com/botho-project/botho/issues/830)) | **Internal completed for its dated scope; external incomplete.** [July bridge report](../../audits/2026-07-13-bridge.md) records 0 Critical/High and the [threat model](../security/bridge-threat-model.md) is published. The reopening comment explicitly preserves the external audit and sign-off gate. Later transports/custody changes need coverage. | Security/release owners: prepare contract **and protocol** scope and internal evidence now. External commissioning and spending are deferred by owner instruction. Later independent assessment, report/retest and explicit sign-off remain required. [#1246](https://github.com/botho-project/botho/issues/1246) is a provider inquiry, not an engagement or completed audit. [#830](https://github.com/botho-project/botho/issues/830) requires no mainnet bridge value until closure. |
| Threshold custody configuration ([#1019](https://github.com/botho-project/botho/issues/1019)) | **Incomplete.** Tasks require distinct Solana mint/admin/pauser Squads authorities, program upgrade-authority revocation, Ethereum single-Safe versus separated-role decision, nonzero slippage bounds and external sign-off. Testnet deployment is not the mainnet ceremony. | Maintainer: document role separation and thresholds. Operators: review/fund keys and vaults, execute/record ceremony and deployed configuration; revoke upgrade authority only at the approved final step. Verify limits, rotation/recovery and deployed authorities against [custody ADR](../decisions/0002-bridge-custody-scp-validator-federation.md) and [elected federation ADR](../decisions/0010-elected-bridge-multisig.md). |
| Both-chain real threshold round trips ([#868](https://github.com/botho-project/botho/issues/868)) | **Unproven.** July drill reached 2-of-3 release authorization but failed safely on an invalid destination; it did not complete a value-moving loop. It detected bootstrap supply drift. [#1050](https://github.com/botho-project/botho/issues/1050) is now closed; [federation.rs](../../bridge/service/src/federation.rs) includes signed order replication and separate-store tests. That former code blocker is not live execution evidence. | Operators: resolve live BTH funding/block-production prerequisites ([#1051](https://github.com/botho-project/botho/issues/1051)) and Solana setup ([#1052](https://github.com/botho-project/botho/issues/1052)/[#1086](https://github.com/botho-project/botho/issues/1086)), then run [the federation driver](../../scripts/bridge-testnet-federation.sh) and [runbook](../bridge/testnet-e2e-runbook.md). Publish deposit→mint→burn→release tx links on **both** chains with independent federation stores, threshold evidence, factor-1 amounts, exactly-once assertions and live reserve/combined-supply accounting. Explicitly reconcile bootstrap supply; an allowed tolerance is not proof of full backing. |
| Squads mint execution and recovery ([#1267](https://github.com/botho-project/botho/issues/1267)/[#1268](https://github.com/botho-project/botho/issues/1268)) | **Local engine implementation merged; custody readiness incomplete.** Merged [#1279](https://github.com/botho-project/botho/pull/1279) reports real pinned-program local CPI and 19 receipts. Merged [#1300](https://github.com/botho-project/botho/pull/1300) reports durable engine/member-store execution and 13 finalized receipt logs. Its confirmed-deposit rows are fixtures, not a live BTH round trip. | Complete and validate archived/late-member recovery [#1295](https://github.com/botho-project/botho/issues/1295), bounded retries and marker-rent funding [#1299](https://github.com/botho-project/botho/issues/1299). Retain backing on ambiguous/unsupported recovery. Then separately evidence authority configuration and both-chain operator drills; local CPI success does not authorize rotation or bridge value. |
| Proof of reserves after CT | **Design handoff required.** [Post-CT analytics](../design/post-ct-analytics.md) describes the disclosure problem: public ledger amounts will no longer provide reserve balances. | Protocol/bridge authors and auditor: specify and validate view-key/attested-opening disclosure, freshness, rotation and revocation with the final CT design; demonstrate that stale or spoofed disclosures cannot establish solvency. Carry this into both audit scopes. |

## Separate service and venue launches

| Workstream | Current evidence/status | Acceptance/owner |
|---|---|---|
| Multi-venue testnet DeFi ([#865](https://github.com/botho-project/botho/issues/865)) | **Incomplete.** Issue records completed Uniswap/Orca and NTT work, with [#868](https://github.com/botho-project/botho/issues/868) and [#877](https://github.com/botho-project/botho/issues/877) outstanding. | Bridge/operators: satisfy [#865](https://github.com/botho-project/botho/issues/865)'s real-federation trading and return-flow evidence across all three venues. This is its own demo acceptance, not a replacement for core or bridge safety gates. |
| Hosted node service ([#721](https://github.com/botho-project/botho/issues/721)/[#722](https://github.com/botho-project/botho/issues/722)) | **Open service gates.** Test-mode rollout and live/legal review are tracked separately. | Service operator: complete their acceptance and business decisions before activating the corresponding service. Do not infer authorization or readiness from a core release. |
| Snap distribution ([#1089](https://github.com/botho-project/botho/issues/1089)) | **Open product acceptance.** [Internal key-handling audit](../../audits/2026-07-20-snap-keyhandling.md) is dated/scoped evidence, not distribution or live-send completion. | Wallet owner: record parity, live-send and publishing acceptance on [#1089](https://github.com/botho-project/botho/issues/1089); supported-wallet claims must match tested releases. |

## Recording a launch decision

For each applicable gate, record the responsible owner, immutable source/tag,
build features, configuration, test/audit artifacts, execution date and explicit
residual disposition in its linked issue. Recheck issue status at handoff.
Historical reports remain unchanged; update this inventory when new evidence
supersedes it. A final go decision needs those artifacts and the required
operator/auditor sign-offs, not simply a count of closed issues.
