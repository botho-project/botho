# Botho Grafana Dashboard

This directory contains Grafana dashboards and alerting rules for monitoring Botho nodes.

## Prerequisites

- Grafana 10.x or later (tested with 10.0+)
- Prometheus data source configured
- Botho node running with metrics enabled (`--metrics-port 9090`)

## Quick Start

### 1. Start Botho with Metrics

```bash
# Start a Botho node with Prometheus metrics enabled
botho run --metrics-port 9090
```

### 2. Configure Prometheus

Add Botho nodes to your Prometheus configuration:

```yaml
# prometheus.yml
scrape_configs:
  - job_name: 'botho'
    static_configs:
      - targets: ['localhost:9090']
    scrape_interval: 15s
```

### 3. Import Dashboard

#### Option A: Manual Import (Recommended)

1. Open Grafana and navigate to **Dashboards** > **Import**
2. Click **Upload JSON file**
3. Select `dashboards/botho-node.json`
4. Configure the Prometheus data source
5. Click **Import**

#### Option B: Provisioning (Automated)

Copy files to Grafana's provisioning directories:

```bash
# Copy dashboard
cp dashboards/botho-node.json /etc/grafana/provisioning/dashboards/

# Create dashboard provider config
cat > /etc/grafana/provisioning/dashboards/botho.yaml << 'EOF'
apiVersion: 1
providers:
  - name: Botho
    orgId: 1
    folder: Botho
    type: file
    disableDeletion: false
    updateIntervalSeconds: 30
    options:
      path: /etc/grafana/provisioning/dashboards
      foldersFromFilesStructure: false
EOF

# Copy alerting rules
cp provisioning/alerting/botho-alerts.yaml /etc/grafana/provisioning/alerting/
```

Restart Grafana to apply:

```bash
sudo systemctl restart grafana-server
```

## Dashboard Panels

The dashboard is organized into the following sections:

### Node Status
| Panel | Description |
|-------|-------------|
| Block Height | Current blockchain height over time |
| Connected Peers | Number of connected peers (gauge + history) |
| Mempool Size | Transactions waiting in mempool |

### Transactions
| Panel | Description |
|-------|-------------|
| TPS | Transactions per second (5-minute rolling average) |
| Mempool Size History | Mempool size over time with thresholds |

### Latency & Performance
| Panel | Description |
|-------|-------------|
| Validation Latency | Transaction validation latency (p50, p95, p99) |
| Consensus Round Duration | SCP consensus round duration (p50, p95, p99) |

### Counters & Totals
| Panel | Description |
|-------|-------------|
| Total Transactions Processed | Cumulative transaction count |
| Total Blocks Processed | Cumulative block count |

### Economics
| Panel | Description |
|-------|-------------|
| Total Minted Supply | BTH minted over time |
| Total Fees Burned | BTH burned in fees over time |

### Errors & Failures
| Panel | Description |
|-------|-------------|
| Validation Failure Rate | Transaction validation failures per second |
| RPC Error Rate | RPC errors by method |

## Alert Rules

The following alerts are configured in `provisioning/alerting/botho-alerts.yaml`:

| Alert | Severity | Condition | Description |
|-------|----------|-----------|-------------|
| Low Peer Count | Critical | `peers < 3` | Node may be isolated from network |
| Block Height Stale | Critical | No blocks for 10min | Node may be stuck or disconnected |
| High Mempool Size | Warning | `mempool > 900` | High transaction load or slow blocks |
| High Validation Latency | Warning | `p95 > 500ms` | Performance degradation |
| High Validation Failures | Warning | `rate > 1/s` | Possible attack or malformed txs |
| Minting Inactive | Info | `minting == 0` | Informational for validator status |

### Configuring Alert Notifications

1. Navigate to **Alerting** > **Contact points**
2. Create contact points for your notification channels (Email, Slack, PagerDuty, etc.)
3. Navigate to **Alerting** > **Notification policies**
4. Configure routing rules based on alert labels (`severity`, `team`)

Example routing:
- `severity: critical` -> PagerDuty
- `severity: warning` -> Slack #alerts
- `severity: info` -> Email digest

## Dashboard Variables

The dashboard includes template variables for flexible filtering:

| Variable | Description | Default |
|----------|-------------|---------|
| `datasource` | Prometheus data source | Prometheus |
| `instance` | Node instance filter | All |

Use the instance selector to filter panels to specific nodes.

## Metrics Reference

Metrics exported by Botho nodes:

### Gauges
| Metric | Description |
|--------|-------------|
| `botho_peers_connected` | Number of connected peers |
| `botho_mempool_size` | Transactions in mempool |
| `botho_block_height` | Current block height |
| `botho_tps` | Transactions per second (5min avg) |
| `botho_difficulty` | Current mining difficulty |
| `botho_total_minted` | Total minted supply (atomic units) |
| `botho_total_fees_burned` | Total fees burned (atomic units) |
| `botho_minting_active` | Minting status (1=active, 0=inactive) |

### Histograms
| Metric | Description |
|--------|-------------|
| `botho_validation_latency_seconds` | Transaction validation latency |
| `botho_consensus_round_duration_seconds` | SCP consensus round duration |
| `botho_block_processing_seconds` | Block processing latency |

### Counters
| Metric | Description |
|--------|-------------|
| `botho_consensus_nominations_total` | Total SCP nominations |
| `botho_transactions_processed_total` | Total transactions processed |
| `botho_blocks_processed_total` | Total blocks processed |
| `botho_validation_failures_total` | Total validation failures |
| `botho_rpc_requests_total` | RPC requests by method |
| `botho_rpc_errors_total` | RPC errors by method |

## Troubleshooting

### Dashboard shows "No data"

1. Verify Prometheus is scraping the Botho node:
   ```bash
   curl http://localhost:9090/metrics
   ```

2. Check Prometheus targets:
   - Navigate to Prometheus UI > Status > Targets
   - Ensure the Botho target is UP

3. Verify data source configuration in Grafana:
   - Settings > Data Sources > Prometheus
   - Click "Test" to verify connectivity

### Alerts not firing

1. Verify alerting is enabled in Grafana:
   ```ini
   # grafana.ini
   [unified_alerting]
   enabled = true
   ```

2. Check alert rule evaluation:
   - Alerting > Alert rules
   - Click on a rule to see evaluation state

3. Verify contact points are configured:
   - Alerting > Contact points
   - Test notification delivery

### High memory usage in Grafana

For large time ranges, reduce panel query precision:
1. Edit panel > Query options
2. Increase "Min step" to 30s or 1m
3. Reduce "Max data points" if needed

## Related Documentation

- [Botho Monitoring Guide](../../docs/monitoring.md)
- [CloudWatch Monitoring](../monitoring/README.md)
- [Prometheus Documentation](https://prometheus.io/docs/)
- [Grafana Documentation](https://grafana.com/docs/)
