# Native stress wallet adapter

Tracker: #1402. Controller: #1404. Baseline node source: `6dbcb92463214694f3122a05c9c48986d4f4f1b7`.

`cargo build --locked -p botho-wallet --bin botho-stress-wallet` builds a Unix/PQ-only JSON adapter. Send one tagged request on stdin; read one JSON response on stdout. Operations are `generate`, `scan`, `prepare`, `inspect`, and `restore_check`. There is no networking or broadcast operation. The separate controller owns the pinned TLS transport, spent-state freshness, rate limits and reservations. This adapter must not be treated as an independent network verification oracle.

Wallets are dedicated testnet mnemonics in owner-only regular files, inside owner-only directories. Paths, public addresses and transaction metadata may be supplied in requests; never put phrases in argv, environment or logs. Prepared artifacts use exclusive creation, file/directory fsync and an atomic hard-link publication. Existing or partial artifacts prevent replacement. The controller must reconcile an artifact before making another signature for its intent.

`prepare` rescans supplied block outputs with the production scanner, verifies selected ownership/key images against the controller's spent response, preserves the ten-block age floor and production age band/ring selection, uses the production fee estimator and hybrid CLSAG builder, and checks exact output/fee accounting. The controller must fetch complete, pinned output ranges and current spent state. The adapter filters the inclusive age window locally; the existing RPC ring wrapper requests one extra end block even though the server's range is inclusive.

The result contains the canonical transaction hash, actual fee, selected input IDs/key images, output values/keys, shape, byte count and protected artifact path. It does not include the mnemonic. The total campaign fee/input budgets are enforced by the controller before any submission.

## Acceptance

`cargo test --locked -p botho-wallet --test stress_node` runs a bounded loopback fixture using the production RPC, mempool and ledger with trivial PoW. It funds through the faucet, scans receipt, prepares without submitting, explicitly submits saved bytes, compares the node's canonical hash, mines/validates the transaction, verifies spent state and exact balances, restores into a separate profile, then spends again. Full and incremental scans agree. This does not claim public consensus, web/Snap, confidential-amount or load acceptance.

Local acceptance on September 22, 2026: passed, two native transfers and exact accounting, 63.34 seconds. Artifact/permission tests are under `stress::tests`. The Linux artifact workflow repeats both gates before publishing an artifact; it does not deploy or release node binaries.

An initial coinbase-funded fixture exposed a separate RPC/scanner inconsistency: `chain_getOutputs` labels a coinbase with output index `u32::MAX`, while the hybrid coinbase derivation binds index zero. The final acceptance fixture uses the same ordinary faucet-funded output path intended for the public campaign. This adapter does not repair or mask the coinbase problem.
