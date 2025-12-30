# Exchange Integration Guide

Integrate Botho (BTH) into your cryptocurrency exchange.

## Overview

Botho is a privacy-preserving cryptocurrency using stealth addresses and ring signatures. This guide covers the technical requirements for exchange integration.

### Key Characteristics

| Property | Value |
|----------|-------|
| Native unit | BTH (credit) |
| Smallest unit | 1 credit (10^-12 BTH) |
| Block time | ~60 seconds |
| Confirmations | 10 recommended for deposits |
| Address format | Stealth addresses (view + spend public keys) |
| Transaction privacy | Ring signatures, stealth addresses |
| API | JSON-RPC 2.0 + WebSocket |

### Infrastructure Requirements

| Component | Recommendation |
|-----------|---------------|
| Full node | Required (runs `botho run`) |
| Storage | 50GB+ SSD for blockchain |
| RAM | 4GB minimum, 8GB recommended |
| Network | Ports 7100 (P2P), 7101 (RPC) |

---

## Architecture

### Recommended Setup

```
                    ┌─────────────────┐
                    │  Exchange Core  │
                    │    Systems      │
                    └────────┬────────┘
                             │
              ┌──────────────┼──────────────┐
              │              │              │
       ┌──────┴──────┐ ┌─────┴─────┐ ┌──────┴──────┐
       │   Hot Node  │ │ Cold Node │ │  Backup     │
       │  (online)   │ │ (offline) │ │   Node      │
       └─────────────┘ └───────────┘ └─────────────┘
```

### Hot Wallet Node

- Handles deposits and small withdrawals
- Connected to network, syncs blockchain
- Limited funds (e.g., 1-5% of total holdings)

### Cold Wallet

- Stores majority of funds
- Air-gapped machine for signing
- Only connects to transfer signed transactions

---

## Running a Node

### Installation

```bash
# Clone and build
git clone https://github.com/botho-project/botho.git
cd botho
cargo build --release

# Install binary
sudo cp target/release/botho /usr/local/bin/

# Create data directory
mkdir -p /var/lib/botho
```

### Configuration

Create `/var/lib/botho/config.toml`:

```toml
[wallet]
# Hot wallet mnemonic - BACK THIS UP SECURELY
# Generated on first run with: botho init

[network]
gossip_port = 7100
rpc_port = 7101

# Restrict RPC access
cors_origins = ["http://localhost"]

bootstrap_peers = [
    "/ip4/98.95.2.200/tcp/7100/p2p/12D3KooWBrjTYjNrEwi9MM3AKFenmymyWVXtXbQiSx7eDnDwv9qQ",
]

[network.quorum]
mode = "recommended"
min_peers = 3

[minting]
enabled = false
```

### systemd Service

Create `/etc/systemd/system/botho-exchange.service`:

```ini
[Unit]
Description=Botho Exchange Node
After=network.target

[Service]
Type=simple
User=botho
WorkingDirectory=/var/lib/botho
ExecStart=/usr/local/bin/botho run
Restart=always
RestartSec=10

# Security
NoNewPrivileges=true
PrivateTmp=true

# Resources
LimitNOFILE=65535
MemoryMax=8G

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable botho-exchange
sudo systemctl start botho-exchange
```

---

## Deposit Handling

### How Deposits Work

Botho uses stealth addresses. Each deposit creates a one-time address that only you can detect:

1. User sends BTH to your stealth address
2. Your node scans blocks for outputs belonging to your wallet
3. You detect the deposit and credit the user's account

### Generating Deposit Addresses

For Botho, you provide a single stealth address to all users. The privacy comes from the stealth address protocol—each transaction creates a unique one-time address.

```bash
# Get your public keys
curl -X POST http://localhost:7101/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"wallet_getAddress","params":{},"id":1}'
```

Response:
```json
{
  "result": {
    "viewKey": "abc123...",
    "spendKey": "def456...",
    "hasWallet": true
  }
}
```

**User Identification**: Since all users deposit to the same address, use payment IDs or encrypted memos to identify deposits:

```
Option 1: Unique memo per user
- Generate unique memo for each user
- User includes memo when sending
- Match memo to user account

Option 2: Withdrawal addresses
- User provides their address first
- Credit account when they withdraw
- Simpler but less flexible
```

### Scanning for Deposits

Poll the node for new outputs:

