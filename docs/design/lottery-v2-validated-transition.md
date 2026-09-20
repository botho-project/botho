# Inactive LotteryV2 accepted local transitions

Part of #1286; implementation checkpoint #1354, originally based on #1353 at
`85a57e19426fcac24298b84faf80c6013bb83742`. This joins actual production validation
and writer functions to an inactive V2 producer on a fresh local heed chain.
Only tests can open the private validated handle. No ordinary Ledger, network,
wallet, migration or reset entrypoint selects these rules.

## Trust and representation

The `validated.1` schema is separate from the `storage.2` unvalidated fixture
schema. Opening either as the other fails. Fresh initialization commits all nine
tables, complete metadata, a deterministic local genesis, rule identity and
checkpoint together. The local genesis reuses the existing testnet genesis
structure and the proposed V2 body commitment; it has no allocations or lottery
reserve. Its isolated easy-PoW target is explicitly pinned to `u64::MAX`, as in
existing positive ledger tests. This is not a proposed mainnet genesis.

Strict reads require exact metadata widths, schema/rules/genesis identity,
checkpoint/tip agreement, and the tip's recomputed body commitment. Source reads
check the existing immediate-edge context/reference invariants and the creating
body commitment. A positive floored tag contribution requires its materialized
wealth row; zero/round-to-zero tags can legitimately have no row. These checks
use accepted local database provenance; they do not authenticate arbitrary RPC
contexts or replay the entire ancestral history on each read.

The private disk envelope remains a local representation, not a wire format.
A peer header version cannot select this path. The experimental four-award bound
remains a candidate bound, not a ratified consensus maximum. Lottery thresholds
come from the actual default draw configuration (720-block maturity); environment
shortcuts are not read by this boundary.

## Shared functions and V1 equivalence

| Parent operation | Current implementation |
|---|---|
| `Ledger::add_block_inner` ordinary gates | `ValidationReads::validate_ordinary_block`, called by V1 with `RootRule::Legacy`; original order/error wrappers preserved |
| Ring, tag, fee floor, import floor, settlement and signature methods | Original bodies in `store/validation.rs`; V1 primitive getters retain their original independent reads and error behavior |
| Candidate selection | One shared capped, rotated two-range algorithm, original eligibility/factor math and wealth cache |
| V1 candidate row policy | Iterator/decode failures still skipped; creation and wealth errors still propagate |
| Experimental candidate row policy | Iteration/decode/key/context/commitment errors abort; no alternative draw from skipped bad rows |
| Actual block effects | Existing `WriteTables::block_effects`, unchanged by this child |
| Per-transaction signature callback | Same position after block/coinbase writes; V1 reads original committed state, experimental reads pinned pre-state |
| Within-block key-image collisions | Same shared writer checks the write transaction |
| V1 hashing, serialization and initialization | Unchanged; frozen eight-table golden retained |

The ring-centroid arithmetic still divides each tag contribution before summing;
candidate wealth still divides after summing. This extraction does not change
CT policy, the age quantile, rounding, fees or lottery selection.

`BlockBuilder::apply_lottery_v2` uses the persisted strict view, actual cap,
accounting and draw helpers, then the existing Award/Domain/derive/Record
primitives. Missing sources are errors. It constructs ordered payouts and the
minting/ordinary/payout/summary body root. Its result is untrusted until accepted.

The independent validator recomputes candidates, checked fees, accounting, draw,
all summary fields (including empty draws), manifest, domains and Record binding.
It recomputes the body root and retains ordinary height/parent/minting/header,
difficulty/real PoW/reward/time, staleness, ring/tag/fee/settlement checks. Signing
and recovery remain the existing positive CLSAG library operations.

Application acquires the writer lock **before** opening its read-only pre-state.
That view supplies all validation and the per-transaction signature callback.
The same write transaction commits envelope, outputs, contexts, every ordinary
index, wealth/import effects, pool/burn/mined/tip, optional emission and checkpoint.
No validated token escapes that state scope. Stale producer output is rejected.

Optional emission updates retain V1's trusted local caller contract: supplied
counters are committed atomically, not independently authenticated by the writer.
`None` preserves counters. The local acceptance fixture uses that contract and
does not claim an integrated live emission-controller or difficulty epoch run.

## Required positive evidence and its limits

The test builds 721 valid mint-only blocks from the empty local genesis, retaining
real PoW/reward/time checks and canonical maturity. Bootstrap lottery emission is
zero, so the reserve remains unfunded until an ordinary fee-paying transfer.
The existing `tx_lifecycle_integration` signing/recovery implementation is shared
through `tests/common/positive_transfer.rs`; its original integration wrapper
still obtains decoys from Ledger. The new fixture supplies actual persisted,
distinct outputs to the same 20-member positive operation.

The funded transition must produce a nonempty award list, pass independent
validation and the shared signature callback, and survive reopen. Assertions
cover exact fee/burn/reserve/distribution conservation, accumulated accepted
rewards, output index 1 context, transaction index, spent key image, payout
addresses, source/context references, and wealth recomputed from stored outputs.
It reuses the pre-funded chain to abort at every actual write with optional
emission, comparing all nine tables and reopening after each abort before the
final commit. The ordinary valid transfer does not populate bridge-import rows;
#1352's separate all-nine-table storage fixture remains that effect's rollback
coverage.

