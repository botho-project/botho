# wbth-ntt — Wormhole NTT deployment config (Sepolia ↔ HyperEVM)

Deployment tooling for bridging **wBTH** from Ethereum Sepolia to Hyperliquid's
HyperEVM testnet via **Wormhole NTT** (issue #876). The full step-by-step,
decimal reconciliation, trust surface, and operator gates live in
[`docs/bridge/hyperliquid-ntt-runbook.md`](../../docs/bridge/hyperliquid-ntt-runbook.md).

## What's tracked here vs. regenerated

The NTT CLI works out of a standalone workspace (`ntt new` clones a Foundry
project with its own git + submodules — too heavy to nest in this repo). That
workspace is **regenerated on demand**; only the small, meaningful artifacts are
tracked here:

- `overrides.example.json` — the RPC overrides (Sepolia + HyperEVM testnet).
  Copy to `overrides.json` in the workspace. The public HyperEVM RPC is
  rate-limited (~100 req/min); swap in an Alchemy/Chainstack key for the deploy.
- `deployment.json` — **committed after the deploy** (records the on-chain
  NttManager/Transceiver/PeerToken addresses for reproducibility). Not present
  until the transport hop runs.

## Toolchain (recorded for reproducibility)

- `ntt` CLI **v1.7.0** (installs from `main`: `curl -fsSL https://raw.githubusercontent.com/wormhole-foundation/native-token-transfers/main/cli/install.sh | bash`)
- Foundry (forge) 1.7.x · Bun ≥1.2.23 · Node ≥18

## Deploy sequence (gated on HyperEVM HYPE gas)

```bash
ntt new wbth-ntt && cd wbth-ntt
ntt init Testnet
cp <repo>/contracts/wbth-ntt/overrides.example.json overrides.json   # edit RPCs

# HUB: Sepolia in LOCKING mode against the EXISTING wBTH — NttManager only locks,
# never mints, so the 2-of-3 Safe minter and the Sepolia token stay untouched.
ntt add-chain Sepolia --latest --mode locking \
    --token 0x49b985ec427ee771a601f11b18f7d4402fa2dd7b

# SPOKE: HyperEVM in BURNING mode (fresh PeerToken; minter = HyperEVM NttManager).
ntt add-chain HyperEVM --latest --mode burning

ntt push          # deploys + wires peers
ntt status        # reconcile
```

> `ntt add-chain` **deploys on-chain** — it is not a local-only config step.
> Sepolia needs the deployer's ETH (have it); HyperEVM needs **HYPE gas** and the
> deployer opted into **big blocks** first (see the runbook's operator gates).
> Both sides are deployed together so peering is coherent.

Decimals: PeerToken 12 · HyperCore weiDecimals 8 · `--evm-extra-wei-decimals 4`;
move only 8-decimal-aligned amounts (bridge quantum = 10,000 picocredits). See
the runbook for the full three-layer explanation.
