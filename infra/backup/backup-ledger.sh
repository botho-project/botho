#!/usr/bin/env bash
#
# Automated backup of Botho seed node ledger to S3
#
# Features:
#   - LMDB safe copy using mdb_copy (or file-level copy when stopped)
#   - Compression with zstd (fast, good compression ratio)
#   - Upload to S3 with server-side encryption (SSE-S3)
#   - 30-day retention via S3 lifecycle policy
#   - Verification of backup integrity
#   - CloudWatch custom metric for monitoring
#
# Prerequisites:
#   - AWS CLI configured with appropriate permissions
#   - lmdb-utils installed (for mdb_copy)
#   - zstd installed for compression
#
# Usage:
#   ./backup-ledger.sh [options]
#
# Options:
#   --dry-run       Show what would be done without executing
#   --verify-only   Only verify the latest backup, don't create new one
#   --help          Show this help message
#
# Environment Variables:
#   BOTHO_LEDGER_PATH   Path to ledger database (default: /var/lib/botho/ledger)
#   BOTHO_BACKUP_BUCKET S3 bucket for backups (required)
#   BOTHO_BACKUP_PREFIX S3 key prefix (default: ledger-backups)
#   AWS_REGION          AWS region (default: us-east-1)
#   BOTHO_INSTANCE_ID   Instance ID for CloudWatch metrics (optional)

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Logging functions
log_info()  { echo -e "${GREEN}[INFO]${NC}  $(date '+%Y-%m-%d %H:%M:%S') $1"; }
log_warn()  { echo -e "${YELLOW}[WARN]${NC}  $(date '+%Y-%m-%d %H:%M:%S') $1"; }
log_error() { echo -e "${RED}[ERROR]${NC} $(date '+%Y-%m-%d %H:%M:%S') $1"; }
log_debug() { echo -e "${BLUE}[DEBUG]${NC} $(date '+%Y-%m-%d %H:%M:%S') $1"; }

# Configuration with defaults
LEDGER_PATH="${BOTHO_LEDGER_PATH:-/var/lib/botho/ledger}"
S3_BUCKET="${BOTHO_BACKUP_BUCKET:-}"
S3_PREFIX="${BOTHO_BACKUP_PREFIX:-ledger-backups}"
AWS_REGION="${AWS_REGION:-us-east-1}"
INSTANCE_ID="${BOTHO_INSTANCE_ID:-$(hostname)}"
RETENTION_DAYS=30

# Temp directory for backup staging
BACKUP_STAGING="/tmp/botho-backup-$$"
BACKUP_TIMESTAMP=$(date +%Y%m%d_%H%M%S)
BACKUP_FILENAME="ledger_${BACKUP_TIMESTAMP}.tar.zst"

# Parse command line arguments
DRY_RUN=false
VERIFY_ONLY=false

while [[ $# -gt 0 ]]; do
    case $1 in
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        --verify-only)
            VERIFY_ONLY=true
            shift
            ;;
        --help)
            head -35 "$0" | tail -33
            exit 0
            ;;
        *)
            log_error "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Cleanup function
cleanup() {
    if [[ -d "$BACKUP_STAGING" ]]; then
        log_debug "Cleaning up staging directory: $BACKUP_STAGING"
        rm -rf "$BACKUP_STAGING"
    fi
}
trap cleanup EXIT

