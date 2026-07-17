# Election Dynamics for the Elected Bridge Multisig

**Status**: DRAFT — comparison writeup for the #1060 ADR. Human ratification
happens on #1060; this document scores the option families from #1062,
recommends one, and specifies the election-result artifact (term document)
schema that the #1061/#1063 rotation drill's mock `rotate-elect` seam adopts.

**Related**: [ADR 0002](../decisions/0002-bridge-custody-scp-validator-federation.md)
(bridge custody federation), [ADR 0005](../decisions/0005-bridge-v1-chain-scope-ethereum-and-solana.md)
(chain scope), [testnet e2e runbook Phase D](testnet-e2e-runbook.md#phase-d--the-key-rotation-drill-1061)
(the rotation drill this feeds), [quorum write-path design](../security/quorum-write-path.md)
(P4.4 curation — the electorate anchor), `scripts/bridge-testnet-federation.sh`
(`rotate-elect` — the mock seam).

---

## 1. Context and requirements

The #1060 direction (maintainer-ratified 2026-07-17): bridge custody is a
**small elected multisig of fewer than five machines** (e.g. 2-of-3 or
3-of-5), deliberately decoupled from SCP quorum structure, rotated by
**periodic elections among the consensus nodes** on a cadence of every few
months, with a **scheduled bridge outage** tolerated during each rotation.
Every rotation is a full re-key on all three custody surfaces — that part is
already built and live-drilled (#1061 / PR #1063): pause → drain → fresh keys
→ old-keys-provably-dead → resume, executed against the live Sepolia Safe.

What is *not* yet designed is the election itself. Requirements, from #1060
and #1062:

| Requirement | Source |
|---|---|
| Electorate = the operator-curated consensus node set (P4.4 / #709 operator-signed quorum curation is the sybil-resistance anchor) | #1060, #1062 |
| Exactly one vote per curated node; **no token-weighting** | #1062 criterion 2 |
| One election must authorize re-keys on **three chains**: BTH reserve (a funds-moving UTXO re-key), Ethereum Safe owner swap, Solana Squads member swap (testnet: `SetAuthority`) | #1060, #1062 criterion 3 |
| Result verifiable by all parties — nodes, bridge members, users | #1062 criterion 1 |
| Quarterly-ish cadence, <5 winners, tolerated outage window | #1060 |
| The mechanism must not be heavier than the thing it governs | #1062 criterion 6 |
| Defined behavior for: stalled chain at election time (#1051 is the live example), no-quorum, ties, refusal to hand over, emergency (compromise-triggered) elections | #1062 criterion 4 |

A structural fact that shapes everything below: **the handover machinery
already consumes a single JSON term document** (`election/term-<K>.json`,
runbook Phase D). Whatever mechanism we pick, its entire output contract is
"produce that document, authenticated." The election mechanism is therefore
separable from the handover mechanism — which is exactly why the #1063 drill
could ship first with a mock.

---

## 2. The option families, concretely

### Option A — On-chain on Botho (ballots as transactions, deterministic tally)

Hold the election on the Botho ledger itself:

- **Ballot**: a transaction from each curated node during a defined voting
  window `[openHeight, closeHeight]`. Two sub-variants:
  - **A1 — convention over existing tx fields**: a self-send carrying a
    structured memo (e.g. `BTHVOTE:v1:<term>:<approved nodeIds>`), signed so
    it binds to the node's curated identity key. No protocol change, no new
    consensus surface; validity rules live entirely in the tally convention.
  - **A2 — dedicated ballot tx type**: first-class validation (only curated
    identities may emit one, one per term, well-formed candidate list). Adds
    a new transaction type = new consensus and audit surface, protocol
    version bump, external-audit scope growth (#830/#616).
- **Tally**: SCP gives ordering and finality, not choice — so the "election"
  is a **deterministic pure function** over the ledger: take the P4.4
  curated-set snapshot at `openHeight`, collect the last valid ballot per
  curated identity in the window, approval-count candidates, top-N wins.
  Anyone replaying the ledger computes the identical result. This is the
  Clique/QBFT lineage: EIP-225 signers vote in headers with majority tally
  and epoch resets; QBFT even supports reading the validator set from
  contract state — deterministic tally at an epoch cutoff is a
  well-precedented pattern in small permissioned networks
  ([EIP-225](https://eips.ethereum.org/EIPS/eip-225),
  [Besu QBFT docs](https://besu.hyperledger.org/private-networks/how-to/configure/consensus/qbft)).
- **Result externalization**: the tally is *implied by* the ledger but not
  *packaged* for external chains. An Ethereum Safe cannot verify a Botho
  ledger tally; something must carry the result off-chain. Option A alone
  stops at "the result exists and is verifiable on Botho."

Known concerns explored:

- **Censorship/grinding of ballot txs**: block producers could delay or drop
  ballots. Mitigations: a voting window many blocks long (days, not blocks),
  ballots re-submittable at will (last-valid-wins), and an electorate small
  enough (5–20 nodes) that a censored voter notices and escalates
  out-of-band. Tie-breaks must not depend on grindable entropy (see §6.3).
- **Stalled chain**: if the chain is halted at `closeHeight` (exactly the
  #1051 situation — betanet frozen at height 202 on the faucet cap), the
  election simply has not happened yet; see §6.1.

### Option B — Third-party governance tooling on ETH/SOL

Hold the election where (most) custody lives, with audited external tooling.
The 2026 landscape matters here, because **the 2023-era answer is dead**:

- **DEAD: Snapshot + oSnap.** UMA deprecated oSnap on **2025-12-15**; its
  docs now state it "will not be able to execute transactions from your
  DAO's Safe treasury" and tell DAOs to disable the module
  ([UMA docs](https://docs.uma.xyz/resources/osnap)). The optimistic-oracle
  trust model it rode on also took a public hit in March 2025 when a whale
  with ~25% of UMA voting power forced a false resolution of a $7M
  Polymarket market
  ([The Block](https://www.theblock.co/post/348171/polymarket-says-governance-attack-by-uma-whale-to-hijack-a-bets-resolution-is-unprecedented)).
  "Snapshot vote → oracle → Safe executes owner swap" is off the table.
- **LEGACY: Zodiac Reality module (SafeSnap).** Still exists, can execute
  `swapOwner`, but last release v2.0.0 (Aug 2022), single audit, and lives
  under Snapshot's v1 plugin interface with no clear future in the v2 UI.
  Snapshot v2's own "execution" is **read-only** (export txs to a Safe for
  manual signing); Snapshot X's real execution strategy drags in a Starknet
  dependency. Snapshot survives as a *signaling* layer only.
- **ALIVE (mainstream): OZ Governor as a Safe module + soulbound badges.**
  Deploy `ERC721Votes` membership NFTs (transfer-locked, one per curated
  node = exact one-node-one-vote), an OZ Governor (Contracts 5.x, actively
  maintained), enable the Governor/Timelock as a **module on the existing
  Safe**; a passed proposal calls `execTransactionFromModule` →
  `safe.swapOwner(...)`. No oracle, no bonds; Tally remains the standard UI
  ([OZ governance docs](https://docs.openzeppelin.com/contracts/4.x/governance),
  [Tally + Zodiac Governor module](https://tally.mirror.xyz/yECOrCqaKHo7-EIWDOeEMXQMIJnmQOter1SPHeE6SEU)).
  *Unverified*: Tally's 2026 pricing for tiny Governor spaces.
- **ALIVE (purpose-built): Hats Protocol + Hats Signer Gate v2.** Safe
  signing rights derive from wearing a "signer hat"; the election outcome
  re-assigns hats and the Safe owner set converges automatically. Sherlock
  contest-audited; real elected-council deployments (Purple, Questbook)
  ([HSG v2 docs](https://docs.hatsprotocol.xyz/for-developers/hats-signer-gate-v2)).
  The hat-admin is the real root of trust and should itself be a Governor —
  i.e. HSG composes with, rather than replaces, the Governor pattern.
- **Solana side: Realms (SPL Governance) council with Membership tokens →
  Squads v4 `config_authority`.** SPL Governance v3 "Membership" tokens are
  non-withdrawable, revocable seats — clean one-member-one-vote — and a
  Governance PDA set as a Squads v4 `config_authority` can add/remove
  members and change thresholds directly
  ([SPL Governance README](https://github.com/solana-labs/solana-program-library/blob/master/governance/README.md),
  [Squads v4](https://github.com/Squads-Protocol/v4)). But the maintenance
  posture is weak: solana-labs SPL was **archived March 2025** (maintained
  fork at Mythic-Project), spl-governance is flagged "minimal maintenance"
  on lib.rs, and the docs themselves warn the programs are unaudited-as-you-
  deploy-them. Squads v4 itself is superbly audited (OtterSec ×3, Neodyme
  ×3, Certora ×3, Trail of Bits) — but Squads has **no election product**;
  its `config_authority` is precisely a delegation of total membership
  control to whatever governance sits above it. The Neodyme audit's warning
  is the design in one sentence: a compromised `config_authority` can
  instantly replace all members.
- **Cross-chain delivery**: an election held on Ethereum must still
  authorize the Solana re-key and the **BTH reserve re-key**. Wormhole
  MultiGov (Tally + ScopeLift) is the 2026-standard hub-and-spoke pattern
  with Solana support, but nothing reaches Botho's own L1 without a custom
  light client — in practice Botho nodes would have to read Ethereum
  finality and attest the result natively, which quietly rebuilds option
  C's attestation layer anyway, with the vote itself now living on a chain
  Botho doesn't control.
- **Electorate binding**: curated Botho node identities must be mapped to
  ETH (and SOL) keys via an attestation registry, and badge mint/burn must
  track P4.4 curation changes — a standing synchronization obligation
  between the curated set and one (or two) external allowlists.

### Option C — Hybrid: vote on Botho, signed term document executed everywhere

Ballots + deterministic tally on the Botho chain exactly as in option A
(sub-variant A1, convention-only, to start). The tally's output is then
packaged as a **signed election-result artifact — the term document** — which
is the *only* thing the per-chain handover machinery consumes:

1. **Election phase**: ballots on Botho; deterministic tally at
   `closeHeight` yields the elected membership.
2. **Attestation phase**: a quorum of the electorate (the curated nodes,
   which each computed the same tally) signs the result — the *tally proof*.
3. **Key-submission phase**: each elected member generates **fresh** per-term
   keys for every surface and submits them, signed by their long-lived
   curated identity key. (This is exactly the drill's `rotate-keys` step,
   now with the keys bound into the document.)
4. **Counter-signature phase**: the **outgoing federation counter-signs the
   sealed document as its final act** — this is what external chains
   actually verify, because on ETH/SOL only the incumbent k-of-n can execute
   the re-key. Refusal to counter-sign is a visible, attributable protocol
   violation (see §6.4).
5. **Execution phase**: the existing #1063 machinery runs, consuming the
   term document: Safe `swapOwner`/`addOwnerWithThreshold`, Squads config
   transaction (mainnet) or `SetAuthority` (testnet), BTH reserve sweep —
   all inside the pause window, all gated by the old-keys-dead assertions,
   all before a **handover deadline** embedded in the document.

This is the shape real bridges converged on. **Gravity Bridge** is the
canonical precedent: the chain produces a new valset, the *current
(outgoing) set signs it*, and the Ethereum contract verifies old-set
signatures over the new set — with signing made mandatory on a deadline
tied to the unbonding period, because a set that can refuse to sign forever
can freeze valset updates forever
([Gravity slashing spec](https://github.com/cosmos/gravity-bridge/blob/main/spec/slashing-spec.md),
[How Gravity works](https://blog.althea.net/how-gravity-works/)).
**THORChain** churns its vault set every ~3 days: new set runs keygen for
fresh vaults *after* selection, funds migrate old→new, and keygen failures
are penalized with exclusion-and-retry
([vault behaviors](https://dev.thorchain.org/bifrost/vault-behaviors.html)).
**Axelar** rotates gateway key sets ~daily with a grace window of ~5 old
epochs ([Axelar security](https://docs.axelar.dev/learn/security/)).
Election-then-artifact-then-old-set-executes is not exotic; it is the
dominant production pattern for exactly this problem.

---

## 3. Scoring matrix

Scores: ++ strong, + adequate, − weak, −− disqualifying-if-unmitigated.

| Criterion | A (on-chain only) | B (ETH/SOL governance) | C (hybrid) |
|---|---|---|---|
| 1. Verifiability by all parties | ++ | + | ++ |
| 2. Electorate binding to P4.4, one-node-one-vote | ++ | − | ++ |
| 3. Cross-chain result delivery (BTH + ETH + SOL) | −− | − | ++ |
| 4. Failure modes (stall, no-quorum, ties, holdout, emergency) | + | − | + |
| 5. Added audit/consensus surface | ++ (A1) / − (A2) | −− | ++ (A1 conventions) |
| 6. Operational weight for <5 seats, quarterly | ++ | −− | + |

Prose for the cells that matter:

- **(1) Verifiability.** A and C: any party replaying the Botho ledger
  recomputes the tally; users and bridge members verify the same artifact.
  B is verifiable *on Ethereum* — a real strength of Governor — but Botho
  nodes and BTH-side users must trust an Ethereum read to learn who
  custodies their own chain's reserve, which inverts the sovereignty
  relationship.
- **(2) Electorate binding.** A/C: the electorate *is* the P4.4 curated set,
  referenced by a snapshot of the operator-signed curation document — no
  mapping layer. B: needs soulbound badges (ETH) plus Membership council
  tokens (SOL) continuously mirrored from the curated set; the badge
  mint/burn authority becomes a new privileged role, and every curation
  change now has three copies that can skew. Snapshot's `whitelist`/`ticket`
  strategies solve one-address-one-vote, but not the identity-mirroring
  obligation.
- **(3) Cross-chain delivery.** This is where A alone fails: a tally that
  lives only in the Botho ledger cannot move a Safe owner set. B fails in
  the other direction: a Governor proposal that swaps Safe owners still
  says nothing about the Solana re-key or the **BTH reserve sweep** (a
  funds-moving transaction on Botho!); bridging the result back needs
  Wormhole MultiGov (ETH→SOL) *plus* a bespoke Botho ingestion — i.e., B
  degenerates into C with extra dependencies. C is designed around this
  criterion: one artifact, three execution intents, one counter-signing
  authority (the outgoing set) that already exists on every chain.
- **(4) Failure modes.** No option escapes the hard one — a colluding
  outgoing k-of-n can refuse to hand over external-chain custody (§6.4).
  B's Governor-module variant *appears* to solve this (governance can
  `swapOwner` without incumbent signatures) but actually relocates it: the
  Safe's current owners can front-run governance by calling
  `disableModule`, so the incumbents retain a veto anyway — while the
  module adds a standing path by which a captured governance process
  re-keys custody *without* incumbent consent. Ronin's August 2024 incident
  is the cautionary tale for exactly this class: the valset-update/upgrade
  path itself was the attack surface (`_totalOperatorWeight = 0` after a
  botched redeploy let a 70% threshold pass with zero votes)
  ([Halborn analysis](https://www.halborn.com/blog/post/explained-the-ronin-network-hack-august-2024)).
- **(5) Audit surface.** A1/C add **zero** protocol changes — ballots are a
  memo convention, the tally is an off-consensus pure function, and the
  external-chain surfaces are the ones #1063 already exercises. A2 (new tx
  type) bumps the protocol version and grows the consensus audit scope; it
  is an upgrade path, not a prerequisite. B adds: badge contract + Governor
  + module wiring (ETH), forked-SPL governance program with weak
  maintenance posture (SOL), and a cross-chain messaging dependency — all
  inside the #830/#616 external-audit perimeter, all for four elections a
  year.
- **(6) Operational weight.** The election is ~5–20 voters choosing <5
  winners quarterly. A/C: one memo tx per voter, one script anyone can run
  to tally, one JSON document. B: contract deployments on two external
  chains, badge lifecycle management, proposal choreography, gas, and UI
  dependencies (Tally pricing unverified) — the mechanism would be heavier
  than the federation it governs, which #1062's criterion 6 explicitly
  forbids.

---

## 4. Recommendation

**Adopt option C**: ballots as memo-convention transactions on the Botho
chain, deterministic approval tally at a cutoff height over the P4.4
curated-set snapshot, packaged into a **versioned, signed term document**
(schema in §5) that the elected members complete with fresh keys, the
electorate attests, the outgoing federation counter-signs, and the existing
#1063 handover machinery executes on all three chains under a hard deadline.

Rationale, condensed:

1. **It is what production bridges actually converged on.** Gravity Bridge
   (chain-produced valset, outgoing-set signature, deadline), THORChain
   (select-then-keygen-then-migrate, exclusion-and-retry), Axelar (rotating
   key sets with grace windows) — every surviving design separates *choosing
   the set* (on the home chain) from *executing the handover* (by the
   incumbent set, everywhere custody lives). Option C is that pattern with
   Botho's names on it.
2. **B's execution machinery is dead, heavy, or a new root of trust.** The
   2023 default (Snapshot+oSnap) was deprecated in December 2025; the
   Reality module is legacy; the live alternatives (Governor-as-module, HSG
   v2, Realms→Squads `config_authority`) all work by **installing a
   standing on-chain authority that can re-key custody** — precisely the
   attack surface Ronin's 2024 incident and the Neodyme `config_authority`
   warning describe. For a quarterly, 5-voter election, that trade is
   upside-down.
3. **C degrades gracefully into what we already have.** The #1063 drill is
   C with a stubbed step 1: the term document exists, the executors exist,
   the old-keys-dead gate exists and was proven live. Adopting C means
   designing one tally convention and one signature envelope — not new
   contracts on three chains.
4. **Sovereignty.** The set that custodies the BTH reserve should be chosen
   by a process the BTH chain's own participants can verify natively.
   Requiring Botho nodes to trust an Ethereum governance outcome to learn
   their own bridge custodians is the wrong direction of dependence.

Start with sub-variant **A1** (memo convention, no protocol change). If the
mechanism proves out over several real elections, promoting ballots to a
dedicated tx type (A2) with first-class validation is a clean,
independently-auditable upgrade — ratify that separately.

### Minority report — the strongest counterargument

The strongest case against C is that **on Ethereum and Solana it leaves the
incumbent set as the sole execution authority, with no on-chain recourse if
they stall** — whereas B's Governor-as-module gives the electorate a
custody-rotation path that does not require incumbent signatures at all.
Under C, if 2-of-3 outgoing members collude (or simply lose keys
simultaneously), the Safe and Squads custody are frozen until social/legal
pressure resolves it; Botho's only native lever is curation exclusion
(§6.4), which punishes but does not unfreeze. A Governor module with
soulbound electorate badges and a timelock would let the *voters* execute
`swapOwner` over the heads of a stalled federation. That is a real
capability C gives up. The counter-counterargument — and why we still
recommend C — is threefold: (a) the module path is only as safe as its
governance, and a captured or buggy governance process becomes an instant
full-custody re-key (Ronin 2024 class); (b) the incumbents can front-run
with `disableModule`, so the module is not actually holdout-proof against a
*malicious* incumbent set, only against a *passive* one; and (c) the
passive-loss case is better handled by threshold margins (3-of-5 tolerates
two dead members for handover signing) and by bounded TVL while the trust
model matures. A scoped emergency-recovery module (heavily timelocked,
enable-able only by unanimous incumbent action at term start) is listed as
an open question in §7 rather than part of this recommendation.

---

## 5. The term document (election-result artifact)

### 5.1 Keys or membership? — resolving the #1063 review question

The PR #1063 review observed that the mock document embeds the
*pre-rotation* attestation pubkeys (fresh keys are generated later, in
`rotate-keys`), so the mock effectively pins **membership identity, not the
new term's keys**, and asked #1062 to decide which the real document
carries.

**Decision: the election pins MEMBERSHIP; the sealed term document pins
BOTH.** Concretely, the document has a two-stage lifecycle:

1. **`elected`** — produced by the tally. Contains the term, electorate
   snapshot reference, elected member *identities* (curated node IDs), and
   the tally proof. Contains **no new keys**, because they do not exist yet.
2. **`sealed`** — produced after key submission. Each elected member has
   generated fresh per-surface keys and bound them to the document with a
   signature from their long-lived curated identity key; the outgoing
   federation has counter-signed the completed document. Only a `sealed`
   document authorizes execution.

Justification:

- **Key-generation timing.** Fresh keys per term is the entire point of the
  #1060 rotation-as-re-key design. Candidates cannot be required to
  pre-generate custody keys before knowing they won (and pre-published keys
  would sit un-used as attack surface); the #1063 drill itself generates
  keys *after* the election (`rotate-keys` follows `rotate-elect`). The
  elected-then-sealed lifecycle matches the drill's phase order exactly, so
  the drill adopts this schema by moving its key-material readback from
  `rotate-elect` to the end of `rotate-keys` (which "seals" the document) —
  nothing else changes.
- **THORChain precedent**: churn selects members first, then the selected
  set runs keygen for the fresh vaults; keygen failure is a *distinct*
  failure mode with its own retry path (§6.2). Conflating election with key
  possession would force re-running the election on a mere keygen failure.
- **But execution must pin keys.** The external-chain executors need exact
  key material (`swapOwner` arguments, Squads member pubkeys, the BTH sweep
  destination), and the old-keys-dead assertions need the authoritative new
  set. An artifact that stops at membership would push key authenticity
  into out-of-band channels — the sealed document keeps the entire
  authorization chain (electorate → member identity → fresh key → outgoing
  counter-signature) in one verifiable object.

### 5.2 Schema (v2)

`v: 1` is the #1063 mock (`electionKind: "mock-same-set"`, flat member keys).
`v: 2` is the real schema below. JSON Schema (draft 2020-12), abridged to
the normative fields:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "botho:bridge:term-document:v2",
  "type": "object",
  "required": ["v", "term", "electionKind", "status", "electorate",
               "tally", "threshold", "members", "execution", "validity",
               "signatures"],
  "properties": {
    "v": { "const": 2 },
    "term": { "type": "integer", "minimum": 1 },
    "electionKind": { "enum": ["scheduled", "emergency", "mock-same-set"] },
    "status": { "enum": ["elected", "sealed"] },
    "electorate": {
      "type": "object",
      "required": ["curationDocHash", "snapshotHeight", "eligible"],
      "properties": {
        "curationDocHash": { "type": "string" },
        "snapshotHeight": { "type": "integer" },
        "eligible": { "type": "array", "items": { "type": "string" } }
      }
    },
    "tally": {
      "type": "object",
      "required": ["rule", "openHeight", "closeHeight", "ballots", "resultHash"],
      "properties": {
        "rule": { "const": "approval-top-N-v1" },
        "openHeight": { "type": "integer" },
        "closeHeight": { "type": "integer" },
        "ballots": { "type": "integer" },
        "resultHash": { "type": "string" }
      }
    },
    "threshold": { "type": "integer", "minimum": 2 },
    "members": {
      "type": "array", "minItems": 2, "maxItems": 5,
      "items": {
        "type": "object",
        "required": ["index", "nodeId"],
        "properties": {
          "index": { "type": "integer" },
          "nodeId": { "type": "string" },
          "approvals": { "type": "integer" },
          "keys": {
            "type": "object",
            "required": ["ed25519AttestationPubkey", "ethSafeOwner",
                         "solanaMember", "bthReserveKey"],
            "properties": {
              "ed25519AttestationPubkey": { "type": "string" },
              "ethSafeOwner": { "type": "string" },
              "solanaMember": { "type": "string" },
              "bthReserveKey": { "type": "string" }
            }
          },
          "keySubmissionSig": { "type": "string" }
        }
      }
    },
    "execution": {
      "type": "object",
      "required": ["ethereum", "solana", "bth"],
      "properties": {
        "ethereum": {
          "type": "object",
          "required": ["safe", "intent"],
          "properties": {
            "safe": { "type": "string" },
            "intent": { "enum": ["swapOwner", "addRemoveOwner"] },
            "newThreshold": { "type": "integer" }
          }
        },
        "solana": {
          "type": "object",
          "required": ["authority", "intent"],
          "properties": {
            "authority": { "type": "string" },
            "intent": { "enum": ["squadsConfigMemberSwap", "setAuthority"] }
          }
        },
        "bth": {
          "type": "object",
          "required": ["intent", "newReserveAddress"],
          "properties": {
            "intent": { "const": "reserveSweepFactor1" },
            "newReserveAddress": { "type": "string" }
          }
        }
      }
    },
    "validity": {
      "type": "object",
      "required": ["electedAt", "handoverDeadline", "termEnd"],
      "properties": {
        "electedAt": { "type": "integer" },
        "handoverDeadline": { "type": "integer" },
        "termEnd": { "type": "integer" }
      }
    },
    "signatures": {
      "type": "object",
      "required": ["tallyAttestations", "outgoing"],
      "properties": {
        "tallyAttestations": {
          "type": "array",
          "items": {
            "type": "object",
            "required": ["nodeId", "sig"],
            "properties": { "nodeId": { "type": "string" },
                            "sig": { "type": "string" } }
          }
        },
        "outgoing": {
          "type": "array",
          "items": {
            "type": "object",
            "required": ["index", "ed25519AttestationPubkey", "sig"],
            "properties": { "index": { "type": "integer" },
                            "ed25519AttestationPubkey": { "type": "string" },
                            "sig": { "type": "string" } }
          }
        }
      }
    }
  },
  "allOf": [
    {
      "if": {
        "required": ["status"],
        "properties": { "status": { "const": "sealed" } }
      },
      "then": {
        "properties": {
          "members": {
            "items": {
              "required": ["index", "nodeId", "keys", "keySubmissionSig"]
            }
          },
          "signatures": {
            "properties": { "outgoing": { "minItems": 1 } }
          }
        }
      }
    }
  ]
}
```

The `if/then` above is normative (it resolves the #1064 review's schema
nit): the per-term `keys` and `keySubmissionSig` on every member, and at least
one `outgoing` counter-signature, are OPTIONAL while `status == "elected"`
(the tally has run but keygen has not) and REQUIRED once `status == "sealed"`.
The standalone, machine-checkable copy of this schema lives at
[`schemas/term-document.v2.schema.json`](schemas/term-document.v2.schema.json)
and a worked, schema-valid instance at
[`schemas/examples/term-3.sealed.example.json`](schemas/examples/term-3.sealed.example.json).

Normative semantics not expressible in JSON Schema:

- **Signing payloads**: every signature is over `botho.bridge.term.v2:`
  (the domain-separation prefix, following the attestation-envelope discipline
  in `bridge/core/src/attestation.rs`) concatenated with a canonical JSON
  encoding (sorted keys, no whitespace). The three signatures cover three
  DIFFERENT, precisely-scoped snapshots — this is the #1064 review's nit (b):
  - `keySubmissionSig` (each member, signed by its long-lived curated identity
    key) covers `{v, term, nodeId, keys}` — that member's own fresh key
    submission and nothing else, so a member binds exactly the keys it
    generated.
  - `tallyAttestations[].sig` (each elector, signed by its long-lived curated
    identity key) covers the **`elected`/tally snapshot**: the
    election-decided fields only —
    `{v, term, electionKind, electorate, tally, threshold,
    members:[{index, nodeId, approvals}]}` (membership identities, NO keys, NO
    execution/validity, `signatures` omitted). Electors sign the tally result
    ONCE at close, and — because none of these fields change when the document
    is later sealed — those same attestations remain valid on the sealed
    document without re-collection. A verifier recomputes this projection from
    the sealed document to check them.
  - `outgoing[].sig` (each outgoing member, signed by its *retired* per-term
    attestation key) covers the **complete `sealed` document with the
    `signatures` object removed** (status `sealed`, all `keys` and
    `keySubmissionSig` present) — the full authorization chain, so the
    outgoing set counter-signs exactly what it is handing custody to.
- **`elected` → `sealed` transition** requires: every `members[].keys`
  block present, every `keySubmissionSig` valid against the member's
  curated identity key, `tallyAttestations` from a majority of
  `electorate.eligible`, and `outgoing` signatures reaching the *outgoing*
  term's threshold.
- **`resultHash`** is the hash of the full tally transcript (every counted
  ballot tx id + the derived ranking), so a verifier can fetch and re-check
  the exact evidence set without replaying the whole ledger.
- **`validity.handoverDeadline`** is a hard wall-clock deadline (Unix
  seconds; heights are unusable across three chains): if execution +
  old-keys-dead verification has not completed by then, the document
  expires unexecuted (§6.4). This is the **Gravity lesson** — valset
  updates that can be deferred indefinitely become a freeze vector, so
  Gravity binds signing to the unbonding period; we bind handover to an
  explicit deadline in the artifact itself.
- **Mock compatibility**: the #1063 drill adopts v2 with
  `electionKind: "mock-same-set"`. `rotate-elect` writes the `elected`
  document (membership only — dropping the v1 mock's premature key fields);
  a dedicated `rotate-seal` phase, running after the key-generating legs
  (`rotate-keys`/`-solana`/`-bth`), collects the fresh per-term keys, has each
  member sign its `keySubmissionSig`, gathers the outgoing counter-signatures
  at threshold, and flips the document to `sealed`. `rotate-verify` then
  treats the sealed document as the authority: it re-validates it against the
  committed schema and asserts its pinned per-member keys equal the live key
  files before running the old-keys-dead probes. `term`, `threshold`,
  `members[].index`, `members[].keys.ed25519AttestationPubkey` and
  `.ethSafeOwner` keep their v1 names, so the diff to the driver is
  incremental. Testnet Solana/BTH custody is single-key (#867/#1051), so those
  per-member key fields carry the shared fresh value (or a `mock:` marker when
  a leg is gated); mainnet issues each member a distinct Squads/reserve share.
  An offline `term-doc-selftest` phase exercises this whole elected→sealed
  transition (real signatures, schema validation) with no live services.

### 5.3 Worked example (sealed, scheduled election, term 3, 2-of-3)

Every address, pubkey and node id below is a NON-FUNCTIONAL placeholder —
repeated-digit hex, `EXAMPLE`-tagged base58, or `node-example-NN` — chosen so
nothing here can be mistaken for a real live deployment (this resolves the
#1064 review's nit (c): the earlier draft reused look-alike values from the
live Sepolia Safe, devnet program and testnet node roster). A committed,
schema-valid copy of this instance lives at
[`schemas/examples/term-3.sealed.example.json`](schemas/examples/term-3.sealed.example.json).

```json
{
  "v": 2,
  "term": 3,
  "electionKind": "scheduled",
  "status": "sealed",
  "electorate": {
    "curationDocHash": "0000…0003",
    "snapshotHeight": 41800,
    "eligible": ["node-example-01", "node-example-02", "node-example-03",
                 "node-example-04", "node-example-05"]
  },
  "tally": {
    "rule": "approval-top-N-v1",
    "openHeight": 41800,
    "closeHeight": 42520,
    "ballots": 5,
    "resultHash": "1111…1111"
  },
  "threshold": 2,
  "members": [
    {
      "index": 1,
      "nodeId": "node-example-01",
      "approvals": 5,
      "keys": {
        "ed25519AttestationPubkey": "1111…1111",
        "ethSafeOwner": "0x1111111111111111111111111111111111111111",
        "solanaMember": "Examp1eSo1anaMember1111111111111111111111111",
        "bthReserveKey": "bth1qexamplereserveshare111…"
      },
      "keySubmissionSig": "ed25519:1111…"
    },
    {
      "index": 2,
      "nodeId": "node-example-02",
      "approvals": 4,
      "keys": { "ed25519AttestationPubkey": "2222…2222",
                "ethSafeOwner": "0x2222222222222222222222222222222222222222",
                "solanaMember": "Examp1eSo1anaMember2222222222222222222222222",
                "bthReserveKey": "bth1qexamplereserveshare222…" },
      "keySubmissionSig": "ed25519:2222…"
    },
    {
      "index": 3,
      "nodeId": "node-example-03",
      "approvals": 4,
      "keys": { "ed25519AttestationPubkey": "3333…3333",
                "ethSafeOwner": "0x3333333333333333333333333333333333333333",
                "solanaMember": "Examp1eSo1anaMember3333333333333333333333333",
                "bthReserveKey": "bth1qexamplereserveshare333…" },
      "keySubmissionSig": "ed25519:3333…"
    }
  ],
  "execution": {
    "ethereum": { "safe": "0x000000000000000000000000000000000000cAfE",
                  "intent": "swapOwner", "newThreshold": 2 },
    "solana":   { "authority": "Examp1eSquadsAuthority11111111111111111111111",
                  "intent": "squadsConfigMemberSwap" },
    "bth":      { "intent": "reserveSweepFactor1",
                  "newReserveAddress": "bth1qexamplereserve333…" }
  },
  "validity": {
    "electedAt": 1760400000,
    "handoverDeadline": 1760659200,
    "termEnd": 1768435200
  },
  "signatures": {
    "tallyAttestations": [
      { "nodeId": "node-example-01", "sig": "ed25519:aa01…" },
      { "nodeId": "node-example-02", "sig": "ed25519:bb02…" },
      { "nodeId": "node-example-03", "sig": "ed25519:cc03…" },
      { "nodeId": "node-example-04", "sig": "ed25519:dd04…" }
    ],
    "outgoing": [
      { "index": 1, "ed25519AttestationPubkey": "old1…old1", "sig": "ed25519:ee05…" },
      { "index": 2, "ed25519AttestationPubkey": "old2…old2", "sig": "ed25519:ff06…" }
    ]
  }
}
```

(Handover window in this example: elected at T, deadline T+72h, term ends
~93 days later — see §7 for the ratification-pending numbers.)

---

## 6. Failure modes

### 6.1 Stalled chain at election time

The live example is #1051: the betanet has been frozen at height 202 since
2026-07-16 (every node paused minting at the faucet cap), so no ballot tx
could confirm today. Because election boundaries are **heights**, a stalled
chain does not corrupt an election — it postpones it: `closeHeight` simply
has not been reached. Policy:

- **The incumbent term auto-extends** until a valid sealed term document
  exists. `validity.termEnd` is a *target*, not a dead-man switch — an
  expired term with no successor keeps custody (the alternative, custody
  lapsing into nothing, is strictly worse).
- If the stall outlasts a defined grace period (proposal: one full election
  cadence), the situation is by definition a chain-level incident, and the
  bridge should be **paused via the existing breaker** anyway — a chain that
  cannot confirm ballots cannot confirm deposits or releases either, so the
  bridge is already effectively halted on the BTH side.

### 6.2 No-quorum elections and keygen failure

- **Turnout below the attestation majority** (fewer than majority-of-eligible
  valid ballots / tally attestations): the election is void; incumbent term
  extends; re-run after a short, fixed interval (proposal: 1 week). Two
  consecutive void elections escalate to the operator (curation layer) —
  chronic non-participation of curated nodes is a curation problem, not an
  election-mechanism problem. Keep the required turnout realistic for a
  5–20 node electorate (the eth-research note on Governor quorum sizing
  applies unchanged: set quorum so a few absentees cannot stall).
- **Elected member fails key submission** (dies, declines, or cannot produce
  valid keys before the seal): THORChain-style **exclusion-and-retry** — the
  next-ranked candidate from the same tally replaces them (the tally is a
  full ranking, not just top-N), no re-vote needed. If the candidate list is
  exhausted, treat as a void election (§6.2 first bullet). A member who
  declines *after* sealing triggers §6.5 machinery (the document is re-cut
  and re-sealed without them if threshold still holds, else emergency
  election).

### 6.3 Tie-breaks

Ties at the Nth seat are broken **deterministically and grind-free**:
lexicographic order of `nodeId` (exact, stable, computable by everyone).
Deliberately *not* hash-of-block-at-closeHeight or any ledger-derived
entropy: block producers can grind such entropy, and #1062 explicitly flags
grinding. Predictability of the tie-break is acceptable in a small
permissioned electorate — voters who dislike the predictable outcome can
break the tie themselves with their approvals. (Precedent: Clique resolves
competing same-epoch votes purely by deterministic majority rules, and
resets pending votes at epoch boundaries so stale ballots cannot enact
late — our per-term ballot binding gives the same hygiene.)

### 6.4 Refusal to hand over

The hardest case, and the one with the richest precedent:

- **Gravity Bridge** makes not-signing a chain-produced valset update a
  slashable offense with a deadline (the unbonding period), precisely so a
  >1/3 cartel cannot freeze valset updates forever; signing a *fake* valset
  is separately slashable by evidence submission.
- **THORChain** penalizes keygen/churn obstruction from bond and retries
  with the offender excluded; the ultimate backstop is that outgoing bond
  exceeds vault value, making holdout economically irrational.
- **Botho has no slashing economics** (bridge members post no bond). What we
  can and cannot do:
  1. **Deadline + visibility** (have): `validity.handoverDeadline` makes
     "the handover did not happen" an objective, timestamped fact, and the
     term document identifies exactly who failed to counter-sign or
     execute. Refusal is attributable, not deniable.
  2. **Curation exclusion** (have): outgoing members who refuse are removed
     from the P4.4 curated set by the operator — losing consensus
     participation, electorate membership, and all future candidacy. This
     is the analog of slashing reputation instead of stake.
  3. **Exclusion-and-retry** (have): if refusal is partial (1 of 3 refuses,
     threshold 2 still reachable), the handover proceeds without the
     refuser — the drill's Safe choreography already migrates the signing
     set swap-by-swap, and the outgoing-signature requirement is a
     threshold, not unanimity. Design rule: **outgoing counter-signature
     threshold = the multisig threshold k, never n**, so up to n−k
     refusers/corpses cannot stall.
  4. **What we cannot do** (honest gap): if a full k-of-n colludes, ETH and
     SOL custody is frozen and BTH reserve funds are hostage until resolved
     socially. No design in this document removes that — B's governance
     module doesn't either (incumbents can `disableModule` first). The
     structural mitigations are bounded bridge TVL while the trust model
     matures, short terms (a quarterly re-key shrinks the window any given
     set is worth corrupting), and the audit-scoped emergency-recovery
     question in §7.

### 6.5 Emergency election on compromise

A mid-term member-key compromise (THORChain's May 2026 Asgard incident is
the live-fire precedent: one vault compromised, signing halted 13h, churn
paused during investigation) reuses the same machinery with tighter
parameters — design once, get both (#1060):

1. **Breaker first**: pause is immediate and unilateral (any operator),
   exactly as in the drill; investigation happens paused.
2. `electionKind: "emergency"`: compressed window (proposal: 48h
   open-to-close instead of ~5 days), same tally rule, same document.
3. **The compromised member's signature is excluded** from the outgoing
   counter-signature requirement (their key is presumed adversarial);
   because the outgoing threshold is k-of-n (§6.4.3), the handover remains
   executable as long as the compromise is a minority. A majority
   compromise is a §6.4.4 event: keep paused, resolve socially, sweep what
   the honest minority plus breaker can protect.
4. The old term's keys are then provably dead via the standard
   `rotate-verify` gate before resume — the emergency path gets the same
   old-keys-powerless guarantee as a scheduled rotation.

---

## 7. Open questions for #1060 ratification

1. **Term length and windows**: quarterly terms (~93 days)? Voting window
   ~5 days? `handoverDeadline` at elected+72h? Emergency window 48h? The
   schema carries all of these as data; the ADR should pin defaults.
2. **Tally rule details**: approval voting with top-N is proposed
   (`approval-top-N-v1`); confirm N (3 vs 5) and the federation threshold
   (2-of-3 vs 3-of-5) — #1060 says "<5 machines", both fit. 3-of-5 buys
   §6.4/§6.5 margin (two refusers/corpses tolerated) at more ops overhead.
3. **Eligibility to *stand*** vs eligibility to *vote*: is every curated
   node a candidate by default, or is candidacy opt-in (a self-nomination
   memo tx before `openHeight`)? Proposal: opt-in, so §6.2's
   exclusion-and-retry list contains only willing operators.
4. **Consecutive-term limits**: may the same machines be re-elected
   indefinitely? (Re-election is still a full re-key, so the security win
   survives; the question is governance concentration, not key hygiene.)
5. **Ballot promotion to a dedicated tx type (A2)**: after how many
   successful convention-based (A1) elections do we ratify first-class
   ballot validation? This is a protocol version bump and an addition to
   the #830/#616 audit scope — it should be its own decision.
6. **Emergency-recovery module (the minority-report question)**: should
   mainnet custody carry a heavily-timelocked, electorate-controlled
   recovery path (Governor-module on the Safe / Squads `config_authority`)
   as insurance against full-set holdout — accepting the Ronin-2024-class
   surface that adds — or is bounded TVL + short terms + curation exclusion
   the accepted answer? This is the one place option B's machinery could
   still enter the design, and it must be inside the external audit scope
   (#830/#616) if adopted.
7. **P4.4 dependency**: the electorate definition assumes the
   operator-signed curated-set document (#709, shipped) is the canonical
   registry including a long-lived identity pubkey per node. Confirm the
   curation doc's identity-key field is the one `keySubmissionSig` and
   `tallyAttestations` verify against, and specify `curationDocHash`'s
   exact hash target.
8. **Threat-model + audit propagation**: fold elected-multisig elections
   (this document) into `docs/security/bridge-threat-model.md` (#829) and
   the external audit scope (#830/#616) once ratified.

---

## Appendix: research provenance

The 2026-state claims in §2 come from two research passes (2026-07,
with URLs inline above). Items those passes flagged as **unverified** and
which this document therefore does not lean on: Tally's current pricing for
small Governor spaces; DAOhaus/Baal maintenance level; a sunset date for
Snapshot v1 plugins; UMA's stated rationale for the oSnap deprecation (the
fact is documented; the reason is not); Squads v4 "immutable since Nov
2024" (from Squads' own docs, not verified on-chain); Realms Today Trust
staffing; THORChain May-2026 incident root cause (still under
investigation at last reporting); exact current Ronin operator count.
