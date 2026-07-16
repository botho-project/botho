#!/usr/bin/env bash
# Bridge DeFi ROUND-TRIP end-to-end driver against a SEPOLIA FORK (#1005).
#
# THE MAINNET LIQUIDITY-LAUNCH REHEARSAL. This is the exact sequence the team
# runs to bootstrap wBTH liquidity on a DEX — first against a Sepolia fork (this
# script, zero creds), then live testnet, then mainnet, by swapping the RPC
# endpoint and a funded key (see docs/bridge/testnet-e2e-runbook.md, layer 0.75
# and the fork -> testnet -> mainnet flip table).
#
# It joins the two already-landed pieces into ONE continuous journey of a coin,
# driven through the REAL bridge engine (OrderProcessor::process_pending_orders)
# and the REAL Uniswap v3 periphery inherited from forked Sepolia state:
#
#   1. MINT BTH on a local Botho node (a funded factor-1 reserve),
#   2. WRAP -> wBTH: t-of-n EIP-712 federation attestation -> Safe bridgeMint,
#   3. FUND gas via *_setBalance + wrap ETH into WETH,
#   4. SEED the pool: create the wBTH/WETH Uniswap v3 pool + add liquidity,
#   5. PURCHASE: swap WETH -> wBTH against the seeded pool (the market buys),
#   6. REPATRIATE: bridgeBurn the swap proceeds -> t-of-n Ed25519 attestation ->
#      BthReleaser pays native BTH to a fresh stealth output the user scans back.
#
# So a coin travels: Botho BTH -> wBTH -> into a DEX pool -> bought via a swap ->
# back to native BTH, with the peg verified at both ends and proof-of-reserves
# drift == 0 across the whole loop.
#
# The Uniswap v3 periphery + WETH already exist on Sepolia, so an
# `anvil --fork-url <sepolia>` node inherits them; only a throwaway WrappedBTH +
# SafeStub is freshly deployed onto the fork. NO funded account, NO deployed
# contract, NO secret — the dev accounts are funded on the fork via *_setBalance.
#
# Usage:
#   ./scripts/bridge-e2e-defi-fork.sh <SEPOLIA_RPC_URL>
#   SEPOLIA_RPC_URL=https://ethereum-sepolia-rpc.publicnode.com \
#     ./scripts/bridge-e2e-defi-fork.sh
#
# Env (Ethereum fork leg):
#   SEPOLIA_RPC_URL      upstream archive/public RPC to fork from (required,
#                        unless passed as the first argument).
#   BRIDGE_FORK_RPC_URL  override the LOCAL fork endpoint the test talks to
#                        (default http://127.0.0.1:8545). Point this at an
#                        already-running fork to skip starting one here.
#
# Env (BTH leg — a funded factor-1 reserve on a local Botho node):
#   BRIDGE_BTH_RPC_URL            node JSON-RPC (default http://127.0.0.1:27200)
#   BRIDGE_BTH_RESERVE_VIEW_KEY   reserve wallet view key (32-byte hex file)
#   BRIDGE_BTH_RESERVE_SPEND_KEY  reserve wallet spend key (32-byte hex file)
#   BRIDGE_BTH_RESERVE_ADDRESS    reserve public BTH address (change target)
#   BRIDGE_BTH_USER_ADDRESS       user BTH address (release destination)
#   BRIDGE_BTH_USER_VIEW_KEY      user wallet view key (scan-back)
#   BRIDGE_BTH_USER_SPEND_KEY     user wallet spend key (scan-back)
#   BRIDGE_BTH_AMOUNT             picocredits to wrap (default 200,000 BTH;
#                                 must cover the pool's wBTH liquidity side)
#
# When the BTH reserve key material is not provided the test SELF-SKIPS (green),
# exactly like scripts/bridge-e2e-full-loop.sh — it never claims a live path it
# could not exercise. Provision the reserve wallet keys (#999) to run the whole
# round trip unattended.
#
# Fork -> testnet -> mainnet flip (same test, config-only — the launch runbook):
#   point BRIDGE_FORK_RPC_URL at a live RPC, set BRIDGE_UNISWAP_* /
#   BRIDGE_WETH_ADDRESS for that chain, set BRIDGE_WBTH_ADDRESS to the
#   #866-deployed token instead of a throwaway deploy, leave
#   BRIDGE_FORK_FUND_ACCOUNTS unset (no setBalance on a real chain), and supply
#   genuinely funded relayer/LP keys.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTRACTS_DIR="$REPO_ROOT/contracts/ethereum"
NODE_PID=""
BOTHO_STARTED=""

