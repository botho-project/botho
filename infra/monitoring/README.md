# CloudWatch Monitoring for Seed Node

Scripts and configuration for AWS CloudWatch monitoring of the Botho seed node.

## Files

| File | Description |
|------|-------------|
| `cloudwatch-agent-config.json` | CloudWatch agent configuration |
| `setup-monitoring.sh` | Install and configure agent on EC2 |
| `create-alarms.sh` | Create CloudWatch alarms |

## Quick Start

```bash
# On seed node (requires sudo)
sudo ./setup-monitoring.sh

# From local machine (requires AWS CLI)
./create-alarms.sh <instance-id> <sns-topic-arn> [region]
```

## Documentation

See [docs/monitoring.md](../../docs/monitoring.md) for full documentation.
