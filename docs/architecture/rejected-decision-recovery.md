# Rejected consensus decisions: containment and recovery design

Status: **design for review; recovery is not implemented; live activation is not authorized**.
The sweep authorizes engineering and review of the recovery design.
Part of #1440. Source baseline: `85039ef0`, 2026-09-26. This document accompanies
terminal containment only. Passing its tests does not establish crash-safe
consensus recovery or permit a stopped node to restart.

## Implemented containment and its limits

On a construction failure or a rejected new-height externalized block, the node
leaves the event loop before the application `advance_slot` boundary, stops
minting, networking and RPC, retains the decision/cache in its evidence, and
writes `consensus-failure.json` beside its configuration, including the observed
ledger height and tip hash (or explicit nulls if the checkpoint cannot be read). Creation is exclusive;
files and their containing directory are synced. Existing evidence is never
overwritten. `Node::new` also derives its ledger directory from the config
parent; alternate config filenames in that directory share the same latch.
Any marker, including an empty file, partial JSON, directory or
broken symlink, refuses startup before configuration loading or socket creation.
The complete runtime is dropped before persistence begins. If persistence fails,
the process parks with evidence
in memory until an operator interrupt. An interrupt is not recovery authority.

This is fail-stop containment, not a write-ahead decision log. A power loss before
marker creation or a persistence failure followed by process/host loss can lose
this evidence. The implementation does not claim safety for that crash window.
The ledger remains independently validated; a read error is not proof that a
height was already filled. Existing duplicate-height handling is retained and
is not proof of equality between an externalized decision and the stored block.

Runtime regressions inject an event at the application boundary and exercise the
real builder, ledger rejection, queued-event suppression, cache retention, RPC
connection shutdown before a blocked persistence boundary, and startup latch. They do **not** cause a real quorum to
externalize the invalid fixture or demonstrate recovery across a crash.

## Source findings that constrain recovery

- `ConsensusService::new` starts SCP at `initial_height + 1`. SCP's
  `Node::externalize` advances its in-memory slot before application; its default
  externalized-slot retention is one slot. Ledger height is therefore not a
  restart cursor once a decision is rejected, skipped or committed at an offset.
- `ScpMessage` contains sender bytes, slot and serialized payload. The inner
  `Msg` contains sender/quorum fields, not a standalone signed decision
  certificate. Discovery uses signed gossip but returns the decoded message
  without retaining its gossip envelope in failure evidence. Transport
  authentication and claimed sender fields are not a transferable quorum proof.
- `Ledger::add_block` writes block effects and metadata in one LMDB transaction.
  It currently has no decision journal/application receipt in that transaction.
  An external cursor file cannot prove whether a crash occurred before or after
  the ledger commit. Unknown commit outcomes must be resolved from storage.
- Peer slot anchoring, sync fast-forward, solo mode, quorum reconfiguration and
  duplicate-height skips also move consensus state. Fixing only the terminal
  branch leaves restart safety incomplete.
- Historical markers preserve values, transaction bytes and a rejected block,
  but no signed externalize certificate. Matching operator captures cannot be
  retroactively labeled a certificate. They remain preserved evidence.

## Proposed safety contract

The following is a proposed protocol change requiring independent review before
implementation or activation. It preserves every committed block and every
certified decision. It never changes a decided value set to make it apply.

A decision is identified by network/genesis domain, protocol version, consensus
epoch, slot, immutable quorum-configuration digest, canonical ordered values and
payload hashes. Application additionally binds its exact parent checkpoint,
validation rules version and reconstructed block hash. Canonical encoding,
hashing/domain separation and size limits must be specified and tested; JSON
formatting or a local diagnostic timestamp must not define this identity.

The durable record must include the authenticated envelopes and quorum sets
needed to verify the decision under the applicable federated quorum rules.
Counting a fixed number of signatures is insufficient for arbitrary SCP quorum
sets. Signing must bind the complete inner sender, slot, epoch and payload;
verification must reject mismatches between authenticated identity and claimed
sender, duplicate signers, wrong domains and conflicting decisions. Missing
payloads or incomplete proofs leave the node stopped and eligible only to fetch
and verify evidence. Peer claims alone never advance the durable cursor.

Persist local safety-critical SCP promises before publishing them. Persisting
only an externalization after it happens cannot prevent conflicting votes after
a crash in nomination/balloting. The design must specify a recoverable SCP state
snapshot or replayable write-ahead transcript, including accepted/prepared/commit
state, emitted messages, configuration history and monotonic slot floor. Do not
reconstruct an empty SCP instance merely from a saved next-slot integer.

## Durable state machine and transaction boundaries

Use versioned tables in the same LMDB environment as the ledger for decision
records, application receipts, recovery certificates and the consensus cursor.
Large payloads stored outside LMDB must be immutable, content addressed and
fsynced before a transaction can reference them. A committed reference to absent
bytes is a startup failure. Retention/compaction must preserve verifiability.

| State | Durable data and permitted transition |
| --- | --- |
| Participating | Persist SCP promises before publishing; no cursor regression. |
| Decided, pending application | Commit verified certificate, exact values/payload references, parent checkpoint and slot before application or next-slot activity. Stop admission while resolving application. |
| Applied | In **one ledger transaction**, write block effects, decision-to-block receipt and next cursor/SCP restart boundary. Only after commit may caches be released and participation resume. |
| Rejected | Abort the effects transaction; durably record the immutable decision, checkpoint and deterministic rejection. No nomination, mining or submission. |
| Recovery prepared | Store a proposed recovery certificate/acknowledgment; this alone never resumes participation. |
| Rejection acknowledged | Atomically store the verified recovery certificate, zero-effects receipt and next cursor. Preserve the original decision and marker. Resume only after peer reconciliation and activation checks. |

