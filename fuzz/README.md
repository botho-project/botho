# Botho Fuzz Testing

This directory contains fuzz testing targets for security-critical crypto primitives and deserialization code.

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

### Crypto Primitives (High Priority)

| Target | Description | Security Risk |
|--------|-------------|---------------|
| `fuzz_clsag` | CLSAG ring signature sign/verify | CRITICAL - Core privacy |
| `fuzz_ring_signature` | MLSAG ring signature verification | CRITICAL - Core privacy |
| `fuzz_subaddress` | Account key/subaddress derivation | HIGH - Key security |
| `fuzz_pq_keys` | ML-KEM/ML-DSA key parsing | MEDIUM - Address parsing |
| `fuzz_mlkem_decapsulation` | ML-KEM decapsulation | HIGH - PQ key exchange |

### Transaction & Network

| Target | Description | Security Risk |
|--------|-------------|---------------|
| `fuzz_transaction` | Transaction and UTXO deserialization | HIGH - Network input |
| `fuzz_pq_transaction` | Quantum-private transaction deserialization | HIGH - Network input |
| `fuzz_block` | Block and header deserialization | HIGH - Sync protocol |
| `fuzz_network_messages` | Sync protocol messages | HIGH - Network input |

### Parsing & Validation

| Target | Description | Security Risk |
|--------|-------------|---------------|
| `fuzz_address_parsing` | Address string parsing | MEDIUM - User input |
| `fuzz_rpc_request` | RPC request parsing | MEDIUM - API input |

## Running Fuzz Tests

### Run a specific target

```bash
cd fuzz
cargo +nightly fuzz run fuzz_clsag
```

### Run with timeout (recommended for CI)

```bash
cargo +nightly fuzz run fuzz_clsag -- -max_total_time=60
```

### Run all crypto targets (10 minutes each)

```bash
cd fuzz
for target in fuzz_clsag fuzz_ring_signature fuzz_subaddress; do
    echo "Fuzzing $target..."
    cargo +nightly fuzz run $target -- -max_total_time=600
done
```

### Run quick smoke test (1 minute each)

```bash
cd fuzz
for target in fuzz_clsag fuzz_subaddress; do
    echo "Smoke testing $target..."
    cargo +nightly fuzz run $target -- -max_total_time=60
done
```

## Reproducing Crashes

When a crash is found, a minimized test case is saved to `fuzz/artifacts/<target>/`.

To reproduce:

```bash
cargo +nightly fuzz run fuzz_clsag fuzz/artifacts/fuzz_clsag/crash-xxxxx
```

## Corpus Management

The fuzzer builds a corpus of interesting inputs in `fuzz/corpus/<target>/`.

Initial corpus seeds are provided for each target.

To minimize the corpus:

```bash
cargo +nightly fuzz cmin fuzz_clsag
```

## Coverage

Generate coverage report:

```bash
cargo +nightly fuzz coverage fuzz_clsag
```

## CI Integration

For CI, run fuzz tests with a time limit to catch regressions:

```yaml
- name: Fuzz Testing
  run: |
    cd fuzz
    cargo +nightly fuzz run fuzz_clsag -- -max_total_time=300
    cargo +nightly fuzz run fuzz_subaddress -- -max_total_time=300
```

## Target Details

### fuzz_clsag

Tests CLSAG (Concise Linkable Spontaneous Anonymous Group) signatures:
- Valid sign/verify cycles with fuzzed parameters
- Malformed signature handling
- Key image consistency across signatures
- Modified ring handling

### fuzz_subaddress

Tests account key subaddress derivation:
- Derivation consistency (same key = same subaddress)
- View account key consistency with full account key
- Special indices (default, change, gift code)
- Private/public key correspondence

## Baseline Fuzzing Run

Before release, complete a baseline fuzzing run:

```bash
# 24-hour baseline run for each crypto target
cd fuzz
for target in fuzz_clsag fuzz_subaddress; do
    echo "Starting 24-hour fuzz of $target..."
    cargo +nightly fuzz run $target -- -max_total_time=86400
done
```

This ensures no crashes exist in the core crypto implementation.
