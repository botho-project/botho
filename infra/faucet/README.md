# Botho Faucet Node Deployment

Deploy and operate the testnet faucet node at `faucet.botho.io`.

## Architecture

```
┌─────────────────────────────────┐      ┌─────────────────────────────────┐
│        seed.botho.io            │      │       faucet.botho.io           │
│         (existing)              │◄────►│          (new)                  │
├─────────────────────────────────┤      ├─────────────────────────────────┤
│  • Bootstrap/discovery          │      │  • Minting enabled              │
│  • Relay blocks & txns          │      │  • Faucet endpoint enabled      │
│  • No wallet (relay-only)       │      │  • Wallet accumulates BTH       │
│  • Minting: OFF                 │      │  • Connects to seed as peer     │
│  • Faucet: OFF                  │      │                                 │
└─────────────────────────────────┘      └─────────────────────────────────┘
```

## Prerequisites

### EC2 Instance Requirements

| Component | Specification |
|-----------|--------------|
| Instance Type | `t3.medium` (2 vCPU, 4GB RAM) |
| Storage | 50GB gp3 SSD |
| OS | Ubuntu 22.04 LTS |
| Region | us-east-1 (or existing region) |

### Security Group Configuration

| Port | Protocol | Source | Purpose |
|------|----------|--------|---------|
| 22 | TCP | Admin IPs | SSH access |
| 17100 | TCP | 0.0.0.0/0 | P2P gossip |
| 17101 | TCP | 0.0.0.0/0 | RPC + Faucet |
| 19090 | TCP | Admin IPs | Prometheus metrics |

### DNS Configuration

Add an A record pointing `faucet.botho.io` to the EC2 Elastic IP.

## Quick Start

### Option 1: Automated Deployment

```bash
# SSH into the EC2 instance
ssh -i your-key.pem ubuntu@<ec2-ip>

# Clone the repository
git clone https://github.com/botho-project/botho.git
cd botho/infra/faucet

# Run the deployment script
sudo ./deploy-faucet.sh
```

### Option 2: Manual Deployment

