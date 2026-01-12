#!/usr/bin/env bash
#
# Reset Botho Seed Node to Testnet
#
# This script:
# 1. Stops the botho service
# 2. Removes incorrect mainnet data
# 3. Installs the correct systemd service
# 4. Starts the node on testnet
#
# Usage:
#   ./reset-to-testnet.sh
#
# Run this on seed.botho.io as ubuntu user with sudo access.

set -euo pipefail

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

log_info() { echo -e "${GREEN}[INFO]${NC} $1"; }
log_warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error() { echo -e "${RED}[ERROR]${NC} $1"; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DATA_DIR="$HOME/.botho"

echo ""
echo "========================================"
echo "  Botho Seed Node - Reset to Testnet"
echo "========================================"
echo ""

# Confirmation
log_warn "This will DELETE all mainnet data and restart as testnet."
log_warn "Data directory: $DATA_DIR"
echo ""
read -p "Are you sure you want to proceed? (yes/no): " confirm
if [[ "$confirm" != "yes" ]]; then
    log_info "Aborted."
    exit 0
fi

# Step 1: Stop services
log_info "Stopping botho services..."
sudo systemctl stop botho-seed 2>/dev/null || true
sudo systemctl stop botho 2>/dev/null || true
sudo systemctl stop botho-faucet 2>/dev/null || true

# Wait for processes to stop
sleep 2

# Check if any botho process is still running
if pgrep -x botho > /dev/null; then
    log_warn "Botho process still running, killing..."
    sudo pkill -9 botho || true
    sleep 1
fi

# Step 2: Remove mainnet data
log_info "Removing mainnet data..."
if [[ -d "$DATA_DIR/mainnet" ]]; then
    rm -rf "$DATA_DIR/mainnet"
    log_info "Removed $DATA_DIR/mainnet"
else
    log_info "No mainnet directory found at $DATA_DIR/mainnet"
fi

# Step 3: Install systemd service
log_info "Installing botho-seed systemd service..."
sudo cp "$SCRIPT_DIR/botho-seed.service" /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable botho-seed

# Step 4: Start the service
log_info "Starting botho-seed service..."
sudo systemctl start botho-seed

# Step 5: Verify
sleep 3
if systemctl is-active --quiet botho-seed; then
    log_info "botho-seed service is running!"
else
    log_error "botho-seed service failed to start"
    log_error "Check logs: journalctl -u botho-seed -f"
    exit 1
fi

# Step 6: Verify testnet
log_info "Verifying network configuration..."
sleep 5  # Give RPC time to start

NETWORK=$(curl -s -X POST http://localhost:17101 \
    -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","method":"node_getStatus","params":{},"id":1}' \
    | grep -o '"network":"[^"]*"' | cut -d'"' -f4 || echo "unknown")

if [[ "$NETWORK" == "botho-testnet" ]]; then
    log_info "SUCCESS! Node is running on testnet."
else
    log_warn "Network reported as: $NETWORK"
    log_warn "Expected: botho-testnet"
    log_warn "Check configuration and logs."
fi

echo ""
log_info "Reset complete!"
echo ""
echo "Useful commands:"
echo "  View logs:    journalctl -u botho-seed -f"
echo "  Check status: systemctl status botho-seed"
echo "  Check RPC:    curl -s localhost:17101 -d '{\"jsonrpc\":\"2.0\",\"method\":\"node_getStatus\",\"params\":{},\"id\":1}'"
echo ""
