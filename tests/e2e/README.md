# End-to-End Consensus Integration Tests

This directory contains comprehensive end-to-end tests for the Botho consensus system. These tests verify multi-node consensus, fault tolerance, and performance characteristics.

## Test Categories

### Core Consensus Tests (in `botho/tests/`)

| Test File | Description | Coverage |
|-----------|-------------|----------|
| `e2e_consensus_integration.rs` | 5-node SCP consensus with mining & transactions | Multi-node consensus, transaction propagation |
| `byzantine_integration.rs` | Byzantine fault tolerance | Silent nodes, message drops, partitions, recovery |
| `network_integration.rs` | P2P networking | Peer discovery, message routing |
| `ledger_consistency_integration.rs` | Ledger state consistency | UTXO tracking, chain state |

### Extended Tests (in `tests/e2e/`)

| Test File | Description | Trigger |
|-----------|-------------|---------|
| `chaos_tests.rs` | Adverse network conditions | Nightly CI |
| `load_tests.rs` | Performance and stress testing | Manual/Nightly |
| `timing_tests.rs` | Transaction propagation timing | CI |

## Running Tests

### Quick Test (CI)

```bash
# Run standard integration tests
cargo test --package botho --test '*_integration' -- --test-threads=1

# Run timing tests
cargo test --package botho --test timing_tests -- --test-threads=1
```

### Full Test Suite (Nightly)

```bash
# Run all E2E tests including chaos and load
cargo test --package botho --test '*' -- --test-threads=1 --include-ignored
```

### Individual Test Categories

```bash
# Chaos tests only (network adversity)
cargo test --package botho --test chaos_tests -- --test-threads=1 --ignored

# Load tests only (performance)
cargo test --package botho --test load_tests -- --test-threads=1 --ignored

# Byzantine tests only
cargo test --package botho --test byzantine_integration -- --test-threads=1
```

## Test Configuration

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `E2E_NUM_NODES` | 5 | Number of nodes in test network |
| `E2E_TIMEOUT_SECS` | 30 | Consensus timeout in seconds |
| `E2E_LOAD_TX_COUNT` | 1000 | Transactions for load tests |
| `E2E_LOAD_DURATION_SECS` | 60 | Duration for sustained load tests |

### Test Timeouts

- **Standard tests**: 30 seconds per consensus round
- **Chaos tests**: 60 seconds (accounts for message delays/drops)
- **Load tests**: 5 minutes (sustained throughput testing)

## Test Scenarios

### Chaos Tests

1. **50% Packet Loss**: Tests consensus resilience under severe message loss
2. **Clock Skew**: Tests consensus with nodes having skewed local clocks
3. **Combined Adversity**: Multiple chaos factors simultaneously

### Load Tests

1. **Sustained Throughput**: 50 tx/s for 1 hour, monitoring for memory leaks
2. **Mempool Stress**: 10,000 pending transactions
3. **Burst Traffic**: 1000 transactions in 10 seconds

### Timing Tests

1. **Transaction Propagation**: Verify tx reaches all nodes within 5 seconds
2. **Consensus Latency**: Measure time from proposal to externalization
3. **Block Propagation**: Verify blocks propagate within timeout

## CI Integration

Tests are integrated into GitHub Actions:

- **On PR**: Run standard integration tests
- **Nightly**: Run full test suite including chaos and load tests
- **Manual**: Can trigger full suite via workflow dispatch

See `.github/workflows/e2e-tests.yml` for configuration.

## Test Infrastructure

### TestNetwork

The `TestNetwork` struct in `e2e_consensus_integration.rs` provides:

- Multi-node setup with configurable topology
- Message passing via crossbeam channels
- LMDB-backed ledgers per node
- Wallet integration for transaction creation

### ByzantineTestNetwork

The `ByzantineTestNetwork` in `byzantine_integration.rs` extends with:

- Configurable Byzantine behaviors per node
- Message interception and modification
- Partition simulation
- Recovery testing

### Timing Extensions

New timing utilities:

- `wait_for_tx_in_all_mempools()` - Verify transaction propagation
- `measure_consensus_latency()` - Time from proposal to externalization
- `assert_propagation_within()` - Timing assertions

## Adding New Tests

1. Follow existing patterns in `byzantine_integration.rs`
2. Use `#[ignore]` for long-running tests (load, chaos)
3. Add to appropriate test file based on category
4. Update this README with new test descriptions
5. Ensure tests are deterministic when possible

## Troubleshooting

### Tests Timeout

- Increase `E2E_TIMEOUT_SECS` environment variable
- Check for deadlocks in channel communication
- Verify quorum configuration (k=3 for 5 nodes)

### Flaky Tests

- Use `--test-threads=1` to prevent resource contention
- Check for timing-dependent assertions
- Consider increasing tolerance for timing tests

### Memory Issues

- Load tests may require increased stack size
- Use `RUST_MIN_STACK=8388608` for large tests
