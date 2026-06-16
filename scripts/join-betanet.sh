#!/usr/bin/env bash
#
# Betanet join smoke test
# =======================
#
# Launches a fresh, throwaway local botho node, points it at the LIVE beta
# testnet seed (seed.botho.io:17100) over the public internet, and verifies
# that the local node:
#
#   1. Connects to the betanet (peerCount >= 1), and
#   2. Syncs the chain (its chainHeight climbs toward the live betanet tip).
#
# This validates the *external* peering path -- joining the betanet from
# outside the AWS VPC -- which the VPC-internal bootstrap setup does not
# exercise.
#
# IMPORTANT -- this is a SMOKE / OPS test, NOT a hermetic CI gate:
#   - It depends on external live infrastructure (seed.botho.io must be up).
#   - It is non-deterministic (sync speed depends on the live network).
#   - It must NOT be wired into PR/push CI. It is manual (workflow_dispatch)
#     and a developer convenience script only.
#
# By DEFAULT the node runs SYNC-ONLY (no minting). The optional --mine flag
# enables minting, which MUTATES THE LIVE CHAIN and must only ever be used
# against the beta testnet on purpose.
#
# The node uses a throwaway config + data dir under $(mktemp -d); it never
# touches ~/.botho. Everything is cleaned up on exit.
#
# Usage:
#   ./scripts/join-betanet.sh [options]
#
# Options:
#   --binary PATH     Path to the botho binary (default: target/release/botho,
#                     else build with `cargo build -p botho --release`).
#   --seed HOST       Seed host to bootstrap from (default: seed.botho.io).
#   --seed-rpc URL    Public RPC URL of the betanet to read the live tip from
#                     (default: https://seed.botho.io/rpc).
#   --timeout SECS    Overall timeout for reaching PASS (default: 180).
#   --mine            DANGER: mint on the live betanet (mutates the live chain;
#                     beta only). Default OFF -- the smoke test is sync-only.
#   -h, --help        Show this help.

set -euo pipefail

# ---------------------------------------------------------------------------
# Defaults
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

BINARY=""
SEED_HOST="seed.botho.io"
SEED_GOSSIP_PORT="17100"
SEED_RPC_URL="https://seed.botho.io/rpc"
TIMEOUT_SECS="180"
MINE="false"

# Local (throwaway) node ports -- deliberately offset from the standard
# testnet ports (17100/17101/19090) to avoid colliding with any local node.
LOCAL_GOSSIP_PORT="27100"
LOCAL_RPC_PORT="27101"
LOCAL_METRICS_PORT="0"  # disabled

# ---------------------------------------------------------------------------
# Colors / logging
# ---------------------------------------------------------------------------
if [[ -t 1 ]]; then
    RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; BLUE='\033[0;34m'; NC='\033[0m'
else
    RED=''; GREEN=''; YELLOW=''; BLUE=''; NC=''
fi
log_info()  { echo -e "${BLUE}[INFO]${NC} $1"; }
log_ok()    { echo -e "${GREEN}[ OK ]${NC} $1"; }
log_warn()  { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error() { echo -e "${RED}[FAIL]${NC} $1"; }

usage() { sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//; /^set -euo/d'; }

# Resolve a hostname to a single IPv4 address (best-effort, portable).
resolve_ipv4() {
    local host="$1" ip=""
    if command -v getent >/dev/null 2>&1; then
        ip="$(getent ahostsv4 "$host" 2>/dev/null | awk '/STREAM/{print $1; exit}')"
    fi
    if [[ -z "$ip" ]] && command -v dig >/dev/null 2>&1; then
        ip="$(dig +short A "$host" 2>/dev/null | grep -E '^[0-9.]+$' | head -n1)"
    fi
    if [[ -z "$ip" ]] && command -v nslookup >/dev/null 2>&1; then
        ip="$(nslookup "$host" 2>/dev/null | awk '/^Address: /{print $2}' | grep -E '^[0-9.]+$' | head -n1)"
    fi
    if [[ -z "$ip" ]] && command -v python3 >/dev/null 2>&1; then
        ip="$(python3 -c "import socket,sys; print(socket.gethostbyname(sys.argv[1]))" "$host" 2>/dev/null)"
    fi
    [[ -n "$ip" ]] && echo "$ip"
}

# ---------------------------------------------------------------------------
# Parse args
# ---------------------------------------------------------------------------
while [[ $# -gt 0 ]]; do
    case "$1" in
        --binary)   BINARY="$2"; shift 2 ;;
        --seed)     SEED_HOST="$2"; shift 2 ;;
        --seed-rpc) SEED_RPC_URL="$2"; shift 2 ;;
        --timeout)  TIMEOUT_SECS="$2"; shift 2 ;;
        --mine)     MINE="true"; shift ;;
        -h|--help)  usage; exit 0 ;;
        *) log_error "Unknown argument: $1"; usage; exit 2 ;;
    esac