```python
import requests
import time

class DepositScanner:
    def __init__(self, rpc_url, start_height=0):
        self.rpc_url = rpc_url
        self.last_height = start_height

    def rpc(self, method, params=None):
        response = requests.post(self.rpc_url, json={
            'jsonrpc': '2.0',
            'method': method,
            'params': params or {},
            'id': 1
        })
        return response.json()['result']

    def scan(self):
        """Scan for new deposits"""
        status = self.rpc('node_getStatus')
        current_height = status['chainHeight']

        if current_height <= self.last_height:
            return []

        deposits = []

        # Scan in batches
        for start in range(self.last_height + 1, current_height + 1, 100):
            end = min(start + 99, current_height)
            outputs = self.rpc('chain_getOutputs', {
                'start_height': start,
                'end_height': end
            })

            for block in outputs:
                for output in block['outputs']:
                    # Check if output belongs to our wallet
                    # This requires the node to have wallet configured
                    if self.is_our_output(output):
                        deposits.append({
                            'tx_hash': output['txHash'],
                            'output_index': output['outputIndex'],
                            'height': block['height'],
                            'confirmations': current_height - block['height']
                        })

        self.last_height = current_height
        return deposits

    def is_our_output(self, output):
        """
        Check if output belongs to our wallet.
        The node handles this internally when wallet is configured.
        """
        # Use wallet_getBalance to get owned UTXOs
        # Implementation depends on your wallet integration
        pass

# Usage
scanner = DepositScanner('http://localhost:7101/')

while True:
    deposits = scanner.scan()
    for deposit in deposits:
        if deposit['confirmations'] >= 10:
            process_deposit(deposit)
    time.sleep(30)
```

### Confirmation Requirements

| Risk Level | Confirmations | Wait Time |
|------------|---------------|-----------|
| Low value | 6 | ~6 minutes |
| Standard | 10 | ~10 minutes |
| High value | 20+ | ~20+ minutes |

For exchanges, **10 confirmations** is recommended as a safe default.

### WebSocket Deposit Monitoring

For real-time notifications:

```python
import asyncio
import websockets
import json

async def monitor_deposits():
    async with websockets.connect('ws://localhost:7101/ws') as ws:
        # Subscribe to block events
        await ws.send(json.dumps({
            'type': 'subscribe',
            'events': ['blocks', 'transactions']
        }))

        async for message in ws:
            msg = json.loads(message)

            if msg.get('type') == 'event':
                if msg['event'] == 'block':
                    block_height = msg['data']['height']
                    # Scan this block for deposits
                    await process_block(block_height)

asyncio.run(monitor_deposits())
```

---

## Withdrawal Handling

### Withdrawal Flow

```
1. User requests withdrawal
2. Validate address and amount
3. Check hot wallet balance
4. Create and sign transaction
5. Submit to network
6. Monitor for confirmation
7. Mark withdrawal complete
```

### Creating Withdrawals

Botho transactions are built and signed by the node's integrated wallet:

```python
def process_withdrawal(user_address, amount_bth):
    """
    Process a withdrawal request.
    Amount is in BTH, converted to credits internally.
    """
    amount_credits = int(amount_bth * 1e12)

    # 1. Estimate fee
    fee_response = rpc('estimateFee', {
        'amount': amount_credits,
        'private': True
    })
    fee = fee_response['recommendedFee']

    # 2. Check balance
    balance = rpc('wallet_getBalance')
    if balance['confirmed'] < amount_credits + fee:
        raise InsufficientFundsError()

    # 3. Build and submit transaction
    # Note: This requires the node to support tx building via RPC
    # Currently, use the CLI or Rust crates for transaction building
    result = execute_withdrawal_cli(user_address, amount_credits)

    return {
        'tx_hash': result['txHash'],
        'fee': fee,
        'amount': amount_credits
    }

def execute_withdrawal_cli(address, amount):
    """Execute withdrawal using CLI"""
    import subprocess
    result = subprocess.run([
        'botho', 'send', address, str(amount)
    ], capture_output=True, text=True)

    if result.returncode != 0:
        raise WithdrawalError(result.stderr)

    # Parse tx hash from output
    return {'txHash': parse_tx_hash(result.stdout)}
```

### Fee Calculation

```python
def calculate_withdrawal_fee(amount):
    """Calculate fee for withdrawal"""
    fee_info = rpc('estimateFee', {
        'amount': amount,
        'private': True,
        'memos': 0
    })

    return {
        'minimum': fee_info['minimumFee'],
        'recommended': fee_info['recommendedFee'],
        'high_priority': fee_info['highPriorityFee'],
        'rate_bps': fee_info['feeRateBps']
    }
```

