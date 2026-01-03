# Deployment Guide

Run Botho in production environments.

## Quick Start

### Basic Production Setup

```bash
# Build release binary
cargo build --release

# Copy to system location
sudo cp target/release/botho /usr/local/bin/

# Create data directory
mkdir -p ~/.botho

# Initialize wallet
botho init

# Run node
botho run
```

---

## systemd Service

### Create Service File

```bash
sudo nano /etc/systemd/system/botho.service
```

```ini
[Unit]
Description=Botho Node
After=network.target
Wants=network-online.target

[Service]
Type=simple
User=botho
Group=botho
WorkingDirectory=/home/botho
ExecStart=/usr/local/bin/botho run
Restart=on-failure
RestartSec=10

# Security hardening
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths=/home/botho/.botho

# Resource limits
LimitNOFILE=65535
MemoryMax=4G

# Logging
StandardOutput=journal
StandardError=journal
SyslogIdentifier=botho

[Install]
WantedBy=multi-user.target
```

### Create Service User

```bash
# Create dedicated user
sudo useradd -r -m -s /bin/false botho

# Set up data directory
sudo mkdir -p /home/botho/.botho
sudo chown -R botho:botho /home/botho/.botho
sudo chmod 700 /home/botho/.botho
```

### Initialize Wallet as Service User

```bash
sudo -u botho botho init
```

### Enable and Start

```bash
# Reload systemd
sudo systemctl daemon-reload

# Enable on boot
sudo systemctl enable botho

# Start service
sudo systemctl start botho

# Check status
sudo systemctl status botho

# View logs
sudo journalctl -u botho -f
```

### Service with Minting

For a minting node, modify the ExecStart line:

```ini
ExecStart=/usr/local/bin/botho run --mint
```

---

## Docker Deployment

### Dockerfile

```dockerfile
FROM rust:1.83-bookworm as builder

WORKDIR /app
COPY . .

RUN cargo build --release --bin botho

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/botho /usr/local/bin/

# Create non-root user
RUN useradd -r -m botho
USER botho
WORKDIR /home/botho

# Data volume
VOLUME /home/botho/.botho

# Expose ports
EXPOSE 7100 7101

ENTRYPOINT ["botho"]
CMD ["run"]
```

### Build and Run

```bash
# Build image
docker build -t botho:latest .

# Run container
docker run -d \
  --name botho-node \
  -p 7100:7100 \
  -p 7101:7101 \
  -v botho-data:/home/botho/.botho \
  botho:latest

# View logs
docker logs -f botho-node

# Run with minting
docker run -d \
  --name botho-miner \
  -p 7100:7100 \
  -p 7101:7101 \
  -v botho-data:/home/botho/.botho \
  botho:latest run --mint
```

### Docker Compose

```yaml
# docker-compose.yml
version: '3.8'

services:
  botho:
    build: .
    container_name: botho-node
    restart: unless-stopped
    ports:
      - "7100:7100"  # P2P
      - "7101:7101"  # RPC
    volumes:
      - botho-data:/home/botho/.botho
    environment:
      - RUST_LOG=info
    command: run
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:7101/"]
      interval: 30s
      timeout: 10s
      retries: 3

  botho-miner:
    build: .
    container_name: botho-miner
    restart: unless-stopped
    ports:
      - "7102:7100"
      - "7103:7101"
    volumes:
      - botho-miner-data:/home/botho/.botho
    environment:
      - RUST_LOG=info
    command: run --mint
    depends_on:
      - botho

volumes:
  botho-data:
  botho-miner-data:
```

```bash
# Start services
docker-compose up -d

# View logs
docker-compose logs -f

# Stop services
docker-compose down
```

---

## Configuration for Production

### Recommended config.toml

```toml
[wallet]
# Mnemonic is generated on init - back it up securely!

[network]
gossip_port = 7100
rpc_port = 7101

# Production bootstrap peers
bootstrap_peers = [
    "/ip4/98.95.2.200/tcp/7100/p2p/12D3KooWBrjTYjNrEwi9MM3AKFenmymyWVXtXbQiSx7eDnDwv9qQ",
]

# Restrict CORS for production
cors_origins = ["https://yourdomain.com"]

[network.quorum]
mode = "recommended"
min_peers = 2

[minting]
enabled = false  # Set true for mining nodes
threads = 0      # Auto-detect
```