# Validate prerequisites
validate_prerequisites() {
    log_info "Validating prerequisites..."

    # Check required commands
    local missing_commands=()

    for cmd in aws zstd tar; do
        if ! command -v "$cmd" &> /dev/null; then
            missing_commands+=("$cmd")
        fi
    done

    if [[ ${#missing_commands[@]} -gt 0 ]]; then
        log_error "Missing required commands: ${missing_commands[*]}"
        log_error "Install with: sudo apt install awscli zstd (or equivalent)"
        exit 1
    fi

    # Check S3 bucket is configured
    if [[ -z "$S3_BUCKET" ]]; then
        log_error "BOTHO_BACKUP_BUCKET environment variable is required"
        exit 1
    fi

    # Check ledger path exists
    if [[ ! -d "$LEDGER_PATH" ]]; then
        log_error "Ledger path does not exist: $LEDGER_PATH"
        exit 1
    fi

    # Check AWS credentials
    if ! aws sts get-caller-identity --region "$AWS_REGION" &> /dev/null; then
        log_error "AWS credentials not configured or invalid"
        exit 1
    fi

    log_info "Prerequisites validated successfully"
}

# Create backup of ledger database
create_backup() {
    log_info "Creating backup of ledger database..."

    mkdir -p "$BACKUP_STAGING/ledger"

    # Check if mdb_copy is available for safe LMDB copy
    if command -v mdb_copy &> /dev/null; then
        log_info "Using mdb_copy for safe LMDB backup..."
        if $DRY_RUN; then
            log_info "[DRY RUN] Would run: mdb_copy $LEDGER_PATH $BACKUP_STAGING/ledger"
        else
            mdb_copy "$LEDGER_PATH" "$BACKUP_STAGING/ledger"
        fi
    else
        # Fallback: direct file copy (safe if node is stopped or using read lock)
        log_warn "mdb_copy not found, using direct file copy"
        log_warn "For hot backups, install lmdb-utils: sudo apt install lmdb-utils"
        if $DRY_RUN; then
            log_info "[DRY RUN] Would copy: $LEDGER_PATH to $BACKUP_STAGING/ledger"
        else
            cp -r "$LEDGER_PATH"/* "$BACKUP_STAGING/ledger/"
        fi
    fi

    # Create compressed tarball
    local backup_path="$BACKUP_STAGING/$BACKUP_FILENAME"
    log_info "Compressing backup with zstd..."

    if $DRY_RUN; then
        log_info "[DRY RUN] Would create: $backup_path"
    else
        tar -C "$BACKUP_STAGING" -cf - ledger | zstd -T0 -10 > "$backup_path"
        local backup_size
        backup_size=$(du -h "$backup_path" | cut -f1)
        log_info "Created backup: $backup_path ($backup_size)"
    fi

    echo "$backup_path"
}

# Upload backup to S3
upload_to_s3() {
    local backup_path="$1"
    local s3_key="${S3_PREFIX}/${BACKUP_FILENAME}"
    local s3_uri="s3://${S3_BUCKET}/${s3_key}"

    log_info "Uploading backup to S3..."
    log_info "Destination: $s3_uri"

    if $DRY_RUN; then
        log_info "[DRY RUN] Would upload: $backup_path -> $s3_uri"
    else
        # Upload with SSE-S3 encryption
        aws s3 cp "$backup_path" "$s3_uri" \
            --region "$AWS_REGION" \
            --sse AES256 \
            --storage-class STANDARD_IA \
            --metadata "created=$(date -Iseconds),instance=$INSTANCE_ID"

        log_info "Upload complete: $s3_uri"
    fi

    echo "$s3_uri"
}

# Verify backup integrity
verify_backup() {
    local s3_uri="${1:-}"

    if [[ -z "$s3_uri" ]]; then
        # Find the latest backup
        log_info "Finding latest backup for verification..."
        s3_uri=$(aws s3 ls "s3://${S3_BUCKET}/${S3_PREFIX}/" \
            --region "$AWS_REGION" \
            | sort -r | head -1 | awk '{print "s3://'"${S3_BUCKET}"'/'"${S3_PREFIX}"'/"$4}')

        if [[ -z "$s3_uri" ]]; then
            log_error "No backups found in bucket"
            return 1
        fi
    fi

    log_info "Verifying backup: $s3_uri"

    if $DRY_RUN; then
        log_info "[DRY RUN] Would verify: $s3_uri"
        return 0
    fi

    # Create temp directory for verification
    local verify_dir="$BACKUP_STAGING/verify"
    mkdir -p "$verify_dir"

    # Download backup
    log_info "Downloading backup for verification..."
    local backup_file="$verify_dir/backup.tar.zst"
    aws s3 cp "$s3_uri" "$backup_file" --region "$AWS_REGION"

    # Verify archive integrity
    log_info "Verifying archive integrity..."
    if ! zstd -t "$backup_file" 2>/dev/null; then
        log_error "Archive integrity check failed"
        return 1
    fi

    # Extract and verify contents
    log_info "Extracting and verifying contents..."
    if ! tar -I zstd -tf "$backup_file" > /dev/null 2>&1; then
        log_error "Archive extraction test failed"
        return 1
    fi

    # Verify LMDB database files exist in archive
    if ! tar -I zstd -tf "$backup_file" | grep -q "ledger/data.mdb"; then
        log_error "Archive does not contain ledger/data.mdb"
        return 1
    fi

    log_info "Backup verification successful"
    return 0
}

# Send CloudWatch custom metric
send_cloudwatch_metric() {
    local metric_name="$1"
    local value="$2"
    local unit="${3:-Count}"

    log_debug "Sending CloudWatch metric: $metric_name = $value"

    if $DRY_RUN; then
        log_info "[DRY RUN] Would send metric: Botho/SeedNode/$metric_name = $value"
        return
    fi

    aws cloudwatch put-metric-data \
        --region "$AWS_REGION" \
        --namespace "Botho/SeedNode" \
        --metric-name "$metric_name" \
        --value "$value" \
        --unit "$unit" \
        --dimensions "InstanceId=$INSTANCE_ID" \
        2>/dev/null || log_warn "Failed to send CloudWatch metric (non-fatal)"
}

# Main backup procedure
run_backup() {
    local start_time
    start_time=$(date +%s)
    local backup_success=0  # CloudWatch convention: 0 = failure, 1 = success

    log_info "=========================================="
    log_info "Botho Seed Node Ledger Backup"
    log_info "=========================================="
    log_info "Timestamp: $(date -Iseconds)"
    log_info "Ledger Path: $LEDGER_PATH"
    log_info "S3 Bucket: $S3_BUCKET"
    log_info "S3 Prefix: $S3_PREFIX"
    log_info "Instance: $INSTANCE_ID"
    if $DRY_RUN; then
        log_warn "DRY RUN MODE - No changes will be made"
    fi
    log_info "=========================================="

    # Validate prerequisites
    validate_prerequisites

    # Create backup
    local backup_path
    backup_path=$(create_backup)

    # Upload to S3
    local s3_uri
    s3_uri=$(upload_to_s3 "$backup_path")

    # Verify backup
    if verify_backup "$s3_uri"; then
        backup_success=1  # Success = 1 for CloudWatch alarm (alarm triggers when < 1)
    fi

    # Calculate duration
    local end_time
    end_time=$(date +%s)
    local duration=$((end_time - start_time))

    # Send metrics
    send_cloudwatch_metric "BackupSuccess" "$backup_success"
    send_cloudwatch_metric "BackupDuration" "$duration" "Seconds"

    # Calculate backup size
    if [[ -f "$backup_path" ]]; then
        local backup_size_bytes
        backup_size_bytes=$(stat -f%z "$backup_path" 2>/dev/null || stat -c%s "$backup_path" 2>/dev/null || echo 0)
        send_cloudwatch_metric "BackupSize" "$backup_size_bytes" "Bytes"
    fi

    log_info "=========================================="
    if [[ $backup_success -eq 1 ]]; then
        log_info "Backup completed successfully in ${duration}s"
        log_info "=========================================="
        return 0
    else
        log_error "Backup completed with errors in ${duration}s"
        log_info "=========================================="
        return 1
    fi
}

# Verify-only mode
run_verify_only() {
    log_info "Running backup verification only..."
    validate_prerequisites

    if verify_backup; then
        log_info "Latest backup is valid"
        return 0
    else
        log_error "Latest backup verification failed"
        return 1
    fi
}

# Entry point
main() {
    if $VERIFY_ONLY; then
        run_verify_only
    else
        run_backup
    fi
}

main "$@"
