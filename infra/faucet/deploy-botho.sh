#!/usr/bin/env bash
#
# Deploy Botho binary to Faucet Node
#
# This script builds and deploys the Botho binary to the faucet server.
# It builds on the server to ensure correct architecture.
#
# Usage:
#   ./deploy-botho.sh [user@host]
#
# Example:
#   ./deploy-botho.sh ubuntu@faucet.botho.io
#   ./deploy-botho.sh  # Uses default: ubuntu@faucet.botho.io

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
DEFAULT_HOST="ubuntu@faucet.botho.io"
HOST="${1:-$DEFAULT_HOST}"
SSH_KEY="${SSH_KEY:-$HOME/.ssh/botho-nodes.pem}"
BUILD_DIR="/tmp/botho-build"
INSTALL_DIR="/usr/local/bin"
SERVICE_NAME="botho"

# Validate SSH key exists
if [[ ! -f "$SSH_KEY" ]]; then
    log_error "SSH key not found: $SSH_KEY"
    log_info "Set SSH_KEY environment variable or ensure key exists at default location"
    exit 1
fi

SSH_OPTS="-i $SSH_KEY -o StrictHostKeyChecking=accept-new"

log_info "Deploying Botho binary to $HOST"

# Step 1: Check if Rust is installed on server
log_step "Checking Rust installation on server..."
if ! ssh $SSH_OPTS "$HOST" ". ~/.cargo/env 2>/dev/null && cargo --version" &>/dev/null; then
    log_warn "Rust not found on server. Installing..."
    ssh $SSH_OPTS "$HOST" "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y"
fi

# Step 2: Ensure build dependencies
log_step "Checking build dependencies..."
ssh $SSH_OPTS "$HOST" "sudo apt-get update -qq && sudo apt-get install -y -qq build-essential pkg-config libssl-dev" >/dev/null

# Step 3: Clone or update repository
log_step "Updating source code on server..."
if ssh $SSH_OPTS "$HOST" "test -d $BUILD_DIR"; then
    ssh $SSH_OPTS "$HOST" "cd $BUILD_DIR && git fetch origin && git reset --hard origin/main"
else
    ssh $SSH_OPTS "$HOST" "git clone --depth 1 https://github.com/botho-project/botho.git $BUILD_DIR"
fi

# Step 4: Build release binary
log_step "Building release binary (this may take 5-10 minutes)..."
ssh $SSH_OPTS "$HOST" ". ~/.cargo/env && cd $BUILD_DIR && cargo build --release -p botho"

# Step 5: Verify binary
log_step "Verifying build..."
ssh $SSH_OPTS "$HOST" "file $BUILD_DIR/target/release/botho"

# Step 6: Stop service, deploy, restart
log_step "Deploying binary..."
ssh $SSH_OPTS "$HOST" "sudo systemctl stop $SERVICE_NAME || true"
ssh $SSH_OPTS "$HOST" "sudo cp $BUILD_DIR/target/release/botho $INSTALL_DIR/botho && sudo chmod +x $INSTALL_DIR/botho"
ssh $SSH_OPTS "$HOST" "sudo systemctl daemon-reload && sudo systemctl start $SERVICE_NAME"

# Step 7: Verify service
log_step "Verifying service..."
sleep 3
ssh $SSH_OPTS "$HOST" "sudo systemctl status $SERVICE_NAME --no-pager"

log_info "Deployment complete!"
echo ""
echo "To verify:"
echo "  curl -s https://faucet.botho.io/rpc -H 'Content-Type: application/json' -d '{\"jsonrpc\":\"2.0\",\"method\":\"node_getStatus\",\"params\":{},\"id\":1}' | jq .result.network"
