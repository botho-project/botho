// Copyright (c) 2024 The Botho Foundation

//! Native (non-libFuzzer) smoke driver for the bridge fuzz harnesses (#920).
//!
//! # Why this exists
//!
//! On macOS 26.5 the pinned nightly's AddressSanitizer runtime deadlocks in
//! `dyld` before `main`, so `cargo fuzz run` never executes a single input on
//! a Mac (see `fuzz/README.md` and issue #920). This binary runs the *exact
//! same* harness bodies (`botho_fuzz::run_target_from_bytes`) with no
//! libFuzzer and no sanitizer, on a stock toolchain, so macOS developers can
//! still smoke-test the bridge parsers before pushing.
//!
//! It is **not** coverage-guided — that still happens in ubuntu CI
//! (`.github/workflows/fuzz.yml`). What it does give you: every committed
//! corpus seed is replayed through the shared harness, plus a configurable
//! number of pseudo-randomly generated inputs, all under `debug_assertions`,
//! so any panic or violated invariant surfaces as a non-zero exit.
//!
//! # Usage
//!
//! ```text
//! # From the fuzz/ directory (feature-gated so it never builds in a normal
//! # `cargo build`, matching the fuzz crate's build-only posture):
//! cargo run --features native-smoke --bin native_smoke
//!
//! # Choose the number of randomized inputs per target (default 100_000):
//! BOTHO_FUZZ_SMOKE_ITERS=2000 cargo run --features native-smoke --bin native_smoke
//!
//! # Deterministic seed for reproducible runs:
//! BOTHO_FUZZ_SMOKE_SEED=42 cargo run --features native-smoke --bin native_smoke
//!
//! # Restrict to one target:
//! cargo run --features native-smoke --bin native_smoke -- fuzz_federation_status_line
//! ```

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

use botho_fuzz::{run_target_from_bytes, NATIVE_SMOKE_TARGETS};
use rand::{RngCore, SeedableRng};
use rand_chacha::ChaCha8Rng;

/// Default number of randomized inputs per target. ~100k is the "dev" budget
/// from #920; CI overrides this with a small value via the env var for a cheap
/// smoke job.
const DEFAULT_ITERS: usize = 100_000;

/// Upper bound on a randomly generated input's length, in bytes. Large enough
/// to fill the structured `Arbitrary` inputs the harnesses decode, small
/// enough to keep 100k iterations fast.
const MAX_RANDOM_LEN: usize = 512;

fn main() -> ExitCode {
    install_panic_hook();

    let args: Vec<String> = env::args().skip(1).collect();

    let iters: usize = env::var("BOTHO_FUZZ_SMOKE_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_ITERS);

    let seed: u64 = env::var("BOTHO_FUZZ_SMOKE_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0xB074_05EE_D000);

    // Which targets to run: either the ones named on the command line, or all
    // of them. Bail loudly on an unknown name so a typo can't silently pass.
    let targets: Vec<&str> = if args.is_empty() {
        NATIVE_SMOKE_TARGETS.to_vec()
    } else {
        for name in &args {
            if !NATIVE_SMOKE_TARGETS.contains(&name.as_str()) {
                eprintln!(
                    "error: unknown target '{name}'. Known targets: {}",
                    NATIVE_SMOKE_TARGETS.join(", ")
                );
                return ExitCode::FAILURE;
            }
        }
        args.iter().map(String::as_str).collect()
    };

    let corpus_root = corpus_root();
    println!(
        "native-smoke: {} target(s), {iters} randomized input(s) each, seed={seed:#x}",
        targets.len()
    );
    println!("corpus root: {}", corpus_root.display());

    let mut total_corpus = 0usize;
    for target in &targets {
        // 1. Replay every committed corpus seed.
        let corpus_dir = corpus_root.join(target);
        let seeds = read_corpus(&corpus_dir);
        for (path, bytes) in &seeds {
            // A panic here aborts the process with the assertion message and a
            // non-zero exit — exactly the failure signal we want, and it names
            // the offending seed via the location in the message below.
            run_one(target, bytes, &format!("corpus seed {}", path.display()));
        }
        total_corpus += seeds.len();

        // 2. Feed randomized inputs through the same harness.
        let mut rng = ChaCha8Rng::seed_from_u64(seed ^ fnv1a(target));
        let mut buf = vec![0u8; MAX_RANDOM_LEN];
        for i in 0..iters {
            let len = (rng.next_u32() as usize) % (MAX_RANDOM_LEN + 1);
            rng.fill_bytes(&mut buf[..len]);
            run_one(target, &buf[..len], &format!("random input #{i}"));
        }

        println!(
            "  {target}: {} corpus seed(s) + {iters} randomized input(s) OK",
            seeds.len()
        );
    }

    println!(
        "native-smoke: PASS — {} corpus seed(s) + {} randomized input(s) across {} target(s), no panics",
        total_corpus,
        iters * targets.len(),
        targets.len()
    );
    ExitCode::SUCCESS
}

/// Run one input through the shared harness. We do NOT catch the panic: a
/// violated invariant must fail the process. A per-input breadcrumb is stashed
/// so the panic hook can name the exact input that triggered the failure.
fn run_one(target: &str, bytes: &[u8], what: &str) {
    LAST_INPUT.with(|cell| {
        *cell.borrow_mut() = format!("{target}: {what} ({} bytes)", bytes.len());
    });
    run_target_from_bytes(target, bytes);
}

thread_local! {
    static LAST_INPUT: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// Read every regular file in a corpus directory as a raw input. Missing
/// directories yield an empty list (a target may legitimately have no seeds).
fn read_corpus(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            if let Ok(bytes) = fs::read(&path) {
                out.push((path, bytes));
            }
        }
    }
    // Deterministic order so a failing seed is reproducible run to run.
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Locate the `corpus/` directory relative to this crate, independent of the
/// process's current working directory.
fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus")
}

/// Tiny FNV-1a so each target gets a distinct-but-deterministic RNG stream
/// from the same base seed.
fn fnv1a(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Install a panic hook that surfaces the offending input's breadcrumb (the
/// default hook still prints the assertion message and location).
fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        LAST_INPUT.with(|cell| {
            let last = cell.borrow();
            if !last.is_empty() {
                eprintln!("native-smoke: FAILURE on {last}");
            }
        });
        default(info);
    }));
}
