#!/usr/bin/env bash
# Bridge DeFi ROUND-TRIP end-to-end driver for the SOLANA venue (#1079) — the
# Solana analog of scripts/bridge-e2e-defi-fork.sh.
#
# It threads the two already-landed-but-disconnected Solana pieces into ONE
# continuous journey of a coin, driven through the REAL bridge engine transports
# (mint::solana + watchers::solana + reserve::SolSupplySource) and the REAL Orca
# Whirlpool on devnet:
#
#   1. MINT BTH on a local Botho node (a funded factor-1 reserve),
#   2. WRAP -> wBTH: the REAL Ed25519 t-of-n mint-submission transport
#      (bridge/service/src/mint/solana.rs) assembles + signs + submits the
#      hardened bridge_mint (#850 per-order marker PDA = exactly-once). NOT a
#      shortcut mint — the wBTH mint's only authority is the federation key.
#   3. SEED the pool: drive contracts/solana/scripts/devnet-orca-pool.ts with the
#      FRESHLY bridge-minted wBTH (the mint recipient IS the Orca LP, so the
#      wrapped coin is what seeds the position — no throwaway mint).
#   4. PURCHASE: swap against the seeded pool (devnet-orca-swap.ts).
#   5. REPATRIATE: burn the swap proceeds -> the REAL Solana burn-watcher
#      transport (bridge/service/src/watchers/solana.rs) decodes the
#      BridgeBurnEvent -> engine release to a fresh BTH stealth output (ADR 0004)
#      the user's own view key scans back.
#
# So a coin travels: Botho BTH -> wBTH -> into an Orca pool -> bought via a swap
# -> back to native BTH, with the Solana-leg proof-of-reserves invariant
# (Σ wBTH supply == locked reserve, reserve.rs / #853), factor-1 amount deltas,
# and exactly-once submission asserted across the loop.
#
# =========================================================================
# HONEST LIMITATION / OPERATOR BOUNDARY (read before running)
# =========================================================================
# Unlike the Ethereum path, Orca Whirlpools CANNOT be forked/cloned
# hermetically (cloning a full Orca deployment + config + tick arrays via
# `--clone` is fragile — see the maintainer note on #865). So the Orca
# pool/swap legs (steps 3-4) can ONLY be validated against LIVE devnet (needs
# devnet SOL + the deployed program / mint), and a federated Solana mint
# additionally needs the Squads multisig from #1052. This driver is therefore
# CONSTRUCTION-VALIDATED: the bridge-transport legs (steps 1-2, 5) run against a
# local solana-test-validator (with the wbth program --clone-upgradeable-program'd
# from devnet) OR self-skip green; the Orca legs are gated behind RUN_ORCA=1 and
# are the operator step tracked by #1052 / #868 — NOT part of this code
# deliverable's green run.
#
# Usage:
#   ./scripts/bridge-e2e-defi-solana.sh              # full driver
#   ./scripts/bridge-e2e-defi-solana.sh --check      # dry-run self-check (no cluster)
#
# Modes:
#   --check | --dry-run   Validate wiring, tools, and the referenced files, print
#                         the plan, and exit 0 WITHOUT starting any node or
#                         touching a cluster. Safe to run anywhere (used by CI
#                         + the runbook as the hermetic sanity gate).
#
# Env (Solana bridge-transport legs — steps 1-2, 5):
#   BRIDGE_SOLANA_RPC_URL     cluster JSON-RPC for the bridge transports (default
#                             http://127.0.0.1:8899, i.e. the local validator).
#   BRIDGE_SOLANA_PROGRAM     deployed wbth program id, base58 (default the #867
#                             devnet program).
#   BRIDGE_SOLANA_WBTH_MINT   wBTH SPL mint, base58 (default the #870 devnet mint).
#   BRIDGE_SOLANA_KEYPAIR     mint-authority keypair (hex seed or CLI json). When
#                             absent the Rust mint/burn drills self-skip (green).
#   BRIDGE_SOLANA_RECIPIENT   base58 pubkey that receives the wrapped wBTH. Set it
#                             to the Orca LP (solana-lp) pubkey so the minted coin
#                             seeds the pool (the thread). Defaults to the LP.
#   BRIDGE_SOLANA_BROADCAST=1 also broadcast + confirm the assembled mint (needs a
#                             funded authority + initialized program). Unset =
#                             assemble-only construction validation.
#   RUN_LOCAL_VALIDATOR=1     boot a local solana-test-validator with the wbth
#                             program cloned from devnet (--clone-upgradeable-program)
#                             for real local execution of the mint/burn transports.
#
# Env (BTH leg — a funded factor-1 reserve on a local Botho node, reused verbatim
#      from bridge-e2e-defi-fork.sh):
#   BRIDGE_BTH_RPC_URL            node JSON-RPC (default http://127.0.0.1:27200)
#   BRIDGE_BTH_RESERVE_VIEW_KEY   reserve wallet view key (32-byte hex file)
#   BRIDGE_BTH_RESERVE_SPEND_KEY  reserve wallet spend key (32-byte hex file)
#   BRIDGE_BTH_RESERVE_ADDRESS    reserve public BTH address (change target)
#   BRIDGE_BTH_USER_ADDRESS       user BTH address (release destination)
#   BRIDGE_BTH_USER_VIEW_KEY      user wallet view key (scan-back)
#   BRIDGE_BTH_USER_SPEND_KEY     user wallet spend key (scan-back)
#
# Env (Orca live-devnet legs — steps 3-4, OPERATOR / #1052 / #868):
#   RUN_ORCA=1        drive devnet-orca-pool.ts + devnet-orca-swap.ts against LIVE
#                     devnet (needs devnet SOL in the solana-lp keypair + the
#                     .secrets/bridge-testnet/solana-lp.json + solana-deployer.json
#                     account keys from #1008). Unset = skip the Orca legs (the
#                     construction-validated default).
#   SLIPPAGE_BPS      forwarded to the Orca scripts (default 100 = 1%).
#
# When the Solana key material is not provided the bridge-transport drills
# SELF-SKIP (green) — exactly like bridge-e2e-defi-fork.sh — never claiming a
# live path they could not exercise. Provision the #1008 accounts + a funded
# authority to run the whole round trip.
#
# Fork -> testnet -> mainnet flip (Solana venue): see the "Layer 0.75-Solana"
# section of docs/bridge/testnet-e2e-runbook.md for the config-only flip table
# (local validator -> devnet -> mainnet-beta; single-key authority -> Squads).

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SOLANA_CONTRACTS_DIR="$REPO_ROOT/contracts/solana"
ORCA_POOL_SCRIPT="$SOLANA_CONTRACTS_DIR/scripts/devnet-orca-pool.ts"
ORCA_SWAP_SCRIPT="$SOLANA_CONTRACTS_DIR/scripts/devnet-orca-swap.ts"

