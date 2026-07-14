# Changelog — botho-from-the-basics.3 → botho-from-the-basics.4

Consumes `botho-from-the-basics.3.review/` (ADVANCE, 44/44, zero critical
flags) and `botho-from-the-basics.3.audit/` (CLEAN, `audit_clean: true`,
zero critical flags). The combined verdict pre-check (primer-revise step 2)
would report AUDITED and exit without writing; **the operator explicitly
waived that early exit** because the spec changed underneath the primer:
whitepaper PR #901 (ratified by ADR 0006,
`docs/decisions/0006-pq-architecture-ratification.md`) replaced the
"ML-DSA-65 minting signatures" design with PoW-preimage attribution
binding, so v3's §6 — clean against the spec it was audited against — now
contradicts its own `spec_ref` oracle (`whitepaper/sections/*.tex`,
re-resolved for this revision; §4 "Minting Attribution" at
`04-cryptography.tex:340` and the §3 "designated future signature family"
paragraph at `03-preliminaries.tex:149–157` are the governing text). This
v4 is the operator-directed spec-consistency restoration scoped by GitHub
issue #905 and its comments. Nothing outside that scope was touched.

## Operator-directed changes (issue #905 / ADR 0006)

| # | Change | Where | Maps to |
|---|---|---|---|
| C1 | §6 minting subsection re-taught: heading "Minting signatures: ML-DSA-65" → "Minting attribution: the work itself is the signature". New teaching beat per WP §4 "Minting Attribution": the RandomX preimage includes the minter's public address keys, so the winning hash is inseparable from the winner's identity; reattribution would destroy the PoW and require redoing the race against an externalized block; hashes are quantum-safe (Grover-only speedup, no Shor break), so minting needs **no signature at all** — saving ~5.3 KB/block vs the lattice-signature route. ML-DSA-65 retained ONLY as the designated future signature family if a PQ authorization path is ever introduced (WP §3). | §6 | #905 item 1; ADR 0006 decision 3; `04-cryptography.tex:340–365`; `03-preliminaries.tex:149–157` |
| C2 | §6 teach-then-point blockquote: WP §4 subsection pointer "Minting Signatures" → "Minting Attribution" (the spec renamed it). | §6 | #905 item 4; ADR 0006 consequences |
| C3 | `exhibits/fig2-hybrid-pq-envelope.mmd` re-authored: the "ML-DSA-65 signatures (~3.3 KB, but only once per block)" box → "PoW-preimage binding (hash-based) — the work itself is the signature — zero signature bytes"; PERM subgraph header "Secrecy must last forever → post-quantum (lattice)" → "Must hold forever → quantum-resistant" (minting attribution is a hash-based *integrity* guarantee, not a lattice secret); lattice gloss moved onto the ML-KEM box. Header comment notes the ADR 0006 basis. Re-rendered to PNG via mmdc 11.15.0 (anvil mermaid theme, system-Chrome puppeteer override), visually verified. | fig2 (.mmd + .png) | #905 item 2; ADR 0006 decision 3 |
| C4 | Body caption for Figure 2 rewritten to match the re-authored figure (recipient identity → ML-KEM-768 lattice handshake; minting attribution → hash-based PoW preimage, no signature). Kept as a one-sentence re-teach with no new numbers, per the v3 review's "captions as one-sentence re-teaches" do-not-sand-off note. | §6 caption | #905 items 2+4 |
| C5 | `exhibits/fig5-capstone-payment-timeline.mmd` step 8 box: "RandomX winner assembles the block + ML-DSA-65 minting transaction" → "+ minting transaction (identity bound by the PoW preimage — no signature)". Re-rendered to PNG via mmdc, visually verified. | fig5 (.mmd + .png) | #905 item 3 |
| C6 | §10 capstone step 8 prose: "(signed with the miner's ML-DSA-65 key, …)" → "(no signature needed: the miner's identity is bound by the PoW preimage itself, per Section 6 — …)". Rest of the parenthetical (new cluster, emission slice) unchanged. | §10 step 8 | #905 item 3 |
| C7 | ML-DSA sweep of the remaining body: §11 literature pointer now "NIST FIPS 203 for ML-KEM (and FIPS 204 for ML-DSA, the designated future signature family)"; §11 "What you now know" recap now credits ML-KEM-768 for recipient privacy and the hash-based PoW binding for minting attribution ("permanent secrets" → "permanent guarantees" since attribution is public-but-unforgeable). No other ML-DSA mentions remain (grep-verified: the only two survivors are the two deliberate designated-future-family framings in §6 and §11). | §11 (two spots) | #905 item 4 |
| C8 | ONE implementation-status caveat added at the top of §11 ("One status note before you go"): the live testnet does not yet hide amounts or carry ML-KEM ciphertexts on outputs — that is the ratified target (ADR 0006; tracked as the CT/ML-KEM implementation epic, #904) — and the opt-in "quantum-private" transaction class from early drafts was retired outright (ADR 0006 decision 4). Placed in §11 because that is where the primer sends readers out to the repository and explorer (per the #905 comment: readers who inspect the explorer and see amounts should not conclude the primer is wrong). Single caveat, no scattered hedges: §5 and §6 teach the ratified design unhedged. | §11 top | #905 item 5 + first #905 comment |

## Review notes (botho-from-the-basics.3.review/)

| # | Source | Disposition |
|---|---|---|
| R1 | Verdict: "None required — advance-clean at the rubric ceiling." | No rubric-driven changes made; every edit above is operator-directed spec-consistency work. |
| R2 | Comments 1–5 (all nit/preserve) | **Preserved.** Comment 1's fig2-caption preview convention is kept (the caption still glosses each scheme with its role); comments 2–5 untouched (fig3 preview, fig1 caption compression, fig4 caption density, implicit-figure convention — all five image references remain image-only paragraphs with blank lines both sides, verified). |
| R3 | "What's working" do-not-sand-off list (v1+v2 lists + the five figures, fig5-as-map, fig3 wall metaphor, captions-as-re-teaches) | **Preserved.** Problems-first spine, recap-before-use, §9.6 graveyard, honest caveats, teach-then-point blockquotes, eleven-step capstone, figure placements — all byte-identical outside the eight scoped changes. Fig2/fig5 keep their placement, style, theme, and caption conventions; only the ML-DSA content changed. |

## Audit notes (botho-from-the-basics.3.audit/)

| # | Source | Disposition |
|---|---|---|
| A1 | Priority 1, item O1: "WP §4 'Minting Signatures' vs live-code divergence on ML-DSA-65's role" (carried operator item) | **Resolved by this revision's cause.** The operator escalation (#899) concluded in ADR 0006 + whitepaper PR #901; the spec now teaches PoW-preimage binding and this v4 realigns the primer to it (C1–C7). |
| A2 | Priority 1, items O2 (WP-internal 5s/3s + CLSAG byte-figure inconsistencies) and N1 (2-in-2-out size tension behind "~5 KB") | **No primer change** — operator-side spec editorial items, carried forward unchanged, as in v3. |
| A3 | Priority 2, N2 (optional "never how much" hedge for factor >1 spends) | **Declined — out of the operator-directed scope** (#905 item 6: "NOTHING else"), same disposition and reason as v3's A2; the audit rates the current text lossy-but-true. |

## Self-discipline re-run (drafter step-5 disciplines on the result)

- **Dependency-order walk**: the new §6 minting teaching uses only
  already-taught black boxes — hashes and their one-wayness (§2 toolbox)
  plus the same "(Section 8)" forward-pointer for mining that the v3 text
  already carried (v3 critics accepted it as bridgeable); "the block of
  data a miner hashes over and over in the mining race" is
  self-explanatory at the §2 level. §10 step 8 points back to §6. The §11
  caveat uses only §5/§6 vocabulary, both long since taught. Figure 2
  still previews scheme names one subsection early — the same
  role-glossed map-before-walk the v3 review explicitly kept (comment 1).
- **Cross-reference-not-duplicate check**: the new §6 text teaches the
  preimage-binding intuition and points to WP §4 "Minting Attribution"
  for the formal statement; it reproduces no equation (the RandomX
  preimage equation stays in the spec), no table, no derivation. The ~5.3
  KB/block figure is a single priced-trade-off number in teaching voice,
  consistent with the section's existing "1,088 bytes / ~700 B / ~50×"
  pattern the critics scored clean.
- **Technical-accuracy check** (against the re-resolved spec_ref):
  preimage contents (nonce ‖ prev_hash ‖ view key ‖ spend key →
  "includes the miner's own public address keys" — lossy-but-true),
  reattribution consequence (hash changes, PoW destroyed, chain already
  externalized), Grover-only quantum exposure, no-signature design,
  ~5.3 KB/block saving (3,309 B signature + 1,952 B public key =
  5,261 B ≈ 5.3 KB, `04-cryptography.tex:363–365`), ML-DSA-65 as
  designated-future-family (`03-preliminaries.tex:153–157`), and the
  caveat's three facts (public amounts today, no ML-KEM ciphertexts
  today, QP tier retired) each verified against ADR 0006 §Context and
  decisions 1/2/4. The revision introduces no fresh instance of the
  failure mode it fixes: zero remaining claims of a minting signature.

Word count: 8,616 → 8,936 (+320: the §6 re-teach is longer than the
paragraph it replaces — the PoW-binding story is a genuine teaching beat,
not a name swap — plus the §11 caveat; everything else is near-neutral).

PDF note: this version intentionally ships no PDF; `primer-figures` runs
after the v4 critic pair (with the #690-thread render hardening per the
#905 comment).
