# Hyperliquid wBTH Runbook — Sepolia → HyperEVM (NTT) → HIP-1 spot

The testnet path that puts **wBTH** on Hyperliquid, covering issue **#876**
(transport hop: Sepolia wBTH → HyperEVM) and **#877** (HIP-1 spot listing +
swap demo). This is mostly ops + third-party (Wormhole / Hyperliquid) tooling,
not core-bridge Rust.

> Status: **planned** — architecture chosen, commands drafted from the current
> (2026-07) Wormhole NTT + Hyperliquid docs. Not yet executed on testnet;
> on-chain steps are gated on HyperEVM gas (HYPE) and, for #877, HyperCore
> testnet USDC. Verify the flagged items live (CLI chain-name string, decimal
> knobs) before running.

## Design decision — hub/locking, keep the Safe untouched

**Sepolia is the NTT hub in LOCKING mode; HyperEVM is a spoke in BURNING mode.**

Why this and not burn-and-mint everywhere: locking mode requires only a
standard ERC-20 on the hub — the NttManager **locks** wBTH via
`transferFrom` (the user approves it) and **never needs the mint role**. So the
2-of-3 Gnosis Safe that is wBTH's sole minter (ADR-0002) is **not modified, not
granted to Wormhole, and the Sepolia token is not redeployed**. Burn-and-mint
would force adding `mint`/`burn` to the Sepolia wBTH and handing the NttManager
minter rights there — a custody regression we explicitly avoid.

On HyperEVM, NTT deploys a fresh **PeerToken** (the HyperEVM-side wBTH) whose
minter is the HyperEVM NttManager. Minting there is a bridge-internal op backed
1:1 by the wBTH locked on Sepolia — unrelated to the Safe.

```
  Sepolia (hub, LOCKING)                         HyperEVM testnet (spoke, BURNING)
  ┌──────────────────────┐   Wormhole VAA        ┌─────────────────────────────┐
  │ wBTH 0x49b985ec…dd7b │  (13/19 guardians)    │ PeerToken (HyperEVM wBTH)   │
  │ minter = 2/3 Safe    │ ───────────────────▶  │ minter = HyperEVM NttManager│
  │ NttManager LOCKS it  │                       │ mints/burns on transfer     │
  └──────────────────────┘                       └─────────────────────────────┘
        Safe untouched                              ─ ntt hype link ─▶ HyperCore HIP-1 spot
```

## Chain facts (HyperEVM testnet)

| Fact | Value |
|---|---|
| Chain id | **998** |
| RPC | `https://rpc.hyperliquid-testnet.xyz/evm` (~100 req/min; use an Alchemy/Chainstack key for the deploy) |
| Explorer | `https://testnet.purrsec.com/` |
| Gas token | **HYPE** (18 decimals) |
| Deploy Spot UI | `https://app.hyperliquid-testnet.xyz/deploySpot` |

**Big blocks required to deploy.** HyperEVM small blocks cap at 2M gas — too
small for contract deploys. Opt the deployer address into big blocks (30M) with
the L1 action `{"type":"evmUserModify","usingBigBlocks":true}` (Hyperliquid API/
SDK) **before** `ntt push`, or deploys silently underprovision.

## Decimals — three stacked layers (highest-risk surface)

wBTH is **12 decimals** (1 unit = 1 picocredit). Three precisions stack:
1. **NTT wire trim = 8 decimals.** Cross-chain amounts are normalized to ≤8
   decimals ⇒ bridge quantum is **1e-8 wBTH = 10,000 picocredits**. Sub-quantum
   remainder isn't sent (in locking mode it simply stays unlocked on Sepolia —
   not burned).
2. **HyperEVM PeerToken `decimals()`** — a deploy-time choice. Set it
   **explicitly to 12** to keep balances 1:1 with picocredits (document that
   transfers quantize to 1e-8 wBTH), OR to 8 to match the bridge trim exactly.
3. **HyperCore `weiDecimals`** via `evmExtraWeiDecimals = EVM_decimals −
   weiDecimals` (must be in [-2, 18]; `ntt hype link` default 10 is HYPE's
   18−8). For PeerToken=12 + weiDecimals=8 → pass `--evm-extra-wei-decimals 4`.
   **Rounding gotcha:** HyperCore↔HyperEVM transfers **burn** any non-round
   remainder below `evmExtraWeiDecimals` zeros. Keep all demo amounts aligned to
   the coarsest precision (8 decimals) end to end.

