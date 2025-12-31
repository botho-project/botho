# CloudWatch Monitoring

Set up AWS CloudWatch monitoring for Botho seed nodes.

## Overview

This guide covers:
- Installing the CloudWatch agent
- Collecting system and application metrics
- Setting up alarms and notifications
- Troubleshooting monitoring issues

## Prerequisites

- EC2 instance running the Botho seed node
- IAM role with `CloudWatchAgentServerPolicy` attached
- SNS topic for alarm notifications
- AWS CLI configured (for alarm creation)

## Quick Start

```bash
# 1. SSH to seed node
ssh ec2-user@seed.botho.io

# 2. Run setup script
sudo ./infra/monitoring/setup-monitoring.sh

# 3. Create alarms (from local machine with AWS CLI)
./infra/monitoring/create-alarms.sh \
    i-03f2b4b35fa7e86ce \
    arn:aws:sns:us-east-1:123456789:botho-ops \
    us-east-1
```

---

## Installation

### Step 1: Attach IAM Role

The EC2 instance needs an IAM role with CloudWatch permissions.

**Create IAM Role** (if not exists):
1. Go to IAM Console → Roles → Create Role
2. Select "AWS service" → EC2
3. Attach policy: `CloudWatchAgentServerPolicy`
4. Name: `BothoSeedNodeRole`

**Attach to Instance**:
```bash
aws ec2 associate-iam-instance-profile \
    --instance-id i-03f2b4b35fa7e86ce \
    --iam-instance-profile Name=BothoSeedNodeRole
```

### Step 2: Install CloudWatch Agent

**Amazon Linux / RHEL**:
```bash
sudo yum install -y amazon-cloudwatch-agent
```

**Ubuntu / Debian**:
```bash
wget https://s3.amazonaws.com/amazoncloudwatch-agent/ubuntu/amd64/latest/amazon-cloudwatch-agent.deb
sudo dpkg -i amazon-cloudwatch-agent.deb
```

### Step 3: Configure Agent

Copy the configuration file:
```bash
sudo cp infra/monitoring/cloudwatch-agent-config.json \
    /opt/aws/amazon-cloudwatch-agent/etc/amazon-cloudwatch-agent.json
```

### Step 4: Start Agent

```bash
sudo /opt/aws/amazon-cloudwatch-agent/bin/amazon-cloudwatch-agent-ctl \
    -a fetch-config \
    -m ec2 \
    -c file:/opt/aws/amazon-cloudwatch-agent/etc/amazon-cloudwatch-agent.json \
    -s

# Enable on boot
sudo systemctl enable amazon-cloudwatch-agent
```

---

## Metrics Collected

### System Metrics (Namespace: `Botho/SeedNode`)

| Metric | Description | Unit |
|--------|-------------|------|
| `cpu_usage_active` | CPU utilization | Percent |
| `mem_used_percent` | Memory utilization | Percent |
| `disk_used_percent` | Disk usage for `/` | Percent |
| `bytes_sent` / `bytes_recv` | Network I/O | Bytes |
| `tcp_established` | Active TCP connections | Count |

### Process Metrics (Botho-specific)

| Metric | Description | Unit |
|--------|-------------|------|
| `pid_count` | Number of Botho processes | Count |
| `cpu_usage` | Botho CPU usage | Percent |
| `memory_rss` | Botho memory (RSS) | Bytes |
| `num_threads` | Thread count | Count |

---

## Alarms

### Configured Alarms

| Alarm | Threshold | Severity | Action |
|-------|-----------|----------|--------|
| `botho-seed-cpu-high` | CPU > 80% for 10 min | WARNING | SNS notification |
| `botho-seed-memory-high` | Memory > 90% for 10 min | WARNING | SNS notification |
| `botho-seed-disk-high` | Disk > 80% for 10 min | WARNING | SNS notification |
| `botho-seed-process-down` | Process count < 1 for 2 min | CRITICAL | SNS notification |
| `botho-seed-status-check-failed` | EC2 status check failed | CRITICAL | SNS notification |
| `botho-seed-network-isolation` | No network traffic for 15 min | WARNING | SNS notification |

### Create Alarms

```bash
./infra/monitoring/create-alarms.sh <instance-id> <sns-topic-arn> [region]

# Example
./infra/monitoring/create-alarms.sh \
    i-03f2b4b35fa7e86ce \
    arn:aws:sns:us-east-1:123456789:botho-ops \
    us-east-1
```

