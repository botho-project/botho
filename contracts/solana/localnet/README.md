# Squads vault CPI execution harness (ADR 0012 Tier 2)

Run from `contracts/solana` with Node 22, Solana **1.18.26** and Anchor **0.29.0**
on PATH:

```sh
npm ci
anchor build
npm run test:squads
```

The driver verifies the committed Squads artifact and IDL hashes, starts a
fresh validator bound to loopback, loads the two real programs, runs the
assertions and stops its validator. Ports 18899–18925 must be available. It
uses deterministic **public test keys** and genesis SOL; never use these
keys or the genesis configuration for anything outside a disposable local
ledger. Your configured Solana wallet and cluster are not used.

A successful run prints `PASS` and writes execution receipts with transaction
signatures, slots, error codes/logs, program/source/IDL/vector hashes and final
SPL supply/recipient balance. Set `SQUADS_EVIDENCE_PATH` to retain the JSON at a
chosen path. Logs and the genesis fixture remain in the printed temporary
directory on failure as well as success. Missing programs, node failures,
wrong error codes or unmet assertions fail the command; there is no skip path.

The vectors are **actual Rust assembly output**, not a parallel TypeScript
implementation. The normal bridge unit test recomputes them and compares
every byte and account flag. The local harness submits those vectors and
cross-checks the pinned official SDK against the actual Squads transaction
account, plus the generated wbth IDL for the inner instruction. To deliberately
regenerate after a reviewed assembler change, from the repository root:

```sh
UPDATE_SQUADS_LOCALNET_VECTORS=1 cargo test -p bth-bridge-service --lib test_localnet_execution_vectors
```

Then rerun this real execution harness. Both Bridge CI and Solana Contracts CI
run when the shared vectors change. The Squads test has a separate ledger
from `anchor test`, since wbth's bridge PDA is a singleton.

Assertions cover immutable 2-of-3 setup, nonmember rejection, one approval
being insufficient, duplicate approval rejection, two distinct approvals,
substituted execution-account rejection, real vault-signed bridge mint,
vault-funded marker rent, same-proposal replay rejection, fresh-proposal
order replay rejection, incorrect mint authority rejection, and unchanged
supply/recipient balances after every rejected transaction. All failures are
submitted to the validator with preflight disabled and checked from confirmed
transaction receipts.

See [artifact provenance](../fixtures/squads-v4/PROVENANCE.md). This proves the
programs and assembler work together on localnet. SolMinter state progression,
configuration, durable order/proposal mapping, multi-machine approvals and
production custody readiness are outside this harness (#1268).
