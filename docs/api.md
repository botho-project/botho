# RPC API Reference

Botho provides a JSON-RPC 2.0 API for thin wallets, web interfaces, and programmatic access to node functionality.

## Connection

The RPC server listens on port `7101` by default.

```bash
# HTTP endpoint
http://localhost:7101/

# WebSocket endpoint (for real-time events)
ws://localhost:7101/ws
```

## Request Format

All requests use JSON-RPC 2.0:

```json
{
  "jsonrpc": "2.0",
  "method": "method_name",
  "params": { ... },
  "id": 1
}
```

## Response Format

Success:
```json
{
  "jsonrpc": "2.0",
  "result": { ... },
  "id": 1
}
```

Error:
```json
{
  "jsonrpc": "2.0",
  "error": {
    "code": -32000,
    "message": "Error description"
  },
  "id": 1
}
```

---

## Node Methods

### `node_getStatus`

Get overall node status.

**Parameters:** None

**Response:**
| Field | Type | Description |
|-------|------|-------------|
| `version` | string | Node version (e.g., "0.1.0") |
| `network` | string | Network identifier |
| `uptimeSeconds` | integer | Node uptime in seconds |
| `syncStatus` | string | Sync status ("synced", "syncing") |
| `chainHeight` | integer | Current blockchain height |
| `tipHash` | string | Hash of the latest block (hex) |
| `peerCount` | integer | Number of connected peers |
| `mempoolSize` | integer | Number of pending transactions |
| `mintingActive` | boolean | Whether minting is enabled |

**Example:**
```bash
curl -X POST http://localhost:7101/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"node_getStatus","params":{},"id":1}'
```

---

## Chain Methods

### `getChainInfo`

Get blockchain information including supply metrics.

**Parameters:** None

**Response:**
| Field | Type | Description |
|-------|------|-------------|
| `height` | integer | Current block height |
| `tipHash` | string | Hash of the latest block (hex) |
| `difficulty` | integer | Current mining difficulty |
| `totalMined` | integer | Gross emission (all BTH ever minted), in nanoBTH |
| `totalFeesBurned` | integer | Cumulative transaction fees burned, in nanoBTH |
| `circulatingSupply` | integer | Net supply (totalMined - totalFeesBurned), in nanoBTH |
| `mempoolSize` | integer | Pending transaction count |
| `mempoolFees` | integer | Total fees in mempool |

**Example:**
```bash
curl -X POST http://localhost:7101/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"getChainInfo","params":{},"id":1}'
```

---

### `getSupplyInfo`

Get circulating supply information. Useful for exchanges and block explorers to accurately report BTH supply.

**Parameters:** None

**Response:**
| Field | Type | Description |
|-------|------|-------------|
| `height` | integer | Current block height |
| `totalMined` | integer | Gross emission (all BTH ever minted), in nanoBTH |
| `totalFeesBurned` | integer | Cumulative transaction fees burned, in nanoBTH |
| `circulatingSupply` | integer | Net supply (totalMined - totalFeesBurned), in nanoBTH |

**Note:** All values are in nanoBTH (1 BTH = 1,000,000,000 nanoBTH). Transaction fees are burned (removed from circulation), which is why circulating supply = totalMined - totalFeesBurned.

**Example:**
```bash
curl -X POST http://localhost:7101/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"getSupplyInfo","params":{},"id":1}'
```

**Response:**
```json
{
  "jsonrpc": "2.0",
  "result": {
    "height": 12345,
    "totalMined": 50000000000000000,
    "totalFeesBurned": 123456789000,
    "circulatingSupply": 49999876543211000
  },
  "id": 1
}
```

---

### `getBlockByHeight`

Get a block by its height.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `height` | integer | Yes | Block height to retrieve |