# Default devnet identities from #867 (program) / #870 (mint).
DEFAULT_PROGRAM="CZDnzeywrqEMd8VLcbPXY2eqDb4Rn9jBnhLuC7Nff1yg"
DEFAULT_WBTH_MINT="F7LsiATxVQxnDEBWemfuq1BgFDYbuzqMMJ5eZjaB7LFX"

MODE="run"
if [[ "${1:-}" == "--check" || "${1:-}" == "--dry-run" ]]; then
    MODE="check"
fi

VALIDATOR_PID=""
BOTHO_STARTED=""

cleanup() {
    if [[ -n "$BOTHO_STARTED" ]]; then
        echo "==> Stopping botho-testnet"
        (cd "$REPO_ROOT" && cargo run --release --bin botho-testnet -- stop) 2>/dev/null || true
    fi
    if [[ -n "$VALIDATOR_PID" ]] && kill -0 "$VALIDATOR_PID" 2>/dev/null; then
        echo "==> Stopping solana-test-validator (pid $VALIDATOR_PID)"
        kill "$VALIDATOR_PID" 2>/dev/null || true
        wait "$VALIDATOR_PID" 2>/dev/null || true
    fi
}

# ---------------------------------------------------------------------------
# --check / --dry-run: hermetic self-check. Validates the wiring WITHOUT a
# cluster so CI + the runbook can gate the driver's shape on every push.
# ---------------------------------------------------------------------------
run_self_check() {
    local ok=1
    echo "==> bridge-e2e-defi-solana.sh self-check (no cluster touched)"

    echo "--> Referenced files:"
    local f
    for f in "$ORCA_POOL_SCRIPT" "$ORCA_SWAP_SCRIPT" \
        "$REPO_ROOT/bridge/service/src/mint/solana.rs" \
        "$REPO_ROOT/bridge/service/src/watchers/solana.rs" \
        "$REPO_ROOT/bridge/service/src/reserve.rs" \
        "$REPO_ROOT/bridge/service/src/solana_devnet_tests.rs"; do
        if [[ -f "$f" ]]; then
            echo "    [ok]   ${f#"$REPO_ROOT"/}"
        else
            echo "    [MISS] ${f#"$REPO_ROOT"/}"
            ok=0
        fi
    done

    echo "--> Tool availability (informational — legs self-skip when absent):"
    local t
    for t in cargo solana solana-test-validator anchor npx node; do
        if command -v "$t" >/dev/null 2>&1; then
            echo "    [have] $t"
        else
            echo "    [none] $t"
        fi
    done

    echo "--> Round-trip plan:"
    echo "    1. MINT BTH        local botho-testnet reserve (factor-1)"
    echo "    2. WRAP  -> wBTH   mint::solana Ed25519 t-of-n transport (no shortcut mint)"
    echo "    3. SEED  pool      devnet-orca-pool.ts, seeded from the minted wBTH (RUN_ORCA=1)"
    echo "    4. SWAP            devnet-orca-swap.ts against the seeded pool (RUN_ORCA=1)"
    echo "    5. REPATRIATE      watchers::solana burn decode -> engine release -> BTH stealth output"
    echo "    peg assertion      Σ wBTH supply == locked reserve (reserve.rs), factor-1, exactly-once"

    echo "--> Bridge-transport drill invoked in full mode:"
    echo "    cargo test -p bth-bridge-service -- --ignored solana_devnet_ --nocapture"

    if [[ "$ok" -ne 1 ]]; then
        echo "self-check FAILED: a referenced file is missing" >&2
        return 1
    fi
    echo "==> self-check OK (construction-validated wiring present)"
    return 0
}

