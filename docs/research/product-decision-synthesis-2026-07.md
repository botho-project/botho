# Product-Decision Synthesis (2026-07): Regulatory Posture, Mobile Custody UX, Community GTM

## Research Summary

**Status:** This is a **reference / decision record**, not scheduled engineering work.
It folds the open product decisions from the Botho vision epics
([#441](https://github.com/botho-project/botho/issues/441),
[#458](https://github.com/botho-project/botho/issues/458)) into a sequenced roadmap,
grounded in a 2024–2026 state-of-the-art research pass. The open roadmap items are
**business/strategy** calls, not architecture — none needs more engineering to decide.
Two of them (roadmap steps 1 and 3) are explicitly marked
**`[OPEN — maintainer decision]`** below and must not be read as already-decided.

Provenance: deep-research run `wf_0ccde072` (2026-07-07). Auto-synthesis was stubbed, so
the load-bearing claims below were re-fetched and verified by hand — provenance is noted
per finding, and refuted/unverified claims were deliberately excluded (see
[Provenance](#provenance)).

### What is already decided (do not re-litigate)

Path-A architecture (curated validators + permissionless thin clients; village
hub-and-spoke), **RandomX** PoW (ratified
[#441](https://github.com/botho-project/botho/issues/441)), Stripe billing under
**2amlogic**, React-Native + UniFFI mobile, the trust/quorum model, picocredits. The open
items below build on these; they do not reopen them.

---

## 1. Finding 1 — Regulatory posture is MUCH more favorable than the pre-research flag suggested

This is the highest-value result: the 2025 SEC staff guidance largely covers Botho's
Mode 1 (managed per-user mining node).

- **SEC Div. of Corporation Finance, March 20 2025 — Proof-of-Work Mining Statement:**
  PoW mining, **both solo and via mining pools**, is **not** a securities offering.
  Reasoning: a miner's rewards come from *its own computational resources*, not the
  "efforts of others" (fails Howey's prong); a **pool operator's role is
  "administrative or ministerial,"** not entrepreneurial/managerial.
  (sec.gov/…proof-work-mining-activities-032025; corroborated by Dechert, Steptoe,
  Duane Morris, and MoFo analyses.)
- **SEC, May 29 2025 — Protocol Staking Statement:** extends the same "administrative or
  ministerial" reasoning to **third-party / custodial / staking-as-a-service** run on a
  user's behalf — a node operator staking on behalf of others "does not alter the
  nature… for the Howey analysis."

**Implication for Botho:** a per-user **dedicated** managed rig (the user's own
t4g.medium mining with its own compute) is the solo-mining / pool fact pattern the SEC
blessed, provided 2amlogic's role is framed and operated as **administrative hosting**
(run the node) — not "invest with us and we generate returns." This is both the honest
framing (mining self-equilibrates to break-even; hosting is the durable margin) and the
legally protective one.

### Caveats (must be honest)

1. These are **staff statements, not law / Commission rules** — withdrawable, and
   Commissioner Crenshaw dissented ("Stake it Till You Make It?"). They reflect the
   current crypto-friendlier SEC posture.
2. They address **securities only**. **Money transmission / MSB (FinCEN) is a separate
   analysis** — but if rewards flow **non-custodially** to the user's own on-device keys
   and 2amlogic never takes custody / converts user funds, MSB exposure is low (FinCEN
   2013 virtual-currency guidance).
3. Still get a **confirmatory legal read** on the specific structure before real-money
   Mode 1. Staff guidance *reduces* the need; it doesn't remove it.

**→ Decision de-risked.** Structure Mode 1 as: dedicated per-user compute +
administrative-hosting framing + non-custodial rewards. The legal-review issue is tracked
(see [Follow-ups](#follow-ups)) but is no longer a scary unknown.

---

## 2. Finding 2 — Mobile custody / recovery: MPC/TSS + passkeys transfer to Botho; account abstraction does NOT

Verified signing-layer vs chain-specific split (Turnkey, Para/Capsule, Dynamic,
Fireblocks):

- **Chain-agnostic (transfer to Botho's UTXO / ring-sig, Ed25519/Ristretto keys):**
  MPC/TSS key management (split the spend key into shares), TEE-based key custody,
  **passkeys / WebAuthn** (gate access to an encrypted key), and **Shamir-style social
  recovery** (guardians hold seed shares — pure secret-sharing, no contract).
- **Does NOT transfer (EVM-only):** ERC-4337 **account abstraction**, smart-contract
  wallets, and Argent-style **smart-contract social recovery** — all require a
  programmable-contract layer Botho does not (and by design won't) have.

**→ Recommendation for the mobile app:** the raw-seed-phrase failure mode is the #1
consumer-UX risk. The realistic, buildable stack for a non-EVM chain is:

1. **Passkey / WebAuthn-gated encrypted key backup** (device Keychain + optional
   iCloud/Google) — the mainstream low-friction path; the mobile app already has iOS
   Keychain integration ([#441](https://github.com/botho-project/botho/issues/441)), so
   this is the natural first step.
2. **MPC/TSS or Shamir social-recovery** for higher-assurance users (device share +
   provider/guardian share).

Both are signing-layer, so no chain changes needed. **Do not chase account abstraction**
— it's a dead end for a UTXO chain.

Regulatory note: non-custodial recovery (user holds the shares/backup) keeps the wallet
outside custody/MSB framing; a provider-held MPC share edges toward custody and should be
structured carefully.

---

## 3. Finding 3 — Community / DePIN adoption: anchor to a real use-case, not token/mining speculation

Transferable lessons (Helium & DePIN pivots; community/local currencies;
Umbrel/Start9 self-hosting):

- **The dominant DePIN failure mode is token-incentive-led supply growth that decouples
  from real demand** — networks grew hardware/contributors fast on token rewards, then
  monetization lagged badly (Helium's pivots; the DePIN "usefulness gap"). *(Specific
  per-project revenue figures surfaced by the research were adversarially refuted and are
  deliberately NOT cited here.)*
- **Community currencies succeed on a real local economic loop, fail on speculation** —
  the durable ones anchored to actual local spending, not appreciation bets.
- **Self-hosting (Umbrel/Start9) shows genuine appetite for "one sovereign host serves
  its people"** — validating the village-host hub-and-spoke shape; low host-onboarding
  friction is repeatedly cited as an enabler.

**→ Recommendation:** anchor the village-host GTM to the **real payment use-case**
(friends actually paying each other from their phones — Mode 2, free), NOT to
mining-income speculation (Mode 1). Mode 2 is the viral loop and the honest product;
Mode 1 is a convenience/revenue add-on. Keep host onboarding dead-simple (the
[#458](https://github.com/botho-project/botho/issues/458) one-click provision). This also
composes with Finding 1 (framing away from "returns").

---

## 4. Proposed Roadmap Sequencing

1. **`[OPEN — maintainer decision]` Decide the root: testnet-only vs. drive to mainnet.**
   Everything else keys off this. Mode 1's economics are hollow on testnet. This is the
   maintainer's call — this document records the options, not a decision.
2. **If mainnet-bound**, the two real gates are now clear and tractable: (a) the
   **external audit** ([#616](https://github.com/botho-project/botho/issues/616) — all
   four technical blockers cleared this session), and (b) a **confirmatory legal read**
   structured around Finding 1 (tracked as
   [#722](https://github.com/botho-project/botho/issues/722); scope it as "does our
   dedicated-compute + administrative-hosting + non-custodial-rewards structure sit
   within the 2025 SEC mining guidance, and are we MSB-exempt if non-custodial?").
3. **`[OPEN — maintainer decision]` Reframe the product** per Finding 3: lead with
   **Mode 2** (free phone-to-phone payments, village-host adoption); position **Mode 1**
   as convenience hosting, not mining income (also de-risks Finding 1). This is a
   product-strategy call for the maintainer — recorded here as a recommendation, not a
   decision.
4. **Mobile custody upgrade** per Finding 2: passkey-gated Keychain backup as the
   near-term MVP wallet-recovery improvement; MPC/Shamir later. This is buildable now and
   independent of mainnet timing. Tracked as a follow-up issue (see
   [Follow-ups](#follow-ups)).
5. **MVP scope** (cascades from step 1): if testnet-only near-term, the cheapest
   high-value move is polishing the **existing web surfaces** (wallet / explorer /
   operator dashboard) into a coherent "try Botho on testnet" experience and advancing
   the **mobile app** (the stated primary entry point) — defer building the
   [#458](https://github.com/botho-project/botho/issues/458) billing control plane until
   mainnet + the legal read make it non-theater.

---

## 5. Follow-ups

- **Legal-review issue (Finding 1 structure)** — tracked mainnet gate on
  [#616](https://github.com/botho-project/botho/issues/616) /
  [#458](https://github.com/botho-project/botho/issues/458). **Already exists:
  [#722](https://github.com/botho-project/botho/issues/722)** (`loom:blocked`,
  intentionally gated pending an external legal read before real-money Mode 1).
  Cross-linked here — not duplicated.
- **Mobile wallet recovery-UX issue (Finding 2: passkey-gated Keychain backup MVP)** —
  filed as its own `loom:triage` issue for later curation. This document records the
  research; the feature itself is future scoped work, not part of this record.
- The two `[OPEN — maintainer decision]` roadmap items (steps 1 and 3) are the
  maintainer's calls. This document records the research-grounded options and
  recommendations; it does not resolve them.

---

## Provenance

Deep-research run `wf_0ccde072` (2026-07-07). Auto-synthesis was stubbed, so primary
sources were re-verified by hand:

- SEC Proof-of-Work Mining Statement (2025-03-20).
- SEC Protocol Staking Statement (2025-05-29).
- Turnkey MPC / TEE / account-abstraction analysis (signing-layer vs. chain-specific
  split).

Refuted or unverified claims were **excluded**: specific per-project DePIN revenue
figures and a specific host-onboarding-time statistic were adversarially refuted during
verification and are deliberately not cited.

---

## Related

- [#441](https://github.com/botho-project/botho/issues/441) — product-architecture epic
  (RandomX ratification; flags mobile custody/recovery UX as an open gap).
- [#458](https://github.com/botho-project/botho/issues/458) — managed-node / one-click
  provision vision (the Mode 1 hosting control plane).
- [#616](https://github.com/botho-project/botho/issues/616) — external audit (mainnet
  gate; technical blockers cleared).
- [#722](https://github.com/botho-project/botho/issues/722) — confirmatory legal-review
  issue for the Finding-1 structure (`loom:blocked`).
- [useful-pow-zk-proof-generation.md](useful-pow-zk-proof-generation.md) — sibling
  "parked research record documenting a maintainer instinct with explicitly open
  decisions" pattern that this document follows.
