# Shared ledger effects: V1 equivalence and inactive V2 preparation

Part of #1286; implementation checkpoint #1352, based on #1351 at
`c4ac0f0df46d82b862ca51ca05850794fea9556c`. This extraction joins the existing
production V1 writer and the private experimental storage handle. It does not
implement V2 producer/validator acceptance, wallet spending or network formats.

## Source equivalence map

Line references below are to the parent commit's `botho/src/ledger/store.rs`.
Current shared code is `botho/src/ledger/store/writer.rs`; existing helper methods
in `store.rs` forward to that implementation for their other callers.

| Original operation | Shared operation and preserved behavior |
|---|---|
| 824–1157 pre-write validation | Remains in `add_block_inner`, in the same order; experimental storage does not claim these checks |
| 1158 block serialization/put | Caller retains exact V1 bincode serialization; `block_effects` writes it first |
| 1182 coinbase | Same amount/envelope, `(block hash, 0)`, creation height, then address append and wealth |
| 1204 per-transaction verification | Same `self.verify_transaction` closure, after block/coinbase writes and before each transaction's index/input/output writes; still reads committed key-image state |
| 1211 transaction index | Same hash key and 12-byte height/position encoding |
| 1216 input key images | Same transaction-local collision lookup, error semantics and 8-byte height; catches intra-block collisions |
| 1221 ordinary outputs | Same transaction-hash/index identity and bincode Utxo bytes; address, wealth, then import recording |
| 1258 payouts | Legacy branch still resolves the persisted winner in the current write transaction, inherits keys/KEM/tags, uses payout amount and no memo; `(block hash, ordinal + 1)`; address/wealth only |
| 1310–1335 base metadata | Height u64, tip hash, total mined u128, actual burn u128, carryover pool u128; same arithmetic and order |
| 1345–1385 optional emission | `None` writes nothing; `Some` preserves supplied difficulty, total/epoch transaction counts, epoch emission/burns and reward, all u64 |
| 1389 commit | Remains one commit in the caller, after every block effect and optional emission write |
| 1685 address helper | Raw appended 36-byte outpoints under target key; no added deduplication |
| 2263 key-image helper | Any existing row is a collision; malformed diagnostic height remains zero; DB errors propagate |
| 2530 transaction-index helper | Height 8-byte LE followed by index 4-byte LE |
| 2663 wealth helper | u128 amount × weight / scale, skip zero contribution, reject non-16-byte existing wealth, saturating u128 addition |
| 2717 import helper | Existing epoch-derived matching rule and empty value; only ordinary outputs invoke it |

V1 selects `Representation::Legacy` explicitly. Its new representation checks
and context operations are no-ops; existing overwrite behavior is unchanged.
A header field cannot select the experimental representation. There are no
changes to fee policy, draw rules, serialization or validator logic.

## Frozen before-extraction evidence

`botho/tests/fixtures/v1-writer-effects.json` was captured before extracting the
writer. It contains a fixed ordinary mint-only block and exact key/value bytes
for all eight tables after `add_block_with_emission`, plus a second snapshot
covering the existing address/wealth/import/key-image/transaction-index helpers.
These are local valid-operation and storage-helper fixtures; no signed-network
transaction was created or submitted.

Fixture SHA-256:
`1f7084834d96b9927b31c17d4a244af9e541420f86d19d3dd75d7d2f475714ea`.
The committed test only replays and compares the fixed file; it has no capture
or regeneration switch. It also compares reopened table contents. The same
fixture passed before and after extraction. Another shared-writer test asserts
the verification callback occurs after the four block/coinbase writes and that
its rejection rolls those writes back, including after reopen.

## Experimental schema and atomicity

The experimental marker advances from storage schema 1 to schema 2. Schema 1
cannot be reopened as a fully indexed store; no migration/repair path exists.
Schema 2 initializes the eight production tables plus derivation contexts and
explicit metadata in a single transaction in an empty test directory. Default
Ledger opens continue rejecting experimental markers before initialization.

The handle uses the actual shared effects writer. Its private fixture method
supplies unvalidated pool/emission values and a fixture-only verification
callback. It preserves source-context consistency checks, rejects conflicting
outpoints and writes its envelope/context/checkpoint in the same transaction.
This is reusable durable infrastructure, **not a validated V2 ledger**. The
subsequent producer/validator child must supply validated transitions and a
coherent pre-state before any real caller is exposed.

The full storage fixture populates every one of the nine tables and checks
metadata widths/values, burn/pool accounting identities, transaction/key-image
records, inherited tag wealth and payout target indexes. These identities test
writer effects, not signature validity or a circulating-supply proof. A raw
storage input exercises key-image writes without signing or submitting it.
The test counts actual shared writes, aborts after each one, and compares all
nine tables immediately and after reopen. It includes the final optional
emission writes and experimental checkpoint, so a partial commit cannot pass.
Earlier nonzero-index, repeated/nested context and strict decoding tests remain.

No snapshots, wallet/RPC exposure, live migration/reset, legacy credit, policy
ratification or activation is included. The broader #1286 gate remains open.
