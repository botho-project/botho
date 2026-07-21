#!/usr/bin/env bash
#
# Re-provision a single relay after a consensus-breaking reset
#
# Composes the existing deploy + wipe steps into ONE operator command for the
# case that has bitten us twice (#1114): a single regional relay is left behind
# on a stale chain after a coordinated, consensus-breaking testnet reset. The
# relay's on-disk ledger is incompatible with the new protocol, so the new
# binary boots against a chain it can never reconcile. deploy-botho.sh alone
# swaps the binary but never wipes the ledger; reset-chain.sh alone wipes the
# ledger but never pins/upgrades the binary. Neither verifies the relay
# actually re-peers and converges on the fleet's tip.
#
# This script does all of it, in the only order that works:
#   1. stop the service
#   2. wipe ONLY the ledger + wallet (config.toml and node_key are preserved)
#   3. deploy the pinned release binary (delegates to deploy-botho.sh)
#   4. restart onto the fresh, empty ledger
#   5. poll the relay's LOCAL RPC until peerCount > 0 AND its tipHash matches a
#      known-good validator, with a bounded timeout
#
# node_key is preserved automatically: it lives at ~/.botho/<network>/node_key,
# a SIBLING of ledger/ and wallet/ (see botho/src/config.rs
# node_key_path_from_config), so wiping ledger/ + wallet/ never touches it. The
# relay keeps its stable peer identity across the re-provision.
#
# Usage:
#   ./reprovision-relay.sh [user@host]
#
# Options:
#   --service NAME        systemd unit to control (default: botho). MUST match
#                         the unit actually installed on the host — the script
#                         verifies it exists and fails loudly otherwise, so it
#                         cannot silently restart the wrong unit.
#   --network NAME        network data dir to wipe (default: testnet)
#   --validator-rpc URL   known-good validator RPC to compare tipHash against
#                         (default: https://seed.botho.io/rpc)
#   --rpc-port PORT       relay's LOCAL RPC port for verification
#                         (default: 17101, the testnet RPC port)
#   --timeout SECONDS     max seconds to wait for re-peering + convergence
#                         (default: 180)
#   --force               skip the confirmation prompt
#   --dry-run             print every remote command without contacting a host
#   --help, -h            show this help and exit
#
# Environment:
#   SSH_KEY      SSH key path (default: ~/.ssh/botho-nodes.pem)
#   RELEASE_TAG  release tag to deploy (default: latest GitHub release).
#                Pin it (e.g. RELEASE_TAG=v0.6.0) for a reproducible reset.
#
# Example:
#   ./reprovision-relay.sh --dry-run ubuntu@eu.seed.botho.io
#   RELEASE_TAG=v0.6.0 ./reprovision-relay.sh ubuntu@eu.seed.botho.io
#   RELEASE_TAG=v0.6.0 ./reprovision-relay.sh --service botho \
#       --validator-rpc https://seed.botho.io/rpc ubuntu@ap.seed.botho.io
#
# Data layout (must match botho/src/config.rs::data_dir / Network::dir_name):
#   ~/.botho/<network>/            base data dir (network = "testnet"|"mainnet")
#   ~/.botho/<network>/ledger/     chain database        (WIPED)
#   ~/.botho/<network>/wallet/     minting/relay wallet  (WIPED — relays carry
#                                  no wallet; see TESTNET_RESET.md §2/§4)
#   ~/.botho/<network>/config.toml node config           (PRESERVED)
#   ~/.botho/<network>/node_key    stable peer identity  (PRESERVED — sibling)
#
# NOTE: This is an OPERATOR script that connects to a live host over SSH.
# It is intentionally never invoked by CI. --dry-run requires no credentials
# and is safe to run anywhere for validation.

set -euo pipefail

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
NC='\033[0m'

log_info() { echo -e "${GREEN}[INFO]${NC} $1"; }
log_step() { echo -e "${BLUE}[STEP]${NC} $1"; }
log_warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error() { echo -e "${RED}[ERROR]${NC} $1"; }

