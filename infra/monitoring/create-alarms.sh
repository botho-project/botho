#!/usr/bin/env bash
#
# Create CloudWatch alarms for Botho seed node monitoring
#
# Prerequisites:
#   - AWS CLI configured with appropriate permissions
#   - SNS topic created for notifications
#
# Usage:
#   ./create-alarms.sh <instance-id> <sns-topic-arn> [region]
#
# Example:
#   ./create-alarms.sh i-03f2b4b35fa7e86ce arn:aws:sns:us-east-1:123456789:botho-ops us-east-1

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

log_info() { echo -e "${GREEN}[INFO]${NC} $1"; }
log_warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error() { echo -e "${RED}[ERROR]${NC} $1"; }

# Validate arguments
if [[ $# -lt 2 ]]; then
    echo "Usage: $0 <instance-id> <sns-topic-arn> [region]"
    echo ""
    echo "Arguments:"
    echo "  instance-id    EC2 instance ID (e.g., i-03f2b4b35fa7e86ce)"
    echo "  sns-topic-arn  SNS topic ARN for alarm notifications"
    echo "  region         AWS region (default: us-east-1)"
    exit 1
fi

INSTANCE_ID="$1"
SNS_TOPIC_ARN="$2"
AWS_REGION="${3:-us-east-1}"

log_info "Creating CloudWatch alarms for instance: $INSTANCE_ID"
log_info "SNS Topic: $SNS_TOPIC_ARN"
log_info "Region: $AWS_REGION"

# Alarm: CPU Utilization > 80% (WARNING)
log_info "Creating CPU utilization alarm..."
aws cloudwatch put-metric-alarm \
    --region "$AWS_REGION" \
    --alarm-name "botho-seed-cpu-high" \
    --alarm-description "WARNING: Seed node CPU utilization exceeds 80%" \
    --metric-name CPUUtilization \
    --namespace AWS/EC2 \
    --statistic Average \
    --period 300 \
    --threshold 80 \
    --comparison-operator GreaterThanThreshold \
    --dimensions "Name=InstanceId,Value=$INSTANCE_ID" \
    --evaluation-periods 2 \
    --alarm-actions "$SNS_TOPIC_ARN" \
    --ok-actions "$SNS_TOPIC_ARN" \
    --treat-missing-data notBreaching

# Alarm: Memory Usage > 90% (WARNING)
log_info "Creating memory utilization alarm..."
aws cloudwatch put-metric-alarm \
    --region "$AWS_REGION" \
    --alarm-name "botho-seed-memory-high" \
    --alarm-description "WARNING: Seed node memory utilization exceeds 90%" \
    --metric-name mem_used_percent \
    --namespace Botho/SeedNode \
    --statistic Average \
    --period 300 \
    --threshold 90 \
    --comparison-operator GreaterThanThreshold \
    --dimensions "Name=InstanceId,Value=$INSTANCE_ID" \
    --evaluation-periods 2 \
    --alarm-actions "$SNS_TOPIC_ARN" \
    --ok-actions "$SNS_TOPIC_ARN" \
    --treat-missing-data notBreaching

# Alarm: Disk Usage > 80% (WARNING)
log_info "Creating disk utilization alarm..."
aws cloudwatch put-metric-alarm \
    --region "$AWS_REGION" \
    --alarm-name "botho-seed-disk-high" \
    --alarm-description "WARNING: Seed node disk utilization exceeds 80%" \
    --metric-name disk_used_percent \
    --namespace Botho/SeedNode \
    --statistic Average \
    --period 300 \
    --threshold 80 \
    --comparison-operator GreaterThanThreshold \
    --dimensions "Name=InstanceId,Value=$INSTANCE_ID" "Name=path,Value=/" \
    --evaluation-periods 2 \
    --alarm-actions "$SNS_TOPIC_ARN" \
    --ok-actions "$SNS_TOPIC_ARN" \
    --treat-missing-data notBreaching

# Alarm: Botho process not running (CRITICAL)
log_info "Creating process monitoring alarm..."
aws cloudwatch put-metric-alarm \
    --region "$AWS_REGION" \
    --alarm-name "botho-seed-process-down" \
    --alarm-description "CRITICAL: Botho process is not running on seed node" \
    --metric-name pid_count \
    --namespace Botho/SeedNode \
    --statistic Minimum \
    --period 60 \
    --threshold 1 \
    --comparison-operator LessThanThreshold \
    --dimensions "Name=InstanceId,Value=$INSTANCE_ID" "Name=pattern,Value=botho" \
    --evaluation-periods 2 \
    --alarm-actions "$SNS_TOPIC_ARN" \
    --ok-actions "$SNS_TOPIC_ARN" \
    --treat-missing-data breaching

# Alarm: StatusCheckFailed (EC2 health check)
log_info "Creating EC2 status check alarm..."
aws cloudwatch put-metric-alarm \
    --region "$AWS_REGION" \
    --alarm-name "botho-seed-status-check-failed" \
    --alarm-description "CRITICAL: EC2 status check failed for seed node" \
    --metric-name StatusCheckFailed \
    --namespace AWS/EC2 \
    --statistic Maximum \
    --period 60 \
    --threshold 1 \
    --comparison-operator GreaterThanOrEqualToThreshold \
    --dimensions "Name=InstanceId,Value=$INSTANCE_ID" \
    --evaluation-periods 2 \
    --alarm-actions "$SNS_TOPIC_ARN" \
    --ok-actions "$SNS_TOPIC_ARN" \
    --treat-missing-data notBreaching

# Alarm: Network connectivity (bytes received)
log_info "Creating network connectivity alarm..."
aws cloudwatch put-metric-alarm \
    --region "$AWS_REGION" \
    --alarm-name "botho-seed-network-isolation" \
    --alarm-description "WARNING: Seed node appears network isolated (no incoming traffic)" \
    --metric-name NetworkIn \
    --namespace AWS/EC2 \
    --statistic Sum \
    --period 300 \
    --threshold 1000 \
    --comparison-operator LessThanThreshold \
    --dimensions "Name=InstanceId,Value=$INSTANCE_ID" \
    --evaluation-periods 3 \
    --alarm-actions "$SNS_TOPIC_ARN" \
    --ok-actions "$SNS_TOPIC_ARN" \
    --treat-missing-data notBreaching

log_info "All alarms created successfully!"
echo ""
echo "Created alarms:"
echo "  - botho-seed-cpu-high (WARNING: CPU > 80%)"
echo "  - botho-seed-memory-high (WARNING: Memory > 90%)"
echo "  - botho-seed-disk-high (WARNING: Disk > 80%)"
echo "  - botho-seed-process-down (CRITICAL: Process not running)"
echo "  - botho-seed-status-check-failed (CRITICAL: EC2 health check)"
echo "  - botho-seed-network-isolation (WARNING: No network traffic)"
echo ""
echo "Verify alarms in CloudWatch Console:"
echo "  https://$AWS_REGION.console.aws.amazon.com/cloudwatch/home?region=$AWS_REGION#alarmsV2:"