done

# ---------------------------------------------------------------------------
# Resolve the botho binary
# ---------------------------------------------------------------------------
if [[ -z "$BINARY" ]]; then
    if [[ -x "$REPO_ROOT/target/release/botho" ]]; then
        BINARY="$REPO_ROOT/target/release/botho"
    elif [[ -x "$REPO_ROOT/target/debug/botho" ]]; then
        BINARY="$REPO_ROOT/target/debug/botho"
    else
        log_info "No prebuilt binary found; building (cargo build -p botho --release)..."
        (cd "$REPO_ROOT" && cargo build -p botho --release)
        BINARY="$REPO_ROOT/target/release/botho"
    fi
fi
if [[ ! -x "$BINARY" ]]; then
    log_error "botho binary not found or not executable: $BINARY"
    exit 1
fi
log_info "Using binary: $BINARY"

# ---------------------------------------------------------------------------
# Throwaway temp config + data dir
# ---------------------------------------------------------------------------
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/betanet-smoke.XXXXXX")"
CONFIG_FILE="$TMP_DIR/config.toml"
NODE_LOG="$TMP_DIR/node.log"
NODE_PID=""

cleanup() {
    local code=$?
    if [[ -n "$NODE_PID" ]] && kill -0 "$NODE_PID" 2>/dev/null; then
        log_info "Stopping local node (pid $NODE_PID)..."
        kill "$NODE_PID" 2>/dev/null || true
        # Give it a moment, then force.
        for _ in 1 2 3 4 5; do
            kill -0 "$NODE_PID" 2>/dev/null || break
            sleep 1
        done
        kill -9 "$NODE_PID" 2>/dev/null || true
    fi
    if [[ -n "${TMP_DIR:-}" && -d "$TMP_DIR" ]]; then
        rm -rf "$TMP_DIR"
        log_info "Removed temp dir $TMP_DIR"
    fi
    exit "$code"
}
trap cleanup EXIT INT TERM

# The ledger DB is created next to the config file (ledger_db_path_from_config),
# so placing the config inside $TMP_DIR fully isolates this node's data.
cat > "$CONFIG_FILE" <<EOF
# Throwaway betanet-smoke follower config. Generated by join-betanet.sh.
# Sync-only by default; do NOT reuse for a real node.
network_type = "testnet"

[network]
gossip_port = $LOCAL_GOSSIP_PORT
rpc_port = $LOCAL_RPC_PORT
metrics_port = $LOCAL_METRICS_PORT
cors_origins = ["http://localhost", "http://127.0.0.1"]

# Bootstrap straight to the live betanet seed over the public internet.
bootstrap_peers = ["/dns4/$SEED_HOST/tcp/$SEED_GOSSIP_PORT"]

# Use the explicit bootstrap peer above; do not also pull DNS seeds.
[network.dns_seeds]
enabled = false

# Follower (sync-only): trust discovered peers, accept a single-peer betanet.
[network.quorum]
mode = "recommended"
min_peers = 1

[minting]
enabled = false
EOF

log_info "Throwaway config written to $CONFIG_FILE"
log_info "Local node ports: gossip=$LOCAL_GOSSIP_PORT rpc=$LOCAL_RPC_PORT (metrics disabled)"

