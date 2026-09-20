# Inactive lottery V2 primitives

Issue #1293 implements the pure portion of the candidate construction in
[#1286](https://github.com/botho-project/botho/issues/1286). These functions are
not called by production consensus, block creation, ledger storage, RPC, wallet
scanning or wallet signing. No feature flag enables them. Existing serialized
structures and V1 header semantics are unchanged.

The implementation is `bth_transaction_clsag::lottery_v2`, shared portable Rust.
It is a candidate for review, not an activation, audited protocol or proof of
mainnet readiness. Parsing a source/context does not authenticate it. Future
consumers must validate source state, recompute the draw and authenticate the
result through the proposed versioned chain commitment.

## Exact construction and codecs

The parent issue specifies every domain and field order. The code follows those
bytes explicitly; it uses no bincode/JSON for consensus preimages. `preimage`
returns the noncircular base transcript so the committed fixture can be reviewed.
`derive` appends each counter from 0 through 255 and selects the first nonzero
delta with nonzero accumulated scalar and nonidentity target. The digest hook
that forces rare branches is private and only reachable by module unit tests.

The original derivation index and cumulative scalar stay separate from the
immediately winning outpoint. A child uses `Pchild=Psource+tG`,
`Tchild=Tsource+t mod l`, and inherits the original index. `recover_with_context`
subtracts `TG`, delegates original classical/hybrid recovery to the existing
wallet derivation, then requires `xG=P` before returning the final key. It does
not return ownership based on a callback's assertion alone.

`Record` encodes the proposed payout fields with canonical points/scalars,
explicit KEM/context tags, exact 1088-byte optional ML-KEM ciphertext and no
trailing bytes. `validate` additionally checks it against supplied source data
and the complete derivation domain. Four awards is an explicit candidate codec bound matching
`LotteryDrawConfig::default().winners_per_draw`; current configuration exposes
that parameter and does not establish a hard consensus maximum of four. Future
V2 consensus integration must ratify or revise this bound explicitly. The
prototype does not alter live lottery configuration.

`manifest`, `payout_root`, `summary_root` and `body_root` are inactive codecs.
They do not replace live `header.tx_root`. In particular, a root computed from
untrusted RPC data is not an authenticated header proof.

## Tests and vector provenance

- Unit tests force zero-delta, zero-accumulator, identity-target and exhausted
  retry paths; check the first valid counter, scalar-order arithmetic, malformed
  encodings, ordered fields, domain/record/summary/body mutations and failed
  ownership recovery.
- `tests/lottery_v2.rs` signs and verifies actual CLSAG transactions for a source,
  repeated awards and two nested levels, both classical and hybrid at original
  index 3. All five outputs have separate accepted signatures and key images.
  A test spent-image set accepts each once and rejects replay. Wrong keys,
  substituted ring members, altered amounts and contexts are rejected. This is
  library-level signature evidence, not production-ledger activation evidence.
- `tests/fixtures/lottery-v2.json` commits byte-level native/WASM expectations,
  including absent/present KEM records, a repeated award and nested contexts.
  `lottery_vector_dump` regenerates a candidate fixture for explicit review;
  generated output must not be accepted merely to make a test green.
- `check-lottery-vector-hashes.py` independently reconstructs the main transcript
  and checks SHA-256/SHA-512, LE fields and scalar reduction/addition with Python
  integers. It does not claim to independently implement Ristretto or CLSAG.
- The detached `lottery_v2_wasm` example is a test-only cdylib, not a wallet API.
  Node instantiates its real WASM bytecode and compares the same fixture. Its
  transitive wasm-bindgen metadata imports trap if invoked; no host cryptography,
  randomness or mock computation supplies results. Vector execution calls none
  of those imports. Actual hybrid recovery/signing is covered by the native
  CLSAG test; the portable vector fixture tests opaque KEM-envelope encoding
  and payout arithmetic, not ML-KEM encapsulation randomness.
- `botho/tests/lottery_v1_hash_golden.rs` calls the actual current kernel's
  `BlockHeader::hash` and `Block::hash`. Source provenance is `e243d10d`.
  The explicit V1 fields (version 1, parent bytes 2, tx-root bytes 3, LE u64
  fields 4/5/6/7, view bytes 8 and spend bytes 9) independently hash with Python
  SHA-256 to `edaa253a91ac3ec3e3f6657a427e05ef8e336e4330b62074edd7cdc5a05c08bb`.
  Changing body data still leaves that V1 header hash unchanged; the test freezes
  existing behavior and does not endorse it as payout authentication.

## Reproduction

```sh
cargo test --locked -p bth-transaction-clsag --features pq
cargo clippy --locked -p bth-transaction-clsag --features pq --all-targets -- -D warnings
cargo test --locked -p botho --test lottery_v1_hash_golden
python3 transaction/clsag/tests/scripts/check-lottery-vector-hashes.py
cargo build --locked -p bth-transaction-clsag --example lottery_v2_wasm --target wasm32-unknown-unknown
node transaction/clsag/tests/scripts/run-lottery-wasm.mjs target/wasm32-unknown-unknown/debug/examples/lottery_v2_wasm.wasm
```

The dedicated Linux workflow executes these scoped tests/runtime vectors without
building the bridge or desktop workspace. Local validation uses the pinned
nightly on macOS and Node 26; CI uses Node 22. Final-head CI must pass before
claiming Linux validation.

## Parent gates remain open

Related-key review of the actual CLSAG transcript, integrated producer/validator
rules, all header/membership-proof consumers, authenticated RPC context,
versioned storage/snapshot migration, every wallet client and real-ledger
independent spends remain #1286 work. Legacy payouts share keys; this prototype
does not repair or credit them. An explicit clean-genesis or reviewed legacy
claim disposition remains necessary before launch. No live reset, activation,
fund movement or external audit engagement is performed here.
