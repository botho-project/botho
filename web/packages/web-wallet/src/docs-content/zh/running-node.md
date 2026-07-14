## 运行 Botho 节点

运行你自己的 Botho 节点能为你提供最高级别的隐私，并有助于增强网络。当你运行节点时，你的钱包直接连接到区块链，无需依赖第三方服务器。

### 为什么要运行节点？

**隐私：** 当你使用轻钱包或网页钱包时，你是在信任服务器不会记录你的地址或交易。运行你自己的节点意味着你的钱包活动始终保留在你的机器上。

**验证：** 你的节点独立验证每一笔交易和每一个区块。你无需信任任何人对网络状态的说法。

**网络健康：** 更多的节点使网络更具韧性。你的节点转发交易和区块，帮助网络运转。

**铸造优势：** 运行你自己的节点能在铸造竞争中为你带来更低的延迟。更快接收到新区块的节点可以更早开始处理下一个区块，从而提高赚取铸造奖励的机会。

**参与：** 如果你想铸造新区块或参与共识，你需要一个全节点。

### 系统要求

**最低要求：**
- 2 个 CPU 核心
- 4 GB 内存
- 50 GB SSD 存储
- 10 Mbps 互联网连接

**推荐配置：**
- 4 个以上 CPU 核心
- 8 GB 内存
- 100 GB NVMe SSD
- 100 Mbps 互联网连接

区块链目前规模较小，但存储需求会随时间增长。

### 安装

**从源码安装（推荐）：**

```bash
# Install Rust if you haven't already
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Clone the repository
git clone https://github.com/botho-project/botho.git
cd botho

# Build in release mode
cargo build --release

# The binary is at ./target/release/botho
```

**首次设置：**

```bash
# Initialize a new wallet and configuration
./target/release/botho init

# This will:
# - Generate a 24-word recovery phrase
# - Create your config and wallet under ~/.botho/

# Variants:
#   botho init --recover   # restore a wallet from an existing mnemonic
#   botho init --relay     # relay/seed node config with no wallet
```

**重要：** 请记下你的恢复助记词并安全保管！

### 运行节点

**基本操作：**

```bash
# Start the node and sync with the network
./target/release/botho run
```

**启用铸造：**

```bash
# Start the node and participate in block production
./target/release/botho run --mint
```

### CLI 命令参考

**钱包命令：**

| 命令 | 说明 |
|---------|-------------|
| `botho init` | 创建一个带有 24 个单词助记词的新钱包 |
| `botho balance` | 显示你当前的钱包余额 |
| `botho address` | 显示你的收款地址（`--save` 会将其写入文件） |
| `botho send <address> <amount>` | 发送 BTH（金额以 BTH 计；`--memo` 用于附加加密备注） |

所有发送均使用 CLSAG 环签名——发送方隐私默认开启，而非一个开关。

**节点命令：**

| 命令 | 说明 |
|---------|-------------|
| `botho run` | 启动节点并与网络同步 |
| `botho run --mint` | 启动并启用铸造（`--mint-threads N` 用于限制 CPU 使用） |
| `botho status` | 显示节点同步状态和对等节点数量 |
| `botho snapshot` | 管理用于快速初始同步的 UTXO 快照 |

### 配置

配置文件位于 `~/.botho/` 下。所有端口都有针对各网络的默认值，因此一份最简配置即可开箱即用：

```toml
# "mainnet" or "testnet"
network_type = "testnet"

[network]
# Defaults: gossip 7100 (mainnet) / 17100 (testnet)
#           RPC    7101 (mainnet) / 17101 (testnet)
#           metrics 9090 (mainnet) / 19090 (testnet), 0 disables
# gossip_port = 17100
# rpc_port = 17101
# metrics_port = 19090

# Optional explicit bootstrap peers (multiaddr format).
# If unset, peers are discovered via DNS seed TXT records
# (seeds.botho.io / seeds.testnet.botho.io).
# bootstrap_peers = ["/dns4/eu.seed.botho.io/tcp/7100/p2p/<peer-id>"]

[minting]
enabled = false
threads = 0   # 0 = use all CPU cores
```

### 防火墙配置

如果你希望你的节点接受入站连接（推荐）：

```bash
# Allow P2P gossip traffic (17100 on testnet, 7100 on mainnet)
sudo ufw allow 17100/tcp

# Optional: Allow RPC access (only if needed externally)
# sudo ufw allow 17101/tcp
```

### 故障排查

**节点无法同步：**
- 检查你的互联网连接
- 确认防火墙允许在 gossip 端口上的出站连接
- 万不得已时，清除 `~/.botho/` 下的链数据库并重新同步（仅限测试网——这会从创世块重新扫描）

**内存占用过高：**
- 减少铸造线程数（RandomX 会保留一个较大的内存数据集）
- 如果内存有限，考虑增加交换空间（swap）

**无法连接到对等节点：**
- 确保你的 gossip 端口（测试网 17100 / 主网 7100）已开放以接受入站连接
- 检查你是否处于严格的 NAT 之后
