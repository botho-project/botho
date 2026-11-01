# Internal Security Audit Process

This directory contains internal security audit reports. Internal reviews are
prior-work evidence, not external audit sign-off. External engagement remains
open in [#616](https://github.com/botho-project/botho/issues/616); bridge
activation separately requires [#830](https://github.com/botho-project/botho/issues/830).
See the [mainnet readiness checklist](../docs/operations/mainnet-readiness.md)
for current gates, evidence requirements and owners.

## Philosophy

Security auditing is iterative. Each internal audit:
1. Reviews code against the checklist in `../AUDIT.md`
2. Documents findings with severity ratings
3. Tracks fixes applied during the audit
4. Identifies areas needing future attention

**Goal**: Multiple consecutive clean internal audits before engaging external auditors.

## Audit Cadence

| Trigger | Scope |
|---------|-------|
| Major release | Full audit (all sections) |
| Crypto changes | Sections 1, 2, 5 |
| Consensus changes | Section 3 |
| Network changes | Section 6 |
| Quarterly | Full audit |

## Running an Audit

1. Copy `TEMPLATE.md` to `YYYY-MM-DD.md`
2. Work through `../AUDIT.md` section by section
3. Document all findings, even minor ones
4. Fix critical/high issues before completing
5. Update the summary table below
6. Commit the report

## Severity Levels

| Level | Definition | Action Required |
|-------|------------|-----------------|
| **CRITICAL** | Exploitable vulnerability, fund loss possible | Fix immediately, halt release |
| **HIGH** | Security weakness, potential exploit path | Fix before release |
| **MEDIUM** | Defense-in-depth issue, hardening needed | Fix within 30 days |
| **LOW** | Code quality, minor improvements | Track for future |
| **INFO** | Observations, no action needed | Document only |

## Audit History

| Date | Auditor | Scope | Critical | High | Medium | Low | Status |
|------|---------|-------|----------|------|--------|-----|--------|
| 2026-10-31 (elliptic) | Internal | Dedicated - elliptic ECDSA advisory in Snap tooling (#1562) | 0 | 0 | 0 | 1 (unresolved, contained) | **Open: no upstream fix; CI containment** |
| 2026-10-29 (lottery V2) | Internal | Dedicated - LotteryV2 payouts P12 (#1647) | 0 | **1** (#1676, fixing pre-x1) | 1 | 2 | Issues found |
| 2026-07-20 (snap) | Internal | Dedicated - MetaMask Snap key-handling (#1096) | **0** | **0** | 0 | 1 (fixed) | **Clean (internal)** |
| 2026-07-13 (bridge) | Internal | Dedicated - BTH↔Ethereum wBTH bridge (#830) | **0** | **0** | 3 | 3 | **Clean (internal); external audit pending #616** |
| 2026-07-12 (c8) | Internal | Delta - Operator write path + consensus/BaaS | **0** | **0** | 1 new | 3 | **Healthy** |
| 2026-07-05 (c7) | Internal | Delta - Verification (block-acceptance + BaaS + u128) | **0** | **0** | 1 new | 3 | **Healthy** |
| 2026-06-11 (c6) | Internal | Delta - Block-acceptance path + consensus economics | **5** | 6 | 8 | 4 | Issues Found (all Criticals fixed by c7) |
| 2026-01-03 (c5) | Internal | Full - Post Onion Gossip | **0** | **0** | 6 | 5 | **Clean** |
| 2026-01-03 (c4) | Internal | Full - Post Security Fixes | **0** | **0** | 6 | 5 | **Clean** |
| 2026-01-03 (c3) | Internal | Full - Post LION Deprecation | **0** | **0** | 10 | 8 | **Clean** |
| 2025-12-30 (c2) | Internal | Full | 3 | 7 | 15+ | 10+ | Issues Found |
| 2025-12-30 (c1) | Internal | Full | 1 (fixed) | 1 | 2 | 2 | Issues Found |

## Path to External Audit

**Evidence review: 2026-09-19.** The historical preparation checklist below
records earlier audit-cycle planning. It is not a current count of unresolved
findings or a substitute for checking the code and the selected freeze commit.

| Historical prerequisite | Evidence and current interpretation |
|---|---|
| Three consecutive full audits without Critical/High findings | Cycles 3–5 recorded this milestone; later delta/bridge/Snap reviews have their own scopes and dates. This does not audit subsequent changes. |
| Resolve Medium findings | The old checklist said six remained. Reconcile each finding and issue at the freeze; do not carry that number forward as current. Accepted residuals need an explicit disposition. |
| Crypto test coverage >80% | Preparation baseline tracked in closed #115 per #616. Attach a measurement for the actual audit commit; no current percentage is asserted here. |
| Operational fuzz infrastructure | [fuzz/README.md](../fuzz/README.md) and [.github/workflows/fuzz.yml](../.github/workflows/fuzz.yml) describe the harness. Record targets actually run, duration, corpus and results for the freeze; existence is not run evidence. |
| Architecture and threat model | Preparation #117/#118 closed; [threat model](../docs/security/threat-model.md) and [engagement scope](../docs/security/external-audit-scope.md) exist. Reconcile live versus target privacy and subsequent changes before handoff. |

The four “settle the audit surface” conditions on #616 were cleared in July
(#581, #532, RandomX/#441, H4/#715). Firm selection, budget, dates, point of
contact and a tagged freeze remain operator actions. The pending CT rollout
(#902/#904) changes the surface again: the engagement must explicitly identify
which implementation it covers and how later changes are reviewed. No external
completion or mainnet fitness is asserted by this index.

## Report Index

- [2026-10-31 elliptic in Snap tooling](2026-10-31-elliptic-snap-tooling.md) - GHSA-848j-6mx2-7j84 (#1562): no upstream fix. elliptic is limited to Snap build/test devDependencies, is absent from shipped bundles (positive-controlled) and is never loaded during the Snap build or tests (runtime probe). A CI guard enforces this containment and the advisory is not suppressed
- [2026-10-31 contracts/ethereum npm advisories](2026-10-31-contracts-ethereum-npm-advisories.md) - npm audit findings in `contracts/ethereum` (#1767): serialize-javascript fixed by in-range bumps, glob/minimatch fixed by scoped overrides on mocha and typechain (19 findings down to 13). The 13 left all come from zod (no 3.x fix; Hardhat 3 requires `zod ^3` and breaks when zod 4 is forced). Dev-only, and no untrusted input reaches it
- [2026-10-31 Dev-only web advisories](2026-10-31-dev-advisories-glob-minimatch-deepmerge.md) - glob/minimatch/deepmerge findings in `web/` (#1745): glob and the vulnerable minimatch lines are fixed with scoped overrides, minimatch 3.1.5 stays (no 3.x fix, and the vulnerable code is not in it), and deepmerge stays (no upstream fix). Dev-only, and no untrusted input reaches it
- [2026-10-29 LotteryV2 payouts](2026-10-29-lottery-v2-internal.md) - P12 internal review of the V2 lottery payout path (#1647): the related-key construction, determinism, the body root and RPC trust are clean. Draw seed grindable (#1676, High, pre-dates V2 and V2 made it worse), validator ordering (#1670), coinbase KEM aliasing (#1677)
- [2026-07-20 Snap key-handling](2026-07-20-snap-keyhandling.md) - Dedicated MetaMask Snap key-handling audit (#1096): SRP→Botho key derivation, claim-link bearer secrets, wasm signing under SES, `snap_manageState` persistence; 0 Critical/High, 1 Low (fixed)
- [2026-07-13 Bridge](2026-07-13-bridge.md) - Dedicated BTH↔Ethereum wBTH bridge audit (#830): custody, attestation protocol, WrappedBTH.sol, watchers/engine exactly-once, reserve peg invariant, privacy boundary; 0 Critical/High, external audit + mainnet gate remain (operator, #616)
- [2026-07-12 Cycle 8](2026-07-12-cycle8.md) - Operator-signed quorum-curation write path (P4.4), consensus deltas, BaaS status-link; cycle-7 findings closed
- [2026-07-05 Cycle 7](2026-07-05-cycle7.md) - Verification cycle: block-acceptance C1–C7 confirmed, BaaS worker, u128 widening, dep advisories
- [2026-06-11 Cycle 6](2026-06-11-cycle6.md) - Block-acceptance hardening, 5 Criticals + Highs
- [2026-01-03 Cycle 5](2026-01-03-cycle5.md) - Onion Gossip Phase 1 audit, 531 tests (+39%)
- [2026-01-03 Cycle 4](2026-01-03-cycle4.md) - Security fixes verified, clippy warnings reduced 76%
- [2026-01-03 Cycle 3](2026-01-03-cycle3.md) - LION deprecation, all Critical/High resolved
- [2025-12-30 Cycle 2](2025-12-30-cycle2.md) - Full audit, wallet/dependency issues found
- [2025-12-30 Cycle 1](2025-12-30.md) - Initial full audit, SCP ballot ordering fixed
