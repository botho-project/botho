# Bridge Testnet End-to-End Runbook (#828)

The full round-trip drill for the BTH ↔ wBTH bridge on live test
infrastructure: **BTH deposit → wBTH mint → hold → wBTH burn → BTH
release**, with the exact assertions each leg must satisfy.

Status: **manual drill**. The Ethereum leg runs fully automated today (see
[Layer 0](#layer-0-hermetic-ethereum-leg-automated)); the BTH live-node
transports (#856 — deposit scan + release construction/submission/
confirmation) are now implemented and unit-tested, with an `#[ignore]`d
live-node integration test (see [Layer 1.5](#layer-15-bth-node-leg-live)).
The federation envelope transport is #858, Solana is #857. The gated CI job
(`.github/workflows/bridge-e2e.yml`) automates Layer 0 nightly and will
absorb the full drill once a testnet node + secrets are wired — the
[automation criteria](#automation-criteria) below define "done" for that.

## Test layers

| Layer | What runs | Where | Cadence |
|---|---|---|---|
| 0 | Ethereum leg, real Rust pipeline, local chain | `scripts/bridge-e2e-local.sh` | nightly CI + on demand |
| 0.5 | Ethereum leg, real Rust pipeline, **Sepolia fork** (no secrets/funds) | `scripts/bridge-e2e-fork.sh` | on-demand CI (`workflow_dispatch`) |
| 1 | wBTH contract on Sepolia (mint through the real Safe) | this runbook, steps 1–5 | before any bridge deploy |
| 2 | Full BTH testnet round trip | this runbook, all steps | blocked on #856 |

## Layer 0: hermetic Ethereum leg (automated)

No testnet or secrets needed:

```bash
./scripts/bridge-e2e-local.sh
```

This deploys `WrappedBTH` plus a Gnosis-Safe-compatible threshold multisig
(`contracts/ethereum/contracts/test/SafeStub.sol`) to a local Hardhat node
and drives the production Rust pipeline end to end
(`bridge/service/src/fork_tests.rs`): 2-of-2 federation attestation over
the EIP-712 SafeTx digest → `Safe.execTransaction(bridgeMint)` submitted by
a role-less relayer EOA → confirmation polling with the order-bound
`BridgeMint` event check → idempotent re-broadcast → user `bridgeBurn` →
watcher burn scan. It asserts exact factor-1 picocredit amounts (ADR 0003)
and that `totalSupply` returns to zero.

## Layer 0.5: Sepolia-fork Ethereum leg (no secrets, #992)

The **same** Rust pipeline as Layer 0, but against a local node that FORKS
real Sepolia state (chain id 11155111) over a public RPC — the closest-to-
real-testnet demonstration achievable with **no funded account, no deployed
contract, and no secret**. A throwaway `WrappedBTH` + `SafeStub` is freshly
deployed onto the forked state, and the four dev accounts are funded on the
fork via `anvil_setBalance` / `hardhat_setBalance` (test ETH — no real funds):

```bash
./scripts/bridge-e2e-fork.sh https://sepolia.example/v2/<key>
# or:  SEPOLIA_RPC_URL=... ./scripts/bridge-e2e-fork.sh
```

The driver starts `anvil --fork-url <rpc>` (or `npx hardhat node --fork`),
waits for RPC, and runs the `#[ignore]`d `fork_` test with two env knobs:

- `BRIDGE_FORK_EXPECTED_CHAIN_ID=11155111` — pins the fork's chain id (the
  test reads the chain id from the node; when this is unset it accepts
  whatever the node reports, which is why the Layer 0 local run — chain id
  31337 — needs no code change).
- `BRIDGE_FORK_FUND_ACCOUNTS=1` — mints test ETH to the dev accounts on the
  fork before deploying. A no-op on the local 31337 path (already funded).

CI wiring: the `ethereum-leg-sepolia-fork` job in
`.github/workflows/bridge-e2e.yml` runs this on `workflow_dispatch` only
(kept off the nightly schedule to avoid public-RPC rate-limit flakiness —
promote to nightly once a stable archive RPC is provisioned). It reads a
`SEPOLIA_RPC_URL` repo secret and skips cleanly when the secret is absent.

**Flip to live Sepolia (#866):** this is the same parametrization the live
deploy reuses — point `BRIDGE_FORK_RPC_URL` at a live Sepolia RPC (no fork),
set `BRIDGE_FORK_EXPECTED_CHAIN_ID=11155111`, leave `BRIDGE_FORK_FUND_ACCOUNTS`
**unset** (there is no `setBalance` on a real chain), and supply a genuinely
funded relayer/owner key in place of the dev keys. No test code changes —
#866 is a config swap, not a rewrite.

## Layer 1.5: BTH node leg (live)

The BTH-node transports (#856) — the deposit view-key scan
(`NodeBthClient`) and the release construction / submission / confirmation
(`BthReleaser`) — carry an `#[ignore]`d integration test that runs the REAL
Rust transport against a live node over JSON-RPC (mirroring
`fork_tests.rs`):

```bash
# 1. Run a local BTH testnet node with JSON-RPC exposed and a funded,
#    factor-1 reserve wallet (view/spend keys written as hex files).
# 2. Point the test at it and run:
BRIDGE_BTH_RPC_URL=http://127.0.0.1:7101 \
BRIDGE_BTH_RESERVE_VIEW_KEY=/path/to/view.hex \
BRIDGE_BTH_RESERVE_SPEND_KEY=/path/to/spend.hex \
  cargo test -p bth-bridge-service -- --ignored bth_node_
```

The test (`bridge/service/src/bth_fork_tests.rs`) drives a real deposit
scan (`NodeBthClient::block_at` → view-key match → factor-1 gate) and, when
the reserve holds spendable factor-1 outputs, a real release
(`BthReleaser::prepare_release` → `tx_submit` → confirmation poll), then
scans the paid output back with the recipient view key to prove ADR 0004
(fresh one-time stealth) and ADR 0003 (change back to the reserve). Without
the environment variables the test **skips** (never a false pass — it never
claims a live path it could not exercise).

The pure transport-parsing and tx-construction logic (`bth_rpc`,
`bth_scan`, `bth_keys`, and the releaser stages above the socket) is fully
covered by native unit tests that run in every `cargo test` pass.

## Prerequisites (Layers 1–2)

- **BTH testnet**: a synced node with JSON-RPC + websocket exposed; the
  bridge reserve wallet funded with **factor-1 (zero-demurrage) coins
  only** (ADR 0003 — the watcher rejects non-factor-1 deposits).
- **Ethereum Sepolia**: `WrappedBTH` deployed via
  `contracts/ethereum/scripts/deploy.ts` with `MINTER_ROLE` held by a real
  Gnosis Safe (t-of-n = the drill federation, ADR 0002; addresses recorded
  in `contracts/ethereum/README.md`), plus a funded relayer EOA (gas only —
  it must hold **no** roles).
- **Bridge service**: `bth-bridge` configured with the federation
  (`ethereum.mint_signers`/`mint_threshold`, `bth.release_signers`/
  `release_threshold`), `bridge.testnet = true`, and the reserve
  reconciler API enabled (`reserve.api_listen`).
- A test user wallet on each side.

## The drill

Record every value in the drill log (amounts in **picocredits**, all tx
hashes, order UUIDs and their derived 32-byte order ids).

### Leg A: BTH → wBTH (mint)

1. Create a mint order via the bridge API; note `order_id`, the deposit
   address, and the memo.
2. Send the BTH deposit (e.g. 100 BTH = `100_000_000_000_000` pc) from the
   user wallet with the order memo (the destination memo carries the order
   UUID). The live deposit scan (`NodeBthClient`, #856) view-key-matches the
   output to the reserve and reads the memo to bind it to the order.
3. Watch the order walk `AwaitingDeposit → DepositDetected →
   DepositConfirmed` (SCP finality) `→ MintPending → Completed`.
4. **Assert (amounts, ADR 0003):** wBTH minted to the user =
   `amount - fee` exactly, 1 base unit == 1 picocredit, no scaling.
   Cross-check `balanceOf` on Sepolia against the order's `net_amount`.
5. **Assert (custody, ADR 0002):** the mint tx is
   `Safe.execTransaction` from the relayer EOA; the `BridgeMint` event
   carries this order's derived 32-byte id; `processedOrders[orderId]` is
   true (replaying the authorization cannot mint twice).

### Hold

6. Wait ≥ 2 reconciler passes, then `GET /api/reserve/proof`:
   locked reserve == wBTH outstanding (exact, tolerance 0), drift alarm
   quiet, `reserve_balance_checked` true once #853 lands.

### Leg B: wBTH → BTH (burn + release)

7. From the user's Ethereum wallet call
   `bridgeBurn(net_amount, <bth destination>)`; note the `BridgeBurn` tx.
8. Watch the order walk `BurnDetected → BurnConfirmed` (depth +
   canonical-hash check) `→ ReleasePending → Released`. The live release
   transport (`BthReleaser`, #856) builds a CLSAG-signed reserve spend,
   submits it via `tx_submit`, and polls `getTransaction` for the
   configured depth (`release_confirmations_required`; 0 = SCP finality).
9. **Assert (amounts):** BTH released = burned amount − bridge fee,
   exactly; the reserve change output returns to the reserve address with
   factor-1 provenance intact (ADR 0003).
10. **Assert (privacy, ADR 0004):** the release pays a **fresh one-time
    stealth address** — the on-chain BTH output is NOT linkable to the
    burn-declared destination string by an outside observer; scan with the
    user's view key to confirm receipt.

### Wrap-up

11. **Assert (terminal states):** the mint order is `Completed`, the burn
    order is `Released`; both are terminal (no further transitions
    accepted); no orders stuck in actionable states; audit log shows the
    full transition history.
12. Re-check `/api/reserve/proof`: invariant restored
    (`locked == outstanding`, both reduced by the released amount).
13. File the drill log on the tracking issue (#816).

## Failure handling

- An order stuck in `DepositConfirmed`/`BurnConfirmed` with
  "attestation threshold not met" means the federation could not reach
  threshold — expected with a single signer until #858 (transport) lands;
  the state is retryable by design (fail-safe, no funds moved).
- A mint tx that executed without the order's `BridgeMint` event is
  surfaced as `Failed` (Safe swallowed an inner revert) — operator
  attention required; do NOT resubmit blindly (rate limits).
- The circuit breaker (`bridge.paused`, auto-trip on backlog) halts
  submit stages only; confirmations keep settling. See
  `docs/bridge/` operations notes and the engine runbook from #854.

## Automation criteria

`bridge-e2e.yml`'s `testnet-round-trip` job replaces this manual drill
when ALL hold:

1. #856 (BTH deposit scan + release transports) merged and validated.
2. Testnet secrets provisioned as repo/environment secrets: reserve
   view/spend keys, relayer key, federation attestation keys (testnet-only
   material — the beta testnet is disposable).
3. The drill scripted end to end with the assertions above as hard
   failures, including the ADR 0004 stealth re-shield check via view-key
   scan.
