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
#   BRIDGE_BTH_RESERVE_PQ_SEED    reserve ML-KEM/ML-DSA BIP39 seed (64-byte hex
#                                 file; required on the 6.0.0 hybrid chain to
#                                 detect + spend the reserve's own outputs)
#   BRIDGE_BTH_RESERVE_ADDRESS    reserve public BTH address (change target)
#   BRIDGE_BTH_USER_ADDRESS       user BTH address (release destination)
#   BRIDGE_BTH_USER_VIEW_KEY      user wallet view key (scan-back)
#   BRIDGE_BTH_USER_SPEND_KEY     user wallet spend key (scan-back)
#   BRIDGE_BTH_USER_PQ_SEED       user ML-KEM/ML-DSA BIP39 seed (64-byte hex)
#   BRIDGE_BTH_AMOUNT             picocredits to wrap (default 1 BTH)
#
# The reserve key material is PROVISIONED AT RUNTIME (#999): after the node is
# up this script runs `botho-testnet gen-bridge-keys`, which exports the
# node's own deterministic (pre-funded) mining wallet as the reserve, and mints
# a fresh random user wallet. NO private key is committed to the repo.
#
# A freshly-mined node accrues ONLY 100%-cluster-tagged coinbases, never
# factor-1 (lottery EMISSION is zero in the bootstrap epoch), so the reserve
# would have nothing the releaser's factor_one filter can spend (#1025). The
# driver therefore also runs `botho-testnet fund-reserve`, which settles one of
# node 0's coinbases into a spendable factor-1/background output (ADR 0003) it
# owns — the zero-cost settlement the bootstrap epoch permits. If you would
# rather bring your own funded reserve, set the BRIDGE_BTH_RESERVE_* vars
# yourself and both the keygen and the funding step are skipped.
#
# When no reserve key material is provided AND provisioning is disabled the
# test SELF-SKIPS (green), exactly like bridge/service/src/bth_fork_tests.rs —
# it never claims a live path it could not exercise.
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
# TWO nodes (reserve = node 0). A lone `--nodes 1` node cannot externalize on
# this harness: the testnet config sets `min_peers = 1`, so the #428
# participation gate (should_propose_this_round) blocks a peerless node from
# minting/externalizing — independent of the #1000 SYNC-gate fix. A 2-node
# cluster satisfies both the participation gate (1 peer each) and the 2-of-2
# SCP quorum, so it actually produces blocks. --clean guarantees a fresh chain.
cargo run --release --bin botho-testnet -- start --nodes 2 --clean --wait-consensus
BOTHO_STARTED=1

# Warm the chain up so the reserve funding + release can each form a CLSAG ring
# (DEFAULT_RING_SIZE = 20, i.e. 19 decoys per input). The release draws its
# decoys from NON-reserve outputs (node 1's coinbases), so we need enough blocks
# that ~19 of them exist. Poll the tip until it clears the warmup bar.
BRIDGE_BTH_WARMUP_HEIGHT="${BRIDGE_BTH_WARMUP_HEIGHT:-50}"
echo "==> Warming the chain up to height >= $BRIDGE_BTH_WARMUP_HEIGHT (decoy anonymity set)"
warmup_deadline=$(( $(date +%s) + 2400 ))
while :; do
    height="$(curl -sf -X POST -H 'Content-Type: application/json' \
        --data '{"jsonrpc":"2.0","method":"getChainInfo","params":{},"id":1}' \
        "$BRIDGE_BTH_RPC_URL" 2>/dev/null | sed -n 's/.*"height":\([0-9]*\).*/\1/p')"
    height="${height:-0}"
    echo "    tip height: $height"
    if [[ "$height" -ge "$BRIDGE_BTH_WARMUP_HEIGHT" ]]; then
        break
    fi
    if [[ "$(date +%s)" -ge "$warmup_deadline" ]]; then
        echo "::warning::chain did not reach height $BRIDGE_BTH_WARMUP_HEIGHT within the" \
             "warmup budget (at $height); continuing — the release may skip on too few decoys."
        break
    fi
    sleep 10
done

# Provision the reserve + user wallet key material at runtime (#999) unless the
# caller supplied their own reserve. The reserve is the node's own pre-funded
# mining wallet, and no secret is committed. `eval` imports the
# `export BRIDGE_BTH_*` lines the tool prints on stdout (human logs -> stderr).
if [[ -z "${BRIDGE_BTH_RESERVE_VIEW_KEY:-}" ]]; then
    echo "==> Provisioning bridge reserve + user keys (runtime keygen; no secret committed)"
    BRIDGE_KEYS_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bridge-full-loop-keys.XXXXXX")"
    eval "$(cargo run --release --bin botho-testnet -- \
        gen-bridge-keys --node 0 --out "$BRIDGE_KEYS_DIR")"

    # A freshly-mined node accrues ONLY 100%-cluster-tagged coinbases — never
    # factor-1 — so the releaser's factor_one filter (release/bth.rs) would find
    # nothing to spend (#1025 gap M2). Settle one of node 0's coinbases to a
    # factor-1/background output it owns, giving the reserve a spendable
    # factor-1 UTXO. In the bootstrap epoch the capitalized settlement charge is
    # zero, so this costs only the base fee. Skipped when the caller BYO reserve.
    echo "==> Funding the reserve with a factor-1 settlement output (#1025)"
    cargo run --release --bin botho-testnet -- \
        fund-reserve --node 0 --amount "${BRIDGE_BTH_AMOUNT:-1000000000000}"
else
    echo "==> Using caller-provided BTH reserve key material"
fi

echo "==> Running the full-loop e2e against ETH=$BRIDGE_FORK_RPC_URL BTH=$BRIDGE_BTH_RPC_URL"
if [[ -z "${BRIDGE_BTH_RESERVE_VIEW_KEY:-}" || -z "${BRIDGE_BTH_RESERVE_SPEND_KEY:-}" ]]; then
    echo "::notice::BTH reserve key material not provided; the full-loop test will"
    echo "::notice::self-skip its BTH leg (see script header + testnet-e2e-runbook.md)."
fi
cargo test -p bth-bridge-service -- --ignored full_loop_ --nocapture

echo "==> Bridge full-loop e2e finished"
