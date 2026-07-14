# Botho Fuzz Testing

This directory contains fuzz testing targets for security-critical crypto primitives and deserialization code.

## Prerequisites

Install cargo-fuzz (pin the version used in CI for reproducibility):

```bash
cargo install cargo-fuzz --version 0.13.1 --locked
```

Note: This crate is **excluded from the root workspace**, so it never builds
during a normal `cargo build`/`cargo test`. That is why it can silently
bit-rot. A PR-triggered build-only job in `.github/workflows/fuzz.yml` now
compiles every target on changes to `fuzz/` or the crates it fuzzes.

### Toolchain

Build and run with the repo's **pinned** nightly (see `rust-toolchain` at the
repo root), not a rolling `+nightly`. The rolling nightly pulls a compiler that
fails to compile a transitive `hashbrown 0.14.5` dependency. Because
`rust-toolchain` pins the channel, invoke cargo-fuzz **without** `+nightly`:

```bash
cd fuzz
cargo fuzz build            # uses the pinned channel via rust-toolchain
```

### macOS execution caveat (#920) — use native-smoke instead

On macOS 26.5.1 (Darwin 25, arm64), every `cargo fuzz run <target>` with the
pinned `nightly-2025-12-03` toolchain **hangs before `main`** and never prints
the libFuzzer banner (observed 30+ min at ~70% CPU on a `-max_total_time=60`
run). `sample` shows a recursive ASan-init deadlock inside `dyld`, not target
code:

```
__asan::AsanInitInternal
  -> __asan::InitializeShadowMemory
    -> __sanitizer::MemoryRangeIsAvailable -> get_dyld_hdr
      -> dyld_shared_cache_iterate_text_swift -> _Block_copy -> malloc
        -> __sanitizer_mz_malloc -> __asan::AsanInitFromRtl
          -> __sanitizer::StaticSpinMutex::LockSlow   <- spins on the lock
                                                          AsanInitInternal holds
```

This reproduces identically across targets (it is environmental — the rustc
ASan runtime vs macOS 26.5 dyld — not specific to any harness), so the targets
**build** on macOS but cannot be **executed** there via libFuzzer. `-s none`
does not help (`if-watch` links as a sancov-instrumented dylib whose
`__sanitizer_cov_trace_*` symbols have no runtime to bind to without a
sanitizer), and a rolling `+nightly` no longer compiles the pinned dep tree
(`hashbrown 0.14` "cannot specialize on trait Copy").

**Coverage-guided runs happen in ubuntu CI (`.github/workflows/fuzz.yml`).**
For local development on macOS, use the **native-smoke** path below, which runs
the *exact same* harness bodies (no libFuzzer, no sanitizer) so it is immune to
the deadlock.

## Native-smoke path (runnable on macOS today)

The bridge fuzz harnesses' bodies live in the `botho-fuzz` **library**
(`fuzz/src/*.rs`). Each `fuzz_targets/*.rs` libFuzzer binary and the
native-smoke driver call the *same* `run_from_bytes` function, so the two paths
can never drift. The native-smoke path replays every committed
`fuzz/corpus/<target>/` seed plus N randomized inputs through those shared
bodies, with `debug_assertions` on, on a stock toolchain — so a panic or a
violated invariant surfaces without ever touching libFuzzer/ASan.

Targets currently covered: `fuzz_bridge_attestation`,
`fuzz_bridge_solana_parse`, `fuzz_federation_status_line`.

### As a `cargo test` (quickest — one test per target)

```bash
cd fuzz
cargo test -p botho-fuzz --features native-smoke
# Override the randomized-input count per target (default 2_000 for the test):
BOTHO_FUZZ_SMOKE_ITERS=100000 cargo test -p botho-fuzz --features native-smoke
```

### As a standalone driver (the ~100k dev budget)

```bash
cd fuzz
# Default: 100_000 randomized inputs per target, plus the whole corpus.
cargo run --features native-smoke --bin native_smoke

# Cheap CI/dev-fast run, deterministic seed, single target:
BOTHO_FUZZ_SMOKE_ITERS=2000 BOTHO_FUZZ_SMOKE_SEED=42 \
  cargo run --features native-smoke --bin native_smoke -- fuzz_federation_status_line
```

The native-smoke path is **not** coverage-guided — it is a smoke test, not a
substitute for the scheduled ubuntu fuzz runs. Its job is to let macOS
developers exercise the harnesses locally and catch obvious regressions before
pushing. Where a fuzz-related issue's test plan says "run each target 60s
locally", macOS developers should instead run the native-smoke suite above.

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
cargo fuzz run fuzz_clsag
```

### Run with timeout (recommended for CI)

```bash
cargo fuzz run fuzz_clsag -- -max_total_time=60
```

### Run all crypto targets (10 minutes each)

```bash
cd fuzz
for target in fuzz_clsag fuzz_ring_signature fuzz_subaddress; do
    echo "Fuzzing $target..."
    cargo fuzz run $target -- -max_total_time=600
done
```

### Run quick smoke test (1 minute each)

```bash
cd fuzz
for target in fuzz_clsag fuzz_subaddress; do
    echo "Smoke testing $target..."
    cargo fuzz run $target -- -max_total_time=60
done
```

## Reproducing Crashes

When a crash is found, a minimized test case is saved to `fuzz/artifacts/<target>/`.

To reproduce:

```bash
cargo fuzz run fuzz_clsag fuzz/artifacts/fuzz_clsag/crash-xxxxx
```

## Corpus Management

The fuzzer builds a corpus of interesting inputs in `fuzz/corpus/<target>/`.

Initial corpus seeds are provided for each target. Corpus directories must
correspond to an existing target; orphaned ones (e.g. for the long-removed
`fuzz_lion_signature` and `fuzz_polynomial` targets) have been deleted.

To minimize the corpus:

```bash
cargo fuzz cmin fuzz_clsag
```

## Coverage

Generate coverage report:

```bash
cargo fuzz coverage fuzz_clsag
```

## CI Integration

For CI, run fuzz tests with a time limit to catch regressions:

```yaml
- name: Fuzz Testing
  run: |
    cd fuzz
    cargo fuzz run fuzz_clsag -- -max_total_time=300
    cargo fuzz run fuzz_subaddress -- -max_total_time=300
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
    cargo fuzz run $target -- -max_total_time=86400
done
```

This ensures no crashes exist in the core crypto implementation.