### Custom Alarm Thresholds

To modify thresholds, edit `create-alarms.sh` or create alarms manually:

```bash
# Example: More aggressive CPU alarm (70% threshold)
aws cloudwatch put-metric-alarm \
    --alarm-name "botho-seed-cpu-warning" \
    --metric-name CPUUtilization \
    --namespace AWS/EC2 \
    --threshold 70 \
    --comparison-operator GreaterThanThreshold \
    --evaluation-periods 2 \
    --period 300 \
    --statistic Average \
    --dimensions "Name=InstanceId,Value=i-03f2b4b35fa7e86ce" \
    --alarm-actions "arn:aws:sns:us-east-1:123456789:botho-ops"
```

---

## SNS Notifications

### Create SNS Topic

```bash
# Create topic
aws sns create-topic --name botho-ops --region us-east-1

# Subscribe email
aws sns subscribe \
    --topic-arn arn:aws:sns:us-east-1:123456789:botho-ops \
    --protocol email \
    --notification-endpoint ops@botho.io

# Subscribe SMS (optional)
aws sns subscribe \
    --topic-arn arn:aws:sns:us-east-1:123456789:botho-ops \
    --protocol sms \
    --notification-endpoint +1234567890
```

### PagerDuty Integration

```bash
# Create HTTPS subscription to PagerDuty
aws sns subscribe \
    --topic-arn arn:aws:sns:us-east-1:123456789:botho-ops \
    --protocol https \
    --notification-endpoint https://events.pagerduty.com/integration/YOUR_KEY/enqueue
```

---

## CloudWatch Dashboard

### Create Dashboard

```bash
aws cloudwatch put-dashboard \
    --dashboard-name BothoSeedNode \
    --dashboard-body file://dashboard.json
```

**Sample Dashboard JSON** (`dashboard.json`):

```json
{
    "widgets": [
        {
            "type": "metric",
            "x": 0, "y": 0, "width": 12, "height": 6,
            "properties": {
                "title": "CPU & Memory",
                "region": "us-east-1",
                "metrics": [
                    ["AWS/EC2", "CPUUtilization", "InstanceId", "i-03f2b4b35fa7e86ce"],
                    ["Botho/SeedNode", "mem_used_percent", "InstanceId", "i-03f2b4b35fa7e86ce"]
                ],
                "period": 300,
                "stat": "Average"
            }
        },
        {
            "type": "metric",
            "x": 12, "y": 0, "width": 12, "height": 6,
            "properties": {
                "title": "Disk Usage",
                "region": "us-east-1",
                "metrics": [
                    ["Botho/SeedNode", "disk_used_percent", "InstanceId", "i-03f2b4b35fa7e86ce", "path", "/"]
                ],
                "period": 300,
                "stat": "Average"
            }
        },
        {
            "type": "metric",
            "x": 0, "y": 6, "width": 12, "height": 6,
            "properties": {
                "title": "Botho Process",
                "region": "us-east-1",
                "metrics": [
                    ["Botho/SeedNode", "pid_count", "InstanceId", "i-03f2b4b35fa7e86ce", "pattern", "botho"],
                    [".", "cpu_usage", ".", ".", ".", "."],
                    [".", "memory_rss", ".", ".", ".", "."]
                ],
                "period": 60,
                "stat": "Average"
            }
        },
        {
            "type": "metric",
            "x": 12, "y": 6, "width": 12, "height": 6,
            "properties": {
                "title": "Network",
                "region": "us-east-1",
                "metrics": [
                    ["AWS/EC2", "NetworkIn", "InstanceId", "i-03f2b4b35fa7e86ce"],
                    [".", "NetworkOut", ".", "."]
                ],
                "period": 300,
                "stat": "Sum"
            }
        }
    ]
}
```

---

## Log Collection

The CloudWatch agent also collects logs from:

| Log File | Log Group | Retention |
|----------|-----------|-----------|
| `/var/log/botho/botho.log` | `/botho/seed-node` | 30 days |
| `/var/log/syslog` | `/botho/seed-node` | 14 days |

### View Logs

```bash
# Via AWS CLI
aws logs tail /botho/seed-node --follow

# Filter for errors
aws logs filter-log-events \
    --log-group-name /botho/seed-node \
    --filter-pattern "ERROR"
```

