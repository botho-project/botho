# Seed Node Ledger Backup

Automated daily backup of the Botho seed node LMDB ledger to S3.

## Features

- Daily automated backups via systemd timer
- Compressed with zstd for efficient storage
- Server-side encryption (SSE-S3)
- 30-day retention policy
- Backup verification
- CloudWatch monitoring for failures

## Files

| File | Description |
|------|-------------|
| `backup-ledger.sh` | Main backup script |
| `setup-backup.sh` | One-time setup (installs deps, creates bucket, enables timer) |
| `create-backup-alarms.sh` | Create CloudWatch alarms for monitoring |
| `botho-backup.service` | systemd service unit |
| `botho-backup.timer` | systemd timer (daily at 2 AM UTC) |
| `backup.env.example` | Example environment configuration |

## Quick Start

```bash
# On seed node (requires sudo and AWS CLI)
sudo ./setup-backup.sh my-botho-backups us-east-1

# Create monitoring alarms
./create-backup-alarms.sh i-0123456789abcdef0 arn:aws:sns:us-east-1:123456789:botho-ops
```

## Manual Backup

```bash
# Run backup manually
sudo systemctl start botho-backup

# Or run script directly
sudo /opt/botho/infra/backup/backup-ledger.sh

# Dry run (shows what would happen)
sudo /opt/botho/infra/backup/backup-ledger.sh --dry-run
```

## Verification

```bash
# Verify latest backup integrity
sudo /opt/botho/infra/backup/backup-ledger.sh --verify-only

# List backups in S3
aws s3 ls s3://your-bucket/ledger-backups/

# Check backup logs
journalctl -u botho-backup -n 50
```

## Restore Procedure

See [docs/seed-node-backup.md](../../docs/seed-node-backup.md) for full restore documentation.

Quick restore:

```bash
# Stop the node
sudo systemctl stop botho

# Download latest backup
aws s3 cp s3://your-bucket/ledger-backups/ledger_YYYYMMDD_HHMMSS.tar.zst /tmp/

# Extract backup
cd /var/lib/botho
sudo rm -rf ledger.bak
sudo mv ledger ledger.bak
sudo mkdir ledger
sudo tar -I zstd -xf /tmp/ledger_YYYYMMDD_HHMMSS.tar.zst -C .

# Restart node
sudo systemctl start botho
```

## Documentation

See [docs/seed-node-backup.md](../../docs/seed-node-backup.md) for:
- Detailed restore procedures
- RPO/RTO analysis
- Disaster recovery scenarios
- Verification test procedures
