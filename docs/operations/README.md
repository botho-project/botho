# Botho Operations

This section covers running, maintaining, and operating Botho nodes.

## Configuration & Deployment

| Document | Description |
|----------|-------------|
| [Configuration](configuration.md) | Complete configuration reference |
| [Deployment](deployment.md) | Production deployment (systemd, Docker) |
| [Reproducible Builds](reproducible-builds.md) | Verify binary integrity |

## Monitoring & Performance

| Document | Description |
|----------|-------------|
| [Monitoring](monitoring.md) | Metrics, alerting, and dashboards |
| [Memory Budget](memory-budget.md) | Memory tuning and optimization |
| [Performance](phase2_performance.md) | Performance benchmarks and tuning |

## Backup & Recovery

| Document | Description |
|----------|-------------|
| [Backup](backup.md) | Wallet backup procedures |
| [Disaster Recovery](disaster-recovery.md) | Recovery procedures and RTO/RPO |
| [Seed Node Backup](seed-node-backup.md) | Seed node specific backup |
| [Troubleshooting](troubleshooting.md) | Common issues and solutions |

## Runbooks

Emergency response procedures:

| Runbook | Scenario | Priority |
|---------|----------|----------|
| [Seed Node Recovery](runbooks/seed-node-recovery.md) | Seed node failure | Critical |
| [Database Corruption](runbooks/database-corruption.md) | LMDB corruption | High |
| [Key Compromise](runbooks/key-compromise.md) | Suspected key leak | Critical |
| [Network Partition](runbooks/network-partition.md) | Network isolation | High |

## Related

- [Security Guide](../concepts/security.md) - Security best practices
- [Getting Started](../getting-started.md) - Initial setup
