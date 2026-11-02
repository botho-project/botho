#!/usr/bin/env bash
# Regenerate (or check) the Aeneas Lean extraction of the cluster-tax kernels
# that Charon + Aeneas accept unchanged (#1712, epic #1704).
#
# Usage:
#   formal-aeneas/regen.sh            # rewrite formal-aeneas/generated/
#   formal-aeneas/regen.sh --check    # fail if generated/ is out of date
#
# Toolchain (pinned in formal-aeneas/toolchain.env):
#   - Aeneas + bundled Charon from the AENEAS_TAG release. Set AENEAS_DIR to an
#     unpacked release directory (containing `aeneas` and `charon`), or leave it
#     unset and the script downloads the release asset for this platform into
#     AENEAS_CACHE (default ~/.cache/botho-aeneas) and checks its sha256.
#   - The Rust nightly CHARON_RUST_NIGHTLY with CHARON_RUST_COMPONENTS, which
#     charon-driver links against. Install it with
#         rustup toolchain install "$CHARON_RUST_NIGHTLY" --profile minimal \
#             -c "$CHARON_RUST_COMPONENTS"
#     (adding a toolchain does not change the rustup default). To keep it
#     entirely separate from your normal rustup state, export a dedicated
#     RUSTUP_HOME first. The workspace `rust-toolchain` is neither read nor
#     changed: charon selects its own toolchain.
#
# Charon builds into CHARON_TARGET_DIR (default target/aeneas-charon), never
# the workspace target dir, because it compiles with a different rustc.
#
# The output is deterministic for a given toolchain.env and source tree. It
# embeds source line spans, so any edit to a mirrored Rust file (even one that
# only moves lines) makes --check fail until regen.sh is re-run. That is the
# point: a Rust change cannot silently drift away from the Lean the proofs
# are about.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
# shellcheck source=formal-aeneas/toolchain.env
. "$here/toolchain.env"

check=0
case "${1:-}" in
  --check) check=1 ;;
  "") ;;
  *) echo "usage: $0 [--check]" >&2; exit 2 ;;
esac

# Kernels extracted into generated/. Only kernels whose extraction is accepted
# unchanged AND whose output type-checks against the pinned Aeneas library AND
# contains no `axiom` belong here (see formal/aeneas/SPIKE.md for the others).
crate_dir="$repo/cluster-tax"
package="bth-cluster-tax"
name="monetary_kernels"   # -> generated/MonetaryKernels.lean
start_from=(
  "bth_cluster_tax::monetary::MonetaryPolicy::halving_reward"
  "bth_cluster_tax::monetary::MonetaryPolicy::calculate_tail_reward_u128"
)

platform_asset() {
  local os arch
  os="$(uname -s)"; arch="$(uname -m)"
  case "$os/$arch" in
    Linux/x86_64) echo "linux-x86_64" ;;
    Linux/aarch64|Linux/arm64) echo "linux-aarch64" ;;
    Darwin/arm64) echo "macos-aarch64" ;;
    *) echo "unsupported platform $os/$arch" >&2; return 1 ;;
  esac
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

if [[ -z "${AENEAS_DIR:-}" ]]; then
  plat="$(platform_asset)"
  var="AENEAS_SHA256_${plat//-/_}"
  want="${!var}"
  cache="${AENEAS_CACHE:-$HOME/.cache/botho-aeneas}/$AENEAS_TAG/$plat"
  if [[ ! -x "$cache/aeneas" ]]; then
    mkdir -p "$cache"
    tarball="$cache.tar.gz"
    url="https://github.com/AeneasVerif/aeneas/releases/download/$AENEAS_TAG/aeneas-$plat.tar.gz"
    echo "== downloading $url"
    curl -sSfL "$url" -o "$tarball"
    got="$(sha256_of "$tarball")"
    if [[ "$got" != "$want" ]]; then
      echo "FAIL: sha256 mismatch for $url: got $got, want $want" >&2
      rm -f "$tarball"; exit 1
    fi
    tar xzf "$tarball" -C "$cache"
    rm -f "$tarball"
  fi
  AENEAS_DIR="$cache"
fi

aeneas="$AENEAS_DIR/aeneas"
charon="$AENEAS_DIR/charon"
for b in "$aeneas" "$charon"; do
  [[ -x "$b" ]] || { echo "FAIL: $b not found or not executable" >&2; exit 1; }
done

got_version="$("$aeneas" -version)"
if [[ "$got_version" != "aeneas $AENEAS_TAG" ]]; then
  echo "FAIL: $aeneas reports '$got_version', toolchain.env pins $AENEAS_TAG" >&2
  exit 1
fi
got_nightly="$("$charon" toolchain-version)"
if [[ "$got_nightly" != "$CHARON_RUST_NIGHTLY" ]]; then
  echo "FAIL: charon wants $got_nightly, toolchain.env pins $CHARON_RUST_NIGHTLY" >&2
  exit 1
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

args=()
for p in "${start_from[@]}"; do args+=(--start-from "$p"); done

echo "== charon ($package)"
(
  cd "$crate_dir"
  CARGO_TARGET_DIR="${CHARON_TARGET_DIR:-$repo/target/aeneas-charon}" \
    "$charon" cargo --preset aeneas "${args[@]}" \
      --dest-file "$work/$name.llbc" -- -p "$package" --lib
)

echo "== aeneas (lean backend)"
mkdir -p "$work/out"
"$aeneas" -backend lean -no-progress-bar -warnings-as-errors \
  -dest "$work/out" "$work/$name.llbc"

# The checked-in set must stay axiom-free: an `axiom` here is an unmodelled
# Rust std function or type, about which nothing can be proven. The check is
# the package's source scan in --generated mode (#1821). It matches `axiom`,
# `opaque`, `sorry`, `unsafe`, `partial` and friends as tokens anywhere outside
# comments, so an indented or attribute-prefixed `@[...] axiom` is caught too,
# not only `axiom` at the start of a line.
if ! python3 -I "$here/scripts/gate_scan.py" --generated "$work/out" >/dev/null; then
  echo "FAIL: extraction contains axiom/opaque/sorry (see above); it does not belong in generated/" >&2
  exit 1
fi

dest="$here/generated"
if [[ $check -eq 1 ]]; then
  if ! diff -ru "$dest" "$work/out"; then
    echo "FAIL: formal-aeneas/generated/ is out of date; run formal-aeneas/regen.sh" >&2
    exit 1
  fi
  echo "OK: formal-aeneas/generated/ matches a fresh extraction"
else
  rm -rf "$dest"
  mkdir -p "$dest"
  cp -R "$work/out/." "$dest/"
  echo "wrote $(cd "$dest" && find . -type f | sort | tr '\n' ' ')"
fi
