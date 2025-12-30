# Botho Fuzz Testing

This directory contains fuzz testing targets for security-critical deserialization code.

## Prerequisites

Install cargo-fuzz:

```bash
cargo install cargo-fuzz
```

Note: Requires nightly Rust for libfuzzer:

```bash
rustup install nightly
```

## Available Fuzz Targets

| Target | Description | Security Risk |
|--------|-------------|---------------|
| `fuzz_transaction` | Transaction and UTXO deserialization | HIGH - Network input |
| `fuzz_pq_transaction` | Quantum-private transaction deserialization | HIGH - Network input |
| `fuzz_block` | Block and header deserialization | HIGH - Sync protocol |
| `fuzz_pq_keys` | ML-KEM/ML-DSA key/signature parsing | MEDIUM - Address/TX parsing |
| `fuzz_network_messages` | Sync protocol messages | HIGH - Network input |

## Running Fuzz Tests

Run a specific target:

```bash
cd fuzz
cargo +nightly fuzz run fuzz_transaction
```

Run with timeout (recommended for CI):

```bash
cargo +nightly fuzz run fuzz_transaction -- -max_total_time=60
```

Run all targets for 1 minute each:

```bash
for target in fuzz_transaction fuzz_pq_transaction fuzz_block fuzz_pq_keys fuzz_network_messages; do
    echo "Fuzzing $target..."
    cargo +nightly fuzz run $target -- -max_total_time=60
done
```

## Reproducing Crashes

When a crash is found, a minimized test case is saved to `fuzz/artifacts/<target>/`.

To reproduce:

```bash
cargo +nightly fuzz run fuzz_transaction fuzz/artifacts/fuzz_transaction/crash-xxxxx
```

## Corpus Management

The fuzzer builds a corpus of interesting inputs in `fuzz/corpus/<target>/`.

To minimize the corpus:

```bash
cargo +nightly fuzz cmin fuzz_transaction
```

## Coverage

Generate coverage report:

```bash
cargo +nightly fuzz coverage fuzz_transaction
```

## CI Integration

For CI, run fuzz tests with a time limit to catch regressions:

```yaml
- name: Fuzz Testing
  run: |
    cd fuzz
    cargo +nightly fuzz run fuzz_transaction -- -max_total_time=300
    cargo +nightly fuzz run fuzz_pq_transaction -- -max_total_time=300
```