### File Permissions

```bash
# Secure the config file (contains mnemonic)
chmod 600 ~/.botho/config.toml

# Secure the data directory
chmod 700 ~/.botho
```

---

## Firewall Configuration

### UFW (Ubuntu)

```bash
# Allow P2P gossip
sudo ufw allow 7100/tcp comment "Botho P2P"

# Allow RPC (if exposing externally - be careful!)
# sudo ufw allow 7101/tcp comment "Botho RPC"

# Or restrict RPC to specific IPs
sudo ufw allow from 10.0.0.0/8 to any port 7101 comment "Botho RPC internal"
```

### iptables

```bash
# Allow P2P
sudo iptables -A INPUT -p tcp --dport 7100 -j ACCEPT

# Allow RPC from localhost only
sudo iptables -A INPUT -p tcp --dport 7101 -s 127.0.0.1 -j ACCEPT
sudo iptables -A INPUT -p tcp --dport 7101 -j DROP
```

---

## Reverse Proxy (nginx)

For exposing RPC over HTTPS:

```nginx
# /etc/nginx/sites-available/botho-rpc
server {
    listen 443 ssl http2;
    server_name rpc.yourdomain.com;

    ssl_certificate /etc/letsencrypt/live/rpc.yourdomain.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/rpc.yourdomain.com/privkey.pem;

    # Security headers
    add_header X-Content-Type-Options nosniff;
    add_header X-Frame-Options DENY;

    # Rate limiting
    limit_req_zone $binary_remote_addr zone=rpc:10m rate=10r/s;
    limit_req zone=rpc burst=20 nodelay;

    location / {
        proxy_pass http://127.0.0.1:7101;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        # For WebSocket support
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_read_timeout 86400;
    }
}
```

```bash
sudo ln -s /etc/nginx/sites-available/botho-rpc /etc/nginx/sites-enabled/
sudo nginx -t && sudo systemctl reload nginx
```

---

## Monitoring

> **CloudWatch Monitoring**: For AWS CloudWatch setup (EC2 instances), see [docs/monitoring.md](monitoring.md).

### Health Check Script

```bash
#!/bin/bash
# /usr/local/bin/botho-health

RESPONSE=$(curl -s -X POST http://localhost:7101/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"node_getStatus","params":{},"id":1}')

if echo "$RESPONSE" | jq -e '.result.chainHeight' > /dev/null 2>&1; then
    HEIGHT=$(echo "$RESPONSE" | jq '.result.chainHeight')
    PEERS=$(echo "$RESPONSE" | jq '.result.peerCount')
    echo "OK - Height: $HEIGHT, Peers: $PEERS"
    exit 0
else
    echo "CRITICAL - Node not responding"
    exit 2
fi
```

### Prometheus Metrics (Custom Exporter)

```python
#!/usr/bin/env python3
# /usr/local/bin/botho-exporter

from prometheus_client import start_http_server, Gauge
import requests
import time

# Metrics
chain_height = Gauge('botho_chain_height', 'Current blockchain height')
peer_count = Gauge('botho_peer_count', 'Number of connected peers')
mempool_size = Gauge('botho_mempool_size', 'Transactions in mempool')
minting_active = Gauge('botho_minting_active', 'Whether minting is active')

def collect_metrics():
    try:
        response = requests.post('http://localhost:7101/', json={
            'jsonrpc': '2.0',
            'method': 'node_getStatus',
            'params': {},
            'id': 1
        }, timeout=5)
        data = response.json()['result']

        chain_height.set(data['chainHeight'])
        peer_count.set(data['peerCount'])
        mempool_size.set(data['mempoolSize'])
        minting_active.set(1 if data['mintingActive'] else 0)
    except Exception as e:
        print(f"Error collecting metrics: {e}")

if __name__ == '__main__':
    start_http_server(9100)
    while True:
        collect_metrics()
        time.sleep(15)
```

### Grafana Dashboard

Import these Prometheus queries:

