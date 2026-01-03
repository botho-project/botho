# Developer Guide

Build applications that interact with the Botho network.

## Overview

There are several ways to integrate with Botho:

| Approach | Best For | Language |
|----------|----------|----------|
| JSON-RPC API | Web apps, scripts, monitoring | Any |
| WebSocket API | Real-time dashboards, wallets | Any |
| TypeScript SDK | Web/Node.js applications | TypeScript |
| Rust crates | Native applications, extensions | Rust |

---

## Quick Start: JSON-RPC

The simplest way to interact with Botho is via the JSON-RPC API.

### Get Node Status

```bash
curl -X POST http://localhost:7101/ \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "method": "node_getStatus",
    "params": {},
    "id": 1
  }'
```

Response:
```json
{
  "jsonrpc": "2.0",
  "result": {
    "version": "0.1.0",
    "network": "botho-mainnet",
    "chainHeight": 12345,
    "peerCount": 8,
    "mempoolSize": 3,
    "mintingActive": true
  },
  "id": 1
}
```

### Get Block by Height

```bash
curl -X POST http://localhost:7101/ \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "method": "getBlockByHeight",
    "params": {"height": 100},
    "id": 1
  }'
```

### Estimate Transaction Fee

```bash
curl -X POST http://localhost:7101/ \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "method": "estimateFee",
    "params": {"amount": 1000000, "private": true},
    "id": 1
  }'
```

See [API Reference](api.md) for all available methods.

---

## JavaScript/TypeScript Integration

### Using fetch (Browser/Node.js)

```javascript
async function rpcCall(method, params = {}) {
  const response = await fetch('http://localhost:7101/', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      jsonrpc: '2.0',
      method,
      params,
      id: Date.now()
    })
  });

  const json = await response.json();
  if (json.error) {
    throw new Error(json.error.message);
  }
  return json.result;
}

// Usage
const status = await rpcCall('node_getStatus');
console.log(`Chain height: ${status.chainHeight}`);

const block = await rpcCall('getBlockByHeight', { height: 100 });
console.log(`Block hash: ${block.hash}`);
```

### WebSocket Events

```javascript
const ws = new WebSocket('ws://localhost:7101/ws');

ws.onopen = () => {
  // Subscribe to events
  ws.send(JSON.stringify({
    type: 'subscribe',
    events: ['blocks', 'transactions']
  }));
};

ws.onmessage = (event) => {
  const msg = JSON.parse(event.data);

  if (msg.type === 'event') {
    if (msg.event === 'block') {
      console.log(`New block: ${msg.data.height} - ${msg.data.hash}`);
    } else if (msg.event === 'transaction') {
      console.log(`New tx: ${msg.data.hash}`);
    }
  }
};

// Keep connection alive
setInterval(() => {
  if (ws.readyState === WebSocket.OPEN) {
    ws.send(JSON.stringify({ type: 'ping' }));
  }
}, 30000);
```

### Using the TypeScript Adapter

Botho provides TypeScript adapters in `@botho/adapters`:

```typescript
import { RemoteNodeAdapter } from '@botho/adapters';

const adapter = new RemoteNodeAdapter({
  seedNodes: ['https://seed.botho.io'],
});

await adapter.connect();

// Get chain info
const height = await adapter.getBlockHeight();
const stats = await adapter.getNetworkStats();

// Subscribe to new blocks
const unsubscribe = adapter.onNewBlock((block) => {
  console.log(`New block: ${block.height}`);
});

// Clean up
unsubscribe();
adapter.disconnect();
```

---

## Python Integration

```python
import requests
import json

class BothoRPC:
    def __init__(self, url='http://localhost:7101/'):
        self.url = url
        self.id = 0

    def call(self, method, params=None):
        self.id += 1
        payload = {
            'jsonrpc': '2.0',
            'method': method,
            'params': params or {},
            'id': self.id
        }
        response = requests.post(self.url, json=payload)
        result = response.json()
        if 'error' in result:
            raise Exception(result['error']['message'])
        return result['result']

# Usage
rpc = BothoRPC()

# Get node status
status = rpc.call('node_getStatus')
print(f"Chain height: {status['chainHeight']}")
print(f"Peers: {status['peerCount']}")

# Get block
block = rpc.call('getBlockByHeight', {'height': 100})
print(f"Block {block['height']}: {block['hash']}")

# Estimate fee
fee = rpc.call('estimateFee', {'amount': 1000000, 'private': True})
print(f"Recommended fee: {fee['recommendedFee']}")
```

