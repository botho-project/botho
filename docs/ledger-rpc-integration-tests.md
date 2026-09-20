# Ledger and RPC integration execution

The workspace PR workflow executes these six targets after its shared test
compilation. They use temporary LMDB ledgers and ephemeral loopback servers;
no public node, faucet, credentials or live funds are required.

```sh
cargo test --locked -p botho \
  --test chain_sync_catchup_integration \
  --test issue_998_fresh_genesis_liveness \
  --test ledger_consistency_integration \
  --test tx_lifecycle_integration \
  --test rpc_integration \
  --test e2e_faucet_workflow
```

## Measured results

On macOS with pinned `nightly-2025-12-03`, all **111 tests passed**, with zero
ignored or filtered tests, on 2026-09-20. Each compiled test binary ran
separately with a three-minute execution limit. Times below are test-harness
times, excluding compilation. The shared compile used a populated node cache
and took 18.57 seconds; that is not a clean-build benchmark.

| Target | Tests | Seconds | Coverage |
| --- | ---: | ---: | --- |
| `chain_sync_catchup_integration` | 10 | 47.46 | Production sync state machine, simulated peer messages and real ledger application |
| `issue_998_fresh_genesis_liveness` | 1 | 3.45 | Successive hybrid coinbases after fresh genesis |
| `ledger_consistency_integration` | 26 | 25.73 | Persistence, indices, concurrency and invalid-block rejection |
| `tx_lifecycle_integration` | 14 | 27.92 | UTXO consumption, fees, mempool and loopback transaction submission |
| `rpc_integration` | 46 | 5.33 | HTTP/WebSocket behavior against local servers |
| `e2e_faucet_workflow` | 14 | 48.67 | Funded local ledgers, faucet requests, rate limits and transaction status |

The executing CI step has a 15-minute limit, allowing for slower Linux
runners and Cargo feature-resolution rebuilds after workspace compilation.
Linux CI must validate the final commit; these local measurements do not
establish its runtime or reliability. Investigate timeout or timing failures
instead of weakening assertions or silently dropping targets.

This change adds execution coverage for existing assertions. It changes no
production code or test expectations and does not prove full consensus
liveness, public-network interoperability, mainnet operations or bridge
custody. Consensus/E2E execution policy remains tracked in
[#1274](https://github.com/botho-project/botho/issues/1274); this work closes
the specific ledger/RPC coverage gap in
[#1273](https://github.com/botho-project/botho/issues/1273).