Every transition uses expected previous state/checkpoint and decision identity,
so retries are idempotent and conflicting rewrites fail closed. The cursor and
application receipt are authoritative together; an unrelated block at the same
height is not an application receipt. Sync must carry authenticated decision
history/receipts, including zero-effects entries, as well as validated blocks.
Unexpected database corruption, an unavailable checkpoint, ambiguous commit or
non-deterministic rejection remains stopped until evidence resolves it.

A proposed rejection acknowledgment explicitly means: slot S decided values V,
V cannot apply under the agreed rules at checkpoint H, V has zero ledger effects,
and the next slot S+1 may decide a new block at H+1. This changes application
semantics and must be approved as such. It is neither ordinary block replay nor
abandonment of the original decision. The rule must distinguish deterministic
invalidity from transient I/O, missing data and unknown commit outcomes; only a
reviewed, stable rejection predicate can authorize zero effects.

The recovery certificate must bind the original decision certificate, parent
checkpoint, rejection predicate/rules version, next slot, next block height,
protocol/epoch and cohort/configuration. Its authorization/quorum rule and
intersection argument with both prior and subsequent decision quorums must be
reviewed. No runtime code may invent a majority threshold or accept an operator
flag as equivalent authority. Until that rule is specified, `Rejected` has no
outgoing recovery transition.

## Startup, peers and subsequent restarts

Startup first checks the latch and durable journal without opening admission.
For the future protocol, a preserved marker may cease to block only when the
verified, exact matching durable recovery receipt authorizes that state; the
marker is never deleted or renamed as a bypass. Legacy markers have no such
receipt and continue to refuse startup.

Reopen LMDB and reconcile journal, ledger tip, cursor, protocol/epoch and saved
SCP state. A pending application is resolved using its exact receipt/checkpoint;
never infer success from height alone. A committed receipt restores the next
slot even if post-commit cleanup never ran. Rejection keeps the process stopped.
Incomplete recovery acknowledgments remain inert. An acknowledged zero-effects
record retains the ledger tip and increments the slot, and every later applied
block atomically carries that offset forward. Repeating startup must not change
any result or redecide any recorded slot.

Peers exchange authenticated epoch/configuration, checkpoint, decision-chain and
recovery-receipt commitments before participating. A peer missing a suffix fetches
and verifies it; conflicting certified history or checkpoints halts reconciliation.
A partition cannot self-authorize recovery. Lagging and newly joining validators
must learn zero-effects entries and receipts, not only block height. Existing
idle slot anchoring and height-only sync are not adequate reconciliation.
Mixed versions must fail closed before voting. Activation must specify a network
protocol version and agreed checkpoint/epoch, rollout/isolation procedure, and
how pre-activation in-flight SCP state is preserved. Old binaries must not reopen
slots or erase journal state after a downgrade.

Historical failure artifacts lack the proof required by this proposal. Whether
they can be admitted under separately authenticated migration authority is an
unresolved design/authorization choice. Default behavior remains stopped. A
closed-testnet exception that abandons an unapplied decision would instead be a
separate epoch/finality tradeoff, requiring explicit operator authorization bound
to exact decision/checkpoint/cohort and a separately reviewed mechanism. It is
not selected here and must never be an automatic fallback.

## Required implementation and verification before closing #1440

1. Review canonical evidence/signature format, identity binding, federated quorum
   verification, rejection predicate and recovery intersection argument. Resolve
   protocol compatibility, activation and historical-evidence authority explicitly.
2. Implement versioned LMDB journal/receipts and durable SCP promises, with an
   atomic block-effects/receipt/cursor commit and explicit schema migration and
   downgrade refusal. Route solo, sync, anchoring, skip and quorum-change paths
   through the same invariants; coordinate cache lifecycle #1437/#1438/#1439.
3. Implement offline evidence inspection/verification and peer reconciliation,
   followed by the reviewed recovery acknowledgment transition. Do not offer
   generic marker deletion, slot overrides, ledger rollback or transaction replay.
4. Add deterministic crash injection and restart tests at every row below, using
   reopened databases and real independently running validators. Compare complete
   logical ledger state, decision history, durable promises and emitted votes;
   success means no slot is decided twice and no committed effect disappears.
5. Independently review implementation and run isolated multi-node qualification.
   Keep live activation downstream of #1404 and separate authorization. Preserve
   wallets, signed intents, original evidence and campaign deadlines throughout.

| Crash/fault point | Required assertion after repeated restarts |
| --- | --- |
| Before/after promise persistence and outbound publication | No contradictory signed promise; incomplete persistence cannot publish. |
| Before/after decision journal commit | Never forget a public final decision or open an empty instance for its slot. |
| During block-effects transaction, before commit | No partial effects; exact pending decision/checkpoint still available. |
| After commit, before cursor/cache cleanup | Receipt, tip and cursor agree; effects occur once, cache cleanup is harmless. |
| Rejection before/after its durable record | Original decision retained, unchanged ledger, no new admission or slot. |
| Recovery prepare/acknowledgment commit | Incomplete acknowledgment inert; committed zero-effects receipt applied once. |
| First and later valid blocks at a slot/height offset | Offset survives each commit and restart; neither prior slot reopens. |
| Lost/corrupt proof, wrong signer/domain, missing payload | No participation; evidence-fetch/reconciliation cannot fabricate authority. |
| Partition, minority recovery, stale/mixed-version peer | No incompatible recovery, decisions or automatic epoch change. |
| Duplicate height with a different decision; unknown commit outcome | Height alone never permits cursor advancement or replay. |

None of these future recovery/crash criteria is satisfied merely by the current
containment runtime tests. This issue remains open after the containment lands.