if [[ "$MODE" == "check" ]]; then
    run_self_check
    exit $?
fi

# ---------------------------------------------------------------------------
# Full driver.
# ---------------------------------------------------------------------------
trap cleanup EXIT

BRIDGE_SOLANA_RPC_URL="${BRIDGE_SOLANA_RPC_URL:-http://127.0.0.1:8899}"
BRIDGE_SOLANA_PROGRAM="${BRIDGE_SOLANA_PROGRAM:-$DEFAULT_PROGRAM}"
BRIDGE_SOLANA_WBTH_MINT="${BRIDGE_SOLANA_WBTH_MINT:-$DEFAULT_WBTH_MINT}"
export BRIDGE_SOLANA_RPC_URL BRIDGE_SOLANA_PROGRAM

echo "==> Solana DeFi round-trip driver"
echo "    RPC=$BRIDGE_SOLANA_RPC_URL program=$BRIDGE_SOLANA_PROGRAM mint=$BRIDGE_SOLANA_WBTH_MINT"

# ---- Optional: local solana-test-validator with the wbth program cloned ----
if [[ "${RUN_LOCAL_VALIDATOR:-}" == "1" ]]; then
    if ! command -v solana-test-validator >/dev/null 2>&1; then
        echo "::notice::RUN_LOCAL_VALIDATOR=1 but solana-test-validator is not installed;"
        echo "::notice::falling back to BRIDGE_SOLANA_RPC_URL=$BRIDGE_SOLANA_RPC_URL as-is."
    else
        echo "==> Starting solana-test-validator, cloning the wbth program from devnet"
        solana-test-validator --reset \
            --url https://api.devnet.solana.com \
            --clone-upgradeable-program "$BRIDGE_SOLANA_PROGRAM" \
            >/tmp/bridge-e2e-defi-solana-validator.log 2>&1 &
        VALIDATOR_PID=$!
        echo "==> Waiting for the local validator RPC to come up"
        for _ in $(seq 1 60); do
            if curl -sf -X POST -H 'Content-Type: application/json' \
                --data '{"jsonrpc":"2.0","id":1,"method":"getHealth"}' \
                http://127.0.0.1:8899 >/dev/null 2>&1; then
                break
            fi
            if ! kill -0 "$VALIDATOR_PID" 2>/dev/null; then
                echo "validator died; log follows:" >&2
                cat /tmp/bridge-e2e-defi-solana-validator.log >&2
                exit 1
            fi
            sleep 1
        done
        export BRIDGE_SOLANA_RPC_URL="http://127.0.0.1:8899"
    fi
