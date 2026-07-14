# ADR 0006: Post-Quantum & Privacy Architecture Ratification

**Status**: Accepted
**Date**: 2026-07-14
**Decision Makers**: Core Team

## Context

The first spec-consistency audit of the whitepaper (run as part of the
"Botho from the Basics" primer work, issue #881) surfaced a divergence
between whitepaper §4 and the implementation: the spec claimed ML-DSA-65
signatures on minting transactions, while the code carries no minting
signature at all (attribution binds via the RandomX PoW preimage over
the minter's public keys). Investigation (#899) widened the picture:

- The live transaction path is classical CLSAG with **public amounts**
  and **no ML-KEM ciphertext on outputs** — the whitepaper's
  confidential-amounts (Pedersen + Bulletproofs) and universal ML-KEM
  stealth machinery are specified but not yet implemented.
- `transaction_pq.rs` (`QuantumPrivateTransaction`: per-input dual
  Schnorr + ML-DSA-65 signatures, ML-KEM outputs) predates ADR 0001,
  survived the LION deprecation, and is **unreachable**: blocks are
  typed `Vec<Transaction>`, so QP transactions can never be included —
  yet `send --quantum` builds them and `submit_pq_tx` silently accepts
  and drops them.
- Two prior PQ-spend paths were abandoned in quick succession
  (ML-DSA+Schnorr hybrid deprecated 2025-12-31 in favor of LION; LION
  deprecated 2026-01-03 by ADR 0001).

Four architecture questions were put to ratification.

## Decisions

### 1. Amounts: confidential; fees public (target state)

Amounts sent are **not** transparent: Pedersen commitments +
Bulletproofs remain the normative design. Fees are public. The live
chain's public amounts are an implementation gap, not the design.

**Consequence — open design work**: the anti-hoarding layer is
value-dependent (demurrage ∝ value × time × factor; lottery odds ∝
value × tilt; cluster-tag blending is value-weighted). The whitepaper
does not yet specify how validators verify these over hidden amounts.
This reconciliation (e.g. homomorphic scalar relations + range proofs
for demurrage, blend proofs for tags, a Merkle-sum construction for
weighted lottery sampling) is tracked as its own spec epic and blocks
the CT implementation.

### 2. Recipient PQ privacy: universal quantum stealth

Every output carries an ML-KEM-768 encapsulation (+1,088 B), exactly as
whitepaper §4 specifies (KEM key derived from the standard address;
addresses stay unified and reusable; outputs are one-time). No opt-in
tier: opt-in quantum receiving would partition the recipient anonymity
set — the same fragmentation critique §2 levels at Zcash's optional
shielding.

### 3. Minting attribution: PoW-preimage binding (no signature)

The code's mechanism is ratified as the design. The RandomX preimage
`nonce ‖ prev_hash ‖ minter_view_key ‖ minter_spend_key` binds
attribution; reattributing a reward changes the minting-tx hash (and
with it the cluster origin and reward output) and requires redoing the
work against an externalized chain. Hash-based ⇒ quantum-sound
(Grover-only). The rejected alternative (ML-DSA-65 minting signatures,
as the whitepaper previously claimed) would cost 5,261 B/block ≈ 33–55
GB/yr against ADR 0001's ~100 GB/yr desktop-node budget, add PQ key
management for miners, and enlarge the side-channel surface (cf. the
cycle-6 ML-DSA side-channel fix) for no additional security in this
trust model.

### 4. Quantum-private paid tier: retired

No separate quantum-protected transaction class. The privacy value it
would have sold (quantum-durable recipient unlinkability) is delivered
universally by Decision 2; per-input ML-DSA dual-signing is anti-theft,
not privacy, forfeits ring anonymity on those spends, and is explicitly
not the product story. `transaction_pq.rs`, the `--quantum` send path,
`submit_pq_tx`, and the separate quantum-address surface are to be
removed (this also eliminates the silent-drop landmine). Quantum theft
protection remains the documented migration story (whitepaper §2), with
ML-DSA-65 the designated future signature family.

## Consequences

- Whitepaper: ML-DSA protocol-role claims removed across
  §2/§3/§4/§5/§6/§8/§9 + appendices; §4 gains "Minting Attribution";
  `docs/concepts/pq-migration.md` corrected. (Hotfix PR alongside this
  ADR; closes #899.)
- Spec epic: CT ↔ value-dependent-economics reconciliation must be
  designed and specified before CT implementation starts.
- Code roadmap: implement CT (Pedersen + Bulletproofs + public fee
  conservation) and universal ML-KEM outputs; remove the QP surface.
  All pre-mainnet (testnet is ephemeral; no fork concerns).
- Primer (#881 artifact): §6 and figure 2 teach ML-DSA-signed minting
  per the old spec — needs a v4 revision against the corrected spec.

## References

- ADR 0001 — LION deprecation (kept: ML-KEM stealth, Pedersen amounts,
  CLSAG sender privacy)
- #899 — whitepaper §4 ↔ code divergence report
- Whitepaper §4 "Minting Attribution", §Post-Quantum Stealth Addresses
