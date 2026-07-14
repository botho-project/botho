# Comments — botho-from-the-basics.4

Line references are to `botho-from-the-basics.4/botho-from-the-basics.md`. One minor, four nits; all five are scope-preserve. Severity legend: blocker / major / minor / nit; scope legend: preserve / expand / reduce.

## 1. [minor / preserve] §6's 'permanent secrets' framing now sits slightly askew of the corrected figure and recap (lines 382–383, 461–462)

The bolded design rule "permanent secrets get post-quantum protection; ephemeral secrets get efficient classical protection" (line 382) and the later "the ledger's permanent secrets (who owns what; who minted what) are quantum-safe now" (line 461) survive from v3 unchanged — but v4's own corrections moved the surrounding apparatus off the word 'secret': the fig2 subgraph header was deliberately re-titled 'Must hold forever → quantum-resistant' *because* minting attribution is a public integrity guarantee, not a lattice secret (changelog C3), and §11's recap now says "*permanent* guarantees" for the same reason (C7). 'Who minted what' is stated three paragraphs earlier to be "deliberately public" (line 424), so calling it a 'secret' is loose, and slightly more visibly so than in v3. Not blocking: the immediate context disambiguates, the prior critics passed the identical sentences at ceiling, and touching them is outside the #905 item-6 "NOTHING else" scope. If a future pass opens the body, align both spots to the guarantee framing (e.g. 'permanent guarantees get quantum-resistant protection'). Flagged at minor rather than nit only because the v4 delta itself established the better vocabulary.

## 2. [nit / preserve] 'The block of data a miner hashes' compresses the four-field preimage (lines 432–434)

"The block of data a miner hashes over and over in the mining race (Section 8) *includes the miner's own public address keys*" is deliberately vague about what the hashed data is — the spec's preimage is specifically nonce ‖ prev_hash ‖ view key ‖ spend key (`04-cryptography.tex:348–351`), not the whole block body. The vagueness is doing pedagogical work (the reader needs 'the hashed data names the winner', nothing more), the changelog self-declares it lossy-but-true, and the conclusion it supports is exactly the spec's. Dim-4-clean simplification; noted so the auditor can adjudicate if it disagrees. No change requested.

## 3. [nit / preserve] §6 says 'a block the network has already finalized' one section before §7 teaches finality (lines 437–438)

"redo the mining race against a block the network has already finalized" uses 'finalized' in its ordinary-English sense (already settled), which a newcomer bridges without SCP knowledge; §7 then sharpens the word into its technical meaning. Same pattern class as the accepted `(Section 8)` forward pointer — a bridgeable preview, not a block. No change requested.

## 4. [nit / preserve] The caveat's retired-tier sentence names a thing the fresh newcomer never met (lines 997–1000)

"a separate, opt-in \"quantum-private\" transaction class from early drafts was retired" addresses readers of older project material, not the primer's own teaching thread — a fresh reader meets the term for the first time in the sentence that buries it. This is the right call (the #905 direction explicitly wanted the retirement stated, and the sentence self-contains: "dropped, not deferred ... never sold as a tier"), and confining it to the single §11 caveat block keeps it out of the teaching sections. Keep single-location. No change requested.

## 5. [nit / preserve] fig2's caption still previews scheme names one subsection early (line 387)

The v3 review's comment-1 convention carries into the rewritten caption: ML-KEM-768 and the PoW-preimage binding are each named with their already-taught role ("recipient identity rides the lattice-based ML-KEM-768 handshake", "minting attribution is bound by the hash-based proof-of-work preimage itself — no signature at all") and are unpacked by the two subsections immediately below. Map-before-walk, glossed at point of use; the caption remains a one-sentence re-teach with no new numbers (the ~5.3 KB figure lives in the prose, not the caption). Keep the placement and the convention. No change requested.

## Scope distribution

preserve: 5, expand: 0, reduce: 0
