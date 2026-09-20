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
moves the transaction payload in memory; the immutable transaction account is
not serialized by Anchor, so this does not erase its persisted payload. A late member without a verified pre-execution binding, a closed
proposal, or execution history unavailable in the newest bounded RPC page
requires historical reconciliation. The engine emits a diagnostic and retains
backing rather than interpreting absence as failure or minting a replacement.
Historical reconstruction and backwards pagination are tracked in
[#1295](https://github.com/botho-project/botho/issues/1295).
There is no automated history import/refund command in this change. Preserve
RPC receipts and database backups for explicit recovery; do not delete the
journal or unlock backing to clear a stuck order.

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
