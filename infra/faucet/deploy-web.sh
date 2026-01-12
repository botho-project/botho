#!/usr/bin/env bash
#
# Deploy Botho Faucet Web Page
#
# This script deploys the web files and nginx configuration to the server.
#
# Usage:
#   ./deploy-web.sh [user@host]
#
# Example:
#   ./deploy-web.sh ubuntu@faucet.botho.io
#   ./deploy-web.sh  # Uses default: ubuntu@faucet.botho.io

set -euo pipefail

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info() { echo -e "${GREEN}[INFO]${NC} $1"; }
log_step() { echo -e "${BLUE}[STEP]${NC} $1"; }

# Configuration
DEFAULT_HOST="ubuntu@faucet.botho.io"
HOST="${1:-$DEFAULT_HOST}"
SSH_KEY="${SSH_KEY:-$HOME/.ssh/botho-nodes.pem}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WEB_DIR="$SCRIPT_DIR/web"
NGINX_CONF="$SCRIPT_DIR/faucet-nginx.conf"

# Validate SSH key exists
if [[ ! -f "$SSH_KEY" ]]; then
    echo -e "${RED}[ERROR]${NC} SSH key not found: $SSH_KEY"
    echo -e "${GREEN}[INFO]${NC} Set SSH_KEY environment variable or ensure key exists at default location"
    exit 1
fi

SSH_OPTS="-i $SSH_KEY -o StrictHostKeyChecking=accept-new"

log_info "Deploying Faucet Web Page to $HOST"

# Step 1: Copy web files
log_step "Copying web files..."
ssh $SSH_OPTS "$HOST" "sudo mkdir -p /var/www/faucet"
rsync -avz --delete -e "ssh $SSH_OPTS" "$WEB_DIR/" "$HOST:/tmp/faucet-web/"
ssh $SSH_OPTS "$HOST" "sudo cp -r /tmp/faucet-web/* /var/www/faucet/ && sudo chown -R www-data:www-data /var/www/faucet"

# Step 2: Copy nginx config
log_step "Copying nginx configuration..."
scp $SSH_OPTS "$NGINX_CONF" "$HOST:/tmp/faucet-nginx.conf"
ssh $SSH_OPTS "$HOST" "sudo cp /tmp/faucet-nginx.conf /etc/nginx/sites-available/faucet.botho.io"

# Step 3: Enable site if not already enabled
log_step "Enabling nginx site..."
ssh $SSH_OPTS "$HOST" "sudo ln -sf /etc/nginx/sites-available/faucet.botho.io /etc/nginx/sites-enabled/ 2>/dev/null || true"

# Step 4: Create cache directory
log_step "Creating cache directory..."
ssh $SSH_OPTS "$HOST" "sudo mkdir -p /var/cache/nginx/faucet && sudo chown www-data:www-data /var/cache/nginx/faucet"

# Step 5: Test and reload nginx
log_step "Testing and reloading nginx..."
ssh $SSH_OPTS "$HOST" "sudo nginx -t && sudo systemctl reload nginx"

log_info "Deployment complete!"
echo ""
echo "Faucet page should now be live at https://faucet.botho.io"
echo ""
echo "To verify:"
echo "  curl -s https://faucet.botho.io/health"
echo "  curl -s https://faucet.botho.io/"
