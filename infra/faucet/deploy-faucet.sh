#!/usr/bin/env bash
#
# Deploy Botho Faucet Node
#
# This script sets up a Botho faucet node on an Ubuntu 22.04 EC2 instance.
# It installs dependencies, builds the Botho binary, configures the node,
# and sets up systemd for auto-restart.
#
# Prerequisites:
#   - Ubuntu 22.04 LTS
#   - Run as root or with sudo
#   - Internet access for package installation
#   - SSH access configured
#
# Usage:
#   sudo ./deploy-faucet.sh [--build-from-source|--use-binary PATH]
#
# Options:
#   --build-from-source   Clone and build Botho from GitHub (default)
#   --use-binary PATH     Use a pre-built binary from the specified path
#

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

log_info() { echo -e "${GREEN}[INFO]${NC} $1"; }
log_warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error() { echo -e "${RED}[ERROR]${NC} $1"; }
log_step() { echo -e "${BLUE}[STEP]${NC} $1"; }

# Configuration
BOTHO_USER="botho"
BOTHO_HOME="/home/$BOTHO_USER"
BOTHO_DATA="$BOTHO_HOME/.botho/testnet"
BOTHO_CONFIG="$BOTHO_DATA/config.toml"
BOTHO_BIN="/usr/local/bin/botho"
SERVICE_FILE="/etc/systemd/system/botho-faucet.service"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BUILD_FROM_SOURCE=true
BINARY_PATH=""

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --build-from-source)
            BUILD_FROM_SOURCE=true
            shift
            ;;
        --use-binary)
            BUILD_FROM_SOURCE=false
            BINARY_PATH="$2"
            shift 2
            ;;
        -h|--help)
            echo "Usage: $0 [--build-from-source|--use-binary PATH]"
            echo ""
            echo "Options:"
            echo "  --build-from-source   Clone and build Botho from GitHub (default)"
            echo "  --use-binary PATH     Use a pre-built binary from the specified path"
            exit 0
            ;;
        *)
            log_error "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Check if running as root
if [[ $EUID -ne 0 ]]; then
    log_error "This script must be run as root (use sudo)"
    exit 1
fi

log_info "Starting Botho Faucet Node deployment..."

# Step 1: Update system and install dependencies
log_step "Installing system dependencies..."
apt-get update
apt-get install -y \
    build-essential \
    curl \
    git \
    pkg-config \
    libssl-dev \
    jq

# Step 2: Install Rust if building from source
if $BUILD_FROM_SOURCE; then
    log_step "Installing Rust toolchain..."
    if ! command -v rustc &> /dev/null; then
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
        source "$HOME/.cargo/env"
    else
        log_info "Rust already installed: $(rustc --version)"
    fi
fi

# Step 3: Create botho user
log_step "Creating botho user..."
if ! id "$BOTHO_USER" &>/dev/null; then
    useradd -r -m -s /bin/bash "$BOTHO_USER"
    log_info "Created user: $BOTHO_USER"
else
    log_info "User $BOTHO_USER already exists"
fi

# Step 4: Build or copy Botho binary
if $BUILD_FROM_SOURCE; then
    log_step "Building Botho from source..."
    cd /tmp
    if [[ -d "botho" ]]; then
        rm -rf botho
    fi
    git clone https://github.com/botho-project/botho.git
    cd botho
    source "$HOME/.cargo/env"
    cargo build --release --bin botho
    cp target/release/botho "$BOTHO_BIN"
    cd /
    rm -rf /tmp/botho
else
    log_step "Installing pre-built binary..."
    if [[ ! -f "$BINARY_PATH" ]]; then
        log_error "Binary not found: $BINARY_PATH"
        exit 1
    fi
    cp "$BINARY_PATH" "$BOTHO_BIN"
fi

chmod 755 "$BOTHO_BIN"
log_info "Installed Botho binary: $BOTHO_BIN"

# Step 5: Create data directories
log_step "Creating data directories..."
mkdir -p "$BOTHO_DATA"
chown -R "$BOTHO_USER:$BOTHO_USER" "$BOTHO_HOME/.botho"
chmod 700 "$BOTHO_HOME/.botho"

# Step 6: Generate wallet mnemonic if not exists
if [[ ! -f "$BOTHO_CONFIG" ]]; then
    log_step "Initializing wallet (generating mnemonic)..."
    sudo -u "$BOTHO_USER" "$BOTHO_BIN" --testnet init
    log_info "Wallet initialized"

    # Backup the mnemonic (IMPORTANT: secure this!)
    log_warn "IMPORTANT: Back up the mnemonic from $BOTHO_CONFIG"
    log_warn "Store it securely - it controls the faucet wallet!"
else
    log_info "Config already exists, skipping wallet initialization"
