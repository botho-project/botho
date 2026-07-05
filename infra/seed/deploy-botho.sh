#!/usr/bin/env bash
#
# Deploy Botho binary to Seed Node
#
# Prefers published release artifacts from GitHub (reproducible, checksummed;
# see docs/operations/reproducible-builds.md). Pass --build-on-host to build
# from source on the server instead (fallback when no suitable release
# exists, e.g. deploying an untagged commit).
#
# Usage:
#   ./deploy-botho.sh [user@host] [--build-on-host]
#
# Environment:
#   SSH_KEY      SSH key path (default: ~/.ssh/botho-nodes.pem)
#   RELEASE_TAG  Release tag to deploy (default: latest GitHub release)
#
# Example:
#   ./deploy-botho.sh ubuntu@seed.botho.io
#   RELEASE_TAG=v0.3.0 ./deploy-botho.sh
#   ./deploy-botho.sh ubuntu@seed.botho.io --build-on-host

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
REPO="botho-project/botho"
SSH_KEY="${SSH_KEY:-$HOME/.ssh/botho-nodes.pem}"
BUILD_DIR="/tmp/botho-build"
INSTALL_DIR="/usr/local/bin"
SERVICE_NAME="botho"

# Parse arguments: optional host, optional --build-on-host
HOST="$DEFAULT_HOST"
BUILD_ON_HOST=false
for arg in "$@"; do
    case "$arg" in
        --build-on-host) BUILD_ON_HOST=true ;;
        *) HOST="$arg" ;;
    esac
done

# Validate SSH key exists
if [[ ! -f "$SSH_KEY" ]]; then
    log_error "SSH key not found: $SSH_KEY"
    log_info "Set SSH_KEY environment variable or ensure key exists at default location"
    exit 1
fi

SSH_OPTS="-i $SSH_KEY -o StrictHostKeyChecking=accept-new"

# ============================================================================
# Preferred path: install a published release artifact (checksummed)
# ============================================================================
deploy_from_release() {
    # Resolve tag (latest release unless RELEASE_TAG is set)
    local tag="${RELEASE_TAG:-}"
    if [[ -z "$tag" ]]; then
        tag=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
            | grep -m1 '"tag_name"' | cut -d'"' -f4) || true
        if [[ -z "$tag" ]]; then
            log_error "Could not resolve latest release tag from GitHub"
            return 1
        fi
    fi

    # Map server architecture to release artifact platform
    local arch platform
    arch=$(ssh $SSH_OPTS "$HOST" "uname -m")
    case "$arch" in
        aarch64) platform="linux-aarch64" ;;
        x86_64)  platform="linux-x86_64" ;;
        *) log_error "Unsupported server architecture: $arch"; return 1 ;;
    esac

    local base="https://github.com/$REPO/releases/download/$tag"
    log_step "Downloading $tag ($platform) release artifact on server..."
    if ! ssh $SSH_OPTS "$HOST" "set -euo pipefail
        rm -rf /tmp/botho-release && mkdir -p /tmp/botho-release
        cd /tmp/botho-release
        curl -fsSL -o botho.tar.gz '$base/botho-$tag-$platform.tar.gz'
        curl -fsSL -o checksums.txt '$base/checksums-$platform.txt'
        tar xzf botho.tar.gz"; then
        log_error "Release download failed (no $tag release, or missing $platform asset)"
        return 1
    fi

    log_step "Verifying checksums..."
    ssh $SSH_OPTS "$HOST" "cd /tmp/botho-release && sha256sum -c checksums.txt"

    log_step "Deploying binary ($tag)..."
    ssh $SSH_OPTS "$HOST" "sudo systemctl stop $SERVICE_NAME || true"
    ssh $SSH_OPTS "$HOST" "sudo install -m755 /tmp/botho-release/botho $INSTALL_DIR/botho"
    ssh $SSH_OPTS "$HOST" "sudo systemctl daemon-reload && sudo systemctl start $SERVICE_NAME"
}

# ============================================================================
# Fallback: build from source on the server (--build-on-host)
# ============================================================================
deploy_build_on_host() {
    # Step 1: Check if Rust is installed on server
    log_step "Checking Rust installation on server..."
    if ! ssh $SSH_OPTS "$HOST" ". ~/.cargo/env 2>/dev/null && cargo --version" &>/dev/null; then
        log_warn "Rust not found on server. Installing..."
        ssh $SSH_OPTS "$HOST" "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y"
    fi

    # Step 2: Ensure build dependencies
    log_step "Checking build dependencies..."
    ssh $SSH_OPTS "$HOST" "sudo apt-get update -qq && sudo apt-get install -y -qq build-essential cmake pkg-config libssl-dev" >/dev/null

    # Step 3: Clone or update repository
    log_step "Updating source code on server..."
    if ssh $SSH_OPTS "$HOST" "test -d $BUILD_DIR"; then
        ssh $SSH_OPTS "$HOST" "cd $BUILD_DIR && git fetch origin && git reset --hard origin/main"
    else
        ssh $SSH_OPTS "$HOST" "git clone --depth 1 https://github.com/$REPO.git $BUILD_DIR"
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
}

log_info "Deploying Botho binary to $HOST"

if [[ "$BUILD_ON_HOST" == "true" ]]; then
    log_warn "Building from source on server (--build-on-host); prefer release artifacts when a suitable tag exists"
    deploy_build_on_host
else
    if ! deploy_from_release; then
        log_error "Artifact deploy failed. Re-run with --build-on-host to build from source instead."
        exit 1
    fi
fi

# Verify service
log_step "Verifying service..."
sleep 3
ssh $SSH_OPTS "$HOST" "sudo systemctl status $SERVICE_NAME --no-pager"

log_info "Deployment complete!"
echo ""
echo "To verify:"
echo "  curl -s https://seed.botho.io/rpc -H 'Content-Type: application/json' -d '{\"jsonrpc\":\"2.0\",\"method\":\"node_getStatus\",\"params\":{},\"id\":1}' | jq .result.network"