### Python WebSocket

```python
import asyncio
import websockets
import json

async def monitor_blocks():
    async with websockets.connect('ws://localhost:7101/ws') as ws:
        # Subscribe to blocks
        await ws.send(json.dumps({
            'type': 'subscribe',
            'events': ['blocks']
        }))

        async for message in ws:
            msg = json.loads(message)
            if msg.get('type') == 'event' and msg.get('event') == 'block':
                block = msg['data']
                print(f"New block: {block['height']} ({block['tx_count']} txs)")

asyncio.run(monitor_blocks())
```

---

## Rust Integration

### Using the Crates Directly

Add dependencies to `Cargo.toml`:

```toml
[dependencies]
bth-transaction-core = { path = "../transaction/core" }
bth-account-keys = { path = "../account-keys" }
bth-crypto-keys = { path = "../crypto/keys" }
```

### Creating Keys

```rust
use bth_account_keys::AccountKey;

// Generate new account from random seed
let account = AccountKey::random(&mut rand::thread_rng());

// Or from BIP39 mnemonic
use bip39::{Mnemonic, Language};

let mnemonic = Mnemonic::generate_in(Language::English, 24).unwrap();
let seed = mnemonic.to_seed("");
let account = AccountKey::from_seed(&seed);

// Get public keys
let view_public = account.view_public_key();
let spend_public = account.spend_public_key();
```

### Creating Transactions

```rust
use bth_transaction_core::{Transaction, TxBuilder};

let mut builder = TxBuilder::new();

// Add inputs (UTXOs you own)
builder.add_input(utxo, ring_members);

// Add outputs
builder.add_output(recipient_address, amount);

// Build and sign
let tx = builder.build(&account)?;
```

---

## Common Patterns

### Monitoring Chain State

```javascript
class ChainMonitor {
  constructor(rpcUrl) {
    this.rpcUrl = rpcUrl;
    this.lastHeight = 0;
  }

  async start(callback) {
    // Initial state
    const status = await this.rpcCall('node_getStatus');
    this.lastHeight = status.chainHeight;

    // Poll for new blocks
    setInterval(async () => {
      const current = await this.rpcCall('node_getStatus');
      if (current.chainHeight > this.lastHeight) {
        for (let h = this.lastHeight + 1; h <= current.chainHeight; h++) {
          const block = await this.rpcCall('getBlockByHeight', { height: h });
          callback(block);
        }
        this.lastHeight = current.chainHeight;
      }
    }, 5000);
  }

  async rpcCall(method, params = {}) {
    // ... (implementation from above)
  }
}

// Usage
const monitor = new ChainMonitor('http://localhost:7101/');
monitor.start((block) => {
  console.log(`Block ${block.height}: ${block.txCount} transactions`);
});
```

### Fee Estimation

```javascript
async function estimateTransactionFee(amount, hasMemo = false) {
  const result = await rpcCall('estimateFee', {
    amount,
    private: true,
    memos: hasMemo ? 1 : 0
  });

  return {
    minimum: result.minimumFee,
    recommended: result.recommendedFee,
    highPriority: result.highPriorityFee
  };
}
```

### Waiting for Confirmation

```javascript
async function waitForConfirmation(txHash, timeoutMs = 120000) {
  const start = Date.now();

  while (Date.now() - start < timeoutMs) {
    const mempool = await rpcCall('getMempoolInfo');

    // Check if tx is still in mempool
    if (mempool.txHashes.includes(txHash)) {
      await new Promise(r => setTimeout(r, 5000));
      continue;
    }

    // Tx not in mempool - either confirmed or rejected
    // Check recent blocks for confirmation
    const status = await rpcCall('node_getStatus');
    for (let h = status.chainHeight; h > status.chainHeight - 10; h--) {
      const block = await rpcCall('getBlockByHeight', { height: h });
      // Note: would need block.txHashes exposed in RPC
    }

    return true; // Assume confirmed if not in mempool
  }

  throw new Error('Transaction confirmation timeout');
}
```

---

## Building a Simple Block Explorer