### Batch Withdrawals

For efficiency, batch multiple withdrawals into fewer transactions:

```python
def batch_withdrawals(pending_withdrawals):
    """
    Batch multiple withdrawals into one transaction.
    Reduces fees and blockchain bloat.
    """
    total_amount = sum(w['amount'] for w in pending_withdrawals)

    # Estimate combined fee
    fee = rpc('estimateFee', {
        'amount': total_amount,
        'private': True
    })['recommendedFee']

    # Build transaction with multiple outputs
    # Implementation requires transaction builder
    pass
```

---

## Balance Management

### Hot/Cold Wallet Split

Recommended allocation:

| Wallet | Allocation | Purpose |
|--------|------------|---------|
| Hot | 2-5% | Automated withdrawals |
| Warm | 10-20% | Manual top-ups |
| Cold | 75-88% | Long-term storage |

### Automated Rebalancing

```python
class WalletRebalancer:
    def __init__(self, hot_target_pct=0.05, threshold_pct=0.02):
        self.hot_target = hot_target_pct
        self.threshold = threshold_pct

    def check_rebalance_needed(self):
        hot_balance = get_hot_wallet_balance()
        total_balance = get_total_balance()

        current_pct = hot_balance / total_balance

        if current_pct < self.hot_target - self.threshold:
            # Hot wallet needs top-up
            return ('top_up', self.hot_target - current_pct)
        elif current_pct > self.hot_target + self.threshold:
            # Hot wallet has excess
            return ('sweep', current_pct - self.hot_target)

        return ('ok', None)
```

### Cold Wallet Signing

For air-gapped cold wallet:

1. **Export unsigned transaction** from hot node
2. **Transfer** to cold machine (USB, QR code)
3. **Sign** on cold machine
4. **Transfer** signed transaction back
5. **Broadcast** from hot node

```bash
# On hot machine: Create unsigned transaction
# (Requires custom tooling - contact maintainers)

# On cold machine: Sign transaction
botho sign-offline <unsigned_tx_file>

# On hot machine: Broadcast signed transaction
botho broadcast <signed_tx_file>
```

---

## Security Considerations

### RPC Security

```bash
# Firewall: Only allow RPC from internal network
sudo ufw allow from 10.0.0.0/8 to any port 7101

# Or use nginx reverse proxy with authentication
```

nginx configuration:
```nginx
server {
    listen 7101 ssl;

    ssl_certificate /etc/ssl/certs/botho.crt;
    ssl_certificate_key /etc/ssl/private/botho.key;

    # Basic auth
    auth_basic "Exchange RPC";
    auth_basic_user_file /etc/nginx/.htpasswd;

    # Rate limiting
    limit_req_zone $binary_remote_addr zone=rpc:10m rate=100r/s;
    limit_req zone=rpc burst=200;

    location / {
        proxy_pass http://127.0.0.1:7101;
    }
}
```

### Mnemonic Security

- **Never** store mnemonic in plain text on networked systems
- Use HSM (Hardware Security Module) for production
- Encrypt at rest with passphrase
- Implement key rotation procedures

### Withdrawal Limits

```python
WITHDRAWAL_LIMITS = {
    'per_transaction': 1000,      # BTH
    'per_hour': 5000,             # BTH
    'per_day': 20000,             # BTH
    'require_manual_approval': 500 # BTH
}

def validate_withdrawal(user, amount):
    if amount > WITHDRAWAL_LIMITS['per_transaction']:
        raise LimitExceeded('Transaction limit')

    hourly = get_user_withdrawals_last_hour(user)
    if hourly + amount > WITHDRAWAL_LIMITS['per_hour']:
        raise LimitExceeded('Hourly limit')

    if amount > WITHDRAWAL_LIMITS['require_manual_approval']:
        queue_for_manual_review(user, amount)
        return False

    return True
```

### Audit Logging

```python
import logging

audit_logger = logging.getLogger('audit')
audit_logger.setLevel(logging.INFO)
handler = logging.FileHandler('/var/log/botho/audit.log')
audit_logger.addHandler(handler)

def log_deposit(user_id, tx_hash, amount, confirmations):
    audit_logger.info(f"DEPOSIT user={user_id} tx={tx_hash} amount={amount} conf={confirmations}")

def log_withdrawal(user_id, tx_hash, amount, destination):
    audit_logger.info(f"WITHDRAWAL user={user_id} tx={tx_hash} amount={amount} dest={destination}")
```

