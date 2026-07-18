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
| 0.75-Solana | **Solana DeFi round trip** (mint→wrap→seed→swap→burn→release) through the real bridge transports + real Orca Whirlpool; bridge legs on `solana-test-validator`, Orca legs on **live devnet** (operator, #1052/#868) | `scripts/bridge-e2e-defi-solana.sh` | construction-validated + on-demand operator run |
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

## Layer 0.75-Solana: Solana DeFi round trip (#1079)

**The Solana analog of Layer 0.75** — the same headline
**mint → wrap → seed → swap → burn → release** journey of a coin, for the
Solana venue, driven through the REAL bridge-service transports and the REAL
Orca Whirlpool:

```bash
./scripts/bridge-e2e-defi-solana.sh            # full driver
./scripts/bridge-e2e-defi-solana.sh --check    # hermetic self-check (no cluster)
```

The driver boots a local `botho-testnet` node (the release/reserve leg) and —
when `RUN_LOCAL_VALIDATOR=1` — a `solana-test-validator` with the wbth program
`--clone-upgradeable-program`d from devnet, then runs the `#[ignore]`d
transport drills in `bridge/service/src/solana_devnet_tests.rs`
(`solana_devnet_*`, including `solana_devnet_defi_round_trip_wrap_peg_burn`),
which walk the legs:

1. **Mint BTH** on the local Botho node (a funded factor-1 reserve).
2. **Wrap → wBTH** — the REAL Ed25519 t-of-n mint-submission transport
   (`bridge/service/src/mint/solana.rs`) assembles + signs the hardened
   `bridge_mint`; the #850 per-order marker PDA makes the on-chain mint
   **exactly-once**. The wBTH mint's only authority is the federation key, so
   **every wBTH is a wrapped coin — no shortcut mint**.
3. **Seed the pool** — drive `contracts/solana/scripts/devnet-orca-pool.ts`
   with the FRESHLY bridge-minted wBTH: set `BRIDGE_SOLANA_RECIPIENT` to the
   `solana-lp` pubkey so the wrapped coin lands in the LP's ATA and seeds the
   Orca position (the thread — not a throwaway mint).
4. **Purchase** — swap against the seeded pool
   (`contracts/solana/scripts/devnet-orca-swap.ts`).
5. **Repatriate** — the REAL Solana burn-watcher transport
   (`bridge/service/src/watchers/solana.rs`, `burns_from_logs` over
   `getSignaturesForAddress` → `getTransaction` logs) decodes the
   `BridgeBurnEvent`; the engine then releases native BTH to a fresh one-time
   stealth output (ADR 0004) the user's OWN view key scans back — the same
   `BthReleaser` leg as Layer 1.5/1.75.

The drill enforces as hard failures the **Solana-leg peg invariant**: a full
`reserve::Reconciler` pass reports `sol_supply` present and equal to the direct
`SolSupplySource` read (`Σ wBTH devnet supply` verified, #853); on a broadcast
run the supply delta equals the wrapped amount (**factor-1**, 12-decimal wBTH ==
picocredits, ADR 0003); and re-preparing the same order re-derives the same
marker PDA (**exactly-once**).

### Honest limitation / operator boundary

Unlike the Ethereum path, **Orca Whirlpools cannot be forked/cloned
hermetically** (cloning a full Orca deployment + config + tick arrays via
`--clone` is fragile — see the maintainer note on #865). So the **Orca
pool/swap legs (steps 3–4) can only be validated against LIVE devnet** (needs
devnet SOL + the deployed program `CZDnzeywrqEM…` / mint `F7Lsi…`), gated behind
`RUN_ORCA=1`, and a **federated** Solana mint additionally needs the Squads
multisig from **#1052**. This layer therefore ships a **construction-validated
driver**: the bridge-transport legs (steps 1–2, 5) run against a local validator
or self-skip green, and the final live-devnet Orca execution is the operator
step tracked by **#1052 / #868** — the accepted pattern for the Solana legs
(cf. `solana_devnet_tests.rs`). The transport drills **self-skip** (green — never
a false pass) unless `BRIDGE_SOLANA_RPC_URL` + `BRIDGE_SOLANA_PROGRAM` (and
`BRIDGE_SOLANA_KEYPAIR` to also assemble the mint) are set.

### Required accounts (reuse the #1008 Phase B provisioning)

| Key | Cluster | Role |
|---|---|---|
| `solana-lp` (`.secrets/bridge-testnet/solana-lp.json`) | devnet | LP wallet: seeds the Orca pool + swaps; the wBTH mint recipient |
| `solana-deployer` (`.secrets/bridge-testnet/solana-deployer.json`) | devnet | deploys/initializes the wbth program + mint authority |
| `BRIDGE_SOLANA_KEYPAIR` | validator/devnet | the federation mint-authority keypair (single-key on testnet; Squads on mainnet, #1052) |

### Fork → testnet → mainnet flip (Solana venue)

The SAME driver + drills flip by config only — no test-logic change:

| Setting | Local validator (now) | Live devnet (operator, #1052/#868) | Mainnet-beta launch |
|---|---|---|---|
| `BRIDGE_SOLANA_RPC_URL` | `http://127.0.0.1:8899` (`RUN_LOCAL_VALIDATOR=1`) | `https://api.devnet.solana.com` | mainnet-beta RPC |
| `BRIDGE_SOLANA_PROGRAM` / `_WBTH_MINT` | cloned from devnet | #867 program / #870 mint | mainnet program / mint |
| `RUN_ORCA` | **unset** (Orca can't be cloned) | `1` (live devnet Orca) | `1` (live mainnet Orca) |
| Mint authority | single-key `BRIDGE_SOLANA_KEYPAIR` | single-key or Squads (#1052) | **Squads multisig** (#1052) |
| BTH reserve | local `botho-testnet` reserve | live testnet reserve | mainnet reserve |

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

The BTH reserve/user wallet key material is **provisioned at runtime** by the
driver (#999): after the node is up it runs `botho-testnet gen-bridge-keys`,
which exports the node's own deterministic **pre-funded mining wallet** as the
reserve and mints a fresh random user wallet. Because the reserve *is* the
miner, it already owns spendable **factor-1** (zero-cluster-weight) outputs
from lottery emission (`LotteryOutput::to_tx_output(ClusterTagVector::empty())`,
ADR 0003) — exactly what the CLSAG release spends. Each wallet emits a
32-byte-hex classical view/spend pair plus a 64-byte-hex ML-KEM/ML-DSA BIP39
seed (`*_PQ_SEED`, issue #972), which the protocol-6.0.0 hybrid chain requires
for the wallet to detect outputs paid to it. **No private key is committed**:
the reserve derives from the harness's in-code disposable node mnemonic and the
user keys are random at runtime. Bring-your-own-reserve still works — set the
`BRIDGE_BTH_RESERVE_*` env vars and auto-provisioning is skipped; leave them
unset and provisioning disabled and the test **self-skips** (green — never a
false pass, the same discipline as Layer 1.5).

> **Gating prerequisite.** The driver boots a single-node `botho-testnet`
> (`start --nodes 1`). Until the harness can bring up a *healthy* single-node
> 6.0.0 testnet that externalizes blocks (the #998/#1000 wedge work — today the
> harness rejects `--nodes 1` outright), the job cannot reach the provisioning
> or test steps. The key-provisioning wiring above lands independently and runs
> the moment that prerequisite is met.

The Ethereum half swaps `local → Sepolia-fork → live Sepolia` purely by
pointing `BRIDGE_FORK_RPC_URL` at a fork/live RPC (+ a funded relayer key for
live) with no test-logic change (companion #992/#866).

## Phase B — account provisioning (#1008)

Before the live-Sepolia drill (Layers 1–2) and the DeFi launch flip
(Layer 0.75 → testnet column) you need funded testnet accounts. One command
generates every keypair into a **git-ignored** directory, prints only the
**public** addresses + a faucet checklist, and emits a role→address env file
the live-deploy harness consumes:

```bash
./scripts/bridge-testnet-accounts.sh
```

It generates:

| Role | Chain | Curve | Purpose |
|---|---|---|---|
| `deployer` | Sepolia | secp256k1 | signs the `WrappedBTH` deploy tx (holds NO roles, ADR 0002) |
| `lp` | Sepolia | secp256k1 | LP / relayer EOA for the DeFi round trip (Layer 0.75) |
| `safe-owner-1/2/3` | Sepolia | secp256k1 | owner EOAs of the Gnosis Safe(s) |
| `solana-deployer` | devnet | ed25519 | Solana program upgrade/authority key |
| `solana-lp` | devnet | ed25519 | Solana LP key |

**Secret discipline (the point of this script):**

- Private keys are written to `.secrets/bridge-testnet/` (`0600`, dir `0700`),
  which is covered by `.gitignore` (`.secrets/`). The script **refuses to run**
  if that directory is not git-ignored — it will not write key material into a
  tracked path.
- Only **public** addresses and file paths are printed. Private keys are
  **never** echoed to stdout and **never** committed.
- Re-running is **idempotent**: an existing key file is skipped ("exists,
  skipping"), never overwritten — so re-running after funding cannot nuke a
  funded account.
- **Testnet only.** The beta testnet is disposable; do not reuse these keys on
  mainnet.

Verify nothing leaked before proceeding:

```bash
git status --porcelain .secrets/                          # must print nothing
git check-ignore -v .secrets/bridge-testnet/eth-deployer.key   # must print a match
```

**Tooling.** The script prefers `cast wallet new` (foundry) for ETH and
`solana-keygen new` for Solana when installed, and otherwise falls back to
`openssl` + an embedded, test-vector-validated keccak-256 / base58 helper — so
it runs with only `bash`, `python3`, and `openssl` present. Install the native
tools for canonical keystores: foundry (`https://getfoundry.sh`), Solana CLI
(`https://docs.solana.com/cli/install-solana-cli-tools`).

**Faucets.** The script prints per-address instructions:

- **Sepolia ETH** — Alchemy faucet (`https://sepoliafaucet.com`, 0.5/day),
  Google Cloud Web3 faucet (0.05/day, no account gate), or the pk910 PoW faucet
  (`https://sepolia-faucet.pk910.de`, mine to earn, no cap). Suggested:
  deployer ~0.4, lp ~0.2, each Safe owner ~0.05 Sepolia ETH.
- **Solana devnet** — `solana airdrop 2 <pubkey> --url devnet` per address
  (2 SOL/request, rate-limited per day/IP).

**Emitted env → live deploy.** The script writes
`.secrets/bridge-testnet/addresses.env` (git-ignored; public addresses only)
and prints it:

```
BRIDGE_SEPOLIA_DEPLOYER=0x…
BRIDGE_SEPOLIA_LP=0x…
BRIDGE_SAFE_OWNER_1=0x…
BRIDGE_SAFE_OWNER_2=0x…
BRIDGE_SAFE_OWNER_3=0x…
BRIDGE_SOLANA_DEPLOYER=…
BRIDGE_SOLANA_LP=…
```

Mapping to `contracts/ethereum/scripts/deploy.ts` +
`contracts/ethereum/hardhat.config.ts`:

- `deploy.ts` signs with the deployer **private** key, which hardhat reads from
  the `PRIVATE_KEY` env var. Wire the two together and confirm they agree:
  ```bash
  export PRIVATE_KEY="$(cat .secrets/bridge-testnet/eth-deployer.key)"
  # the deploy log's "Deployer" line must equal BRIDGE_SEPOLIA_DEPLOYER
  ```
- The three `safe-owner-*` EOAs are the **owners** of the Gnosis Safe(s). Deploy
  the Safe(s) from those owners (Safe UI/SDK), then set the resulting Safe
  **contract** addresses as `WBTH_ADMIN_SAFE` / `WBTH_MINTER_SAFE` /
  `WBTH_PAUSER_SAFE` for `deploy.ts` (ADR 0002). This script provisions the
  owner EOAs, not the Safe addresses themselves.
- `BRIDGE_SEPOLIA_LP` is the LP / relayer EOA the Layer 0.75 DeFi round trip
  uses (its private key file feeds the harness's LP key env var).

## Phase B.1 — deploy the custody Safe + WrappedBTH (#1011)

The first live-deploy step. Custody is a SINGLE **2-of-3 Gnosis Safe** used for
all three WrappedBTH roles (admin/minter/pauser), owned by the three
`BRIDGE_SAFE_OWNER_{1,2,3}` EOAs from the account-provisioning step above
(maintainer-ratified). The deployer EOA only pays gas and receives NO roles
(ADR 0002).

`contracts/ethereum/hardhat.config.ts` now auto-loads a **git-ignored**
`contracts/ethereum/.env` (via `dotenv/config`), so `PRIVATE_KEY`,
`SEPOLIA_RPC_URL`, `ETHERSCAN_API_KEY`, `BRIDGE_SAFE_OWNER_*` and `WBTH_*_SAFE`
resolve without a manual `source`. Never print or commit that file.

The Safe is created against the **canonical Safe v1.3.0** deployment (identical
on mainnet and Sepolia, pinned from `safe-global/safe-deployments`, all verified
to have live bytecode on Sepolia): SafeProxyFactory
`0xa6B71E26C5e0845f74c812102Ca7114b6a896AB2`, GnosisSafe singleton
`0xd9Db270c1B5E3Bd161E8c8503c55cEABeE709552`, CompatibilityFallbackHandler
`0xf48f2B2d2a534e402487b3ee7C18c33Aec0Fe5e4`. `deploy-safe.ts` calls
`createProxyWithNonce(singleton, Safe.setup(owners, 2, 0x0, 0x, handler, 0x0, 0,
0x0), saltNonce)` and self-asserts `getOwners()`/`getThreshold()`.

### B.1.a — fork-test first (no real ETH, no secret)

Prove the whole bring-up against a Sepolia fork before spending testnet ETH.
Requires foundry (`anvil` + `cast`):

```bash
./scripts/deploy-safe-fork-test.sh https://ethereum-sepolia-rpc.publicnode.com
```

It forks Sepolia (`anvil --fork-url`), `anvil_setBalance`-funds a throwaway
deployer, deploys the 2-of-3 Safe, asserts owners + threshold, deploys
WrappedBTH with the Safe as admin/minter/pauser, and asserts the Safe holds all
three roles while the deployer holds none. Ends with `FORK TEST PASSED`. Uses
throwaway anvil dev accounts — no `.env`, no real key.

### B.1.b — live Sepolia run

Fill `contracts/ethereum/.env` (git-ignored) with the funded deployer key and
the three owner addresses, then:

```bash
cd contracts/ethereum
cat > .env <<'EOF'          # git-ignored — never commit
PRIVATE_KEY=0x<funded deployer private key>
SEPOLIA_RPC_URL=https://<your sepolia rpc>
ETHERSCAN_API_KEY=<etherscan key, for verify>
BRIDGE_SAFE_OWNER_1=0x<owner 1>
BRIDGE_SAFE_OWNER_2=0x<owner 2>
BRIDGE_SAFE_OWNER_3=0x<owner 3>
EOF

# 1. Deploy the 2-of-3 Safe (prints SAFE_ADDRESS=0x...):
npx hardhat run scripts/deploy-safe.ts --network sepolia

# 2. Point all three WrappedBTH roles at that one Safe and deploy the token:
echo "WBTH_ADMIN_SAFE=0x<safe address>"  >> .env
echo "WBTH_MINTER_SAFE=0x<safe address>" >> .env
echo "WBTH_PAUSER_SAFE=0x<safe address>" >> .env
npx hardhat run scripts/deploy.ts --network sepolia

# 3. Verify on Etherscan (constructor args = the Safe, three times):
npx hardhat verify --network sepolia 0x<wbth address> \
  0x<safe address> 0x<safe address> 0x<safe address>
```

Or run steps 1–2 in one shot (deploy-safe → wire roles → deploy WrappedBTH →
print addresses + Etherscan links):

```bash
cd contracts/ethereum
npx hardhat run scripts/deploy-all.ts --network sepolia
```

Record the resulting Safe + wBTH addresses and the threshold in the deployment
table in `contracts/ethereum/README.md`. Then the Layer 1–2 drill below can run
with `BRIDGE_WBTH_ADDRESS` = the deployed token and the Safe as its minter.

## Phase C — the live threshold-federation drill (#868)

Phase B deployed the live artifacts (Sepolia wBTH + 2-of-3 Safe, devnet wbth
program). Phase C stands up a REAL t-of-n federation — N independent
`bth-bridge` processes, each holding only ITS attestation keys, exchanging
envelopes over the authenticated `POST /api/attest` wire (#858) — against the
LIVE test infrastructure, and runs the round-trip drill through it. The
reproducible driver is:

```bash
./scripts/bridge-testnet-federation.sh   # run with no args for full usage
```

### Topology

| Piece | Value (default) |
|---|---|
| Federation | 3 × `bth-bridge` processes, threshold **2** |
| BTH chain | live betanet, `https://seed.botho.io/rpc` (nodes: seed / seed2 / faucet / eu.seed / ap.seed `.botho.io`) |
| Ethereum | live Sepolia: wBTH `0x49b985eC427EE771A601F11b18f7d4402fA2DD7B`, 2-of-3 Safe `0x61274F558f9027e2D402d3340dE89152FA3F3947` |
| Solana | live devnet: program `CZDnzeywrqEM5ereWJmtYKUQ9uJXxX2PydqqKTQStxxE`, mint `F7LsiATxVQxnDEBWemfuq1BgFDYbuzqMMJ5eZjaB7LFX` |
| Ethereum federation identities | the three Safe owner keys (`.secrets/bridge-testnet/eth-safe-owner-{1,2,3}.key`) — fixed by the on-chain Safe owner set |
| BTH/Solana federation identities | 3 fresh Ed25519 keys (`federation/ed25519-{1,2,3}.key`, generated by `keys`) |
| Relayer | LP EOA (`eth-lp.key`) — gas only, holds no roles (ADR 0002) |
| Instance ports | ops `9741/9751/9761`, attest `9742/9752/9762`, public API `9743` (instance 1) |
| Order store | ONE shared SQLite file (WAL + busy-timeout) — see the caveat below |

**Single-host caveat (order replication gap).** Today's code exchanges
*attestation envelopes* between instances but NOT *order records*: the attest
endpoint refuses envelopes for orders it has never heard of
(`refused:unknown_order`), mint orders exist only in the DB of the instance
whose public API created them, and each instance's burn watcher would mint a
DIFFERENT random order UUID for the same on-chain burn. The drill therefore
runs all N instances on one host sharing one order store — the cryptographic
custody is still genuinely t-of-n over the wire (each process signs with only
its key; nothing is prepared below threshold; the DB-level
`record_mint_submitted`/`record_release_tx` guards arbitrate the multi-writer
submission race exactly-once). Cross-host order replication (deterministic
burn-order ids derived from the source tx + order gossip/sync) is the tracked
follow-up from the #868 findings.

**Solana custody caveat.** The devnet wbth `mint_authority` is a single key
(deploy #867). The startup custody guard (`mint/solana.rs`) HARD-FAILS a
federation posture (`solana.mint_signers` + threshold) whose authority is a
lone local key — correctly. The Solana leg of the drill therefore stays
disabled (`BRIDGE_SOLANA_FEDERATION=0`) until the devnet authority migrates
to a real multisig (Squads), after which flipping the env var wires the
ed25519 federation + keypair in.

### Operator prerequisites (live BTH legs)

| # | Prerequisite | Why | Status 2026-07-16 |
|---|---|---|---|
| 1 | Betanet producing blocks | deposits/releases must externalize; SCP finality is the confirmation gate | **BLOCKED** — chain frozen at height 202 (~20 h): every node paused minting at the 10 000 BTH faucet cap (`totalMined 10 100 BTH`). Resume minting on ≥1 validator (or raise the cap) |
| 2 | Reserve funded with **factor-1** outputs | ADR 0003 — the releaser spends only factor-1; the deposit gate rejects non-factor-1 | **BLOCKED** — bootstrap-epoch emission is zero, so NO factor-1 coins exist on the betanet except via settlement; faucet grants are cluster-tagged. Needs `dev_settleToBackground` (testnet-only RPC, #1025) run against a funded wallet on a live node, or wallet-side settlement support |
| 3 | User wallet funded (deposit + memo) | the deposit must carry the order memo (UUID binding) | faucet grants work (`faucet_request` at `https://faucet.botho.io/rpc`, 1 BTH/grant, 3/addr/day) once (1) unfreezes; send the deposit via the web wallet `/trade` export panel (#1035/#1043) pointed at instance 1's public API (`http://127.0.0.1:9743`) — the wallet CLI cannot attach memos |
| 4 | Sepolia relayer gas | Safe `execTransaction` submission | ✅ LP EOA holds ~0.18 Sepolia ETH |
| 5 | Devnet multisig authority | Solana federation custody guard | **BLOCKED** — single-key authority by design of #867; migrate to Squads for the federated Solana leg |

### The drill

```bash
# one-time
./scripts/bridge-testnet-federation.sh keys         # federation ed25519 keys + attest token
./scripts/bridge-testnet-federation.sh gen-reserve  # throwaway RANDOM reserve+user wallets
# fund the reserve per prerequisite (2), the user per (3)

./scripts/bridge-testnet-federation.sh up           # 3 instances, threshold 2, live endpoints
./scripts/bridge-testnet-federation.sh status       # every instance healthy + component states
./scripts/bridge-testnet-federation.sh proof        # live proof-of-reserves snapshots

# Leg A (mint): creates the order, prints deposit params (address+memo+amount),
# polls to `completed`, then asserts factor-1 (supply & balance deltas == net),
# exactly-once (single `mints` row), and zero reserve drift on every instance:
./scripts/bridge-testnet-federation.sh drill-mint 100000000000000 0x<user-eth-addr>

# Leg B (burn): submits a real `bridgeBurn` on live Sepolia from the LP wallet,
# waits for detect → threshold release attestation → BTH release, then dumps
# the attestation audit trail and re-asserts the invariant:
./scripts/bridge-testnet-federation.sh drill-burn 100000000000000 <bth-user-addr>

./scripts/bridge-testnet-federation.sh attest-log   # the full federation trail
./scripts/bridge-testnet-federation.sh down
```

Record every tx link and the `attest-log` output in the drill log on #816/#868.

### What the 2026-07-17 live run PROVED (drill log)

The federation was stood up against the live endpoints (3 instances,
threshold 2; Sepolia via `https://sepolia.drpc.org`; per-instance betanet
RPCs) and driven as far as the operator prerequisites allow. Verified live:

1. **Real over-the-wire threshold custody.** With the Ethereum watcher
   cursor back-filled to before the real repatriation burn
   ([`0x6e4258a6…de284`](https://sepolia.etherscan.io/tx/0x6e4258a6838277bc11c548bfe636787eab8e682d6c037e1ac34dedd6e7ede284),
   9 066.108938801491 BTH, destination string
   `bth-testnet-repatriation-demo`), all three instances re-detected and
   confirmed it (4 086 confirmations), each self-attested with ONLY its own
   Ed25519 key, exchanged envelopes through the authenticated
   `POST /api/attest` endpoints, and the audit log recorded
   `attestation_authorized action=bridge.release_bth threshold=2 signers=2`
   (then `signers=3`) — a genuine 2-of-3 across three processes.
2. **Fail-safe release.** `prepare_release` then refused the unparseable
   demo destination (`config error: release recipient address`) — the order
   parked retryably at `burn_confirmed`, no reserve key material touched.
3. **Live proof-of-reserves on BOTH chains.** `GET /api/reserve/proof`:
   `ethSupply=91033891061198509` (live Sepolia `totalSupply`),
   `solSupply=100000000000000000` (live devnet mint), `lockedReserve=0`,
   `drift=191033891061198509`, `reserveBalanceChecked=true` — the
   reconciler correctly flags the entire #866–#870 manually-bootstrapped
   supply as unbacked relative to a fresh reserve ledger.
4. **Defense-in-depth fired at every layer, live**: the breaker auto-tripped
   on the peg alert (`paused_reason="reserve drift alert (peg unhealthy)"`)
   until `BRIDGE_RESERVE_TOLERANCE` covered the audited baseline; the
   per-order cap (1 000 BTH) and per-address daily cap (100 BTH) each
   deferred the 9 066 BTH release until raised for the drill.
5. **Public order surface.** `POST /api/bridge/orders` on instance 1 opened
   mint order `8051d814-6acd-4943-aa7f-b505999fd5d8` (100 BTH gross,
   0.1 BTH fee) bound to a memo and the reserve deposit address —
   `awaiting_deposit`, pending betanet block production (prerequisite 1).

A production deployment starts from a genesis reserve ledger reconciled to
any pre-existing supply (or a fresh token) so the tolerance stays 0; drill
assertions use drift *deltas* around each leg either way.

## Phase D — the key-rotation drill (#1061)

Per the #1060 direction (bridge = small **elected** multisig, rotated by
periodic elections with a tolerated outage), the federation driver carries a
reproducible end-to-end **rotation drill**: a MOCK election re-elects the
**same** member set — membership-stable, keys-fresh — and the entire
handover machinery then runs with fresh keys on every custody surface. The
same-set mock is deliberate: it needs zero election-mechanism design, yet
exercises exactly the part that must be provably correct before any real
election matters — the re-key handover and the *old-keys-are-dead* proof.

```bash
# with the Phase C federation up (same env knobs — tolerance, caps):
./scripts/bridge-testnet-federation.sh rotate
# or phase by phase:
#   rotate-elect  rotate-pause  rotate-drain  rotate-keys  rotate-safe
#   rotate-solana rotate-bth    rotate-seal   rotate-restart rotate-verify
#   rotate-resume rotate-attest
# offline artifact self-check (no live services needed):
./scripts/bridge-testnet-federation.sh term-doc-selftest
```

### The mock-election interface (the #1060 seam) — v2 term document

`rotate-elect` writes `federation/election/term-<K>.json` as a **v2 term
document** (ADR 0010; canonical schema
[`docs/bridge/schemas/term-document.v2.schema.json`](schemas/term-document.v2.schema.json),
full field reference in `docs/bridge/election-dynamics.md` §5.2). The document
has a **two-stage lifecycle** (select-then-keygen): `rotate-elect` pins
MEMBERSHIP as `status: "elected"` (no keys yet), and `rotate-seal` binds the
fresh per-term keys and flips it to `status: "sealed"`.

```json
{
  "v": 2,
  "term": 2,
  "electionKind": "mock-same-set",
  "status": "elected",
  "electorate": { "curationDocHash": "…", "snapshotHeight": 0,
                  "eligible": ["node-fed-01", "node-fed-02", "node-fed-03"] },
  "tally": { "rule": "approval-top-N-v1", "ballots": 3, "resultHash": "…", "…": "…" },
  "threshold": 2,
  "members": [ { "index": 1, "nodeId": "node-fed-01", "approvals": 3 }, "…" ],
  "execution": { "ethereum": { "safe": "0x…", "intent": "swapOwner" }, "…": "…" },
  "validity": { "electedAt": 0, "handoverDeadline": 0, "termEnd": 0 },
  "signatures": { "tallyAttestations": [ "…" ], "outgoing": [] }
}
```

`rotate-seal` then adds each member's `keys` + `keySubmissionSig` (fresh keys
signed by the member's long-lived identity key) and the `signatures.outgoing`
counter-signature at threshold, and sets `status: "sealed"`. Everything after
the seal consumes ONLY this document; `rotate-verify` re-validates it against
the schema and cross-checks its pinned keys against the live key files. A real
#1060/#1067 election replaces `rotate-elect`'s tally with an on-chain ballot —
same `elected` schema, possibly different members — and `rotate-seal` and the
rest are **unchanged**. A membership-changing election additionally maps
added/removed indices onto `addOwnerWithThreshold`/`removeOwner` instead of
pure `swapOwner`; the drill's pause → drain → re-key → seal → verify → resume
skeleton is identical.

### Phase order and what each step asserts

| Phase | Action | Gate/assertion |
|---|---|---|
| `rotate-elect` | mock same-set election → **v2 `elected` term document** (membership only, no keys) | emitted document validates against `docs/bridge/schemas/term-document.v2.schema.json` |
| `rotate-pause` | trip the breaker (shared store — all N pause) | every `/api/status` shows `paused`; the PUBLIC order API answers **503** to an order-create probe (deliberately invalid body — proves the gate with no order created). The public surface gained a READ-ONLY pause gate for exactly this (#1061): while paused the bridge must stop *accepting* orders, not merely stop settling them |
| `rotate-drain` | wait for in-flight orders | **hard** states (`mint_pending`, `release_pending` — value in motion) are never overridable; **soft** states (`awaiting_deposit`, `deposit_*`, `burn_*` — parked, retryable) require an explicit `BRIDGE_ROTATE_ACK_PARKED` operator ack, recorded in the drill state (their attestation sets rebuild under the new keys) |
| `rotate-keys` | archive term-`K−1` material under `federation/retired/term-<K−1>/`; fresh Ed25519 federation keys, fresh shared attest bearer token, fresh Safe-owner secp256k1 keys | all writes under the git-ignored secrets dir (`require_gitignored`); the LP/relayer key is NOT rotated — it holds no roles (ADR 0002) |
| `rotate-safe` | LIVE Sepolia: `swapOwner(prev, old, new)` per member, executed through the Safe by 2-of-3 `execTransaction`; the signing set migrates as swaps land (swap 1 signed by old-2+old-3, swap 2 by new-1+old-3, swap 3 by new-1+new-2 — the real handover choreography) | receipt `status == 1`; `isOwner(old) == false`, `isOwner(new) == true`, threshold unchanged after every swap |
| `rotate-solana` | devnet wbth SPL mint-authority `SetAuthority` to a fresh key (single-key testnet custody per #867) | authority re-read from chain equals the new pubkey; **gated with exact commands when Solana tooling is absent** |
| `rotate-bth` | new random BTH reserve wallet; `reserve.env` re-pointed | the funds-moving sweep (old reserve → new reserve, factor-1 preserved) is a live BTH tx — **operator-gated while #1051 holds**; on this betanet the locked reserve is 0 (funding itself is blocked, prerequisite 2), so the sweep is vacuous and the procedure is documented instead |
| `rotate-seal` | **select-then-keygen seal**: bind the fresh per-term keys into the term document (each winner's `keySubmissionSig`, signed by its long-lived identity key), gather the outgoing counter-signatures at threshold, flip `status` → `sealed` | the sealed document validates against the v2 schema; only a `sealed` document authorizes execution (the `if/then` in the schema makes per-term keys REQUIRED at `sealed`) |
| `rotate-restart` | re-render configs (pin the NEW pubkey sets + token) and restart all N | requires a `sealed` term document; staggered `/health` wait, as in `up` |
| `rotate-verify` | **the resume gate: old keys are powerless** | first re-validates the `sealed` term document against the v2 schema and asserts its pinned per-member keys equal the live key files (the document is the authority); then see below — resume refuses to run until this phase passes |
| `rotate-resume` | lift the breaker, commit `current-term = K` | all instances unpaused; the public probe now answers **400** (validation) instead of 503 — gate lifted, still no order created |
| `rotate-attest` | post-rotation proof round | a fresh `attestation_authorized` audit row AFTER the resume timestamp (the NEW set reaching threshold — old envelopes cannot contribute, per `rotate-verify`), and `/api/reserve/proof` drift EXACTLY equal to the pre-pause baseline on every instance (rotation moves no value) |

### The old-keys-dead assertions (`rotate-verify`)

The drill does not merely diff configs — it fires real, validly-signed
old-key material at the live surfaces and requires refusal:

1. **Federation attestation**: a canonically-encoded, domain-separated,
   correctly Ed25519-signed release attestation envelope — signed by a
   RETIRED key, bound to a real on-record order — is POSTed to every
   instance's authenticated `/api/attest`. Required outcome on all N:
   `refused:unknown_signer`. (The envelope is built outside the Rust
   codebase — python + openssl mirroring `bridge/core/src/attestation.rs` —
   so the probe cannot accidentally share a bug with the verifier.)
2. **Impersonation control**: the same envelope presented under a NEW
   member's signer id but still signed by the old key must be
   `refused:bad_signature` — proves the pipeline is verifying signatures,
   not blanket-refusing, and that a retired key cannot ride a current
   identity.
3. **Transport auth**: the OLD shared attest bearer token gets HTTP 401.
4. **Sepolia Safe**: `isOwner(old) == false` for every retired owner, and an
   `execTransaction` **simulation** (`eth_call` — free, no state change) of a
   benign 0-value self-call signed by two OLD owner keys must revert
   (`GS026`), while the same call signed by two NEW owner keys succeeds.
5. **Gated legs** (Solana/BTH when tooling or #1051 gates them) must have a
   recorded outcome — a leg can be gated, never silently skipped.

Only after all of the above does `rotate-resume` lift the pause.

### What the 2026-07-17 live rotation run PROVED (drill log)

Run against the live Phase C topology (3 instances, threshold 2; live
Sepolia Safe `0x61274F55…`; per-instance betanet RPCs;
`BRIDGE_RESERVE_TOLERANCE` at the audited `191033891061198509` baseline),
term 1 → term 2:

1. **Mock election** re-elected the 3 sitting members
   (`federation/election/term-2.json`, `electionKind=mock-same-set`).
2. **Pause**: breaker tripped (`rotation drill term 2`), all 3 instances
   reported paused off the shared store, and the PUBLIC order API answered
   **503** to the invalid-body probe — the new read-only pause gate live.
3. **Drain**: no value-in-motion orders; the Phase C parked burn order
   `b25af574-…7d6b` (9 066 BTH, `burn_confirmed`, destination unparseable
   by design) was explicitly acknowledged via `BRIDGE_ROTATE_ACK_PARKED`
   and recorded; the stale Phase C mint order had expired on its own.
4. **Re-key**: term-1 material archived to `federation/retired/term-1/`;
   3 fresh Ed25519 keys, fresh attest token, 3 fresh Safe owner keys.
5. **LIVE Sepolia Safe handover** — three real `swapOwner` 2-of-3
   `execTransaction`s, signing set migrating as swaps landed:
   * owner 1 `0xc74E98E2…` → `0x8475814F…`
     [`0xa527245f…a7c3`](https://sepolia.etherscan.io/tx/0xa527245f454a484ee9e29bf11dfd2016bfa14b8bd72f8b32c16d14c60df1a7c3)
   * owner 2 `0x1D72CDeC…` → `0xdc6d844a…`
     [`0xe681874f…def1`](https://sepolia.etherscan.io/tx/0xe681874f686c0964e96dc34c6355451df825ce15ac40ed2c7b9349242327def1)
   * owner 3 `0x53bce951…` → `0x1590A5Fd…`
     [`0x493c408e…64b5`](https://sepolia.etherscan.io/tx/0x493c408e0bc9e970de23a832d619462bec40150cb6b314c5da8badce366464b5)
   Threshold 2 unchanged throughout.
6. **Solana leg GATED (tooling)**: live devnet read showed mint authority
   `5bLHYxv4P5NMoAFaiKN2WfWQxwK2FL6PqKYZqPgSs9mx` (single key per #867);
   `solana-keygen`/`spl-token` absent on the drill host, exact operator
   commands printed and recorded.
7. **BTH leg**: new random reserve wallet generated and `reserve.env`
   re-pointed; the sweep was **vacuous** — the reserve ledger's locked
   balance was 0 pc (funding itself is blocked, prerequisite 2), with the
   funded-case sweep documented and gated on #1051.
8. **Old keys proven POWERLESS (the resume gate)** — all live:
   * validly-signed old-key release attestation →
     `refused:unknown_signer` on **all 3** instances;
   * old key under a NEW signer id → `refused:bad_signature`;
   * old attest bearer token → HTTP 401;
   * all 3 old owners `isOwner == false`, all 3 new owners `true`;
   * `execTransaction` simulation signed by two OLD owners reverted
     **GS026**; the same call signed by two NEW owners simulated `true`.
9. **Resume**: breaker lifted, all instances unpaused, public probe flipped
   503 → 400 (gate lifted, no order created), `current-term = 2` committed.
10. **Post-rotation proof round**: 2 s after resume the audit log recorded
    `attestation_authorized action=bridge.release_bth threshold=2
    signers=2` (then `signers=3`) for the parked burn order — the NEW set
    reaching threshold over the wire (old envelopes cannot contribute, per
    step 8). Proof-of-reserves drift was **exactly**
    `191033891061198509` on every instance both before the pause and after
    the resume — the rotation moved no value.

### Live vs documented legs (honest scorecard)

| Surface | This drill | Mainnet delta |
|---|---|---|
| Federation Ed25519 attestation keys | **live** — swapped, old refused `unknown_signer` | same mechanics; keys live in HSM/enclave per operator |
| Shared attest bearer token | **live** — rotated, old token 401 | per-peer mTLS/token per #1019 hardening |
| Sepolia Safe owner set | **live** — 3 × on-chain `swapOwner` via 2-of-3 `execTransaction` | per-role Safes (#1019); owner EOAs → hardware keys; same `swapOwner` choreography |
| Solana wbth mint authority | **gated: tooling** (single-key `SetAuthority`, exact commands printed) | authority = Squads multisig: rotation is a member-swap **proposal inside Squads**, not `SetAuthority` |
| BTH reserve wallet | wallet re-keyed; sweep **vacuous** (locked = 0) / **operator-gated on #1051** when funded | reserve sweep is a normal factor-1 reserve spend to the new reserve address, run under the SAME pause window, verified by the reconciler before resume |
| Election | **mock same-set** | real #1060 election result document, same schema |

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
