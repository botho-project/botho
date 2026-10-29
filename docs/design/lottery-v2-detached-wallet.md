# Authenticated detached native wallet (LotteryV2, #1499)

Part of #1286, step P10. Code: `botho/src/ledger/store/v2/detached.rs`.
This is an **inactive** boundary. It adds no RPC route, network message,
consensus rule, storage schema or cryptography, and it reinterprets no V1
record. It rebases the 2026-09-30 experimental prototype (branch
`backup/issue-1499-stale-20260930`) onto the production protocol 7.0.0 rules.

## Problem

The native node wallet (P7) recovers V2 payouts through contexts it reads from
its own authoritative store. A *detached* wallet, with no ledger, must recover
and spend the same outputs from bytes that someone else served. The context
codec alone is not an authentication boundary.

## Trust boundary

- The caller supplies a `TrustedCheckpoint` `(network, height, accepted block
  hash)` from its **own** trust source. The type has no `Deserialize` and is
  never encoded into a witness, so a witness cannot carry, select or imply its
  own anchor. It is not enough to match a body against a header that arrived
  with the response; the only hash that counts is the caller's.
- `Ledger::accepted_checkpoint` is the offline local integration: a node reads
  its own accepted tip, in a call separate from the export. A remote client
  must authenticate the same three fields itself.
- A verified witness proves **inclusion**: every byte it uses is committed by
  the anchored hash. It does not replay consensus (PoW, draw eligibility,
  signatures, monetary rules), because the anchor being accepted is the
  caller's obligation.
- It also proves nothing past the anchor. Spent/decoy facts are "as of the
  anchor"; later spends and the mempool are invisible. Signing requires the
  caller's *current* trusted anchor to equal the snapshot's, and final ledger
  validation of the transaction remains the authority on unspentness.
- Decoded bytes (`UnverifiedEntry`) are private to the ledger module. Callers
  only ever receive a `VerifiedSnapshot`, which holds no store or view handle.
  Discovery, key-image calculation and signing therefore never consult the
  authoritative store.

## What is authenticated (no new commitments)

Under protocol 7.0.0 the header's `tx_root` is the V2 body root
(`lottery_v2::root`). It commits:

| Component | Commitment |
|---|---|
| Minting tx | `MintingTx::hash` (every field) |
| Ordinary transactions | `Block::compute_tx_root` over `Transaction::hash`. Since #1638 this covers every input key image and every field of every output (amount, keys, memo, cluster tags, ML-KEM ciphertext), plus fee, height and settlement. |
| Payouts | `payout_root` over each `Record` (winner, amount, target, DH key, ciphertext, derivation context) |
| Summary | `summary_root` |

The prototype needed a private full-payload `ordinary_root` because the
pre-#1638 transaction hash omitted output fields. The production hash makes
that unnecessary, so the witness reuses the production body root unchanged.

`Transaction::hash` does not commit rings or CLSAG signatures, so the witness
does not carry them. A witness transaction is the hash projection (key
images, complete outputs, fee, height, settlement). Every byte in the witness
is authenticated.

The hash layouts leave one alias: a minting ciphertext of `None` hashes the
same as `Some(empty)`. The witness admits only `None` or exactly 1,088 bytes.

## Verification

1. Bounds and decoding (below). The block count must equal anchor height + 1.
2. Block 0 must be the canonical genesis of the anchor's network.
3. Each later block must satisfy:
   - its height follows the previous one, and its parent is the previous
     block's hash;
   - its payouts are rebuilt from the records (`bind_outputs`);
   - its recomputed V2 body root equals its `tx_root`.
4. The final block's hash must equal the anchor hash.
5. Outputs are then replayed in the production writer's order: coinbase
   `(hash, 0)`, transaction outputs `(tx hash, i)`, then payouts
   `(hash, 1 + ordinal)`. Each payout's winner must be an already replayed,
   strictly older output. That rules out missing dependencies and cycles
   without recursion. Each record is then re-checked with `Record::validate`
   against the replayed source and the shared `lottery_v2::domain_for`. Even
   an attacker who also controls the anchor cannot assert lineage, a context,
   an envelope or an amount.
6. These are rejected: duplicate outpoints, duplicate transaction IDs, and
   duplicate key images.
7. Only the first outpoint per target key is ownable, spendable or a decoy
   (the address-index rule).

Winning outpoint, payout outpoint and original derivation index are separate
fields of `DetachedOutput`. A source's spent key image never marks its
independently keyed award spent.

## Recovery and signing

