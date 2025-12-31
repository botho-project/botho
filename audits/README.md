# Internal Security Audit Process

This directory contains internal security audit reports. External audits will be commissioned once internal audits consistently return clean results.

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
| 2025-12-30 (c3) | Internal | Verification | **0** | **1** | 3 | 2 | **Significant Progress** |
| 2025-12-30 (c2) | Internal | Full | 3 | 7 | 15+ | 10+ | Issues Found |
| 2025-12-30 (c1) | Internal | Full | 1 (fixed) | 1 | 2 | 2 | Issues Found |

## Path to External Audit

External audit will be commissioned when:

- [ ] 3+ consecutive full audits with no Critical/High findings
- [ ] All Medium findings from previous audits resolved
- [ ] Test coverage > 80% on crypto code
- [ ] Fuzz testing infrastructure operational
- [ ] Documentation complete (architecture, threat model)

## Report Index

- [2025-12-30 Cycle 3](2025-12-30-cycle3.md) - Verification audit, all CRITICAL resolved
- [2025-12-30 Cycle 2](2025-12-30-cycle2.md) - Full audit, wallet/dependency issues found
- [2025-12-30 Cycle 1](2025-12-30.md) - Initial full audit, SCP ballot ordering fixed
