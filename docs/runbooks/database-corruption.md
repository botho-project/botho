# Runbook: Database Corruption Recovery

Procedure to recover from LMDB database corruption.

**Target RTO:** 45-90 minutes
**Severity:** High
**Owner:** Infrastructure

---

## Detection

### Symptoms

Database corruption may manifest as:
- Node crashes on startup with LMDB errors
- "Failed to open database" in logs
- Hash mismatches during block validation
- Unexpected "Invalid block" errors after crash

### Verification

```bash
# Check for error patterns in logs
sudo journalctl -u botho -n 500 | grep -i "lmdb\|database\|corrupt"

# Check LMDB file integrity
mdb_stat -ef ~/.botho/mainnet/ledger/data.mdb

# Check for lock file issues
ls -la ~/.botho/mainnet/ledger/
```

---

## Recovery Options

Choose based on your situation:

| Option | When to Use | Time | Data Loss |
|--------|-------------|------|-----------|
| Option A: Resync | Minor corruption, no backup | 30-60 min | None (blockchain) |
| Option B: Restore | Have recent backup | 15-30 min | Since backup |
| Option C: Repair | Specific table corruption | 45 min | Minimal |

---

## Option A: Full Resync from Network

**Best for:** Complete corruption, no usable backup, fresh start needed.

```bash
# 1. Stop the service
sudo systemctl stop botho

# 2. Backup current state (if recoverable data exists)
cp -r ~/.botho/mainnet/ledger ~/.botho/mainnet/ledger.corrupted.$(date +%Y%m%d)

# 3. Remove corrupted database
rm -rf ~/.botho/mainnet/ledger

# 4. Preserve config (contains mnemonic)
ls -la ~/.botho/config.toml  # Verify it exists

# 5. Start service to resync
sudo systemctl start botho

# 6. Monitor sync progress
sudo journalctl -u botho -f
# Look for: "Synced to block X" messages
```

**Expected Time:** 30-60 minutes depending on chain height.

**Verification:**
```bash
# Check sync progress
curl -s http://localhost:7101/ \
  -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"node_getStatus","params":{},"id":1}' | jq '.result.chainHeight'
```

---

## Option B: Restore from Backup

**Best for:** Have recent backup, minimize sync time.

```bash
# 1. Stop the service
sudo systemctl stop botho

# 2. Remove corrupted database
rm -rf ~/.botho/mainnet/ledger

# 3. Find latest backup
ls -lt /backup/botho/ledger_*.tar.gz | head -5

# 4. Restore from backup
tar -xzf /backup/botho/ledger_latest.tar.gz -C ~/.botho/mainnet/

# 5. Verify integrity
sha256sum -c /backup/botho/ledger_latest.sha256

# 6. Start service
sudo systemctl start botho

# 7. Monitor catch-up sync
sudo journalctl -u botho -f
```

**Note:** The node will sync blocks created since the backup was taken.

---

## Option C: Targeted Repair

**Best for:** Specific database table corruption, want to preserve most data.

### Identify Corrupted Tables

```bash
# Check each LMDB database
for db in blocks metadata utxos address_index key_images tx_index cluster_wealth; do
  echo "Checking $db..."
  mdb_stat -e ~/.botho/mainnet/ledger/data.mdb -s $db 2>&1 || echo "CORRUPTED: $db"
done
```

### Repair Specific Tables

Some tables can be regenerated from others:

| Table | Can Regenerate? | Method |
|-------|-----------------|--------|
| blocks | No | Resync required |
| metadata | Partially | Rebuilt on resync |
| utxos | Yes | Rescan blocks |
| address_index | Yes | Rescan blocks |
| key_images | Yes | Rescan blocks |
| tx_index | Yes | Rescan blocks |
| cluster_wealth | Yes | Rescan blocks |

```bash
# If only index tables are corrupted, trigger rescan
# (Feature may not be available - check docs)
botho run --reindex

# Otherwise, full resync is safest
```

---

## Database Structure Reference

LMDB databases in `~/.botho/mainnet/ledger/`:

```
data.mdb      # Main data file
lock.mdb      # Lock file (safe to delete when stopped)
```

**Key databases:**
| Database | Purpose | Size Impact |
|----------|---------|-------------|
| blocks | Block headers and bodies | Largest |
| metadata | Chain state, height | Small |
| utxos | Unspent outputs | Medium |
| address_index | Address lookups | Medium |
| key_images | Spent tracking | Medium |
| tx_index | TX lookups | Medium |
| cluster_wealth | Cluster tax state | Small |

---

## Verification

After recovery, verify data integrity:

```bash
# 1. Service is running
sudo systemctl status botho

# 2. Chain height is current
curl -s http://localhost:7101/ \
  -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"node_getStatus","params":{},"id":1}' | jq

# 3. Database stats look normal
mdb_stat -ef ~/.botho/mainnet/ledger/data.mdb

# 4. Wallet balance correct (if applicable)
botho balance

# 5. No errors in logs
sudo journalctl -u botho -n 100 | grep -i error
```

---

## Prevention

### Regular Backups

```bash
# Add to crontab for weekly backups
0 3 * * 0 /usr/local/bin/backup-ledger.sh
```

**backup-ledger.sh:**
```bash
#!/bin/bash
BACKUP_DIR=/backup/botho
DATE=$(date +%Y%m%d)

# Stop service briefly for consistent backup
sudo systemctl stop botho
tar -czf $BACKUP_DIR/ledger_$DATE.tar.gz ~/.botho/mainnet/ledger
sha256sum $BACKUP_DIR/ledger_$DATE.tar.gz > $BACKUP_DIR/ledger_$DATE.sha256
sudo systemctl start botho

# Keep only last 4 weekly backups
find $BACKUP_DIR -name "ledger_*.tar.gz" -mtime +28 -delete
```

### Disk Monitoring

Ensure disk never fills (corruption risk):
- CloudWatch alarm at 80% disk usage
- Auto-cleanup of old logs

### Safe Shutdown

Always stop service cleanly:
```bash
sudo systemctl stop botho
# Wait for clean shutdown before any maintenance
```

---

## Escalation

If corruption persists after recovery:

1. **Check for hardware issues**
   - Run disk diagnostics
   - Check system logs for I/O errors
   - Consider instance replacement

2. **Escalate to Infrastructure Lead**
   - Document recovery attempts
   - Provide LMDB diagnostic output
   - Share relevant logs

3. **Consider fresh instance**
   - If repeated corruption
   - Hardware may be failing

---

## Related Documentation

- [Troubleshooting Guide](../troubleshooting.md#database-issues)
- [Backup & Recovery Guide](../backup.md)
- [Deployment Guide](../deployment.md)
