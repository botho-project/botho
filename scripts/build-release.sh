#!/bin/bash
# Reproducible Release Build Script
#
# This script produces deterministic binaries by controlling all
# environment factors that can affect compilation output.
#
# Usage:
#   ./scripts/build-release.sh [--target TARGET] [--sign]
#
# Options:
#   --target TARGET  Build for specific target (default: host)
#   --sign           GPG sign the binaries after building
#
# Environment:
#   GPG_KEY_ID       GPG key ID for signing (required if --sign)
#
# Output:
#   dist/
#     botho                    - Main node binary
#     botho.sha256             - SHA256 checksum
#     botho-wallet             - Wallet CLI binary
#     botho-wallet.sha256      - SHA256 checksum
#     botho-exchange-scanner   - Exchange scanner binary
#     botho-exchange-scanner.sha256 - SHA256 checksum
#     checksums.txt            - All checksums in one file
#     *.sig                    - GPG signatures (if --sign)

set -euo pipefail

# Script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Parse arguments
TARGET=""
SIGN=false

while [[ $# -gt 0 ]]; do
    case $1 in
        --target)
            TARGET="$2"
            shift 2
            ;;
        --sign)
            SIGN=true
            shift
            ;;
        -h|--help)
            head -30 "$0" | tail -n +2 | sed 's/^# //' | sed 's/^#//'
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

cd "$PROJECT_ROOT"

echo "=== Reproducible Build ==="
echo "Project root: $PROJECT_ROOT"
echo "Target: ${TARGET:-host}"
echo "Sign: $SIGN"
echo ""

# ============================================================================
# Reproducibility Environment
# ============================================================================

# Use git commit timestamp for reproducible timestamps in binaries
# This ensures the same source always produces the same binary
export SOURCE_DATE_EPOCH=$(git log -1 --format=%ct)
echo "SOURCE_DATE_EPOCH: $SOURCE_DATE_EPOCH ($(date -r "$SOURCE_DATE_EPOCH" 2>/dev/null || date -d "@$SOURCE_DATE_EPOCH"))"

# Isolated cargo directories to prevent host contamination
export CARGO_HOME="$PROJECT_ROOT/.cargo-home"
export CARGO_TARGET_DIR="$PROJECT_ROOT/target-release"

# Deterministic locale and timezone
export LC_ALL=C.UTF-8
export TZ=UTC

# Rust flags for reproducibility
# - Disable incremental compilation (can cause non-determinism)
# - Use static relocation model for consistent addresses
export CARGO_INCREMENTAL=0
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$PROJECT_ROOT=botho"

# Ensure consistent ordering
export LANG=C.UTF-8

echo "CARGO_HOME: $CARGO_HOME"
echo "CARGO_TARGET_DIR: $CARGO_TARGET_DIR"
echo ""

# ============================================================================
# Build
# ============================================================================

echo "=== Building Release Binaries ==="

BUILD_ARGS=(
    --release
    --workspace
)

# Add target if specified
if [[ -n "$TARGET" ]]; then
    BUILD_ARGS+=(--target "$TARGET")
    BINARY_DIR="$CARGO_TARGET_DIR/$TARGET/release"
else
    BINARY_DIR="$CARGO_TARGET_DIR/release"
fi

# Build all binaries
cargo build "${BUILD_ARGS[@]}"

echo ""
echo "=== Collecting Binaries ==="

# Create dist directory
DIST_DIR="$PROJECT_ROOT/dist"
rm -rf "$DIST_DIR"
mkdir -p "$DIST_DIR"

# List of binaries to package
BINARIES=(
    "botho"
    "botho-wallet"
    "botho-exchange-scanner"
)

# Copy binaries and generate checksums
for bin in "${BINARIES[@]}"; do
    BINARY_PATH="$BINARY_DIR/$bin"

    if [[ -f "$BINARY_PATH" ]]; then
        cp "$BINARY_PATH" "$DIST_DIR/"

        # Generate individual checksum
        (cd "$DIST_DIR" && sha256sum "$bin" > "$bin.sha256")

        echo "  $bin: $(cat "$DIST_DIR/$bin.sha256" | cut -d' ' -f1)"
    else
        echo "  $bin: NOT FOUND (skipped)"
    fi
done

# Generate combined checksums file
(cd "$DIST_DIR" && sha256sum * 2>/dev/null | grep -v '\.sha256$' | grep -v '\.sig$' | grep -v 'checksums.txt' > checksums.txt) || true

echo ""
echo "=== Build Metadata ==="

# Save build metadata for verification
cat > "$DIST_DIR/build-info.txt" << EOF
Build Information
=================

Git Commit: $(git rev-parse HEAD)
Git Tag: $(git describe --tags --always 2>/dev/null || echo "untagged")
Build Date: $(date -u +"%Y-%m-%dT%H:%M:%SZ")
SOURCE_DATE_EPOCH: $SOURCE_DATE_EPOCH

Rust Version:
$(rustc --version)
$(cargo --version)

Target: ${TARGET:-$(rustc -vV | grep host | cut -d' ' -f2)}

Cargo Profile: release

Checksums (SHA256):
$(cat "$DIST_DIR/checksums.txt")
EOF

cat "$DIST_DIR/build-info.txt"

# ============================================================================
# Signing (optional)
# ============================================================================

if [[ "$SIGN" == "true" ]]; then
    echo ""
    echo "=== GPG Signing ==="

    if [[ -z "${GPG_KEY_ID:-}" ]]; then
        echo "ERROR: GPG_KEY_ID environment variable required for signing"
        exit 1
    fi

    for bin in "${BINARIES[@]}"; do
        if [[ -f "$DIST_DIR/$bin" ]]; then
            gpg --armor --detach-sign --default-key "$GPG_KEY_ID" "$DIST_DIR/$bin"
            echo "  Signed: $bin.asc"
        fi
    done

    # Sign the checksums file
    gpg --armor --detach-sign --default-key "$GPG_KEY_ID" "$DIST_DIR/checksums.txt"
    echo "  Signed: checksums.txt.asc"
fi

echo ""
echo "=== Build Complete ==="
echo "Output directory: $DIST_DIR"
echo ""
ls -la "$DIST_DIR"