**Response:**
| Field | Type | Description |
|-------|------|-------------|
| `height` | integer | Block height |
| `hash` | string | Block hash (hex) |
| `prevHash` | string | Previous block hash (hex) |
| `timestamp` | integer | Unix timestamp |
| `difficulty` | integer | Block difficulty |
| `nonce` | integer | Mining nonce |
| `txCount` | integer | Number of transactions |
| `mintingReward` | integer | Block reward |

**Example:**
```bash
curl -X POST http://localhost:7101/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"getBlockByHeight","params":{"height":100},"id":1}'
```

---

### `getMempoolInfo`

Get mempool information.

**Parameters:** None

**Response:**
| Field | Type | Description |
|-------|------|-------------|
| `size` | integer | Number of transactions |
| `totalFees` | integer | Sum of all fees |
| `txHashes` | string[] | Transaction hashes (up to 100) |

---

### `chain_getOutputs`

Get transaction outputs for a range of blocks. Used by thin wallets to scan for owned outputs.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `start_height` | integer | Yes | Start block height |
| `end_height` | integer | No | End block height (default: start + 100) |

**Response:** Array of blocks, each containing:
| Field | Type | Description |
|-------|------|-------------|
| `height` | integer | Block height |
| `outputs` | array | Transaction outputs in this block |

Each output contains:
| Field | Type | Description |
|-------|------|-------------|
| `txHash` | string | Transaction hash (hex) |
| `outputIndex` | integer | Output index within transaction |
| `targetKey` | string | Stealth address target key (hex) |
| `publicKey` | string | One-time public key (hex) |
| `amountCommitment` | string | Amount commitment (hex) |

---

## Transaction Methods

### `tx_submit` / `sendRawTransaction`

Submit a signed transaction to the network.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `tx_hex` | string | Yes | Serialized transaction (hex-encoded bincode) |

**Response:**
| Field | Type | Description |
|-------|------|-------------|
| `txHash` | string | Transaction hash (hex) |

**Errors:**
| Code | Message |
|------|---------|
| -32602 | Missing tx_hex parameter |
| -32602 | Invalid hex encoding |
| -32602 | Invalid transaction (deserialization failed) |
| -32000 | Failed to add transaction (validation error) |

---

### `pq_tx_submit`

Submit a post-quantum signed transaction.

**Parameters:**
| Name | Type | Required | Description |
|------|------|----------|-------------|
| `tx_hex` | string | Yes | Serialized PQ transaction (hex-encoded bincode) |

**Response:**
| Field | Type | Description |
|-------|------|-------------|
| `txHash` | string | Transaction hash (hex) |
| `type` | string | Always "quantum-private" |
| `size` | integer | Transaction size in bytes |

**Note:** Requires the node to be built with the `pq` feature.

---

### `estimateFee` / `tx_estimateFee`

Estimate the transaction fee.

**Parameters:**
| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `amount` | integer | No | 0 | Transaction amount in nanoBTH |
| `private` | boolean | No | true | Whether transaction uses privacy features |
| `memos` | integer | No | 0 | Number of encrypted memos |

**Response:**
| Field | Type | Description |
|-------|------|-------------|
| `minimumFee` | integer | Minimum acceptable fee |
| `feeRateBps` | integer | Fee rate in basis points |
| `recommendedFee` | integer | Recommended fee for normal priority |
| `highPriorityFee` | integer | Fee for high priority confirmation |
| `params` | object | Echo of input parameters |

**Example:**
```bash
curl -X POST http://localhost:7101/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"estimateFee","params":{"amount":1000000,"private":true},"id":1}'
```

---

## Wallet Methods

### `wallet_getBalance`

Get wallet balance (requires node to have wallet configured).

**Parameters:** None

**Response:**
| Field | Type | Description |
|-------|------|-------------|
| `confirmed` | integer | Confirmed balance |
| `pending` | integer | Pending balance |
| `total` | integer | Total balance |
| `utxoCount` | integer | Number of unspent outputs |

**Note:** For thin wallets, sync locally using `chain_getOutputs` instead.

---

