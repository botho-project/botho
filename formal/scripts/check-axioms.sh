#!/usr/bin/env bash
# Axiom gate for the Botho formal model (#1705).
#
# Compiles Theorems.lean (after `lake build`) and parses every
# `#print axioms` result. Fails (exit 1) if:
#   - Theorems.lean does not compile;
#   - it contains zero `#print axioms` lines (silently-empty manifest);
#   - the number of parsed results differs from the number of
#     `#print axioms` lines;
#   - any listed theorem depends on `sorryAx`, or on any axiom not named in
#     axioms-allowlist.txt.
#
# Usage: check-axioms.sh [FORMAL_ROOT]
#   FORMAL_ROOT defaults to the directory above this script.
# Environment: LAKE overrides the lake binary (default: lake).
set -euo pipefail

root="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
root="$(cd "$root" && pwd)"
lake="${LAKE:-lake}"
manifest="$root/Theorems.lean"
allowlist="$root/axioms-allowlist.txt"

for f in "$manifest" "$allowlist"; do
  if [[ ! -f "$f" ]]; then
    echo "FAIL: missing $f" >&2
    exit 1
  fi
done

out="$(mktemp)"
trap 'rm -f "$out"' EXIT

echo "== axiom gate: lake build ($root)"
if ! (cd "$root" && "$lake" build) >"$out" 2>&1; then
  cat "$out"
  echo "FAIL: lake build failed" >&2
  exit 1
fi

echo "== axiom gate: lake env lean Theorems.lean"
if ! (cd "$root" && "$lake" env lean Theorems.lean) >"$out" 2>&1; then
  cat "$out"
  echo "FAIL: Theorems.lean does not compile" >&2
  exit 1
fi
cat "$out"

python3 - "$manifest" "$allowlist" "$out" <<'PY'
import re
import sys

manifest, allowlist, output = sys.argv[1:]

with open(manifest, encoding="utf-8") as fh:
    expected = sum(1 for line in fh if re.match(r"^\s*#print\s+axioms\b", line))

allowed = set()
with open(allowlist, encoding="utf-8") as fh:
    for line in fh:
        name = line.split("#", 1)[0].strip()
        if name:
            allowed.add(name)

with open(output, encoding="utf-8") as fh:
    text = fh.read()

results = {}
# Anchor on Lean's full result-line format rather than on the quotes alone:
#   'NAME' depends on axioms: [A, B, ...]
#   'NAME' does not depend on any axioms
# NAME may itself end in (or contain) `'` -- primed names such as
# `transfer_preserves'` are idiomatic Lean -- so the name is matched lazily up
# to the first `' depends on axioms: [` / `' does not depend on any axioms`
# on the same line. Long axiom lists wrap across lines, so the list itself is
# matched with DOTALL.
RESULT = re.compile(
    r"^'([^\n]+?)' (?:depends on axioms: \[(.*?)\]|does not depend on any axioms)",
    re.M | re.S,
)
for m in RESULT.finditer(text):
    listed = m.group(2)
    if listed is None:
        results[m.group(1)] = []
        continue
    axioms = [a.strip() for a in listed.replace("\n", " ").split(",") if a.strip()]
    results[m.group(1)] = axioms

errors = []
if expected == 0:
    errors.append("Theorems.lean has no '#print axioms' lines (empty manifest)")
if len(results) != expected:
    errors.append(
        f"expected {expected} '#print axioms' results, parsed {len(results)}"
    )
for thm, axioms in sorted(results.items()):
    for ax in axioms:
        if ax == "sorryAx":
            errors.append(f"{thm} depends on sorryAx (a sorry is reachable)")
        elif ax not in allowed:
            errors.append(f"{thm} depends on non-allow-listed axiom {ax}")

if errors:
    for e in errors:
        print(f"FAIL: {e}", file=sys.stderr)
    sys.exit(1)
print(f"OK: axiom gate passed ({len(results)} theorems; allow-list: {', '.join(sorted(allowed))})")
PY