---

## Verification

### Test Plan

After setup, verify monitoring works:

1. **Verify metrics collection**:
   - Go to CloudWatch Console → Metrics → Botho/SeedNode
   - Confirm metrics appear within 5 minutes

2. **Test CPU alarm**:
   ```bash
   # On seed node
   stress --cpu 4 --timeout 300
   ```
   - Verify WARNING alert triggers within 10 minutes

3. **Test process alarm**:
   ```bash
   # Stop Botho temporarily
   sudo systemctl stop botho
   # Wait 2 minutes, verify CRITICAL alert
   sudo systemctl start botho
   ```

4. **Verify SNS delivery**:
   - Check email/SMS for test alerts
   - Verify OK notifications when conditions clear

5. **Check dashboard**:
   - Open CloudWatch Dashboard
   - Verify all widgets show data

---

## Troubleshooting

### Agent Not Starting

```bash
# Check agent status
amazon-cloudwatch-agent-ctl -a status

# View agent logs
tail -100 /var/log/amazon/amazon-cloudwatch-agent/amazon-cloudwatch-agent.log

# Common issues:
# - Missing IAM role: Attach CloudWatchAgentServerPolicy
# - Config syntax error: Validate JSON
# - Permission denied: Run as root
```

### Metrics Not Appearing

```bash
# Verify agent is collecting
cat /opt/aws/amazon-cloudwatch-agent/logs/amazon-cloudwatch-agent.log | grep -i error

# Check namespace
aws cloudwatch list-metrics --namespace Botho/SeedNode

# Verify dimensions
aws cloudwatch list-metrics \
    --namespace Botho/SeedNode \
    --dimensions Name=InstanceId,Value=i-03f2b4b35fa7e86ce
```

### Alarms Not Triggering

```bash
# Check alarm state
aws cloudwatch describe-alarms --alarm-names botho-seed-cpu-high

# Verify metric data exists
aws cloudwatch get-metric-statistics \
    --namespace AWS/EC2 \
    --metric-name CPUUtilization \
    --dimensions Name=InstanceId,Value=i-03f2b4b35fa7e86ce \
    --start-time $(date -u -d '1 hour ago' +%Y-%m-%dT%H:%M:%SZ) \
    --end-time $(date -u +%Y-%m-%dT%H:%M:%SZ) \
    --period 300 \
    --statistics Average
```

### Process Monitoring Issues

If `pid_count` always shows 0:

```bash
# Verify process name matches
ps aux | grep botho

# Test procstat pattern
/opt/aws/amazon-cloudwatch-agent/bin/amazon-cloudwatch-agent -test \
    -config /opt/aws/amazon-cloudwatch-agent/etc/amazon-cloudwatch-agent.json
```

---

## Configuration Reference

### CloudWatch Agent Config

Location: `/opt/aws/amazon-cloudwatch-agent/etc/amazon-cloudwatch-agent.json`

Source: `infra/monitoring/cloudwatch-agent-config.json`

Key sections:
- `agent`: Global settings (collection interval, run user)
- `metrics.metrics_collected`: System metrics (CPU, memory, disk, network)
- `metrics.metrics_collected.procstat`: Process-specific metrics
- `logs`: Log file collection

### Alarm Scripts

Location: `infra/monitoring/`

| Script | Purpose |
|--------|---------|
| `setup-monitoring.sh` | Install and configure CloudWatch agent |
| `create-alarms.sh` | Create CloudWatch alarms |
| `cloudwatch-agent-config.json` | Agent configuration |

---

## Cost Optimization

CloudWatch costs can add up. Optimize by:

1. **Reduce metric frequency**: Change `metrics_collection_interval` from 60 to 300 seconds
2. **Limit log retention**: Set appropriate retention periods
3. **Use alarm actions wisely**: Avoid excessive SNS notifications
4. **Clean up unused alarms**: Delete alarms for terminated instances

**Estimated monthly cost** (us-east-1):
- Custom metrics (10): ~$3.00
- Alarms (6): ~$0.60
- Logs (10 GB): ~$5.00
- **Total**: ~$10/month per node

---

## See Also

- [Deployment Guide](deployment.md) - Full production setup
- [Backup Strategy](backup.md) - Data protection
- [AWS CloudWatch Documentation](https://docs.aws.amazon.com/AmazonCloudWatch/latest/monitoring/)