See [Manual Deployment Steps](#manual-deployment-steps) below.

## Files

| File | Description |
|------|-------------|
| `deploy-faucet.sh` | Automated deployment script |
| `botho-faucet.service` | systemd service file |
| `faucet-config.toml.template` | Configuration template |

## Configuration

The faucet node uses the following configuration:

```toml
network_type = "testnet"

[wallet]
mnemonic = "... 24 words ..."  # Generated on init

[network]
gossip_port = 17100
rpc_port = 17101
metrics_port = 19090
cors_origins = ["*"]
bootstrap_peers = ["/dns4/seed.botho.io/tcp/17100"]
max_connections_per_ip = 50

[network.dns_seeds]
enabled = false

[network.quorum]
mode = "recommended"
min_peers = 1

[minting]
enabled = true
threads = 2

[faucet]
enabled = true
amount = 10_000_000_000_000        # 10 BTH per request
per_ip_hourly_limit = 5
per_address_daily_limit = 3
daily_limit = 10_000_000_000_000_000  # 10,000 BTH/day
cooldown_secs = 60
```

### Rate Limiting

The faucet includes built-in rate limiting to prevent abuse:

- **Per IP**: 5 requests per hour
- **Per Address**: 3 requests per 24 hours
- **Cooldown**: 60 seconds minimum between requests from same IP
- **Daily Total**: 10,000 BTH maximum per day

## Manual Deployment Steps

### 1. Launch EC2 Instance

1. Launch a `t3.medium` instance with Ubuntu 22.04 LTS
2. Attach a 50GB gp3 SSD
3. Allocate and associate an Elastic IP
4. Configure the security group with the ports listed above

### 2. Install Dependencies

```bash
sudo apt-get update
sudo apt-get install -y build-essential curl git pkg-config libssl-dev

# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source ~/.cargo/env
```

### 3. Build Botho

```bash
git clone https://github.com/botho-project/botho.git
cd botho
cargo build --release --bin botho
sudo cp target/release/botho /usr/local/bin/
```

### 4. Create Service User

```bash
sudo useradd -r -m -s /bin/bash botho
sudo mkdir -p /home/botho/.botho/testnet
sudo chown -R botho:botho /home/botho/.botho
sudo chmod 700 /home/botho/.botho
```

### 5. Initialize Wallet

```bash
sudo -u botho /usr/local/bin/botho --testnet init
```

**Important**: Back up the generated mnemonic from `/home/botho/.botho/testnet/config.toml`!

### 6. Configure Faucet

Edit `/home/botho/.botho/testnet/config.toml` to add:

```toml
[faucet]
enabled = true
amount = 10_000_000_000_000
per_ip_hourly_limit = 5
per_address_daily_limit = 3
daily_limit = 10_000_000_000_000_000
cooldown_secs = 60
```

And update the `[minting]` section:

```toml
[minting]
enabled = true
threads = 2
```

Set secure permissions:

```bash
sudo chmod 600 /home/botho/.botho/testnet/config.toml
```

### 7. Install systemd Service

```bash
sudo cp botho-faucet.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable botho-faucet
sudo systemctl start botho-faucet
```

### 8. Configure DNS

Add an A record for `faucet.botho.io` pointing to the Elastic IP.

### 9. Verify Deployment

```bash
# Check service status
sudo systemctl status botho-faucet

# Check logs
sudo journalctl -u botho-faucet -f

# Test RPC endpoint
curl -X POST http://localhost:17101/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"node_getStatus","params":{},"id":1}'

# Test faucet endpoint
curl -X POST http://localhost:17101/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"faucet_request","params":{"address":"YOUR_TESTNET_ADDRESS"},"id":1}'
```

## Operations

### View Logs

```bash
sudo journalctl -u botho-faucet -f
```

### Restart Service

```bash
sudo systemctl restart botho-faucet
```

### Check Node Status

```bash
curl -s -X POST http://localhost:17101/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"node_getStatus","params":{},"id":1}' | jq
```

### Check Faucet Balance

```bash
curl -s -X POST http://localhost:17101/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"wallet_getBalance","params":{},"id":1}' | jq
```

## Monitoring

### Set Up CloudWatch Monitoring

```bash
cd ../monitoring
sudo ./setup-monitoring.sh
```

### Key Metrics to Monitor

- **Chain height**: Should increase over time
- **Peer count**: Should be >= 1 (connected to seed)
- **Minting status**: Should be active
- **Faucet balance**: Monitor for depletion

### Health Check Script

```bash
#!/bin/bash
RESPONSE=$(curl -s -X POST http://localhost:17101/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"node_getStatus","params":{},"id":1}')

if echo "$RESPONSE" | jq -e '.result.chainHeight' > /dev/null 2>&1; then
    HEIGHT=$(echo "$RESPONSE" | jq '.result.chainHeight')
    PEERS=$(echo "$RESPONSE" | jq '.result.peerCount')
    MINTING=$(echo "$RESPONSE" | jq '.result.mintingActive')
    echo "OK - Height: $HEIGHT, Peers: $PEERS, Minting: $MINTING"
    exit 0
else
    echo "CRITICAL - Node not responding"
    exit 2
fi
```

## Security Considerations

1. **Mnemonic Protection**: The config file contains the wallet mnemonic. Keep it secure:
   - File permissions: `chmod 600 config.toml`
   - Back up securely (encrypted, offline)
   - Never commit to version control

2. **SSH Access**: Restrict SSH to known admin IPs only

3. **Metrics Port**: Keep port 19090 internal-only (not publicly exposed)

4. **Rate Limiting**: The built-in rate limiting prevents abuse, but monitor for patterns

5. **Future Enhancement**: Consider adding nginx reverse proxy with TLS

## Troubleshooting

### Node Won't Start

```bash
# Check logs for errors
sudo journalctl -u botho-faucet -n 100

# Check config syntax
cat /home/botho/.botho/testnet/config.toml

# Check port availability
sudo lsof -i :17100
sudo lsof -i :17101
```

### No Peers Connected

```bash
# Check network connectivity to seed
nc -zv seed.botho.io 17100

# Check firewall
sudo ufw status
```

### Faucet Not Working

```bash
# Check if faucet is enabled in config
grep -A5 '\[faucet\]' /home/botho/.botho/testnet/config.toml

# Check wallet balance
curl -s -X POST http://localhost:17101/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"wallet_getBalance","params":{},"id":1}' | jq
```

### High Resource Usage

```bash
# Check memory
ps aux | grep botho

# Check disk usage
df -h /home/botho/.botho

# Limit minting threads if needed (edit config.toml)
# [minting]
# threads = 1
```

## Acceptance Criteria Checklist

- [ ] EC2 instance running Ubuntu 22.04
- [ ] Botho node running with minting enabled
- [ ] Faucet endpoint responding at `http://faucet.botho.io:17101`
- [ ] Node connected to seed.botho.io as peer
- [ ] Blocks being minted (check block height increasing)
- [ ] Faucet dispenses testnet BTH correctly
- [ ] Systemd service configured for auto-restart
- [ ] Metrics available on port 19090 (internal only)
- [ ] DNS resolves faucet.botho.io correctly
