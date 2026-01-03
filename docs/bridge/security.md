# Bridge Security

Security considerations and operational requirements for the BTH Bridge.

## Threat Model

### Assets at Risk

| Asset | Location | Value |
|-------|----------|-------|
| BTH Hot Wallet | Bridge service | Bridge liquidity |
| Ethereum Private Key | Bridge service | wBTH minting authority |
| Solana Keypair | Bridge service | wBTH minting authority |
| Order Database | Bridge service | Order state, rate limits |

### Threat Actors

1. **External Attackers** - Attempting to exploit bridge for profit
2. **Malicious Operators** - Insider threat from bridge operators
3. **Network Attackers** - Chain-level attacks (reorgs, censorship)

## Hot Wallet Security

### Key Storage Requirements

**Production Environment:**
- Keys stored in encrypted files with strong passphrase
- Passphrase loaded from environment variable (not command line)
- File permissions: `chmod 600` (owner read/write only)
- HSM recommended for high-value deployments

**Configuration Example:**
```toml
[bth]
view_key_file = "/secure/bridge/bth_view.enc"
spend_key_file = "/secure/bridge/bth_spend.enc"

[ethereum]
private_key_file = "/secure/bridge/eth_key.enc"

[solana]
keypair_file = "/secure/bridge/sol_keypair.enc"
```

### Key Rotation

**Recommended Schedule:**
- Rotate keys every 90 days for normal operations
- Immediate rotation if compromise suspected
- Document rotation in operational runbook

**Rotation Procedure:**
1. Generate new keys on air-gapped machine
2. Transfer new public key to production
3. Migrate liquidity to new addresses
4. Update bridge configuration
5. Verify new keys are operational
6. Securely destroy old key material

### Cold Storage

For significant bridge reserves:
- Maintain cold wallet with majority of liquidity
- Hot wallet holds only operational float (e.g., 24-hour volume)
- Regular rebalancing from cold to hot as needed

## Rate Limiting

### Defense-in-Depth

Rate limits protect against:
- Single actor draining bridge liquidity
- Coordinated attack exhausting reserves
- Exploitation of undiscovered vulnerabilities

### Current Limits

```toml
[bridge]
max_order_amount = 1000000000000000      # 1M BTH max per order
daily_limit_per_address = 100000000000000  # 100k BTH/day per address
global_daily_limit = 10000000000000000    # 10M BTH/day total
```

### Limit Tuning

Adjust limits based on:
- Bridge liquidity (limits < total liquidity)
- Normal usage patterns (limits > 99th percentile)
- Market conditions (tighten during volatility)

## Incident Response

### Severity Levels

| Level | Description | Response Time |
|-------|-------------|---------------|
| P0 | Active exploitation, funds at risk | Immediate |
| P1 | Suspected compromise, service degraded | 1 hour |
| P2 | Anomalous activity, investigation needed | 4 hours |
| P3 | Minor issue, no immediate risk | 24 hours |

### P0: Active Exploitation

**Immediate Actions (within 15 minutes):**
1. **Pause bridge** - Disable new order creation
2. **Revoke keys** - Rotate all signing keys immediately
3. **Freeze contracts** - If supported, pause wBTH contracts
4. **Alert team** - Page all operators

**Investigation (within 1 hour):**
1. Identify attack vector
2. Quantify losses
3. Preserve evidence (logs, database snapshots)
4. Prepare incident report

**Recovery:**
1. Patch vulnerability
2. Deploy new keys
3. Resume service with enhanced monitoring
4. Post-mortem within 48 hours

### P1: Suspected Compromise

**Assessment (within 30 minutes):**
1. Review recent orders for anomalies
2. Check key access logs
3. Verify contract state
4. Monitor for unusual transactions

**Escalation Criteria:**
- Unauthorized transactions detected → Escalate to P0
- No evidence of compromise → Continue monitoring
- Inconclusive → Maintain P1, extend investigation

## Monitoring and Alerting

### Required Metrics