`VerifiedSnapshot::discover` runs P7's `recover_owned` on the verified
contexts. That is the same base scan, the same tweak, and the same final
`x·G = P` check in `recover_with_context`.

`VerifiedSnapshot::create_transaction` calls the shared native constructor
`Wallet::create_private_transaction_impl` through the `WalletRead` seam, so
ring selection, tag inheritance, signing hash and CLSAG are unchanged.
Within that seam:

- `recover_input` refuses inputs that differ from the verified output or are
  already spent as of the anchor;
- `decoys` mirror the ledger's rule (canonical outputs with at least 10
  confirmations, gamma selector).

## Codec and bounds (local transport policy, not consensus)

```
"BTHWIT" 0x02 0x01            magic: lottery rules 2, witness format 1
u32 LE   block count          = anchor height + 1
per block:
  u32 LE length, canonical bincode (fixint, LE) block projection
  u8     record count          <= CANDIDATE_MAX_AWARDS (4)
  per record: u16 LE length, canonical Record::encode bytes
```

Rules:

- All bytes must be consumed.
- Bodies must re-encode byte-identically.
- `Record::decode` is canonical and rejects unknown context versions and
  zero/noncanonical tweaks.
- Every length is checked against its bound and the remaining input before
  allocation.

Bounds:

| Item | Bound |
|---|---|
| Witness | 64 MiB |
| Blocks (including genesis) | 16,384 |
| Block projection | 16 MiB (the ledger's own stored-block bound) |
| Record | 2,048 bytes |
| Aggregate outputs | 262,144 |
| Aggregate key images | 262,144 |

A full history from genesis avoids unauthenticated sparse lineage and spent
or decoy assertions. It is deliberately unsuitable as an unlimited production
sync protocol, and oversized chains fail closed. The real-maturity profile's
733-block chain exports a 1.17 MB witness.

## Tests

All are in `detached/tests.rs`. Negative controls are local and unsigned.

- **`detached_wallet_recovers_and_spends_repeated_nested_payouts`.** Setup:
  - Ordinary blocks are sealed by `seal_block` and accepted by `add_block`.
  - Three award blocks are writer-applied with honest records: `v2::derive`
    over persisted sources, the shared domain, and the real body root. Their
    awards are chosen, not drawn, because no draw can award before 720-block
    maturity.
  - The awards are: one to a coinbase spent before its award, a repeated pair
    to an output at original index 1, and a payout winning twice in a row
    (nested twice).

  Checks:
  - Detached discovery equals `Ledger::discover_wallet_outputs` exactly.
  - All six inputs are detached-signed and accepted in one block by the real
    `add_block`, with distinct key images and exact amounts.
  - Replay is rejected.
  - A stale snapshot or stale discovery cannot sign.
  - A fresh witness reports every input spent and refuses to re-sign.
- **`trust_anchor_is_the_callers_not_the_responses`** covers wrong hash, wrong
  height, wrong network and an over-bound anchor. A prefix witness verifies
  only against its own earlier anchor and shows that anchor's state.
- **`codec_controls_fail_closed`** covers trailing, truncated, unsupported
  witness version, V1 rules byte, oversized block, record count and record
  length, a noncanonical record context version, and a header bit flip.
- **`every_committed_body_component_is_authenticated`** mutates every output
  field, key images, fee, settlement, an extra transaction, mint fields, the
  mint ciphertext alias, the summary, every record field, record order,
  duplication and omission, header fields, the final header and genesis. Each
  mutation gets a typed refusal.
- **`lineage_is_derived_not_asserted_even_under_a_forged_anchor`** re-roots
  and re-links the chain and anchors at the forged tip. It covers an altered
  tweak, base index, target, envelope, a V1-shaped copied target, a missing or
  later (cycle-shaped) source, a duplicate transaction and a duplicate key
  image.
- **`export_reads_accepted_contexts_strictly`**: a missing context fails the
  export.
- **`detached_wallet_spends_natural_payout_at_real_maturity`** (ignored; run
  on the `ci` profile by `workspace-build.yml`) is the natural
  producer → validator → persisted V2 state → witness → detached
  verification, discovery, signing → accepted spend path at the real 720-block
  maturity. It covers the payout plus a fee helper, and the source when still
  unspent. It also checks replay rejection and exact wallet conservation.

## Remaining production gates (parent #1286, not this child)

- A real remote anchor source: a trusted checkpoint or header chain for a
  client.
- Scalable authenticated sync (compact or sparse witnesses) and versioned V2
  snapshots and import.
- Production RPC/client integration (P4 #1641 fields, botho-wallet, WASM,
  mobile).
- Independent cryptographic review (#616).
- Activation disposition.
