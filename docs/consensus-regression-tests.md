# Consensus and transfer regression execution

`e2e-tests.yml` executes four explicit targets on matching Rust/Cargo/toolchain
pull requests, nightly, and manual `regression` or `all` dispatches:

```sh
cargo test --locked -p botho \
  --test consensus_cluster_convergence \
  --test e2e_transfer_patterns \
  --test chaos_tests \
  --test load_tests \
  -- --test-threads=1
```

This selects twelve ordinary tests. The eight ignored chaos/load tests remain
ignored here and retain their separate manual opt-in jobs. Selecting
`regression` with `include_ignored=false` still executes both ordinary baselines.
Other timing, Byzantine, network, consensus and fee suites retain their existing
manual selectors; adding a schedule does not silently enable them.

The job pins `nightly-2025-12-03`, compiles the four targets together with a
30-minute limit, and gives execution a separate 15-minute limit. This isolates
execution failures from cold-build costs without compiling the desktop workspace.
Its cache is shared across these four targets. Original 30-second per-block
no-stall deadlines and all existing test assertions remain unchanged. The
previous `E2E_TIMEOUT_SECS` setting did not control these tests and was removed.
The result summary fails when a prerequisite fails or is cancelled; intentional
selector skips are allowed.

## What the tests establish

Convergence runs 2-, 3- and 4-node clusters through real SCP and production
Ledger/BlockBuilder operations, asserting no forks, advancing height and one
coinbase per block. Transfers cover concurrent payments, consolidation,
splitting, sequential spending, mixed batches and load, using real CLSAG
signatures. The ordinary chaos and load baselines assert normal consensus and
matching chain heights. Transports are in-process and ledgers temporary; no
public node, credentials, faucet or funds are involved.

## Failure exposed by executing the suite

The first transfer execution failed all six tests: mined wallets appeared empty,
which also caused a later unsigned subtraction to overflow. The shared test
scanner and three transaction builders still used classical-only ownership and
spend-key recovery, whereas current minting creates hybrid ML-KEM outputs.
The harness now uses the production wallet's unified scan/recovery APIs with
the creating output index for coinbases and ordinary transaction outputs. This
preserves classical change-output compatibility and spent-key-image filtering.
No production cryptography, balance assertions, fees or deadlines changed.
Lottery payout derivation is outside this bounded correction; these short
transfer scenarios do not establish mature hybrid lottery payout spendability.

## Evidence and limits

Local results use the pinned nightly on macOS. The shared compile took 20.40
seconds with a populated compatible cache (not a cold-build benchmark).

| Target | Passed | Ignored | Test-harness seconds |
| --- | ---: | ---: | ---: |
| `consensus_cluster_convergence` | 4 | 0 | 18.92 |
| `chaos_tests` | 1 | 4 | 3.09 |
| `load_tests` | 1 | 4 | 3.57 |
| `e2e_transfer_patterns` after the harness correction | 6 | 0 | 96.83 |

Linux CI on the final published
head must pass before this is treated as verified Linux coverage. A green local
run is not proof of public-network liveness, mainnet readiness or an external
security audit. Investigate timeout or consensus failures rather than extending
deadlines or dropping assertions.
