#!/usr/bin/env bash
#
# Reset Blockchain Data on Seed Node
#
# This script stops the Botho service, clears all blockchain data,
# and restarts the service. Use with caution - this is destructive!
#
# Usage:
#   ./reset-chain.sh [user@host]
#
# Options:
#   --force    Skip confirmation prompt
#
# Example:
#   ./reset-chain.sh ubuntu@seed.botho.io
#   ./reset-chain.sh --force  # Skip confirmation

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

# Configuration
DEFAULT_HOST="ubuntu@seed.botho.io"
SSH_KEY="${SSH_KEY:-$HOME/.ssh/botho-nodes.pem}"
SERVICE_NAME="botho"
DATA_DIR=".botho/testnet"
FORCE=false

# Parse arguments
HOST="$DEFAULT_HOST"
for arg in "$@"; do
    case $arg in
        --force)
            FORCE=true
            ;;
        *)
            HOST="$arg"
            ;;
    esac
done

# Validate SSH key exists
if [[ ! -f "$SSH_KEY" ]]; then
    log_error "SSH key not found: $SSH_KEY"
    log_info "Set SSH_KEY environment variable or ensure key exists at default location"
    exit 1
fi

SSH_OPTS="-i $SSH_KEY -o StrictHostKeyChecking=accept-new"

# Confirmation
if [[ "$FORCE" != "true" ]]; then
    log_warn "This will DELETE ALL blockchain data on $HOST"
    log_warn "Data directory: ~/$DATA_DIR"
    echo ""
    read -p "Are you sure? Type 'yes' to confirm: " confirm
    if [[ "$confirm" != "yes" ]]; then
        log_info "Aborted."
        exit 0
    fi
fi

log_info "Resetting blockchain data on $HOST"

# Step 1: Stop service
log_step "Stopping Botho service..."
ssh $SSH_OPTS "$HOST" "sudo systemctl stop $SERVICE_NAME || true"

# Step 2: Backup config (just in case)
log_step "Backing up configuration..."
ssh $SSH_OPTS "$HOST" "cp ~/$DATA_DIR/config.toml /tmp/botho-config-backup.toml 2>/dev/null || true"

# Step 3: Clear blockchain data
log_step "Clearing blockchain data..."
ssh $SSH_OPTS "$HOST" "rm -rf ~/$DATA_DIR/ledger ~/$DATA_DIR/blocks ~/$DATA_DIR/state ~/$DATA_DIR/peers.json"

# Step 4: Restore config if needed
log_step "Ensuring configuration exists..."
ssh $SSH_OPTS "$HOST" "mkdir -p ~/$DATA_DIR && cp /tmp/botho-config-backup.toml ~/$DATA_DIR/config.toml 2>/dev/null || true"

# Step 5: Restart service
log_step "Starting Botho service..."
ssh $SSH_OPTS "$HOST" "sudo systemctl daemon-reload && sudo systemctl start $SERVICE_NAME"

# Step 6: Verify
sleep 3
log_step "Verifying service..."
ssh $SSH_OPTS "$HOST" "sudo systemctl status $SERVICE_NAME --no-pager"

log_info "Reset complete! The node will sync from genesis."
