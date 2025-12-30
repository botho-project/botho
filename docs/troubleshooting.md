# Troubleshooting Guide

Common issues and their solutions when running a Botho node.

## Network Issues

### "No bootstrap peers configured"

**Symptom:** Warning message at startup:
```
No bootstrap peers configured. Add bootstrap_peers to config.toml
```

**Cause:** The node doesn't know how to find other peers on the network.

**Solution:** Add bootstrap peers to your config file:

```toml
# ~/.botho/config.toml
[network]
bootstrap_peers = [
    "/ip4/98.95.2.200/tcp/7100/p2p/12D3KooWBrjTYjNrEwi9MM3AKFenmymyWVXtXbQiSx7eDnDwv9qQ",
]
```

The official seed node is `seed.botho.io` (IP: 98.95.2.200, port 7100).

---

### Node won't connect to peers

**Symptoms:**
- Peer count stays at 0
- "Failed to dial" errors in logs

**Possible causes:**

1. **Firewall blocking connections**
   - Ensure port 7100 (gossip) is open for outbound connections
   - For inbound connections, forward port 7100 to your node

2. **Incorrect bootstrap peer format**
   - Use multiaddr format: `/ip4/<ip>/tcp/<port>/p2p/<peer_id>`
   - The peer ID is required for libp2p connections

3. **Network unreachable**
   - Check internet connectivity
   - Verify the bootstrap peer is online

**Diagnostic:**
```bash
# Check if you can reach the seed node
nc -zv 98.95.2.200 7100
```

---

### "Rate limit exceeded"

**Symptom:** Log message:
```
Rate limit exceeded
```

**Cause:** A peer is sending too many sync requests. This is a DDoS protection mechanism.

**Solution:** This is normal and protects your node. The peer will be temporarily throttled. No action needed.

---

## Sync Issues

### Node stuck syncing

**Symptoms:**
- Chain height not increasing
- "Sync failed from peer" messages

**Possible causes:**

1. **Poor peer connections**
   - The peers you're connected to may be unreliable
   - Solution: Restart the node to find new peers

2. **Corrupted local database**
   - Rare, but possible after crashes
   - Solution: Remove the data directory and resync:
     ```bash
     rm -rf ~/.botho/data
     botho run
     ```

3. **Network partition**
   - Your node may be on a minority fork
   - Solution: Ensure you're connected to the main network via the official seed node

---

### "Failed to add block"

**Symptom:** Warning messages about failed blocks.

**Possible causes:**

1. **Block validation failed**
   - The block didn't pass consensus rules
   - This is normal if receiving invalid blocks from malicious peers

2. **Out of order blocks**
   - Blocks must be added sequentially
   - The sync process handles ordering automatically

**Solution:** Usually no action needed. The node will request valid blocks.

---

## Minting Issues

### "Quorum lost! - stopping minting"

**Symptom:** Minting stops with quorum warning.

**Cause:** Not enough peers are available to form a consensus quorum. Botho uses the Stellar Consensus Protocol (SCP) which requires agreement from a threshold of nodes.

**Solution:**
1. Wait for more peers to connect
2. Check your quorum configuration:
   ```toml
   [network.quorum]
   mode = "recommended"
   min_peers = 1  # Increase if you want stricter requirements
   ```

3. For testing alone, you can set `min_peers = 0` (not recommended for production)

---

### Minting not starting

**Symptoms:**
- `--mint` flag used but no minting activity
- "Minting requested but..." message

**Possible causes:**

1. **No quorum established**
   - Need minimum peers for consensus
   - Check `min_peers` setting

2. **Still syncing**
   - Node must be synced before minting
   - Wait for sync to complete

3. **Minting disabled in config**
   ```toml
   [minting]
   enabled = true  # Must be true
   ```

**Diagnostic:**
```bash
curl -X POST http://localhost:7101/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"minting_getStatus","params":{},"id":1}'
```

---

### Low hash rate

**Symptom:** Hash rate lower than expected.

**Solutions:**

1. **Increase minting threads:**
   ```toml
   [minting]
   threads = 8  # Set to number of CPU cores
   ```

2. **Check CPU throttling** - Ensure your system isn't thermal throttling

