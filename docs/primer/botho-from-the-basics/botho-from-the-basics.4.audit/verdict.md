# Audit verdict — botho-from-the-basics.4

**Audit: CLEAN.** Zero unresolved audit critical flags.

- **Critical flags: none.**
  - **"Contradicts cited spec"** (flag 2, ACTIVE — `spec_ref: ../../whitepaper/sections/*.tex` resolved to all 18 whitepaper LaTeX sections, **post-PR-#901 / ADR-0006 content**): **0 contradictions found.** Every claim in the v4 delta (6 body hunks + the re-authored fig2 and fig5, PNGs viewed) was cross-checked against the corrected spec; the v3→v4 revision exists precisely to remove the contradictions PR #901 created, and it removed all of them. The ML-DSA sweep confirms exactly two surviving mentions, both the designated-future-family framing that `03-preliminaries.tex:149–157` itself uses. Zero remaining claims — in body, captions, or rendered diagrams — that minting transactions are signed.
  - **"Subtly-wrong intuition"** (flag 3, always eligible): **0 false simplifications found.** The new §6 compressions all resolve lossy-but-true: "includes the miner's own public address keys" (spec preimage `nonce ‖ prev_hash ‖ pk_view ‖ pk_spend`, code-confirmed in `botho/src/pow.rs::pow_preimage`); the reattribution story (hash changes → PoW destroyed → redo the race against an externalized chain) is the spec's own argument minus the cluster-origin corollary; "~5.3 KB saved every block" is the spec's 5,261 B (3,309 B sig + 1,952 B pk) restated per-block — arithmetically identical to the spec's ≈33 GB/yr at the 5-second reference pace; Grover-only quantum exposure is stated correctly and matches `04-cryptography.tex:359–362`.

`audit_clean: true`

## Scope and oracle-shift handling

The spec changed underneath the clean-audited v3 (PR #901, ratified by ADR 0006), so this audit did NOT blindly carry the v3 verdict for unchanged prose. Three-part scope instead:

1. **Delta walk (D1–D16 in `findings.md`)**: all six body hunks (changelog C1/C2/C4/C6/C7/C8 — diff re-run and perimeter confirmed by this auditor; changelog's own hunk accounting maps C7's two §11 edits into one hunk) and both re-rendered figures, every claim verified against `04-cryptography.tex:340–365` ("Minting Attribution"), `03-preliminaries.tex:149–157`, tab:hybrid, ADR 0006 decisions 1–4, and — where the spec is thin — the code itself (`botho/src/block.rs` `MintingTx`: no signature field; `botho/src/pow.rs::pow_preimage`: exact preimage layout; `transaction/clsag/src/lib.rs` `TxOutput`: cleartext `amount: u64`, no KEM ciphertext field).
2. **Carried-claim spot re-verification (S1–S10)**: ten v3-verified load-bearing claims re-checked against the post-#901 sections — all still hold (the #901 edit surface did not touch them).
3. **Exhibit perimeter**: `diff -rq` confirms fig1/fig3/fig4 are byte-identical carries; only fig2/fig5 changed, and both PNGs faithfully render their re-authored sources with no stale ML-DSA content.

## Highlights

- The §11 status caveat (C8) is exactly right on all three facts: amounts ARE public on today's chain (`TxOutput.amount: u64`), outputs carry NO ML-KEM ciphertexts today, and the opt-in quantum-private class was retired outright (ADR 0006 decision 4) — the caveat's "dropped, not deferred" vs "implementation gap" distinction mirrors the ADR precisely.
- The fig2 PERM-group reframing ("Must hold forever → quantum-resistant", lattice gloss moved onto the ML-KEM box) is an accuracy improvement, not just a compliance edit — the minting leg is hash-based integrity, not a lattice secret, and the caption/recap "secrets"→"guarantees" shift tracks that correctly.

## Top audit priorities for the reviser / operator

No blocking items. In descending order:

1. **O3 (new, operator-side)**: PR #901 left a WP-internal tension — `06-consensus.tex:177` still writes the PoW check as `Hash(block_header ‖ nonce) < target` while the new §4 equation (and the code) hashes `nonce ‖ prev_hash ‖ pk_view ‖ pk_spend`. The primer is safe under either formulation (reader's carried-away belief correct: the minter's keys are in the hashed data), but the spec should reconcile §6 with §4.
2. **Carried operator items**: O2 (5s/3s block-time and CLSAG byte-figure editorial inconsistencies, untouched by #901); N1 (the 2-in-2-out "~5 KB" size tension). O1 (the original ML-DSA divergence) is now RESOLVED by ADR 0006 + #901 + this v4.
3. **N2 (minor, optional, carried)**: the capstone's absolute "never how much" — declined in-scope per #905 item 6; remains lossy-but-true for the factor-1 capstone instance.
