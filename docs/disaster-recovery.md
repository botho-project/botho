# Disaster Recovery

This document defines recovery objectives and procedures for Botho network infrastructure.

## Recovery Objectives

### RTO (Recovery Time Objective)

**Target: < 1 hour**

The maximum acceptable time to restore service after a failure. This includes:
- Detection and alerting
- Diagnosis
- Recovery execution
- Verification

| Component | Target RTO | Method |
|-----------|------------|--------|
| Seed node | < 1 hour | Resync from network or restore from backup |
| Validator | < 1 hour | Rejoin quorum (may vary based on quorum size) |
| Wallet | < 1 hour | Recover from mnemonic backup |
| Exchange scanner | < 30 min | Restart with ledger state |

### RPO (Recovery Point Objective)

**Target: < 5 minutes (last confirmed block)**

The maximum acceptable data loss measured in time. For Botho:
- Block time: ~5 seconds
- Data is replicated across all network nodes
- Wallet state is derived from blockchain (no local state to lose)

| Component | Target RPO | Notes |
|-----------|------------|-------|
| Seed node | < 5 min | State synced from network |
| Validator | < 5 min | Consensus state in latest block |
| Wallet | Last block | Full recovery from mnemonic |
| Exchange scanner | < 5 min | Deposits tracked in ledger |

---

## Recovery Procedures Overview

Each scenario has a dedicated runbook with step-by-step instructions:

| Scenario | Runbook | Estimated Time | Priority |
|----------|---------|----------------|----------|
| Seed node failure | [seed-node-recovery.md](runbooks/seed-node-recovery.md) | 30-60 min | Critical |
| Database corruption | [database-corruption.md](runbooks/database-corruption.md) | 45-90 min | High |
| Key compromise | [key-compromise.md](runbooks/key-compromise.md) | 15-30 min | Critical |
| Network partition | [network-partition.md](runbooks/network-partition.md) | 15-45 min | High |

---

## Emergency Contact List

Maintain this list with current contact information:

| Role | Name | Contact | Backup |
|------|------|---------|--------|
| On-call Engineer | TBD | TBD | TBD |
| Infrastructure Lead | TBD | TBD | TBD |
| Security Lead | TBD | TBD | TBD |
| Project Maintainer | TBD | TBD | TBD |

**Escalation Path:**
1. On-call Engineer (first 15 min)
2. Infrastructure Lead (if unresolved after 15 min)
3. Security Lead (if security-related)
4. Project Maintainer (critical decisions)

---

## Monitoring and Alerting

Recovery procedures integrate with existing monitoring infrastructure:

| Alert | Threshold | Severity | Action |
|-------|-----------|----------|--------|
| `botho-seed-process-down` | Process count < 1 for 2 min | CRITICAL | [Seed Node Recovery](runbooks/seed-node-recovery.md) |
| `botho-seed-status-check-failed` | EC2 status check failed | CRITICAL | [Seed Node Recovery](runbooks/seed-node-recovery.md) |
| `botho-seed-disk-high` | Disk > 80% for 10 min | WARNING | Investigate, may need cleanup |
| `botho-seed-network-isolation` | No traffic for 15 min | WARNING | [Network Partition](runbooks/network-partition.md) |

For CloudWatch alarm configuration, see [docs/monitoring.md](monitoring.md).

---

## Backup Strategy

### What to Back Up

| Item | Frequency | Retention | Location |
|------|-----------|-----------|----------|
| Wallet mnemonic | Once (at creation) | Forever | Offline secure storage |
| config.toml | Daily | 30 days | Encrypted remote backup |
| Ledger snapshot | Weekly | 4 weeks | S3 or equivalent |

### Backup Verification

Verify backups are recoverable:

```bash
# Test mnemonic recovery (on isolated test machine)
botho init --recover
# Enter mnemonic, verify address matches

# Test config decryption
gpg -d /backup/botho/config_latest.gpg > /dev/null

# Test ledger snapshot integrity
sha256sum -c ledger_snapshot.sha256
```

For detailed backup procedures, see [docs/backup.md](backup.md).

---

## DR Testing Schedule

Regular testing ensures procedures work when needed:

### Quarterly DR Tests

| Test | Frequency | Duration | Owner |
|------|-----------|----------|-------|
| Seed node failover | Quarterly | 2 hours | Infrastructure |
| Wallet recovery | Quarterly | 1 hour | Infrastructure |
| Database restore | Quarterly | 2 hours | Infrastructure |
| Full DR simulation | Annually | 4 hours | All teams |

### Test Checklist

**Before Test:**
- [ ] Notify stakeholders
- [ ] Prepare staging environment
- [ ] Document current production state
- [ ] Have rollback plan ready

