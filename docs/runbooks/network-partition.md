# Runbook: Network Partition Recovery

Procedure to diagnose and recover from network partition or isolation.

**Target RTO:** 15-45 minutes
**Severity:** High
**Owner:** Infrastructure

---

## Detection

### Alerts

This runbook is triggered by:
- `botho-seed-network-isolation`: No network traffic for 15 minutes
- Peer count drops to 0
- Node stops receiving new blocks
- Manual report of connectivity issues

### Symptoms

- Node shows 0 peers
- Chain height not advancing
- No gossip traffic on port 7100
- RPC works but node is "stuck"

---

## Diagnosis

### Step 1: Verify the Issue

```bash
# SSH to the node
ssh ec2-user@seed.botho.io

# Check peer count
curl -s http://localhost:7101/ \
  -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"node_getStatus","params":{},"id":1}' | jq '.result.peerCount'

# Check chain height (compare with known good node)
curl -s http://localhost:7101/ \
  -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"node_getStatus","params":{},"id":1}' | jq '.result.chainHeight'

# Check last block time
# If significantly behind, node is partitioned
```

### Step 2: Determine Cause

**Check local network:**
```bash
# Can we reach the internet?
ping -c 3 8.8.8.8

# Can we reach bootstrap peers?
nc -zv 98.95.2.200 7100

# Check listening ports
ss -tlnp | grep botho

# Check firewall
sudo iptables -L -n | grep 7100
sudo ufw status
```

**Check DNS:**
```bash
# Verify DNS resolution
dig seed.botho.io

# Check if IP changed
nslookup seed.botho.io
```

**Check EC2/Cloud:**
```bash
# Security group rules (from AWS CLI locally)
aws ec2 describe-security-groups \
  --group-ids sg-XXXXXXXX \
  --query 'SecurityGroups[0].IpPermissions'

# Network ACLs
aws ec2 describe-network-acls \
  --network-acl-ids acl-XXXXXXXX
```

---

## Recovery Steps

### Case 1: Firewall Blocking

If firewall is blocking gossip traffic:

```bash
# UFW
sudo ufw allow 7100/tcp comment "Botho P2P"
sudo ufw reload

# iptables
sudo iptables -A INPUT -p tcp --dport 7100 -j ACCEPT
sudo iptables -A OUTPUT -p tcp --dport 7100 -j ACCEPT
```

### Case 2: Bootstrap Peers Unreachable

If all bootstrap peers are down:

```bash
# Check current bootstrap peers
grep bootstrap_peers ~/.botho/config.toml

# Try alternative peers (if known)
# Edit config.toml to add alternative peers

# Restart service
sudo systemctl restart botho
```

### Case 3: DNS Issues

If DNS resolution failing:

```bash
# Temporarily use IP directly in config
# Edit ~/.botho/config.toml:
# bootstrap_peers = ["/ip4/98.95.2.200/tcp/7100/p2p/..."]

# Or fix DNS
# Check /etc/resolv.conf
cat /etc/resolv.conf

# Try alternative DNS
echo "nameserver 8.8.8.8" | sudo tee /etc/resolv.conf
```

### Case 4: EC2 Security Group

If AWS security group blocking:

```bash
# Add inbound rule for P2P
aws ec2 authorize-security-group-ingress \
  --group-id sg-XXXXXXXX \
  --protocol tcp \
  --port 7100 \
  --cidr 0.0.0.0/0

# Add outbound rule if restricted
aws ec2 authorize-security-group-egress \
  --group-id sg-XXXXXXXX \
  --protocol tcp \
  --port 7100 \
  --cidr 0.0.0.0/0
```

### Case 5: Process Hung

If the gossip subsystem is hung:

```bash
# Check for high memory/CPU that might cause issues
top -bn1 | grep botho

# Restart the service
sudo systemctl restart botho

# Monitor recovery
sudo journalctl -u botho -f
```

### Case 6: Network Partition (Chain Fork)

If node is on a minority fork:

```bash
# Check if we're on a different chain
# Compare block hash at same height with known good node

# If on wrong fork, resync:
sudo systemctl stop botho
rm -rf ~/.botho/mainnet/ledger
sudo systemctl start botho
```

---

## Verification

After applying fix:

```bash
# 1. Wait 60 seconds for peer discovery
sleep 60

# 2. Check peer count (should be > 0)
curl -s http://localhost:7101/ \
  -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"node_getStatus","params":{},"id":1}' | jq '.result.peerCount'

# 3. Check chain height is advancing
# Run twice with 30 second gap
curl -s http://localhost:7101/ \
  -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"node_getStatus","params":{},"id":1}' | jq '.result.chainHeight'

sleep 30

curl -s http://localhost:7101/ \
  -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"node_getStatus","params":{},"id":1}' | jq '.result.chainHeight'

# 4. Verify gossip traffic
sudo tcpdump -i eth0 port 7100 -c 10

# 5. Check CloudWatch alarm clears
aws cloudwatch describe-alarms \
  --alarm-names "botho-seed-network-isolation" \
  --query 'MetricAlarms[0].StateValue'
```

---

## Quorum Impact

Network partition may affect consensus:

### Check Quorum Status

```bash
# Check if minting is active (if applicable)
curl -s http://localhost:7101/ \
  -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"node_getStatus","params":{},"id":1}' | jq '.result.mintingActive'

# If minting stopped due to quorum loss, it will resume
# automatically when peers reconnect
```

### Quorum Recovery

If using `mode = "recommended"`:
- Quorum rebuilds automatically as peers reconnect
- Minting resumes when `min_peers` threshold met

If using `mode = "explicit"`:
- Ensure specified quorum members are reachable
- Check each member is online and connected

---

## Prevention

### Network Monitoring

Add these CloudWatch metrics:
- Network bytes in/out
- TCP connection count
- Peer count (custom metric)

### Redundancy

- Multiple bootstrap peers in different regions
- DNS failover for seed node
- Multiple seed nodes behind load balancer

### Regular Connectivity Tests

```bash
# Add to monitoring cron
*/5 * * * * /usr/local/bin/check-peers.sh

# check-peers.sh
#!/bin/bash
PEERS=$(curl -s http://localhost:7101/ \
  -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"node_getStatus","params":{},"id":1}' | jq '.result.peerCount')

if [ "$PEERS" -lt 1 ]; then
  echo "ALERT: Zero peers detected" | logger -t botho-monitor
fi
```

---

## Escalation

If network partition persists:

### 15 minutes

- Verify basic connectivity (ping, DNS)
- Check firewall/security groups
- Restart service

### 30 minutes

- Escalate to Infrastructure Lead
- Check if issue affects other nodes
- Consider alternative bootstrap peers

### 45+ minutes

- Investigate broader network issues
- Contact cloud provider if needed
- Consider service notification to users

---

## Related Documentation

- [Configuration Reference](../configuration.md) - Bootstrap peer settings
- [Troubleshooting Guide](../troubleshooting.md#network-issues)
- [Monitoring Guide](../monitoring.md)
