# Botho Faucet Node Deployment

Deploy and operate the testnet faucet node at `faucet.botho.io`.

## Architecture

```
┌─────────────────────────────────┐      ┌───────────────────────────────────────────┐
│        seed.botho.io            │      │           faucet.botho.io                 │
│         (existing)              │◄────►│                                           │
├─────────────────────────────────┤      │  ┌─────────────────┐  ┌─────────────────┐│
│  • Bootstrap/discovery          │      │  │   nginx         │  │   Botho Node    ││
│  • Relay blocks & txns          │      │  │   (port 80/443) │  │   (port 17101)  ││
│  • No wallet (relay-only)       │      │  │                 │  │                 ││
│  • Minting: OFF                 │      │  │  /     → static │  │  • Minting: ON  ││
│  • Faucet: OFF                  │      │  │  /rpc  → proxy ─┼──┼─►• Faucet: ON   ││
│                                 │      │  │                 │  │  • Wallet       ││
└─────────────────────────────────┘      │  └─────────────────┘  └─────────────────┘│
                                         └───────────────────────────────────────────┘
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

> **Build prerequisites.** A from-source build needs
> `build-essential cmake pkg-config libssl-dev` plus a Rust toolchain. `cmake`
> is mandatory: `randomx-rs` compiles RandomX's C++ via a cmake build script and
> the build aborts without it. The deploy scripts (`deploy-faucet.sh`,
> `deploy-botho.sh`) install these automatically.

### Preferred: deploy release artifacts (no build at all)

When deploying a tagged release, skip building entirely — the release
workflow publishes reproducible, checksummed `linux-aarch64` binaries
(see `docs/operations/reproducible-builds.md`). `deploy-botho.sh` does this
by default; manually:

```bash
TAG=v0.3.0
curl -fsSLO https://github.com/botho-project/botho/releases/download/$TAG/botho-$TAG-linux-aarch64.tar.gz
curl -fsSLO https://github.com/botho-project/botho/releases/download/$TAG/checksums-linux-aarch64.txt
tar xzf botho-$TAG-linux-aarch64.tar.gz && sha256sum -c checksums-linux-aarch64.txt
sudo install -m755 botho /usr/local/bin/botho  # then restart the service
```

### Fallback for untagged commits: build-once-and-distribute

The seed boxes are **t4g.small (1.8 GiB RAM, 0 swap)** and a `--release` build of
this workspace OOMs there. Do **not** build on the seeds. When you must deploy
an untagged commit, the verified path (used in the #323 redeploy) is to build
once on the faucet and copy the binary to the seeds:

```bash
# On the faucet (t4g.medium, 3.7 GiB RAM) — add a temporary swapfile first so
# the RandomX-linked release build doesn't OOM:
sudo fallocate -l 4G /swapfile && sudo chmod 600 /swapfile
sudo mkswap /swapfile && sudo swapon /swapfile

cd ~/botho && git fetch origin && git reset --hard origin/main
cargo build --release -p botho

# Distribute the identical aarch64 binary to each seed (all boxes are
# Ubuntu 24.04.3 / glibc 2.39, so the binary is portable):
scp target/release/botho ubuntu@seed.botho.io:/tmp/botho
scp target/release/botho ubuntu@seed2.botho.io:/tmp/botho
# then on each seed: sudo install -m755 /tmp/botho /usr/local/bin/botho && restart the service

# Optional: remove the swapfile when done.
sudo swapoff /swapfile && sudo rm /swapfile
```

### Option 2: Manual Deployment

See [Manual Deployment Steps](#manual-deployment-steps) below.

## Web UI

The faucet includes a user-friendly web interface at `https://faucet.botho.io`.

### Features

- **Address input**: Paste wallet address in `view:...\nspend:...` format
- **Drip decay**: Reduced amounts for frequent requests (see table below)
- **Real-time status**: Shows faucet availability and daily usage
- **Transaction feedback**: Displays TX hash with copy functionality

### Drip Amount Decay

To gently discourage rapid re-requests while still allowing access:

| Time Since Last Request | Amount Dispensed |
|------------------------|------------------|
| First request ever | 1.0 BTH (full) |
| < 1 hour | 0.1 BTH (10%) |
| 1-6 hours | 0.25 BTH (25%) |
| 6-12 hours | 0.5 BTH (50%) |
| 12-24 hours | 0.75 BTH (75%) |
| > 24 hours | 1.0 BTH (full) |

**Note**: Decay tracking uses localStorage on the client side. Server-side rate limiting remains the authoritative protection.

### Web UI Deployment

```bash
# Install nginx if not present
sudo apt-get install -y nginx

# Copy web files
sudo mkdir -p /var/www/faucet
sudo cp -r web/* /var/www/faucet/

# Install nginx configuration
sudo cp faucet-nginx.conf /etc/nginx/sites-available/faucet.botho.io
sudo ln -sf /etc/nginx/sites-available/faucet.botho.io /etc/nginx/sites-enabled/

# Set up SSL with Let's Encrypt
sudo apt-get install -y certbot python3-certbot-nginx
sudo certbot --nginx -d faucet.botho.io

# Test and reload nginx
sudo nginx -t
sudo systemctl reload nginx
```