### `wallet_getAddress`

Get wallet public keys.

**Parameters:** None

**Response:**
| Field | Type | Description |
|-------|------|-------------|
| `viewKey` | string | Public view key (hex) |
| `spendKey` | string | Public spend key (hex) |
| `hasWallet` | boolean | Whether wallet is configured |

---

## Minting Methods

### `minting_getStatus`

Get minting (mining) status.

**Parameters:** None

**Response:**
| Field | Type | Description |
|-------|------|-------------|
| `active` | boolean | Whether minting is running |
| `threads` | integer | Number of minting threads |
| `hashrate` | float | Current hash rate (H/s) |
| `totalHashes` | integer | Total hashes computed |
| `blocksFound` | integer | Blocks found by this node |
| `currentDifficulty` | integer | Current network difficulty |
| `uptimeSeconds` | integer | Minting uptime |

---

## Network Methods

### `network_getInfo`

Get network statistics.

**Parameters:** None

**Response:**
| Field | Type | Description |
|-------|------|-------------|
| `peerCount` | integer | Total connected peers |
| `inboundCount` | integer | Inbound connections |
| `outboundCount` | integer | Outbound connections |
| `bytesSent` | integer | Total bytes sent |
| `bytesReceived` | integer | Total bytes received |
| `uptimeSeconds` | integer | Network uptime |

---

### `network_getPeers`

Get connected peer information.

**Parameters:** None

**Response:**
| Field | Type | Description |
|-------|------|-------------|
| `peers` | array | List of connected peers |

---

## WebSocket API

Connect to `ws://localhost:7101/ws` for real-time event streaming.

### Subscribing to Events

Send a subscribe message:
```json
{
  "type": "subscribe",
  "events": ["blocks", "transactions", "mempool", "peers", "minting"]
}
```

The server confirms:
```json
{
  "type": "subscribed",
  "events": ["blocks", "transactions"]
}
```

### Unsubscribing

```json
{
  "type": "unsubscribe",
  "events": ["transactions"]
}
```

### Ping/Pong

Keep the connection alive:
```json
{"type": "ping"}
```

Response:
```json
{"type": "pong"}
```

### Event Types

#### `block` - New Block

```json
{
  "type": "event",
  "event": "block",
  "data": {
    "height": 12345,
    "hash": "abc123...",
    "timestamp": 1704067200,
    "tx_count": 5,
    "difficulty": 1000000
  }
}
```

#### `transaction` - New Transaction

```json
{
  "type": "event",
  "event": "transaction",
  "data": {
    "hash": "def456...",
    "fee": 1000,
    "in_block": null
  }
}
```

`in_block` is `null` for mempool transactions, or the block height if confirmed.

#### `mempool` - Mempool Update

```json
{
  "type": "event",
  "event": "mempool",
  "data": {
    "size": 42,
    "total_fees": 50000
  }
}
```

#### `peers` - Peer Status Change

```json
{
  "type": "event",
  "event": "peers",
  "data": {
    "peer_count": 8,
    "event": {
      "connected": { "peer_id": "12D3KooW..." }
    }
  }
}
```

Event types: `connected`, `disconnected`, `count_changed`

#### `minting` - Minting Status Update

```json
{
  "type": "event",
  "event": "minting",
  "data": {
    "active": true,
    "hashrate": 125000.5,
    "blocks_found": 3
  }
}
```

---

## Error Codes

| Code | Meaning |
|------|---------|
| -32700 | Parse error (invalid JSON) |
| -32600 | Invalid request |
| -32601 | Method not found |
| -32602 | Invalid params |
| -32603 | Internal error |
| -32000 | Application error (see message) |

---

## CORS

The RPC server supports CORS for browser-based clients. Configure allowed origins in `config.toml`:

```toml
[rpc]
cors_origins = ["http://localhost", "http://127.0.0.1", "https://botho.io"]
```

Use `"*"` to allow all origins (not recommended for production).