```html
<!DOCTYPE html>
<html>
<head>
  <title>Botho Explorer</title>
  <style>
    body { font-family: monospace; padding: 20px; }
    .block { border: 1px solid #ccc; padding: 10px; margin: 10px 0; }
  </style>
</head>
<body>
  <h1>Botho Block Explorer</h1>
  <div id="status"></div>
  <div id="blocks"></div>

  <script>
    const RPC_URL = 'http://localhost:7101/';

    async function rpc(method, params = {}) {
      const res = await fetch(RPC_URL, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ jsonrpc: '2.0', method, params, id: 1 })
      });
      const json = await res.json();
      return json.result;
    }

    async function updateStatus() {
      const status = await rpc('node_getStatus');
      document.getElementById('status').innerHTML = `
        <strong>Height:</strong> ${status.chainHeight} |
        <strong>Peers:</strong> ${status.peerCount} |
        <strong>Mempool:</strong> ${status.mempoolSize} txs
      `;
    }

    async function loadBlocks() {
      const status = await rpc('node_getStatus');
      const blocksDiv = document.getElementById('blocks');
      blocksDiv.innerHTML = '';

      for (let h = status.chainHeight; h > status.chainHeight - 10 && h >= 0; h--) {
        const block = await rpc('getBlockByHeight', { height: h });
        blocksDiv.innerHTML += `
          <div class="block">
            <strong>Block ${block.height}</strong><br>
            Hash: ${block.hash.slice(0, 16)}...<br>
            Time: ${new Date(block.timestamp * 1000).toLocaleString()}<br>
            Transactions: ${block.txCount}<br>
            Reward: ${block.mintingReward / 1e12} BTH
          </div>
        `;
      }
    }

    // Initial load
    updateStatus();
    loadBlocks();

    // Auto-refresh
    setInterval(updateStatus, 5000);
    setInterval(loadBlocks, 30000);
  </script>
</body>
</html>
```

---

## Error Handling

### RPC Error Codes

| Code | Meaning |
|------|---------|
| -32700 | Parse error (invalid JSON) |
| -32600 | Invalid request |
| -32601 | Method not found |
| -32602 | Invalid params |
| -32603 | Internal error |
| -32000 | Application error |

### Robust Error Handling

```javascript
class BothoClient {
  constructor(url, options = {}) {
    this.url = url;
    this.timeout = options.timeout || 10000;
    this.retries = options.retries || 3;
  }

  async call(method, params = {}) {
    let lastError;

    for (let attempt = 0; attempt < this.retries; attempt++) {
      try {
        const controller = new AbortController();
        const timeoutId = setTimeout(() => controller.abort(), this.timeout);

        const response = await fetch(this.url, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            jsonrpc: '2.0',
            method,
            params,
            id: Date.now()
          }),
          signal: controller.signal
        });

        clearTimeout(timeoutId);

        const json = await response.json();

        if (json.error) {
          throw new RPCError(json.error.code, json.error.message);
        }

        return json.result;
      } catch (err) {
        lastError = err;
        if (err.name === 'AbortError') {
          console.warn(`Timeout on attempt ${attempt + 1}`);
        }
        await new Promise(r => setTimeout(r, 1000 * (attempt + 1)));
      }
    }

    throw lastError;
  }
}

class RPCError extends Error {
  constructor(code, message) {
    super(message);
    this.code = code;
    this.name = 'RPCError';
  }
}
```

---

## Security Considerations

### CORS

When building web applications, ensure your origin is allowed:

```toml
# Node config (~/.botho/config.toml)
[network]
cors_origins = ["https://yourdomain.com"]
```

### Rate Limiting

Implement client-side rate limiting to be a good network citizen:

```javascript
class RateLimitedClient {
  constructor(client, requestsPerSecond = 10) {
    this.client = client;
    this.minInterval = 1000 / requestsPerSecond;
    this.lastRequest = 0;
  }

  async call(method, params) {
    const now = Date.now();
    const elapsed = now - this.lastRequest;

    if (elapsed < this.minInterval) {
      await new Promise(r => setTimeout(r, this.minInterval - elapsed));
    }

    this.lastRequest = Date.now();
    return this.client.call(method, params);
  }
}
```

### Input Validation

Always validate user input before sending to the node:

```javascript
function validateAddress(address) {
  // Botho addresses are hex-encoded public keys
  if (!/^[0-9a-fA-F]{64}$/.test(address)) {
    throw new Error('Invalid address format');
  }
  return address.toLowerCase();
}

function validateAmount(amount) {
  const num = BigInt(amount);
  if (num <= 0n) {
    throw new Error('Amount must be positive');
  }
  return num;
}
```

---

## Next Steps

- [API Reference](api.md) — Complete RPC method documentation
- [Architecture](concepts/architecture.md) — Understand how Botho works
- [Testing Guide](testing.md) — Test your integration
