# Seed Node Backup & Recovery

Automated backup and disaster recovery for Botho seed node ledger data.

## Overview

The seed node maintains the authoritative copy of the blockchain ledger. This guide covers:
- Automated daily backups to S3
- Recovery Point Objective (RPO): 24 hours
- Recovery Time Objective (RTO): < 1 hour
- Backup verification procedures

## Backup Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                        Seed Node                                 │
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────┐       │
│  │ LMDB Ledger  │───>│ backup-ledger│───>│    zstd      │       │
│  │ /var/lib/    │    │    .sh       │    │ compression  │       │
│  │ botho/ledger │    └──────────────┘    └──────┬───────┘       │
│  └──────────────┘                               │               │
└─────────────────────────────────────────────────│───────────────┘
                                                  │
                                                  ▼
┌─────────────────────────────────────────────────────────────────┐
│                         AWS S3                                   │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │ s3://bucket/ledger-backups/                              │   │
│  │   ├── ledger_20241230_020000.tar.zst                     │   │
│  │   ├── ledger_20241231_020000.tar.zst                     │   │
│  │   └── ...                                                │   │
│  │                                                          │   │
│  │ Features:                                                │   │
│  │   • SSE-S3 encryption at rest                            │   │
│  │   • 30-day lifecycle policy                              │   │
│  │   • Versioning enabled                                   │   │
│  │   • Public access blocked                                │   │
│  └──────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

## What Gets Backed Up

| Item | Path | Size | Notes |
|------|------|------|-------|
| Ledger Database | `/var/lib/botho/ledger/` | ~1-50 GB | LMDB files (data.mdb, lock.mdb) |

**Not backed up** (can be regenerated):
- Configuration files (contain secrets - back up separately)
- Log files
- Peer connection cache

## Setup

### Prerequisites

- AWS CLI configured with appropriate IAM permissions
- sudo access on seed node
- S3 bucket (or permissions to create one)

### Required IAM Permissions

```json
{
    "Version": "2012-10-17",
    "Statement": [
        {
            "Effect": "Allow",
            "Action": [
                "s3:PutObject",
                "s3:GetObject",
                "s3:ListBucket",
                "s3:DeleteObject"
            ],
            "Resource": [
                "arn:aws:s3:::your-backup-bucket",
                "arn:aws:s3:::your-backup-bucket/*"
            ]
        },
        {
            "Effect": "Allow",
            "Action": [
                "cloudwatch:PutMetricData"
            ],
            "Resource": "*",
            "Condition": {
                "StringEquals": {
                    "cloudwatch:namespace": "Botho/SeedNode"
                }
            }
        }
    ]
}
```

### Installation

```bash
# Clone repository or copy scripts
cd /opt/botho

# Run setup (creates bucket, configures timer)
sudo ./infra/backup/setup-backup.sh my-botho-backups us-east-1

# Create CloudWatch alarms for monitoring
./infra/backup/create-backup-alarms.sh \
    i-0123456789abcdef0 \
    arn:aws:sns:us-east-1:123456789:botho-ops \
    us-east-1
```

### Verify Installation

```bash
# Check timer is enabled
systemctl status botho-backup.timer

# Check configuration
cat /etc/botho/backup.env

# Test backup (dry run)
sudo /opt/botho/infra/backup/backup-ledger.sh --dry-run

# Run actual backup
sudo systemctl start botho-backup

# Check backup logs
journalctl -u botho-backup -n 50

# List backups in S3
aws s3 ls s3://your-bucket/ledger-backups/
```

---

## Recovery Procedures

### Scenario 1: Restore to Same Instance

**Use case**: Ledger corruption, accidental deletion, or rollback needed.