3. **Close competing applications** - Other CPU-intensive tasks reduce mining performance

---

## Wallet Issues

### "File permissions too permissive"

**Symptom:** Warning about config file permissions.

**Cause:** The config file contains your mnemonic and should be protected.

**Solution:**
```bash
chmod 600 ~/.botho/config.toml
```

---

### Balance shows 0 after receiving funds

**Possible causes:**

1. **Node not synced**
   - Check sync status: `botho status`
   - Wait for full sync

2. **Wrong wallet**
   - Verify you're using the correct mnemonic
   - Check your address: `botho address`

3. **Transaction not confirmed**
   - Check the mempool for pending transactions
   - Wait for block confirmation

---

### Cannot send transaction

**Symptoms:**
- "Insufficient balance"
- "Invalid transaction"

**Solutions:**

1. **Check balance includes fees**
   - Transactions require fees
   - Ensure balance > amount + fee

2. **Check minimum amounts**
   - Very small amounts may be below dust threshold

3. **Wait for UTXOs to confirm**
   - Recently received funds need confirmation before spending

---

## RPC Issues

### Cannot connect to RPC

**Symptom:** Connection refused on port 7101.

**Solutions:**

1. **Verify node is running**
   ```bash
   ps aux | grep botho
   ```

2. **Check RPC port**
   - Default is 7101
   - Verify in logs: "RPC server listening on..."

3. **Check firewall**
   - Ensure localhost connections are allowed
   - For remote access, open port 7101

---

### CORS errors in browser

**Symptom:** Browser console shows CORS errors.

**Cause:** Origin not in allowed list.

**Solution:** Add your origin to config:
```toml
[rpc]
cors_origins = ["http://localhost:3000", "https://yourdomain.com"]
```

---

### WebSocket disconnects

**Symptom:** WebSocket connection drops frequently.

**Possible causes:**

1. **Client not sending pings**
   - Send periodic ping messages to keep connection alive
   ```json
   {"type": "ping"}
   ```

2. **Network instability**
   - Implement reconnection logic in your client

3. **Event backlog**
   - If you're not processing events fast enough, you may lag behind
   - Subscribe only to events you need

---

## Database Issues

### "Failed to open database"

**Symptom:** Node fails to start with database errors.

**Solutions:**

1. **Check disk space**
   ```bash
   df -h ~/.botho
   ```

2. **Check file permissions**
   ```bash
   ls -la ~/.botho/data
   ```

3. **Recovery option** - Remove and resync:
   ```bash
   rm -rf ~/.botho/data
   botho run
   ```

---

### Database corruption

**Symptoms:**
- Unexpected crashes
- Invalid block errors
- Hash mismatches

**Solution:** The database uses LMDB which is crash-safe, but corruption can occur with disk errors.

```bash
# Backup current state (optional)
cp -r ~/.botho/data ~/.botho/data.bak

# Remove and resync
rm -rf ~/.botho/data
botho run
```

---

## Performance Issues

### High memory usage

**Possible causes:**

1. **Large mempool**
   - Many pending transactions consume memory
   - Mempool is automatically pruned

2. **Many peer connections**
   - Each peer uses some memory
   - This is normal for healthy nodes

---

### High CPU usage (not minting)

**Possible causes:**

1. **Initial sync**
   - Block validation is CPU-intensive
   - Normal during sync

2. **Block propagation**
   - Processing many incoming blocks
   - Should stabilize after sync

---

## Logs and Debugging

### Enable verbose logging

Set the `RUST_LOG` environment variable:

```bash
# Info level (default)
RUST_LOG=info botho run

# Debug level (more verbose)
RUST_LOG=debug botho run

# Trace specific modules
RUST_LOG=botho::consensus=debug,botho::network=trace botho run
```

### Log locations

Logs are written to stderr by default. Redirect to a file:

```bash
botho run 2>&1 | tee ~/.botho/botho.log
```

---

## Getting Help

If you can't resolve an issue:

1. **Check the documentation** - [docs/](.)
2. **Search existing issues** - [GitHub Issues](https://github.com/botho-project/botho/issues)
3. **Open a new issue** with:
   - Botho version (`botho --version`)
   - Operating system
   - Steps to reproduce
   - Relevant log output
