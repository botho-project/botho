#!/usr/bin/env bash
# Fork-test the wBTH bridge custody bring-up (#1011) against a SEPOLIA FORK —
# proves scripts/deploy-safe.ts + scripts/deploy.ts work end-to-end with NO
# real testnet ETH and NO secret, before the live Sepolia run.
#
# It:
#   1. starts `anvil --fork-url <sepolia rpc>` (real Sepolia state, chain id
#      11155111 — so the canonical Safe v1.3.0 factory/singleton/handler the
#      deploy script pins are really there),
#   2. funds a throwaway deployer on the fork via `anvil_setBalance` (test ETH
#      only — never a real chain),
#   3. runs deploy-safe.ts on `--network fork` and asserts a real 2-of-3 Safe
#      deployed (the script itself checks getOwners()==owners, getThreshold()==2
#      and aborts on mismatch),
#   4. wires that Safe as WrappedBTH admin/minter/pauser and runs deploy.ts on
#      the fork, then asserts the Safe holds all three roles,
#   5. tears the fork down.
#
# Usage:
#   ./scripts/deploy-safe-fork-test.sh <SEPOLIA_RPC_URL>
#   SEPOLIA_RPC_URL=https://ethereum-sepolia-rpc.publicnode.com ./scripts/deploy-safe-fork-test.sh
#
# Requires: foundry (anvil, cast) and node/npm. anvil is the only fork backend
# here (we need anvil_setBalance + cast for the role assertions).

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTRACTS_DIR="$REPO_ROOT/contracts/ethereum"
FORK_PORT="${FORK_PORT:-8545}"
FORK_URL="http://127.0.0.1:${FORK_PORT}"
NODE_PID=""

SEPOLIA_RPC_URL="${1:-${SEPOLIA_RPC_URL:-}}"
if [[ -z "$SEPOLIA_RPC_URL" ]]; then
    echo "error: SEPOLIA_RPC_URL is required (pass as \$1 or export it)." >&2
    echo "       e.g. ./scripts/deploy-safe-fork-test.sh https://ethereum-sepolia-rpc.publicnode.com" >&2
    exit 2
fi

if ! command -v anvil >/dev/null 2>&1 || ! command -v cast >/dev/null 2>&1; then
    echo "error: foundry (anvil + cast) required. Install: https://getfoundry.sh" >&2
    exit 2
fi

# Deterministic anvil dev accounts (fork-only throwaways — NOT secrets):
# account[0] is the deployer (gas), accounts[1..3] are the Safe owners.
DEPLOYER_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
DEPLOYER_ADDR="0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
OWNER_1="0x70997970C51812dc3A010C7d01b50e0d17dc79C8"
OWNER_2="0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC"
OWNER_3="0x90F79bf6EB2c4f870365E785982E1f101E93b906"

