#!/usr/bin/env bash
# sorry / axiom gate for the formal-aeneas package (#1821).
#
# The package is the Aeneas extraction (generated/, lean_libs MonetaryKernels
# and PoolAccounting)
# plus proofs about it (BothoAeneas/, lean_lib BothoAeneas). It depends on the
# Aeneas Lean library and Mathlib, so it is kept out of formal/ and its gates.
# This gate mirrors formal/scripts/ in shape, scaled to what the package needs:
#
#   0. Source scan (gate_scan.py): lakefile layout, import allow-list, and no
#      `sorry` / `admit` / `axiom` / `opaque` / `unsafe` / `partial` / native
#      evaluation / metaprogramming in any package source, matched as tokens
#      anywhere on a line (so `@[simp] axiom` and indented forms are caught).
#   1. `lake build` of all three libraries.
#   2. Axiom walk (AxiomReport.lean): imports the compiled package with
#      environment extensions off and walks declaration bodies. Every package
#      declaration (generated and proofs alike, listed or not) may reach only
#      the axioms in axioms-allowlist.txt, and none may reach `sorryAx`,
#      declare an `axiom` or `opaque`, or be `unsafe` / `partial`. Every
#      theorem listed in Theorems.lean must exist.
#   3. `#print axioms` cross-check: Theorems.lean (only `import` and
#      `#print axioms NAME` lines) is compiled with `lean --json`; it must
#      produce exactly one `#print axioms` result per line, with no errors or
#      warnings, and each result must equal the walk's.
#
# The allow-list, the manifest and the report program are read before the
# first `lake` call, so code that runs during the build cannot rewrite them.
#
# Trust: unlike formal/ (#1799), there is no separate trusted verify job. This
# gate runs in the same job as the build, against a dependency (Aeneas and
# Mathlib) whose own lakefile is a Lean program. It is a correctness check
# for honest contributors, and it is not a required check; see
# formal-aeneas/README.md.
#
# Usage: check-gates.sh [--no-scan] [ROOT]
#   ROOT defaults to the directory above this script.
#   --no-scan  skip step 0. Only selftest-gates.sh passes it, to show that
#              steps 2 and 3 catch defects on their own. CI never passes it.
# Environment: LAKE overrides the lake binary (default: lake).
set -euo pipefail

main() {
scan=1
if [[ "${1:-}" == "--no-scan" ]]; then scan=0; shift; fi
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "${1:-$here/..}" && pwd)"
lake="${LAKE:-lake}"

for f in "$root/Theorems.lean" "$root/axioms-allowlist.txt" "$here/AxiomReport.lean" \
         "$here/gate_scan.py"; do
  [[ -f "$f" ]] || { echo "FAIL: missing $f" >&2; exit 1; }
done
manifest_src="$(cat "$root/Theorems.lean")"
allowlist_src="$(cat "$root/axioms-allowlist.txt")"
report_src="$(cat "$here/AxiomReport.lean")"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

if [[ "$scan" == 1 ]]; then
  echo "== formal-aeneas gate: source scan"
  python3 -I "$here/gate_scan.py" "$root"
fi

echo "== formal-aeneas gate: manifest syntax"
if ! GATE_MANIFEST="$manifest_src" python3 -I - >"$tmp/commands" <<'PY'
import os
import re
import sys

src = os.environ["GATE_MANIFEST"]
# Only line comments are accepted in the manifest, so they can be dropped
# without a full lexer.
if "/-" in src:
    print("FAIL: Theorems.lean: block comments are not allowed", file=sys.stderr)
    sys.exit(1)
IMPORT = re.compile(r"import\s+(BothoAeneas(\.[A-Za-z_][A-Za-z0-9_]*)*|MonetaryKernels|PoolAccounting)\s*\Z")
PRINT = re.compile(r"#print\s+axioms\s+([A-Za-z_][A-Za-z0-9_'.]*)\s*\Z")
errors, seen, rows, imports_done = [], set(), [], False
for lineno, raw in enumerate(src.split("\n"), 1):
    body = raw.split("--", 1)[0].rstrip()
    if not body.strip():
        continue
    m = PRINT.fullmatch(body)
    if m:
        imports_done = True
        name = m.group(1)
        if name in seen:
            errors.append(f"line {lineno}: {name} listed twice")
        seen.add(name)
        rows.append(f"{lineno}\t{name}")
        continue
    if not imports_done and IMPORT.fullmatch(body):
        continue
    errors.append(f"line {lineno}: only `import` and `#print axioms NAME` lines allowed: {body[:80]}")
if not rows:
    errors.append("no `#print axioms` lines")
for e in errors:
    print(f"FAIL: Theorems.lean: {e}", file=sys.stderr)
if errors:
    sys.exit(1)
print("\n".join(rows))
PY
then
  exit 1
fi
commands="$(cat "$tmp/commands")"
names=()
while IFS=$'\t' read -r _ name; do names+=("$name"); done <<<"$commands"

