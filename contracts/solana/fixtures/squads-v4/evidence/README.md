# Executed local result, 2026-09-20

`localnet-2026-09-20.json` contains 19 confirmed local-validator transaction
receipts: 12 successful transactions and 7 expected rejections. It records
all logs, slots, signatures, genesis hash, runtime version and SHA-256 hashes
of the programs, source, fixture vectors, harness and dependency lockfile.
The final SPL supply and recipient balance are both **5,000,000,000,000** raw
picocredits, produced by exactly one successful Squads → wbth mint CPI.

Executed with Solana 1.18.26, Anchor 0.29.0 and Node v26.9.0 on an ARM Mac.
The programs were built/loaded with the same toolchain pins as CI; CI uses
Node 22. No live-chain transactions were submitted. All signatures refer to
the discarded local genesis and are not public-cluster explorer links.

Commands and other checks:

- `anchor build`: succeeded, committed Cargo.lock unchanged.
- `anchor test --skip-build --provider.wallet <ephemeral-test-wallet>`:
  **14 passing** existing wbth tests.
- `npm run test:squads`: **PASS** with the receipts recorded here.
- `cargo test -p bth-bridge-service --lib`: **277 passed, 11 ignored**;
  ignored tests require other chains/infrastructure and are unrelated to this
  non-skipping local harness. Includes all **19 Squads assembly tests**.
- `cargo clippy -p bth-bridge-service --lib --tests -- -D warnings`: passed.
- Harness TypeScript check, Prettier, rustfmt and shell syntax: passed.
- A corrupted temporary copy of the Squads binary was rejected by the hash
  verifier before starting execution.

The harness exposed two bugs in the former construction-only assembler:
`proposal_approve` requires a writable member (the fee payer masked this for
the first member), and outer execute must preserve non-vault inner signer
requirements. Official SDK comparisons failed before both corrections;
the dedicated Rust non-vault-signer regression also failed before its fix.
These receipt hashes identify the corrected source and executed vectors.

The protected Squads program-config initializer is not exercised: matching
config state is supplied at local genesis, with owner/discriminator/fields
asserted before multisig creation. This record proves Tier-2 interoperability,
not deferred bridge-engine integration or production readiness.

## Engine integration result (#1268)

`engine-2026-09-20.json` adds a separate fresh-ledger execution through actual
Rust `OrderProcessor`/`SolMinter`, real federation attestations and independent
file-backed member databases. The final run passed in 71.06 seconds and
preserves 13 finalized successful transaction logs plus 17 exact
source/program/IDL/lockfile hashes. Negative vote/execute submissions
are checked against real RPC preflight rejection; metadata/packet-loss fault
injection is distinguished from successful on-chain execution in the harness.

The final supply and recipient balance remain 5,000,000,000,000. Only the first
order completes. Rejected, competing and ambiguous orders retain backing. The
record covers paused quorum/restart, actual expired blockhash refresh, accepted
send with dropped response, shared-index contention, real stale Approved
execution, strict custody rejection and read-only recovery with config drift.
Late discovery without the original verified binding deliberately remains a
recovery hold. See the [engine runbook](../../../../../docs/bridge/solana-squads-engine.md)
for limits and the explicit command. The original Tier-2 record above is kept
as historical evidence; its source hashes identify that earlier revision.
