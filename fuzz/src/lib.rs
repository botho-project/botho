// Copyright (c) 2024 The Botho Foundation

//! Shared fuzz-harness bodies for the `botho-fuzz` crate (#920).
//!
//! # Why this library exists
//!
//! Each `fuzz_targets/*.rs` binary is a libFuzzer entry point that can only be
//! **run** with a sanitizer-instrumented toolchain. On macOS 26.5 that
//! toolchain's AddressSanitizer runtime deadlocks in `dyld` before `main`
//! (see [`fuzz/README.md`](../README.md) and issue #920), so macOS developers
//! cannot execute the coverage-guided fuzzer locally at all.
//!
//! To keep the harnesses runnable on macOS, every fuzz target's *body* (its
//! `Arbitrary` input type plus the assertions it makes) lives here as a plain
//! `pub fn run_from_bytes(&[u8])`. The libFuzzer target and the native-smoke
//! driver both call the **same** function, so there is a single source of
//! truth: a harness change can never drift between the coverage-guided path
//! and the macOS smoke path.
//!
//! # The two consumers
//!
//! * `fuzz_targets/<target>.rs` — a one-line `fuzz_target!(|input: T| {
//!   <module>::run(&input); });`. libFuzzer decodes `T` via
//!   `Arbitrary::arbitrary_take_rest`, the same decoding [`decode_take_rest`]
//!   uses, so the two paths agree byte-for-byte. Coverage-guided,
//!   sanitizer-built, runs in ubuntu CI (`.github/workflows/fuzz.yml`).
//! * The native-smoke driver (`bin/native_smoke.rs`, and the `#[cfg(test)]`
//!   suite here) — feeds every committed `fuzz/corpus/<target>/*` seed plus a
//!   configurable number of randomized inputs through the *same*
//!   `run_from_bytes`, with `debug_assertions` on, on a stock toolchain. No
//!   libFuzzer, no sanitizer, so it runs on macOS today.
//!
//! # Input decoding parity
//!
//! libFuzzer's `fuzz_target!(|input: T|)` builds `T` via
//! `Arbitrary::arbitrary_take_rest`. The native-smoke path decodes bytes the
//! same way (see [`decode_take_rest`]); when a target consumes raw `&[u8]`
//! directly, `run_from_bytes` passes the bytes through unchanged. Either way
//! the byte-to-work mapping is identical to what libFuzzer would do, so a
//! corpus seed exercises the same code path in both worlds.

use arbitrary::{Arbitrary, Unstructured};

pub mod bridge_attestation;
pub mod bridge_ethereum_parse;
pub mod bridge_solana_parse;
pub mod federation_status_line;

/// Decode a structured fuzz input from raw bytes exactly the way
/// `libfuzzer_sys::fuzz_target!(|input: T|)` does (`arbitrary_take_rest`).
///
/// Returns `None` when the bytes cannot produce a value (e.g. not enough
/// entropy); callers skip those inputs, mirroring libFuzzer, which simply
/// discards runs where `Arbitrary` construction fails.
pub fn decode_take_rest<'a, T: Arbitrary<'a>>(data: &'a [u8]) -> Option<T> {
    T::arbitrary_take_rest(Unstructured::new(data)).ok()
}

/// The set of targets whose harness bodies are shared through this library and
/// exercised by the native-smoke driver. Keep this in lockstep with the
/// `run_from_bytes` functions below and the `[[bin]]` fuzz targets.
pub const NATIVE_SMOKE_TARGETS: &[&str] = &[
    "fuzz_bridge_attestation",
    "fuzz_bridge_ethereum_parse",
    "fuzz_bridge_solana_parse",
    "fuzz_federation_status_line",
];

/// Dispatch one raw fuzz input to the named target's shared harness body.
///
/// Used by the native-smoke driver so a single loop can drive every target.
/// Panics (via the harness assertions) exactly where the libFuzzer target
/// would. Unknown target names are a programming error.
pub fn run_target_from_bytes(target: &str, data: &[u8]) {
    match target {
        "fuzz_bridge_attestation" => bridge_attestation::run_from_bytes(data),
        "fuzz_bridge_ethereum_parse" => bridge_ethereum_parse::run_from_bytes(data),
        "fuzz_bridge_solana_parse" => bridge_solana_parse::run_from_bytes(data),
        "fuzz_federation_status_line" => federation_status_line::run_from_bytes(data),
        other => panic!("unknown native-smoke target: {other}"),
    }
}

// ============================================================================
// Native-smoke test suite (#920)
// ============================================================================
//
// Gated behind the `native-smoke` feature so it only runs when explicitly
// requested (`cargo test -p botho-fuzz --features native-smoke`). This is the
// macOS-runnable substitute for `cargo fuzz run`: it replays every committed
// corpus seed through the SAME `run_from_bytes` the libFuzzer targets call,
// plus a modest number of deterministic randomized inputs, with
// debug-assertions on. No libFuzzer, no sanitizer — so it does not trip the
// macOS 26.5 ASan/dyld deadlock (#920).
#[cfg(all(test, feature = "native-smoke"))]
mod native_smoke_tests {
    use super::*;
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use rand::{RngCore, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    /// Randomized inputs per target for the `cargo test` path. Kept modest so
    /// the suite stays cheap; the `native_smoke` binary is the place for the
    /// ~100k dev budget. Override with `BOTHO_FUZZ_SMOKE_ITERS`.
    const TEST_ITERS: usize = 2_000;
    const MAX_RANDOM_LEN: usize = 512;

    fn iters() -> usize {
        std::env::var("BOTHO_FUZZ_SMOKE_ITERS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(TEST_ITERS)
    }

    fn corpus_dir(target: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("corpus")
            .join(target)
    }

    /// Replay every committed corpus seed for a target through the shared
    /// harness. A violated invariant panics the test, naming the seed.
    fn replay_corpus(target: &str) -> usize {
        let dir = corpus_dir(target);
        let Ok(entries) = fs::read_dir(&dir) else {
            return 0;
        };
        let mut count = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                let bytes = fs::read(&path).expect("read corpus seed");
                // Panics here abort the test with the assertion message; the
                // seed path is included so the failure is reproducible.
                run_target_from_bytes(target, &bytes);
                count += 1;
            }
        }
        count
    }

    /// Feed deterministic randomized inputs through the shared harness.
    fn replay_random(target: &str) {
        let mut rng = ChaCha8Rng::seed_from_u64(0xB074_05EE_D000);
        let mut buf = vec![0u8; MAX_RANDOM_LEN];
        for _ in 0..iters() {
            let len = (rng.next_u32() as usize) % (MAX_RANDOM_LEN + 1);
            rng.fill_bytes(&mut buf[..len]);
            run_target_from_bytes(target, &buf[..len]);
        }
    }

    fn smoke(target: &str) {
        assert!(
            NATIVE_SMOKE_TARGETS.contains(&target),
            "{target} missing from NATIVE_SMOKE_TARGETS"
        );
        replay_corpus(target);
        replay_random(target);
    }

    #[test]
    fn smoke_bridge_attestation() {
        smoke("fuzz_bridge_attestation");
    }

    #[test]
    fn smoke_bridge_ethereum_parse() {
        smoke("fuzz_bridge_ethereum_parse");
    }

    #[test]
    fn smoke_bridge_solana_parse() {
        smoke("fuzz_bridge_solana_parse");
    }

    #[test]
    fn smoke_federation_status_line() {
        smoke("fuzz_federation_status_line");
    }
}