echo "== formal-aeneas gate: lake build"
if ! (cd "$root" && "$lake" build) >"$tmp/build" 2>&1; then
  cat "$tmp/build"
  echo "FAIL: lake build failed" >&2
  exit 1
fi
# `lake build` succeeds on a `sorry` (it is a warning); the walk below is
# what rejects it. Show warnings for the log.
grep -E "warning|error" "$tmp/build" || true

echo "== formal-aeneas gate: axiom walk (AxiomReport.lean)"
printf '%s\n' "$report_src" >"$tmp/AxiomReport.lean"
if ! (cd "$root" && "$lake" env lean --run "$tmp/AxiomReport.lean" "$tmp/report" "${names[@]}") \
    >"$tmp/out" 2>&1; then
  cat "$tmp/out"
  echo "FAIL: AxiomReport.lean did not run" >&2
  exit 1
fi

echo "== formal-aeneas gate: #print axioms (Theorems.lean, from the snapshot)"
# A compile error (for example an unknown name) shows up as an error message
# in the JSON, which the check below reports together with the walk's
# findings; the exit status is checked there too.
lean_status=0
json="$(cd "$root" && "$lake" env lean --json --stdin <<<"$manifest_src" 2>&1)" || lean_status=$?

GATE_LEAN_STATUS="$lean_status" GATE_COMMANDS="$commands" GATE_JSON="$json" GATE_REPORT="$(cat "$tmp/report")" \
  GATE_ALLOWLIST="$allowlist_src" python3 -I - <<'PY'
import json
import os
import re
import sys

commands = {}
order = []
for row in os.environ["GATE_COMMANDS"].splitlines():
    line, name = row.split("\t")
    commands[int(line)] = name
    order.append(name)
allowed = {r.split("#", 1)[0].strip() for r in os.environ["GATE_ALLOWLIST"].splitlines()}
allowed.discard("")
errors = []
if os.environ["GATE_LEAN_STATUS"] != "0":
    errors.append(f"Theorems.lean does not compile (lean exit {os.environ['GATE_LEAN_STATUS']})")

walked, own, done = {}, 0, False
for row in os.environ["GATE_REPORT"].splitlines():
    parts = row.split("\t")
    kind = parts[0]
    if kind == "AXIOMS":
        walked[parts[1]] = sorted(a for a in parts[2].split(",") if a)
    elif kind == "MISSING":
        errors.append(f"{parts[1]}: listed in Theorems.lean but not a declaration")
    elif kind == "OWN":
        own += 1
        bad = [a for a in parts[2].split(",") if a and a not in allowed]
        if bad:
            errors.append(f"{parts[1]} depends on axioms outside the allow-list: {bad}")
    elif kind == "SORRY":
        errors.append(f"{parts[1]} depends on sorryAx")
    elif kind == "AXIOMDECL":
        errors.append(f"{parts[1]}: `axiom` declared in the package")
    elif kind == "OPAQUEDECL":
        errors.append(f"{parts[1]}: `opaque` declared in the package")
    elif kind == "UNSAFE":
        errors.append(f"{parts[1]}: `unsafe` or `partial` definition in the package")
    elif kind == "DONE":
        done = True
if not done or own == 0:
    errors.append("axiom walk did not complete (no DONE line or no declarations)")

RESULT = re.compile(
    r"'(.+?)' (?:depends on axioms: \[(.*)\]|does not depend on any axioms)\s*", re.S)
printed = {}
for raw in os.environ["GATE_JSON"].splitlines():
    if not raw.strip():
        continue
    try:
        msg = json.loads(raw)
        line, severity, data = int(msg["pos"]["line"]), msg["severity"], msg["data"]
    except (ValueError, KeyError, TypeError):
        errors.append(f"lean printed a line that is not a Lean message: {raw[:120]}")
        continue
    if severity != "information":
        errors.append(f"Theorems.lean:{line}: {severity}: {data[:200]}")
        continue
    name = commands.get(line)
    m = RESULT.fullmatch(data)
    if name is None or not m or m.group(1) != name or name in printed:
        errors.append(f"Theorems.lean:{line}: unexpected message {data[:120]!r}")
        continue
    printed[name] = sorted(a.strip() for a in (m.group(2) or "").replace("\n", " ").split(",")
                           if a.strip())

for name in order:
    p, w = printed.get(name), walked.get(name)
    if p is None:
        errors.append(f"no `#print axioms` result for {name}")
    elif w is not None and p != w:
        errors.append(f"{name}: #print axioms says {p}, the walk says {w}")
    if w is not None:
        print(f"  {name}: {', '.join(w) or '(no axioms)'}")

if errors:
    for e in errors:
        print(f"FAIL: {e}", file=sys.stderr)
    sys.exit(1)
print(f"OK: {len(order)} listed theorems and {own} package declarations use only "
      f"allow-listed axioms ({', '.join(sorted(allowed))}); no sorry, axiom or opaque")
PY
}

main "$@"