fi

# ---- Start a local Botho node for the release / reserve leg ----
export BRIDGE_BTH_RPC_URL="${BRIDGE_BTH_RPC_URL:-http://127.0.0.1:27200}"
echo "==> Starting local Botho node (botho-testnet)"
cd "$REPO_ROOT"
cargo run --release --bin botho-testnet -- start --nodes 1 --clean --wait-consensus
BOTHO_STARTED=1

echo "==> Mining a reserve warmup (spendable factor-1 outputs + decoy ring)"
sleep 15

if [[ -z "${BRIDGE_SOLANA_KEYPAIR:-}" ]]; then
    echo "::notice::BRIDGE_SOLANA_KEYPAIR not provided; the mint/burn transport drills"
    echo "::notice::will self-skip (see script header + testnet-e2e-runbook.md)."
fi

# ---- Steps 1-2 + 5: the REAL bridge-transport drills (mint / burn / supply) ----
echo "==> Running the Solana bridge-transport drills (mint -> wrap, burn -> release, peg)"
echo "    mint::solana + watchers::solana + reserve::SolSupplySource against $BRIDGE_SOLANA_RPC_URL"
cargo test -p bth-bridge-service -- --ignored solana_devnet_ --nocapture

# ---- Steps 3-4: the Orca pool/swap legs (LIVE devnet only — operator step) ----
if [[ "${RUN_ORCA:-}" == "1" ]]; then
    echo "==> Driving the Orca pool + swap legs against LIVE devnet (operator step, #1052/#868)"
    echo "    seeding the pool from the freshly bridge-minted wBTH ($BRIDGE_SOLANA_WBTH_MINT)"
    if [[ ! -d "$SOLANA_CONTRACTS_DIR/node_modules" ]]; then
        echo "==> Installing Solana contract deps"
        (cd "$SOLANA_CONTRACTS_DIR" && npm ci)
    fi
    TS_OPTS='{"module":"commonjs","target":"ES2020","esModuleInterop":true,"skipLibCheck":true}'
    (cd "$SOLANA_CONTRACTS_DIR" && npx ts-node --compiler-options "$TS_OPTS" "$ORCA_POOL_SCRIPT")
    (cd "$SOLANA_CONTRACTS_DIR" && npx ts-node --compiler-options "$TS_OPTS" "$ORCA_SWAP_SCRIPT")
    echo "==> Orca live-devnet legs finished"
else
    echo "::notice::RUN_ORCA unset — skipping the Orca pool/swap legs. Orca Whirlpools"
    echo "::notice::cannot be forked/cloned hermetically, so those legs are the operator"
    echo "::notice::live-devnet step tracked by #1052 / #868 (see the runbook). The"
    echo "::notice::bridge-transport legs above are the construction-validated deliverable."
fi

echo "==> Bridge DeFi round-trip Solana driver finished"
