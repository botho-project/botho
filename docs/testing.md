# Testing Guide

How to run and write tests for Botho.

## Running Tests

### All Tests

```bash
# Run all tests in the workspace
cargo test

# Run with output visible
cargo test -- --nocapture

# Run in release mode (faster, but less debug info)
cargo test --release
```

### Specific Crate Tests

```bash
# Test the main botho binary
cargo test -p botho

# Test consensus implementation
cargo test -p bth-consensus-scp

# Test cryptographic primitives
cargo test -p bth-crypto-keys
cargo test -p bth-crypto-ring-signature

# Test transaction core
cargo test -p bth-transaction-core

# Test cluster-tax (progressive fees)
cargo test -p bth-cluster-tax
```

### Specific Test

```bash
# Run tests matching a pattern
cargo test test_genesis

# Run a specific test
cargo test -p botho mempool::tests::test_add_transaction

# Run tests in a specific file
cargo test -p botho --test network_integration
```

### Integration Tests

```bash
# Run integration tests only
cargo test -p botho --test '*'

# Specific integration test suites
cargo test -p botho --test network_integration
cargo test -p botho --test e2e_consensus_integration
cargo test -p botho --test pq_integration
```

### Botho integration coverage in CI

The workspace job compiles every integration target with `--no-run`; that is
**not execution coverage**. It also executes `compact_block_integration` on
matching pull requests. Run that same bounded suite locally with:

```bash
cargo test --locked -p botho --test compact_block_integration
```

The suite checks compact-block creation, reconstruction, missing-transaction
exchange, bandwidth reduction, and serialized-size accuracy, including hybrid
coinbases and lottery outputs. It needs no external services or ledger database.
Its 17 tests passed locally in 6.07 seconds after compilation on macOS using
the pinned toolchain; none are ignored. This gate prevents a recurrence of #1187: the
existing size assertion was correct, but CI only compiled it.

The inventory below covers all 24 top-level `botho/tests/*.rs` targets as of
September 2026. `common/` contains shared helpers, not another integration target.
The seven targets named in `e2e-tests.yml` are **manual-dispatch only**, not PR
execution gates. The 15 workspace-gated targets below passed 245 local tests;
their measured runtimes are recorded here and in the linked suite reports.
Do not infer that the remaining unmeasured suites are fast or reliable from
their names.

