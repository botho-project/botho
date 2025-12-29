# Contributing to Cadence

Thanks for your interest in contributing to Cadence!

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