```promql
# Chain height
botho_chain_height

# Peer count
botho_peer_count

# Mempool size
botho_mempool_size

# Minting status
botho_minting_active
```

---

## Log Management

### journald Configuration

```bash
# /etc/systemd/journald.conf.d/botho.conf
[Journal]
SystemMaxUse=1G
MaxRetentionSec=7day
```

### Log Rotation (if using files)

```bash
# /etc/logrotate.d/botho
/var/log/botho/*.log {
    daily
    rotate 7
    compress
    delaycompress
    missingok
    notifempty
    create 0640 botho botho
}
```

### Structured Logging

Set log level via environment:

```bash
# In systemd service
Environment=RUST_LOG=info

# More verbose
Environment=RUST_LOG=botho=debug

# Specific modules
Environment=RUST_LOG=botho::consensus=debug,botho::network=info
```

---

## Backup Strategy

### Automated Backups

```bash
#!/bin/bash
# /usr/local/bin/botho-backup

BACKUP_DIR="/backup/botho"
DATE=$(date +%Y%m%d_%H%M%S)

# Create backup directory
mkdir -p "$BACKUP_DIR"

# Backup config (contains mnemonic!)
cp ~/.botho/config.toml "$BACKUP_DIR/config_$DATE.toml"

# Encrypt the backup
gpg --symmetric --cipher-algo AES256 "$BACKUP_DIR/config_$DATE.toml"
rm "$BACKUP_DIR/config_$DATE.toml"

# Keep only last 30 backups
ls -t "$BACKUP_DIR"/config_*.gpg | tail -n +31 | xargs -r rm

echo "Backup complete: $BACKUP_DIR/config_$DATE.toml.gpg"
```

Add to crontab:
```bash
0 2 * * * /usr/local/bin/botho-backup
```

---

## High Availability

### Multiple Nodes

Run multiple nodes behind a load balancer for RPC availability:

```
                    ┌─────────────┐
                    │   HAProxy   │
                    │  (RPC LB)   │
                    └──────┬──────┘
           ┌───────────────┼───────────────┐
           │               │               │
    ┌──────┴──────┐ ┌──────┴──────┐ ┌──────┴──────┐
    │   Node 1    │ │   Node 2    │ │   Node 3    │
    │  (sync)     │ │  (sync)     │ │  (mining)   │
    └─────────────┘ └─────────────┘ └─────────────┘
```

### HAProxy Configuration

```
# /etc/haproxy/haproxy.cfg
frontend botho_rpc
    bind *:7101
    default_backend botho_nodes

backend botho_nodes
    balance roundrobin
    option httpchk POST / HTTP/1.1\r\nContent-Type:\ application/json\r\nContent-Length:\ 62\r\n\r\n{"jsonrpc":"2.0","method":"node_getStatus","params":{},"id":1}
    http-check expect status 200

    server node1 10.0.0.1:7101 check
    server node2 10.0.0.2:7101 check
    server node3 10.0.0.3:7101 check backup
```

---

## Troubleshooting Production Issues

### Node Won't Start

```bash
# Check logs
sudo journalctl -u botho -n 100

# Check permissions
ls -la ~/.botho/

# Check disk space
df -h ~/.botho/

# Check port availability
sudo lsof -i :7100
sudo lsof -i :7101
```

### High Resource Usage

```bash
# Check memory
ps aux | grep botho

# Check open files
lsof -p $(pgrep botho) | wc -l

# Increase limits if needed
ulimit -n 65535
```

### Network Issues

```bash
# Check connectivity to seed
nc -zv 98.95.2.200 7100

# Check listening ports
ss -tlnp | grep botho

# Check firewall
sudo ufw status
```

---

## Checklist

### Pre-deployment
- [ ] Build release binary
- [ ] Create dedicated user
- [ ] Set up data directory with proper permissions
- [ ] Initialize wallet and backup mnemonic
- [ ] Configure firewall rules

### Deployment
- [ ] Install systemd service
- [ ] Enable and start service
- [ ] Verify node syncs
- [ ] Verify peer connections

### Post-deployment
- [ ] Set up monitoring
- [ ] Configure log rotation
- [ ] Set up automated backups
- [ ] Document recovery procedures
- [ ] Test failover (if HA)