Cheap local controls exercise schema separation, corrupted state identities,
creating commitments, missing context/positive wealth, stale production, empty
summaries and V1-root mismatch. These are local representation/validation tests,
not signed malicious transactions or network reproductions.

Measured host results and raw execution evidence are recorded alongside the
implementation. Compilation and execution are separate in Linux CI; the actual
lib-test binary runs **all** `ledger::` tests with a 600-second execution deadline
and 12-minute step budget. Compile JSON and logs, exact checkout/toolchain/host
and source hashes are retained even after failure. Local workstation timing is
not a Linux throughput, network latency or energy measurement.

## Remaining integration

This demonstrates accepted local experimental payouts, not independent wallet
discovery or accepted payout spending. Repeated/nested derivation primitives and
storage regressions remain separate evidence. Public envelope/version selection,
full/compact/snapshot/import/membership-proof handling, clients and trust models,
legacy disposition and activation remain outstanding. #1308 codecs are outside
this change. No legacy credits, migration, activation, live reset or audit
engagement is performed; #1286 remains open.

## Execution evidence and gate disposition (2026-09-20)

[Raw ledger run](../../botho/tests/fixtures/lottery-v2-validation/local-ledger.txt),
[metrics and source identities](../../botho/tests/fixtures/lottery-v2-validation/local-evidence.json),
and [diagnostic excerpts](../../botho/tests/fixtures/lottery-v2-validation/diagnostic-excerpts.txt)
retain both success and failure. The original complete parallel ledger run was **92 passed,
1 failed**, in 258.43 seconds. The funded test passed, including 33 actual write
aborts/reopens: 721 maturity blocks, two payouts, and exactly 1 BTH fee = 0.2 BTH
burn + 0.5 BTH distribution + 0.3 BTH reserve.

One existing test failed at fresh `Ledger::open` with OS `EINVAL`, before its fee
calculation. A bounded current-code follow-up cohort reproduced raw `EINVAL` in
two existing fixture open/persist operations. The isolated original test passed;
three unchanged-parent cohorts passed; later controlled cohorts and a bounded
8-worker/400-cycle open/read/close/reopen diagnostic also passed. Those passing
repetitions did not establish a baseline issue or resolve the cause. The original
failure log and comparison hashes above remain unchanged.

Subsequent tracing attributed two parallel-suite failures to LMDB's System V
`semop` at first-reader allocation, rather than proving invalid semaphore IDs.
A separate direct project diagnostic then opened ten real Ledger environments,
held their ten write transactions, and observed EINVAL when normally opening an
eleventh on a host with `kern.sysv.semume=10`. This demonstrates a resource
boundary; no kernel undo-entry count was captured at the historical failures,
so it does not prove each failure's precise cause.

Merged [#1362](https://github.com/botho-project/botho/pull/1362) provides a scoped
macOS ledger-test launcher. It reads the kernel limit and budgets two entries per
active ledger test plus two entries of headroom, capped at four workers. It adds
no skip filters or retries; ordinary ignored-test semantics still apply. The
production System V backend remains unchanged: the POSIX alternative was rejected
because it loses automatic stale-writer recovery. The normal Node opens one shared
Ledger environment, contributing two semaphore numbers; this is not a global
bound on custom embeddings or other semaphore users.

The exact implementation at `5127d4c2cc59e9f5b842517717a95566dadebfd2` now has both
complete execution results, including canonical maturity and all funded rollback
assertions:

| Platform and execution policy | Complete ledger result | Raw evidence |
|---|---|---|
| macOS, merged launcher, four workers, no interposer/debugger | **93 passed, 0 failed**, 271.24 seconds | [macOS log](../../botho/tests/fixtures/lottery-v2-validation/macos-bounded-ledger.txt) |
| Linux, existing default parallel execution | **93 passed, 0 failed**, 388.10 seconds | [Linux log](../../botho/tests/fixtures/lottery-v2-validation/linux-ledger.txt) |

The macOS run used the preserved exact-source binary (SHA256 `e599a15bdc9ea32ccf89169d762265092316fe1efc7df9daa0f54581c9dcb5f9`)
and launcher SHA256 `e8e491103bb7881469f56fd7ee437485ed6c05beab608f83526d73376ad7bccc`.
Linux evidence is from [run 35543628003](https://github.com/botho-project/botho/actions/runs/35543628003),
artifact `10616326753`, synthetic merge checkout
`e751d6354aa48ef897a5b5d67c197c0cbaade74b`. Its recorded implementation hashes
match the tested PR source. Both execution deadlines were 600 seconds; neither
run weakened the funded acceptance assertions. Exact raw hashes, provenance and
limits are recorded in `local-evidence.json`.

This evidence addresses the execution gate under the documented test resource
policy while retaining the historical failures and diagnostic uncertainty. This
amendment changes no implementation. Independent Judge review and current-head CI
remain required; the PR stays draft until that review. No production retry,
locking change, activation or payout-spendability claim follows from these runs.
