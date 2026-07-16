# Hyperliquid wBTH Runbook — Sepolia → HyperEVM (NTT) → HIP-1 spot

The testnet path that puts **wBTH** on Hyperliquid, covering issue **#876**
(transport hop: Sepolia wBTH → HyperEVM) and **#877** (HIP-1 spot listing +
swap demo). This is mostly ops + third-party (Wormhole / Hyperliquid) tooling,
not core-bridge Rust.

> Status: **DEPLOYED on testnet (2026-07-16, #1026)** — the NTT bridge is live
> and a Sepolia→HyperEVM round trip is proven. Addresses below. #877 (HIP-1
> spot) is next.

## Deployed (testnet, 2026-07-16, #1026)

Single deployer `0x111018cfe4523097B7f651f3A06fA9a2956CF155` (Sepolia ETH +
HyperEVM HYPE). Full config in [`contracts/wbth-ntt/deployment.json`](../../contracts/wbth-ntt/deployment.json).

| | Sepolia (hub, LOCKING) | HyperEVM 998 (spoke, BURNING) |
|---|---|---|
| NttManager | `0xC5652d52fBE4c41c91a65Ecd18304B20e58Df491` | `0x07F159042E9F89484dfdA37D09057c871dbCB475` |
| WormholeTransceiver | `0xbEe886BcC887e96487C2103e46fDa7aDA6b89195` | `0xC5652d52fBE4c41c91a65Ecd18304B20e58Df491` |
| Token | wBTH `0x49b985ec…` (existing, **untouched**) | PeerToken `0x230f154Ae33A53dcFFEDedB2d92cc1F32BcE7610` (`WbthPeerToken.sol`, 12 dec, minter = NttManager) |

Round trip proven: 10 wBTH locked on Sepolia (`NttManager.transfer`, deployer
100→90) → VAA seq 2 → **manually redeemed** on HyperEVM (no Wormhole executor on
testnet: `WormholeTransceiver.receiveMessage`, script `hl-8`) → 10 wBTH PeerToken
minted, 1:1. The Sepolia 2-of-3 Safe was **never touched** (locking uses
`approve` + `transferFrom`, not the mint path). Peered both directions,
inbound+outbound rate limits set.

**Ops scripts** (`contracts/ethereum/scripts/hl-1..8`, keys read from gitignored
`.secrets/`): 1-3 = HYPE funding via the official HL route (bridge ETH→Arbitrum,
swap→USDC, deposit to HL mainnet), 4 = forward HYPE to deployer, 5 = deploy
PeerToken, 6 = set minter, 7 = mint demo wBTH via Safe, 8 = redeem VAA.

**Deploy caveats hit** (for a re-run): Sepolia `add-chain` needs `--skip-verify`
(no etherscan verifier configured); HyperEVM burning mode does **not** auto-deploy
the PeerToken (deploy `WbthPeerToken` first, pass `--token`); the deployer must
be HL-testnet-activated *and* opted into big blocks before HyperEVM deploys; no
Wormhole executor on HyperEVM testnet ⇒ redeem manually.

---

> Original plan (retained for reference). Verify the flagged items on any re-run
> (CLI chain-name string, decimal knobs).

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