```bash
# 1. Stop the node
sudo systemctl stop botho

# 2. List available backups
aws s3 ls s3://your-bucket/ledger-backups/ | tail -10

# 3. Download desired backup
BACKUP_FILE="ledger_20241230_020000.tar.zst"
aws s3 cp "s3://your-bucket/ledger-backups/$BACKUP_FILE" /tmp/

# 4. Backup current ledger (safety)
cd /var/lib/botho
sudo mv ledger ledger.corrupted.$(date +%Y%m%d_%H%M%S)

# 5. Create fresh ledger directory
sudo mkdir ledger
sudo chown botho:botho ledger

# 6. Extract backup
sudo tar -I zstd -xf "/tmp/$BACKUP_FILE" -C /var/lib/botho

# 7. Set permissions
sudo chown -R botho:botho /var/lib/botho/ledger

# 8. Start node
sudo systemctl start botho

# 9. Verify node is syncing
journalctl -u botho -f

# 10. Cleanup
rm "/tmp/$BACKUP_FILE"
```

### Scenario 2: Restore to New Instance

**Use case**: Instance failure, migration, or disaster recovery.

```bash
# On new instance:

# 1. Install Botho
curl -sSL https://get.botho.io | bash

# 2. Configure AWS CLI
aws configure

# 3. List available backups
aws s3 ls s3://your-bucket/ledger-backups/ | tail -10

# 4. Download latest backup
BACKUP_FILE=$(aws s3 ls s3://your-bucket/ledger-backups/ | sort -r | head -1 | awk '{print $4}')
aws s3 cp "s3://your-bucket/ledger-backups/$BACKUP_FILE" /tmp/

# 5. Create directories
sudo mkdir -p /var/lib/botho
sudo chown botho:botho /var/lib/botho

# 6. Extract backup
sudo tar -I zstd -xf "/tmp/$BACKUP_FILE" -C /var/lib/botho

# 7. Create configuration (from secure backup or secrets manager)
sudo mkdir -p /etc/botho
sudo vim /etc/botho/config.toml  # Add your configuration

# 8. Start node
sudo systemctl enable botho
sudo systemctl start botho

# 9. Monitor sync progress
journalctl -u botho -f
```

### Scenario 3: Point-in-Time Recovery

**Use case**: Need to restore to a specific date.

```bash
# 1. List all backups with dates
aws s3 ls s3://your-bucket/ledger-backups/

# Output:
# 2024-12-28 02:15:00  12345678 ledger_20241228_020000.tar.zst
# 2024-12-29 02:14:00  12345679 ledger_20241229_020000.tar.zst
# 2024-12-30 02:16:00  12345680 ledger_20241230_020000.tar.zst

# 2. Download specific backup
aws s3 cp s3://your-bucket/ledger-backups/ledger_20241228_020000.tar.zst /tmp/

# 3. Follow "Restore to Same Instance" steps above
```

---

## Verification

### Test Backup Integrity

Run this monthly to verify backups are restorable:

```bash
# On a test instance (NOT production):

# 1. Download latest backup
BACKUP_FILE=$(aws s3 ls s3://your-bucket/ledger-backups/ | sort -r | head -1 | awk '{print $4}')
aws s3 cp "s3://your-bucket/ledger-backups/$BACKUP_FILE" /tmp/

# 2. Verify archive integrity
zstd -t "/tmp/$BACKUP_FILE"

# 3. Test extraction
mkdir -p /tmp/backup-test
tar -I zstd -tf "/tmp/$BACKUP_FILE"  # List contents
tar -I zstd -xf "/tmp/$BACKUP_FILE" -C /tmp/backup-test

# 4. Verify LMDB database
ls -la /tmp/backup-test/ledger/
# Should contain: data.mdb, lock.mdb

# 5. Optional: Start test node and verify
# (Requires full Botho installation on test instance)

# 6. Cleanup
rm -rf /tmp/backup-test "/tmp/$BACKUP_FILE"
```

### Automated Verification

The backup script includes automatic verification:

```bash
# Verify latest backup
sudo /opt/botho/infra/backup/backup-ledger.sh --verify-only
```

---

## Monitoring

### CloudWatch Metrics

The backup script publishes these metrics to `Botho/SeedNode` namespace:

| Metric | Description | Unit |
|--------|-------------|------|
| `BackupSuccess` | 0 = failed, 1 = success | Count |
| `BackupDuration` | Time to complete backup | Seconds |
| `BackupSize` | Size of compressed backup | Bytes |

