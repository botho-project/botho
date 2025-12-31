#!/usr/bin/env bash
#
# Setup CloudWatch monitoring on Botho seed node
#
# This script installs and configures the CloudWatch agent on an EC2 instance
# running the Botho seed node.
#
# Prerequisites:
#   - Run as root or with sudo
#   - EC2 instance with IAM role including CloudWatchAgentServerPolicy
#   - Internet access for package installation
#
# Usage:
#   sudo ./setup-monitoring.sh
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

# Check if running as root
if [[ $EUID -ne 0 ]]; then
    log_error "This script must be run as root (use sudo)"
    exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONFIG_FILE="$SCRIPT_DIR/cloudwatch-agent-config.json"

# Verify config file exists
if [[ ! -f "$CONFIG_FILE" ]]; then
    log_error "CloudWatch agent config not found: $CONFIG_FILE"
    exit 1
fi

# Detect OS
detect_os() {
    if [[ -f /etc/os-release ]]; then
        . /etc/os-release
        echo "$ID"
    elif [[ -f /etc/redhat-release ]]; then
        echo "rhel"
    else
        echo "unknown"
    fi
}

OS=$(detect_os)
log_info "Detected OS: $OS"

# Step 1: Install CloudWatch Agent
log_step "Installing CloudWatch Agent..."

case "$OS" in
    "amzn"|"rhel"|"centos"|"fedora")
        if command -v amazon-cloudwatch-agent &> /dev/null; then
            log_info "CloudWatch Agent already installed"
        else
            yum install -y amazon-cloudwatch-agent
        fi
        ;;
    "ubuntu"|"debian")
        if command -v amazon-cloudwatch-agent &> /dev/null; then
            log_info "CloudWatch Agent already installed"
        else
            wget -q https://s3.amazonaws.com/amazoncloudwatch-agent/ubuntu/amd64/latest/amazon-cloudwatch-agent.deb
            dpkg -i amazon-cloudwatch-agent.deb
            rm -f amazon-cloudwatch-agent.deb
        fi
        ;;
    *)
        log_error "Unsupported OS: $OS"
        log_info "Please install CloudWatch Agent manually:"
        log_info "  https://docs.aws.amazon.com/AmazonCloudWatch/latest/monitoring/install-CloudWatch-Agent-on-EC2-Instance.html"
        exit 1
        ;;
esac

# Step 2: Create log directory for Botho
log_step "Creating log directories..."
mkdir -p /var/log/botho
chown -R botho:botho /var/log/botho 2>/dev/null || true

# Step 3: Configure CloudWatch Agent
log_step "Configuring CloudWatch Agent..."
cp "$CONFIG_FILE" /opt/aws/amazon-cloudwatch-agent/etc/amazon-cloudwatch-agent.json

# Step 4: Start CloudWatch Agent
log_step "Starting CloudWatch Agent..."
/opt/aws/amazon-cloudwatch-agent/bin/amazon-cloudwatch-agent-ctl \
    -a fetch-config \
    -m ec2 \
    -c file:/opt/aws/amazon-cloudwatch-agent/etc/amazon-cloudwatch-agent.json \
    -s

# Step 5: Enable CloudWatch Agent to start on boot
log_step "Enabling CloudWatch Agent on boot..."
systemctl enable amazon-cloudwatch-agent

# Step 6: Verify agent is running
log_step "Verifying CloudWatch Agent status..."
if systemctl is-active --quiet amazon-cloudwatch-agent; then
    log_info "CloudWatch Agent is running"
else
    log_error "CloudWatch Agent failed to start"
    log_info "Check logs: /var/log/amazon/amazon-cloudwatch-agent/amazon-cloudwatch-agent.log"
    exit 1
fi

# Step 7: Verify metrics are being collected
log_step "Waiting for initial metrics collection..."
sleep 10

# Check agent status
/opt/aws/amazon-cloudwatch-agent/bin/amazon-cloudwatch-agent-ctl -a status

echo ""
log_info "CloudWatch monitoring setup complete!"
echo ""
echo "Next steps:"
echo "  1. Create CloudWatch alarms (run create-alarms.sh)"
echo "  2. Verify metrics in CloudWatch Console"
echo "  3. Subscribe to SNS topic for notifications"
echo ""
echo "Useful commands:"
echo "  Check agent status:  amazon-cloudwatch-agent-ctl -a status"
echo "  View agent logs:     tail -f /var/log/amazon/amazon-cloudwatch-agent/amazon-cloudwatch-agent.log"
echo "  Restart agent:       systemctl restart amazon-cloudwatch-agent"
