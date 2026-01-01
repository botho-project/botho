# Runbook: Seed Node Recovery

Procedure to recover a failed Botho seed node.

**Target RTO:** < 1 hour
**Severity:** Critical
**Owner:** Infrastructure

---

## Detection

### Alerts

This runbook is triggered by:
- `botho-seed-process-down`: Process count < 1 for 2 minutes
- `botho-seed-status-check-failed`: EC2 status check failed
- Manual report of node unavailability

### Verification

Confirm the issue before proceeding:

```bash
# Check if node is reachable
nc -zv seed.botho.io 7100

# Check RPC endpoint
curl -s -m 5 http://seed.botho.io:7101/ \
  -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"node_getStatus","params":{},"id":1}'

# SSH to node (if network reachable)
ssh ec2-user@seed.botho.io
```

---

## Recovery Steps

### Step 1: Assess the Situation

```bash
# SSH to the node
ssh ec2-user@seed.botho.io

# Check service status
sudo systemctl status botho

# Check recent logs
sudo journalctl -u botho -n 100 --no-pager

# Check system resources
df -h ~/.botho
free -m
top -bn1 | head -20
```

**Decision Tree:**
- Service stopped unexpectedly → Go to Step 2 (Restart)
- Service running but not responding → Go to Step 3 (Process Recovery)
- Disk full → Go to Step 4 (Disk Cleanup)
- Instance unreachable → Go to Step 5 (Instance Recovery)

### Step 2: Service Restart

If the service stopped unexpectedly:

```bash
# Restart the service
sudo systemctl restart botho

# Wait for startup
sleep 10

# Verify status
sudo systemctl status botho

# Check logs for errors
sudo journalctl -u botho -f
```

**Expected Outcome:**
- Service starts successfully
- Node begins syncing (if behind)
- Peer connections established within 60 seconds

### Step 3: Process Recovery

If the process is hung or unresponsive:

```bash
# Force stop the service
sudo systemctl stop botho
sleep 5

# Kill any remaining processes
sudo pkill -9 -f botho

# Clear any stale locks
sudo rm -f ~/.botho/mainnet/ledger/lock.mdb 2>/dev/null

# Start the service
sudo systemctl start botho

# Monitor startup
sudo journalctl -u botho -f
```

### Step 4: Disk Cleanup

If disk is full:

```bash
# Check disk usage
df -h ~/.botho

# Find large files
du -sh ~/.botho/*

# Clean old logs
sudo journalctl --vacuum-time=3d

# Remove old backups if present
find /backup/botho -name "*.gpg" -mtime +30 -delete

# Restart service after cleanup
sudo systemctl restart botho
```

### Step 5: Instance Recovery

If the EC2 instance is unreachable:

```bash
# From AWS CLI (local machine)

# Check instance status
aws ec2 describe-instance-status \
  --instance-ids i-03f2b4b35fa7e86ce

# If instance is running but unreachable
aws ec2 reboot-instances \
  --instance-ids i-03f2b4b35fa7e86ce

# Wait 5 minutes and retry SSH
sleep 300
ssh ec2-user@seed.botho.io
```

If instance cannot be recovered:

```bash
# Launch new instance from latest AMI
# (Use your IaC tooling or manual launch)

# After new instance is running:
# 1. Install Botho
cargo build --release
sudo cp target/release/botho /usr/local/bin/

# 2. Restore config from backup
gpg -d /backup/botho/config_latest.gpg > ~/.botho/config.toml
chmod 600 ~/.botho/config.toml

# 3. Start service
sudo systemctl start botho

# 4. Wait for sync
# Monitor: sudo journalctl -u botho -f
```

---

## Verification

After recovery, verify the node is healthy:

```bash
# 1. Check service status
sudo systemctl status botho

# 2. Check chain height
curl -s http://localhost:7101/ \
  -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"node_getStatus","params":{},"id":1}' | jq

# 3. Check peer count
# Should be > 0 within 60 seconds

# 4. Verify external connectivity
nc -zv seed.botho.io 7100

# 5. Check monitoring
# Verify CloudWatch alarms return to OK state
aws cloudwatch describe-alarms \
  --alarm-names "botho-seed-process-down" \
  --query 'MetricAlarms[0].StateValue'
```

---

## Escalation

If recovery fails after 30 minutes:

1. **Escalate to Infrastructure Lead**
   - Document all steps attempted
   - Include relevant log excerpts
   - Note current system state

2. **Consider Failover**
   - If secondary seed node exists, ensure it's healthy
   - Update DNS if manual failover needed
   - Notify stakeholders of extended outage

3. **Engage Security**
   - If compromise is suspected
   - If unusual activity in logs
   - If data integrity concerns

---

## Post-Incident

After recovery:

1. **Document the incident**
   - Start time, end time
   - Root cause (if known)
   - Steps taken
   - Time to recovery

2. **Update monitoring**
   - Add alerts for conditions not caught
   - Tune thresholds if too sensitive/insensitive

3. **Review this runbook**
   - Was anything missing?
   - Were steps unclear?
   - Update as needed

---

## Related Documentation

- [Deployment Guide](../deployment.md)
- [Monitoring Guide](../monitoring.md)
- [Troubleshooting Guide](../troubleshooting.md)
