# Bridge Testnet End-to-End Runbook (#828)

The full round-trip drill for the BTH ↔ wBTH bridge on live test
infrastructure: **BTH deposit → wBTH mint → hold → wBTH burn → BTH
release**, with the exact assertions each leg must satisfy.

Status: **local full loop automated; live-testnet drill manual**. The
Ethereum leg runs fully automated (see
[Layer 0](#layer-0-hermetic-ethereum-leg-automated)); the BTH live-node
transports (#856 — deposit scan + release construction/submission/
confirmation) are implemented and unit-tested, with an `#[ignore]`d
live-node integration test (see [Layer 1.5](#layer-15-bth-node-leg-live)).
Now that #856 is closed, the **orchestrated full loop** (wrap → mint → burn
→ release, driven through the real engine with a local Botho node AND a
local Hardhat node) runs as an `#[ignore]`d harness (see
[Layer 1.75](#layer-175-orchestrated-full-loop-local-automated), #993). The
federation envelope transport is #858, Solana is #857. The gated CI job
(`.github/workflows/bridge-e2e.yml`) automates Layer 0 and the local full
loop; the **live-Sepolia** round trip below stays a manual drill until
funded Safes + Sepolia ETH land (#866/#868) — the
[automation criteria](#automation-criteria) below define "done" for that.

## Test layers

| Layer | What runs | Where | Cadence |
|---|---|---|---|
| 0 | Ethereum leg, real Rust pipeline, local chain | `scripts/bridge-e2e-local.sh` | nightly CI + on demand |
| 0.5 | Ethereum leg, real Rust pipeline, **Sepolia fork** (no secrets/funds) | `scripts/bridge-e2e-fork.sh` | on-demand CI (`workflow_dispatch`) |
| 0.75 | **Full DeFi round trip** (mint→wrap→fund→pool→swap→repatriate) through the real engine + real Uniswap v3, **Sepolia fork** + local Botho node (no secrets/funds) | `scripts/bridge-e2e-defi-fork.sh` | on-demand CI (`workflow_dispatch`) |
| 1.75 | Orchestrated full loop through the real engine, local Botho + Hardhat nodes | `scripts/bridge-e2e-full-loop.sh` | nightly CI + on demand |
| 1 | wBTH contract on Sepolia (mint through the real Safe) | this runbook, steps 1–5 | before any bridge deploy |
| 2 | Full BTH testnet round trip (live Sepolia) | this runbook, all steps | blocked on #866/#868 |

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

## Layer 0.75: full DeFi round trip (Sepolia fork, #1005)

**The mainnet liquidity-launch rehearsal.** This is the headline
demonstration AND the exact sequence the team runs at mainnet to bootstrap
wBTH liquidity on a DEX — first against a Sepolia fork (here, zero creds),
then live testnet, then mainnet, by swapping the RPC endpoint and a funded
key (see the [flip table](#fork--testnet--mainnet-flip-the-launch-runbook)).

It joins two already-landed pieces into ONE continuous journey of a coin,
driven end to end through the REAL bridge engine
(`OrderProcessor::process_pending_orders`) and the REAL Uniswap v3 periphery
inherited from forked Sepolia state:

```bash
./scripts/bridge-e2e-defi-fork.sh https://ethereum-sepolia-rpc.publicnode.com
# or:  SEPOLIA_RPC_URL=... ./scripts/bridge-e2e-defi-fork.sh
```

The driver compiles the contracts, starts `anvil --fork-url <rpc>` (chain id
11155111) and a `botho-testnet` node, mines a reserve warmup, then runs the
`#[ignore]`d test `bridge/service/src/defi_round_trip_tests.rs`, which walks
the six steps:

1. **Mint BTH** on the local Botho node (a funded factor-1 reserve).
2. **Wrap → wBTH** — the engine drives the mint order to `Completed` via
   t-of-n EIP-712 federation attestation → `Safe.execTransaction(bridgeMint)`
   (reusing the Layer 1.75 wrap leg). The token's ONLY `MINTER` is the Safe,
   so **every wBTH in the demo is a wrapped coin**.
3. **Fund gas** on the fork via `*_setBalance` and wrap ETH into WETH (the
   faucet + WETH stand-ins — no real funds).
4. **Seed the pool** — create the wBTH/WETH Uniswap v3 pool and add two-sided
   liquidity, the wBTH side drawn from the wrap (reusing the #1004 harness
   helpers `create_pool_and_add_liquidity`).
5. **Purchase** — swap WETH → wBTH against the seeded pool (the market buys
   wBTH; `swap_weth_for_wbth`).
6. **Repatriate** — `bridgeBurn` exactly the swap proceeds and let the engine
   drive the burn order to `Released` via t-of-n Ed25519 attestation →
   `BthReleaser` reserve spend to a fresh stealth output the user scans back
   (reusing the Layer 1.75 unwrap leg).

So a coin genuinely travels **Botho BTH → wBTH → into a DEX pool → bought via
a swap → back to native BTH.** The test enforces as hard failures:

1. **Peg on wrap** — `wbth.balanceOf(user) == totalSupply() == BTH locked`
   (ADR 0003 factor-1, exact); the reconciler reports `drift == 0` after the
   mint.
2. **Pool + swap** — `factory.getPool(...)` is non-zero, the position has
   `liquidity > 0`, and the swap moved WETH → wBTH in the right direction.
3. **Provenance of the repatriated coin** — the burn amount **equals the swap
   output**, and the released BTH equals that amount (net of fees, zero here),
   paid to a fresh one-time stealth output (ADR 0004) the user's OWN view key
   scans back, on a tx unlinkable to the EVM burn.
4. **Proof-of-reserves across the whole loop** — `drift == 0` at the start
   (`0/0`), after the mint (`WRAP/WRAP`), and after the partial repatriation
   (`WRAP−swapOut / WRAP−swapOut`): only the backing for the repatriated coins
   is unlocked; the rest stays locked behind the wBTH still circulating in the
   pool.

The test **self-skips** (green — never a false pass) unless BOTH a Sepolia
fork is reachable (the Uniswap periphery only exists on a fork/live chain)
AND the BTH reserve/user wallet key material is provided (32-byte hex files
via the env vars in the script header). Without the reserve keys the BTH legs
skip, exactly like Layers 1.5/1.75 — pending the reserve key provisioning
(#999).

CI wiring: the `defi-round-trip` job in `.github/workflows/bridge-e2e.yml`
runs this on `workflow_dispatch` only (kept off the nightly schedule for the
same public-RPC rate-limit reason as Layer 0.5), reads the `SEPOLIA_RPC_URL`
repo secret, and skips cleanly when it is absent.

### Fork → testnet → mainnet flip (the launch runbook)

The SAME test + driver seed a live pool by swapping endpoints only — no
test-logic change. This is the literal mainnet liquidity-bootstrap procedure:

| Setting | Fork (Layer 0.75, now) | Live testnet (Phase B) | Mainnet launch |
|---|---|---|---|
| `BRIDGE_FORK_RPC_URL` | local `anvil --fork-url <sepolia>` | live Sepolia RPC | mainnet RPC |
| `BRIDGE_FORK_EXPECTED_CHAIN_ID` | `11155111` | `11155111` | `1` |
| `BRIDGE_UNISWAP_*` / `BRIDGE_WETH_ADDRESS` | Sepolia defaults | Sepolia defaults | mainnet addresses |
| `BRIDGE_WBTH_ADDRESS` | throwaway deploy | #866-deployed token | mainnet token |
| `BRIDGE_FORK_FUND_ACCOUNTS` | `1` (`*_setBalance`) | **unset** (real gas) | **unset** (real gas) |
| LP / relayer key | dev key | funded testnet key | funded mainnet key |
| BTH reserve | local `botho-testnet` reserve | live testnet reserve | mainnet reserve |

Phase B (#866/#868/#869) supplies the live deploy, funded keys, and a
persistent pool; the Solana venue is #867/#870. This layer is the fork
rehearsal + the harness those reuse verbatim.

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

## Layer 1.75: orchestrated full loop (local, automated)

The single **wrap → mint-wBTH → burn → release-BTH** round trip, driven end
to end through the REAL bridge engine (`OrderProcessor::process_pending_orders`)
with BOTH chains live and zero external creds — the compelling "demonstrate
wrapped BTH" artifact (#993). Unlike Layer 0 (Ethereum leg only, synthetic
order) and Layer 1.5 (BTH transport only), this boots a local Botho node AND
a local Hardhat node and lets the engine walk a real mint order
`DepositConfirmed → … → Completed` and a real burn order
`BurnConfirmed → … → Released`:

```bash
./scripts/bridge-e2e-full-loop.sh
```

The driver compiles the contracts, starts a Hardhat node (chain id 31337)
and a `botho-testnet` node, mines a reserve warmup, then runs the
`#[ignore]`d test `bridge/service/src/e2e_full_loop_tests.rs`. That test
wires ONE `BridgeConfig` across both chains, builds the production
`EthMinter` + `BthReleaser` + `FederationAttestationProvider`, and enforces
four properties as hard failures:

1. **Peg intake** — `wbth.balanceOf(user)` and `totalSupply()` equal the
   order's net amount, which equals the reserve ledger's locked backing
   (ADR 0003 factor-1, exact).
2. **Correct release to a fresh stealth output** — the live `BthReleaser`
   pays `net_amount` to a one-time output (ADR 0004) that the user's OWN
   view key scans back off the live node, distinct from the EVM burn tx.
3. **Proof-of-reserves invariant** — the `Reconciler` reports `drift == 0`
   (`Σ wBTH == locked reserve`) after the mint and again after the release,
   with the live BTH node consulted for the custody leg, returning to `0/0`
   once unwrapped.
4. **Federation authorization** — a single Safe-owner signature does NOT
   mint (the engine leaves the order `DepositConfirmed`) and a single
   Ed25519 signature does NOT release (left `BurnConfirmed`); both cross the
   configured 2-of-2 threshold before any value moves.

Provision the BTH reserve/user wallet key material (32-byte hex files) via
the environment variables documented in the script header. Without them the
test **self-skips** (green — never a false pass, the same discipline as
Layer 1.5). The Ethereum half swaps `local → Sepolia-fork → live Sepolia`
purely by pointing `BRIDGE_FORK_RPC_URL` at a fork/live RPC (+ a funded
relayer key for live) with no test-logic change (companion #992/#866).

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

The **local** full loop is already automated — `bridge-e2e.yml`'s
`full-loop` job boots both nodes and runs Layer 1.75 (#993). The remaining
manual drill is the **live-Sepolia** round trip; `bridge-e2e.yml`'s
`testnet-round-trip` job replaces it when ALL hold:

1. ~~#856 (BTH deposit scan + release transports) merged and validated.~~
   Done — exercised by Layers 1.5 and 1.75.
2. #866/#868: `WrappedBTH` + Safes deployed to live Sepolia, and testnet
   secrets provisioned as repo/environment secrets — reserve view/spend
   keys, relayer key, federation attestation keys (testnet-only material —
   the beta testnet is disposable).
3. The drill scripted end to end with the assertions above as hard
   failures, including the ADR 0004 stealth re-shield check via view-key
   scan. The local `full-loop` job (Layer 1.75) already enforces exactly
   these assertions; the live variant swaps in the Sepolia RPC + funded
   relayer key (no test-logic change).
