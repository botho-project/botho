# Contributing to Botho

Thanks for your interest in contributing to Botho!

## Getting Started

1. Fork this repository
2. Clone your fork locally
3. Create a branch for your changes
4. Make your changes and commit
5. Push to your fork and create a pull request

## Building

```bash
cargo build
cargo test
```

## Ledger tests on macOS

Run the complete ledger unit suite with the platform-aware launcher:

```bash
./scripts/test-ledger.sh
# Or execute an already-compiled botho library test binary:
./scripts/test-ledger.sh --binary /absolute/path/to/botho-lib-test
```

The Cargo path uses `--locked -p botho --lib ledger::`. Neither mode adds skip filters or retries failures; normal libtest ignored-test
semantics still apply. Non-macOS platforms retain their existing libtest concurrency;
the Linux CI gate remains unchanged. A supplied binary must be the intended
checkout's compiled library test target; the launcher does not establish provenance.

macOS System V semaphores limit outstanding per-process `SEM_UNDO` entries.
On the observed host, `kern.sysv.semume=10`: holding writers in ten distinct real
Ledger environments caused normal opening of an eleventh to fail with EINVAL.
The launcher reads this limit without modifying it, budgets two entries per active
ledger test plus two entries of headroom, and uses at most four test threads.
Smaller positive `RUST_TEST_THREADS` requests are preserved; larger requests are
capped. Malformed limits/requests or limits below four fail before tests start.
The effective limit and concurrency are printed for reproducible evidence.

The budget follows the current ledger tests: one held writer and a transient
reader-allocation lock, without spawned background fixture workers. Review the
budget if tests add simultaneous writers across environments or background work.
It is not a guarantee for arbitrary application embeddings or the entire library's
other test suites. Ordinary node startup opens one shared Ledger environment;
that specific ledger uses two semaphore numbers, unlike concurrent temporary
ledger fixtures. Tools or custom applications may open additional environments.

Production locking remains System V. The considered `heed/posix-sem` alternative
was rejected: LMDB documents automatic stale-writer recovery with System V
`SEM_UNDO`, whereas plain POSIX semaphore configurations can require all programs
using an environment to close and reopen after a stale lock. The test launcher
does not alter database formats, crash recovery, global kernel settings or Cargo
runners used by production commands. The demonstrated resource boundary does not
prove that every historical EINVAL has the same cause.

## Coding Style

### Automated Checks

We use these tools to maintain code quality:

* `rustfmt`: Formats code according to [rustfmt.toml](rustfmt.toml)
* `cargo clippy`: Checks for non-idiomatic Rust patterns

### Style Guidelines

We follow the [Rust Style Guide](https://doc.rust-lang.org/1.0.0/style/style/README.html) with these additions:

#### Sort Your Imports

Order imports as follows:

1. `extern crate` directives
2. `pub use` re-exports
3. `pub mod` exports
4. `mod` definitions
5. `use` imports

Example:

```rust
extern crate alloc;

pub use crate::module::TypeToExport;
pub use dependency::TypeWereUsing;

mod module;

use dependency::SomeTypeWeUseOurselves;
```

#### Export Types at the Crate Level

Re-export all publicly visible types at the crate level for easier discovery.

#### Use Scopes Instead of Manual Drops

Prefer `{}`-braced scopes over `core::drop()`:

```rust
fn use_mutex(m: sync::mutex::Mutex<int>) {
    {
        let guard = m.lock();
        do_work(guard);
    } // unlocking happens automatically
    // do other work
}
```