# NOTE on the bootstrap multiaddr:
#   The node's libp2p transport currently does NOT support /dns4/ multiaddrs --
#   dialing "/dns4/seed.botho.io/tcp/17100" fails with MultiaddrNotSupported.
#   So we resolve the seed host to an IPv4 address ourselves and write an
#   /ip4/<addr>/tcp/<port> bootstrap entry instead. (See PR notes / issue #373.)
SEED_IP="$(resolve_ipv4 "$SEED_HOST" || true)"
if [[ -n "$SEED_IP" ]]; then
    log_info "Resolved $SEED_HOST -> $SEED_IP; using /ip4/$SEED_IP/tcp/$SEED_GOSSIP_PORT (transport lacks /dns4/ support)."
    # Rewrite the bootstrap_peers line in-place with the resolved IPv4 multiaddr.
    if sed --version >/dev/null 2>&1; then
        sed -i "s#bootstrap_peers = .*#bootstrap_peers = [\"/ip4/$SEED_IP/tcp/$SEED_GOSSIP_PORT\"]#" "$CONFIG_FILE"
    else
        sed -i '' "s#bootstrap_peers = .*#bootstrap_peers = [\"/ip4/$SEED_IP/tcp/$SEED_GOSSIP_PORT\"]#" "$CONFIG_FILE"
    fi
else
    log_warn "Could not resolve $SEED_HOST to an IPv4 address; leaving /dns4/ bootstrap entry"
    log_warn "(dialing will likely fail with MultiaddrNotSupported)."
fi

# ---------------------------------------------------------------------------
# Read the live betanet tip (best-effort; used as the sync target)
# ---------------------------------------------------------------------------
rpc_height() {
    # $1 = RPC URL. Echoes the chainHeight integer, or empty on failure.
    curl -s -m 10 -X POST "$1" \
        -H 'Content-Type: application/json' \
        -d '{"jsonrpc":"2.0","method":"node_getStatus","params":{},"id":1}' 2>/dev/null \
        | grep -o '"chainHeight":[0-9]*' | head -n1 | cut -d: -f2
}
rpc_peer_count() {
    curl -s -m 10 -X POST "$1" \
        -H 'Content-Type: application/json' \
        -d '{"jsonrpc":"2.0","method":"node_getStatus","params":{},"id":1}' 2>/dev/null \
        | grep -o '"peerCount":[0-9]*' | head -n1 | cut -d: -f2
}

BETANET_TIP="$(rpc_height "$SEED_RPC_URL" || true)"
if [[ -n "$BETANET_TIP" ]]; then
    log_info "Live betanet tip (from $SEED_RPC_URL): height $BETANET_TIP"
else
    log_warn "Could not read live betanet tip from $SEED_RPC_URL (continuing; will still check peering + height advance)."
fi

# ---------------------------------------------------------------------------
# Launch the local node
# ---------------------------------------------------------------------------
RUN_ARGS=(--testnet -c "$CONFIG_FILE" run)
if [[ "$MINE" == "true" ]]; then
    log_warn "DANGER: --mine enabled. This node will MINT on the LIVE betanet and MUTATE the live chain."
    RUN_ARGS+=(--mint)
else
    log_info "Sync-only mode (no minting). The live chain will not be mutated."
fi

log_info "Launching: $BINARY ${RUN_ARGS[*]}"
"$BINARY" "${RUN_ARGS[@]}" >"$NODE_LOG" 2>&1 &
NODE_PID=$!
log_info "Local node started (pid $NODE_PID); logs at $NODE_LOG"

LOCAL_RPC="http://127.0.0.1:$LOCAL_RPC_PORT"

# ---------------------------------------------------------------------------
# Poll for PASS: peerCount >= 1 AND height advancing toward the live tip
# ---------------------------------------------------------------------------
DEADLINE=$(( $(date +%s) + TIMEOUT_SECS ))
PEERED="false"
SYNCED="false"
LAST_HEIGHT="0"
MAX_HEIGHT="0"
PEER_COUNT="0"

log_info "Polling local RPC ($LOCAL_RPC) for up to ${TIMEOUT_SECS}s..."