### RPC Response Caching

The nginx configuration includes caching for RPC responses to reduce node load and improve response times for frequently-accessed stats endpoints.

#### Cached Endpoints

| RPC Method | Cache TTL | Rationale |
|------------|-----------|-----------|
| `node_getStatus` | 10s | Block time ~30s, 10s provides freshness |
| `faucet_getStatus` | 10s | Daily stats, less volatile |
| `faucet_request` | Never | User-specific, must always execute |
| `/api/metrics` | 60s | Historical metrics, updated every 5 min |

**Note**: Standard nginx uses a uniform 10s TTL for RPC methods. For method-specific TTLs, see [OpenResty Configuration](#openresty-configuration-optional) below.

#### Cache Directory Setup

```bash
# Create cache directory (required)
sudo mkdir -p /var/cache/nginx/rpc
sudo chown www-data:www-data /var/cache/nginx/rpc
```

#### Verifying Cache Status

Check the `X-Cache-Status` header to verify caching is working:

```bash
# First request - should be MISS
curl -s -X POST https://faucet.botho.io/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"node_getStatus","params":{},"id":1}' \
  -D - -o /dev/null | grep X-Cache-Status

# Second request within 10s - should be HIT
curl -s -X POST https://faucet.botho.io/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"node_getStatus","params":{},"id":1}' \
  -D - -o /dev/null | grep X-Cache-Status

# faucet_request - should be BYPASS
curl -s -X POST https://faucet.botho.io/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"faucet_request","params":{"address":"test"},"id":1}' \
  -D - -o /dev/null | grep X-Cache-Status
```

Cache status values:
- `HIT` - Served from cache
- `MISS` - Not in cache, fetched from upstream
- `BYPASS` - Cache bypassed (non-cacheable method)
- `EXPIRED` - Cache entry expired, fetched fresh

#### Monitoring Cache Hit Ratio

```bash
# Check nginx cache stats
sudo tail -f /var/log/nginx/access.log | grep -E '"(HIT|MISS|BYPASS)"'

# Or parse for hit ratio
sudo awk '/"HIT"/{hit++}/"MISS"/{miss++}END{print "Hit ratio:", hit/(hit+miss)*100"%"}' \
  /var/log/nginx/access.log
```

Target: > 80% hit ratio for stats endpoints.

#### OpenResty Configuration (Optional)

For method-specific cache TTLs (10s for `node_getStatus`, 30s for `faucet_getStatus`), use OpenResty with Lua:

```bash
# Install OpenResty instead of nginx
sudo apt-get install -y openresty

# Replace nginx with openresty in service commands
sudo systemctl stop nginx
sudo systemctl disable nginx
sudo systemctl enable openresty
sudo systemctl start openresty
```

Create `/etc/openresty/conf.d/rpc-cache.lua`:

```lua
-- Parse JSON-RPC method and apply appropriate cache TTL
local cjson = require "cjson.safe"

local cache_ttl = {
    ["node_getStatus"] = 10,
    ["faucet_getStatus"] = 30,
}

local function set_cache_header()
    ngx.req.read_body()
    local body = ngx.req.get_body_data()
    if not body then return end

    local req, err = cjson.decode(body)
    if not req or not req.method then return end

    local ttl = cache_ttl[req.method]
    if ttl and ttl > 0 then
        ngx.header["X-Accel-Expires"] = ttl
    else
        ngx.header["X-Accel-Expires"] = 0  -- Don't cache
    end
end

return { set_cache_header = set_cache_header }
```

Update the `/rpc` location in nginx config:

```nginx
location /rpc {
    # ... existing config ...

    # Use X-Accel-Expires from Lua for dynamic TTL
    proxy_cache_valid 200 0;  # Defer to X-Accel-Expires

    header_filter_by_lua_block {
        local cache = require "rpc-cache"
        cache.set_cache_header()
    }
}
```

### Security Group Update

Add these ports for the web UI:

| Port | Protocol | Source | Purpose |
|------|----------|--------|---------|
| 80 | TCP | 0.0.0.0/0 | HTTP (redirects to HTTPS) |
| 443 | TCP | 0.0.0.0/0 | HTTPS |

## Fleet Metrics Daemon (botho-metrics)

`metrics-daemon/` is a small standalone crate that polls `node_getStatus`
on every testnet node every 5 minutes, stores per-node samples in SQLite
(with hourly/daily rollups and retention), and serves a public history API
for the wallet network dashboard (issue #697).

### API contract

Public base URL: `https://faucet.botho.io/metrics-api/`
(nginx strips the `/metrics-api` prefix; the daemon itself listens on
`127.0.0.1:17102`).

**`GET /metrics-api/api/metrics/latest`** — one entry per node:

```json
[
  {
    "node": "seed",
    "timestamp": 1751846400,
    "height": 12345,
    "peerCount": 4.0,
    "scpPeerCount": 3.0,
    "mempoolSize": 0.0,
    "mintingActive": false,
    "uptimeSeconds": 86400,
    "heightStale": false
  }
]
```

`heightStale` is true when the node's height has not changed across the
last 3 samples (~15 minutes) — the chain-height alerting signal.

**`GET /metrics-api/api/metrics/history?node=<name>&resolution=5min|hourly|daily&since=<unix-seconds>`**
— samples for one node (`node` required; `resolution` defaults to `5min`;
`since` defaults to 0):

```json
[
  {
    "timestamp": 1751846400,
    "height": 12345,
    "peerCount": 4.0,
    "scpPeerCount": 3.0,
    "mempoolSize": 0.0,
    "txTotal": 12
  }
]
```

**`GET /metrics-api/health`** — plain `OK`.

Retention: 5min samples 24h, hourly 30d, daily 1y.

### Deploying on the faucet box

Build on the host (small crate, fine to build in place):

```bash
cd ~/botho && git pull
cargo build --release -p botho-metrics-daemon
sudo install -m755 target/release/botho-metrics-daemon /usr/local/bin/

# Install + start the service (polls all 5 testnet nodes, see unit file)
sudo cp infra/faucet/botho-metrics.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now botho-metrics

# The DB lives at /var/lib/botho-metrics/metrics.db (StateDirectory).
# Schema changed in #697 (per-node rows, no migration; testnet DB is
# disposable) — delete any old single-node metrics.db before first start.

# nginx route: faucet-nginx.conf already contains the /metrics-api/
# location block (GET-only, CORS '*', proxied to 127.0.0.1:17102).
sudo cp infra/faucet/faucet-nginx.conf /etc/nginx/sites-available/faucet.botho.io
sudo nginx -t && sudo systemctl reload nginx

# Verify
curl -s https://faucet.botho.io/metrics-api/health
curl -s https://faucet.botho.io/metrics-api/api/metrics/latest | jq .
```

Single-node fallback (no `--node` args): `--node-url <url>` polls one node
and stores it under the node name `default`.

## Files

| File | Description |
|------|-------------|
| `deploy-faucet.sh` | Automated deployment script |
| `botho-faucet.service` | systemd service file |
| `botho-metrics.service` | systemd unit for the fleet metrics daemon |
| `metrics-daemon/` | Fleet metrics collector + history API crate |
| `faucet-config.toml.template` | Configuration template |
| `faucet-nginx.conf` | nginx configuration for web UI |
| `web/` | Static web UI files |

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
sudo apt-get install -y build-essential cmake curl git pkg-config libssl-dev

# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source ~/.cargo/env
```

> **`cmake` is required.** `randomx-rs` compiles the RandomX C++ library via a
> cmake build script; without it a from-source build fails with
> ``error: failed to run custom build command for `randomx-rs` ... is `cmake`
> not installed?``. The full build prerequisite set is
> `build-essential cmake pkg-config libssl-dev` plus a Rust toolchain.

### 3. Install Botho

Prefer the published release artifact (reproducible + checksummed; see the
"Preferred: deploy release artifacts" section above). Build from source only
for untagged commits:

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

### Botho Node
- [ ] EC2 instance running Ubuntu 22.04
- [ ] Botho node running with minting enabled
- [ ] Faucet endpoint responding at `http://localhost:17101`
- [ ] Node connected to seed.botho.io as peer
- [ ] Blocks being minted (check block height increasing)
- [ ] Faucet dispenses testnet BTH correctly
- [ ] Systemd service configured for auto-restart
- [ ] Metrics available on port 19090 (internal only)

### Web UI
- [ ] Web page accessible at https://faucet.botho.io
- [ ] User can enter address and receive BTH
- [ ] Drip amount decays based on time since last request
- [ ] Current drip amount shown before clicking
- [ ] Hint shows time to wait for full amount
- [ ] Transaction hash displayed with copy functionality
- [ ] Faucet stats shown on page (enabled, daily usage)
- [ ] Clear, user-friendly error messages for all failure modes
- [ ] Mobile-responsive design
- [ ] Page loads quickly (< 2s)
- [ ] SSL certificate valid and working

### RPC Caching (#309)
- [ ] nginx cache zone configured (`/var/cache/nginx/rpc`)
- [ ] `node_getStatus` cached (X-Cache-Status shows HIT)
- [ ] `faucet_getStatus` cached (X-Cache-Status shows HIT)
- [ ] `faucet_request` never cached (X-Cache-Status shows BYPASS)
- [ ] X-Cache-Status header visible in responses
- [ ] Cache size limited (100MB max)
- [ ] Stale entries cleaned up (60m inactive timeout)
- [ ] Cache hit ratio > 80% for stats endpoints
