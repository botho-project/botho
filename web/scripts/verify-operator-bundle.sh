#!/bin/bash
# Verify the operator dashboard bundle
#
# The operator dashboard (`/operator`, see docs/security/quorum-write-path.md)
# imports the operator Ed25519 key into the browser and signs quorum-curation
# envelopes client-side. §8.3 of that doc identifies the serious residual risk:
# a malicious dashboard *bundle* could prompt the operator to sign
# attacker-chosen bytes. This script lets an operator (or an independent
# verifier) confirm that the bundle they are about to trust is bit-for-bit the
# one a maintainer published a hash for, before they ever import their key.
#
# Because `/operator` is one route inside the shared Vite SPA
# (web/packages/web-wallet), the unit of trust is the *whole* built `dist/`
# tree, not a single chunk. This script computes:
#
#   1. Per-file SHA-256 checksums for every asset in `dist/` (SHA256SUMS-style),
#      excluding non-deterministic build artifacts (source maps, the PWA
#      service-worker manifest revision, etc.), and
#   2. A single aggregate "bundle hash" = SHA-256 over the sorted per-file
#      checksum list. This one value is what a maintainer publishes and an
#      operator compares.
#
# This parallels scripts/build-release.sh + SHA256SUMS.txt for the node
# binaries (docs/operations/reproducible-builds.md), extended to the web bundle.
#
# Usage:
#   web/scripts/verify-operator-bundle.sh [--dist DIR] [--expected HASH] [--manifest]
#
# Options:
#   --dist DIR        Path to the built bundle (default: dist next to this repo's
#                     web-wallet package, i.e. web/packages/web-wallet/dist)
#   --expected HASH   Compare the computed aggregate bundle hash against HASH and
#                     exit non-zero on mismatch. Without this flag the script
#                     just prints the hash (publish mode).
#   --manifest        Also print the full per-file SHA256SUMS listing to stdout.
#
# Exit codes:
#   0  hash computed (and matched --expected, if given)
#   1  usage / environment error (no dist dir, etc.)
#   2  hash mismatch against --expected
#
# Example (maintainer, publishing):
#   pnpm --filter @botho/web-wallet build
#   web/scripts/verify-operator-bundle.sh
#   # -> prints: operator bundle hash: sha256-<...>
#
# Example (operator, verifying a build they made from a pinned tag):
#   web/scripts/verify-operator-bundle.sh --expected sha256-<published-value>

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# web/scripts -> web/packages/web-wallet
DEFAULT_DIST="$(cd "$SCRIPT_DIR/../packages/web-wallet" && pwd)/dist"

DIST_DIR="$DEFAULT_DIST"
EXPECTED=""
PRINT_MANIFEST=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --dist)
            DIST_DIR="$2"
            shift 2
            ;;
        --expected)
            EXPECTED="$2"
            shift 2
            ;;
        --manifest)
            PRINT_MANIFEST=1
            shift
            ;;
        --)
            # Bare separator (e.g. inserted by `pnpm run … -- <args>`); skip it.
            shift
            ;;
        -h|--help)
            grep '^#' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "error: unknown argument: $1" >&2
            exit 1
            ;;
    esac
done

if [[ ! -d "$DIST_DIR" ]]; then
    echo "error: dist directory not found: $DIST_DIR" >&2
    echo "       build it first: pnpm --filter @botho/web-wallet build" >&2
    exit 1
fi

# Portable SHA-256: coreutils sha256sum on Linux, shasum on macOS.
sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$@"
    else
        shasum -a 256 "$@"
    fi
}

# Files excluded from the trust set because they are not part of the executable
# bundle the browser runs, or are not deterministic across environments:
#   *.map              source maps — debugging aid, not shipped to the browser
#                      runtime as executable code; large and toolchain-sensitive.
# The service-worker manifest (sw.js / workbox-*.js) IS included: it controls
# what the PWA caches and executes, so it is part of the trust boundary. See the
# self-hosting runbook for how to pin the deployment against auto-update.
EXCLUDE_REGEX='\.map$'

# Compute per-file checksums with paths RELATIVE to dist so the manifest is
# stable regardless of where the checkout lives. Sort by path for determinism.
MANIFEST="$(
    cd "$DIST_DIR"
    # -type f, prune the excluded patterns, hash each, strip the leading "./".
    find . -type f | LC_ALL=C sort | grep -Ev "$EXCLUDE_REGEX" | while IFS= read -r f; do
        rel="${f#./}"
        line="$(sha256 "$f")"
        # Replace the (possibly "./"-prefixed) path with the normalized rel path
        # while keeping the hash column, so the manifest is host-independent.
        hash="${line%% *}"
        printf '%s  %s\n' "$hash" "$rel"
    done
)"

if [[ -z "$MANIFEST" ]]; then
    echo "error: no files hashed under $DIST_DIR (empty build?)" >&2
    exit 1
fi

# Aggregate bundle hash = SHA-256 over the sorted manifest text.
AGG="$(printf '%s\n' "$MANIFEST" | sha256 | awk '{print $1}')"
BUNDLE_HASH="sha256-$AGG"

if [[ "$PRINT_MANIFEST" -eq 1 ]]; then
    echo "=== per-file SHA256SUMS ($DIST_DIR) ==="
    printf '%s\n' "$MANIFEST"
    echo "==="
fi

echo "operator bundle hash: $BUNDLE_HASH"
echo "  dist:  $DIST_DIR"
echo "  files: $(printf '%s\n' "$MANIFEST" | wc -l | tr -d ' ')"

if [[ -n "$EXPECTED" ]]; then
    if [[ "$BUNDLE_HASH" == "$EXPECTED" ]]; then
        echo "MATCH: bundle hash matches expected value."
        exit 0
    else
        echo "MISMATCH: bundle hash does NOT match expected value!" >&2
        echo "  expected: $EXPECTED" >&2
        echo "  actual:   $BUNDLE_HASH" >&2
        echo "  Do NOT import your operator key into this bundle." >&2
        exit 2
    fi
fi
