# Build

## Requirements

- Rust (pinned nightly toolchain `nightly-2025-12-03`; see `rust-toolchain` — rustup selects it automatically)
- Cargo
- `cmake` (builds the vendored RandomX C++ library)
- `pkg-config` (Tauri desktop crate native dependencies)

## Building

```bash
cargo build --release
```

## Testing

```bash
cargo test
```

## Development

For development builds without optimizations:

```bash
cargo build
```

### IDE Support

An example workspace configuration for Rust Analyzer:

```json
{
    "rust-analyzer.checkOnSave.overrideCommand": [
        "cargo", "check", "--workspace", "--message-format=json", "--all-targets"
    ]
}
```
