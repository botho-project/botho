#!/usr/bin/env bash
#
# Reset Blockchain Data on Seed Node
#
# This script stops the Botho service, clears all blockchain data (ledger +
# wallet), preserves config.toml, and restarts the service. Use with caution -
# this is destructive!
#
# Usage:
#   ./reset-chain.sh [user@host]
#
# Options:
#   --force            Skip confirmation prompt
#   --dry-run          Print the remote commands that would run, do NOT execute
#   --network NAME     Network data dir to reset (default: testnet)
#   --service NAME     systemd service to control (default: botho-seed)
#   --help, -h         Show this help and exit
#
# Example:
#   ./reset-chain.sh ubuntu@seed.botho.io
#   ./reset-chain.sh --force                 # Skip confirmation
#   ./reset-chain.sh --dry-run               # Show what would happen (no SSH)
#
# Data layout (must match botho/src/config.rs::data_dir / Network::dir_name):
#   ~/.botho/<network>/            base data dir (network = "testnet"|"mainnet")
#   ~/.botho/<network>/ledger/     chain database (deleted on reset)
#   ~/.botho/<network>/wallet/     minting/relay wallet (deleted on reset)
#   ~/.botho/<network>/config.toml node config (preserved on reset)
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
    # Print the leading comment block (lines 2..N up to the first blank-after-header)
    sed -n '2,33p' "$0" | sed 's/^#\s\{0,1\}//'
}

# Configuration
DEFAULT_HOST="ubuntu@seed.botho.io"
SSH_KEY="${SSH_KEY:-$HOME/.ssh/botho-nodes.pem}"
SERVICE_NAME="botho-seed"
NETWORK="testnet"
FORCE=false
DRY_RUN=false

# Parse arguments
HOST="$DEFAULT_HOST"
host_set=false
while [[ $# -gt 0 ]]; do
    case "$1" in
        --force)
            FORCE=true
            ;;
        --dry-run)
            DRY_RUN=true
            ;;
        --network)
            NETWORK="$2"
            shift
            ;;
        --service)
            SERVICE_NAME="$2"
            shift
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

SSH_OPTS_DISPLAY="-i $SSH_KEY -o StrictHostKeyChecking=accept-new"
SSH_OPTS="$SSH_OPTS_DISPLAY"

# Validate SSH key exists (skip in dry-run so it works without credentials)
if [[ "$DRY_RUN" != "true" ]]; then
    if [[ ! -f "$SSH_KEY" ]]; then
        log_error "SSH key not found: $SSH_KEY"
        log_info "Set SSH_KEY environment variable or ensure key exists at default location"
        exit 1
    fi
fi

if [[ "$DRY_RUN" == "true" ]]; then
    log_warn "DRY RUN: no SSH connection will be made; commands are printed only."
fi

# Confirmation
if [[ "$FORCE" != "true" && "$DRY_RUN" != "true" ]]; then
    log_warn "This will DELETE ALL blockchain data on $HOST"
    log_warn "Data directory: ~/$DATA_DIR (ledger + wallet)"
    echo ""
    read -r -p "Are you sure? Type 'yes' to confirm: " confirm
    if [[ "$confirm" != "yes" ]]; then
        log_info "Aborted."
        exit 0
    fi
fi

log_info "Resetting $NETWORK chain data on $HOST (service: $SERVICE_NAME)"

# Step 1: Stop service
log_step "Stopping Botho service ($SERVICE_NAME)..."
run_remote "sudo systemctl stop $SERVICE_NAME || true"

# Step 2: Backup config (just in case)
log_step "Backing up configuration..."
run_remote "cp ~/$DATA_DIR/config.toml /tmp/botho-config-backup.toml 2>/dev/null || true"

# Step 3: Clear blockchain data
# The node writes its chain DB to ~/.botho/<network>/ledger and its wallet to
# ~/.botho/<network>/wallet (see botho/src/config.rs). config.toml is preserved.
log_step "Clearing blockchain data (ledger + wallet)..."
run_remote "rm -rf ~/$DATA_DIR/ledger ~/$DATA_DIR/wallet"

# Step 4: Restore config if needed
log_step "Ensuring configuration exists..."
run_remote "mkdir -p ~/$DATA_DIR && cp /tmp/botho-config-backup.toml ~/$DATA_DIR/config.toml 2>/dev/null || true"

# Step 5: Restart service
log_step "Starting Botho service ($SERVICE_NAME)..."
run_remote "sudo systemctl daemon-reload && sudo systemctl start $SERVICE_NAME"

# Step 6: Verify
if [[ "$DRY_RUN" != "true" ]]; then
    sleep 3
fi
log_step "Verifying service..."
run_remote "sudo systemctl status $SERVICE_NAME --no-pager"

if [[ "$DRY_RUN" == "true" ]]; then
    log_info "Dry run complete. No changes were made."
else
    log_info "Reset complete! The node will mint/sync from a fresh genesis."
fi

# Avoid 'unused variable' lint when host not explicitly provided
: "${host_set}"
