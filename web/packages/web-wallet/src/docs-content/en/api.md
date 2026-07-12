## JSON-RPC API

Botho nodes expose a JSON-RPC 2.0 API on port **7101** (mainnet) or **17101** (testnet) by default; override with `rpc_port` in the config. All requests use the standard JSON-RPC 2.0 format.

### Request Format

```json
{
  "jsonrpc": "2.0",
  "method": "METHOD_NAME",
  "params": { ... },
  "id": 1
}
```

---

## Node Methods

### node_getStatus

Get node status and sync information.

**Response (selected fields):**
- `version` - Node software version
- `network` - Network name (e.g., "botho-testnet")
- `uptimeSeconds` - Node uptime in seconds
- `syncStatus` / `syncProgress` / `synced` - Live sync state
- `chainHeight` - Current blockchain height
- `tipHash` - Hash of the latest block
- `peerCount` / `scpPeerCount` - Connected peers / SCP-participating peers
- `mempoolSize` - Transactions in mempool
- `mintingActive` - Whether minting is enabled
- `quorumFaultTolerant` / `quorumDegenerate` - BFT posture (fault tolerance requires ≥ 4 participating nodes)

The full response also includes build info, SCP slot progress, quorum-gate state, and miner-health fields for monitoring.

---

## Chain Methods

### getChainInfo

Get blockchain information.

**Response:**
- `height` - Current block height
- `tipHash` - Hash of the tip block
- `difficulty` - Current mining difficulty
- `totalMined` - Total coins mined (picocredits, as a string)
- `totalFeesBurned` - Cumulative burned fees (picocredits, as a string)
- `circulatingSupply` - totalMined minus burns (picocredits, as a string)
- `mempoolSize` - Number of pending transactions
- `mempoolFees` - Total fees in mempool

### getBlockByHeight

Get a block by its height.

**Parameters:**
- `height` (number) - Block height

**Response:**
- `height` - Block height
- `hash` - Block hash
- `prevHash` - Previous block hash
- `timestamp` - Block timestamp
- `difficulty` - Block difficulty
- `nonce` - Mining nonce
- `txCount` - Number of transactions
- `mintingReward` - Minting reward amount

### getMempoolInfo

Get mempool statistics.

**Response:**
- `size` - Number of transactions
- `totalFees` - Total fees from all transactions
- `txHashes` - Array of transaction hashes (up to 100)

### estimateFee (alias: tx_estimateFee)

Estimate transaction fee.

**Parameters:**
- `amount` (number) - Transaction amount
- `memos` (number) - Number of encrypted memo fields

**Response:**
- `minimumFee` - Minimum required fee
- `clusterFactor` - Progressive multiplier, scaled ×1000 (1000 = 1x, 6000 = 6x)
- `clusterFactorDisplay` - Human-readable factor (e.g., "1.25x")
- `clusterWealth` - Cluster wealth the factor was derived from
- `recommendedFee` - Recommended fee for normal priority
- `highPriorityFee` - Fee for high priority confirmation

---

## Wallet Methods

### chain_getOutputs

Get transaction outputs for wallet sync.

**Parameters:**
- `start_height` (number) - Starting block height
- `end_height` (number) - Ending block height (max 100 blocks per request)

**Response:** Array of blocks, each containing:
- `height` - Block height
- `outputs` - Array of outputs with `txHash`, `outputIndex`, `targetKey`, `publicKey`, `amountCommitment`

### wallet_getBalance

Get wallet balance (requires local wallet).

**Response:**
- `confirmed` - Confirmed balance
- `pending` - Pending balance
- `total` - Total balance
- `utxoCount` - Number of unspent outputs

### wallet_getAddress

Get wallet keys and address info.

**Response:**
- `viewKey` - Public view key (hex)
- `spendKey` - Public spend key (hex)
- `hasWallet` - Whether node has a wallet configured

---

## Transaction Methods

### tx_submit / sendRawTransaction

Submit a signed transaction.

**Parameters:**
- `tx_hex` (string) - Hex-encoded serialized transaction

**Response:**
- `txHash` - Transaction hash

---

## Minting Methods

### minting_getStatus

Get minting status.

**Response:**
- `active` - Whether minting is enabled
- `threads` - Number of minting threads
- `hashrate` - Current hashrate
- `totalHashes` - Total hashes computed
- `blocksFound` - Blocks mined by this node
- `currentDifficulty` - Current network difficulty
- `uptimeSeconds` - Minting uptime

---

## Network Methods

### network_getInfo

Get network connection information.

**Response:**
- `peerCount` - Total peer count
- `inboundCount` - Inbound connections
- `outboundCount` - Outbound connections
- `bytesSent` - Total bytes sent
- `bytesReceived` - Total bytes received
- `uptimeSeconds` - Connection uptime

### network_getPeers

Get list of connected peers.

**Response:**
- `peers` - Array of peer information

---

## Other Methods

The API surface is larger than this page. Notable additional methods:

| Method | Purpose |
|--------|---------|
| `getBlockByHash` | Fetch a block by hash instead of height |
| `getSupplyInfo` | Emission and supply details |
| `tx_get` / `tx_getStatus` | Look up a transaction / its confirmation status |
| `address_validate` | Check whether an address string is well-formed |
| `fee_getRate` | Current dynamic fee rate |
| `cluster_getWealth` / `cluster_getAllWealth` | Cluster wealth queries (powers the explorer views) |
| `chain_areKeyImagesSpent` | Check key images for double-spend detection |
| `faucet_getStatus` / `faucet_request` | Testnet faucet |
| `operator_*` | Operator trust surface (disabled unless configured; see the operator runbooks) |
