# ADR 0010: Bridge Custody — Small Elected Multisig, Decoupled from SCP Quorum Structure

**Status**: Accepted (ratified 2026-07-17 by maintainer; option C per `docs/bridge/election-dynamics.md`)
**Date**: 2026-07-17
**Decision Makers**: Core Team
**Related**: ADR 0002 (bridge custody = SCP validator federation — **signer-identity model superseded by this ADR**; the t-of-n threshold-signing mechanics it ratified are unchanged); issues #1060 (direction), #1061/PR #1063 (rotation drill, run live), #1062/PR #1064 (election-dynamics comparison), #1050 (re-scoped), #1019 (mainnet custody hardening), #830/#616 (external audit scope)

## Context

ADR 0002 made the bridge federation *be* the SCP validator set. That identity choice does not survive contact with the custody surfaces: SCP membership is a graph of overlapping quorum slices with no flat, globally-agreed signer list, while every surface we custody on — Ethereum Safe owners, Solana Squads members, the BTH reserve's threshold-attestation key set — is a flat k-of-n list. There is no way to "sign with a quorum graph," and keeping a bridge signer set synchronized with a drifting trust topology is a standing operational hazard. The #1053 federation drill and finding #1050 (order records do not replicate across federation members) made the mismatch concrete.

The full analysis of how the signer set should instead be chosen — option families, 2026 tooling survey, precedents (Gravity Bridge, THORChain, Axelar, Ronin, Clique/QBFT), scoring, and failure modes — lives in `docs/bridge/election-dynamics.md` (#1062/PR #1064). This ADR records the ratified decision and its parameters; it does not repeat the analysis.

## Decision

1. **The bridge custody set is a small elected multisig, not the SCP federation.** Target shape: **3-of-5** machines (threshold k=3, membership n=5). The set is deliberately decoupled from SCP quorum structure; bridge membership confers no consensus weight and vice versa.

2. **Elections follow option C** of `docs/bridge/election-dynamics.md`:
   - **Ballots** are memo-convention transactions on the Botho chain (sub-variant A1 — no protocol change, no new tx type).
   - **Tally** is a deterministic, off-consensus pure function: approval voting, top-N, computed at a published cutoff height over a snapshot of the **P4.4 operator-signed curated node set** (#709) — one node, one vote.
   - **The result is a versioned term document** (schema v2, `docs/bridge/election-dynamics.md` §5). The election pins **membership**; the document **seals** when each winner submits fresh per-term keys signed by their long-lived curated identity key (select-then-keygen). The sealed document carries per-chain execution intents (Safe owner swaps, Squads config, BTH reserve re-key), electorate tally attestations, and the **outgoing set's counter-signature at threshold k (never n)**.
   - **The incumbent set executes the handover** on all three chains via the drilled `rotate` machinery (PR #1063): breaker pause → drain → re-key → **old-keys-powerless assertions gate the resume** → post-rotation threshold attestation + proof-of-reserves.

3. **Scheduled rotation outages are accepted.** Every election is a full re-key on every surface; the pause window is a feature (clean drain + rotation), not a defect.

## Ratified parameters

| Parameter | Value |
|---|---|
| Federation shape | 3-of-5 (tolerates 2 refusers/corpses at handover per §6.4.3) |
| Term length | Quarterly (~93 days) |
| Voting window | ~5 days open-to-close |
| Handover deadline | elected + 72 h (`validity.handoverDeadline`; breach is objective and attributable) |
| Emergency election window | 48 h, `electionKind: "emergency"`; compromised member excluded from counter-signature |
| Candidacy | **Opt-in** (self-nomination memo tx before `openHeight`); every curated node may vote |
| Consecutive-term limits | **None** — re-election is still a full re-key; concentration is monitored, not capped |
| Outgoing counter-signature threshold | k (=3), never n |
| Stalled-chain rule | If the chain cannot confirm ballots at election time, the current term auto-extends until a tally at the deferred cutoff is possible; the breaker remains available throughout |

## Explicitly deferred

- **Ballot promotion to a dedicated tx type (A2)**: revisit after several successful A1 elections; it is a protocol version bump and an external-audit-scope addition, ratified separately.
- **Emergency-recovery module** (electorate-controlled, heavily-timelocked Governor-module/`config_authority` path as insurance against full-set holdout): **not adopted now**. The full k-of-n-collusion gap is accepted for testnet and mitigated by bounded TVL, short terms, and curation exclusion; the recovery-module question is delegated to the external audit (#830/#616) to price at mainnet time. This is the single place option B machinery may still enter the design.

## Consequences

- **ADR 0002** is amended: threshold-signing mechanics stand; "federation = SCP validator set" is superseded.
- **#1050** is re-scoped: order replication must work for ≤5 members (the shared-store topology validated in #1053 already suffices for the single-host drill; a small-cluster design covers multi-host).
- **Build work unlocked**: adopt the v2 term-document schema in the drill's `rotate-elect` step (replacing the v1 mock format), and implement the ballot + tally tooling (self-nomination memo, ballot memo, deterministic tally over the curation snapshot, term-document assembly/sealing).
- **Propagation**: fold elected-multisig custody + rotation into `docs/security/bridge-threat-model.md` (#829) and the external audit scope (#830/#616). #1019's per-chain hardening (Squads, per-role Safes, HSM) plugs into this governance model.
- The schema nits from the #1064 review (sealed-status `if/then` key requirement, tally-signature snapshot scope, canonical example addresses) are folded into the schema-adoption build.

## Alternatives considered

See `docs/bridge/election-dynamics.md` §2–§4: option A alone (no cross-chain delivery — disqualified), option B (third-party governance on ETH/SOL — execution machinery dead (oSnap, Dec 2025), legacy, or a standing external root of trust over custody, the Ronin-2024 attack class; electorate mirroring burden; sovereignty inversion), and the minority report (§4) documenting the one real capability C gives up — electorate-executed rotation over a stalled incumbent set — and why it does not survive a malicious incumbent anyway (`disableModule` front-run).
