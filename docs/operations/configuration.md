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

# Local IP the gossip, RPC and metrics listeners bind to (optional).
# Unset = "0.0.0.0" (all IPv4 interfaces). Use "127.0.0.1" to keep all three
# loopback-only, or a specific interface IP such as "10.0.0.5". IPv6 ("::1",
# "::") is accepted too.
# listen_address = "0.0.0.0"

# Bootstrap peers for network discovery
bootstrap_peers = [
    "/ip4/98.95.2.200/tcp/7100/p2p/12D3KooWBrjTYjNrEwi9MM3AKFenmymyWVXtXbQiSx7eDnDwv9qQ",
]

# Allowed origins for browser-based wallets
cors_origins = ["http://localhost", "http://127.0.0.1", "https://botho.io"]

# Quorum configuration for consensus
[network.quorum]
mode = "recommended"   # or "explicit"
fault_model = "crash"  # (recommended mode) "crash" (default) or "bft"
min_peers = 1          # For recommended mode: minimum peers before minting
threshold = 2          # For explicit mode: required agreement count
members = []           # For explicit mode: list of trusted peer IDs

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
| `metrics_port` | integer | 9090 | Port for the Prometheus metrics endpoint (19090 on testnet); `0` disables it |
| `listen_address` | string | "0.0.0.0" | Local IP address the gossip, RPC **and** metrics listeners bind to. IPv4 or IPv6, bare address only (no port or brackets). Invalid values fail config load |
| `bootstrap_peers` | array | [] | Initial peers to connect to. Empty means "use the defaults" (DNS discovery and/or the built-in seeds), not "no peers" |
| `fallback_seeds` | bool | true | Fall back to the built-in **public** seed nodes when `bootstrap_peers` is empty and DNS discovery is disabled, fails, or returns no records. Set to `false` to isolate a node or private cluster |
| `cors_origins` | array | ["http://localhost", "http://127.0.0.1"] | Allowed CORS origins for RPC |

**Listen address:** one key covers all three listeners; there are no
per-listener addresses. With `listen_address = "127.0.0.1"` the node is not
reachable from other machines on any port, so it cannot accept inbound gossip
peers (outbound dials still work) and remote wallets cannot reach its RPC.
To expose gossip publicly while keeping RPC/metrics private, leave the default
and restrict RPC/metrics with a firewall. At startup the node logs
`Gossip listening on <multiaddr>`.

**Bootstrap peer format:** Multiaddr format, e.g., `/ip4/192.168.1.100/tcp/7100/p2p/<peer_id>`

**Bootstrap order:** (1) `bootstrap_peers`, if non-empty; (2) DNS TXT discovery,
if `[network.dns_seeds] enabled = true` (the default); (3) the hardcoded
fallback seeds, unless `fallback_seeds = false`. Step (3) also applies when DNS
is enabled but the query fails or returns no records. The resolved list is
dialed at startup and re-dialed by the reconnect tick whenever the node is
under-connected. At startup the node logs `Resolved bootstrap peers` with
`source` = `configured`, `dns`, `fallback_seeds` or `none`.

The `[network.dns_seeds]` table takes `enabled` (bool, default `true`) and an
optional `domain` override. DNS with a private `domain` and `fallback_seeds =
false` is a valid combination: the node uses only what your DNS publishes.

**CORS origins:** For browser-based wallets, add your domain. Use `"*"` to allow all origins (not recommended for production).

### [network.quorum]

Controls how the node participates in SCP consensus.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `mode` | string | "recommended" | Either "recommended" or "explicit" |
| `fault_model` | string | "crash" | (Recommended mode) "crash" (2f+1) or "bft" (3f+1) |
| `min_peers` | integer | 1 | (Recommended mode) Minimum peers before minting |
| `threshold` | integer | 2 | (Explicit mode) Required agreement count |
| `members` | array | [] | (Explicit mode) List of trusted peer IDs |

#### Recommended Mode

Automatically trusts discovered peers and calculates a quorum threshold from
the configured **fault model** over `n` (peers + self).

```toml
[network.quorum]
mode = "recommended"
fault_model = "crash"   # default; use "bft" for a Byzantine posture
min_peers = 1
```

##### Fault model: `crash` (default)

Crash-fault tolerance via a **2f+1 simple majority**: `threshold = floor(n/2) + 1`.
This is the recommended posture for trusted homogeneous clusters (the testnet
default). It gives genuine high-availability / liveness — a single crashed or
lagging node cannot stall consensus — and matches Stellar Core's homogeneous
default. Crash-fault safety (no fork) is preserved because any two majority
subsets always intersect, and block-apply tip checks are unchanged.

| Nodes | Threshold | Crash faults tolerated |
|-------|-----------|------------------------|
| 1     | 1-of-1    | 0                      |
| 2     | 2-of-2    | 0                      |
| 3     | 2-of-3    | 1                      |
| 4     | 3-of-4    | 1                      |
| 5     | 3-of-5    | 2                      |
| 6     | 4-of-6    | 2                      |

##### Fault model: `bft`

Byzantine-fault tolerance via a **3f+1 quorum**: `threshold = n - floor((n-1)/3)`.
Tolerates up to `f` arbitrarily malicious (Byzantine) members. Note that
**genuine BFT requires at least 4 nodes**; at `n <= 3` this collapses to
unanimity (n-of-n) and tolerates zero faults.

```toml
[network.quorum]
mode = "recommended"
fault_model = "bft"
```

| Nodes | Threshold | Byzantine faults tolerated |
|-------|-----------|----------------------------|
| 1     | 1-of-1    | 0                          |
| 2     | 2-of-2    | 0                          |
| 3     | 3-of-3    | 0                          |
| 4     | 3-of-4    | 1                          |
| 5     | 4-of-5    | 1                          |
| 6     | 5-of-6    | 1                          |

An invalid `fault_model` value (anything other than `crash` or `bft`) is
rejected at config load with a clear error.

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

### Isolated Cluster (devnet / CI / qualification)

A node or private cluster that must never dial the public network. Set both
keys: disabling DNS alone still falls back to the built-in public seeds.

```toml
[network]
fallback_seeds = false
# Empty for a standalone node (or the key-generation boot that learns the
# peer ID); otherwise list only cluster members.
bootstrap_peers = [
    "/ip4/10.0.0.2/tcp/17100/p2p/<peer_id_of_node_2>",
]

[network.dns_seeds]
enabled = false
```

With an empty list the node logs that it is isolated and only accepts inbound
peers. Neither the startup dial nor the reconnect tick will contact a
hardcoded seed. Note that the gossip, RPC and metrics servers still bind
`0.0.0.0` (#1582), so restrict inbound traffic with a firewall or security
group as well.

## Environment Variables

Currently, Botho does not support environment variable configuration. All settings must be in the config file.

## File Permissions

The config file contains your wallet mnemonic. Ensure it has restrictive permissions:

```bash
chmod 600 ~/.botho/config.toml
```

Botho will warn if the file permissions are too permissive.