cleanup() {
    if [[ -n "$NODE_PID" ]] && kill -0 "$NODE_PID" 2>/dev/null; then
        echo "==> Stopping fork node (pid $NODE_PID)"
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

echo "==> Starting anvil forking Sepolia"
anvil --fork-url "$SEPOLIA_RPC_URL" --port "$FORK_PORT" \
    >/tmp/deploy-safe-fork-node.log 2>&1 &
NODE_PID=$!

echo "==> Waiting for the fork RPC to come up"
for _ in $(seq 1 60); do
    if cast chain-id --rpc-url "$FORK_URL" >/dev/null 2>&1; then
        break
    fi
    if ! kill -0 "$NODE_PID" 2>/dev/null; then
        echo "fork node died; log follows:" >&2
        cat /tmp/deploy-safe-fork-node.log >&2
        exit 1
    fi
    sleep 1
done

CHAIN_ID="$(cast chain-id --rpc-url "$FORK_URL")"
echo "==> Fork chain id: $CHAIN_ID (expect 11155111)"

echo "==> Funding deployer $DEPLOYER_ADDR via anvil_setBalance (test ETH only)"
cast rpc anvil_setBalance "$DEPLOYER_ADDR" 0x21e19e0c9bab2400000 \
    --rpc-url "$FORK_URL" >/dev/null

export BRIDGE_FORK_RPC_URL="$FORK_URL"
export PRIVATE_KEY="$DEPLOYER_KEY"
export BRIDGE_SAFE_OWNER_1="$OWNER_1"
export BRIDGE_SAFE_OWNER_2="$OWNER_2"
export BRIDGE_SAFE_OWNER_3="$OWNER_3"
unset SAFE_ADDRESS WBTH_ADMIN_SAFE WBTH_MINTER_SAFE WBTH_PAUSER_SAFE

echo "==> Deploying 2-of-3 Safe on the fork"
SAFE_OUT="$(npx hardhat run scripts/deploy-safe.ts --network fork)"
echo "$SAFE_OUT"
SAFE_ADDRESS="$(echo "$SAFE_OUT" | sed -n 's/^SAFE_ADDRESS=//p' | tail -1)"
if [[ -z "$SAFE_ADDRESS" ]]; then
    echo "error: deploy-safe.ts did not print SAFE_ADDRESS" >&2
    exit 1
fi

echo "==> Asserting Safe owners + threshold via cast"
GOT_THRESHOLD="$(cast call "$SAFE_ADDRESS" 'getThreshold()(uint256)' --rpc-url "$FORK_URL")"
GOT_OWNERS="$(cast call "$SAFE_ADDRESS" 'getOwners()(address[])' --rpc-url "$FORK_URL")"
echo "    getThreshold() = $GOT_THRESHOLD"
echo "    getOwners()    = $GOT_OWNERS"
[[ "$GOT_THRESHOLD" == "2" ]] || { echo "FAIL: threshold != 2" >&2; exit 1; }
for o in "$OWNER_1" "$OWNER_2" "$OWNER_3"; do
    echo "$GOT_OWNERS" | grep -iq "${o#0x}" || { echo "FAIL: owner $o missing" >&2; exit 1; }
done
echo "    OK: 2-of-3 Safe with the three owners"

echo "==> Deploying WrappedBTH with the Safe as admin/minter/pauser"
export WBTH_ADMIN_SAFE="$SAFE_ADDRESS"
export WBTH_MINTER_SAFE="$SAFE_ADDRESS"
export WBTH_PAUSER_SAFE="$SAFE_ADDRESS"
WBTH_OUT="$(npx hardhat run scripts/deploy.ts --network fork)"
echo "$WBTH_OUT"
WBTH_ADDRESS="$(echo "$WBTH_OUT" | sed -n 's/.*WrappedBTH deployed at: //p' | tail -1)"
if [[ -z "$WBTH_ADDRESS" ]]; then
    echo "error: deploy.ts did not print the WrappedBTH address" >&2
    exit 1
fi

echo "==> Asserting the Safe holds all three WrappedBTH roles via cast"
DEFAULT_ADMIN_ROLE="0x0000000000000000000000000000000000000000000000000000000000000000"
MINTER_ROLE="$(cast keccak 'MINTER_ROLE')"
PAUSER_ROLE="$(cast keccak 'PAUSER_ROLE')"
for role_pair in "admin=$DEFAULT_ADMIN_ROLE" "minter=$MINTER_ROLE" "pauser=$PAUSER_ROLE"; do
    label="${role_pair%%=*}"; role="${role_pair##*=}"
    has="$(cast call "$WBTH_ADDRESS" 'hasRole(bytes32,address)(bool)' "$role" "$SAFE_ADDRESS" --rpc-url "$FORK_URL")"
    echo "    hasRole($label) = $has"
    [[ "$has" == "true" ]] || { echo "FAIL: Safe missing $label role" >&2; exit 1; }
done
# Deployer must hold NO admin role (ADR 0002).
DEP_ADMIN="$(cast call "$WBTH_ADDRESS" 'hasRole(bytes32,address)(bool)' "$DEFAULT_ADMIN_ROLE" "$DEPLOYER_ADDR" --rpc-url "$FORK_URL")"
[[ "$DEP_ADMIN" == "false" ]] || { echo "FAIL: deployer holds admin role" >&2; exit 1; }
echo "    OK: deployer holds no roles"

echo ""
echo "==> FORK TEST PASSED"
echo "    SAFE_ADDRESS=$SAFE_ADDRESS (2-of-3)"
echo "    WBTH_ADDRESS=$WBTH_ADDRESS (admin=minter=pauser=Safe)"