### CloudWatch Alarms

Created by `create-backup-alarms.sh`:

| Alarm | Condition | Severity |
|-------|-----------|----------|
| `botho-backup-failed` | BackupSuccess < 1 | CRITICAL |
| `botho-backup-missing` | No backup in 26 hours | WARNING |
| `botho-backup-slow` | Duration > 30 minutes | WARNING |
| `botho-backup-size-anomaly` | Size < 1 MB | WARNING |

### Check Backup Status

```bash
# View recent backup logs
journalctl -u botho-backup --since "24 hours ago"

# Check timer status
systemctl status botho-backup.timer

# List recent backups
aws s3 ls s3://your-bucket/ledger-backups/ | tail -10
```

---

## RPO/RTO Analysis

### Recovery Point Objective (RPO): 24 hours

- Backups run daily at 2:00 AM UTC
- Maximum data loss: ~24 hours of blockchain data
- Mitigation: Run more frequent backups if needed

### Recovery Time Objective (RTO): < 1 hour

| Step | Time Estimate |
|------|---------------|
| Detect failure | 5-15 min (via CloudWatch alarm) |
| Download backup | 5-20 min (depends on size) |
| Extract and restore | 5-15 min |
| Start and verify node | 5-10 min |
| **Total** | **20-60 minutes** |

### Improving RPO

For lower RPO (e.g., 6 hours):

```bash
# Edit timer to run every 6 hours
sudo systemctl edit botho-backup.timer

# Add override:
[Timer]
OnCalendar=*-*-* 00,06,12,18:00:00
```

---

## Troubleshooting

### Backup Fails: "AWS credentials not configured"

```bash
# Check AWS CLI configuration
aws sts get-caller-identity

# If not configured, run:
aws configure
# Or use instance profile (recommended for EC2)
```

### Backup Fails: "Ledger path does not exist"

```bash
# Check ledger path
ls -la /var/lib/botho/ledger/

# If different path, update config:
sudo vim /etc/botho/backup.env
# Set BOTHO_LEDGER_PATH=/correct/path
```

### Backup Fails: "mdb_copy not found"

```bash
# Install LMDB utilities
sudo apt install lmdb-utils  # Debian/Ubuntu
sudo yum install lmdb        # RHEL/CentOS

# The script falls back to file copy if mdb_copy is unavailable,
# but mdb_copy is recommended for hot backups
```

### Restore Fails: "Permission denied"

```bash
# Ensure correct ownership
sudo chown -R botho:botho /var/lib/botho/ledger

# Ensure correct permissions
sudo chmod 755 /var/lib/botho/ledger
sudo chmod 644 /var/lib/botho/ledger/*
```

### Restore Fails: "LMDB: MDB_INVALID"

The database may be corrupted. Try:

```bash
# 1. Use an older backup
aws s3 ls s3://your-bucket/ledger-backups/
# Choose a backup from before the corruption

# 2. If all backups are corrupted, re-sync from network
sudo rm -rf /var/lib/botho/ledger
sudo systemctl start botho
# Node will sync from peers (slow but recovers all data)
```

---

## Security Considerations

### S3 Bucket Security

- Server-side encryption (SSE-S3) enabled
- Public access blocked
- Versioning enabled for accidental deletion protection
- Access limited via IAM policies

### Secrets Management

**Do NOT store in S3:**
- Private keys
- Mnemonic phrases
- API keys

These should be stored separately in:
- AWS Secrets Manager
- HashiCorp Vault
- Encrypted local backup (see [backup.md](backup.md))

### Access Control

Limit backup access to:
- Backup service account (write-only to S3)
- Operations team (read-only from S3)
- Emergency recovery account (full access)

---

## Related Documentation

- [Monitoring Guide](monitoring.md) — CloudWatch monitoring setup
- [Wallet Backup Guide](backup.md) — User wallet backup (mnemonic)
- [Security Guide](../concepts/security.md) — Complete security practices
- [Disaster Recovery](disaster-recovery.md) — Seed node operations
