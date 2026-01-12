#!/usr/bin/env bash
#
# Deploy Botho Seed Node Status Page
#
# This script deploys the web files and nginx configuration to the server.
#
# Usage:
#   ./deploy-web.sh [user@host]
#
# Example:
#   ./deploy-web.sh ubuntu@seed.botho.io
#   ./deploy-web.sh  # Uses default: ubuntu@seed.botho.io

set -euo pipefail

# Colors
GREEN='\033[0;32m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info() { echo -e "${GREEN}[INFO]${NC} $1"; }
log_step() { echo -e "${BLUE}[STEP]${NC} $1"; }

# Configuration
DEFAULT_HOST="ubuntu@seed.botho.io"
HOST="${1:-$DEFAULT_HOST}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WEB_DIR="$SCRIPT_DIR/web"
NGINX_CONF="$SCRIPT_DIR/seed-nginx.conf"

log_info "Deploying Seed Node Status Page to $HOST"

# Step 1: Copy web files
log_step "Copying web files..."
ssh "$HOST" "sudo mkdir -p /var/www/seed"
rsync -avz --delete "$WEB_DIR/" "$HOST:/tmp/seed-web/"
ssh "$HOST" "sudo cp -r /tmp/seed-web/* /var/www/seed/ && sudo chown -R www-data:www-data /var/www/seed"

# Step 2: Copy nginx config
log_step "Copying nginx configuration..."
scp "$NGINX_CONF" "$HOST:/tmp/seed-nginx.conf"
ssh "$HOST" "sudo cp /tmp/seed-nginx.conf /etc/nginx/sites-available/seed.botho.io"

# Step 3: Enable site if not already enabled
log_step "Enabling nginx site..."
ssh "$HOST" "sudo ln -sf /etc/nginx/sites-available/seed.botho.io /etc/nginx/sites-enabled/ 2>/dev/null || true"

# Step 4: Create cache directory
log_step "Creating cache directory..."
ssh "$HOST" "sudo mkdir -p /var/cache/nginx/seed && sudo chown www-data:www-data /var/cache/nginx/seed"

# Step 5: Test and reload nginx
log_step "Testing and reloading nginx..."
ssh "$HOST" "sudo nginx -t && sudo systemctl reload nginx"

log_info "Deployment complete!"
echo ""
echo "Status page should now be live at https://seed.botho.io"
echo ""
echo "To verify:"
echo "  curl -s https://seed.botho.io/health"
echo "  curl -s https://seed.botho.io/"