while [[ $(date +%s) -lt $DEADLINE ]]; do
    if ! kill -0 "$NODE_PID" 2>/dev/null; then
        log_error "Local node process exited unexpectedly. Last log lines:"
        tail -n 30 "$NODE_LOG" || true
        exit 1
    fi

    pc="$(rpc_peer_count "$LOCAL_RPC" || true)"
    h="$(rpc_height "$LOCAL_RPC" || true)"
    [[ -n "$pc" ]] && PEER_COUNT="$pc"
    if [[ -n "$h" ]]; then
        LAST_HEIGHT="$h"
        (( h > MAX_HEIGHT )) && MAX_HEIGHT="$h"
    fi

    if [[ "${PEER_COUNT:-0}" -ge 1 && "$PEERED" == "false" ]]; then
        PEERED="true"
        log_ok "Connected to betanet: peerCount=$PEER_COUNT"
    fi

    # Refresh the live tip occasionally so a moving target is tracked.
    cur_tip="$(rpc_height "$SEED_RPC_URL" || true)"
    [[ -n "$cur_tip" ]] && BETANET_TIP="$cur_tip"

    # Synced if we have a peer and our height has reached/closely approached the
    # live tip (allow a small lag for a moving tip). If the tip is unknown,
    # accept any positive, advancing height.
    if [[ "$PEERED" == "true" ]]; then
        if [[ -n "${BETANET_TIP:-}" && "$BETANET_TIP" -gt 0 ]]; then
            if [[ "${MAX_HEIGHT:-0}" -ge $(( BETANET_TIP - 2 )) && "${MAX_HEIGHT:-0}" -gt 0 ]]; then
                SYNCED="true"
            fi
        elif [[ "${MAX_HEIGHT:-0}" -gt 0 ]]; then
            SYNCED="true"
        fi
    fi

    echo "  peerCount=$PEER_COUNT localHeight=$LAST_HEIGHT (max $MAX_HEIGHT) betanetTip=${BETANET_TIP:-?}"

    if [[ "$PEERED" == "true" && "$SYNCED" == "true" ]]; then
        break
    fi
    sleep 5
done

# ---------------------------------------------------------------------------
# Report
# ---------------------------------------------------------------------------
echo ""
echo "======================================================================"
echo "  Betanet join smoke test -- result"
echo "======================================================================"
echo "  Seed bootstrap : ${SEED_IP:+/ip4/$SEED_IP/tcp/$SEED_GOSSIP_PORT (resolved from $SEED_HOST)}"
echo "  Betanet tip    : ${BETANET_TIP:-unknown}"
echo "  Local peerCount: $PEER_COUNT"
echo "  Local height   : $LAST_HEIGHT (max observed: $MAX_HEIGHT)"
echo "  Mining         : $MINE"
echo "----------------------------------------------------------------------"

# Did the local node actually receive blocks from the betanet but reject them
# because it cannot backfill missing history? That's the external-sync finding.
RECEIVED_BLOCKS="false"
if grep -qiE "Failed to add (network|reconstructed) block|Expected height" "$NODE_LOG" 2>/dev/null; then
    RECEIVED_BLOCKS="true"
fi

if [[ "$PEERED" == "true" && "$SYNCED" == "true" ]]; then
    log_ok "PASS: connected to the betanet and synced toward the live tip."
    exit 0
elif [[ "$PEERED" == "true" ]]; then
    log_warn "PARTIAL: connected to the betanet (peerCount=$PEER_COUNT) but did NOT"
    log_warn "fully sync within ${TIMEOUT_SECS}s (local max height $MAX_HEIGHT vs tip ${BETANET_TIP:-?})."
    if [[ "$RECEIVED_BLOCKS" == "true" ]]; then
        log_warn "REAL FINDING: the node peered and RECEIVED gossiped blocks at the live"
        log_warn "tip, but rejected them (\"Expected height 1, got N\") because it has no"
        log_warn "working historical block-range backfill over the external path -- it only"
        log_warn "sees the current tip via gossip and cannot fetch blocks 1..N-1. External"
        log_warn "peering works; external initial-sync does not."
    fi
    log_warn "Last node log lines:"
    tail -n 20 "$NODE_LOG" || true
    exit 1
else
    log_error "FAIL: could not connect to the betanet (peerCount stayed 0) within ${TIMEOUT_SECS}s."
    log_error "If you see 'MultiaddrNotSupported', the transport rejected the bootstrap"
    log_error "multiaddr (the node does not support /dns4/; an /ip4/ address is required)."
    log_error "Last node log lines:"
    tail -n 30 "$NODE_LOG" || true
    exit 1
fi
