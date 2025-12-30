# Configuration Reference

Botho uses a single TOML configuration file located at `~/.botho/config.toml`.

## Full Example

```toml
[wallet]
# BIP39 mnemonic (24 words) - KEEP SECRET
mnemonic = "word1 word2 word3 ... word24"

[network]
# Port for gossip protocol
gossip_port = 7100

# Port for JSON-RPC API
rpc_port = 7101

# Bootstrap peers for network discovery
bootstrap_peers = [
    "/ip4/98.95.2.200/tcp/7100/p2p/12D3KooWBrjTYjNrEwi9MM3AKFenmymyWVXtXbQiSx7eDnDwv9qQ",
]

# Allowed origins for browser-based wallets
cors_origins = ["http://localhost", "http://127.0.0.1", "https://botho.io"]

# Quorum configuration for consensus
[network.quorum]
mode = "recommended"  # or "explicit"
min_peers = 1         # For recommended mode: minimum peers before minting
threshold = 2         # For explicit mode: required agreement count
members = []          # For explicit mode: list of trusted peer IDs

[minting]
enabled = false
threads = 0  # 0 = auto-detect CPU count
```

## Sections

### [wallet]

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `mnemonic` | string | (generated) | BIP39 24-word mnemonic phrase |

**Security:** The mnemonic is stored in plain text. Ensure the config file has restrictive permissions (mode 0600).

### [network]

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `gossip_port` | integer | 7100 | Port for libp2p gossipsub communication |
| `rpc_port` | integer | 7101 | Port for JSON-RPC server |
| `bootstrap_peers` | array | [] | Initial peers to connect to |
| `cors_origins` | array | ["http://localhost", "http://127.0.0.1"] | Allowed CORS origins for RPC |

**Bootstrap peer format:** Multiaddr format, e.g., `/ip4/192.168.1.100/tcp/7100/p2p/<peer_id>`

**CORS origins:** For browser-based wallets, add your domain. Use `"*"` to allow all origins (not recommended for production).

### [network.quorum]

Controls how the node participates in SCP consensus.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `mode` | string | "recommended" | Either "recommended" or "explicit" |
| `min_peers` | integer | 1 | (Recommended mode) Minimum peers before minting |
| `threshold` | integer | 2 | (Explicit mode) Required agreement count |
| `members` | array | [] | (Explicit mode) List of trusted peer IDs |

#### Recommended Mode

Automatically trusts discovered peers and calculates a BFT-safe threshold:

```toml
[network.quorum]
mode = "recommended"
min_peers = 1
```

The threshold is calculated as `n - f` where `f = (n - 1) / 3` (failures tolerated).

| Nodes | Threshold | Fault Tolerance |
|-------|-----------|-----------------|
| 2     | 2-of-2    | 0               |
| 3     | 2-of-3    | 1               |
| 4     | 3-of-4    | 1               |
| 5     | 4-of-5    | 1               |
| 6     | 4-of-6    | 2               |
| 7     | 5-of-7    | 2               |

#### Explicit Mode

Manually specify trusted peers and threshold:

```toml
[network.quorum]
mode = "explicit"
threshold = 2
members = [
    "12D3KooWBootstrap...",
    "12D3KooWMinter1...",
]
```

Use explicit mode for:
- Private networks
- Specific trust relationships
- High-security deployments

### [minting]

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enabled` | boolean | false | Whether to mine |
| `threads` | integer | 0 | Number of minting threads (0 = auto-detect) |

## Example Configurations

### First Minter Joining Network

```toml
[minting]
enabled = true
threads = 4

[network.quorum]
mode = "explicit"
threshold = 2
members = ["12D3KooWBootstrapPeerIdHere..."]
```

### Established Minter (Auto-Trust)

```toml
[minting]
enabled = true

[network.quorum]
mode = "recommended"
min_peers = 2
```

### Bootstrap Server (No Minting)

```toml
[minting]
enabled = false

[network.quorum]
mode = "recommended"
min_peers = 1
```

### Non-Minting Full Node

```toml
[minting]
enabled = false

[network]
bootstrap_peers = [
    "/ip4/98.95.2.200/tcp/7100/p2p/12D3KooWBrjTYjNrEwi9MM3AKFenmymyWVXtXbQiSx7eDnDwv9qQ",
]

[network.quorum]
mode = "recommended"
min_peers = 1
```

## Environment Variables

Currently, Botho does not support environment variable configuration. All settings must be in the config file.

## File Permissions

The config file contains your wallet mnemonic. Ensure it has restrictive permissions:

```bash
chmod 600 ~/.botho/config.toml
```

Botho will warn if the file permissions are too permissive.