SEPOLIA_RPC_URL="${1:-${SEPOLIA_RPC_URL:-}}"
if [[ -z "$SEPOLIA_RPC_URL" && -z "${BRIDGE_FORK_RPC_URL:-}" ]]; then
    echo "error: SEPOLIA_RPC_URL is required (pass as \$1 or export it), unless" >&2
    echo "       BRIDGE_FORK_RPC_URL points at an already-running fork node." >&2
    echo "       e.g. ./scripts/bridge-e2e-defi-fork.sh https://ethereum-sepolia-rpc.publicnode.com" >&2
    exit 2
fi

cleanup() {
    if [[ -n "$BOTHO_STARTED" ]]; then
        echo "Stopping botho-testnet"
        (cd "$REPO_ROOT" && cargo run --release --bin botho-testnet -- stop) 2>/dev/null || true
    fi
    if [[ -n "$NODE_PID" ]] && kill -0 "$NODE_PID" 2>/dev/null; then
        echo "Stopping fork node (pid $NODE_PID)"
        kill "$NODE_PID" 2>/dev/null || true
        wait "$NODE_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

echo "==> Compiling Ethereum contracts (throwaway WrappedBTH + SafeStub)"
cd "$CONTRACTS_DIR"
if [[ ! -d node_modules ]]; then
    npm ci
fi
npx hardhat compile

if [[ -z "${BRIDGE_FORK_RPC_URL:-}" ]]; then
    if command -v anvil >/dev/null 2>&1; then
        echo "==> Starting anvil forking Sepolia"
        anvil --fork-url "$SEPOLIA_RPC_URL" --port 8545 \
            >/tmp/bridge-e2e-defi-fork-node.log 2>&1 &
    else
        echo "==> anvil not found; starting hardhat node forking Sepolia"
        cd "$CONTRACTS_DIR"
        npx hardhat node --fork "$SEPOLIA_RPC_URL" --port 8545 \
            >/tmp/bridge-e2e-defi-fork-node.log 2>&1 &
    fi
    NODE_PID=$!

    echo "==> Waiting for the fork RPC to come up"
    for _ in $(seq 1 60); do
        if curl -sf -X POST -H 'Content-Type: application/json' \
            --data '{"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":1}' \
            http://127.0.0.1:8545 >/dev/null 2>&1; then
            break
        fi
        if ! kill -0 "$NODE_PID" 2>/dev/null; then
            echo "fork node died; log follows:" >&2
            cat /tmp/bridge-e2e-defi-fork-node.log >&2
            exit 1
        fi
        sleep 1
    done
    export BRIDGE_FORK_RPC_URL="http://127.0.0.1:8545"
else
    echo "==> Using existing fork node at $BRIDGE_FORK_RPC_URL"
fi

# Pin the expected chain id to Sepolia and fund the dev accounts on the fork.
export BRIDGE_FORK_EXPECTED_CHAIN_ID="${BRIDGE_FORK_EXPECTED_CHAIN_ID:-11155111}"
export BRIDGE_FORK_FUND_ACCOUNTS=1
export BRIDGE_BTH_RPC_URL="${BRIDGE_BTH_RPC_URL:-http://127.0.0.1:27200}"

echo "==> Starting local Botho node (botho-testnet)"
cd "$REPO_ROOT"
# A single node externalizes blocks under SCP with an n=1 quorum. --clean
# guarantees a fresh chain each run.
cargo run --release --bin botho-testnet -- start --nodes 1 --clean --wait-consensus
BOTHO_STARTED=1

echo "==> Mining a reserve warmup (spendable factor-1 outputs + decoy ring)"
# The CLSAG release needs reserve-owned factor-1 outputs AND enough decoys in
# the recent window. Give the chain time to produce a spendable window large
# enough to also cover the ~200,000 BTH wrap.
sleep 15

if [[ -z "${BRIDGE_BTH_RESERVE_VIEW_KEY:-}" || -z "${BRIDGE_BTH_RESERVE_SPEND_KEY:-}" ]]; then
    echo "::notice::BTH reserve key material not provided; the round-trip test will"
    echo "::notice::self-skip its BTH legs (see script header + testnet-e2e-runbook.md)."
fi

echo "==> Running the DeFi round-trip e2e against ETH(fork)=$BRIDGE_FORK_RPC_URL BTH=$BRIDGE_BTH_RPC_URL"
echo "    mint -> wrap -> fund -> pool -> swap -> repatriate (mainnet liquidity-launch rehearsal)"
cargo test -p bth-bridge-service -- --ignored defi_round_trip_ --nocapture

echo "==> Bridge DeFi round-trip Sepolia-fork e2e finished"
