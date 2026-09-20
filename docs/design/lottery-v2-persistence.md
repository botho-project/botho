# Inactive LotteryV2 persistence boundary

Part of #1286; storage checkpoints #1349 and #1352. This private infrastructure persists
ordinary synthetic block/output fixtures in actual LMDB. It is **not a usable
V2 ledger, validated consensus state, wallet balance, or independent-spend
result**. No production caller can construct its handle. No network protocol
version, reset, migration, credit or activation is introduced.

`Ledger::open` and `open_for_network` retain their V1 record formats and APIs.
Before creating ordinary tables or initializing genesis, they read the optional
`meta/storage_schema` marker. Any present marker is rejected. Existing unmarked
V1 stores remain unchanged. The experimental handle is a separate private type,
with no conversion or dereference to Ledger, so V1 mutation methods cannot be
called on incomplete experimental state.

## Representation

The test-only fresh constructor requires an empty directory. It initializes
nine actual heed tables and the exact schema marker in one transaction. Reopen
requires an existing `data.mdb`, the exact marker and all nine tables; it creates
nothing. The marker is `botho.experimental.lottery-v2.storage.2`, a storage
schema identifier **not** a consensus version. Successful database-handle opens
commit their read transaction before returning the handles.

- `blocks`: the existing little-endian u64 height key. Value is eight bytes
  `BLV2\0\0\0\x01`, u32 little-endian legacy Block payload length, the unchanged
  fixed-integer bincode Block payload, u32 record count, and each record as a
  u32 byte length followed by the existing `LotteryV2::Record` encoding.
  Full consumption is required at every boundary. The 16 MiB Block payload
  bound is local experimental storage policy, not a proposed network limit.
  The existing candidate four-award bound remains unratified.
- `utxos`: unchanged 36-byte actual outpoint keys and unchanged bincode Utxo
  payloads. Outpoints and original derivation indices remain distinct.
- `derivation_contexts`: the same outpoint keys; value is tag u8, creating
  height u64, base index u32, canonical scalar bytes[32], and (for lottery only)
  ordinal u32. Integers are little-endian. Tag 0 is Direct and requires zero
  tweak; tag 2 is Lottery. Unknown tags, truncation and trailing bytes error.
- `meta`: schema marker and experimental checkpoint (height u64 followed by
  block hash[32]), plus the existing chain/accounting/emission metadata encodings.
  The other five production tables retain address, key-image, transaction,
  cluster-wealth and bridge-import indexes. These are fixture effects, not
  proof of a validated chain state.

The envelope rejects disagreements between its ordered Records and the legacy
Block payout fields. Context reads check actual outpoint, creating envelope,
output bytes, height and payout ordinal. Lottery reads also check the immediate
source's stored identity, context presence, matching height, strictly earlier
creation and inherited base index before inheriting cluster tags. These are
bounded immediate-edge storage-consistency checks, not recursive ancestry or
chain authentication. Record derivation, draw eligibility, body-root validity,
PoW and signatures are intentionally not established here.

## Atomicity and remaining integration

One private write transaction calls the same effects writer as V1 Ledger,
inserting the envelope, all coinbase/ordinary/payout outputs and contexts,
indexes, monetary/emission metadata, and the experimental checkpoint. It requires sequential fixture
heights/parent hashes and rejects existing blocks or outpoints. Missing sources
and inconsistent contexts fail. A test callback aborts after each write stage;
all nine table contents must match the prior snapshot immediately and after reopen.
Iteration returns every row or an explicit error, never a partial balance.

`store/writer.rs` now shares the actual V1 effects, including optional emission
updates, with the private experimental handle. See
[the extraction/equivalence record](lottery-v2-shared-writer.md). The earlier
storage-only schema 1 is explicitly rejected; there is no automatic upgrade.
Only an empty, owned test directory can initialize schema 2.

Experimental fixture callers supply accounting and skip signature validation;
production V1 always supplies its existing verification callback. These fixture
inputs are not a validated transition token. The next producer/validator
checkpoint must recompute the actual draw and commitments, preserve all normal
block/transaction checks and bind validation to the applied pre-state before
exposing any real writer. Native wallet discovery/recovery and independently
accepted spends remain subsequent work, as do snapshots, RPC, compact sync,
WASM/mobile clients and legacy disposition. CT transaction encryption codecs
are outside this change.

Run the focused storage tests with:

```sh
cargo test --locked -p botho --lib ledger::experimental::tests
```

Fixtures use no signed transactions or network submission. They cover nonzero
original indices, repeated/nested records, reopen, every write-stage abort,
conflicts, strict decoding, missing/inconsistent references and cross-schema
open rejection without modifying `data.mdb`. Existing V1 ledger regression tests
remain necessary; these fixtures do not replace them.
