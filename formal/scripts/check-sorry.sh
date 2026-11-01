#!/usr/bin/env bash
# Sorry gate for the Botho formal model (#1705, hardened in #1724 and #1770).
#
# Fails (exit 1) if:
#   (a) `lake build` fails or warns "declaration uses `sorry`" (any quote
#       style, so the check survives Lean changing its message);
#   (b) the sweep finds a declaration of the Botho library that reaches
#       `sorryAx`. AxiomReport.lean walks the compiled declarations directly,
#       so this holds even when the warning in (a) was suppressed (for example
#       by `#guard_msgs in theorem ... := by stop ...`); or
#   (c) the source scan (gate_scan.py, shared with the axiom gate) rejects
#       the tree: a layout other than Botho.lean + Botho/**/*.lean +
#       Theorems.lean, an import of anything but a scanned Botho module (no
#       `import Lean`, `Std`, `Lake`, ...), a proof escape (`sorry`,
#       `sorryAx`, `admit`, `native_decide`), a command or attribute that runs
#       code at build or import time (`#eval`, `initialize`, `@[init]`,
#       `@[implemented_by]`, ...), a metaprogramming command, or
#       `debug.skipKernelTC`. See gate_scan.py for the full lists.
#
# Before anything else the gate checks the build configuration
# (gate-common.sh): lakefile.toml keeps the pinned build layout, so Lake
# compiles exactly the scanned files, and uses only allow-listed keys (no
# [[require]], no code-loading keys), and lake-manifest.json lists no
# packages.
#
# The trusted verify job (verify-artifacts.sh, #1799) runs the base branch's
# copy of this script with --no-build over a pull request's sources, as data:
# then only the build-configuration check and the scan run, and no `lake`
# call is made.
#
# The scan runs first, before `lake build`, and a failed scan stops the gate
# before any model code runs. Code that runs during the build could rewrite
# the sources or the compiled output; the import allow-list is what keeps
# such code out of the model (see "Limits" in formal/README.md). The whole
# script is one function, so bash parses it completely before running
# anything.
#
# `native_decide` trusts the compiler and adds compiler-trust axioms (such as
# `Lean.ofReduceBool`).
# It is allowed on one line only when that line carries the marker comment
# `-- formal-gate: allow native_decide`. The axiom gate still has to accept the
# resulting axiom, so the marker alone is not enough to pass CI.
#
# Usage: check-sorry.sh [--no-build] [--no-scan] [--no-sweep] [FORMAL_ROOT]
#   FORMAL_ROOT defaults to the directory above this script.
#   --no-build  skip (a) and (b); scan the source only.
#   --no-scan   skip (c).
#   --no-sweep  skip (b).
#   The self-test uses these to prove each check works on its own.
# Environment: LAKE overrides the lake binary (default: lake).
set -euo pipefail

main() {
build=1
scan=1
sweep=1
while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-build) build=0; sweep=0; shift ;;
    --no-scan) scan=0; shift ;;
    --no-sweep) sweep=0; shift ;;
    *) break ;;
  esac
done
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="${1:-$(cd "$here/.." && pwd)}"
root="$(cd "$root" && pwd)"
lake="${LAKE:-lake}"
# The library the gates check: `Botho` and every `Botho.*` module.
gate_lib=Botho
# shellcheck disable=SC1091  # gate-common.sh sits next to this script
. "$here/gate-common.sh"
fail=0
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
gate_scanned=0

lakefile_preflight

# (c) runs first, before any `lake` call, so code that runs during the build
# cannot edit what is scanned; a failed scan stops here, before the build.
if [[ "$scan" == 1 ]]; then
  echo "== sorry gate: source scan ($root)"
  if ! source_scan; then
    exit 1
  fi
fi

if [[ "$build" == 1 ]]; then
  echo "== sorry gate: lake build ($root)"
  log="$tmp/build.log"
  if ! (cd "$root" && "$lake" build) >"$log" 2>&1; then
    cat "$log"
    echo "FAIL: lake build failed" >&2
    exit 1
  fi
  cat "$log"
  if grep -Eq "declaration uses ['\`\"]?sorry" "$log"; then
    echo "FAIL: lake build reported a declaration that uses 'sorry'" >&2
    fail=1
  fi
fi

if [[ "$sweep" == 1 ]]; then
  echo "== sorry gate: sweep $gate_lib for sorryAx (AxiomReport.lean)"
  run_report "$tmp/report"
  if grep -q $'^SORRY\t' "$tmp/report"; then
    grep $'^SORRY\t' "$tmp/report" | cut -f2 | sed 's/^/  reaches sorryAx: /' >&2
    echo "FAIL: a declaration in $gate_lib reaches sorryAx (see above)" >&2
    fail=1
  fi
fi

if [[ "$fail" != 0 ]]; then
  exit 1
fi
if [[ "$scan" == 1 ]]; then
  echo "OK: sorry gate passed ($gate_scanned files scanned)"
else
  echo "OK: sorry gate passed (no source scan)"
fi
}

main "$@"