---

## Monitoring

### Health Checks

```python
def check_node_health():
    """Comprehensive node health check"""
    try:
        status = rpc('node_getStatus')

        checks = {
            'synced': status['syncStatus'] == 'synced',
            'peers': status['peerCount'] >= 3,
            'recent_block': is_block_recent(status['chainHeight']),
        }

        return all(checks.values()), checks
    except Exception as e:
        return False, {'error': str(e)}

def is_block_recent(height, max_age_minutes=5):
    """Check if latest block is recent"""
    block = rpc('getBlockByHeight', {'height': height})
    block_time = block['timestamp']
    return time.time() - block_time < max_age_minutes * 60
```

### Metrics to Monitor

| Metric | Alert Threshold |
|--------|-----------------|
| Block height | Stale > 10 minutes |
| Peer count | < 3 peers |
| Sync status | Not "synced" |
| Hot wallet balance | < minimum threshold |
| Pending withdrawals | > N transactions |
| RPC response time | > 5 seconds |

### Prometheus Integration

See [deployment.md](deployment.md) for Prometheus exporter setup.

---

## Testing

### Testnet

Before mainnet deployment:

1. Request testnet access from maintainers
2. Test full deposit/withdrawal cycle
3. Verify confirmation counting
4. Test error handling
5. Load test with simulated traffic

### Integration Tests

```python
def test_deposit_detection():
    """Test deposit detection works correctly"""
    # 1. Get current height
    start_height = get_chain_height()

    # 2. Send test transaction
    tx_hash = send_test_deposit()

    # 3. Wait for confirmation
    wait_for_confirmations(tx_hash, 1)

    # 4. Run deposit scanner
    deposits = scanner.scan()

    # 5. Verify deposit detected
    assert any(d['tx_hash'] == tx_hash for d in deposits)

def test_withdrawal_execution():
    """Test withdrawal works correctly"""
    # 1. Get initial balance
    initial = get_hot_wallet_balance()

    # 2. Execute withdrawal
    result = process_withdrawal(TEST_ADDRESS, 1.0)

    # 3. Wait for confirmation
    wait_for_confirmations(result['tx_hash'], 1)

    # 4. Verify balance decreased
    final = get_hot_wallet_balance()
    assert final < initial
```

---

## Troubleshooting

### Common Issues

| Issue | Cause | Solution |
|-------|-------|----------|
| Deposits not detected | Node not synced | Wait for sync, check `node_getStatus` |
| Withdrawal fails | Insufficient balance | Check UTXO availability |
| RPC timeout | Node overloaded | Increase resources, add rate limiting |
| Confirmations stuck | Network congestion | Increase fee, wait |

### Debug Commands

```bash
# Check node status
curl -s http://localhost:7101/ -d '{"jsonrpc":"2.0","method":"node_getStatus","params":{},"id":1}' | jq

# Check wallet balance
curl -s http://localhost:7101/ -d '{"jsonrpc":"2.0","method":"wallet_getBalance","params":{},"id":1}' | jq

# View logs
journalctl -u botho-exchange -f
```

---

## Support

### Getting Help

- GitHub Issues: [github.com/botho-project/botho/issues](https://github.com/botho-project/botho/issues)
- Technical documentation: [docs/](.)

### Listing Requirements

To list BTH on your exchange:

1. Complete technical integration
2. Test on testnet
3. Security review
4. Contact maintainers for mainnet coordination

---

## Checklist

### Pre-Launch

- [ ] Node running and synced
- [ ] Wallet initialized and backed up
- [ ] Deposit detection working
- [ ] Withdrawal processing working
- [ ] Confirmation counting accurate
- [ ] Security measures implemented
- [ ] Monitoring configured
- [ ] Tested on testnet

### Go-Live

- [ ] Hot wallet funded
- [ ] Cold wallet set up
- [ ] Rate limits configured
- [ ] Alerts configured
- [ ] Runbook documented
- [ ] Support team trained

---

## Related Documentation

- [API Reference](api.md) — Complete RPC documentation
- [Developer Guide](developer-guide.md) — Integration examples
- [Security Guide](security.md) — Security best practices
- [Deployment Guide](deployment.md) — Production deployment