**Order Processing:**
- Orders per hour (total, by type)
- Average order completion time
- Failed orders (count, reasons)
- Orders in each state

**Rate Limiting:**
- Current usage vs limits (per-address, global)
- Addresses approaching limits
- Rate limit rejections

**Chain Health:**
- BTH node sync status
- Ethereum RPC latency
- Confirmation delays

### Alert Thresholds

| Metric | Warning | Critical |
|--------|---------|----------|
| Failed orders/hour | > 5 | > 20 |
| Avg completion time | > 10 min | > 30 min |
| Global limit usage | > 70% | > 90% |
| Per-address limit usage | > 80% | > 95% |
| Chain RPC errors | > 1% | > 5% |

### Example Alerting Configuration

```yaml
alerts:
  - name: high_failure_rate
    condition: failed_orders_1h > 20
    severity: critical
    action: page_oncall

  - name: approaching_global_limit
    condition: global_usage_pct > 90
    severity: warning
    action: slack_notify

  - name: order_stuck
    condition: order_age > 30m AND status NOT IN (completed, released, failed)
    severity: critical
    action: page_oncall
```

## Audit Scope

### Smart Contract Audit

**Ethereum wBTH Contract:**
- [ ] Mint/burn authorization checks
- [ ] Reentrancy protection
- [ ] Integer overflow/underflow
- [ ] Access control (admin functions)
- [ ] Emergency pause mechanism
- [ ] Upgrade safety (if upgradeable)

**Solana wBTH Program:**
- [ ] Account validation
- [ ] Authority checks
- [ ] Rent exemption handling
- [ ] Cross-program invocation safety

### Bridge Service Audit

**Order Processing:**
- [ ] State machine correctness
- [ ] Double-spend prevention
- [ ] Race condition handling
- [ ] Error state recovery

**Key Management:**
- [ ] Key storage security
- [ ] Key loading procedures
- [ ] Signing authorization

**Database:**
- [ ] SQL injection prevention
- [ ] Transaction atomicity
- [ ] Backup and recovery

## Operational Security

### Access Control

**Production Systems:**
- SSH key-based authentication only
- MFA required for all operators
- Principle of least privilege
- Regular access review (quarterly)

**Key Material:**
- Two-person rule for key access
- Hardware security modules (recommended)
- Secure key ceremony for generation
- Documented chain of custody

### Network Security

**Bridge Service:**
- Run in private network (no public IP)
- Load balancer with DDoS protection
- TLS for all external connections
- Firewall rules: explicit allow list

**RPC Endpoints:**
- Use authenticated RPC providers
- Rate limit outbound requests
- Monitor for RPC endpoint compromise

## Disaster Recovery

### Backup Requirements

| Data | Frequency | Retention | Location |
|------|-----------|-----------|----------|
| Order database | Hourly | 30 days | Encrypted off-site |
| Configuration | On change | 90 days | Version control |
| Keys (encrypted) | On rotation | Indefinite | Cold storage |

### Recovery Procedures

**Database Recovery:**
1. Stop bridge service
2. Restore from latest backup
3. Replay any unprocessed orders
4. Verify state consistency
5. Resume service

**Key Recovery:**
1. Retrieve encrypted backup from cold storage
2. Decrypt on air-gapped machine
3. Transfer to production via secure channel
4. Verify key integrity
5. Resume operations

### Business Continuity

**Recovery Time Objectives:**
- P0 incident: Service restored within 4 hours
- Database loss: Restored within 2 hours
- Key compromise: New keys deployed within 1 hour

## Compliance Considerations

### Regulatory Requirements

Depending on jurisdiction, consider:
- KYC/AML requirements for large transfers
- Licensing for money transmission
- Reporting obligations
- Sanctions screening

### Transparency

- Publish bridge addresses for verification
- On-chain proof of reserves (if applicable)
- Regular third-party audits
- Open source bridge contracts

## Related Documentation

- [Architecture](architecture.md) - Bridge system design
- [Monitoring](../operations/monitoring.md) - General monitoring setup
- [Deployment](../operations/deployment.md) - Production deployment guide
