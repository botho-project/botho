# Botho Seed Node

Primary bootstrap node for the Botho network, with web-based status page.

## Features

- Testnet relay node for peer discovery
- Real-time node status (uptime, peers, chain height)
- Chain information (tip hash, difficulty, circulating supply)
- Auto-refresh every 10 seconds
- RPC response caching for performance

## Directory Structure

```
infra/seed/
├── README.md                 # This file
├── botho-seed.service        # Systemd service file
├── reset-to-testnet.sh       # Reset script for testnet
├── deploy-web.sh             # Deploy web files to server
├── seed-nginx.conf           # Nginx configuration
└── web/
    ├── index.html            # Main status page
    ├── css/
    │   └── style.css         # Custom styles
    └── js/
        └── status.js         # Status fetching logic
```

## Node Setup

### Install Systemd Service

```bash
# Copy service file
sudo cp botho-seed.service /etc/systemd/system/

# Enable and start
sudo systemctl daemon-reload
sudo systemctl enable botho-seed
sudo systemctl start botho-seed

# Check status
systemctl status botho-seed
journalctl -u botho-seed -f
```

### Reset to Testnet

If the node was accidentally running mainnet, use the reset script:

```bash
./reset-to-testnet.sh
```

This will:
1. Stop the botho service
2. Remove mainnet data (~/.botho/mainnet/)
3. Install the correct systemd service
4. Start the node on testnet

### Data Directories

- Base: `~/.botho/`
- Testnet: `~/.botho/testnet/`
- Mainnet: `~/.botho/mainnet/` (should not exist on seed node)
- Ledger: `~/.botho/testnet/ledger/`
- Config: `~/.botho/testnet/config.toml`

## Deployment

### Prerequisites

- Nginx with SSL support
- Let's Encrypt certificates for seed.botho.io
- Running Botho node on port 17101

### Installation

1. **Copy nginx configuration:**
   ```bash
   sudo cp seed-nginx.conf /etc/nginx/sites-available/seed.botho.io
   sudo ln -s /etc/nginx/sites-available/seed.botho.io /etc/nginx/sites-enabled/
   ```

2. **Create cache directory:**
   ```bash
   sudo mkdir -p /var/cache/nginx/seed
   sudo chown www-data:www-data /var/cache/nginx/seed
   ```

3. **Copy web files:**
   ```bash
   sudo mkdir -p /var/www/seed
   sudo cp -r web/* /var/www/seed/
   sudo chown -R www-data:www-data /var/www/seed
   ```

4. **Obtain SSL certificate (if not already done):**
   ```bash
   sudo certbot certonly --webroot -w /var/www/certbot -d seed.botho.io
   ```

5. **Test and reload nginx:**
   ```bash
   sudo nginx -t
   sudo systemctl reload nginx
   ```

## Configuration

### RPC Caching

The nginx configuration caches responses for read-only RPC methods:
- `node_getStatus` - 5 second cache
- `getChainInfo` - 5 second cache

This reduces load on the node while keeping the status page responsive.

### Refresh Interval

The status page auto-refreshes every 10 seconds. To change this, edit `CONFIG.refreshInterval` in `web/js/status.js`.

## RPC Methods Used

The status page uses these RPC endpoints:

| Method | Description |
|--------|-------------|
| `node_getStatus` | Node uptime, peers, sync status, minting |
| `getChainInfo` | Chain height, tip hash, difficulty, supply |

## Styling

The status page uses the same design system as the faucet:
- Tailwind CSS via CDN
- Custom color palette (botho-bg, botho-card, botho-cyan, etc.)
- Gradient logo and accents
- Dark theme optimized

## Troubleshooting

### Status shows "Offline"
- Check if Botho node is running: `systemctl status botho`
- Verify RPC port is accessible: `curl -X POST http://localhost:17101 -d '{"jsonrpc":"2.0","method":"node_getStatus","params":{},"id":1}'`

### Page not loading
- Check nginx error log: `tail -f /var/log/nginx/error.log`
- Verify web files exist: `ls -la /var/www/seed/`

### Stale data
- Check cache status header in browser dev tools (X-Cache-Status)
- Clear nginx cache: `rm -rf /var/cache/nginx/seed/*`
