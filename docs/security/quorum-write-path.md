# Quorum Write-Path Security Design (P4.3)

**Status**: DRAFT — review round 1 (issue #708, 2026-07-08) returned
CHANGES REQUESTED; this revision addresses all four findings. Awaiting
re-review on #708.
**Scope**: the operator-signed quorum-curation write path for the P4 admin
dashboard (#441 P4; architecture proposal on #695; decision record on #664).
**Exit criterion**: implementation issue **#709 is blocked until this design
is security-reviewed and the review outcome is recorded on issue #708.**

Every mechanism claim below is grounded in code on `main` as of this writing;
citations are file:line or function names. Elements that do NOT yet exist are
marked **(new, #709)**.

---

## 1. Why this document exists

A quorum edit is the most dangerous operator action Botho exposes. The quorum
set determines both **safety** (which coalitions can fork the chain — FBAS
splitting sets) and **liveness** (which crash sets halt it — blocking sets);
see `consensus/quorum-sim/README.md` for the verified framing and the
canonical counterexample (a `2-of-4` symmetric set admits the disjoint quorums
`{A,B}` / `{C,D}`, which can externalize conflicting values — an unrecoverable
fork). The #664 decision ratified a remotely-reachable write surface for
exactly this configuration, on the condition that the write path is designed
and reviewed as its own deliverable before any of it ships. This is that
design.

Two structural mitigations do the heavy lifting; everything else is
defense-in-depth around them:

1. **Every edit flows through the existing promotion gate.** The gate
   (`gated_scp_quorum_set`, `botho/src/commands/run.rs:2217`) already refuses
   any candidate quorum set whose FBAS model admits disjoint quorums
   (`symmetric_quorum_has_intersection`, applied at run.rs:2377, keeping the
   previous safe set on refusal). The write path adds **no second way to
   construct a quorum set** — an operator action only mutates the gate's
   *inputs* (`[network.quorum] members`, `max_auto_members`), and the gate
   recomputes and re-validates the output exactly as it does on peer churn.
   A safety-bad edit is therefore **refused before application**; the
   residual failure class is liveness (a stall), which is recoverable (§7).
2. **The signing key never exists where the attacker is.** The operator's
   Ed25519 private key lives on the operator's machine only — never on nodes,
   never in the dashboard's hosting (Cloudflare Pages), never in the BaaS
   worker. Compromise of any deployed component yields at most read access
   and denial, not quorum control (§8).

## 2. Operator keypair lifecycle

- **Algorithm**: Ed25519 (already in the dependency tree via libp2p identity
  keys; small signatures, no parameter choices to get wrong).
- **Generation (new, #709)**: `botho operator keygen` runs on the operator's
  workstation and writes a keypair file (private key encrypted at rest under
  a **mandatory** passphrase — the #474/#475 lesson: no plaintext-by-default,
  no optional password). Prints the public key and its fingerprint
  (`blake2b-256(pubkey)[..8]`, hex) for provisioning.
- **Provisioning**: each node's `config.toml` gains
  `[rpc.operator] action_public_keys = ["<hex pubkey>", ...]` — public keys
  only. A node with an empty/absent list has **no write surface at all**
  (fail-closed: `operator_submitAction` returns "operator actions not
  configured"). Initial provisioning and any change to the key list is an
  SSH/config operation, deliberately outside the signed-action surface —
  key management must not be self-referential (a stolen key must not be able
  to enroll further keys).
- **Dashboard use**: the dashboard (Pages-hosted, `/operator/actions`) signs
  envelopes client-side. The key is imported into the browser encrypted under
  the same passphrase (reusing the wallet vault machinery:
  AES-256-GCM + PBKDF2, `web/packages/core/src/wallet/vault.ts`), held in
  memory for the session only. The Pages host serves static assets and never
  sees the key; there is intentionally **no server-side signing component**.
- **Rotation**: generate a new keypair; add its pubkey to
  `action_public_keys` on every node (SSH); confirm new-key actions verify;
  remove the old pubkey. The list form makes rotation a two-step overlap
  rather than a flag-day.
- **Revocation**: remove the pubkey from `action_public_keys` on every node
  (SSH) — immediate, node-local, and works even if the key is actively being
  abused, because config changes ride the SSH trust domain the attacker (by
  assumption §8.5) does not hold. The audit log (§6) records the signer
  fingerprint of every applied action, so post-revocation forensics can
  attribute damage.

## 3. Signed-action envelope

Canonical JSON (UTF-8, keys sorted lexicographically, no insignificant
whitespace, integers only — no floats), signed as
`Ed25519-sign(sk, "botho-operator-action-v1" || canonical_bytes)`. The domain
separator prevents cross-protocol signature reuse.

```json
{
  "action": "quorum.pin_member",
  "dryRun": false,
  "expiresAt": 1783450000,
  "issuedAt": 1783449700,
  "nonce": "9f2c...32 hex chars (128-bit random)...",
  "params": { "peerId": "12D3KooW..." },
  "signerKeyId": "a1b2c3d4e5f60708",
  "targetNode": "12D3KooWJ5U2gk6Pe9ehZb6aHng2zu7RnUwAKzEYxHbaM6VRo592",
  "v": 1
}
```

- **Action enum (v1)** — deliberately minimal, and restricted to
  `recommended`-mode nodes (see §7.3 for why explicit mode is excluded):
  - `quorum.pin_member` — add a base58 PeerId to `[network.quorum] members`
    (the curated set the gate always admits).
  - `quorum.unpin_member` — remove one.
  - `quorum.set_max_auto_members` — set the auto-promotion cap (u32).
  - **Out of scope for v1**: `mode` flips (explicit↔recommended) and explicit
    `threshold` changes. These have the widest blast radius (an explicit
    threshold interacts with unreachable configured members, §7.2) and stay
    SSH-only until v1 has operational history.
- **The v1 action-set boundary is a verifier-level invariant, not just a
  scoping choice.** The mode/threshold exclusion is the entire mitigation
  bounding a compromised-dashboard attack (§8.3) to recoverable liveness
  harassment. Therefore: the verifier rejects any action outside the v1
  allowlist (fail-closed on unknown actions, §4.7), and **adding a
  mode/threshold action in any future version is gated on a fresh security
  review of this document** — it must not be introduced as an incidental
  feature PR. This is an explicit exit condition on #709.
- **`targetNode`**: the receiving node's base58 PeerId (its libp2p identity,
  surfaced by `node_getIdentity`, #500). Binds the envelope to exactly one
  node — an envelope captured in transit cannot be replayed against a
  different node. Fleet-wide changes are N envelopes, one per node, each
  individually signed; the dashboard automates composing them.
- **`nonce`**: 128-bit random hex, single-use per node (§5).
- **`issuedAt` / `expiresAt`**: unix seconds; `expiresAt − issuedAt ≤ 300`,
  and the node enforces its own clock against both (±30 s skew allowance —
  deliberately tighter than the BaaS webhook verifier's 300 s tolerance,
  because operator actions are interactive and fleet nodes run NTP).
- **`signerKeyId`**: fingerprint of the signing pubkey; lets the node select
  the right key from `action_public_keys` and lets the audit log attribute
  actions without embedding whole keys.
- **`v`**: envelope version; unknown versions are rejected (no downgrade
  path: v1 verifiers reject anything but `"v": 1`).
- **`dryRun` (mandatory bool)** — review finding 1 (#708): dry-run-ness is
  part of the SIGNED payload, never an RPC parameter. A signed preview is
  therefore structurally incapable of being replayed as a real apply (and
  vice versa): the operator's signature covers the operator's intent. This
  generalizes to a verifier invariant stated in §4: **no request parameter
  outside the signed envelope may influence processing.**
- **`acknowledgeDegenerate` (optional bool, new, #709)**: REQUIRED to be
  `true` when the action's resulting membership is below 4 nodes (the BFT
  floor — below it the formula degenerates to n-of-n crash-only tolerance,
  #509). Extends the ratified warn-don't-refuse posture into the write path:
  the node refuses the action UNLESS the operator explicitly acknowledged the
  degenerate posture in the signed payload itself, so the acknowledgment is
  attributable and non-repudiable.
- **Solo-quorum hard floor (review finding 2, #708)**: an action whose
  resulting membership is **1 (the node alone) is refused outright — no
  acknowledgment override**. A 1-of-1 quorum trivially passes the
  intersection check (`1 > 1/2`) yet lets the node externalize alone and
  diverge from the fleet — a self-fork the gate cannot see, because its FBAS
  model is node-local. No legitimate remote-curation use case shrinks a node
  to solo; that remains SSH-only. Evaluated at apply time against the
  then-connected peer set, same as the `acknowledgeDegenerate` rule.

## 4. Node-side verification and apply

**Transport: a new authenticated RPC `operator_submitAction` (new, #709)** —
NOT a config-file reload path. Justification (proposal §3.3):

- A reload path (SIGHUP / file-watch) has no authentication of its own — it
  inherits whatever could write the file, which collapses the design back to
  "SSH is the only boundary" while ADDING a background apply mechanism that
  races the event loop.
- The RPC gives an atomic request→verdict round trip: the caller learns
  *synchronously* whether the gate accepted or refused the edit (§4 step 6),
  which the dashboard renders truthfully (anti-#541: outcomes only ever come
  from node responses).
- `Config::load` runs once at startup (`botho/src/config.rs:798`); there is
  no existing reload machinery to reuse, so a reload path would be MORE new
  code than the RPC, with worse properties.

**Verification order** (all checks constant-time where secret-dependent;
fail-closed; first failure wins and is audit-logged as a refusal):

1. **Config gate**: `action_public_keys` non-empty, else "not configured".
2. **Signer known**: `signerKeyId` matches a configured pubkey.
3. **Signature valid** over the canonicalized envelope with the domain
   separator.
4. **Target binding**: `targetNode` equals this node's own PeerId.
5. **Freshness**: `issuedAt − 30 ≤ now ≤ expiresAt` and
   `expiresAt − issuedAt ≤ 300`.
6. **Nonce unseen** (§5); the nonce is recorded before apply (reserve-then-
   apply, so a crash between the two fails safe — the envelope can never be
   applied twice, at the cost of requiring a re-signed retry after a crash).
7. **Payload validity**: action is a known v1 action; `peerId` parses as a
   base58 PeerId (mirroring the gate's own parse-and-warn at run.rs:2279);
   `max_auto_members` within sane bounds (0..=64); the
   `acknowledgeDegenerate` rule and the solo-quorum hard floor (§3).

**Apply path** — mirrors the #674 relay pattern (RPC → mpsc channel → event
loop), because `rebuild_scp_quorum_set` needs `NetworkDiscovery` and the
consensus handle, which live in the `commands::run` event loop:

1. The RPC handler sends `(envelope, responder)` over a bounded mpsc channel
   into the event loop (the same architectural seam as `tx_relay`,
   run.rs — RPC state holds the sender).
2. The event loop clones the current `QuorumConfig`, applies the mutation to
   the **clone**.
3. It runs `rebuild_scp_quorum_set` with the mutated clone and
   `previous = Some(current quorum set)` — the EXISTING gate, including the
   deterministic auto-set trimming and the
   `symmetric_quorum_has_intersection` check (run.rs:2377).
4. **If the gate refuses** (`quorumGateIntersectionRefused`): the action is
   REJECTED, the config clone is dropped, the previous quorum set stays live
   (that is the gate's built-in behavior), and the refusal — with the gate's
   verdict — is returned to the caller and audit-logged. Nothing persists.
5. **If the gate accepts**: the in-memory config is replaced by the clone,
   the new quorum set is installed in consensus (the same call sites as the
   peer-churn rebuilds at run.rs :624/:947/:1030 use), the config is
   persisted via `Config::save` (`botho/src/config.rs:807`) so a restart
   converges to the same state, and the applied action is audit-logged.
6. The responder returns the full outcome (applied/refused, gate snapshot,
   resulting `[network.quorum]`) to the RPC caller.

**Dry runs.** An envelope with `dryRun: true` (a signed field, §3) runs steps
1–4 identically but never mutates, persists, or consumes the nonce — it
returns the gate verdict for the hypothetical edit. The dashboard uses this
to show the operator the consequence of an action before signing the real
one; the real apply is a **separately signed** envelope with `dryRun: false`
and a fresh nonce. Dry runs still require a valid signature (they reveal
operator-only information: the would-be quorum composition).

**Invariant (review finding 1, #708): the node acts only on the signed
canonical bytes.** `operator_submitAction` takes exactly one argument — the
envelope and its signature. There are no sibling RPC parameters, and any
future parameter that could influence verification or application MUST be a
field inside the signed envelope. The verifier checks signatures over the
**received canonical byte string** and only then parses those exact bytes
(parse-after-verify); it never re-canonicalizes a separately-parsed object.
Optional-field ambiguity is excluded by construction: every v1 field,
including `dryRun` and `acknowledgeDegenerate` where required (§3), has a
defined canonical presence, and an envelope whose bytes parse to unknown or
duplicate keys is rejected at the parse step (after signature verification,
as part of §4 step 7 payload validity).

## 5. Replay protection

- **Nonce store (new, #709)**: a small persisted set of
  `(signerKeyId, nonce, expiresAt)` under the node's data dir. Retention is
  bounded by construction: entries are only needed until their `expiresAt`
  passes (an expired envelope is already rejected by the freshness check), so
  the store is garbage-collected on insert and its size is bounded by the
  action rate within any 5-minute window — trivially small. Persistence
  matters because a node restart inside the 5-minute window must not reopen
  a replay slot.
- **What a captured envelope yields an attacker**: replay against the SAME
  node — blocked by the nonce; replay against ANOTHER node — blocked by
  `targetNode`; replay after 5 minutes — blocked by `expiresAt`; modification
  of any field — blocked by the signature. A captured envelope is therefore
  worthless except as an information leak (it reveals one intended config
  change), and transport is TLS anyway (§8.2).
- **Dry-run exception (accepted)**: because dry runs never consume their
  nonce (§4), a captured `dryRun: true` envelope CAN be re-submitted against
  its target node until `expiresAt`. This is deliberate and bounded: the
  re-submission cannot change state or be converted to an apply (`dryRun` is
  signed, finding 1), and its only yield is re-reading the gate verdict for
  one fixed hypothetical edit for ≤5 minutes — an information leak already
  in the §8.2 row, further limited by TLS transport.
- **Reordering**: two envelopes signed in sequence can arrive out of order
  within their windows; each is individually gate-validated against the
  then-current state, so the result is always a gate-accepted configuration,
  though possibly not the operator's intended final one. The dashboard
  mitigates by submitting sequentially and reconciling against
  `operator_getQuorumInfo` after each apply. (v1 accepts this; a per-signer
  monotonic sequence number is the v2 upgrade if operational history shows
  it matters.)

## 6. Audit logging

- **Store (new, #709)**: append-only JSONL at
  `<data-dir>/operator-audit.jsonl`, one entry per **authenticated**
  verification outcome — applied, gate-refused, and post-signature refusals
  (bad target, expired, replayed nonce, invalid payload). **Pre-signature
  failures (§4 steps 1–3) are NOT audit-logged** (review finding 3, #708):
  they are reachable by any unauthenticated caller on the RPC port, so
  logging them would hand out an unbounded disk-fill / log-spam primitive.
  They get rate-limited `debug!`-level tracing plus a rejected-requests
  counter surfaced in `node_getStatus`. Authenticated entries:

```json
{"ts":1783449912,"signerKeyId":"a1b2c3d4e5f60708","envelopeHash":"blake2b-256 hex",
 "action":"quorum.pin_member","params":{"peerId":"12D3KooW..."},"dryRun":false,
 "outcome":"applied|gate_refused|verify_refused:<reason>",
 "prevQuorum":{"mode":"recommended","members":[],"maxAutoMembers":8},
 "newQuorum":{"mode":"recommended","members":["12D3KooW..."],"maxAutoMembers":8},
 "gate":{"intersectionRefused":false,"threshold":4,"members":5,"curated":1,"auto":3,"suppressed":0}}
```

- `newQuorum` is present only for `applied`; refusals log the *attempted*
  mutation in `params` but no new state (none exists).
- **Surfacing**: `operator_getAuditLog` (new, #709; read-token gated per
  proposal §2) returns recent entries; the dashboard's actions page renders
  outcomes exclusively from node responses — it never fabricates or infers a
  local success state (anti-#541).
- **Tracing mirror**: every entry also emits a `warn!`-level tracing event
  (operator actions are rare and always operationally significant), so
  journald/CloudWatch capture them without depending on the JSONL file.
- The log is node-local and append-only by convention, not tamper-proof: an
  attacker with node root can rewrite it. That is consistent with §8.4 — node
  root already loses that node — and fleet-level attribution survives via the
  other nodes' logs plus the journald mirror.

## 7. Failure modes and recovery

### 7.1 Safety (fork risk) — structurally refused

Splitting-set failures (disjoint quorums; the `2-of-4` counterexample the
quorum-sim verifies) cannot be introduced through this path: step 4.3 routes
every candidate through `symmetric_quorum_has_intersection`, and refusal
keeps the previous set — identical to how the gate already protects the
peer-churn rebuilds (#651). There is no bypass flag, and mode/threshold
actions (the riskiest inputs to that check) are not in the v1 action set at
all.

### 7.2 Liveness (stall risk) — possible, bounded, recoverable

Examples: pinning a peer that then goes offline (in `recommended` mode a
curated member is always admitted when connected, so this mostly self-heals;
in `explicit` mode configured members count toward the threshold whether or
not connected — run.rs:2261 — so pinning dead peers can raise the bar past
what's reachable), or lowering `max_auto_members` below what quorum needs on
a small fleet. The gate accepts these (intersection holds) but the node may
stop externalizing — a stall, **not** a fork (`slotStalled` in
`node_getStatus`, #653, and the /network dashboard make this visible within
seconds).

**Recovery ladder**, in order:

1. **Compensating signed action over RPC.** A stalled SCP slot does not stop
   the RPC server or the event loop; submit the inverse action
   (`unpin_member`, restore the cap). This is the designed, no-SSH path.
2. **Degenerate-posture edits** require the signed
   `acknowledgeDegenerate: true` (§3), so shrinking a quorum below 4 to
   restore liveness is possible but deliberate and attributable.
3. **SSH + `config.toml` + restart** — the root recovery that needs no
   cooperation from the write path at all: edit `[network.quorum]`, restart
   `botho.service`. On startup the gate re-seeds from config and connected
   peers (#427 behavior), and the under-connected re-dial fix (#690/#692)
   makes post-restart mesh healing reliable — validated live in the v0.3.2
   rolling deploy.

An operator runbook (`docs/operations/runbooks/quorum-edit-recovery.md`)
ships with #709 covering ladder steps with exact commands, alongside the
existing runbooks (database-corruption, key-compromise, network-partition,
seed-node-recovery).

### 7.3 Fleet divergence — and why v1 is `recommended`-mode only

Per-node targeting means the fleet's `[network.quorum]` configs can diverge
(one node accepted a pin, another refused because its connected-peer view
differed). Under **`recommended` mode** this is merely operational
untidiness: a curated member counts toward a node's quorum *only when
connected* (run.rs:2297), so a divergent-but-each-intersection-valid fleet
still forms quorums over the peers actually reachable. Each node's LIVE set
passed its own intersection check; the trust dashboard (#707) surfaces the
divergence, and the fleet-action composer treats "some nodes refused" as a
first-class partial-failure outcome.

Under **`explicit` mode** the same divergence is more dangerous: configured
members count toward the threshold whether or not connected (run.rs:2261), so
a fleet that disagrees on membership can have each node individually pass
intersection yet collectively fail to form a quorum any node will accept — a
fleet-wide liveness stall that no single compensating action fixes (each node
needs a different corrective edit). **This is why v1 pin/unpin actions are
restricted to `recommended`-mode nodes** (§3): the write path refuses a
membership edit against an explicit-mode node, so reaching this failure state
requires the SSH path, where the operator already sees and edits the whole
config atomically. Explicit-mode curation graduating into the write path is
part of the same future-review gate as mode/threshold actions (§3).

## 8. Threat analysis

| # | Attacker holds | Can | Cannot |
|---|---|---|---|
| 8.1 | **Stolen read token** | View operator-only reads: per-peer gate classification (a targeting map), quorum configs, audit log, dry-run verdicts — until TTL (≤7d) or secret rotation | Change anything: the token is verify-only HMAC on the read path; `operator_submitAction` never accepts it |
| 8.2 | **Captured envelope** (network vantage) | Learn one intended config change (TLS makes even this unlikely) | Replay (nonce), retarget (targetNode), outlive 5 min (expiry), or modify (signature) — §5 |
| 8.3 | **Compromised dashboard host** (Cloudflare Pages) | Serve a malicious bundle: steal read tokens; prompt the operator to sign attacker-chosen envelopes (the serious risk) | Sign anything itself (no key on the host). Mitigations: the dashboard shows the canonical envelope before signing; malicious signed actions are still gate-checked (no fork), audit-logged (attributable), and bounded by the v1 action set (worst case = liveness harassment, recoverable via §7). Residual risk accepted for testnet; mainnet hardening (SRI, self-hosting the operator page) tracked in #709. |
| 8.4 | **Compromised node** (root) | Ignore/rewrite its own config, forge its own audit log, lie in RPC responses — that node is lost regardless of this design | Sign actions against OTHER nodes (no private key material anywhere on nodes — the design's core invariant); other nodes' gates and logs are unaffected |
| 8.5 | **Malicious insider with the operator key** | Everything the write path allows: liveness harassment within the v1 action set, degenerate-posture edits down to membership 2 (self-acknowledged, attributable) | Fork the fleet (gate, §7.1); drive any node to a self-externalizing solo quorum (membership-1 hard floor, §3); flip mode/threshold (not in v1); enroll new keys or erase fleet-wide audit trail (key list and logs are SSH-domain). Recovery: revoke via §2; SSH ladder §7. |

The design's honest summary: **the write path converts "quorum edit" from a
fork-capable SSH superpower into an attributable, rate-bounded capability
that cannot fork the fleet** — the intersection gate refuses splitting-set
candidates, and the membership-1 hard floor (§3) closes the one edge where a
gate-accepted edit could still make a node diverge (solo self-
externalization). The SSH superpower remains, unchanged, as the recovery
root. The new surface strictly dominates the status quo for safety while
adding operator convenience.

## 9. Review checklist (for the #708 reviewer)

- [ ] Envelope canonicalization is unambiguous (one byte-string per logical
      envelope) and the domain separator is applied.
- [ ] Verification order in §4 has no check that depends on secret data
      before signature verification (oracle risk).
- [ ] Reserve-then-apply nonce semantics (§4.6) fail safe across crashes.
- [ ] The gate-reuse claim holds: no code path in #709 may construct or
      install a QuorumSet except through `gated_scp_quorum_set`.
- [ ] v1 action set stays minimal (no mode/threshold).
- [ ] `dryRun` is a signed envelope field and no request parameter outside
      the signed envelope influences processing (round-1 finding 1).
- [ ] The membership-1 hard floor refuses solo-quorum-producing actions with
      no acknowledgment override (round-1 finding 2).
- [ ] Audit log records authenticated outcomes only; pre-signature failures
      are rate-limited tracing + a counter (round-1 finding 3).
- [ ] 8.3's residual risk (malicious bundle prompting the operator) is
      acceptable for testnet, and the mainnet hardening list is filed.