| Target | Execution owner | Runtime / environment classification |
| --- | --- | --- |
| `compact_block_integration` | Workspace PR job | Local block/protocol operations; measured in seconds after build |
| `circuit_handshake_integration` | Workspace PR job | In-process cryptographic circuit handshakes |
| `ice_stun_integration` | Workspace PR job | ICE/STUN configuration, candidate and encoding checks; no public STUN requests |
| `onion_broadcast_integration` | Workspace PR job | Simulated local circuit pools and broadcast pipeline |
| `privacy_integration` | Workspace PR job | Randomized simulated relays, adversaries, load and privacy assertions |
| `relay_handler_integration` | Workspace PR job | In-process relay pipeline; one 1.1-second key-expiry sleep |
| `signaling_integration` | Workspace PR job | Local signaling state/encoding; 50-ms expiry sleep |
| `traffic_indistinguishability` | Workspace PR job | Seeded statistical sampling |
| `transport_negotiation_integration` | Workspace PR job | In-memory duplex streams with negotiation timeouts |
| `chain_sync_catchup_integration` | Workspace PR job | Production sync state machine, simulated messages, temporary LMDB ledgers |
| `issue_998_fresh_genesis_liveness` | Workspace PR job | Repeated hybrid-coinbase block application in a temporary ledger |
| `ledger_consistency_integration` | Workspace PR job | Temporary LMDB persistence/concurrency and invalid-block checks |
| `tx_lifecycle_integration` | Workspace PR job | Ledger/transaction validation plus ephemeral loopback RPC |
| `rpc_integration` | Workspace PR job | Real HTTP/WebSocket requests to ephemeral loopback servers |
| `e2e_faucet_workflow` | Workspace PR job | Local faucet/RPC servers, funded temporary ledgers and rate-limit sleeps |
| `consensus_cluster_convergence` | Pending [#1274](https://github.com/botho-project/botho/issues/1274) | Multi-node in-process consensus; 30-second per-block no-stall deadlines |
| `e2e_transfer_patterns` | Pending #1274 | Multi-node transfer, mining and stress/load scenarios |
| `byzantine_integration` | Manual E2E `byzantine` / `all` | Adversarial multi-node consensus and wall-clock deadlines |
| `e2e_consensus_integration` | Manual E2E `all` | Five-node consensus, mining and transfers |
| `e2e_progressive_fees` | Manual E2E `all` | Fee-curve assertions plus in-process consensus and setup sleeps |
| `network_integration` | Manual E2E `all` | Real loopback libp2p sockets and networking deadlines |
| `timing_tests` | Manual E2E `timing` / `all` | Wall-clock propagation/consensus timing assertions |
| `chaos_tests` | Manual E2E `chaos` / `all`, ignored opt-in | Four ignored long cases; ordinary non-ignored smoke case is omitted by that job |
| `load_tests` | Manual E2E `load` / `all`, ignored opt-in | Four ignored long cases; ordinary non-ignored smoke case is omitted by that job |

[#1274](https://github.com/botho-project/botho/issues/1274) tracks the remaining
consensus/E2E execution policy, including the ordinary chaos/load smoke cases.
It requires measured runtime and investigation of real failures before choosing
a PR or scheduled job. Do not enable every ignored test
or relax assertions to obtain a green gate. No external-service suite is inferred
from its name: ICE/STUN here is local, and RPC uses loopback; actual public-network
coverage would require its own explicit environment and job.

The workspace PR job also runs `cargo test --locked -p botho --lib
network::transport::webrtc::`. Its real peer tests bind loopback UDP sockets and
exercise SDP exchange, bidirectional DTLS/SCTP delivery, partial reads, candidate
fanout, bounded receive overflow, close/drop cleanup and fresh-peer reconnect.
They use no public STUN/TURN service. WebRTC remains inactive in the transport
manager; browser interoperability and real NAT/TURN traversal are not established
by these tests. The wrapper uses concrete interface bindings (excluding scoped
IPv6 link-local addresses), a 1 MiB unread-byte limit, and a one-second bounded
channel-close grace period before stopping the driver.

The workspace PR job also executes eight protocol/privacy integration suites.
See [protocol integration coverage](protocol-integration-tests.md) for the exact
command, measured runtimes, environmental requirements, and validation limits.

The workspace PR job also executes six stateful ledger/RPC integration targets.
See [ledger and RPC execution](ledger-rpc-integration-tests.md) for the command,
measured runtimes and environment requirements.

---

## Test Organization

### Unit Tests

Unit tests live alongside the code they test:

```
botho/src/
├── mempool.rs          # Contains #[cfg(test)] mod tests
├── wallet.rs           # Contains unit tests
├── block.rs            # Contains unit tests
└── ...
```

Example structure:
```rust
// In mempool.rs
pub struct Mempool { ... }

impl Mempool {
    pub fn add_tx(&mut self, tx: Transaction) -> Result<()> { ... }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_transaction() {
        let mut mempool = Mempool::new();
        // ...
    }
}
```

### Integration Tests

Integration tests live in `tests/` directories:

```
botho/tests/
├── network_integration.rs      # Network layer tests
├── e2e_consensus_integration.rs # Full consensus tests
└── pq_integration.rs           # Post-quantum tests

consensus/scp/tests/
├── test_mesh_networks.rs       # SCP mesh topology
├── test_cyclic_networks.rs     # SCP cyclic topology
└── mock_network/               # Test infrastructure
```

---

## Writing Tests

### Basic Unit Test

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_functionality() {
        // Arrange
        let input = create_test_input();

        // Act
        let result = function_under_test(input);

        // Assert
        assert_eq!(result, expected_value);
    }

    #[test]
    #[should_panic(expected = "error message")]
    fn test_expected_panic() {
        function_that_should_panic();
    }

    #[test]
    fn test_result_handling() -> Result<(), Box<dyn std::error::Error>> {
        let result = fallible_function()?;
        assert!(result.is_valid());
        Ok(())
    }
}
```

### Async Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_async_operation() {
        let result = async_function().await;
        assert!(result.is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_concurrent_operations() {
        let (tx, rx) = tokio::sync::mpsc::channel(10);
        // ...
    }
}
```

### Test Helpers

Create reusable test utilities:

```rust
// In tests/common/mod.rs or src/test_helpers.rs
#[cfg(test)]
pub mod test_helpers {
    use super::*;

    pub fn create_test_transaction() -> Transaction {
        Transaction {
            inputs: vec![create_test_input()],
            outputs: vec![create_test_output()],
            fee: 1000,
            ..Default::default()
        }
    }

    pub fn create_test_block(height: u64) -> Block {
        Block::new(height, [0u8; 32], vec![], create_test_minting_tx())
    }
}
```

### Testing Cryptography

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use bth_util_from_random::OsRng;

    #[test]
    fn test_key_generation() {
        let keypair = KeyPair::generate(&mut OsRng);
        assert!(keypair.public_key().is_valid());
    }

    #[test]
    fn test_signature_roundtrip() {
        let keypair = KeyPair::generate(&mut OsRng);
        let message = b"test message";

        let signature = keypair.sign(message);
        assert!(keypair.public_key().verify(message, &signature));
    }

    #[test]
    fn test_signature_fails_wrong_message() {
        let keypair = KeyPair::generate(&mut OsRng);
        let signature = keypair.sign(b"original");

        assert!(!keypair.public_key().verify(b"modified", &signature));
    }
}
```

### Testing Network Code

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_peer_discovery() {
        // Create mock network
        let (tx1, rx1) = mpsc::channel(10);
        let (tx2, rx2) = mpsc::channel(10);

        let node1 = TestNode::new(tx1, rx2);
        let node2 = TestNode::new(tx2, rx1);

        // Simulate peer discovery
        node1.announce_presence().await;

        // Verify node2 received announcement
        let msg = node2.receive().await.unwrap();
        assert!(matches!(msg, NetworkMessage::PeerAnnounce { .. }));
    }
}
```

