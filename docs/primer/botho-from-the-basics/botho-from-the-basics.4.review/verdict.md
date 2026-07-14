# Review verdict — botho-from-the-basics.4

**Verdict: ADVANCE (review side).** Total **44/44** (threshold ≥35). Review critical flags: **none**. The final READY/AUDITED determination combines this verdict with the parallel `primer-audit` sibling at revise time.

Scored on v4's own merits. The v3→v4 perimeter was verified mechanically before scoring: a `diff` of the two bodies shows exactly **six body hunks**, and a byte-compare of all ten exhibit files shows exactly **two re-authored `.mmd` sources** (fig2, fig5) — eight source hunks, matching changelog items C1–C8 one-for-one — plus the two re-rendered PNGs; fig1/fig3/fig4 (.mmd and .png) are byte-identical carries. Nothing outside the operator-directed #905/ADR-0006 scope was touched, exactly as the changelog claims. For unchanged material this review leans on `botho-from-the-basics.3.review/` (44/44, zero flags), cited per dimension in `scoring.md`; all eight hunks and both re-rendered PNGs were reviewed fresh against the re-resolved spec (post-PR-#901 `whitepaper/sections/*.tex`, 18 files, `missing: false`).

## Critical flags: none

**Duplicates formal spec section — did not fire.** The `spec_ref` tier is ACTIVE (`../../whitepaper/sections/*.tex` resolves to 18 sections via `resolve_spec_ref`, `missing: false`). The sweep concentrated on the critical surface: the new §6 minting subsection against the corrected whitepaper §4 "Minting Attribution" (`04-cryptography.tex:340–365`). The primer **teaches then points**: it reproduces no equation — the RandomX preimage equation (`04-cryptography.tex:348–351`) is never restated; the primer's version is the deliberately loose 'the block of data a miner hashes over and over ... includes the miner's own public address keys' — no normative table (`tab:hybrid` is not reproduced; Figure 2 is a teaching diagram organized by the threat, not the spec's four-row data-lifetime table, and omits the amounts row entirely), and no derivation (the spec's 33 GB/year chain-growth analysis is compressed to a single per-block trade-off number in teaching voice, consistent with the section's existing priced-trade-off pattern the prior critics scored clean). The teach-then-point blockquote was correctly repointed to the renamed subsection ("Minting Attribution"). The §11 caveat cites ADR 0006, which is not the spec and reproduces nothing from it. Unchanged body: clean sweep carried from the v3 review.

## The four focus questions (operator's scope note)

1. **§6 PoW-binding teaching.** "The work itself is the signature." is load-bearing and correct at dim-2 standard: the property being defended is unforgeable *attribution*, and the paragraph derives — rather than asserts — why the binding delivers it (swap the keys → hash changes → the proof of work that conferred the reward is destroyed → and the race would have to be redone against an already-settled chain). The wrong-instinct-first opening ("so sign it, with a post-quantum signature", then the ~5.3 KB price) is a genuine teaching beat, not a name swap. Dependency order is preserved: the re-teach uses only §2's hash one-wayness plus the pre-existing bridgeable `(Section 8)` pointer, now with an inline gloss that makes the pointer optional; §10 step 8 points backwards to §6.
2. **fig2/fig5.** Both PNGs viewed. Figure 2's diagram matches its rewritten caption exactly (PERM lane: recipient identity → 'ML-KEM-768 stealth handshake (lattice)'; minting attribution → 'PoW-preimage binding (hash-based) ... zero signature bytes'; the subgraph header's shift to 'Must hold forever → quantum-resistant' is itself a correctness fix — attribution is an integrity guarantee, not a secret). Figure 5's step-8 box matches the updated §10 prose. Placements are unchanged and still earn their dim-3/dim-7 credit: fig2 remains the role-glossed map-before-walk the v3 review kept (its comment 1), fig5 remains the announced capstone map. Render quality: theme intact, text legible, no clipping.
3. **§11 caveat.** Exactly ONE caveat, as directed — a single signposted block ("One status note before you go.") at the top of the section that sends readers to the repository and explorer, which is precisely where the gap would bite. Honest without undermining: §5 and §6 teach the ratified design unhedged (grep-verified), and the caveat frames the gap from the reader's own vantage point ("if you open today's block explorer and can read every amount, you are looking at an implementation gap, not a different design").
4. **Duplication sweep on §6 vs WP §4.** Clean — see the flag paragraph above. Zero duplication flags.

Residual-claim sweep (grep-verified): exactly two ML-DSA mentions remain, both deliberate designated-future-family framings (§6 parenthetical, §11 FIPS 204 pointer, both matching `03-preliminaries.tex:149–157`); no claim anywhere in body or exhibits that minting transactions are signed.

## Scores

1. Pedagogical scaffolding / learnability — **7/7**
2. Intuition before formalism — **6/6**
3. Worked-example / walkthrough concreteness — **5/5**
4. Technical accuracy (judgment side) — **5/5**
5. Spec cross-reference discipline — **5/5**
6. Audience calibration — **4/4**
7. Structure & navigation — **4/4**
8. Prose clarity — **4/4**
9. Rhetorical economy — **4/4**

## Top revision priorities

None required — advance-clean at the rubric ceiling on the review side. Nothing in `comments.md` rises above minor, and the single minor (comment 1: the pre-existing 'permanent secrets' phrasing in §6 now sits slightly askew of the corrected figure header and §11's "*permanent* guarantees") is explicitly out of the operator-directed #905 scope; it is a candidate for whatever future pass next opens the body, not for a v5.

## What's working (do not sand off in any future revision)

Everything on the v1–v3 lists, verified byte-identical outside the eight hunks. New to v4 and worth protecting:

- **The wrong-instinct-first beat in §6** ("Your first instinct is probably ... and that instinct is expensive") — it teaches the design by pricing the alternative the reader would have chosen, which is the strongest possible setup for 'no signature at all'.
- **"The work itself is the signature." as a derived, not asserted, analogy** — the reattribution chain does the proving; keep the derivation ahead of the slogan.
- **The single-location status caveat** — one block, at the exit door, quarantined from the teaching sections. Resist any future urge to scatter per-section hedges; §5/§6 teaching the ratified design unhedged is what keeps the primer a companion to the spec rather than to the testnet.
