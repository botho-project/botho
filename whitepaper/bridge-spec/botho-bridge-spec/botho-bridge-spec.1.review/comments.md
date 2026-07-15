# Comments — botho-bridge-spec.1 (spec-review)

Severity: blocker / major / minor / nit. Scope: preserve / expand / reduce.

## major — Referenced figures not rendered (step 4c; caps dims 6 & 7)

- **§Custody (fig3, line 174)**, **§Peg (fig1, line 220)**, **§Import (fig2, line 384)**: the body references `\includegraphics{exhibits/fig3-federation-custody.png}`, `exhibits/fig1-wrap-mint-flow.png`, and `exhibits/fig2-unwrap-import-flow.png`, but no `exhibits/` directory exists on this version. Rationale: "Referenced figure not rendered — spec-figures has not run on this version. The reader sees a broken image reference, not the diagram the spec depends on." Suggested fix: **Run spec-figures botho-bridge-spec**. Caps dim 6 AND dim 7 at 2 of weight. Scope: expand.

## major — `\addlinespace` undefined in whitepaper preamble (not drop-in `\input`-able)

- **§Implementation Status (lines 467, 473, 479, 485, 491)**: the register table uses `\addlinespace` 5×, but the whitepaper `preamble.tex` does not load `booktabs` (it redefines `\toprule`/`\midrule`/`\bottomrule` as `\hline` variants) and defines no `\addlinespace`. The section will error on `\input` into the whitepaper. Suggested fix: define `\addlinespace` in the preamble or replace it with a preamble-available vertical spacer (e.g. `\noalign{\smallskip}` or a `[0.5em]` row break). Scope: preserve (content), fix macro.

## major — Dangling caption reference "register row IMP-1"

- **§Import, Figure 2 caption (line 389)**: "the current implementation releases a factor-1 output (register row IMP-1)." The `## Implementation status` register (lines 456–499) has no row IDs — rows are keyed by Component name only, so `IMP-1` resolves to nothing. Suggested fix: add `IMP-1`-style IDs to the register rows, or change the caption to name the row by its Component ("the Import-cluster-tagging row"). Scope: preserve.

## major — Base-layer target-state privacy claim lacks a register row (step 5b)

- **§Privacy at the Boundary (line 256)**: "amounts are (on the confidential-amounts roadmap) hidden in Pedersen commitments." Reads as target-state for a property the live chain does not yet exhibit (live amounts are public), with no `## Implementation status` row. Rationale: "Normative claim reads as target-state but is not recorded in the ## Implementation status register — a reader cannot tell an intentional live/target gap from an undocumented drift." Borderline — the property is base-layer, not a bridge component, so arguably outside the register's bridge scope. Suggested fix: restate as explicitly roadmap-qualified in place (make the future tense unambiguous) OR add a register row. Accumulates into dim 1; not a flag. Scope: preserve.

## minor — RFC-2119 keyword discipline absent (dim 3)

- **Throughout** (e.g. §Custody line 69 "must therefore be authorized"; §Privacy line 265 "Wallet UX must warn"; §Peg line 185 "The peg must hold"): normative obligations are lowercase prose with no RFC-2119 conformance-keywords clause. For an audit-grade spec an implementer benefits from mechanically extractable `MUST`/`SHALL`/`MAY`. Suggested fix: add a conformance clause and lift genuine obligations to uppercase. Scope: preserve.

## nit — `import_epoch_blocks` / `import_factor_floor` declared twice

- **§Import (lines 330–331 inline suffixes; lines 335–336 standalone comments)**: each of the two import constants carries an inline table-row `% anvil-const:` suffix AND a duplicate standalone comment immediately below the table. The gate treats these as benign matching re-declarations (no violation). Harmless, but one marker per constant is the convention; consider dropping the standalone pair. Scope: reduce.

## preserve — Load-bearing normative statements (do not weaken)

- **§Peg (lines 205–209)**: "Only factor-1 coins are wrappable. The reserve holds only zero-demurrage coins, so \eqref{eq:bridge:peg} holds over time by construction." Keep verbatim.
- **§Implementation Status (lines 501–509)**: the divergence note naming `bth_scan.rs` empty-cluster-tags as the live behavior vs the ADR-0007 target. This is the drift-guard the class exists for. Preserve.
- **§Threshold authorization (lines 84–89)**: `t \geq t_{SCP}` with "it must never be easier to move the reserve than to move consensus." Preserve.