fi

# Step 7: Configure faucet settings
log_step "Configuring faucet node..."

# Read existing config and update faucet settings
if [[ -f "$BOTHO_CONFIG" ]]; then
    # Create a temporary config with faucet enabled
    # We preserve the existing mnemonic and add faucet config

    # Check if faucet section exists
    if ! grep -q '\[faucet\]' "$BOTHO_CONFIG"; then
        cat >> "$BOTHO_CONFIG" << 'EOF'

[faucet]
enabled = true
amount = 10_000_000_000_000
per_ip_hourly_limit = 5
per_address_daily_limit = 3
daily_limit = 10_000_000_000_000_000
cooldown_secs = 60
EOF
        log_info "Added faucet configuration"
    else
        log_info "Faucet configuration already present"
    fi

    # Check if minting section has enabled = true
    if grep -q 'enabled = false' "$BOTHO_CONFIG" | head -1; then
        sed -i 's/enabled = false/enabled = true/' "$BOTHO_CONFIG"
        log_info "Enabled minting"
    fi

    # Update CORS to allow all origins for testnet
    if grep -q 'cors_origins' "$BOTHO_CONFIG"; then
        log_info "CORS already configured"
    else
        # Add CORS config in network section
        sed -i '/\[network\]/a cors_origins = ["*"]' "$BOTHO_CONFIG" 2>/dev/null || true
    fi
fi

# Set secure permissions on config (contains mnemonic)
chown "$BOTHO_USER:$BOTHO_USER" "$BOTHO_CONFIG"
chmod 600 "$BOTHO_CONFIG"
log_info "Config permissions set to 600"

# Step 8: Install systemd service
log_step "Installing systemd service..."
cp "$SCRIPT_DIR/botho-faucet.service" "$SERVICE_FILE"
chmod 644 "$SERVICE_FILE"

# Step 9: Enable and start service
log_step "Starting Botho faucet service..."
systemctl daemon-reload
systemctl enable botho-faucet
systemctl start botho-faucet

# Wait for service to start
sleep 5

# Step 10: Verify service is running
log_step "Verifying service status..."
if systemctl is-active --quiet botho-faucet; then
    log_info "Botho faucet service is running"
else
    log_error "Botho faucet service failed to start"
    log_info "Check logs: journalctl -u botho-faucet -n 50"
    exit 1
fi

# Step 11: Test faucet endpoint
log_step "Testing faucet endpoint..."
sleep 5  # Give the RPC server time to start

RESPONSE=$(curl -s -X POST http://localhost:17101/ \
    -H "Content-Type: application/json" \
    -d '{"jsonrpc":"2.0","method":"node_getStatus","params":{},"id":1}' 2>/dev/null || echo "FAILED")

if echo "$RESPONSE" | jq -e '.result.chainHeight' > /dev/null 2>&1; then
    HEIGHT=$(echo "$RESPONSE" | jq '.result.chainHeight')
    log_info "Node responding - Chain height: $HEIGHT"
else
    log_warn "Node not responding yet - may still be initializing"
    log_info "Check status with: curl -X POST http://localhost:17101/ -H 'Content-Type: application/json' -d '{\"jsonrpc\":\"2.0\",\"method\":\"node_getStatus\",\"params\":{},\"id\":1}'"
fi

echo ""
log_info "Botho Faucet Node deployment complete!"
echo ""
echo "==========================================="
echo "  Faucet Node Summary"
echo "==========================================="
echo ""
echo "  Service:    botho-faucet"
echo "  User:       $BOTHO_USER"
echo "  Config:     $BOTHO_CONFIG"
echo "  Data:       $BOTHO_DATA"
echo "  Binary:     $BOTHO_BIN"
echo ""
echo "  Ports:"
echo "    P2P:      17100/tcp"
echo "    RPC:      17101/tcp"
echo "    Metrics:  19090/tcp"
echo ""
echo "  Useful commands:"
echo "    View logs:      journalctl -u botho-faucet -f"
echo "    Check status:   systemctl status botho-faucet"
echo "    Restart:        systemctl restart botho-faucet"
echo "    Test endpoint:  curl -X POST http://localhost:17101/ \\"
echo "                    -H 'Content-Type: application/json' \\"
echo "                    -d '{\"jsonrpc\":\"2.0\",\"method\":\"node_getStatus\",\"params\":{},\"id\":1}'"
echo ""
echo "  Next steps:"
echo "    1. Configure DNS: Add A record for faucet.botho.io"
echo "    2. Configure firewall: Open ports 17100, 17101"
echo "    3. Set up monitoring (optional): ./setup-monitoring.sh"
echo "    4. Back up the mnemonic from $BOTHO_CONFIG"
echo ""
