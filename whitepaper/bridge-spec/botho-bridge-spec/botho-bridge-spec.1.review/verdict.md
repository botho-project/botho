# Verdict — botho-bridge-spec.1 (spec-review)

**Reviewer scope**: prose / structure / consistency / normative correctness (by judgment). The parallel `spec-audit` owns the exhaustive code↔spec factual cross-check and the three-way `implementation_contradicts_spec` disposition — not duplicated here.

## Decision

**BLOCK.** Total **38/44** (< 39 audit-grade threshold). Review critical flags: **none**.

`advance: false` — total is one point under threshold, and both the figure-cap chain (dims 6/7 capped at 2 by the missing `exhibits/`) and the RFC-2119 / drop-in-`\input` defects are cheap to close. Combine with the audit sibling at revise time.

## Constant-consistency gate (step 3b)

`check_constant_consistency_multi({botho-bridge-spec.tex: ...})` → **found=true, 7 declarations, 0 violations, passed=true**. Five distinct constants (`wbth_decimals=12`, `bridge_threshold_floor=t_scp`, `ring_size=20`, `import_epoch_blocks=17280`, `import_factor_floor=1.5`); `import_epoch_blocks` and `import_factor_floor` are each re-declared once with identical value+unit (the inline table-row suffix plus a standalone comment beneath the table) — benign, no value-mismatch. **No Self-contradiction flag fires from the mechanical half**, and the judgment sweep found no unmarked prose-level drift.

## Critical flags

Critical flags: none.

- **Self-contradiction (flag 1)**: not raised. Gate clean; peg invariant `\eqref{eq:bridge:peg}` and the factor-1/zero-demurrage predicate are each stated once and reused by reference across §Peg / §Privacy / §Security with no incompatible restatement.
- **Undefined normative term (flag 2)**: not raised. Every term in a normative obligation is defined at or before first normative use — `t`/`t_{SCP}` ("$t_{\mathrm{SCP}}$ is the SCP safety threshold" — §Threshold authorization), `factor-1` ("a \textbf{factor-1} (background / commerce) coin pays exactly \emph{zero} demurrage" — §Peg), `c_{import}(m)` and `import_factor(m)` (eqs. in §Import), `orderId` ("a deterministic 32-byte on-chain \texttt{orderId}" — §Attestation). No `MUST`/validity-predicate term is left undefined.

## Register-completeness finding (step 5b, prose half)

The `## Implementation status` register carries six rows, and **every bridge-scoped target-state claim in the prose maps to a row**: the demurrage-settlement on-ramp ("It is \textbf{target-state}, tracked in the register" — §Peg → Demurrage-settlement row / botho#831), epoch-keyed import tagging (§Import → Import-cluster-tagging row / botho#938, reinforced by the §Implementation-Status divergence note), Solana transports and live-supply transport (two rows), and the mainnet-gate external audit ("a hard gate, tracked in the register" — §Security → External-security-audit row). This is the register doing its job well.

**One review-side finding (major, accumulates into dim 1 — NOT a flag):** the base-layer privacy claim "amounts are (on the confidential-amounts roadmap) hidden in Pedersen commitments" (§Privacy at the Boundary) reads as target-state for a property the live chain does not yet exhibit, yet has no register row. It is borderline — the property is base-layer, not a bridge component, so it is arguably outside the register's bridge scope — but a reader cannot tell an intentional roadmap gap from drift. Restate as explicitly roadmap-qualified in place, or add a register row. (Recorded in comments as major.)

## Top revision priorities

1. **Render the three figures** (highest leverage — unlocks dims 6+7 from the 2-caps). The body references `exhibits/fig1-wrap-mint-flow.png`, `exhibits/fig2-unwrap-import-flow.png`, `exhibits/fig3-federation-custody.png` but no `exhibits/` directory exists on this version. Run `spec-figures botho-bridge-spec`. This alone plausibly recovers ~+4 (dims 6, 7 to their prose merit) and clears the block.
2. **Fix the drop-in-`\input` LaTeX defects (dim 7).** `\addlinespace` (used 5× in the register table) is undefined in the whitepaper `preamble.tex` (no booktabs) — replace with a preamble-defined spacer or add the macro; and the Figure-2 caption's "register row IMP-1" points at a non-existent row ID — either add `IMP-1`-style IDs to the register or change the caption to name the row by Component. A revision-history/change-marker block would also lift dim 7.
3. **Apply RFC-2119 discipline (dim 3).** Add a conformance-keywords clause and lift genuinely normative obligations to uppercase `MUST`/`SHALL`/`MAY` (e.g. "Every privileged cross-chain action **MUST** be authorized by..."), so an implementer can mechanically extract the conformance requirements.

## What's working — do NOT weaken on revise

- The **factor-1-only wrapping** normative rule and its over-time peg-solvency argument ("Only factor-1 coins are wrappable. The reserve holds only zero-demurrage coins, so \eqref{eq:bridge:peg} holds over time by construction" — §Peg).
- The **target-state marking discipline**: the divergence note naming `bth_scan.rs` empty-cluster-tags as the live behavior against the ADR-0007 target is exactly the drift-guarding the class exists for. Keep it verbatim.
- The **threshold-floor invariant** `t \geq t_{SCP}` with its "never easier to move the reserve than to move consensus" rationale (§Threshold authorization), and the three-Safe role split (§three-Safe structure).
- The **exactly-once mint/release** invariant pair and the **fail-closed** posture (§Security).
