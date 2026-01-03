# Memory Budget

This document describes memory requirements and optimization strategies for Botho full nodes.

## Target Requirements

| Metric | Target | Notes |
|--------|--------|-------|
| Peak RSS | < 4 GB | Full node with 10K tx mempool |
| Mempool Capacity | 10,000 tx | Configurable via `MAX_MEMPOOL_SIZE` |
| Sustained Load | 100+ tx/s | Without memory leaks |

## Component Memory Breakdown

### Mempool (`botho/src/mempool.rs`)

The mempool stores pending transactions awaiting inclusion in blocks.

| Component | Formula | Estimate (10K tx) |
|-----------|---------|-------------------|
| Transaction HashMap | 10K × (32 + 8 + ptr) | ~0.5 MB |
| Private Transactions (CLSAG) | 10K × ~4 KB each | ~40 MB |
| Minting Transactions | 10K × ~1.5 KB each | ~15 MB |
| Key Images HashSet | 10K × 32 bytes | ~0.3 MB |
| **Total (typical mix)** | | **~41 MB** |

**Notes:**
- Private transactions use CLSAG ring signatures (~700 bytes per input)
- Minting transactions are smaller (no ring signatures)
- Memory is released when transactions are confirmed or evicted

### Ledger (LMDB) (`botho/src/ledger/store.rs`)

The ledger uses LMDB for persistent storage with memory-mapped I/O.

| Component | Size | Notes |
|-----------|------|-------|
| LMDB Map Size | 1 GB (configured) | Virtual, not RSS |
| Active Page Cache | Variable | Depends on access patterns |
| UTXO Database | Grows with chain | Bounded by map size |
| Key Images Database | Grows with chain | One entry per spent output |
| Cluster Wealth Index | ~8 bytes × clusters | Progressive fee tracking |

**Notes:**
- LMDB uses virtual memory mapping; RSS reflects actual page usage
- Consider reducing `map_size` on memory-constrained systems
- Hot pages stay in memory; cold pages are paged out by OS

### P2P Network (`botho/src/network/`)

| Component | Per-Connection | At 100 Peers |
|-----------|----------------|--------------|
| libp2p Buffers | ~64 KB | ~6.4 MB |
| Gossipsub State | ~16 KB | ~1.6 MB |
| Connection Metadata | ~1 KB | ~0.1 MB |
| **Total** | | **~8 MB** |

### Cryptographic Operations

Ring signature operations have significant stack requirements:

| Operation | Memory | Duration |
|-----------|--------|----------|
| CLSAG Verification | ~10 KB stack | ~2 ms |
| ML-DSA Verification | ~50 KB stack | ~5 ms |
| Batch Verification (10 tx) | ~200 KB stack | ~50 ms |

**Notes:**
- Stack allocations are temporary during verification
- Consider thread pool size when planning memory

## Memory Optimization Strategies

### 1. Transaction Size Estimation

**Before (allocates):**
```rust
let tx_size = bincode::serialize(&tx).map(|b| b.len()).unwrap_or(1);
```

**After (no allocation):**
```rust
let tx_size = tx.estimate_size().max(1);
```

Savings: Avoids allocating transaction bytes for size calculation.

### 2. Mempool Eviction

Transactions are evicted when:
- Mempool reaches `MAX_MEMPOOL_SIZE` (lowest fee evicted)
- Transaction age exceeds 1 hour (`MAX_TX_AGE_SECS`)
- Key image is spent on-chain

### 3. LMDB Page Management

For memory-constrained environments:
```rust
// Reduce map size (requires restart)
EnvOpenOptions::new()
    .map_size(512 * 1024 * 1024)  // 512 MB instead of 1 GB
    .open(path)
```

### 4. Connection Limits

Limit concurrent peer connections:
```toml
# node.toml
[network]
max_peers = 50  # Reduce from default 100
```

## Profiling Instructions

### heaptrack (Linux)

```bash
# Install
sudo apt install heaptrack heaptrack-gui

# Profile
heaptrack cargo run --release -p botho -- --config node.toml

# Analyze
heaptrack_gui heaptrack.botho.*.gz
```

### valgrind massif (Linux/macOS)

```bash
valgrind --tool=massif --massif-out-file=massif.out \
    ./target/release/botho --config node.toml

ms_print massif.out > memory_report.txt
```

### Instruments (macOS)

```bash
cargo instruments -t Allocations --release -p botho -- --config node.toml
```

### Memory Metrics via RPC

```bash
# Get mempool statistics
curl -X POST http://localhost:8332/rpc \
    -H "Content-Type: application/json" \
    -d '{"jsonrpc":"2.0","method":"getmempoolinfo","params":[],"id":1}'
```

## Configuration Knobs

| Setting | Location | Default | Memory Impact |
|---------|----------|---------|---------------|
| `MAX_MEMPOOL_SIZE` | `mempool.rs` | 10,000 | ~17 MB per 1K tx |
| `map_size` | `store.rs` | 1 GB | Virtual memory |
| `max_peers` | `node.toml` | 100 | ~80 KB per peer |

## Memory Leak Detection

### Symptoms
- RSS grows monotonically over hours
- OOM killer terminates node
- Swap usage increases continuously

### Debugging Steps

1. **Profile with heaptrack** for allocation tracking
2. **Check for HashSet/HashMap growth** without corresponding removals
3. **Verify transaction eviction** works correctly
4. **Monitor key image cleanup** after block confirmations

### Known Safe Patterns
- LMDB virtual size is constant (map_size)
- Mempool size is bounded by `MAX_MEMPOOL_SIZE`
- Key images are pruned when transactions confirm

## Future Optimizations

Potential improvements for memory-constrained deployments:

1. **Arc<Transaction>** - Share transaction data between mempool and block building
2. **Bloom Filter for Key Images** - Quick rejection before HashSet lookup
3. **UTXO Pruning** - Remove spent outputs after finality depth
4. **Lazy Signature Decompression** - Decompress signatures only during verification
5. **Connection Pooling** - Reuse libp2p buffers across connections

## Related Issues

- #32: Add criterion benchmarks for crypto operations
- #34: Add botho-testnet binary (load testing)

## References

- [heaptrack documentation](https://github.com/KDE/heaptrack)
- [valgrind massif manual](https://valgrind.org/docs/manual/ms-manual.html)
- [LMDB memory usage](http://www.lmdb.tech/doc/starting.html)
