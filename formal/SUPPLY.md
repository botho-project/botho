# Block supply accounting (#1839)

`Botho/Supply/Accounting.lean` proves the scalar accounting step using the
existing lottery models. It does **not** close #1839 or prove that every
accepted Rust block satisfies the step's premises. Source references below
were checked at `2c8d5cd2`.

## Fee truncation result

**Proven (model):** for every fee and every configuration,

```text
pool + burn = max(fees, pool)
```

`splitFees_total` includes the `as u64` truncation. The burn share is
recomputed as `fees.saturating_sub(pool)` after that cast. Thus truncation
cannot silently destroy any of the original fees. An invalid fraction can
instead create value: `oversized_fraction_creates_value` checks that one
unit of fees with `pool_fraction_permille = 2000` produces a two-unit pool.
This is a configuration counterexample, not an accepted-block exploit.

**Proven (model):** `splitFees_exact`, `splitFees_conserves` and
`splitFees_rounding` show that for a `u64` fee and a fraction at most 1000,
the cast is lossless, pool plus burn equals the fee, and the rounding
remainder goes to burn. The difference from the exact rational burn is
nonnegative and strictly less than one unit. The consensus constructor
`botho/src/ledger/lottery_v2.rs::canonical_config` fixes the fraction at 800.
The original DRT's truncating cases use fractions above 1000; they are
evidence about the helper's unrestricted domain, not silent consensus burns.

## Accounting identity

Let `U` be economic unspent value, `P` lottery carryover, `B` cumulative
fee burn, and `M` cumulative minted reward. The invariant is
`U + P + B = M`. In a block let `I` be spent value, `O` ordinary outputs,
`F` fees, `R` reward, `E` lottery emission and `D` actual distribution:

```text
I = O + F
U' = U - I + O + (R - E) + D
P' = P + E + fee_pool - D
B' = B + fee_burn
M' = M + R
```

`apply_conserves` proves the invariant after this step. It calls the actual
existing Lean models, including their saturating arithmetic; it does not
replace them with an idealized pool addition. `pool_inflow_fits` derives
that the additions cannot saturate from the pre-state invariant, spending
bound and total issuance budget. `counters_fit` bounds both cumulative Rust
`u128` counters. `issuance_step_fits` preserves `M ≤ height * u64::MAX` and
derives the next `u128` budget for a checked height successor and `u64`
reward. Genesis has zero reward in `Block::genesis_for_network`.

`arbitrary_pool_saturates` records the negative result outside that budget:
an arbitrary pool of `u128::MAX` loses a one-unit emission to saturation.
It is not a reachable-state counterexample. Reachability cannot be omitted
when stating conservation of `compute_pool_accounting`.

Demurrage, settlement and downgrade charges are components of the fee floor
in `store/validation.rs::consensus_fee_floor` and its settlement checks.
They are already included in `F`; subtracting them as separate burns would
double count them. `store/writer.rs::block_effects` records only the validated
lottery fee-burn share in the cumulative burn counter.

## Premises still to connect to the ledger

| Premise | Rust source / remaining obligation |
|---|---|
| `I = O + F` | `Transactions.batch_balance` derives exact Nat aggregation from the checked arithmetic model. `checked_ordinary_flow` connects those amounts to available stored sources under the named binding predicates below. Production correspondence remains trusted/tested, not proved. |
| `I ≤ U` | `Transactions.checked_keys_sources` constructs distinct prestate consumption under `CLSAGBinding`, `KeyAssociation` and `UnspentCorrespondence`; `consume_partition` derives its value bound. This is a finite model theorem under explicit cryptographic/ledger assumptions. |
| `F ≤ u64::MAX` | `Transactions.checked_sum_exact` and `batch_balance` prove the existing `sumFeesChecked` fold returns the exact Nat fee sum within u64 on success. Rust/model correspondence remains a separate obligation. |
| `E ≤ R` | `cluster-tax/src/monetary.rs::lottery_emission_bps` caps the rate at 5000/10000; the exact schedule/model bridge remains #1710. |
| `D ≤ available`, payout outputs sum to `D` | Connect `Draw`, V2 payout validation and no-winner behavior; #1849 covers the lottery conservation proofs. |
| Genesis and issuance induction | Bind the zero genesis state, checked successor and reward width to the state transition. `issuance_step_fits` supplies the arithmetic induction step. |
| The stored transition equals `apply` | Audit/DRT of the complete accepted-block writer, including transaction atomicity and identity/collision assumptions, remains open. |

