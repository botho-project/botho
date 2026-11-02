# libcrux evaluation harness (issue #1715)

Dev-only harness behind
[ADR 0013](../../../docs/decisions/0013-libcrux-pq-evaluation.md). It compares
Cryspen's libcrux ML-KEM-768 / ML-DSA-65 (`=0.0.11`) with the crates Botho
uses today, RustCrypto `ml-kem 0.2.3` / `ml-dsa 0.1.1`, and with Botho's own
`bth-crypto-pq` facade.

**This harness is not part of the node build.** Every crate here declares its
own `[workspace]`, and the root `Cargo.toml` does not list it. libcrux never
enters the node's dependency graph or the root `Cargo.lock`. Nothing here runs
in CI.

| Path | What it does |
|------|--------------|
| `src/lib.rs` | Byte-level adapters over both implementations. Every libcrux backend can be pinned (`Auto`, `Portable`, `Neon`, `Avx2`). Also reproduces Botho's ChaCha20-seeded ML-KEM keygen. |
| `tests/kat.rs` | NIST ACVP FIPS 203/204 vectors (ML-KEM-768 keyGen, encap, decap, and key checks; ML-DSA-65 keyGen, sigGen, and sigVer), run against both implementations. |
| `tests/cross.rs` | Cross-implementation tests: Botho keys reproduced by libcrux, Botho ciphertexts decapsulated by libcrux and the reverse, byte identity, implicit rejection, signatures in both directions, and tamper handling. |
| `examples/bench.rs` | Timed-loop throughput comparison. |
| `fetch-kats.sh` | Downloads the ACVP vectors from a pinned commit and checks their SHA-256. |
| `wasm-probe/` | A minimal `no_std` C-ABI crate. It checks that the wasm32, iOS, and Android builds work, compares sizes, and runs the KATs and a benchmark under Node. |
| `fuzz/` | `fuzz_mlkem_decapsulation` ported to libcrux. It also compares every result against `bth-crypto-pq`. |

## Reproduce

```bash
export CARGO_TARGET_DIR=/tmp/libcrux-eval-target   # anywhere outside the repo
./fetch-kats.sh /tmp/acvp
LIBCRUX_EVAL_KAT_DIR=/tmp/acvp LIBCRUX_EVAL_ITERS=10000 \
  cargo test --release -- --nocapture --test-threads=4
# Botho's release profile has overflow-checks = true; to match it:
CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS=true cargo run --release --example bench -- 3

# wasm32 (opt-level 3 matches the root release profile that wasm-pack uses)
cd wasm-probe
CARGO_PROFILE_RELEASE_OPT_LEVEL=3 cargo build --release --lib --target wasm32-unknown-unknown
node run-kats.mjs $CARGO_TARGET_DIR/wasm32-unknown-unknown/release/libcrux_wasm_probe.wasm /tmp/acvp
node bench.mjs    $CARGO_TARGET_DIR/wasm32-unknown-unknown/release/libcrux_wasm_probe.wasm /tmp/acvp
# baseline: add --no-default-features --features rustcrypto
# mobile: cargo rustc --release --lib --crate-type staticlib --target aarch64-apple-ios   (or aarch64-linux-android)

# fuzz (cargo-fuzz, nightly)
cd ../fuzz
cargo fuzz run -O -a fuzz_mlkem_decapsulation_libcrux -- -max_total_time=900 -fork=4
```