---

## Test Coverage

### Generate Coverage Report

Using `cargo-tarpaulin`:

```bash
# Install tarpaulin
cargo install cargo-tarpaulin

# Generate coverage report
cargo tarpaulin --out Html

# Open report
open tarpaulin-report.html
```

Using `cargo-llvm-cov`:

```bash
# Install llvm-cov
cargo install cargo-llvm-cov

# Generate coverage
cargo llvm-cov --html

# Open report
open target/llvm-cov/html/index.html
```

### Coverage Guidelines

- Aim for >80% coverage on core logic
- 100% coverage on cryptographic operations
- Test edge cases and error paths
- Don't chase coverage numbers at the expense of meaningful tests

---

## Property-Based Testing

Using `proptest` for property-based tests:

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn test_serialization_roundtrip(value: u64) {
        let bytes = value.to_le_bytes();
        let recovered = u64::from_le_bytes(bytes);
        prop_assert_eq!(value, recovered);
    }

    #[test]
    fn test_amount_never_negative(
        a in 0u64..1_000_000_000,
        b in 0u64..1_000_000_000
    ) {
        let result = a.saturating_sub(b);
        prop_assert!(result <= a);
    }
}
```

---

## Mocking

### Mock Traits

```rust
// Define a trait for the dependency
pub trait BlockStore {
    fn get_block(&self, height: u64) -> Option<Block>;
    fn add_block(&mut self, block: Block) -> Result<()>;
}

// Real implementation
pub struct LmdbBlockStore { ... }

// Mock for testing
#[cfg(test)]
pub struct MockBlockStore {
    blocks: HashMap<u64, Block>,
}

#[cfg(test)]
impl BlockStore for MockBlockStore {
    fn get_block(&self, height: u64) -> Option<Block> {
        self.blocks.get(&height).cloned()
    }

