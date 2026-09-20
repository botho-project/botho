# Offline legacy lottery inventory

Part of #1286; this tool inventories existing public legacy payout semantics. It
neither repairs payouts nor changes storage, consensus, wallets or balances.

Create a **consistent, immutable offline ledger snapshot** using your existing
backup process before running this tool. Do not pass a live node directory.
Copying a changing `data.mdb` is not a snapshot protocol. The tool requires an
explicit acknowledgement via its sole input option:

```sh
cargo run --locked -p botho --example legacy_lottery_inventory -- \
  --offline-copy /path/to/immutable-ledger-copy > inventory.json
cargo test --locked -p botho --example legacy_lottery_inventory
```

The tool reads the source file, copies it into an owned temporary directory,
and LMDB-opens **only the disposable copy**, read-only. It does not call
`Ledger::open`, initialize tables, modify source data/locks, migrate records or
submit transactions. The output is stdout; choose an output location outside
the source directory. Source file size/mtime checks detect some concurrent
changes but cannot establish snapshot consistency. The operator remains
responsible for provenance and completeness.

The deterministic report includes the copied data digest, tool source digest,
checkpoint height/hash, available genesis hash, four table digests and record
counts. It strictly enumerates `blocks`, `meta`, `utxos` and `key_images` in one
read transaction, with errors rather than skipped malformed records. Other
indexes are not checked or repaired. Existing V1 serializers are used unchanged;
unsupported block versions fail. Structural identity, adjacent history links,
checkpoint and stored payout/source envelope/amount relationships are checked.
This is **not consensus replay, signature verification or independently
authenticated history**. A coherent edited archive is not proven valid by this
report. The fixture tests are deliberately unsigned structural records.

Known payout edges connect awards to their stored winning outpoints. Groups use
the inherited target/public/KEM envelope and include all matching inventory
records, including original sources. Original derivation index is reported only
when available creation history reaches an ordinary output. Repeated and nested
awards retain their distinct outpoints. Missing blocks, source records and
creation history remain explicit, unresolved entries. Missing expected payout
and ordinary records are listed separately from values actually present.

Every value is nominal accounting inventory, **not an independently spendable
balance**. Unresolved UTXO value is a subset of total UTXO nominal value; group
values may include those same unresolved records and must not be added again.
Unattributed key-image counts cannot identify which shared-key claim, if any,
was consumed. Presence or absence of an image never reduces a group's value or
claims recovery. Missing-history reports cannot certify all aliases were found.
No output of this tool authorizes activation, credits or a legacy disposition.

Finite limits: source 512 MiB, individual key/value 8 MiB, total enumerated
payload 256 MiB, 100,000 enumerated/derived records, and 1,000,000 lineage steps.
Exceeding a limit fails with no partial report. These bounds restrict work and
allocation; they are not a wall-clock timeout or an untrusted-file sandbox.
Production-sized archives exceeding them require a separately reviewed tool
extension, not silent truncation. No timestamps or temporary paths enter JSON.

Tests cover repeated/nested awards and nonzero source index, unattributed image
ambiguity, incomplete history, missing winners/payouts, corrupt tables, wrong
payout amounts, unsupported versions and resource rejection. Source directory
byte comparisons cover success and corrupt-input errors. No actual spend or
accepted-block proof is claimed; that remains later #1286 integration work.
