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
4. **Lineage-reset gamers** - Domestic holders trying to bypass the
   cluster-factor anti-hoarding mechanism via the bridge (see below)

## Economic Security: Import Cluster Tagging (ADR 0007)

Beyond custody and key security, the bridge must not become an economic
side-door around Botho's anti-hoarding mechanism, which prices **coin lineage**
(a demurrage / fee / lottery multiplier of 1×–6× derived from the wealth
traceable to a coin's cluster origin). Two vectors arise at the boundary:

- **The entry leak** — external wealth unwrapping in at factor-1 pays no lineage
  premium. This is unavoidable in general (taxing external wealth entry would
  break bridge liquidity), but it is **materially narrowed** below.
- **The round-trip laundromat** — a domestic holder round-trips
  (BTH → wBTH → unwrap → factor-1 BTH) to reset a high-factor lineage to
  background for the price of a wrap.

**Mitigation ([ADR 0007](../decisions/0007-bridge-import-cluster-tagging.md);
normative in whitepaper §11; live as of protocol 5.0.0):** unwrapping does
**not** release a plain factor-1 background coin. The released output is tagged
100% to a **block-epoch import cluster** `c_import(m) = H("bridge-import" ‖ ⌊h/K⌋)`
at an elevated factor `max(F, ClusterFactorCurve(Σ unwrap amounts in the epoch))`,
with:

| Constant | Value | Meaning |
|----------|-------|---------|
| `K` | **17,280 blocks (1 day)** | import epoch length |
| `F` | **1.5×** | import-factor floor |

All unwraps in an epoch share one accumulating cluster (so intra-epoch splitting
is Sybil-resistant — diluting costs wall-clock time, not free splits), and the
factor decays **only by circulation** (≈9 domestic-mixing spends from a 6× flood
to the floor). This **closes the round-trip laundromat** — a round-trip now
degrades lineage instead of resetting it — and **collapses the reset-vector map**
(the bridge is removed from the lineage-reset-door list). The full threat → test
mapping is in the [bridge threat model](../security/bridge-threat-model.md).

## Hot Wallet Security

### Relayer / Submit Key At-Rest Handling (#1077)

The Ethereum **relayer** key (`ethereum.private_key_file` / `..._env`) and the
Solana **submit** key (`solana.keypair_file` / `..._env`) are **not** custody
keys — custody is the Gnosis Safe / Squads threshold per
[ADR 0002](../decisions/0002-bridge-custody-scp-validator-federation.md). They
only pay gas and broadcast the threshold-authorized mint transaction. A
compromised relayer/submit key still enables **gas drain** and **submission
griefing**, so the loader hardens their at-rest handling:

- **Load source.** A key may come from a plaintext file on disk (an explicit
  testnet opt-in) OR from an **environment variable** (`private_key_env` /
  `keypair_env`). When an env-var name is configured it takes **precedence** over
  the file, so a mainnet deployment never needs a plaintext key file on disk —
  the secret is injected at runtime by a secrets manager, a systemd
  `LoadCredential=`, or an OS keyring that exports into the process environment.
  A configured-but-unset var **fails closed** (no silent fallback to a file).
- **File-permission preflight.** A group- or world-accessible key file is
  refused when `enforce_key_permissions = true` (recommended for mainnet) and
  otherwise **warned** about (the testnet-compatible default), always logging the
  offending octal mode. Tighten key files to `chmod 600`.
- **Zeroization.** The raw key buffer is wiped from memory after it is parsed
  into the signer. Key material is **never logged** — only the file path, its
  permission mode, and the env-var name appear in messages.

**Production Environment:**
- Prefer the **env-var** load path (`*_env`) so no plaintext key file is on disk
- Set `enforce_key_permissions = true` so an insecure key file is refused
- If a file is used, `chmod 600` (owner read/write only)
- Passphrase / secret loaded from the environment (not command line)
- HSM recommended for high-value deployments

**Configuration Example (mainnet — env-var keys, permission enforcement on):**
```toml
[bth]
view_key_file = "/secure/bridge/bth_view.enc"
spend_key_file = "/secure/bridge/bth_spend.enc"

[ethereum]
# No plaintext key file on disk: the relayer key is injected via the environment
# (e.g. systemd LoadCredential / secrets manager) under this variable name.
private_key_env = "BTH_BRIDGE_ETH_RELAYER_KEY"
enforce_key_permissions = true

[solana]
keypair_env = "BTH_BRIDGE_SOL_SUBMIT_KEY"
enforce_key_permissions = true
```

**Configuration Example (testnet — plaintext file opt-in, still supported):**
```toml
[ethereum]
private_key_file = "/secure/bridge/eth_key.hex"   # chmod 600

[solana]
keypair_file = "/secure/bridge/sol_keypair.json"  # chmod 600
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
max_order_amount = 1000000000000000      # 1,000 BTH max per order
daily_limit_per_address = 100000000000000  # 100 BTH/day per address
global_daily_limit = 10000000000000000    # 10,000 BTH/day total
```

The service-side `global_daily_limit` (10,000 BTH/day) is intentionally
1000× below the wBTH contract's `autoPauseThreshold` (`WrappedBTH.sol:104`,
10M BTH/day): the federation cap is the tight first-line breaker, the
contract threshold is the last-resort on-chain halt if the federation layer
is compromised. Raising the federation cap is an operator decision (#895).

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