    fn add_block(&mut self, block: Block) -> Result<()> {
        self.blocks.insert(block.height(), block);
        Ok(())
    }
}
```

### Using mockall

```rust
use mockall::{automock, predicate::*};

#[automock]
pub trait NetworkClient {
    fn send(&self, peer: &str, msg: &[u8]) -> Result<()>;
    fn receive(&self) -> Option<Vec<u8>>;
}

#[test]
fn test_with_mock() {
    let mut mock = MockNetworkClient::new();
    mock.expect_send()
        .with(eq("peer1"), always())
        .returning(|_, _| Ok(()));

    // Use mock in test
    let service = Service::new(mock);
    service.broadcast(b"hello").unwrap();
}
```

---

## Benchmarks

### Writing Benchmarks

```rust
// In benches/crypto_bench.rs
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_signature(c: &mut Criterion) {
    let keypair = KeyPair::generate(&mut OsRng);
    let message = [0u8; 32];

    c.bench_function("sign", |b| {
        b.iter(|| keypair.sign(black_box(&message)))
    });

    let signature = keypair.sign(&message);
    c.bench_function("verify", |b| {
        b.iter(|| keypair.public_key().verify(black_box(&message), black_box(&signature)))
    });
}

criterion_group!(benches, bench_signature);
criterion_main!(benches);
```

### Running Benchmarks

```bash
# Run all benchmarks
cargo bench

# Run specific benchmark
cargo bench --bench crypto_bench

# Save baseline for comparison
cargo bench -- --save-baseline main

# Compare against baseline
cargo bench -- --baseline main
```

---

## Continuous Integration

### GitHub Actions Example

```yaml
# .github/workflows/test.yml
name: Tests

on: [push, pull_request]

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-action@stable

      - name: Cache cargo
        uses: actions/cache@v3
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            target
          key: ${{ runner.os }}-cargo-${{ hashFiles('**/Cargo.lock') }}

      - name: Run tests
        run: cargo test --all

      - name: Run clippy
        run: cargo clippy --all -- -D warnings
```

---

## Testing Checklist

Before submitting a PR:

- [ ] All existing tests pass (`cargo test`)
- [ ] New functionality has tests
- [ ] Edge cases are covered
- [ ] Error conditions are tested
- [ ] No clippy warnings (`cargo clippy`)
- [ ] Code is formatted (`cargo fmt`)

### What to Test

| Component | Test Focus |
|-----------|------------|
| Cryptography | Correctness, edge cases, invalid inputs |
| Consensus | Byzantine behavior, network partitions |
| Transactions | Validation, serialization, signatures |
| Mempool | Ordering, limits, duplicate handling |
| Network | Message handling, timeouts, reconnection |
| RPC | Request/response format, error codes |

---

## Debugging Tests

### Print Debug Output

```rust
#[test]
fn test_with_debug() {
    let result = complex_operation();
    eprintln!("Debug: result = {:?}", result);  // Visible with --nocapture
    assert!(result.is_ok());
}
```

### Run with Logging

```bash
RUST_LOG=debug cargo test -- --nocapture
```

### Use test-specific logging

```rust
#[test]
fn test_with_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .try_init();

    tracing::info!("Starting test");
    // ...
}
```

---

## Common Issues

### Tests Hang

- Check for deadlocks in async code
- Ensure channels are properly closed
- Use timeouts: `tokio::time::timeout(Duration::from_secs(5), async_fn()).await`

### Flaky Tests

- Avoid timing-dependent assertions
- Use proper synchronization (channels, barriers)
- Mock external dependencies

### Slow Tests

- Use `#[ignore]` for slow tests, run with `cargo test -- --ignored`
- Parallelize with `cargo test -- --test-threads=4`
- Consider test fixtures to avoid repeated setup

---

## Next Steps

- [Developer Guide](developer-guide.md) — Build applications with Botho
- [Architecture](concepts/architecture.md) — Understand the codebase structure
- [Contributing](../CONTRIBUTING.md) — Submit your changes