usage() {
    # Print the leading comment block (up to the blank line before `set -e`).
    sed -n '2,67p' "$0" | sed 's/^#\s\{0,1\}//'
}

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Configuration
DEFAULT_HOST="ubuntu@seed.botho.io"
SSH_KEY="${SSH_KEY:-$HOME/.ssh/botho-nodes.pem}"
SERVICE_NAME="botho"
NETWORK="testnet"
VALIDATOR_RPC="https://seed.botho.io/rpc"
RPC_PORT="17101"
TIMEOUT="180"
FORCE=false
DRY_RUN=false

# Parse arguments
HOST="$DEFAULT_HOST"
host_set=false
while [[ $# -gt 0 ]]; do
    case "$1" in
        --service)
            SERVICE_NAME="$2"
            shift
            ;;
        --network)
            NETWORK="$2"
            shift
            ;;
        --validator-rpc)
            VALIDATOR_RPC="$2"
            shift
            ;;
        --rpc-port)
            RPC_PORT="$2"
            shift
            ;;
        --timeout)
            TIMEOUT="$2"
            shift
            ;;
        --force)
            FORCE=true
            ;;
        --dry-run)
            DRY_RUN=true
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        -*)
            log_error "Unknown option: $1"
            usage
            exit 1
            ;;
        *)
            HOST="$1"
            host_set=true
            ;;
    esac
    shift
done

DATA_DIR=".botho/$NETWORK"

SSH_OPTS_DISPLAY="-i $SSH_KEY -o StrictHostKeyChecking=accept-new"
SSH_OPTS="$SSH_OPTS_DISPLAY"

# run_remote: execute (or, in dry-run mode, just print) a command on $HOST.
run_remote() {
    local cmd="$1"
    if [[ "$DRY_RUN" == "true" ]]; then
        echo "    ssh $SSH_OPTS_DISPLAY $HOST \"$cmd\""
    else
        # SC2086: word-split SSH_OPTS intentionally. SC2029: $cmd is a fixed,
        # script-controlled string (no untrusted input), so client-side
        # expansion is fine.
        # shellcheck disable=SC2086,SC2029
        ssh $SSH_OPTS "$HOST" "$cmd"
    fi
}

# run_remote_capture: like run_remote but returns stdout (for preflight checks
# and RPC polling). In dry-run mode it prints the command and returns empty.
run_remote_capture() {
    local cmd="$1"
    if [[ "$DRY_RUN" == "true" ]]; then
        echo "    ssh $SSH_OPTS_DISPLAY $HOST \"$cmd\"" >&2
        echo ""
    else
        # shellcheck disable=SC2086,SC2029
        ssh $SSH_OPTS "$HOST" "$cmd"
    fi
}

# rpc_field: POST node_getStatus to an RPC endpoint and extract a .result field.
# Usage: rpc_field <curl-target-command> <field>
# where <curl-target-command> is the full curl invocation printing raw JSON.
NODE_STATUS_REQ='{"jsonrpc":"2.0","method":"node_getStatus","params":{},"id":1}'

# Validate SSH key exists (skip in dry-run so it works without credentials)
if [[ "$DRY_RUN" != "true" ]]; then
    if [[ ! -f "$SSH_KEY" ]]; then
        log_error "SSH key not found: $SSH_KEY"
        log_info "Set SSH_KEY environment variable or ensure key exists at default location"
        exit 1
    fi
    if ! command -v jq >/dev/null 2>&1; then
        log_error "jq is required for the verification step but was not found on PATH"
        exit 1
    fi
fi

if [[ "$DRY_RUN" == "true" ]]; then
    log_warn "DRY RUN: no SSH connection will be made; commands are printed only."
fi