Recommended for the demo: PeerToken **12 decimals**, HyperCore `weiDecimals`
**8**, `--evm-extra-wei-decimals 4`, and only ever move 8-decimal-aligned
amounts.

## #876 — command sequence (transport hop)

Prereqs: Foundry (`forge`), Bun ≥1.2.23, Node/npm. Deployer key in
`ETH_PRIVATE_KEY` (use the gitignored `.secrets/bridge-testnet/eth-deployer.key`
account — it needs Sepolia ETH **and** HyperEVM HYPE).

```bash
# Install the NTT CLI (tracks main — record `ntt --version` in the drill log).
curl -fsSL https://raw.githubusercontent.com/wormhole-foundation/native-token-transfers/main/cli/install.sh | bash
ntt --version

ntt new wbth-ntt && cd wbth-ntt
ntt init Testnet

# HUB = Sepolia, LOCKING, existing wBTH (no --token deploy):
ntt add-chain Sepolia --latest --mode locking \
    --token 0x49b985ec427ee771a601f11b18f7d4402fa2dd7b

# SPOKE = HyperEVM testnet, BURNING (verify the exact chain-name string via
# `ntt add-chain --help`; omit --token to auto-deploy a PeerToken, or deploy the
# PeerToken with Forge first and pass --token <addr> + grant minter to manager):
ntt add-chain <HyperEVM-testnet-name> --latest --mode burning

ntt push        # deploy NttManager + WormholeTransceiver (+ PeerToken) on-chain
ntt status      # reconcile; ntt pull to sync deployment.json
```

Permissions to verify after `push`:
- Sepolia: NttManager needs **no token role**; users `approve` it to lock wBTH.
- HyperEVM: PeerToken **minter == HyperEVM NttManager** (`setMinter`).

Then a demo transfer (Sepolia wBTH → HyperEVM wBTH), amounts 8-decimal-aligned,
recorded with tx links. Consider putting the Sepolia NttManager **owner** behind
a Safe (`ntt transfer-ownership`).

## #877 — HIP-1 spot listing + swap (chains off #876)

1. **Deploy Spot on HyperCore** via the testnet UI (`/deploySpot`) — irreversible,
   consumes HyperCore testnet USDC: set `szDecimals`/`weiDecimals` with
   `szDecimals + 5 <= weiDecimals`; save the **Token Index**; genesis-mint supply
   to the asset-bridge address `0x2000…0{tokenIndex:4-hex}`; RegisterSpot; deploy
   Hyperliquidity; trigger genesis.
2. **Link** HyperCore spot ↔ HyperEVM PeerToken:
   `ntt hype link --token-index <IDX> --evm-extra-wei-decimals <N>`.
3. **Bridge + trade:** `ntt hype bridge-in/out`, then a spot order-book swap vs
   spot USDC (API/SDK), capturing order/fill ids + balances.

## Trust surface (added by NTT)

- **Wormhole Guardians (13/19 quorum):** ≥13 malicious guardians could mint
  unbacked HyperEVM wBTH; ≥7 could censor. This is the core added trust vs. our
  own Safe federation.
- **NttManager owner keys** (per chain) can reconfigure peers/thresholds/rate
  limits and pause — put them behind a Safe.
- **HyperEVM PeerToken minter = NttManager** — on HyperEVM, bridge security *is*
  the token's mint authority. Sepolia adds **no** new mint trust (locking).
- Set inbound/outbound **rate limits** in `deployment.json` so a demo doesn't
  silently queue.

## Operator gates (need a human)

- [ ] HyperEVM **HYPE** gas for the deployer (third-party faucet — Chainstack/
      QuickNode; the official faucet gates on a prior mainnet deposit).
- [ ] Opt the deployer into **big blocks** before deploying.
- [ ] (#877) HyperCore testnet **USDC** for Deploy Spot (official faucet, same
      mainnet-deposit gate) + the irreversible UI flow.

## Sources
Wormhole NTT: deploy-to-hyperliquid, deploy-to-evm, cli-commands, get-started,
overview, faqs. Hyperliquid: hyperevm, dual-block-architecture,
hypercore↔hyperevm transfers, HIP-1. (Full URLs in the #876 research thread.)
