#!/usr/bin/env bash
#
# Create CloudWatch alarms for Botho seed node backup monitoring
#
# Prerequisites:
#   - AWS CLI configured with appropriate permissions
#   - SNS topic created for notifications
#
# Usage:
#   ./create-backup-alarms.sh <instance-id> <sns-topic-arn> [region]
#
# Example:
#   ./create-backup-alarms.sh i-03f2b4b35fa7e86ce arn:aws:sns:us-east-1:123456789:botho-ops us-east-1

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
    echo "  instance-id    EC2 instance ID or hostname used in backup metrics"
    echo "  sns-topic-arn  SNS topic ARN for alarm notifications"
    echo "  region         AWS region (default: us-east-1)"
    exit 1
fi

INSTANCE_ID="$1"
SNS_TOPIC_ARN="$2"
AWS_REGION="${3:-us-east-1}"

log_info "Creating CloudWatch backup alarms for: $INSTANCE_ID"
log_info "SNS Topic: $SNS_TOPIC_ARN"
log_info "Region: $AWS_REGION"

# Alarm: Backup failed (BackupSuccess = 0)
log_info "Creating backup failure alarm..."
aws cloudwatch put-metric-alarm \
    --region "$AWS_REGION" \
    --alarm-name "botho-backup-failed" \
    --alarm-description "CRITICAL: Botho ledger backup failed" \
    --metric-name BackupSuccess \
    --namespace Botho/SeedNode \
    --statistic Minimum \
    --period 86400 \
    --threshold 1 \
    --comparison-operator LessThanThreshold \
    --dimensions "Name=InstanceId,Value=$INSTANCE_ID" \
    --evaluation-periods 1 \
    --alarm-actions "$SNS_TOPIC_ARN" \
    --ok-actions "$SNS_TOPIC_ARN" \
    --treat-missing-data breaching

# Alarm: Backup not running (no metric data in 26 hours)
log_info "Creating backup missing alarm..."
aws cloudwatch put-metric-alarm \
    --region "$AWS_REGION" \
    --alarm-name "botho-backup-missing" \
    --alarm-description "WARNING: No backup completed in the last 26 hours" \
    --metric-name BackupSuccess \
    --namespace Botho/SeedNode \
    --statistic SampleCount \
    --period 93600 \
    --threshold 1 \
    --comparison-operator LessThanThreshold \
    --dimensions "Name=InstanceId,Value=$INSTANCE_ID" \
    --evaluation-periods 1 \
    --alarm-actions "$SNS_TOPIC_ARN" \
    --ok-actions "$SNS_TOPIC_ARN" \
    --treat-missing-data breaching

# Alarm: Backup taking too long (> 30 minutes)
log_info "Creating backup duration alarm..."
aws cloudwatch put-metric-alarm \
    --region "$AWS_REGION" \
    --alarm-name "botho-backup-slow" \
    --alarm-description "WARNING: Backup took longer than 30 minutes" \
    --metric-name BackupDuration \
    --namespace Botho/SeedNode \
    --statistic Maximum \
    --period 86400 \
    --threshold 1800 \
    --comparison-operator GreaterThanThreshold \
    --dimensions "Name=InstanceId,Value=$INSTANCE_ID" \
    --evaluation-periods 1 \
    --alarm-actions "$SNS_TOPIC_ARN" \
    --ok-actions "$SNS_TOPIC_ARN" \
    --treat-missing-data notBreaching

# Alarm: Backup size anomaly (< 1MB might indicate empty backup)
log_info "Creating backup size alarm..."
aws cloudwatch put-metric-alarm \
    --region "$AWS_REGION" \
    --alarm-name "botho-backup-size-anomaly" \
    --alarm-description "WARNING: Backup size is suspiciously small (< 1MB)" \
    --metric-name BackupSize \
    --namespace Botho/SeedNode \
    --statistic Minimum \
    --period 86400 \
    --threshold 1048576 \
    --comparison-operator LessThanThreshold \
    --dimensions "Name=InstanceId,Value=$INSTANCE_ID" \
    --evaluation-periods 1 \
    --alarm-actions "$SNS_TOPIC_ARN" \
    --ok-actions "$SNS_TOPIC_ARN" \
    --treat-missing-data notBreaching

log_info "All backup alarms created successfully!"
echo ""
echo "Created alarms:"
echo "  - botho-backup-failed (CRITICAL: Backup failed)"
echo "  - botho-backup-missing (WARNING: No backup in 26 hours)"
echo "  - botho-backup-slow (WARNING: Backup > 30 minutes)"
echo "  - botho-backup-size-anomaly (WARNING: Backup < 1MB)"
echo ""
echo "Verify alarms in CloudWatch Console:"
echo "  https://$AWS_REGION.console.aws.amazon.com/cloudwatch/home?region=$AWS_REGION#alarmsV2:"
