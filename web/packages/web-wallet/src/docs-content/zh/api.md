## JSON-RPC API

Botho 节点默认在端口 **7101**（主网）或 **17101**（测试网）上暴露 JSON-RPC 2.0 API；可通过配置中的 `rpc_port` 覆盖。所有请求均使用标准的 JSON-RPC 2.0 格式。

### 请求格式

```json
{
  "jsonrpc": "2.0",
  "method": "METHOD_NAME",
  "params": { ... },
  "id": 1
}
```

---

## 节点方法

### node_getStatus

获取节点状态和同步信息。

**响应（部分字段）：**
- `version` - 节点软件版本
- `network` - 网络名称（例如 "botho-testnet"）
- `uptimeSeconds` - 节点运行时长（秒）
- `syncStatus` / `syncProgress` / `synced` - 实时同步状态
- `chainHeight` - 当前区块链高度
- `tipHash` - 最新区块的哈希
- `peerCount` / `scpPeerCount` - 已连接的对等节点 / 参与 SCP 的对等节点
- `mempoolSize` - 内存池中的交易数
- `mintingActive` - 是否已启用铸造
- `quorumFaultTolerant` / `quorumDegenerate` - BFT 状态（容错需要 ≥ 4 个参与节点）

完整响应还包含构建信息、SCP 槽位进度、法定人数门控状态，以及用于监控的矿工健康字段。

---

## 链方法

### getChainInfo

获取区块链信息。

**响应：**
- `height` - 当前区块高度
- `tipHash` - 链尖区块的哈希
- `difficulty` - 当前挖矿难度
- `totalMined` - 已挖出的总币量（picocredits，以字符串表示）
- `totalFeesBurned` - 累计销毁的手续费（picocredits，以字符串表示）
- `circulatingSupply` - totalMined 减去销毁部分（picocredits，以字符串表示）
- `mempoolSize` - 待处理交易的数量
- `mempoolFees` - 内存池中的总手续费

### getBlockByHeight

按高度获取区块。

**参数：**
- `height`（number）- 区块高度

**响应：**
- `height` - 区块高度
- `hash` - 区块哈希
- `prevHash` - 前一区块哈希
- `timestamp` - 区块时间戳
- `difficulty` - 区块难度
- `nonce` - 挖矿 nonce
- `txCount` - 交易数量
- `mintingReward` - 铸造奖励金额

### getMempoolInfo

获取内存池统计信息。

**响应：**
- `size` - 交易数量
- `totalFees` - 所有交易的总手续费
- `txHashes` - 交易哈希数组（最多 100 个）

### estimateFee（别名：tx_estimateFee）

估算交易手续费。

**参数：**
- `amount`（number）- 交易金额
- `memos`（number）- 加密备注字段的数量

**响应：**
- `minimumFee` - 最低所需手续费
- `clusterFactor` - 渐进式乘数，按 ×1000 缩放（1000 = 1x，6000 = 6x）
- `clusterFactorDisplay` - 人类可读的系数（例如 "1.25x"）
- `clusterWealth` - 推导该系数所依据的集群财富
- `recommendedFee` - 普通优先级的推荐手续费
- `highPriorityFee` - 高优先级确认的手续费

---

## 钱包方法

### chain_getOutputs

获取用于钱包同步的交易输出。

**参数：**
- `start_height`（number）- 起始区块高度
- `end_height`（number）- 结束区块高度（每次请求最多 100 个区块）

**响应：** 区块数组，每个区块包含：
- `height` - 区块高度
- `outputs` - 输出数组，包含 `txHash`、`outputIndex`、`targetKey`、`publicKey`、`amountCommitment`

### wallet_getBalance

获取钱包余额（需要本地钱包）。

**响应：**
- `confirmed` - 已确认余额
- `pending` - 待处理余额
- `total` - 总余额
- `utxoCount` - 未花费输出的数量

### wallet_getAddress

获取钱包密钥和地址信息。

**响应：**
- `viewKey` - 公开的视图密钥（hex）
- `spendKey` - 公开的花费密钥（hex）
- `hasWallet` - 节点是否已配置钱包

---

## 交易方法

### tx_submit / sendRawTransaction

提交已签名的交易。

**参数：**
- `tx_hex`（string）- 十六进制编码的序列化交易

**响应：**
- `txHash` - 交易哈希

---

## 铸造方法

### minting_getStatus

获取铸造状态。

**响应：**
- `active` - 是否已启用铸造
- `threads` - 铸造线程数
- `hashrate` - 当前算力
- `totalHashes` - 已计算的哈希总数
- `blocksFound` - 本节点挖出的区块数
- `currentDifficulty` - 当前网络难度
- `uptimeSeconds` - 铸造运行时长

---

## 网络方法

### network_getInfo

获取网络连接信息。

**响应：**
- `peerCount` - 对等节点总数
- `inboundCount` - 入站连接数
- `outboundCount` - 出站连接数
- `bytesSent` - 已发送的总字节数
- `bytesReceived` - 已接收的总字节数
- `uptimeSeconds` - 连接运行时长

### network_getPeers

获取已连接对等节点的列表。

**响应：**
- `peers` - 对等节点信息数组

---

## 其他方法

API 的范围比本页所列更广。值得关注的其他方法：

| 方法 | 用途 |
|--------|---------|
| `getBlockByHash` | 按哈希而非高度获取区块 |
| `getSupplyInfo` | 发行和供应量详情 |
| `tx_get` / `tx_getStatus` | 查询交易 / 其确认状态 |
| `address_validate` | 检查地址字符串是否格式正确 |
| `fee_getRate` | 当前动态手续费费率 |
| `cluster_getWealth` / `cluster_getAllWealth` | 集群财富查询（为浏览器视图提供支持） |
| `chain_areKeyImagesSpent` | 检查密钥镜像以进行双花检测 |
| `faucet_getStatus` / `faucet_request` | 测试网水龙头 |
| `operator_*` | 运营者信任接口（除非配置，否则禁用；参见运营者操作手册） |
