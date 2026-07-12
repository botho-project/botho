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
# Two complementary layers of trust, both covered by this script:
#
#   * Whole-bundle hash (option (b), #757). The built `dist/` tree is hashed into
#     one aggregate value a maintainer publishes and an operator compares before
#     importing their key. This is operator-enforced (you must remember to run
#     it) and is the natural companion to self-hosting the exact `dist/`.
#
#   * Operator-entry Subresource Integrity (option (a), #772). The operator
#     dashboard is now its own Vite build entry (`operator.html`) whose emitted
#     HTML pins `integrity="sha384-…"` on each JS/CSS chunk it references, so the
#     BROWSER refuses a tampered chunk with no operator action required. The
#     `--verify-sri` mode below re-derives each pinned hash from the file on disk
#     and fails on any missing/mismatched attribute — the scriptable equivalent
#     of the browser's own check.
#
# The whole-bundle hash computes:
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
#   web/scripts/verify-operator-bundle.sh --verify-sri [--dist DIR]
#
# Options:
#   --dist DIR        Path to the built bundle (default: dist next to this repo's
#                     web-wallet package, i.e. web/packages/web-wallet/dist)
#   --expected HASH   Compare the computed aggregate bundle hash against HASH and
#                     exit non-zero on mismatch. Without this flag the script
#                     just prints the hash (publish mode).
#   --manifest        Also print the full per-file SHA256SUMS listing to stdout.
#   --verify-sri      Verify the operator entry's browser-enforced SRI: parse
#                     dist/operator.html, and for every root-relative <script>/
#                     <link> reference confirm it has an integrity="sha384-…"
#                     attribute whose hash matches the file on disk. Exits 3 on
#                     any missing or mismatched attribute. Runs instead of the
#                     aggregate-hash computation.
#
# Exit codes:
#   0  hash computed (and matched --expected, if given) / SRI verified
#   1  usage / environment error (no dist dir, etc.)
#   2  hash mismatch against --expected
#   3  operator-entry SRI verification failed (--verify-sri)
#
# Example (maintainer, publishing):
#   pnpm --filter @botho/web-wallet build
#   web/scripts/verify-operator-bundle.sh
#   # -> prints: operator bundle hash: sha256-<...>
#
# Example (operator, verifying a build they made from a pinned tag):
#   web/scripts/verify-operator-bundle.sh --expected sha256-<published-value>
#
# Example (confirm the operator entry's browser-enforced SRI is intact):
#   web/scripts/verify-operator-bundle.sh --verify-sri

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# web/scripts -> web/packages/web-wallet
DEFAULT_DIST="$(cd "$SCRIPT_DIR/../packages/web-wallet" && pwd)/dist"

DIST_DIR="$DEFAULT_DIST"
EXPECTED=""
PRINT_MANIFEST=0
VERIFY_SRI=0

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
        --verify-sri)
            VERIFY_SRI=1
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

# sha384 of a file, as base64 (the encoding SRI uses in `sha384-<base64>`).
sha384_b64() {
    # shasum prints hex; convert hex -> raw -> base64. Portable across macOS
    # (shasum) and Linux (sha384sum).
    if command -v sha384sum >/dev/null 2>&1; then
        sha384sum "$1" | awk '{print $1}' | xxd -r -p | base64 | tr -d '\n'
    else
        shasum -a 384 "$1" | awk '{print $1}' | xxd -r -p | base64 | tr -d '\n'
    fi
}

# --verify-sri: browser-enforced integrity check for the operator entry (#772,
# §8.3.1 option (a)). Parse dist/operator.html and confirm every root-relative
# <script src>/<link href> reference carries an integrity="sha384-…" attribute
# whose hash matches the file on disk — the scriptable equivalent of the check a
# browser performs on load. Exits 3 on any missing/mismatched attribute.
if [[ "$VERIFY_SRI" -eq 1 ]]; then
    OPERATOR_HTML="$DIST_DIR/operator.html"
    if [[ ! -f "$OPERATOR_HTML" ]]; then
        echo "error: operator entry not found: $OPERATOR_HTML" >&2
        echo "       build it first: pnpm --filter @botho/web-wallet build" >&2
        exit 1
    fi

    echo "operator entry SRI verification"
    echo "  html:  $OPERATOR_HTML"

    fail=0
    checked=0
    # Explicit error handling below (grep returns non-zero on no-match, which we
    # treat as "attribute absent"), so relax `set -e`/pipefail for this block.
    set +e
    set +o pipefail
    # Extract every <script …>/<link …> tag onto its own line, then inspect the
    # ones that reference a root-relative /assets/ chunk.
    tags="$(tr '\n' ' ' < "$OPERATOR_HTML" | grep -oE '<(script|link)\b[^>]*>')"
    while IFS= read -r tag; do
        [[ -z "$tag" ]] && continue
        # Only script/stylesheet/modulepreload tags carry executable sub-resources.
        is_script=0; is_asset_link=0
        [[ "$tag" =~ ^\<script ]] && is_script=1
        if [[ "$tag" =~ ^\<link ]] && \
           { [[ "$tag" =~ rel=\"stylesheet\" ]] || [[ "$tag" =~ rel=\"modulepreload\" ]]; }; then
            is_asset_link=1
        fi
        [[ "$is_script" -eq 0 && "$is_asset_link" -eq 0 ]] && continue

        if [[ "$is_script" -eq 1 ]]; then
            ref="$(printf '%s' "$tag" | grep -oE 'src="[^"]+"' | head -1 | sed 's/^src="//;s/"$//')"
        else
            ref="$(printf '%s' "$tag" | grep -oE 'href="[^"]+"' | head -1 | sed 's/^href="//;s/"$//')"
        fi
        # Only local, root-relative chunk references are integrity-checkable.
        [[ "$ref" == /assets/* ]] || continue

        integrity="$(printf '%s' "$tag" | grep -oE 'integrity="sha384-[^"]+"' | head -1 | sed 's/^integrity="sha384-//;s/"$//')"
        if [[ -z "$integrity" ]]; then
            echo "  MISSING integrity: $ref" >&2
            fail=1
            continue
        fi

        file="$DIST_DIR/${ref#/}"
        if [[ ! -f "$file" ]]; then
            echo "  MISSING file for pinned ref: $ref" >&2
            fail=1
            continue
        fi
        actual="$(sha384_b64 "$file")"
        if [[ "$actual" != "$integrity" ]]; then
            echo "  MISMATCH: $ref" >&2
            echo "    pinned:   sha384-$integrity" >&2
            echo "    on disk:  sha384-$actual" >&2
            fail=1
            continue
        fi
        checked=$((checked + 1))
        echo "  OK: $ref"
    done <<< "$tags"

    if [[ "$checked" -eq 0 && "$fail" -eq 0 ]]; then
        echo "error: no integrity-pinned asset references found in operator.html" >&2
        echo "       (expected the SRI plugin to pin the operator entry chunks)" >&2
        exit 3
    fi
    if [[ "$fail" -ne 0 ]]; then
        echo "FAIL: operator entry SRI verification failed." >&2
        echo "  Do NOT import your operator key into this bundle." >&2
        exit 3
    fi
    echo "MATCH: all $checked operator asset references have a correct sha384 integrity hash."
    exit 0
fi

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
