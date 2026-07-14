# Findings — botho-from-the-basics.4 (audit)

Spec oracle: `spec_ref: ../../whitepaper/sections/*.tex` → 18 files under
`whitepaper/sections/` (resolved, active — **post-PR-#901 content**, the
ADR-0006-ratified text; governing passages `04-cryptography.tex:340–365`
"Minting Attribution" and `03-preliminaries.tex:149–157` designated
future signature family).

**Scope**: the v4 delta from the clean-audited v3 is the operator-directed
spec-consistency restoration (issue #905 / ADR 0006). Diff re-run by this
auditor: **exactly 6 body hunks** (= changelog changes C1/C2/C4/C6/C7/C8;
C7's two §11 spots land in one hunk together with the recap) **plus 2
re-authored exhibits** (fig2, fig5 — `.mmd` + re-rendered `.png` each;
fig1/fig3/fig4 byte-identical to v3). Nothing outside the #905 scope
changed. Because the spec itself changed underneath v3 (PR #901), the v3
clean verdict does NOT carry blindly: unchanged prose was spot-re-verified
against the post-#901 oracle (rows S1–S10 below) in addition to the full
walk of the new claim surface (rows D1–D16).

Severity legend: **critical** = fires an audit flag (none); **minor** =
precision note, no false belief created; **obs** = observation /
operator-facing.

## Diff verification

| # | Check | Result |
|---|---|---|
| V1 | `diff` v3 body vs v4 body | 6 hunks exactly: fig2 caption (C4); §6 heading + minting re-teach (C1); §6 teach-then-point blockquote "Minting Signatures"→"Minting Attribution" (C2); §10 step 8 parenthetical (C6); §11 status caveat insert (C8); §11 literature pointer + recap (C7, both spots). No other body change. |
| V2 | `diff -rq` v3 exhibits vs v4 exhibits | Only fig2 and fig5 differ (.mmd + .png each); fig1/fig3/fig4 byte-identical carries. |
| V3 | Re-rendered PNGs vs `.mmd` sources | Both PNGs viewed; each renders its v4 source faithfully (fig2 PERM header "Must hold forever → quantum-resistant", minting box "PoW-preimage binding (hash-based) / the work itself is the signature / zero signature bytes", lattice gloss on the ML-KEM box; fig5 step 8 "identity bound by the PoW preimage — no signature"). No stale ML-DSA text survives in any exhibit. Theme intact, text legible, no clipping. |

## Per-claim table — Delta claims (new §6 minting subsection, C1)

| # | Claim | Kind | Verified? | Evidence / cited source |
|---|---|---|---|---|
| D1 | Minting transactions carry no signature; attribution is bound by the PoW itself | factual + spec-consistency | **verified** | `04-cryptography.tex:343–353` ("Block rewards (minting transactions) carry no signature… attribution is bound by the proof of work itself"); code: `botho/src/block.rs:177–210` `MintingTx` struct has NO signature field (height, reward, minter view/spend keys, target_key, public_key, prev_block_hash, difficulty, nonce, timestamp) |
| D2 | "The block of data a miner hashes over and over… *includes the miner's own public address keys*" | spec-consistency | **verified-with-simplification** | Spec eq: `h_PoW = RandomX(nonce ‖ prev_hash ‖ pk_view ‖ pk_spend)` (`04-cryptography.tex:348–351`); code `botho/src/pow.rs:119–135` `pow_preimage` = nonce(8B) ‖ prev_block_hash(32B) ‖ minter_view_key(32B) ‖ minter_spend_key(32B). "Includes the miner's keys" is lossy-but-true (the primer does not enumerate the preimage — correct choice; the equation stays in the spec per the cross-reference rule) |
| D3 | Reattribution requires swapping keys inside the hashed data → hash changes → PoW destroyed → must redo the mining race against a block the network has already finalized | spec-consistency | **verified** | `04-cryptography.tex:354–359` ("Reattributing a block's reward would change the minting transaction's hash… and would require redoing the proof of work against a chain whose blocks are already externalized by consensus"); ADR 0006 decision 3 verbatim. Primer omits the spec's cluster-origin/stealth-reward-output corollary — lossy-but-true subset |
| D4 | Hash binding is quantum-safe: no Shor break, only a modest generic search speedup (Grover) | factual + spec-consistency | **verified** | `04-cryptography.tex:359–362` ("subject only to Grover-type quantum speedups and remains sound against a cryptographically relevant quantum computer"); standard result (Grover gives quadratic search speedup; Shor does not apply to hash preimages) |
| D5 | A lattice signature plus its public key would add "roughly 5.3 KB to every block, forever"; the no-signature design saves "roughly 5.3 KB… every block" | spec-consistency | **verified** | `04-cryptography.tex:362–365`: ML-DSA-65 3,309 B signature + 1,952 B public key = 5,261 B ≈ 5.3 KB/block; spec states the cost as ≈33 GB/yr at reference block time — 6.3 M blocks/yr × 5,261 B ≈ 33.1 GB ✓, so the per-block and per-year statements are the same number in different units. ADR 0006 decision 3 gives the identical 5,261 B/block figure |
| D6 | ML-DSA-65 "is the designated signature family *if* a post-quantum authorization path is ever introduced in a future protocol version" | spec-consistency | **verified** | `03-preliminaries.tex:149–157` verbatim shape: "The protocol uses no post-quantum signature scheme… Should a post-quantum authorization path be introduced in a future protocol version, ML-DSA… at security level 3 (ML-DSA-65) is the designated signature family" |
| D7 | New heading "Minting attribution: the work itself is the signature" + blockquote pointer to WP §4 "Minting Attribution" (C2) | spec-consistency | **verified** | Spec subsection renamed: `\subsection{Minting Attribution}` at `04-cryptography.tex:340`; grep confirms "Minting Signatures" no longer exists anywhere in `whitepaper/sections/*.tex` |

## Per-claim table — Delta claims (figures + captions, C3/C4/C5)

| # | Claim | Kind | Verified? | Evidence / cited source |
|---|---|---|---|---|
| D8 | Fig2 caption: guarantees that must hold forever get quantum-resistant protection — recipient identity via lattice ML-KEM-768 handshake; minting attribution via hash-based PoW preimage, no signature; sender anonymity decays → classical CLSAG + migration path | spec-consistency | **verified** | tab:hybrid (`04-cryptography.tex:370–384`): Recipient identity / Permanent / ML-KEM-768; Sender identity / Ephemeral / CLSAG; **Minting attribution / Permanent / PoW hash binding / "Hash-based; quantum-resistant"**. Migration path: `11-implementation.tex` (unchanged, v2/v3-audited). Caption's "quantum-resistant" (not "lattice") for the PERM group is exactly right now that one PERM leg is hash-based — the v3 wording ("post-quantum (lattice)") would have been wrong |
| D9 | Fig2 diagram minting box: "PoW-preimage binding (hash-based) — the work itself is the signature — zero signature bytes"; PERM header "Must hold forever → quantum-resistant"; lattice gloss moved onto the ML-KEM box | spec-consistency | **verified** | PNG viewed; matches tab:hybrid row + `04-cryptography.tex:343` (no signature ⇒ zero signature bytes). ML-KEM box retains 1,088-byte ciphertext claim (`03-preliminaries.tex:145`, S1 below) |
| D10 | Fig5 step 8: "RandomX winner assembles the block + minting transaction (identity bound by the PoW preimage — no signature)" | spec-consistency | **verified** | PNG viewed; matches D1/D2; one minting tx per block unchanged (`05-transactions.tex`, v2-audited, re-confirmed present) |

## Per-claim table — Delta claims (§10 step 8, §11 sweep + caveat, C6/C7/C8)

| # | Claim | Kind | Verified? | Evidence / cited source |
|---|---|---|---|---|
| D11 | §10 step 8: "no signature needed: the miner's identity is bound by the PoW preimage itself, per Section 6" (rest of parenthetical unchanged) | spec-consistency | **verified** | D1/D2/D3; diff confirms only the signature clause changed — new-cluster founding and lottery-slice routing text intact (both v2/v3-audited, re-confirmed: `05-transactions.tex` cluster founding, `07-monetary.tex:273–276` pool routing) |
| D12 | §11 literature pointer: "NIST FIPS 203 for ML-KEM (and FIPS 204 for ML-DSA, the designated future signature family)" | spec-consistency | **verified** | FIPS 203 = ML-KEM, FIPS 204 = ML-DSA (correct standard numbers, `03-preliminaries.tex` \cite{fips203}/\cite{fips204}); "designated future signature family" = `03-preliminaries.tex:153–157` |
| D13 | §11 recap: permanent *guarantees* hardened — recipient privacy with ML-KEM-768, minting attribution with the hash-based PoW binding itself | spec-consistency | **verified** | tab:hybrid; the "secrets"→"guarantees" rewording is a genuine accuracy improvement (minting attribution is public-but-unforgeable integrity, not secrecy) |
| D14 | §11 caveat fact 1: amounts are still public on today's testnet chain | factual | **verified** | Code: `transaction/clsag/src/lib.rs:300–302` `TxOutput { pub amount: u64, … }` — cleartext picocredits; ADR 0006 Context ("live transaction path is classical CLSAG with public amounts") + decision 1 (CT is target state; live public amounts = implementation gap) |
| D15 | §11 caveat fact 2: outputs do not yet carry ML-KEM ciphertexts | factual | **verified** | `TxOutput` fields: amount, target_key, public_key (curve ephemeral), optional 66-B e_memo, cluster_tags — no 1,088-B KEM ciphertext field; ADR 0006 Context ("no ML-KEM ciphertext on outputs") + decision 2 (universal quantum stealth = ratified target) |
| D16 | §11 caveat fact 3: the opt-in "quantum-private" transaction class was retired outright — quantum-durable recipient privacy is delivered universally, never sold as a tier | factual | **verified** | ADR 0006 decision 4 ("Quantum-private paid tier: retired… delivered universally by Decision 2… explicitly not the product story"); decision 2 (no opt-in tier — anonymity-set fragmentation critique). Framing "dropped, not deferred" matches the ADR's "retired" |

## ML-DSA mention sweep

| # | Check | Result |
|---|---|---|
| M1 | Body grep | Exactly 2 ML-DSA mentions: line 445 (§6 designated-future-family parenthetical) and line 1025 (§11 FIPS 204 pointer, same framing). Both match `03-preliminaries.tex:149–157`. Zero claims anywhere that minting (or anything else live) is ML-DSA-signed |
| M2 | Exhibits grep | Zero ML-DSA mentions across all five `.mmd` sources; PNGs visually confirmed clean |

## Carried-claim spot re-verification (v3-verified, re-checked against the POST-#901 oracle)

| # | Claim (unchanged v3 text) | Verified? | Evidence (post-#901 spec) |
|---|---|---|---|
| S1 | ML-KEM-768 ciphertext 1,088 B per output, biggest single line item | **verified** | `03-preliminaries.tex:145`; `04-cryptography.tex:103,117` |
| S2 | CLSAG ~700 B/input; PQ rings ~50× (~35 KB); multi-input >100 KB; desktop/phone nodes impractical | **verified** | `04-cryptography.tex:386–392` ("Why not full post-quantum?" — unchanged by #901); phone clause = carried lossy-but-true extension |
| S3 | Quorum intersection: "any two quorums share at least one honest node" | **verified** | `06-consensus.tex:323` verbatim |
| S4 | Externalized blocks final, no reorgs ever; halt-don't-fork | **verified** | `06-consensus.tex:384` ("no reorganization can replace"), `:471`, `:132` ("preserves safety by halting") |
| S5 | Miners never receive fees; no MEV motive | **verified** | `07-monetary.tex:407` ("minter_income = R(h) (no fees)"), `:304` (Anti-MEV) |
| S6 | Fees: 20% burned / 80% to lottery pool; 4 UTXOs per block | **verified** | `07-monetary.tex:273–276` |
| S7 | Lottery seed from the already-finalized previous block's hash (grind-proof) | **verified** | `07-monetary.tex:328,344` ("seed depends only on the already-externalized previous block") |
| S8 | Cluster factor: log-domain sigmoid, 1×–6×, midpoint 100,000 BTH at exactly 3.5× | **verified** | `05-transactions.tex:229–246` |
| S9 | Emission: 50 BTH halving yearly to 3.125, Phase-1 total ≈611 M BTH, perpetual ~2% net tail (gross > net anticipating burns) | **verified** | `07-monetary.tex:43–87` (611,010,000 BTH; 2.5% gross / 2.0% net) |
| S10 | Sybil-proofness of progressive fees (splitting preserves factor) | **verified** | `05-transactions.tex:297–324` (φ depends only on tag vector + global cluster wealth) |

## Non-flag findings

| ID | Severity | Finding |
|---|---|---|
| N1 | minor (carried from v2/v3) | Fig5's "~5 KB tx" body figure: the WP-internal 2-in-2-out size tension remains a whitepaper editorial item, unchanged by #901; not a primer defect. |
| N2 | minor (carried from v2/v3, unchanged) | Capstone summary's absolute "never how much" — out of the #905 scope by explicit operator direction (changelog A3); still rated lossy-but-true for factor-1 spends; optional hedge for factor>1 spends remains available for a future in-scope revision. |
| O1 | obs (operator) — **RESOLVED** | v1–v3's carried O1 (WP "Minting Signatures" vs live-code divergence) is closed by ADR 0006 + PR #901; this v4 realigns the primer. No residue. |
| O2 | obs (operator, carried) | WP-internal editorial inconsistencies unchanged by #901: §10 "Min block time 5s" vs §7's 3 s floor; §2 CLSAG byte figure. No v4 claim touches them. |
| O3 | obs (operator, **new**) | New WP-internal tension introduced by #901: `06-consensus.tex:177` still writes the PoW check as `Hash(block_header ‖ nonce) < target`, while the new `04-cryptography.tex:348–351` equation (and `botho/src/pow.rs:119–135`) hashes `nonce ‖ prev_hash ‖ pk_view ‖ pk_spend` only. The primer is safe on both sides of the tension: §8's unchanged "whole header's hash" wording follows §6, the new §6 minting text follows §4, and the reader's carried-away belief (the hashed data includes the miner's keys, so the win is identity-bound) is true under either formulation — the minter keys sit in the header (`06-consensus.tex:263–265`, `block.rs:173–188`). Spec editorial reconciliation is operator-side. |

**Majors: none.** (`spec_ref` declared and resolves to 18 sections — the missing-spec major does not apply.)
**Critical flags: none.** `audit_clean: true`.
