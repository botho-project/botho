# Build

## Requirements

- Rust (stable toolchain)
- Cargo

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