**During Test:**
- [ ] Record start time
- [ ] Follow runbook exactly
- [ ] Document any deviations
- [ ] Record end time

**After Test:**
- [ ] Calculate actual RTO achieved
- [ ] Identify improvements
- [ ] Update runbooks if needed
- [ ] File test report

### Test Scenarios

1. **Seed Node Recovery Test**
   - Terminate seed node instance
   - Measure time to detect (should be < 2 min)
   - Execute [seed-node-recovery.md](runbooks/seed-node-recovery.md)
   - Verify node syncs to tip
   - Verify peer connections restored

2. **Database Corruption Test**
   - Corrupt test database (staging only)
   - Execute [database-corruption.md](runbooks/database-corruption.md)
   - Verify full recovery
   - Verify data integrity

3. **Wallet Recovery Test**
   - Use test mnemonic (never production!)
   - Execute recovery procedure
   - Verify address matches expected
   - Verify balance after sync

4. **Network Partition Test**
   - Isolate node from network
   - Verify alerting triggers
   - Execute [network-partition.md](runbooks/network-partition.md)
   - Verify reconnection

---

## Component Recovery Details

### Seed Node

**Data Locations:**
- Configuration: `~/.botho/config.toml`
- Ledger: `~/.botho/{network}/ledger/` (LMDB)
- Logs: `/var/log/botho/` or journald

**Key Files:**
- Node initialization: `botho/src/node/mod.rs`
- Ledger storage: `botho/src/ledger/store.rs`
- Bootstrap peers: `botho/src/network/discovery.rs`

**Health Check:**
```bash
curl -s -X POST http://localhost:7101/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"node_getStatus","params":{},"id":1}'
```

### Database (LMDB)

**Databases:**
| Database | Purpose |
|----------|---------|
| blocks | Block headers and bodies |
| metadata | Chain state metadata |
| utxos | Unspent transaction outputs |
| address_index | Address to UTXO mapping |
| key_images | Spent output tracking |
| tx_index | Transaction lookup |
| cluster_wealth | Cluster tax state |

**Integrity Check:**
```bash
# Check LMDB file integrity
mdb_stat -ef ~/.botho/mainnet/ledger/data.mdb
```

### Wallet

**Key Derivation:**
- BIP39 mnemonic (24 words)
- Implementation: `account-keys/src/account_keys.rs`
- Storage: `botho-wallet/src/storage.rs`

**Recovery Process:**
1. Obtain mnemonic from secure backup
2. Run `botho init --recover`
3. Enter 24 words when prompted
4. Wait for blockchain sync
5. Verify balance and addresses

---

## DNS Failover

For high-availability deployments with multiple seed nodes:

### Configuration

```
seed.botho.io → Primary (98.95.2.200)
              → Failover (secondary IP)
```

### Failover Triggers

DNS failover activates when:
- Health check fails 3 consecutive times
- Network isolation detected
- Manual failover initiated

### Integration with Monitoring

CloudWatch alarms trigger Route 53 health checks:
- `botho-seed-process-down` → Health check fails
- `botho-seed-status-check-failed` → Health check fails

---

## Emergency Rollback

For bad releases that cause service degradation:

### Rollback Procedure

1. **Identify the issue**
   ```bash
   # Check current version
   botho --version

   # Review recent logs
   journalctl -u botho -n 500 --no-pager
   ```

2. **Stop the service**
   ```bash
   sudo systemctl stop botho
   ```

3. **Restore previous binary**
   ```bash
   # Binaries are tagged with version
   sudo cp /opt/botho/releases/v0.1.x/botho /usr/local/bin/
   ```

4. **Restore database if needed**
   ```bash
   # Only if data migration occurred
   cp -r /backup/botho/ledger_latest ~/.botho/mainnet/ledger
   ```

5. **Restart service**
   ```bash
   sudo systemctl start botho
   sudo systemctl status botho
   ```

6. **Verify recovery**
   ```bash
   # Check health
   curl -s http://localhost:7101/ \
     -X POST -H "Content-Type: application/json" \
     -d '{"jsonrpc":"2.0","method":"node_getStatus","params":{},"id":1}'
   ```

---

## Related Documentation

- [Backup & Recovery Guide](backup.md) — Mnemonic backup procedures
- [Deployment Guide](deployment.md) — Production deployment setup
- [Monitoring Guide](monitoring.md) — CloudWatch alarms and metrics
- [Troubleshooting Guide](troubleshooting.md) — Common issue resolution
- [Configuration Reference](configuration.md) — Network and quorum settings

---

## Document History

| Date | Author | Changes |
|------|--------|---------|
| 2025-12-31 | Builder | Initial version |
