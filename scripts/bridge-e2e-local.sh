#!/usr/bin/env bash
# Bridge Ethereum-leg end-to-end driver (#828).
#
# Runs the full happy-path pipeline hermetically on a local machine / CI
# runner — no testnet access or secrets required:
#
#   1. compiles the contracts (WrappedBTH + the SafeStub test multisig),
#   2. starts a local Hardhat JSON-RPC node (chain id 31337),
#   3. runs the #[ignore]d Rust fork tests (bridge/service/src/fork_tests.rs)
#      which deploy the contracts and drive the REAL bridge pipeline:
#      federation attestation -> Safe-wrapped bridgeMint -> confirmation
#      polling -> bridgeBurn -> watcher burn scan,
#   4. tears the node down.
#
# Usage: ./scripts/bridge-e2e-local.sh
#
# Env:
#   BRIDGE_FORK_RPC_URL  override the RPC endpoint (skips starting a node —
#                        e.g. point it at an already-running `anvil`).
#
# The live-testnet round-trip drill (real BTH deposits/releases) is a
# separate, manual procedure: docs/bridge/testnet-e2e-runbook.md.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTRACTS_DIR="$REPO_ROOT/contracts/ethereum"
NODE_PID=""

cleanup() {
    if [[ -n "$NODE_PID" ]] && kill -0 "$NODE_PID" 2>/dev/null; then
        echo "Stopping hardhat node (pid $NODE_PID)"
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
    echo "==> Starting local hardhat node"
    npx hardhat node --port 8545 >/tmp/bridge-e2e-hardhat-node.log 2>&1 &
    NODE_PID=$!

    echo "==> Waiting for RPC to come up"
    for _ in $(seq 1 30); do
        if curl -sf -X POST -H 'Content-Type: application/json' \
            --data '{"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":1}' \
            http://127.0.0.1:8545 >/dev/null 2>&1; then
            break
        fi
        if ! kill -0 "$NODE_PID" 2>/dev/null; then
            echo "hardhat node died; log follows:" >&2
            cat /tmp/bridge-e2e-hardhat-node.log >&2
            exit 1
        fi
        sleep 1
    done
    export BRIDGE_FORK_RPC_URL="http://127.0.0.1:8545"
else
    echo "==> Using existing node at $BRIDGE_FORK_RPC_URL"
fi

echo "==> Running Rust fork tests against $BRIDGE_FORK_RPC_URL"
cd "$REPO_ROOT"
cargo test -p bth-bridge-service -- --ignored fork_ --nocapture

echo "==> Bridge Ethereum-leg e2e passed"
