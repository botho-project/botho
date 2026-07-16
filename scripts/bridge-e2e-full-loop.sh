#!/usr/bin/env bash
# Bridge FULL-LOOP end-to-end driver (#993).
#
# The single orchestrated wrap -> mint-wBTH -> burn -> release-BTH round trip,
# driven through the REAL bridge engine (OrderProcessor::process_pending_orders)
# with BOTH chains live and ZERO external creds:
#
#   1. compiles the Ethereum contracts (WrappedBTH + the SafeStub multisig),
#   2. starts a local Hardhat JSON-RPC node (chain id 31337),
#   3. starts a local Botho node via the `botho-testnet` harness and mines a
#      warmup so the reserve has spendable factor-1 outputs + a decoy ring,
#   4. runs the #[ignore]d full-loop test
#      (bridge/service/src/e2e_full_loop_tests.rs), which deploys the token,
#      wires ONE BridgeConfig across both chains, and lets the engine walk a
#      mint order to Completed and a burn order to Released — asserting the
#      four peg/custody/proof-of-reserves/federation properties,
#   5. tears both nodes down.
#
# Usage: ./scripts/bridge-e2e-full-loop.sh
#
# Env (Ethereum leg — hermetic by default):
#   BRIDGE_FORK_RPC_URL   override the ETH RPC (skips starting hardhat; point
#                         at an already-running anvil, a Sepolia fork, or live
#                         Sepolia — companion #992/#866, no test-logic change).
#
# Env (BTH leg — a funded factor-1 reserve on the local Botho node):
#   BRIDGE_BTH_RPC_URL            node JSON-RPC (default http://127.0.0.1:27200)
#   BRIDGE_BTH_RESERVE_VIEW_KEY   reserve wallet view key (32-byte hex file)
#   BRIDGE_BTH_RESERVE_SPEND_KEY  reserve wallet spend key (32-byte hex file)
#   BRIDGE_BTH_RESERVE_ADDRESS    reserve public BTH address (change target)
#   BRIDGE_BTH_USER_ADDRESS       user BTH address (release destination)
#   BRIDGE_BTH_USER_VIEW_KEY      user wallet view key (scan-back)
#   BRIDGE_BTH_USER_SPEND_KEY     user wallet spend key (scan-back)
#   BRIDGE_BTH_AMOUNT             picocredits to wrap (default 1 BTH)
#
# When the BTH reserve key material is not provided the test SELF-SKIPS
# (green), exactly like bridge/service/src/bth_fork_tests.rs — it never claims
# a live path it could not exercise. Provision the reserve wallet keys (an
# operator or a key-export step) to run the loop unattended.
#
# The live-Sepolia variant of this same loop stays a manual drill (funded
# Safes + Sepolia ETH): docs/bridge/testnet-e2e-runbook.md.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTRACTS_DIR="$REPO_ROOT/contracts/ethereum"
HARDHAT_PID=""
BOTHO_STARTED=""

cleanup() {
    if [[ -n "$BOTHO_STARTED" ]]; then
        echo "Stopping botho-testnet"
        (cd "$REPO_ROOT" && cargo run --release --bin botho-testnet -- stop) 2>/dev/null || true
    fi
    if [[ -n "$HARDHAT_PID" ]] && kill -0 "$HARDHAT_PID" 2>/dev/null; then
        echo "Stopping hardhat node (pid $HARDHAT_PID)"
        kill "$HARDHAT_PID" 2>/dev/null || true
        wait "$HARDHAT_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

echo "==> Compiling Ethereum contracts"
cd "$CONTRACTS_DIR"
if [[ ! -d node_modules ]]; then
    npm ci
fi
npx hardhat compile

if [[ -z "${BRIDGE_FORK_RPC_URL:-}" ]]; then
    echo "==> Starting local hardhat node"
    npx hardhat node --port 8545 >/tmp/bridge-full-loop-hardhat.log 2>&1 &
    HARDHAT_PID=$!
    echo "==> Waiting for the ETH RPC to come up"
    for _ in $(seq 1 30); do
        if curl -sf -X POST -H 'Content-Type: application/json' \
            --data '{"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":1}' \
            http://127.0.0.1:8545 >/dev/null 2>&1; then
            break
        fi
        if ! kill -0 "$HARDHAT_PID" 2>/dev/null; then
            echo "hardhat node died; log follows:" >&2
            cat /tmp/bridge-full-loop-hardhat.log >&2
            exit 1
        fi
        sleep 1
    done
    export BRIDGE_FORK_RPC_URL="http://127.0.0.1:8545"
else
    echo "==> Using existing ETH node at $BRIDGE_FORK_RPC_URL"
fi

export BRIDGE_BTH_RPC_URL="${BRIDGE_BTH_RPC_URL:-http://127.0.0.1:27200}"

echo "==> Starting local Botho node (botho-testnet)"
cd "$REPO_ROOT"
# A single node externalizes blocks under SCP with an n=1 quorum; the harness
# default is fine too. --clean guarantees a fresh chain each run.
cargo run --release --bin botho-testnet -- start --nodes 1 --clean --wait-consensus
BOTHO_STARTED=1

echo "==> Mining a reserve warmup (spendable factor-1 outputs + decoy ring)"
# The CLSAG release needs reserve-owned factor-1 outputs AND enough decoys in
# the recent window (DEFAULT_RING_SIZE-1 per input). The harness pre-funds a
# deterministic wallet; give the chain time to produce a spendable window.
sleep 15

echo "==> Running the full-loop e2e against ETH=$BRIDGE_FORK_RPC_URL BTH=$BRIDGE_BTH_RPC_URL"
if [[ -z "${BRIDGE_BTH_RESERVE_VIEW_KEY:-}" || -z "${BRIDGE_BTH_RESERVE_SPEND_KEY:-}" ]]; then
    echo "::notice::BTH reserve key material not provided; the full-loop test will"
    echo "::notice::self-skip its BTH leg (see script header + testnet-e2e-runbook.md)."
fi
cargo test -p bth-bridge-service -- --ignored full_loop_ --nocapture

echo "==> Bridge full-loop e2e finished"
