# Line-level audit comments — botho-from-the-basics.1/botho-from-the-basics.md

Severity tags: [major] operator-facing defect · [minor] wording-level accuracy improvement ·
[note] verified observation, no change required. Line numbers refer to the v1 body.

- **L79–82 [note]** "Botho's answer — protect *permanent* data with post-quantum cryptography…" — verified against WP §4 Hybrid Architecture Rationale; scope framing ("one of the places where Botho genuinely differs") consistent with WP §1 contribution 1.
- **L120–128 [note]** Ristretto255 + one-way scalar-point picture — verified (WP §3). The paint-mixing analogy carries only one-wayness; lossy-but-true.
- **L190–194 [note]** View-key-to-accountant capability — true under WP §4 (scanning uses only view-side material: sk_kem = DeriveKEM(a)); spend requires b.
- **L229–231 [note]** "picks 19 other outputs… ring of 20" — verified: WP §4 (n = 20), WP §5 `RING_SIZE = 20`, code `transaction/types/src/constants.rs:155`.
- **L239–241 [note]** "roughly 700 bytes per input" — verified (WP §5: 704 B; Parameter appendix: ~700 B at ring 20). WP §2's PQ table lists 704 B against ring *16* — a spec-internal inconsistency (finding F10b), not a primer error.
- **L334 [note]** "sane range (0 to 2⁶⁴)" — verified; WP writes [0, 2^64). No reader-visible difference.
- **L391 [note]** "ML-KEM-768 parameter set (NIST's middle security category)" — verified; FIPS category 3 of 5. Deliberately avoids WP §3's "≈192-bit quantum" figure; no contradiction (finding: none — drafter-flagged spot 2 clears).
- **L402 [note]** "ciphertext (1,088 bytes)" — verified (WP §3/§4/§5, and `PQ_CIPHERTEXT_SIZE = 1088` in code).
- **L417–428 [major → F2]** ML-DSA-65 minting attribution is exactly WP §4/§2/§5. However the reference implementation diverges: live `MintingTx` (`botho/src/block.rs:164-210`) has NO ML-DSA signature (PoW preimage binds minter view/spend keys), while `botho/src/transaction_pq.rs` puts per-INPUT ML-DSA-65 signatures on a quantum-private transaction variant. No primer edit required (the primer's preamble L7 declares "the whitepaper wins"); the operator should reconcile whitepaper §4 with the code (or vice versa).
- **L434–435 [note]** "~50× a CLSAG… ~35 KB per input… exceed 100 KB" — verified (WP §4, §2 survey).
- **L505–507 [note]** Tiered default quorum — verified (WP §6: 3-of-4 infrastructure AND 2-of-3 community).
- **L511–521 [note]** Nominate/ballot/externalize — WP §6 counts four phases by including PoW proposal, which the primer teaches in its §8; lossy-but-true split, dependency-ordered.
- **L521 [note]** "takes a few seconds after a block is proposed" — verified (WP §6 timing table: + ~3–4 s).
- **L594–597 [note]** "validators earn no fees" — verified by exhaustion: WP §7 splits 100% of fees (80% pool / 20% burn) and pays minters R(h) only; WP §10 lists only non-fee validator incentives.
- **L606–607 [minor → F5]** "buys tickets at roughly fair odds per watt" — slightly stronger than WP's ASIC-resistance + linear-scaling claims; hedged. Optional tightening: "at roughly fair odds for its compute class."
- **L614–616 [note]** "3 seconds under heavy use out to 40 seconds… 5 seconds as the reference" — verified (WP §7 dynamic timing; §1 contribution 4). WP §10's constants table saying "Min block time 5s" is spec-internal drift (F10a); the primer follows the governing sections.
- **L643–645 [minor → F7]** "the overwhelming majority of supply" — accurate for decades (69% at year 20) but decays forever under the 2% tail; suggest a horizon qualifier.
- **L664–665 [note]** "1 BTH = 10¹² picocredits; protocol arithmetic is integer picocredits" — verified (WP §7; integer-only code paths). The historical 1000× fee-unit bug is not reproduced anywhere in this primer.
- **L713–719 [note → F8]** "million-BTH lineages saturate near 6×" — WP table says ≈5.2–6.0× at ≥1M (≈5.2× at exactly 1M). Consider "climb past 5× toward the 6× ceiling."
- **L723–726 [note]** Decay 5%/qualifying transfer, 720-block (~1 h) age gate, clock-not-count — verified verbatim against WP §5.
- **L751–759 [note]** Demurrage gloss — verified against code (`cluster-tax/src/demurrage.rs`): charge ∝ value × (factor−1) × elapsed; "proportional to… the cluster factor" is affine-in-factor in truth, but the explicitly stated factor-1 exemption prevents any false belief. WP §7's own phrasing omits the elapsed term; the primer is closer to the implementation than the spec's gloss. Drafter-flagged spot 4 clears: lossy-but-true, not false.
- **L772–774 [note]** "burning can even outpace tail emission" — verified (WP §7 Effective Inflation).
- **L776–780 [note]** Emission share ramping "to 50% of each reward" — verified (WP §7: 10 pp per halving epoch, 50% cap).
- **L788–790 [note]** "a factor-1 coin gets six times the winning weight per BTH of a factor-6 coin" — verified: w = v(6 − φ + 1) gives 6:1.
- **L806–811 [note]** Grinding paragraph — verified against WP §9 ("Grinding is unprofitable by construction"). The primer's "costs more than it can ever win" matches the spec's stated bound.
- **L830–834 [minor → F6]** "while redistribution marginally *improves*" — for the full mechanism WP §10's table shows +0.078 honest vs +0.076 gamed. Suggest "is essentially unchanged" (the spec's own claim is "does not degrade"). "Costs the attacker roughly a fifth" ✓ (~19%).
- **L863–867 [minor → F3]** Fee formula in capstone step 2 omits the output-count penalty (min(2,10)² = 4) that produces the quoted "few tens of nano-BTH" (20,000 pico = 20 nano at ~5 KB). Either add "× a small multi-output penalty" or footnote it; the magnitude as stated is correct.
- **L897 [note]** "~5 KB transaction" for 2-in-2-out — verified: Parameter Justification appendix pins "<5 KB for 2-in-2-out" explicitly.
- **L905–907 [note]** "adaptive target block time is sitting near the 5-second reference" under moderate busy-ness — consistent with WP §7 (5 s at ≥5 tx/s sustained).
- **L933–934 [minor → F4]** "spending two-of-forty possible inputs" — upper bound; rings may in principle share decoys (WP §5 enforces canonical ordering within a ring only). Suggest "two inputs, each hidden among twenty candidates."
- **L935–936 [note → F9]** "a fee whose size reveals only the *lineage class*" — also reveals tx size (public) and, for factor >1 spends, the demurrage clock; accurate in this factor-1 walkthrough.
- **L967 [note]** "NIST FIPS 203 and 204 for ML-KEM and ML-DSA" — verified (WP §3 citations fips203/fips204).
