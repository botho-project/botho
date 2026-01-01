# Reproducible Builds

This document explains how to verify Botho release binaries using reproducible builds.

## Overview

Reproducible builds ensure that the same source code always produces identical binaries. This allows anyone to:

1. **Verify releases**: Confirm that published binaries match the source code
2. **Detect tampering**: Identify if binaries have been modified
3. **Build trust**: Multiple parties can independently verify the same result

## Quick Verification

To verify a release binary:

```bash
# 1. Download the release
wget https://github.com/botho-project/botho/releases/download/v1.0.0/botho-v1.0.0-linux-x86_64.tar.gz
wget https://github.com/botho-project/botho/releases/download/v1.0.0/SHA256SUMS.txt

# 2. Verify the checksum
sha256sum -c SHA256SUMS.txt

# 3. (Optional) Rebuild from source and compare
git checkout v1.0.0
./scripts/build-release.sh
cat dist/checksums.txt  # Should match SHA256SUMS.txt
```

## How It Works

### Pinned Rust Toolchain

The project uses a pinned nightly Rust version in `rust-toolchain`:

```toml
[toolchain]
channel = "nightly-2025-12-03"
components = ["rustfmt", "clippy"]
```

This ensures all builders use the identical compiler version.

### Reproducible Build Script

The `scripts/build-release.sh` script controls all environment factors:

| Factor | Setting | Purpose |
|--------|---------|---------|
| `SOURCE_DATE_EPOCH` | Git commit timestamp | Consistent embedded timestamps |
| `CARGO_INCREMENTAL` | `0` | Disable incremental compilation |
| `RUSTFLAGS` | `--remap-path-prefix` | Normalize file paths |
| `LC_ALL` | `C.UTF-8` | Consistent locale |
| `TZ` | `UTC` | Consistent timezone |
| `CARGO_HOME` | Isolated | Prevent host contamination |

### Build Isolation

Each build uses isolated directories:

```
project/
├── .cargo-home/       # Isolated cargo cache
├── target-release/    # Isolated build output
└── dist/              # Final binaries and checksums
```

## Full Verification Process

### Prerequisites

- Git
- Rust (installed via rustup)
- Same OS as the target platform

### Step 1: Clone and Checkout

```bash
git clone https://github.com/botho-project/botho.git
cd botho
git checkout v1.0.0
```

### Step 2: Build

```bash
./scripts/build-release.sh
```

### Step 3: Compare Checksums

```bash
# Download official checksums
wget https://github.com/botho-project/botho/releases/download/v1.0.0/SHA256SUMS.txt

# Compare
diff <(sort dist/checksums.txt) <(sort SHA256SUMS.txt)
```

If the diff is empty, your build matches the official release.

### Step 4: GPG Signature Verification (Optional)

For signed releases:

```bash
# Import the release signing key
gpg --keyserver keyserver.ubuntu.com --recv-keys <KEY_ID>

# Verify signature
gpg --verify SHA256SUMS.txt.asc SHA256SUMS.txt
```

## Cross-Platform Verification

### Linux (x86_64)

```bash
./scripts/build-release.sh --target x86_64-unknown-linux-gnu
```

### macOS (Intel)

```bash
./scripts/build-release.sh --target x86_64-apple-darwin
```

### macOS (Apple Silicon)

```bash
./scripts/build-release.sh --target aarch64-apple-darwin
```

### Windows

```bash
./scripts/build-release.sh --target x86_64-pc-windows-msvc
```

## Docker Reference Build

For maximum reproducibility, use Docker:

```bash
docker run --rm -v $(pwd):/workspace -w /workspace \
  rust:1.83-bookworm \
  bash -c "
    rustup default nightly-2025-12-03 && \
    ./scripts/build-release.sh --target x86_64-unknown-linux-gnu
  "
```

This provides an identical build environment regardless of host system.

## Troubleshooting

### Checksums Don't Match

1. **Wrong Rust version**: Verify `rustc --version` matches the pinned version
2. **Different target**: Ensure you're building for the correct platform
3. **Modified source**: Check `git status` for uncommitted changes
4. **Native dependencies**: Some dependencies compile differently across systems

### Build Fails

1. **Missing dependencies**: Install platform-specific build tools
2. **Disk space**: Ensure sufficient space for build artifacts (~5GB)
3. **Memory**: Large builds may require 8GB+ RAM

## Known Limitations

1. **Native code**: Dependencies with C/C++ code may vary across systems
2. **Nightly Rust**: Using nightly means the toolchain date is critical
3. **Cross-compilation**: Some targets require additional setup

## CI/CD Integration

The GitHub Actions workflow automatically:

1. Builds for all supported platforms
2. Generates checksums
3. Runs a reproducibility check (rebuild and compare)
4. Publishes signed releases

See `.github/workflows/release.yml` for details.

## Security Considerations

### Supply Chain Security

- All dependencies are locked in `Cargo.lock`
- Patched crates use specific Git commits
- The build script isolates cargo cache

### Verification Recommendations

For high-security deployments:

1. Build from source on trusted hardware
2. Compare checksums with multiple independent verifiers
3. Verify GPG signatures when available
4. Audit the build script before running

## Resources

- [Reproducible Builds](https://reproducible-builds.org/) - Standards and best practices
- [Rust Reproducibility](https://reproducible-builds.org/docs/source-date-epoch/) - Rust-specific guidance
- [cargo-auditable](https://github.com/rust-secure-code/cargo-auditable) - Dependency auditing

## Release Key

The Botho project signs releases with the following GPG key:

```
Key ID: [TO BE PUBLISHED]
Fingerprint: [TO BE PUBLISHED]
```

The public key is available at:
- GitHub: https://github.com/botho-project/botho/blob/main/keys/release-signing-key.asc
- Keyserver: keyserver.ubuntu.com
