# Solana Squads engine and recovery

The federated Solana minter uses the audited Squads v4 vault transaction path
from [ADR 0012](../decisions/0012-solana-squads-pda-mint-execution.md). Each
member signs its own outer transaction. Off-chain Ed25519 attestations authorize
an order; they never substitute for the separate on-chain votes. The vault PDA
signs the inner `bridge_mint` through Squads CPI and pays order-marker rent.

## Configuration

All members use the same multisig, vault index, proposer, member list and
threshold. Each runs with its own member signing key and database:

```toml
[solana]
rpc_url = "<cluster RPC>"
wbth_program = "<verified wbth program address>"
keypair_file = "/run/secrets/solana-member-key"
enforce_key_permissions = true
commitment = "finalized"
mint_signers = ["<member1 hex Ed25519 key>", "<member2 hex key>", "<member3 hex key>"]
mint_threshold = 2
development_direct_mint = false

[solana.squads]
multisig = "<Squads multisig account, not vault address>"
vault_index = 0
proposer = "<member1 base58 public key>"
```

The engine requires the canonical Squads program, correctly owned executable
program/accounts, the expected multisig PDA, exact membership with all three
permissions, a threshold of at least two, and no external `config_authority`.
It verifies the derived vault equals wbth's mint authority, validates the SPL
mint, and checks the vault has a positive lamport balance. This does not prove
sufficient marker rent; exact rent preflight and bounded action retry/fee policy
are tracked in [#1299](https://github.com/botho-project/botho/issues/1299).
RPC transaction preflight is enabled, but funding can change before execution.
Missing or mismatched state blocks
new contributions. Keep fee funding on each member and marker-rent funding on
the vault. This document does not deploy, rotate or fund any live authority.

**One member is the proposer.** Its concurrent processes must share the same
SQLite database. Other members may have independent databases; they discover
and verify the proposer's exact stored instruction before voting. A proposer
outage stops new proposals. There is no automatic failover. Do not start a
second proposer with an independent database or change the proposer while
unresolved proposals exist. Coordinated failover is separate work.

Non-federated local tests can explicitly set `development_direct_mint = true`
and omit Squads/member federation configuration. Its default is false. A failed
Squads validation never activates this mode.

## Durable processing and pause behavior

The global lifecycle remains `DepositConfirmed -> MintPending -> Completed`.
The `solana_mint_intents` table persists the immutable full instruction bundle,
multisig/index reservation, verified binding and each signed action before
broadcast. A unique `(multisig,index)` and action revision CAS coordinate workers
sharing a store. Back up this table together with orders, mints and reserve
records. Generic mint rollback deliberately preserves the intent.

Before execution, `dest_tx` is an explicit `squads:<order UUID>` operation handle,
not a chain signature. Creating or approving a proposal does not complete a mint.
Completion requires the correctly owned order marker, the bound Executed
proposal and an attributed wbth event matching recipient, amount and order at
the configured commitment. A guarded database transaction replaces the handle
in both order and mint records with the actual execution signature.

The local circuit breaker blocks every new signed contribution/execute and
rebroadcast, including work driven by confirmation polling. It still permits
read-only completion of transactions that already landed. Each member has its
own local breaker: pausing one node does not revoke another member's on-chain
permissions. An already transmitted transaction cannot be recalled by a pause.

## Recovery holds

RPC failures, unknown signatures, missing accounts, config drift and ambiguous
proposals retain the binding and reserve backing. Expired action blockhashes
can refresh only on the same binding after chain reconciliation. A competing
index is abandoned only with positive finalized evidence of another payload
and expiry of the old signed action; a timeout is insufficient.

Rejected and cancelled proposals are held in `MintPending` with a diagnostic,
not automatically refunded. An Approved proposal remains executable even when
its index becomes stale after a Squads configuration transaction. Neither stale
indices nor membership changes prove it safe to release backing.

Squads permits closing eligible proposal and transaction accounts. Execution
moves the transaction payload in memory; Anchor does not serialize this
immutable account. The real fixture verifies all persisted bytes remain
unchanged before explicitly closing the accounts through Squads.

## Historical completion

An existing verified binding with a visible Executed proposal/event retains the
ordinary completion path. Otherwise, a finalized owned order marker triggers
read-only historical recovery. Each newly prepared order pins its genesis,
exact payload, multisig, proposer, membership and threshold in a separate
immutable policy journal. Version 2 also pins the already-authorized source
record: order UUID, nonempty source transaction/address, BTH-to-Solana mint route,
gross amount, fee and recipient. Recovery compares both the caller and current
database record with that original binding. It does not re-verify the BTH chain. Restart preserves that policy and the scan cursor.

Recovery requires the successful original `multisig_create_v2` transaction to
match the pinned policy, the exact canonical vault creation/proposal bundle,
actual successful top-level votes from at least the pinned threshold of distinct members,
and the successful Squads execute with its exact wbth CPI/event and owned marker.
A closed account is acceptable only with this positive history; a still-present
account must agree. Completion atomically records the actual execution signature,
order/mint state and evidence audit under intent/history/order CAS checks. It
does not change locked backing or submit a transaction.

The first supported historical authorization epoch runs from multisig deployment
to its first unsupported governance or proposal mutation. The verifier scans
all available finalized multisig history through the deployment anchor and
rejects unsupported Squads actions before execution, including membership,
threshold and vote revocations. Same-slot ordering uncertainty also holds.
Deployment membership is **not** treated as authorization for every later epoch.
Configuration changes in later slots do not invalidate an earlier proven
execution, and an existing pinned policy is not replaced by current settings.
Unsupported later epochs require a future explicit epoch verifier; their backing
remains locked. An unrelated rejected proposal in the same multisig can therefore
make this conservative historical path inconclusive.

The transport supports complete legacy RPC messages and the engine's single
outer execute instruction. Versioned messages, lookup tables, ephemeral signers,
extra mint instructions, missing/pruned transactions, incomplete invocation
logs, contradictory account state and ambiguous proofs retain `MintPending`.
Finalized evidence uses the configured RPC trust boundary; this is not an
independent consensus or archive-availability proof.

Each recovery tick fetches at most one ordinary page and one unresolved
transaction, with a newest-signature probe every 16 ticks. Async I/O has a 15-second
tick timeout (10 seconds for the scan). This is not a preemptive CPU deadline:
synchronous verification uses PDA indexes and explicitly checks a 200,000-step /
250-millisecond cooperative budget between bounded operations. Historical HTTP
bodies are capped at 256 KiB before JSON parsing (account metadata at 4 MiB),
with separate instruction/account/log/data count bounds. Each journal is capped
at 8 MiB and policy data at 64 KiB. Exceeding any limit is inconclusive. Scans preserve same-slot signature
ordering, deduplicate overlap, and refresh through an explicit prior-anchor
overlap before using newer history. Null/error responses retain the unresolved
queue; exhausted search never becomes evidence of failure. Configure bounds in
`[solana.squads]`:

```toml
history_page_size = 32 # 1..100 signatures per page
history_capacity = 4096 # 1..16384 retained transactions/signatures per scan
```

Raise an exhausted capacity deliberately when using an archive RPC. Increasing a
bound resumes the same queued work; it cannot skip the page that exhausted it.
The engine advances recovery automatically while an order remains pending,
including while paused. For one explicit, bounded tick without starting watchers:

```sh
bth-bridge --config bridge.toml --reconcile-solana <existing-order-uuid>
```

Repeat that command to resume the durable cursor. It only accepts an existing
`MintPending` order and intent. It re-fetches trusted RPC evidence and accepts no
unsigned receipt imports or signature overrides. The normal configuration/key
loader is used, but this path never signs or sends. Source confirmation and
federation authorization remain prerequisites of the ordinary prepare path.
Old journals without a pinned historical policy are held with a migration
diagnostic; the command does not guess an old policy from current configuration.
A fresh member must first prepare the existing source-confirmed order through
the normal authorized engine path. If current custody has already drifted enough
to prevent that preparation, recovery requires a separately reviewed migration.

Preserve RPC receipts and database backups. No historical search result permits
an automatic refund, reserve unlock, replacement mint or proposer failover.

## Local verification

Use Solana 1.18.26, Anchor 0.29.0, the committed npm lockfile, and the root Rust
toolchain. From `contracts/solana`:

```sh
npm ci
anchor build
./localnet/run-squads.sh
SQUADS_ENGINE_TEST=1 SQUADS_ENGINE_EVIDENCE="$PWD/engine-execution.json" ./localnet/run-squads.sh
```

The driver loads the committed provenance-checked Squads executable and locally
built wbth into a fresh loopback validator. It uses public deterministic test
keys, a genesis ProgramConfig fixture (not the privileged initializer), and
8 ticks per slot to bound genuine blockhash-expiry tests. No live RPC, wallet,
faucet or external funds are used. Setup waits for finalized state.

The ignored Rust test is invoked explicitly and fails if its driver environment
is absent. It runs real OrderProcessor instances, real federation attestations,
separate file-backed member databases and actual HTTP RPC transactions. It
covers distinct votes, paused quorum/restart, accepted-send response loss,
actual expiry and same-index refresh, competing indices, rejected/ambiguous
proposals, configuration rejection, stale Approved execution, final recipient
balance/supply and exactly-once accounting. Fault injection changes transport
responses/metadata only; successful mint/approval/config transactions execute
on the real programs. The JSON artifact preserves source/program hashes and
finalized transaction logs. This is internal execution evidence, not an
external audit or a live deployment approval.

Run the separate closed-account historical conformance fixture on a fresh local
ledger (the driver refuses an occupied RPC port):

```sh
SQUADS_ENGINE_TEST=1 SQUADS_HISTORY_TEST=1 ./localnet/run-squads.sh
```

This fixture creates a rent collector through real multisig initialization,
executes through two independent engine databases, verifies retained account
bytes, closes both eligible accounts, generates real marker history across
multiple pages and recovers through fresh member databases. It checks restart,
one explicitly injected null RPC response, paused/local-policy-drift recovery,
zero recovery broadcasts, wrong-genesis and changed-source rejection, mixed-order
history, and a hold for a real later governance epoch.
