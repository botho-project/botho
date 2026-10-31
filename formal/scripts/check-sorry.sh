#!/usr/bin/env bash
# Sorry gate for the Botho formal model (#1705).
#
# Fails (exit 1) if:
#   (a) `lake build` fails or warns "declaration uses `sorry`" (any quote
#       style, so the check survives Lean changing its message), or
#   (b) a source scan of Botho.lean, Botho/**/*.lean and Theorems.lean finds
#       the token `sorry`, `sorryAx`, `admit` or `native_decide` outside
#       comments and string literals.
#
# `native_decide` trusts the compiler and adds compiler-trust axioms (such as
# `Lean.ofReduceBool`).
# It is allowed on one line only when that line carries the marker comment
# `-- formal-gate: allow native_decide`. The axiom gate still has to accept the
# resulting axiom, so the marker alone is not enough to pass CI.
#
# Usage: check-sorry.sh [--no-build | --no-scan] [FORMAL_ROOT]
#   FORMAL_ROOT defaults to the directory above this script.
#   --no-build  skip step (a); scan the source only.
#   --no-scan   skip step (b); check the build log only (used by the
#               self-test to prove step (a) works on its own).
# Environment: LAKE overrides the lake binary (default: lake).
set -euo pipefail

build=1
scan=1
case "${1:-}" in
  --no-build) build=0; shift ;;
  --no-scan) scan=0; shift ;;
esac
root="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
root="$(cd "$root" && pwd)"
lake="${LAKE:-lake}"
fail=0

if [[ "$build" == 1 ]]; then
  echo "== sorry gate: lake build ($root)"
  log="$(mktemp)"
  trap 'rm -f "$log"' EXIT
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

if [[ "$scan" == 0 ]]; then
  if [[ "$fail" != 0 ]]; then exit 1; fi
  echo "OK: sorry gate passed (build log only)"
  exit 0
fi

echo "== sorry gate: source scan ($root)"
files=()
[[ -f "$root/Botho.lean" ]] && files+=("$root/Botho.lean")
[[ -f "$root/Theorems.lean" ]] && files+=("$root/Theorems.lean")
if [[ -d "$root/Botho" ]]; then
  while IFS= read -r f; do files+=("$f"); done \
    < <(find "$root/Botho" -name '*.lean' -type f | LC_ALL=C sort)
fi
if [[ ${#files[@]} -eq 0 ]]; then
  echo "FAIL: no Lean sources found under $root" >&2
  exit 1
fi

if ! python3 - "${files[@]}" <<'PY'
import re
import sys

FORBIDDEN = ("sorry", "sorryAx", "admit", "native_decide")
ALLOW_MARKER = "-- formal-gate: allow native_decide"
IDENT = re.compile(r"[A-Za-z_][A-Za-z0-9_'!?.]*")


def strip(src):
    """Replace comments and string literals with spaces, keeping newlines so
    line numbers survive. Handles nested /- -/ block comments."""
    out = []
    i, n, depth = 0, len(src), 0
    while i < n:
        c = src[i]
        two = src[i:i + 2]
        if depth > 0:
            if two == "/-":
                depth += 1
                out.append("  ")
                i += 2
            elif two == "-/":
                depth -= 1
                out.append("  ")
                i += 2
            else:
                out.append("\n" if c == "\n" else " ")
                i += 1
        elif two == "/-":
            depth = 1
            out.append("  ")
            i += 2
        elif two == "--":
            while i < n and src[i] != "\n":
                out.append(" ")
                i += 1
        elif c == '"':
            out.append(" ")
            i += 1
            while i < n and src[i] != '"':
                if src[i] == "\\" and i + 1 < n:
                    out.append("  ")
                    i += 2
                    continue
                out.append("\n" if src[i] == "\n" else " ")
                i += 1
            out.append(" ")
            i += 1
        else:
            out.append(c)
            i += 1
    return "".join(out)


bad = 0
for path in sys.argv[1:]:
    with open(path, encoding="utf-8") as fh:
        raw = fh.read()
    raw_lines = raw.splitlines()
    for lineno, line in enumerate(strip(raw).splitlines(), 1):
        for tok in IDENT.findall(line):
            if tok not in FORBIDDEN:
                continue
            original = raw_lines[lineno - 1] if lineno <= len(raw_lines) else ""
            if tok == "native_decide" and ALLOW_MARKER in original:
                continue
            print(f"{path}:{lineno}: forbidden token '{tok}': {original.strip()}")
            bad = 1
sys.exit(bad)
PY
then
  echo "FAIL: forbidden proof-escape token in source (see above)" >&2
  fail=1
fi

if [[ "$fail" != 0 ]]; then
  exit 1
fi
echo "OK: sorry gate passed (${#files[@]} files scanned)"