log_info "Re-provisioning relay on $HOST"
log_info "  service:        $SERVICE_NAME"
log_info "  network:        $NETWORK (data dir ~/$DATA_DIR)"
log_info "  release tag:    ${RELEASE_TAG:-<latest>}"
log_info "  validator RPC:  $VALIDATOR_RPC"

# Confirmation
if [[ "$FORCE" != "true" && "$DRY_RUN" != "true" ]]; then
    log_warn "This will STOP $SERVICE_NAME, WIPE ~/$DATA_DIR/{ledger,wallet}, deploy"
    log_warn "${RELEASE_TAG:-the latest release}, and restart onto a fresh chain."
    log_warn "config.toml and node_key are preserved."
    echo ""
    read -r -p "Are you sure? Type 'yes' to confirm: " confirm
    if [[ "$confirm" != "yes" ]]; then
        log_info "Aborted."
        exit 0
    fi
fi

# ---------------------------------------------------------------------------
# Step 0: Preflight — confirm the target systemd unit actually exists so we
# never silently restart the wrong unit. (deploy-botho.sh / reset-chain.sh /
# reset-to-testnet.sh disagree on botho vs botho-seed; this is that drift.)
# ---------------------------------------------------------------------------
log_step "Preflight: verifying systemd unit '$SERVICE_NAME' exists on host..."
if [[ "$DRY_RUN" == "true" ]]; then
    run_remote "systemctl list-unit-files '${SERVICE_NAME}.service' | grep -q '${SERVICE_NAME}.service'"
else
    if ! run_remote_capture "systemctl list-unit-files '${SERVICE_NAME}.service' 2>/dev/null | grep -q '${SERVICE_NAME}.service'"; then
        log_error "systemd unit '${SERVICE_NAME}.service' not found on $HOST."
        log_error "Installed botho units are:"
        run_remote_capture "systemctl list-unit-files | grep -i botho || echo '  (none found)'"
        log_error "Re-run with the correct --service NAME (e.g. --service botho-seed)."
        exit 1
    fi
    log_info "Unit '${SERVICE_NAME}.service' confirmed present."
fi

# ---------------------------------------------------------------------------
# Step 1: Stop the service (idempotent — || true tolerates already-stopped)
# ---------------------------------------------------------------------------
log_step "Stopping $SERVICE_NAME..."
run_remote "sudo systemctl stop $SERVICE_NAME || true"

# ---------------------------------------------------------------------------
# Step 2: Wipe ONLY ledger + wallet. config.toml and node_key are siblings and
# are left untouched (see the data-layout note in the header). We back up
# config.toml first, mirroring reset-chain.sh, in case the dir is recreated.
# ---------------------------------------------------------------------------
log_step "Backing up config.toml..."
run_remote "cp ~/$DATA_DIR/config.toml /tmp/botho-config-backup.toml 2>/dev/null || true"

log_step "Wiping stale chain data (ledger + wallet; config.toml + node_key preserved)..."
run_remote "rm -rf ~/$DATA_DIR/ledger ~/$DATA_DIR/wallet"

log_step "Ensuring config.toml is in place..."
run_remote "mkdir -p ~/$DATA_DIR && cp /tmp/botho-config-backup.toml ~/$DATA_DIR/config.toml 2>/dev/null || true"

# ---------------------------------------------------------------------------
# Step 3: Deploy the pinned release binary. We delegate to deploy-botho.sh
# rather than duplicating its checksum-verify flow — passing SERVICE_NAME and
# RELEASE_TAG through the environment. deploy-botho.sh will stop (already
# stopped), install the binary, and start the service onto the now-empty
# ledger, which is exactly the ordering we need.
# ---------------------------------------------------------------------------
log_step "Deploying release binary via deploy-botho.sh..."
if [[ "$DRY_RUN" == "true" ]]; then
    echo "    SSH_KEY=$SSH_KEY SERVICE_NAME=$SERVICE_NAME RELEASE_TAG=${RELEASE_TAG:-<latest>} \\"
    echo "        $SCRIPT_DIR/deploy-botho.sh $HOST"
