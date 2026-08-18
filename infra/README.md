# infra

Deployment and operations assets for the Botho testnet: node provisioning,
backups, monitoring and the public-facing seed/faucet hosts. Everything here is
operator tooling — none of it is compiled into the node or wallet binaries.

Each subdirectory has its own README with the full procedure; this page is just
the index.

| Directory | What it deploys / operates |
|-----------|----------------------------|
| [`baas/`](baas/) | Cloud-init / EC2 user-data that turns a fresh instance into a working mining node with no manual SSH — the automation behind the managed-Node (BaaS) product |
| [`backup/`](backup/) | Daily seed-node LMDB ledger backup to S3 (systemd timer, zstd compression, retention + CloudWatch failure alarms) |
| [`faucet/`](faucet/) | The testnet faucet host `faucet.botho.io` — node + faucet + metrics services, nginx config and deploy scripts |
| [`grafana/`](grafana/) | Grafana dashboards, provisioning and alerting rules for node metrics |
| [`monitoring/`](monitoring/) | CloudWatch agent configuration and alarm creation for the seed node |
| [`seed/`](seed/) | The primary bootstrap/seed node `seed.botho.io` — service unit, gossip firewall, nginx, chain-reset procedures and the web status page |

## Related

- [`../scripts/`](../scripts/) — repo-level build, release and bridge E2E scripts
- [`../docs/`](../docs/) — protocol and operations documentation