**Trusted (ledger/cryptography):** these premises must be named when citing
`apply_conserves` as evidence about Rust. In particular, `U` is not the sum
of records in `utxo_db`: the database retains spent outputs as ring decoys,
and spending records key images. A raw database sum is not a supply measure.
Confidential amounts (#904) would replace the public-amount balance bridge
with a commitment-balance argument; the scalar identity alone cannot do so.

## Checked ordinary transactions and finite sources (#1894)

**Proven (model):** `Botho/Supply/Transactions.lean` supplies the transaction
and finite-source premises for the later block-composition layer. All amounts
and totals are naturals. Only the individual input/output/final-fee checks and
the separate aggregate fee fold constrain a result to `u64::MAX`; global input
and output totals can exceed it. No new fee-split or lottery theorem is assumed.

The source mapping was rechecked at
`7e3905550ca65f89af8377255b5ae6d9fc6a3beb`:

| Model / public result | Production source and precise boundary |
|---|---|
| `checkTransaction`, `transaction_balance` | `transaction/clsag/src/lib.rs::verify_ring_signatures`: checked input fold, checked output fold, separate output-plus-fee checked addition, then equality. Signature validation is outside this arithmetic model. |
| `checkBatch`, `checked_sum_exact`, `batch_balance` | `botho/src/ledger/lottery_v2.rs::drawing`: `try_fold(0u64, checked_add)` over fees. The reused `Lottery.sumFeesChecked` nil case returns its accumulator unchanged, so the generalized theorem requires an initially bounded accumulator. |
| `lookup`, `consume`, `consume_partition` | `store/validation.rs::verify_transaction` and `verify_ring_members` read one pinned pre-block snapshot. The finite economic map is a semantic projection; Rust retains spent rows and records keys instead of deleting real-input rows. |
| `checkKeys`, `checked_keys_sources` | `store/writer.rs::record_key_image` rejects keys already in the incremental writer transaction. The batch model checks absence from old history and pairwise distinct new keys. Its sound source-selection bridge additionally requires the three predicates below. |
| `insertFresh`, `insertion_preserves` | `store/writer.rs::Representation::check_new` rejects IDs in either output or context storage. The model checks the complete retained domain, including spent decoys, then extends that domain for each insertion. Its correspondence to both tables needs `ContextDomainAgreement` below. |
| `ordinaryStep`, `ordinary_step_preserves`, `checked_ordinary_flow` | A checked ghost projection from prestate plus transaction/source witnesses. Sources are consumed before any ordinary output is inserted, so an output created in the block cannot fund another transaction in that block. This is not a replacement Rust validator. |
| `ordinary_step_correspondence` | Preserves the semantic key/history invariant under the explicit `IdentityExtension` obligation for new identities; it proves no cryptographic key-generation property. |

`Input.source.id` is a supplied semantic real-source witness.
`Input.source.amount` is the authenticated pseudo amount used by the arithmetic
checks; it denotes the actual persisted amount only when `CLSAGBinding` or the
explicit checked source lookup establishes that equality. Identifiers and keys
are abstract naturals, without a claim about a wire encoding or hidden-input
recovery. `Store.unspent` is economic U, `Store.retained` includes spent decoys,
and `Store.history` is the explicitly known spent-key history. `WellFormed`
requires unique economic/stored identities and identical amounts for every
economic row in the retained store. Theorems carry this invariant explicitly;
the executable functions do not silently repair a malformed prestate.

**Trusted (cryptography/ledger), as separate theorem parameters:**

- `CLSAGBinding`: every authenticated claim identifies a retained source with
  exactly the claimed amount and its associated semantic key image. Resolving
  every public ring member and finding an amount match does not itself identify
  the hidden real input or prove this premise.
- `KeyAssociation`: one stable function maps retained output identities to
  semantic key images, injectively on that domain. The function is a logical
  parameter, not an implementation extracted from CLSAG or the public ring.
- `UnspentCorrespondence`: for each authenticated retained row, its amount is
  present in economic U **if and only if** its associated key is absent from
  history. The reverse implication is essential to fresh-key availability.
- `IdentityExtension`: the association remains injective on the enlarged stored
  domain, with each newly created identity's key absent from the resulting
  history. Fresh outpoint bytes alone do not establish this property.

`checked_keys_sources` derives distinct available pre-block sources from
successful key checks and the first three predicates, then constructs successful
checked removals. It does not assume the desired removals or a spent-value bound.
`consume_partition` proves consumed plus remaining value equals prevalue and
preserves every unselected identity/amount. `insertion_preserves` adds precisely
the new value, preserves unique IDs and all old retained/economic lookups, and
leaves history unchanged. `ordinary_step_preserves` extends history by exactly
the checked new keys and preserves every untouched old economic row.
`checked_ordinary_flow` composes these facts from prestate checks alone, deriving
`I = O + F`, `I ≤ U`, the fee bound and the finite ordinary-output update. No Rust
poststate is an input or expected answer. `ordinary_step_correspondence` also
preserves the named unspent/history relation under `IdentityExtension`.

**Separate production obligations:** `ContextDomainAgreement` says the abstract
retained identity domain covers the output and context tables with consistent
amounts; `StoredContextFrame` says successful commits preserve every old stored
row and derivation context and create exactly the validated outputs;
`CommitCorrespondence` connects a successful real commit to the selected model
checks and economic update. These are named external obligations, not Lean
axioms or consequences of the arithmetic. The #1893 oracle below tests complete
old-row/context frames, exact new output sets and its fixture's complete **known**
key history, plus independently known real-input amounts. Those bounded
observations do not prove CLSAG binding, arbitrary hidden history completeness,
or universal production/model equivalence.

The five kernel-checked boundary claims cover exact-u64 success, two individually
valid arithmetic transactions whose aggregate input/output totals exceed u64,
empty folds (including an out-of-range starting accumulator), each of the three
arithmetic overflows, equality failure, aggregate-fee overflow, duplicate/missing
sources, amount mismatch, duplicate/already-spent keys, duplicate created IDs,
retained spent-decoy reuse, and an attempted same-block created-output spend.
They include a valid finite update with untouched rows/history and a fresh-key
countermodel without the correspondence premise. Zero-fee arithmetic examples
are intentional; none asserts signatures, fee-floor compliance, full consensus
validity, or a reachable exploit. They use kernel `decide`, not native execution.

The exact public declaration records land separately before the proof source.
This layer is **proven (model) under named assumptions**; no production DRT leg,
random campaign, extraction result or whole-block theorem is added. #1895 owns
emission, payout and genesis composition; #1896 owns model/production campaign
evidence. #1890 and #1839 retain those obligations. The existing oracle evidence
below remains independent and unchanged.

## Validation

The new theorems are listed in `Theorems.lean` and checked by the existing
sorry gate, axiom gate and independent kernel replay. Their statement
records must land before the proof PR, as required by the base-owned record
workflow. No additional axiom or trusted executable proof tactic is used.

The existing DRT legs `split_fees`, `compute_pool_accounting` and
`carryover_after` test the Rust primitives reused by this proof. Those legs
do not test the new aggregate `apply` projection or establish the open
ledger premises above. Whole-block DRT remains part of #1839.

At source `8085ad6a`, seed 1839 passed 100,000 cases per primitive plus two
saved regressions, with zero mismatches. The
[receipt](../formal-drt/results/supply-1839.json) pins the source, toolchains,
input-file hashes and branch coverage: 8,355 truncating fee casts, 16,344
pool-saturation cases, and 29,150 carryover underflows. These unrestricted
helper inputs do not establish that such states are reachable in consensus.

## Actual accepted-block oracle (#1889)

`botho::ledger::formal_drt_supply` is available only with `test-fixtures`.
The isolated `formal-drt` crate exercises it with:

```sh
cd formal-drt
cargo test --release supply_oracle -- --nocapture
```

This oracle observes real `Ledger::seal_block` / `Ledger::add_block`
commits, including the production signature and checked-balance callback,
key-image collision checks, V2 context writes and LMDB transaction commit.
A separate read-only invocation of the production pre-writer V2 checks
identifies the rejection stage; it does not replace validation in
`add_block`. Database, encoding and other local errors fail the fixture;
only `LedgerError::is_block_fault()` outcomes count as consensus rejection.

The bounded fixture starts at fresh genesis and changes the stored
difficulty to `u64::MAX`, matching the existing activation-test profile.
Every header still passes actual PoW verification. Lottery maturity remains
720 blocks, and candidate, fee and transaction validation rules are
unchanged. This is a **genesis-origin chain under an altered test profile**,
not canonical production-schedule reachability. It uses neither seeded
snapshots nor the drawing double. Arbitrary storage fixtures and
premise-satisfying scalar examples remain different evidence domains.

Each observed case records the full signed candidate block, pre/post chain
state and pool, every persisted output and derivation context, and watched
key-image state. Newly persisted amounts are partitioned into coinbase,
ordinary and lottery outputs. The independent ghost map contains the real
source outpoints known to the fixture signer. It consumes each accepted
source once and adds actual persisted outputs; the raw row sum is never
called economic unspent value. The checked comparison is:

```text
new_output_value + pool_after + burn_delta
  = known_real_spent_value + reward + pool_before
```

This cross-addition form permits a decreasing pool. Demurrage and settlement
charges remain components of fees and are not subtracted again. The
accounting comparison does not prove source ownership, general output
spendability, CLSAG soundness or all accepted-block supply invariants.

The profile covers zero fees with no candidates and carryover, three fresh
signed transfers, real V2 payouts at maturity, a subsequently signed payout
spend, replay, two independently signed transactions spending one source,
and a signed-output mutation. The latter two are sealed with valid V2 body
roots, pass the pre-writer check and fail in the real transaction/writer
path. A payout-binding mutation is an earlier-stage control. Rejections
compare complete observation fingerprints before/after and after closing
and reopening LMDB. Accepted observations are also checked after reopen.
An executable negative control changes observed `pool_after` by one,
requires the comparison to fail, restores it, and requires success.

The [oracle receipt](../formal-drt/results/accepted-block-oracle-1839.json)
records measured runs. `SUPPLY_ORACLE_CASE` logs separate signing, sealing,
precheck, actual add-block and observation costs. The receipt separates
wallet initialization and cold chain construction from those steady-case
costs. `SUPPLY_ORACLE_DIR` selects the retained evidence directory: it holds
the entire signed chain and per-case observations/known inputs, with hashes
in `receipt.json`. These are replay inputs, not cached acceptance counts.
CI uploads them with the unit-test log for 14 days.

The existing release unit-test step runs the new tests without a filter,
with a 420-second command limit and eight-minute step limit. The profile
also checks a 360-second execution bound. The separate five-minute random
DRT tier is unchanged. There is no new random leg or nightly campaign in
this stage: #1890 must use the measured real-verification costs to choose
its fixture reuse and account for the existing million-case nightly budget.
No Lean theorem or statement record is added here; the parent #1839 and
its model-composition child #1890 remain separate work.

The first optimized Linux CI measurement at PR head `87bc3798` passed all
three unfiltered unit tests (two oracle tests) in 18.99 seconds. Its oracle
profile committed 743 blocks, with seven observed accepted scenarios, four
intentional consensus rejections and zero unexpected infrastructure errors.
Cold chain construction took 17.90 seconds; the three fresh ordinary
signed commits took 6.06–6.24 ms each, with 6.57–7.03 ms signing costs.
The mature payout spend took 7.97 ms in `add_block`, plus 7.90 ms signing,
3.28 ms precheck and 4.85 ms observation. Reopening, artifact writing and
setup are additional costs, not hidden in the commit timing.

The nightly workers run four **different legs**. Each leg's million cases
in 3600 seconds allow **3.6 ms per case**, including startup and every
per-case cost. The earlier 14.4 ms figure assumed four workers dedicated to
one leg and does not describe this workflow. Even the original partial
ordinary-case measurements exceed 3.6 ms. This is a feasibility gap, not a
proof that every faithful design is impossible.

## Accepted frame and saved replay (#1893)

`formal_drt_supply::check_accepted_frame(observation, history)` validates
unique IDs before constructing maps. It retains every old full output row
and derivation context, derives the exact candidate-created ID set, and
requires both the after snapshot and the redundant `new_outputs` vector to
match it, including kind and ordinal. Coinbase and ordinary rows use their
production conversion/index rules. Payout checking uses the pre-state
winner, the public V2 manifest/domain/derivation contract, the inherited
envelope/tags, and the **original** base index and cumulative tweak. It does
not substitute a zero index/tweak for a lottery source.

`KeyHistory` is the fixture's complete known prefix: every key from accepted
blocks and attempted candidates, including previously spent markers and
keys still absent after rejection. Before observations must match that
history exactly; after observations must retain old markers and add each
accepted input at the candidate height. Missing/duplicate observations
fail. This does not enumerate unknown key images in the database. The
fixture supplies known real outpoints separately: CLSAG real-input binding
remains a named trust premise, never an inference from ring members.

`Ghost::check/apply` runs the frame check before accounting; `apply` commits
its economic map only after every check passes. Executable controls mutate
observations (not stored ledgers) to test old-row amounts/contexts/removal,
missing/extra/duplicate new rows, inconsistencies in `new_outputs`, key
history, and payout tweaks. Each must fail for the expected reason, retain
the ghost map, and pass after restoration. These are checker controls, not
claims that production accepted corrupt blocks. Secondary indexes, other
tables' accepted transitions, winner-selection correctness and general
cryptographic soundness remain outside this frame comparison.

The callable test-only contract in `formal-drt/src/supply.rs` is
`Saved::load(directory, expected_manifest_sha256)`, `Saved::reset`, and
`execute(saved, reset, candidate)`. Saving drops **all** fixture ledger
handles, then copies the closed `data.mdb` into a fresh directory using
independent files. Its byte hash covers the entire persisted store: every
table, full block history, timestamp window, difficulty and snapshot-base
metadata, plus LMDB's own data-file metadata. `lock.mdb` is process
synchronization and is deliberately recreated. Unexpected directory
entries fail. This is a full store copy, not the narrower production
snapshot import and never the unvalidated fixture seeder.

Every reset copies that closed image again, checks its bytes, opens a new
ledger, checks that opening did not change the image, checks the runtime
network, and compares the observed snapshot. The saved identity also binds
the economic known-source map and the complete watched history. The
manifest pins schema, profile, selected production/checker source bytes,
fixture/replay source bytes, toolchain and lockfile. Its expected SHA-256
comes from the receipt. Missing/unreadable files, mismatched hashes, schemas
or profiles fail; executable corruption/restoration controls exercise the
loader. The observed-snapshot hash alone is never called a full-store hash.

The default bounded test saves four prefixes: ordinary signed spending at
short history, late signature rejection at short history, a mature payout,
and a mature payout spend. Each gets three exact signed-byte replays and
three freshly signed candidates, alternating from independently reset
pre-states. A different fresh candidate therefore intervenes between saved
replays. Every sample performs actual precheck and `add_block`; no cached
verdict counts. The 720 maturity, real PoW and protocol-8 checks remain.

To replay an artifact from a receipt with the **matching source checkout**:

```sh
cd formal-drt
SUPPLY_ORACLE_REPLAY_DIR=/path/to/artifact/replay-signed_transfer_0 \
SUPPLY_ORACLE_REPLAY_SHA256=<manifest_sha256-from-receipt> \
cargo test --release supply_oracle_real_commits_rollback_and_reopen -- --nocapture
```

This loads the retained store, signed candidate, real-input map and watched
history and executes production validation/commit again. It does not
regenerate a chain from a seed. The output explicitly marks reused signed
bytes and zero fresh signatures. Reusing a store requires quiescent handles
and an isolated destination; the source image is checked unchanged afterward.

`frame-replay-receipt.json` retains every measured sample, min/median/max
and totals, pre-state/manifest/image/artifact hashes, history/row counts and
storage sizes. Timings separate prefix capture, wallet setup, artifact
load/validation, reset/open/pre-state check, fresh signing, template/PoW/
seal, precheck, actual commit/rejection, observation, frame/accounting,
serialization/write, cleanup and remaining overhead. End-to-end totals are
measured independently; the disjoint per-case parts sum to those totals.
Saved replays have zero signing/sealing cost and are not newly signed
samples. Full fixture wall time includes its control checks and prefix
construction; it is not added again to per-case timings.

The existing 360-second fixture guard and 420-second unfiltered release CI
step remain. Optimized Linux results must be tied to the exact PR head and
retained artifact hashes in the review receipt. Lean codec/model execution
is **unmeasured additional cost**, as are the rest of the shared PR tier and
large-campaign history growth. No leg is registered here. #1896 still owes
faithful complete-budget evidence before registration; #1890 and #1839 keep
their remaining obligations. A bounded receipt does not establish a 10k or
million-case campaign.

The [bounded local receipt](../formal-drt/results/frame-replay-1893.json)
records 2 passing focused tests in 45.04 seconds, the original 743 committed
blocks/7 accepted/4 rejected scenarios, 33 frame corruption/restoration
checks, and 24 additional actual attempts (18 accepted, 6 late rejections;
12 fresh signatures and 12 saved-byte replays). Artifact-loading CLI runs
also reproduced an accepted commit and a late rejection in fresh processes.
The receipt pins final implementation source hashes; Linux exact-head
measurements remain a separate review requirement.

On this Darwin arm64 machine, median complete repeated-case times were:

| Pre-state | Rows / closed image bytes | Saved bytes | Fresh signature |
| --- | ---: | ---: | ---: |
| Ordinary, height 64 | 64 / 737,280 | 30.420 ms | 119.709 ms |
| Late rejection, height 68 | 71 / 917,504 | 91.822 ms | 79.053 ms |
| Mature payout, height 720 | 723 / 4,014,080 | 128.769 ms | 138.142 ms |
| Payout spend, height 742 | 776 / 4,308,992 | 141.502 ms | 155.692 ms |

Each cell has only three samples; the receipt includes extrema and totals
without discarding outliers. Both prefix size and reset/observation costs
grow with retained history. These measurements leave no 3.6 ms margin for
the forthcoming Lean work and establish no Linux throughput guarantee.