else
    SSH_KEY="$SSH_KEY" SERVICE_NAME="$SERVICE_NAME" ${RELEASE_TAG:+RELEASE_TAG="$RELEASE_TAG"} \
        "$SCRIPT_DIR/deploy-botho.sh" "$HOST"
fi

# ---------------------------------------------------------------------------
# Step 4: Verify re-peering + convergence via the relay's LOCAL RPC. We poll
# the LOCAL RPC (not public HTTPS) per TESTNET_RESET.md §7 so nginx/DNS caching
# can't mask a wedge. Success = peerCount > 0 AND the relay's tipHash matches
# the validator's tipHash.
# ---------------------------------------------------------------------------
log_step "Verifying re-peering + tip convergence (timeout ${TIMEOUT}s)..."
if [[ "$DRY_RUN" == "true" ]]; then
    log_info "Would poll relay LOCAL RPC and compare to validator until convergence:"
    run_remote "curl -s localhost:$RPC_PORT/rpc -H 'content-type: application/json' -d '$NODE_STATUS_REQ'"
    echo "    curl -s $VALIDATOR_RPC -H 'content-type: application/json' -d '$NODE_STATUS_REQ'"
    log_info "Dry run complete. No changes were made."
    : "${host_set}"
    exit 0
fi

deadline=$(( $(date +%s) + TIMEOUT ))
last_local_peers="?"
last_local_tip="?"
last_val_tip="?"
converged=false
while [[ "$(date +%s)" -lt "$deadline" ]]; do
    local_json="$(run_remote_capture "curl -s localhost:$RPC_PORT/rpc -H 'content-type: application/json' -d '$NODE_STATUS_REQ'" || true)"
    val_json="$(curl -s "$VALIDATOR_RPC" -H 'content-type: application/json' -d "$NODE_STATUS_REQ" || true)"

    last_local_peers="$(echo "$local_json" | jq -r '.result.peerCount // "?"' 2>/dev/null || echo "?")"
    last_local_tip="$(echo "$local_json"  | jq -r '.result.tipHash // "?"'   2>/dev/null || echo "?")"
    local_height="$(echo "$local_json"    | jq -r '.result.chainHeight // "?"' 2>/dev/null || echo "?")"
    last_val_tip="$(echo "$val_json"      | jq -r '.result.tipHash // "?"'   2>/dev/null || echo "?")"
    val_height="$(echo "$val_json"        | jq -r '.result.chainHeight // "?"' 2>/dev/null || echo "?")"

    log_info "relay: peers=$last_local_peers height=$local_height tip=${last_local_tip:0:12}… | validator: height=$val_height tip=${last_val_tip:0:12}…"

    if [[ "$last_local_peers" =~ ^[0-9]+$ && "$last_local_peers" -gt 0 \
          && "$last_local_tip" != "?" && "$last_local_tip" == "$last_val_tip" ]]; then
        converged=true
        break
    fi
    sleep 10
done

if [[ "$converged" == "true" ]]; then
    log_info "Relay re-peered and converged on the validator's tip. Re-provision complete!"
else
    log_error "Relay did not converge within ${TIMEOUT}s."
    log_error "  last relay peerCount: $last_local_peers"
    log_error "  last relay tipHash:   $last_local_tip"
    log_error "  validator tipHash:    $last_val_tip"
    if [[ "$last_local_peers" == "0" ]]; then
        log_error "peerCount is 0 — the relay cannot find peers. The MOST LIKELY cause is"
        log_error "the validator gossip firewall dropping this relay's IP (see #1117): the"
        log_error "validators' iptables lockdown on gossip :17100 is un-persisted and can"
        log_error "silently strand external relays even when the security group looks open."
        log_error "Confirm the relay's public IP is ACCEPTed on every validator's :17100."
    fi
    exit 1
fi
