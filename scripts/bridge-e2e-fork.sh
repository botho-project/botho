#!/usr/bin/env bash
# Bridge Ethereum-leg end-to-end driver against a SEPOLIA FORK (#992).
#
# Runs the exact same #[ignore]d Rust fork test as scripts/bridge-e2e-local.sh
# (bridge/service/src/fork_tests.rs), but against a local node that FORKS real
# Sepolia state over a public RPC. This is the closest-to-real-testnet
# demonstration achievable with NO funded account, NO deployed contract, and
# NO secret:
#
#   1. compiles the contracts (WrappedBTH + the SafeStub test multisig),
#   2. starts a local fork of Sepolia — `anvil --fork-url <rpc>` (preferred)
#      or `npx hardhat node --fork <rpc>` — presenting chain id 11155111 with
#      real Sepolia state,
#   3. funds the four dev accounts on the fork via *_setBalance (the test does
#      this itself when BRIDGE_FORK_FUND_ACCOUNTS is set — no real ETH needed),
#   4. freshly deploys a throwaway WrappedBTH + SafeStub onto the fork and runs
#      the full pipeline: federation attestation -> Safe-wrapped bridgeMint ->
#      confirmation polling -> bridgeBurn -> watcher burn scan,
#   5. tears the node down.
#
# Usage:
#   ./scripts/bridge-e2e-fork.sh <SEPOLIA_RPC_URL>
#   SEPOLIA_RPC_URL=https://sepolia.example/v2/<key> ./scripts/bridge-e2e-fork.sh
#
# Env:
#   SEPOLIA_RPC_URL      upstream archive/public RPC to fork from (required,
#                        unless passed as the first argument).
#   BRIDGE_FORK_RPC_URL  override the LOCAL fork endpoint the test talks to
#                        (default http://127.0.0.1:8545). Point this at an
#                        already-running fork to skip starting one here.
#
# Flip to LIVE Sepolia (#866): skip this script, point BRIDGE_FORK_RPC_URL at a
# real Sepolia RPC, set BRIDGE_FORK_EXPECTED_CHAIN_ID=11155111, leave
# BRIDGE_FORK_FUND_ACCOUNTS unset (no setBalance on a real chain), and supply a
# genuinely funded relayer/owner key. Same test, config-only change.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTRACTS_DIR="$REPO_ROOT/contracts/ethereum"
NODE_PID=""

SEPOLIA_RPC_URL="${1:-${SEPOLIA_RPC_URL:-}}"
if [[ -z "$SEPOLIA_RPC_URL" ]]; then
    echo "error: SEPOLIA_RPC_URL is required (pass as \$1 or export it)." >&2
    echo "       e.g. ./scripts/bridge-e2e-fork.sh https://sepolia.example/v2/<key>" >&2
    exit 2
fi

cleanup() {
    if [[ -n "$NODE_PID" ]] && kill -0 "$NODE_PID" 2>/dev/null; then
        echo "Stopping fork node (pid $NODE_PID)"
        kill "$NODE_PID" 2>/dev/null || true
        wait "$NODE_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

echo "==> Compiling contracts"
cd "$CONTRACTS_DIR"
if [[ ! -d node_modules ]]; then
    npm ci
fi
npx hardhat compile

if [[ -z "${BRIDGE_FORK_RPC_URL:-}" ]]; then
    if command -v anvil >/dev/null 2>&1; then
        echo "==> Starting anvil forking Sepolia"
        anvil --fork-url "$SEPOLIA_RPC_URL" --port 8545 \
            >/tmp/bridge-e2e-fork-node.log 2>&1 &
    else
        echo "==> anvil not found; starting hardhat node forking Sepolia"
        cd "$CONTRACTS_DIR"
        npx hardhat node --fork "$SEPOLIA_RPC_URL" --port 8545 \
            >/tmp/bridge-e2e-fork-node.log 2>&1 &
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
            cat /tmp/bridge-e2e-fork-node.log >&2
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

echo "==> Running Rust fork test against forked Sepolia at $BRIDGE_FORK_RPC_URL"
echo "    (expected chain id: $BRIDGE_FORK_EXPECTED_CHAIN_ID; dev accounts funded via *_setBalance)"
cd "$REPO_ROOT"
cargo test -p bth-bridge-service -- --ignored fork_ --nocapture

echo "==> Bridge Ethereum-leg Sepolia-fork e2e passed"
